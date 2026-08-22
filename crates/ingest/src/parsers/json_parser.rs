//! JSON parser。美化输出 + 校验合法性。

use crate::document::Document;
use crate::error::IngestResult;
use crate::parser::{make_meta, DocumentParser};
use std::path::Path;

pub struct JsonParser;

impl DocumentParser for JsonParser {
    fn format_name(&self) -> &'static str {
        "json"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "json".to_string());
        let meta = make_meta(path, &ext, bytes);
        let raw = std::fs::read_to_string(path)?;

        // JSON Lines / NDJSON：每行一个独立 JSON 值；非法行标出但不中断
        if ext == "jsonl" || ext == "ndjson" {
            let mut out = String::from("```jsonl\n");
            for (i, line) in raw.lines().enumerate() {
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(t) {
                    Ok(v) => {
                        out.push_str(&serde_json::to_string(&v)?);
                        out.push('\n');
                    }
                    Err(e) => {
                        out.push_str(&format!("[第 {} 行非法 JSON: {e}] {}\n", i + 1, t));
                    }
                }
            }
            out.push_str("```");
            return Ok(Document::new_text(out, meta));
        }

        // 解析再美化（顺带校验合法性）
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        let pretty = serde_json::to_string_pretty(&value)?;
        Ok(Document::new_text(format!("```json\n{pretty}\n```"), meta))
    }
}
