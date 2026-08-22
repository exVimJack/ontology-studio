//! Document：统一摄取输出结构（§四 ingest/）。
//!
//! 一期聚焦文本提取；`tables` / `multimodal_parts` 为二期预留但已建模，
//! 让 parser 实现可渐进增强（如 XLSX 一期即可输出表格）。

use serde::{Deserialize, Serialize};

/// 单个文档的统一产物。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// 文本内容（一期主要产物；Markdown 优先，纯文本次之）。
    pub text: String,
    /// 文档元信息。
    pub meta: DocumentMeta,
    /// 结构化表格（XLSX / PDF 表格；一期 XLSX 输出）。
    #[serde(default)]
    pub tables: Vec<Table>,
    /// 多模态 part（图片 base64；一期图片输入走此，交 VLM）。
    #[serde(default)]
    pub multimodal_parts: Vec<MultimodalPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentMeta {
    pub source_path: String,
    /// 推断的格式名（pdf/docx/xlsx/epub/png/...）。
    pub format: String,
    /// 估算字符数（用于配额/截断）。
    pub char_count: usize,
    /// 源文件字节数。
    pub source_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub name: String,
    /// 行优先；每行单元格的字符串表示。
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalPart {
    /// MIME 类型，如 "image/png"。
    pub mime: String,
    /// base64 编码的原始字节。
    pub data_b64: String,
}

impl Document {
    pub fn new_text(text: String, meta: DocumentMeta) -> Self {
        let char_count = text.chars().count();
        let meta = DocumentMeta { char_count, ..meta };
        Self {
            text,
            meta,
            tables: Vec::new(),
            multimodal_parts: Vec::new(),
        }
    }
}
