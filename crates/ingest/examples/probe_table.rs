//! 表格坐标探针：dump 指定页（或含关键字的页）的字符级坐标，验证列对齐可行性。

use pdfium_render::prelude::*;
use std::env;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pdf = env::args().nth(1).expect("usage: probe <pdf> <keyword>");
    let keyword = env::args().nth(2).expect("need keyword to locate page");
    let lib = env::var("ONTO_PDFIUM_LIB").expect("ONTO_PDFIUM_LIB");
    let bindings = Pdfium::bind_to_library(&PathBuf::from(lib))?;
    let pdfium = Pdfium::new(bindings);
    let doc = pdfium.load_pdf_from_file(Path::new(&pdf), None)?;
    let total = doc.pages().len();

    // 找含 keyword 的页
    let mut target: Option<i32> = None;
    for i in 0..total {
        let t = doc.pages().get(i)?.text()?.all();
        if t.contains(&keyword) {
            target = Some(i);
            break;
        }
    }
    let pi = target.ok_or("keyword not found")?;
    println!("=== 找到关键字「{keyword}」在第 {} 页（0-indexed）===", pi);

    let page = doc.pages().get(pi)?;
    let text = page.text()?;
    // 按字符 dump：x, y(left, bottom), char
    // PDFium 坐标系：原点左下，y 向上。用 bottom 做行聚类键。
    let mut chars: Vec<(f32, f32, f32, char)> = Vec::new();
    for ch in text.chars().iter() {
        let c = match ch.unicode_char() {
            Some(c) => c,
            None => continue,
        };
        if c.is_whitespace() && c != '\u{00a0}' {
            // 保留普通空格用于分隔，但跳过纯空白
        }
        let b = ch.loose_bounds()?;
        chars.push((b.left().value, b.bottom().value, b.right().value, c));
    }
    println!("总字符数：{}", chars.len());

    // 按 bottom 聚类成行（容差 2pt）
    chars.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.partial_cmp(&b.0).unwrap()));
    let mut rows: Vec<Vec<(f32, f32, f32, char)>> = Vec::new();
    let mut cur: Vec<(f32, f32, f32, char)> = Vec::new();
    let mut cur_y = f32::MAX;
    for c in chars {
        if (c.1 - cur_y).abs() > 2.0 {
            if !cur.is_empty() { rows.push(std::mem::take(&mut cur)); }
            cur_y = c.1;
        }
        cur.push(c);
    }
    if !cur.is_empty() { rows.push(cur); }
    println!("聚类行数：{}\n", rows.len());

    // 打印前 25 行，每行：y | 各字符按 x 排列（单元格间用 | 分隔，x 跳变>5pt 视为新单元格）
    for row in rows.iter().take(25) {
        let y = row.first().unwrap().1;
        // 按 x 聚类单元格
        let mut cells: Vec<String> = vec![String::new()];
        let mut last_right = f32::MIN;
        for &(_, _, right, ch) in row {
            if last_right != f32::MIN && (right - last_right).abs() > 6.0 {
                cells.push(String::new());
            }
            cells.last_mut().unwrap().push(ch);
            last_right = right;
        }
        let cells_str: Vec<String> = cells.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        println!("y={:>7.1} | {}", y, cells_str.join(" | "));
    }
    Ok(())
}
