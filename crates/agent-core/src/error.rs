//! 统一错误枚举（见 ARCHITECTURE.md §13.4 AppError 的 Agent 子项）。

use thiserror::Error;

pub type AgentResult<T> = Result<T, AgentError>;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("provider: {0}")]
    Provider(String),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("stream: {0}")]
    Stream(String),

    /// 用户主动中断（§15 状态机 Cancelled）
    #[error("cancelled")]
    Cancelled,

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("mcp: {0}")]
    Mcp(String),
}
