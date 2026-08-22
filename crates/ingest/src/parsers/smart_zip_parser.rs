//! SmartZipParser：对 PK\x03\x04 头的文件（docx/xlsx/epub/zip 都是 zip 容器），
//! 依次尝试 office_oxide → epub → zip，取首个成功者。
//!
//! 用于内容嗅探路径（无扩展名时）。

use crate::document::Document;
use crate::error::IngestResult;
use crate::parser::{DocumentParser, ParseProgress};
use std::path::Path;

pub struct SmartZipParser;

impl SmartZipParser {
    pub fn new() -> Self {
        Self
    }
}

impl DocumentParser for SmartZipParser {
    fn format_name(&self) -> &'static str {
        "zip-container"
    }

    fn parse_with_progress(&self, path: &Path, progress: &dyn ParseProgress) -> IngestResult<Document> {
        // 1. office_oxide（docx/xlsx/pptx/doc/xls/ppt）
        progress.on_phase("尝试 Office 容器");
        let office = crate::parsers::OfficeParser::new();
        if let Ok(doc) = office.parse_with_progress(path, progress) {
            if !doc.text.trim().is_empty() {
                return Ok(doc);
            }
        }
        // 2. epub
        progress.on_phase("尝试 EPUB 容器");
        let epub = crate::parsers::EpubParser;
        if let Ok(doc) = epub.parse_with_progress(path, progress) {
            if !doc.text.trim().is_empty() {
                return Ok(doc);
            }
        }
        // 3. 普通 zip
        progress.on_phase("解压普通 zip");
        crate::parsers::ZipParser.parse_with_progress(path, progress)
    }
}

impl Default for SmartZipParser {
    fn default() -> Self {
        Self::new()
    }
}
