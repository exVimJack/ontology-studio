//! PDF parser（决策 5：pdfium-render + 预编译 PDFium 动态库）。
//!
//! 主路径：pdfium-render 借用进程级 `Pdfium` 单例（见 `pdfium.rs`），逐页提取文本。
//!
//! 为什么不用纯 Rust（lopdf/pdfsink-rs）：对中文 PDF 的 CIDFontType2 + ToUnicode CMap
//! 解码存在系统性缺陷，实测《曾国藩合集》输出纯乱码/空。pdfium 是 Chrome 内核，
//! 渲染级文本提取，CJK/CID 零乱码，1371 页实测 4.15s。
//!
//! 单文件内逐页串行（保逐页进度 + 可取消）；多文件 batch 级并发在 IPC 层
//! `ingest_files`（JoinSet + 信号量），但 PDFium C 库非线程安全，本 parser 用
//! 进程级锁串行化所有 pdfium 调用（见 `pdfium.rs`），故多文件实际串行执行。
//!
//! 扫描件（无文本层）返回提示，二期走 VLM。

use crate::document::Document;
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser, ParseProgress};
use crate::pdfium::{pdfium, with_pdfium};
use std::path::Path;

pub struct PdfParser;

impl DocumentParser for PdfParser {
    fn format_name(&self) -> &'static str {
        "pdf"
    }

    fn parse_with_progress(&self, path: &Path, progress: &dyn ParseProgress) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let meta = make_meta(path, "pdf", bytes);

        tracing::info!(?path, bytes, "pdf: opening with pdfium");

        // PDFium C 库非线程安全（见 pdfium.rs 模块文档），整个 load→extract→drop
        // 临界区必须持进程级锁，否则并发调用导致内存损坏（FormatError/崩溃）。
        // guard 在函数返回（含提前 return/Err）时 drop 释放锁。
        //
        // batch 多个 PDF 并发时，这里会阻塞等锁。phase 先报"等待解析（排队）"，
        // 拿到锁后再报"打开 PDF"——让前端能区分"排队等锁"与"正在打开"，
        // 避免用户误以为多个 PDF 在同时解析（实际严格串行）。
        progress.on_phase("等待解析（排队）");
        let _guard = with_pdfium()?;

        progress.on_phase("打开 PDF");

        let pdfium = pdfium()?;
        let doc = pdfium
            .load_pdf_from_file(path, None)
            .map_err(|e| IngestError::Pdf(format!("打开 PDF 失败（pdfium: {e}）")))?;

        let total = doc.pages().len() as usize;
        tracing::info!(?path, total, "pdf: loaded, extracting text");

        progress.on_phase("提取 PDF 文本");
        let mut out = String::new();
        let mut empty_pages = 0usize;

        for i in 0..total {
            // cooperative 取消检查点
            if progress.is_cancelled() {
                tracing::info!(?path, page = i, "pdf: cancelled");
                return Err(IngestError::Cancelled);
            }

            match doc.pages().get(i as i32) {
                Ok(page) => match page.text() {
                    Ok(text) => {
                        let s = text.all();
                        if s.trim().is_empty() {
                            empty_pages += 1;
                        }
                        progress.on_chars(s.len());
                        out.push_str(&s);
                        out.push_str("\n\n");
                    }
                    Err(e) => {
                        tracing::warn!(?path, page = i, err = %e, "pdf: page text failed");
                    }
                },
                Err(e) => {
                    tracing::warn!(?path, page = i, err = %e, "pdf: page get failed");
                }
            }
            progress.on_progress(i + 1, Some(total));
            if i % 50 == 0 && i > 0 {
                tracing::info!(?path, page = i + 1, total, out_len = out.len(), "pdf: progress");
            }
        }

        tracing::info!(?path, out_len = out.len(), empty_pages, "pdf: extraction done");

        if out.trim().is_empty() {
            return Err(IngestError::Pdf(format!(
                "PDF 无可提取文本（{total} 页均为空，疑似扫描件/纯图片 PDF，当前版本暂不支持 OCR）"
            )));
        }

        Ok(Document::new_text(out, meta))
    }
}
