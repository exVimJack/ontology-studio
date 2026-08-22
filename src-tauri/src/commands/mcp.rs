//! MCP server 配置与连接命令（二期 A3）。
//!
//! 配置经 tauri-plugin-store 持久化（同 provider）。设置页调用 set_mcp_servers
//! 传入完整 server 列表，命令重建 McpManager 并连接所有 server，工具注册到
//! 共享 tool_handle；同时把 tool_handle 注入 ChatService（若已配置 provider）。
//!
//! 业务逻辑在 crates/agent-core 的 McpManager，此处只做 IPC 薄封装（§六）。

use agent_core::{McpManager, McpServerConfig};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, State};
use tauri_plugin_store::StoreExt;
use tracing::{info, warn};

use super::error::{AppError, AppResult};
use crate::state::AppState;

const STORE_FILE: &str = "settings.json";
const KEY_MCP_SERVERS: &str = "mcp_servers";

/// MCP server 连接结果（单个 server 的状态）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct McpServerStatus {
    /// 配置中的 server 名（前端展示用）。
    pub name: String,
    /// 是否连接成功。
    pub connected: bool,
    /// 注册的工具数量（连接成功时）。
    #[specta(type = specta_typescript::Number)]
    pub tool_count: usize,
    /// 失败原因（连接失败时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 配置并连接 MCP server（整体替换）。
///
/// 传入完整 server 列表，命令：
///   1. 持久化配置到 store
///   2. drop 旧 McpManager（断开旧连接）
///   3. 新建 McpManager 连接所有 server，工具注册到共享 tool_handle
///   4. 把 tool_handle 注入 ChatService（若已配置）
///
/// 返回每个 server 的连接状态。单个失败不影响其他。
#[tauri::command]
#[specta::specta]
pub async fn set_mcp_servers(
    app: AppHandle,
    state: State<'_, AppState>,
    servers: Vec<McpServerConfig>,
) -> AppResult<Vec<McpServerStatus>> {
    // 1. 持久化配置
    let store = app
        .store(STORE_FILE)
        .map_err(|e| AppError::Memory(format!("open store: {e}")))?;
    let json = serde_json::to_value(&servers)
        .map_err(|e| AppError::Memory(format!("serialize mcp servers: {e}")))?;
    store.set(KEY_MCP_SERVERS, json);
    store
        .save()
        .map_err(|e| AppError::Memory(format!("save store: {e}")))?;

    // 2. drop 旧 McpManager（断开旧连接，旧工具随 handle 清空需重建 handle）
    //    注意：旧工具仍注册在共享 tool_handle 上，需要换新 handle 避免脏工具。
    //    但 tool_handle 被 ChatService 持有引用，无法替换——改为：旧 manager drop
    //    断开连接，旧 McpTool 的 peer 失效（调用会失败）。一期接受此局限，
    //    后续若需精确清理，ToolServerHandle 支持 remove_tool（按名删）。
    let mut mcp_guard = state.mcp.write().await;
    drop(mcp_guard.take()); // drop 旧 manager

    // 3. 新建 McpManager 连接
    let mut manager = McpManager::new(state.tool_handle.clone());
    let errors = manager.connect_all(&servers).await;

    // 4. 构造状态返回
    let mut statuses: Vec<McpServerStatus> = Vec::new();
    for cfg in &servers {
        let err = errors.iter().find(|(name, _)| name == cfg.name());
        if let Some((_, e)) = err {
            statuses.push(McpServerStatus {
                name: cfg.name().to_string(),
                connected: false,
                tool_count: 0,
                error: Some(e.to_string()),
            });
        } else {
            // 成功连接的 server，工具数需从 handle 查（一期简化：不精确到每个 server，
            // 用总数 / 成功数估算无意义，故 tool_count 留 0，前端用 list_mcp_tools 查总数）
            statuses.push(McpServerStatus {
                name: cfg.name().to_string(),
                connected: true,
                tool_count: 0,
                error: None,
            });
        }
    }

    *mcp_guard = Some(manager);

    // 5. 注入 tool_handle 到 ChatService（若已配置）
    let mut chat_guard = state.chat.write().await;
    if let Some(chat) = chat_guard.as_mut() {
        chat.set_tool_handle(Some(state.tool_handle.clone()));
    }
    drop(chat_guard);
    drop(mcp_guard);

    let connected = statuses.iter().filter(|s| s.connected).count();
    info!(
        total = servers.len(),
        connected,
        failed = errors.len(),
        "MCP servers configured"
    );

    Ok(statuses)
}

/// 读取已持久化的 MCP server 配置（不含连接状态，启动恢复用）。
#[tauri::command]
#[specta::specta]
pub async fn get_mcp_servers(app: AppHandle) -> AppResult<Vec<McpServerConfig>> {
    let store = app
        .store(STORE_FILE)
        .map_err(|e| AppError::Memory(format!("open store: {e}")))?;
    match store.get(KEY_MCP_SERVERS) {
        Some(json) => serde_json::from_value::<Vec<McpServerConfig>>(json)
            .map_err(|e| AppError::Memory(format!("deserialize mcp servers: {e}"))),
        None => Ok(Vec::new()),
    }
}

/// 列出当前 tool_handle 上注册的所有工具（已连接 MCP server 暴露的工具）。
#[tauri::command]
#[specta::specta]
pub async fn list_mcp_tools(state: State<'_, AppState>) -> AppResult<Vec<McpToolDef>> {
    let defs = state
        .tool_handle
        .get_tool_defs(None)
        .await
        .map_err(|e| AppError::Provider(format!("list tools: {e}")))?;
    Ok(defs
        .into_iter()
        .map(|d| McpToolDef {
            name: d.name,
            description: d.description,
            parameters: d.parameters.to_string(),
        })
        .collect())
}

/// 工具定义（前端展示用）。parameters 为 JSON Schema 字符串（serde_json::Value 不实现 specta::Type）。
#[derive(Debug, Clone, Serialize, Type)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema（序列化字符串，前端 JSON.parse 后用）
    pub parameters: String,
}

/// 启动时从 store 恢复 MCP server 连接。
/// 失败不阻断启动（仅 warn），用户可在设置页重试。
/// 内部通过 app.state() 取 AppState（需在 app.manage 之后调用）。
pub async fn restore_mcp_servers(app: AppHandle) {
    restore_mcp_servers_inner(app).await;
}

async fn restore_mcp_servers_inner(app: AppHandle) {
    use tauri::Manager;
    let store = match app.store(STORE_FILE) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "restore mcp: open store failed");
            return;
        }
    };
    let json = match store.get(KEY_MCP_SERVERS) {
        Some(j) => j,
        None => return,
    };
    let servers: Vec<McpServerConfig> = match serde_json::from_value(json) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "restore mcp: deserialize failed");
            return;
        }
    };
    if servers.is_empty() {
        return;
    }

    // 通过 app.state() 取 AppState
    let state: tauri::State<'_, AppState> = app.state();
    let mut manager = McpManager::new(state.tool_handle.clone());
    let errors = manager.connect_all(&servers).await;
    for (name, e) in &errors {
        warn!(server = name, error = %e, "restore mcp: connect failed");
    }
    let connected = manager.connected_count();
    *state.mcp.write().await = Some(manager);

    // 注入 ChatService（若已恢复 provider）
    let mut chat_guard = state.chat.write().await;
    if let Some(chat) = chat_guard.as_mut() {
        chat.set_tool_handle(Some(state.tool_handle.clone()));
    }

    info!(total = servers.len(), connected, "MCP servers restored");
}
