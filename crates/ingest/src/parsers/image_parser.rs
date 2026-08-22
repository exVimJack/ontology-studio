//! 图片 parser：image 解码 + base64 → multimodal_part。
//!
//! 一期：解码校验 + 输出 base64 part（交 VLM 理解，决策 7）。
//! 不内嵌本地 OCR/VLM（原则 2 轻量化）。

use crate::document::{Document, MultimodalPart};
use crate::error::IngestResult;
use crate::parser::{make_meta, DocumentParser};
use base64::Engine;
use std::io::Cursor;
use std::path::Path;

pub struct ImageParser;

impl DocumentParser for ImageParser {
    fn format_name(&self) -> &'static str {
        "image"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "png".to_string());
        let mime = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            "gif" => "image/gif",
            _ => "image/png",
        };
        let meta = make_meta(path, &ext, bytes);

        // 解码验证（确保是合法图片；同时可在此做尺寸/重采样，二期）
        let raw = std::fs::read(path)?;
        let _img = image::load(Cursor::new(&raw), image::ImageFormat::from_extension(&ext).unwrap_or(image::ImageFormat::Png))?;

        // base64 编码原始字节（一期直接传原图；二期可按需缩放）
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);

        let mut doc = Document::new_text(
            format!("[图片: {}，{} 字节，等待 VLM 理解]", path.display(), bytes),
            meta,
        );
        doc.multimodal_parts.push(MultimodalPart {
            mime: mime.to_string(),
            data_b64: b64,
        });
        Ok(doc)
    }
}
