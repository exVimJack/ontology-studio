//! Provider 配置命令（见 ARCHITECTURE.md §20.9 设置页）。
//!
//! 一期：单 provider。配置经 tauri-plugin-store 持久化到磁盘（Rust 侧），
//! 敏感 API Key 一期暂明文存（二期落 SQLite 加密，§20.9）。
//! 配置变更后重建 ChatService 并写入 AppState。

use agent_core::{
    ChatService, InputType, ProviderConfig, ProviderKind, ReasoningLevel, invalidate_cache,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_store::StoreExt;
use tracing::info;

use super::error::{AppError, AppResult};
use crate::state::AppState;

const STORE_FILE: &str = "settings.json";
const KEY_PROVIDER: &str = "provider";

/// 设置页输入的 provider 配置（API Key 明文传，Rust 侧落盘）。
///
/// 字段对齐 `ProviderConfig`，一期一期二期逐步补全。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetProviderInput {
    // ── 连接 ──
    pub kind: ProviderKind,
    pub api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    pub model: String,
    // ── 采样（二期补全） ──
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[specta(type = specta_typescript::Number)]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub top_p: Option<f64>,
    // ── 兼容性（二期补全） ──
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_developer_role: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(default = "default_input_types")]
    pub input_types: Vec<InputType>,
    // ── 增强 ──
    /// 模型上下文窗口（token 数），可选。None 时自动探测。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[specta(type = specta_typescript::Number)]
    pub context_window: Option<u64>,
    /// 系统人设（preamble），可选。None = 无系统人设（仅 skill 段作 preamble）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub preamble: Option<String>,
    /// 自定义 HTTP Header（代理、X-Title、anthropic_betas 等）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
    /// 深度思考级别（运行时 Composer toggle 会覆盖为 High/Off）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<ReasoningLevel>,
}

fn default_input_types() -> Vec<InputType> {
    vec![InputType::Text]
}

#[tauri::command]
#[specta::specta]
pub async fn set_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    input: SetProviderInput,
) -> AppResult<ProviderConfig> {
    let config = ProviderConfig {
        kind: input.kind,
        api_key: input.api_key,
        base_url: input.base_url,
        model: input.model,
        temperature: input.temperature,
        max_tokens: input.max_tokens,
        top_p: input.top_p,
        supports_developer_role: input.supports_developer_role,
        supports_reasoning_effort: input.supports_reasoning_effort,
        input_types: input.input_types,
        context_window: input.context_window,
        preamble: input.preamble,
        extra_headers: input.extra_headers,
        reasoning: input.reasoning,
    };

    // 1. 构造 ChatService（验证配置有效，如 api_key 非空）
    let mut chat = ChatService::new(config.clone()).map_err(|e| {
        AppError::Provider(format!("无法创建对话服务：{e}"))
    })?;

    // 2. 持久化到 store（明文 JSON，一期）
    let store = app.store(STORE_FILE).map_err(|e| {
        AppError::Memory(format!("open store: {e}"))
    })?;
    let json = serde_json::to_value(&config)
        .map_err(|e| AppError::Memory(format!("serialize provider: {e}")))?;
    store.set(KEY_PROVIDER, json);
    store.save().map_err(|e| AppError::Memory(format!("save store: {e}")))?;

    // 3. 配置变更，清 context_window 探测缓存（base_url/model 可能变了）
    invalidate_cache();

    // 4. 注入 memory backend（带 CompactingMemory 自动压缩）。
    //    解析真实 context_window（五层 fallback），探测有超时保护不阻断。
    let context_window = agent_core::resolve_context_window(&config).await as usize;
    chat.set_memory(state.memory.clone(), context_window);

    // 4.5 注入联邦查询服务（三期阶段 1c）。若已初始化，把 Arc<FederationService> 挂上。
    //     同时注入本体存储（三期：本体建模作为 agent 工具，始终可用）。
    if let Some(svc) = state.federation.read().await.as_ref() {
        chat.set_federation(std::sync::Arc::new(svc.clone()));
    }
    chat.set_ontology_store(state.ontology_store.clone());

    // 4.6 注入 Skill 管理器（决策 20）：preamble Tier 1 + active skill doc_paths。
    chat.set_skill_manager(std::sync::Arc::clone(&state.skill_manager));

    // 5. 更新 AppState
    *state.chat.write().await = Some(chat);
    *state.provider_config.write().await = Some(config.clone());

    info!(kind = config.kind.as_str(), model = %config.model, "provider configured");
    Ok(config)
}

#[tauri::command]
#[specta::specta]
pub async fn get_provider(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Option<ProviderConfig>> {
    // 优先返回内存中已激活的（含运行时验证）
    if let Some(c) = state.provider_config.read().await.clone() {
        return Ok(Some(c));
    }
    // 否则从 store 读（启动时）
    let store = app
        .store(STORE_FILE)
        .map_err(|e| AppError::Memory(format!("open store: {e}")))?;
    let Some(val) = store.get(KEY_PROVIDER) else {
        return Ok(None);
    };
    let config: ProviderConfig = serde_json::from_value(val)
        .map_err(|e| AppError::Memory(format!("deserialize provider: {e}")))?;
    Ok(Some(config))
}

/// 启动时调用：从 store 恢复 provider 配置并重建 ChatService。
pub fn restore_provider(app: &AppHandle, state: &AppState) -> AppResult<()> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| AppError::Memory(format!("open store: {e}")))?;
    let Some(val) = store.get(KEY_PROVIDER) else {
        return Ok(());
    };
    let config: ProviderConfig = serde_json::from_value(val)
        .map_err(|e| AppError::Memory(format!("deserialize provider: {e}")))?;

    match ChatService::new(config.clone()) {
        Ok(mut chat) => {
            // 启动期无法 async 探测 /v1/models，用同步解析：
            // 用户配置 → 内置已知模型表（DeepSeek V4 1M 等）→ 保守默认 100K。
            // 后续用户重设 provider 时会调 resolve_context_window 探测真实窗口重建。
            chat.set_memory(
                state.memory.clone(),
                agent_core::resolve_known_or_default(&config),
            );
            // 注入 Skill 管理器（决策 20）：preamble Tier 1 + active skill doc_paths。
            // 启动期同步注入（skill_manager 已在 setup hook 构造）。
            chat.set_skill_manager(std::sync::Arc::clone(&state.skill_manager));
            // 注入本体存储（三期：本体建模作为 agent 工具，始终可用）。
            chat.set_ontology_store(state.ontology_store.clone());
            // 启动期无并发，用 try_write 同步写入 tokio::sync::RwLock（无需 runtime 句柄）
            if let Ok(mut g) = state.chat.try_write() {
                *g = Some(chat);
            }
            if let Ok(mut g) = state.provider_config.try_write() {
                *g = Some(config.clone());
            }
            info!(kind = config.kind.as_str(), model = %config.model, "provider restored");
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to restore provider, ignored");
        }
    }
    Ok(())
}
