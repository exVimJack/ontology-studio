//! 会话与消息相关命令（见 ARCHITECTURE.md §13 IPC 契约）。
//!
//! 纯薄封装：调用 memory crate，不含业务逻辑（AGENTS.md 工程结构硬约束）。
//! 例外：`generate_conversation_title` 调 agent-core 的 LLM 概括能力生成标题，
//! 但它本身仍是薄编排（取首条消息 → 调 LLM → 写回标题），不含业务规则。

use memory::{ConversationRow, ConversationSummary, MessageRole, MessageRow, MessageStatus};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use tracing::info;

use super::error::{AppError, AppResult};
use crate::state::AppState;

// ── 会话 ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CreateConversationInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn create_conversation(
    state: State<'_, AppState>,
    input: CreateConversationInput,
) -> AppResult<ConversationRow> {
    let row = state
        .memory
        .create_conversation(input.title.as_deref())?;
    Ok(row)
}

#[tauri::command]
#[specta::specta]
pub async fn list_conversations(
    state: State<'_, AppState>,
) -> AppResult<Vec<ConversationSummary>> {
    Ok(state.memory.list_conversations()?)
}

#[tauri::command]
#[specta::specta]
pub async fn rename_conversation(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> AppResult<ConversationRow> {
    Ok(state.memory.rename_conversation(&id, &title)?)
}

/// 自动生成会话标题（LLM 概括首条用户消息）。
///
/// 触发时机：前端在首条 AI 回复结束后调用，且仅当当前标题仍是默认 “新会话”
/// （避免覆盖用户手动改名）。
///
/// 流程：
///   1. 取该会话首条 user 消息文本
///   2. 调 `ChatService::generate_title`（同 provider 轻量 LLM 补全）
///   3. 写回 `rename_conversation`
///
/// 失败时返回 Err，前端降级为截断式兜底（deriveTitle）。
/// 标题为空或仍为默认值时返回 Err，提示前端走兜底。
#[tauri::command]
#[specta::specta]
pub async fn generate_conversation_title(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<ConversationRow> {
    // 取首条 user 消息
    let messages = state.memory.list_messages(&id)?;
    let first_user = messages
        .iter()
        .find(|m| matches!(m.role, MessageRole::User) && !m.content.is_empty())
        .ok_or_else(|| AppError::Agent("无可用的首条用户消息生成标题".into()))?;

    // 调 LLM 生成标题
    let chat_guard = state.chat.read().await;
    let chat = chat_guard
        .as_ref()
        .ok_or_else(|| AppError::Provider("未配置模型提供商，无法生成标题".into()))?;
    let title = chat.generate_title(&first_user.content).await?;
    drop(chat_guard);

    // 空标题降级（模型异常输出）
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::Agent("LLM 返回空标题".into()));
    }

    info!(conv_id = %id, title = %title, "auto-generated conversation title");
    Ok(state.memory.rename_conversation(&id, title)?)
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetPinnedInput {
    pub id: String,
    pub pinned: bool,
}

#[tauri::command]
#[specta::specta]
pub async fn set_conversation_pinned(
    state: State<'_, AppState>,
    input: SetPinnedInput,
) -> AppResult<()> {
    state.memory.set_pinned(&input.id, input.pinned)?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_conversation(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<()> {
    state.memory.delete_conversation(&id)?;
    Ok(())
}

// ── 消息 ───────────────────────────────────────────────────────

#[tauri::command]
#[specta::specta]
pub async fn list_messages(
    state: State<'_, AppState>,
    conversation_id: String,
    // 返回条数上限（None=全部，Some=取最后 N 条）。默认建议 50。
    // u32 而非 usize：specta 禁止导出 64 位整数到 TS（AGENTS.md BigInt 公约），
    // 值域 < 2^32 足够消息条数，调用处 `as usize` 转换。
    limit: Option<u32>,
) -> AppResult<Vec<MessageRow>> {
    Ok(state.memory.list_messages_limited(&conversation_id, limit.map(|n| n as usize))?)
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DeleteMessageInput {
    pub message_id: String,
}

#[tauri::command]
#[specta::specta]
pub async fn delete_message(
    state: State<'_, AppState>,
    input: DeleteMessageInput,
) -> AppResult<()> {
    state.memory.delete_message(&input.message_id)?;
    Ok(())
}

/// 删除指定消息及其之后的全部消息（用于重新生成 / 编辑重发）。
/// 返回被删除的消息条数（含目标消息自身）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DeleteMessageAndAfterInput {
    pub message_id: String,
}

#[tauri::command]
#[specta::specta]
pub async fn delete_message_and_after(
    state: State<'_, AppState>,
    input: DeleteMessageAndAfterInput,
) -> AppResult<u32> {
    let affected = state.memory.delete_message_and_after(&input.message_id)?;
    Ok(affected as u32)
}

/// 用于将消息标记为错误/取消的辅助命令（前端重试/中断后收尾用）。
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetMessageStatusInput {
    pub message_id: String,
    pub status: MessageStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn set_message_status(
    state: State<'_, AppState>,
    input: SetMessageStatusInput,
) -> AppResult<()> {
    state
        .memory
        .set_message_status(&input.message_id, input.status, input.error.as_deref())?;
    Ok(())
}
