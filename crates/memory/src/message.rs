//! 消息数据模型。
//!
//! 与前端 `src/lib/domain/message.ts` 一一对应（由 tauri-specta 生成 TS 绑定，
//! 见 ARCHITECTURE.md 决策 F5）。一期只建模纯文本对话；多模态 part（图片/附件）
//! 一期 MVP 先支持「图片输入」（决策 7 一期），结构在 agent-core 侧组装为 Rig Message。

use serde::{Deserialize, Serialize};

use crate::timestamp::Timestamp;
use specta_typescript::Number;

/// 消息角色。对应 Rig `Message` 的三个变体（rig::completion::message::Message）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// 消息状态机（见 ARCHITECTURE.md §十五 对话状态机）。
///
/// streaming → complete | error | cancelled
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum MessageStatus {
    /// 流式产出中
    Streaming,
    /// 正常完成
    Complete,
    /// 出错（附 error 字段）
    Error,
    /// 用户中断（已产出内容保留为 partial）
    Cancelled,
}

impl MessageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "streaming" => Some(Self::Streaming),
            "complete" => Some(Self::Complete),
            "error" => Some(Self::Error),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// 消息行（DB 行 ↔ 前端 DTO）。
///
/// `content` 为正文（纯文本/Markdown，流式增量累积）。
/// `reasoning` 为 reasoning 模型的思考链（独立于正文，前端单独渲染为可折叠块）。
/// 多模态 part、tool call、citation 等结构属二期。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct MessageRow {
    pub id: String,
    pub conversation_id: String,
    pub role: MessageRole,
    pub status: MessageStatus,
    pub content: String,
    /// reasoning 模型的思考链（仅 assistant，可空）。独立于 content，前端单独展示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// status=error 时的错误信息，可空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// assistant 消息记录所用模型，可空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// 二期 B1：provider 报告的输入 token（含历史+本轮 prompt）。可空（未报告/旧消息）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Number)]
    pub prompt_tokens: Option<u64>,
    /// 二期 B1：provider 报告的输出 token。可空。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Number)]
    pub completion_tokens: Option<u64>,
    /// 二期 B1：provider 报告的总 token。可空。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Number)]
    pub total_tokens: Option<u64>,
}
