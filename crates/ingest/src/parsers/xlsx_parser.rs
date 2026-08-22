//! XLSX 结构化表格 parser（calamine）。
//!
//! 注：一期 dispatcher 把 xlsx 交给 office_oxide（统一路径）。
//! 此 parser 保留供二期「表格保真」切换：保留单元格类型/合并/公式语义。
//! 当 dispatcher 决定走 calamine 时输出 tables 而非纯 text。

use crate::document::{Document, Table};
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser};
use calamine::{open_workbook, Data, Reader, Xlsx};
use std::path::Path;

pub struct XlsxCalamineParser;

impl DocumentParser for XlsxCalamineParser {
    fn format_name(&self) -> &'static str {
        "xlsx"
    }

    fn parse(&self, path: &Path) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let meta = make_meta(path, "xlsx", bytes);

        let mut workbook: Xlsx<_> = open_workbook(path)
            .map_err(|e| IngestError::Xlsx(format!("打开工作簿失败: {e:?}")))?;

        let sheets = workbook.worksheets();
        let mut tables = Vec::with_capacity(sheets.len());
        let mut text = String::new();

        for (name, range) in sheets {
            let mut rows = Vec::new();
            for row in range.rows() {
                let cells: Vec<String> = row.iter().map(cell_to_string).collect();
                rows.push(cells);
            }
            // 同时拼成文本（Markdown 表格）
            text.push_str(&format!("## {name}\n\n"));
            if !rows.is_empty() {
                // header
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
                text.push('\n');
            }
            tables.push(Table { name, rows });
        }

        let mut doc = Document::new_text(text, meta);
        doc.tables = tables;
        Ok(doc)
    }
}

fn cell_to_string(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            // 整数浮点不显示小数点
            if f.fract() == 0.0 && f.is_finite() {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        Data::String(s) => s.clone(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => format!("{dt}"),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#ERR:{e:?}"),
    }
}
