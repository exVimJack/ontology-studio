//! 统一错误枚举（决策 8）。所有 parser 的错误归一为 `IngestError`。

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("不支持的文件格式: {ext}（路径: {path}）")]
    UnsupportedFormat { ext: String, path: PathBuf },

    #[error("文件不存在或不可读: {0}")]
    Io(#[from] std::io::Error),

    #[error("PDF 解析失败: {0}")]
    Pdf(String),

    #[error("Office 文档解析失败: {0}")]
    Office(String),

    #[error("XLSX 解析失败: {0}")]
    Xlsx(String),

    #[error("ePub 解析失败: {0}")]
    Epub(String),

    #[error("图片解码失败: {0}")]
    Image(#[from] image::ImageError),

    #[error("CSV 解析失败: {0}")]
    Csv(String),

    #[error("JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("压缩包解析失败: {0}")]
    Archive(String),

    /// 安全限制触发（zip 炸弹 / 解压深度 / 文件数 / 总大小）
    #[error("安全限制触发: {0}")]
    SecurityViolation(String),

    #[error("文件过大: {actual_bytes} 字节（上限 {limit_bytes}）")]
    FileTooLarge {
        actual_bytes: u64,
        limit_bytes: u64,
    },

    /// 用户取消（cooperative cancellation）
    #[error("已取消")]
    Cancelled,
}

pub type IngestResult<T> = Result<T, IngestError>;
