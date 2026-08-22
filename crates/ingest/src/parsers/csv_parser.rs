//! CSV/TSV parser。输出 Markdown 表格 + 保留 tables 结构。

use crate::document::{Document, Table};
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser};
use std::io::Read;
use std::path::Path;

pub struct CsvParser;

impl DocumentParser for CsvParser {
    fn format_name(&self) -> &'static str {
        "csv"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "csv".to_string());
        let meta = make_meta(path, &ext, bytes);

        let delim = if ext == "tsv" { b'\t' } else { b',' };
        let mut content = String::new();
        std::fs::File::open(path)?.read_to_string(&mut content)?;

        let mut rdr = csv::ReaderBuilder::new()
            .delimiter(delim)
            .has_headers(false)
            .flexible(true)
            .from_reader(content.as_bytes());

        let mut rows: Vec<Vec<String>> = Vec::new();
        for record in rdr.records() {
            let record = record.map_err(|e| IngestError::Csv(e.to_string()))?;
            rows.push(record.iter().map(|s| s.to_string()).collect());
        }

        // Markdown 表格
        let mut text = String::new();
        if !rows.is_empty() {
            text.push_str("| ");
            text.push_str(&rows[0].join(" | "));
            text.push_str(" |\n| ");
            text.push_str(&vec!["---"; rows[0].len()].join(" | "));
            text.push_str(" |\n");
            for r in &rows[1..] {
                text.push_str("| ");
                text.push_str(&r.join(" | "));
                text.push_str(" |\n");
            }
        }

        let mut doc = Document::new_text(text, meta);
        doc.tables.push(Table {
            name: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("sheet")
                .to_string(),
            rows,
        });
        Ok(doc)
    }
}
