//! AppError：统一错误枚举（见 ARCHITECTURE.md §13.4）。
//!
//! 前端按变体做差异化 UX：Provider(401) → 设置页引导；Cancelled → 静默；其他 → toast + 重试。

use serde::Serialize;
use specta::Type;
use thiserror::Error;

#[derive(Debug, Error, Serialize, Type)]
#[serde(tag = "kind", content = "message")]
pub enum AppError {
    #[error("ingest: {0}")]
    Ingest(String),

    #[error("agent: {0}")]
    Agent(String),

    #[error("memory: {0}")]
    Memory(String),

    #[error("provider: {0}")]
    Provider(String),

    #[error("federation: {0}")]
    Federation(String),

    #[error("ontology: {0}")]
    Ontology(String),

    #[error("skill: {0}")]
    Skill(String),

    #[error("skill-scan-timeout: {0}")]
    SkillScanTimeout(String),

    /// 用户主动中断（§15 状态机 Cancelled）——前端静默处理
    #[error("cancelled")]
    Cancelled,
}

// ── 从各子 crate 错误转换 ──────────────────────────────────────

impl From<memory::MemoryError> for AppError {
    fn from(e: memory::MemoryError) -> Self {
        AppError::Memory(e.to_string())
    }
}

impl From<agent_core::AgentError> for AppError {
    fn from(e: agent_core::AgentError) -> Self {
        match e {
            agent_core::AgentError::Cancelled => AppError::Cancelled,
            other => AppError::Agent(other.to_string()),
        }
    }
}

impl From<ingest::IngestError> for AppError {
    fn from(e: ingest::IngestError) -> Self {
        AppError::Ingest(e.to_string())
    }
}

impl From<federation::FederationError> for AppError {
    fn from(e: federation::FederationError) -> Self {
        AppError::Federation(e.to_string())
    }
}

impl From<ontology_store::StoreError> for AppError {
    fn from(e: ontology_store::StoreError) -> Self {
        AppError::Ontology(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
