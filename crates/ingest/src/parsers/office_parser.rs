//! Office parser：office_oxide 统一处理 DOCX/DOC/PPTX/PPT/XLSX/XLS。
//!
//! office_oxide 提供 extract_text / to_markdown 一行式 API（决策 4）。
//! 兼容性自测见 tests/office_compat.rs。

use crate::document::Document;
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser};
use std::path::Path;

pub struct OfficeParser;

impl OfficeParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OfficeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentParser for OfficeParser {
    fn format_name(&self) -> &'static str {
        "office"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let format = match ext.as_str() {
            "docx" => "docx",
            "doc" => "doc",
            "pptx" => "pptx",
            "ppt" => "ppt",
            "xlsx" => "xlsx",
            "xls" => "xls",
            _ => "office",
        };
        let meta = make_meta(path, format, bytes);

        // 优先 to_markdown（保留标题/列表/表格结构），失败回退 plain_text
        let text = match office_oxide::to_markdown(path) {
            Ok(md) if !md.trim().is_empty() => md,
            Ok(_) => match office_oxide::extract_text(path) {
                Ok(t) => t,
                Err(e) => return Err(IngestError::Office(format!("文档无可提取内容: {e}"))),
            },
            Err(e) => {
                // markdown 失败，尝试纯文本
                match office_oxide::extract_text(path) {
                    Ok(t) if !t.trim().is_empty() => t,
                    Ok(_) => {
                        return Err(IngestError::Office(format!(
                            "文档无可提取内容（to_markdown 失败: {e}）"
                        )))
                    }
                    Err(e2) => {
                        return Err(IngestError::Office(format!(
                            "解析失败（to_markdown: {e}; extract_text: {e2}）"
                        )))
                    }
                }
            }
        };

        Ok(Document::new_text(text, meta))
    }
}
