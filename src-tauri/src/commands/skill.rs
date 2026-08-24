//! Skill 系统命令（决策 20）。
//!
//! 仅做 `#[tauri::command]` 薄封装，业务逻辑全在 `agent_core::skill`。
//! 类型经 tauri-specta 生成 TS 绑定（决策 F5）。

use agent_core::skill::SkillSource;
use serde::{Deserialize, Serialize};
use tauri::State;

use super::error::{AppError, AppResult};
use crate::state::AppState;

/// 前端展示用的 Skill DTO。
///
/// 合并了扫描结果（SkillRecord）+ 激活状态（全局禁用/会话级 enabled）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub disable_model_invocation: bool,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    /// 本会话是否激活（会话级 enabled 状态；None=无会话级记录，按 source 默认行为）。
    /// 前端据此显示开关初始态。
    pub conversation_enabled: Option<bool>,
    /// 是否全局禁用（层次 2）。
    pub globally_disabled: bool,
}

/// 列出全部已发现的 skill，合并激活状态。
///
/// `conversation_id` 为 None 时只返回全局状态（conversation_enabled 全 None）。
#[tauri::command]
#[specta::specta]
pub async fn list_skills(
    state: State<'_, AppState>,
    conversation_id: Option<String>,
) -> AppResult<Vec<SkillDto>> {
    let t = std::time::Instant::now();
    tracing::info!(
        conversation_id = ?conversation_id,
        "list_skills invoked"
    );
    let mgr = &state.skill_manager;
    let records = mgr.discover_all();
    let disabled: std::collections::HashSet<String> = mgr.memory()
        .list_disabled_skills()
        .map_err(|e| AppError::Memory(e.to_string()))?
        .into_iter()
        .collect();
    let conv_skills = match &conversation_id {
        Some(cid) => mgr.memory()
            .list_conversation_skills(cid)
            .map_err(|e| AppError::Memory(e.to_string()))?,
        None => vec![],
    };

    let mut out = Vec::with_capacity(records.len());
    for r in records {
        let globally_disabled = disabled.contains(&r.name);
        let conv_entry = conv_skills.iter().find(|cs| cs.skill_name == r.name);
        let conversation_enabled = conv_entry.map(|cs| cs.enabled);
        out.push(SkillDto {
            name: r.name,
            description: r.description,
            source: r.source,
            disable_model_invocation: r.disable_model_invocation,
            license: r.license,
            compatibility: r.compatibility,
            allowed_tools: r.allowed_tools,
            conversation_enabled,
            globally_disabled,
        });
    }
    tracing::info!(
        elapsed_ms = t.elapsed().as_millis() as u64,
        count = out.len(),
        "list_skills done"
    );
    Ok(out)
}

/// 导入本地 skill 目录（复制到 ~/.onto-studio/skills/<name>/）。
#[tauri::command]
#[specta::specta]
pub async fn import_skill_from_dir(
    state: State<'_, AppState>,
    src_path: String,
) -> AppResult<String> {
    let path = std::path::Path::new(&src_path);
    state
        .skill_manager
        .import_from_dir(path)
        .map_err(|e| AppError::Memory(e.to_string()))
}

/// 导入 zip skill（解压 + 校验 + 复制）。
#[tauri::command]
#[specta::specta]
pub async fn import_skill_from_zip(
    state: State<'_, AppState>,
    zip_path: String,
) -> AppResult<String> {
    let path = std::path::Path::new(&zip_path);
    state
        .skill_manager
        .import_from_zip(path)
        .map_err(|e| AppError::Memory(e.to_string()))
}

/// 卸载导入的 skill（仅 imported 可卸载）。
#[tauri::command]
#[specta::specta]
pub async fn uninstall_skill(
    state: State<'_, AppState>,
    skill_name: String,
) -> AppResult<()> {
    state
        .skill_manager
        .uninstall(&skill_name)
        .map_err(|e| AppError::Memory(e.to_string()))
}

/// 设置会话级 skill enabled 状态（层次 3）。
///
/// enabled=true：Builtin/External 保持进 preamble，Imported 激活进 preamble；
/// enabled=false：Builtin/External 显式排除，Imported 不激活。
#[tauri::command]
#[specta::specta]
pub async fn set_skill_conversation_enabled(
    state: State<'_, AppState>,
    conversation_id: String,
    skill_name: String,
    source: SkillSource,
    enabled: bool,
) -> AppResult<()> {
    state
        .skill_manager
        .memory()
        .set_conversation_skill_enabled(&conversation_id, &skill_name, source.as_str(), enabled)
        .map_err(|e| AppError::Memory(e.to_string()))
}

/// 设置全局 skill 禁用状态（层次 2）。
#[tauri::command]
#[specta::specta]
pub async fn set_skill_globally_disabled(
    state: State<'_, AppState>,
    skill_name: String,
    disabled: bool,
) -> AppResult<()> {
    state
        .skill_manager
        .memory()
        .set_skill_globally_disabled(&skill_name, disabled)
        .map_err(|e| AppError::Memory(e.to_string()))
}
