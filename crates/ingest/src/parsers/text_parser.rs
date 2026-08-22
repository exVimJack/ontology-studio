//! 纯文本/代码/Markdown parser。直接读为 UTF-8。

use crate::document::Document;
use crate::error::IngestResult;
use crate::parser::{make_meta, DocumentParser};
use std::path::Path;

pub struct TextParser;

impl DocumentParser for TextParser {
    fn format_name(&self) -> &'static str {
        "text"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "txt".to_string());
        let meta = make_meta(path, &ext, bytes);
        let text = std::fs::read_to_string(path)?;
        Ok(Document::new_text(text, meta))
    }
}
