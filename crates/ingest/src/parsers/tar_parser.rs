//! TAR parser（含 .tar.gz；bz2/xz 一期不启用）。
//! 同 ZIP：流式 + 安全预算 + 递归深度限制。

use crate::document::Document;
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser, ParseProgress};
use crate::security::{ArchiveBudget, MAX_ARCHIVE_DEPTH, MAX_ARCHIVE_ENTRY_BYTES};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct TarParser;

impl DocumentParser for TarParser {
    fn format_name(&self) -> &'static str {
        "tar"
    }

    fn parse_with_progress(&self, path: &Path, progress: &dyn ParseProgress) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let meta = make_meta(path, "tar", bytes);

        let name_lower = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        progress.on_phase("打开 tar");
        let file = std::fs::File::open(path)?;
        let decoder: Box<dyn Read> = if name_lower.ends_with(".gz") || name_lower.ends_with(".tgz")
        {
            Box::new(flate2::read::GzDecoder::new(file))
        } else if name_lower.ends_with(".bz2") || name_lower.ends_with(".tbz2") {
            return Err(IngestError::Archive(
                "bz2 解压一期未启用，请用 .tar.gz 或纯 .tar".to_string(),
            ));
        } else if name_lower.ends_with(".xz") || name_lower.ends_with(".txz") {
            return Err(IngestError::Archive(
                "xz 解压一期未启用，请用 .tar.gz 或纯 .tar".to_string(),
            ));
        } else {
            Box::new(file)
        };

        let mut archive = tar::Archive::new(decoder);
        let mut budget = ArchiveBudget::new();
        let mut text = String::new();

        let entries = archive
            .entries()
            .map_err(|e| IngestError::Archive(format!("读取 tar 失败: {e}")))?;
        for (i, entry) in entries.enumerate() {
            if progress.is_cancelled() {
                return Err(IngestError::Cancelled);
            }
            // tar 是流式，无法预知总数，传 None 为不确定态
            progress.on_progress(i + 1, None);
            let mut entry =
                entry.map_err(|e| IngestError::Archive(format!("条目读取失败: {e}")))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let name = entry
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| format!("entry-{i}"));

            let tmp_path: PathBuf = std::env::temp_dir().join(format!("onto-studio-tar-{i}"));
            let written = {
                let mut out = std::fs::File::create(&tmp_path).map_err(IngestError::from)?;
                let mut buf = [0u8; 64 * 1024];
                let mut n_bytes: u64 = 0;
                loop {
                    let n = entry.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])?;
                    n_bytes += n as u64;
                    if n_bytes > MAX_ARCHIVE_ENTRY_BYTES {
                        let _ = std::fs::remove_file(&tmp_path);
                        return Err(IngestError::SecurityViolation(format!(
                            "单条目过大: {n_bytes} 字节"
                        )));
                    }
                }
                n_bytes
            };
            budget.account(written)?;

            text.push_str(&format!("\n--- {name} ---\n"));
            ingest_recursive(&tmp_path, &mut budget, 0, &mut text);
            let _ = std::fs::remove_file(&tmp_path);
        }

        Ok(Document::new_text(text, meta))
    }
}

fn ingest_recursive(tmp_path: &Path, budget: &mut ArchiveBudget, depth: usize, out: &mut String) {
    if depth >= MAX_ARCHIVE_DEPTH {
        out.push_str("[达到最大递归深度，跳过]\n");
        return;
    }
    match crate::ingest_file(tmp_path) {
        Ok(doc) => {
            let text = if doc.text.len() > 50_000 {
                format!(
                    "{}…[截断，共 {} 字符]",
                    &doc.text[..50_000],
                    doc.text.len()
                )
            } else {
                doc.text
            };
            out.push_str(&text);
            out.push('\n');
            let _ = budget.account(doc.meta.source_bytes);
        }
        Err(IngestError::UnsupportedFormat { .. }) => {
            out.push_str("[二进制/不支持的格式，跳过]\n");
        }
        Err(e) => {
            out.push_str(&format!("[解析失败: {e}]\n"));
        }
    }
}
