//! agent-core: Rig 驱动的 agent loop + MCP 工具系统 + 多模态理解
//!
//! 平台无关的业务核心（见 ARCHITECTURE.md §四 / 决策 1）。
//!
//! 一期 MVP（§九）：
//!   - OpenAI 兼容 provider 接入（含 OpenAI / DeepSeek / Ollama / OpenRouter）
//!   - 流式对话（StreamChunk 统一输出，对齐 §13.2）
//!   - 图片输入（决策 7 一期：UserContent::Image）
//!
//! 二期：MCP 工具系统（rmcp）、RAG、tool call 卡片。

pub mod chat;
pub mod context_budget;
pub mod context_window;
pub mod document_tools;
pub mod error;
pub mod federation_tools;
pub mod mcp;
pub mod memory_bridge;
pub mod ontology_tools;
pub mod ontology_ttl_tools;
pub mod provider;
pub mod skill;

pub use chat::{
    split_last_as_prompt, text_history_to_messages, text_prompt,
    multimodal_prompt, ChatService, ContextImage, StreamChunk, StreamKind,
    TokenUsage, ToolCallInfo,
};
pub use context_budget::{
    estimate_message_tokens, estimate_messages_tokens, estimate_tokens, trim_history,
    trim_history_rows, estimate_row_tokens, BudgetConfig, TrimmedHistory, TrimmedRows,
    DEFAULT_KEEP_RECENT_TURNS, DEFAULT_MAX_CONTEXT_TOKENS, SUMMARY_TOKEN_BUDGET,
};
// re-export rig 消息类型，供上层（src-tauri）不直接依赖 rig
pub use rig::completion::message::{Message, UserContent};
pub use error::{AgentError, AgentResult};
pub use mcp::{McpManager, McpServerConfig};
pub use memory_bridge::{build_compacting_memory, LlmCompactor, SqliteMemory, SummaryArtifact, SummaryFn};
pub use provider::{InputType, ProviderConfig, ProviderKind, ReasoningLevel, reasoning_to_params};
pub use skill::{SkillManager, SkillRecord, SkillSource};
// re-export rig 工具服务类型，供上层（src-tauri）不直接依赖 rig
pub use rig::tool::server::{ToolServer, ToolServerHandle};
// re-export 上下文窗口解析（二期 B1）
pub use context_window::{
    resolve_context_window, resolve_known_or_default, invalidate_cache, DEFAULT_CONTEXT_WINDOW,
};
