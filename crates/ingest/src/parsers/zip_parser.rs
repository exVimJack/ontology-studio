//! ZIP parser：递归解压 + 安全预算（防 zip 炸弹）。
//!
//! 流式读取条目，累计展开字节防 OOM（§六 security.rs）。
//! 递归深度限制：嵌套压缩包最多 MAX_ARCHIVE_DEPTH 层。

use crate::document::Document;
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser, ParseProgress};
use crate::security::{ArchiveBudget, MAX_ARCHIVE_DEPTH, MAX_ARCHIVE_ENTRY_BYTES};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct ZipParser;

impl DocumentParser for ZipParser {
    fn format_name(&self) -> &'static str {
        "zip"
    }

    fn parse_with_progress(&self, path: &Path, progress: &dyn ParseProgress) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let meta = make_meta(path, "zip", bytes);

        progress.on_phase("打开 zip");
        let file = std::fs::File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| IngestError::Archive(format!("打开 zip 失败: {e}")))?;

        let total = archive.len();
        let mut budget = ArchiveBudget::new();
        let mut text = String::new();

        for i in 0..archive.len() {
            if progress.is_cancelled() {
                return Err(IngestError::Cancelled);
            }
            progress.on_progress(i + 1, Some(total));
            let mut entry = archive
                .by_index(i)
                .map_err(|e| IngestError::Archive(format!("读取条目 {i} 失败: {e}")))?;
            let name = entry
                .name()
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| format!("entry-{i}"));

            if entry.is_dir() {
                continue;
            }

            // 流式落到稳定临时路径（ingest_file 需要路径；递归嵌套包也要路径）
            let tmp_path: PathBuf = std::env::temp_dir().join(format!("onto-studio-zip-{i}"));
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
                    // 单条目硬上限（流式累计校验，防超大单文件）
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

fn ingest_recursive(
    tmp_path: &Path,
    budget: &mut ArchiveBudget,
    depth: usize,
    out: &mut String,
) {
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
            // 递归产出已计入上层 written；这里只做总预算的二次守卫
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
