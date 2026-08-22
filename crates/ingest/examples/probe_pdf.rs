//! 临时 PDF 解析探针：用已落地的 ingest PdfParser 解析指定 PDF，
//! 输出字符数、前 3 页预览、文本落盘。验证用，非项目正式代码。

use ingest::parser::{ingest_file_with_progress, NoProgress};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pdf = std::env::args()
        .nth(1)
        .expect("usage: probe <pdf-path>");
    let lib = std::env::var("ONTO_PDFIUM_LIB").expect("需设 ONTO_PDFIUM_LIB");
    ingest::init_pdfium(&PathBuf::from(lib))?;

    let t = Instant::now();
    let doc = ingest_file_with_progress(&PathBuf::from(pdf), &NoProgress)?;
    let elapsed = t.elapsed();

    let n = doc.text.len();
    let pages_preview: Vec<&str> = doc.text.splitn(4, "\n\n").collect();
    println!("=== 解析完成 ===");
    println!("字符数：{n}");
    println!("耗时：{elapsed:?}");
    println!("format：{}", doc.meta.format);
    println!("size：{} bytes", doc.meta.source_bytes);
    println!();
    println!("=== 前 3 段（按 \\n\\n 切分）预览 ===");
    for (i, seg) in pages_preview.iter().take(3).enumerate() {
        let head: String = seg.chars().take(400).collect();
        println!("--- 段 {i} ({} 字符) ---", seg.chars().count());
        println!("{head}");
        println!();
    }

    // 落盘
    let out = PathBuf::from("probe_out.txt");
    std::fs::write(&out, &doc.text)?;
    println!("全文已写入：{}", out.display());
    Ok(())
}
