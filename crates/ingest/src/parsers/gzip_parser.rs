//! GZIP parser（单文件 .gz，如 .log.gz / .json.gz）。
//!
//! flate2 已随 tar.gz 引入，零新依赖（决策：gz 复用 flate2::GzDecoder）。
//! gzip 只压缩单个文件：解压落临时文件 → 递归 ingest_file（复用内层 parser）。

use crate::document::Document;
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser};
use crate::security::{ArchiveBudget, MAX_ARCHIVE_ENTRY_BYTES};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct GzipParser;

impl DocumentParser for GzipParser {
    fn format_name(&self) -> &'static str {
        "gzip"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let meta = make_meta(path, "gzip", bytes);

        let file = std::fs::File::open(path)?;
        let mut decoder = flate2::read::GzDecoder::new(file);
        let tmp_path: PathBuf =
            std::env::temp_dir().join(format!("onto-studio-gz-{}", std::process::id()));
        let written = {
            let mut out = std::fs::File::create(&tmp_path).map_err(IngestError::from)?;
            let mut buf = [0u8; 64 * 1024];
            let mut n_bytes: u64 = 0;
            loop {
                let n = decoder.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                n_bytes += n as u64;
                if n_bytes > MAX_ARCHIVE_ENTRY_BYTES {
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(IngestError::SecurityViolation(format!(
                        "解压单条目过大: {n_bytes} 字节"
                    )));
                }
            }
            n_bytes
        };

        let mut budget = ArchiveBudget::new();
        budget.account(written)?;

        // 内层可能无扩展名（.log.gz → 纯文本），靠 sniff 兜底路由
        let inner = crate::ingest_file(&tmp_path)
            .map_err(|e| IngestError::Archive(format!("gzip 解压后内层解析失败: {e}")))?;
        let _ = std::fs::remove_file(&tmp_path);

        let mut doc = Document::new_text(inner.text, meta);
        doc.tables = inner.tables;
        doc.multimodal_parts = inner.multimodal_parts;
        Ok(doc)
    }
}