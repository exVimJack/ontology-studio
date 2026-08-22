//! 错误类型。

use thiserror::Error;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("实体未找到: {0}")]
    NotFound(String),

    #[error("api_name 校验失败: {entity_kind} '{api_name}' 不符合 {pattern}")]
    InvalidApiName {
        entity_kind: &'static str,
        api_name: String,
        pattern: &'static str,
    },

    #[error("data_type 非法: '{0}' 不在 DataType 枚举内")]
    InvalidDataType(String),

    #[error("主键约束违反: ObjectType '{ot_api_name}' 必须有且仅一个 is_primary_key=true 的属性")]
    PrimaryKeyViolation { ot_api_name: String },

    #[error("引用完整性违反: {0}")]
    ReferentialIntegrity(String),

    #[error("字段非法: 实体 '{entity}' 出现未知字段 '{field}'")]
    UnknownField { entity: String, field: String },

    #[error("JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("SQLite 错误: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("其他: {0}")]
    Other(#[from] anyhow::Error),
}
