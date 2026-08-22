//! federation: 统一错误枚举（见 PHASE3-FEDERATION.md §四 error.rs）。
//!
//! 对齐既有 crate 错误范式（memory::MemoryError / ingest::IngestError）：
//! thiserror 派生，单一枚举，Display 友好。

use thiserror::Error;

/// 联邦查询错误。
#[derive(Debug, Error)]
pub enum FederationError {
    /// 数据源连接失败（网络/认证/数据库不存在）
    #[error("数据源连接失败: {0}")]
    Connect(String),

    /// 数据源配置无效（缺字段/格式错）
    #[error("数据源配置无效: {0}")]
    InvalidConfig(String),

    /// SQL 解析失败（sqlparser）
    #[error("SQL 解析失败: {0}")]
    SqlParse(String),

    /// SQL 被只读护栏拦截（非 SELECT/WITH）
    #[error("只读模式，已拦截非查询语句: {0}")]
    ReadonlyViolation(String),

    /// 查询执行失败（DataFusion / sqlx）
    #[error("查询执行失败: {0}")]
    Query(String),

    /// 数据源不存在（未注册或已删除）
    #[error("数据源不存在: {0}")]
    SourceNotFound(String),

    /// CSV/Excel 文件不可访问
    #[error("文件不可访问: {0}")]
    File(String),

    /// 行数超限
    #[error("结果行数超限（最大 {0}）")]
    RowLimitExceeded(usize),

    /// 查询超时
    #[error("查询超时")]
    Timeout,

    /// 底层存储错误（memory SQLite）
    #[error("存储错误: {0}")]
    Storage(String),

    /// Arrow 类型转换失败
    #[error("类型转换失败: {0}")]
    Arrow(String),

    /// 其他（兜底）
    #[error("{0}")]
    Other(String),
}

pub type FederationResult<T> = Result<T, FederationError>;

impl From<sqlx::Error> for FederationError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::Database(db) => FederationError::Query(db.message().to_string()),
            _ => FederationError::Connect(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for FederationError {
    fn from(e: serde_json::Error) -> Self {
        FederationError::Other(format!("serde_json: {e}"))
    }
}

impl From<std::io::Error> for FederationError {
    fn from(e: std::io::Error) -> Self {
        FederationError::File(e.to_string())
    }
}

impl From<datafusion::common::DataFusionError> for FederationError {
    fn from(e: datafusion::common::DataFusionError) -> Self {
        FederationError::Query(e.to_string())
    }
}

impl From<rusqlite::Error> for FederationError {
    fn from(e: rusqlite::Error) -> Self {
        FederationError::Storage(e.to_string())
    }
}

impl From<calamine::Error> for FederationError {
    fn from(e: calamine::Error) -> Self {
        FederationError::File(format!("{e:?}"))
    }
}
