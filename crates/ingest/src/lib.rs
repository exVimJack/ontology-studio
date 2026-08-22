//! ingest: 多模态文件摄取管道 + VLM 增强
//!
//! 架构（见 ARCHITECTURE.md §四 / 决策 8）：
//!   - `DocumentParser` trait 统一接口
//!   - dispatcher 按 MIME/扩展名路由
//!   - 统一错误枚举
//!   - 流式解析防 OOM（security.rs 防 zip 炸弹）
//!
//! 落地路线：一期 PDF/Office/文本/图片/CSV/JSON/压缩包；二期 VLM 增强解析。
//!
//! ## 用法
//! ```no_run
//! use ingest::ingest_file;
//! let doc = ingest_file(std::path::Path::new("report.pdf"))?;
//! println!("{} chars from {}", doc.text.len(), doc.meta.format);
//! # Ok::<(), ingest::IngestError>(())
//! ```

pub mod dispatcher;
pub mod document;
pub mod error;
pub mod parser;
pub mod parsers;
pub mod pdfium;
pub mod security;

pub use document::{Document, DocumentMeta, MultimodalPart, Table};
pub use error::{IngestError, IngestResult};
pub use parser::{ingest_file, ingest_file_with_progress, DocumentParser, NoProgress, ParseProgress, ProgressRef};
pub use pdfium::{init_pdfium, pdfium, with_pdfium};

pub const CRATE_NAME: &str = "ingest";
