//! 并发 PDF 解析回归测试：验证进程级 mutex 串行化 pdfium 调用。
//!
//! 复现并验证修复：3 线程并发解析同一 PDF。
//! - 修复前（无锁）：STATUS_STACK_BUFFER_OVERRUN 崩溃 或 FormatError
//! - 修复后（with_pdfium 锁）：3 个线程串行执行，全部成功返回相同字符数
//!
//! 用法：`ONTO_PDFIUM_LIB=path/to/pdfium.dll cargo run --example probe_pdf_concurrent -- <pdf-path>`

use ingest::parser::{ingest_file_with_progress, NoProgress};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pdf = Arc::new(PathBuf::from(
        std::env::args().nth(1).expect("usage: probe_pdf_concurrent <pdf-path>"),
    ));
    let lib = std::env::var("ONTO_PDFIUM_LIB").expect("需设 ONTO_PDFIUM_LIB");
    ingest::init_pdfium(&PathBuf::from(lib))?;
    println!("pdfium initialized, spawning 3 concurrent parses of {}", pdf.display());

    let t = Instant::now();
    let handles: Vec<_> = (0..3)
        .map(|i| {
            let pdf = Arc::clone(&pdf);
            thread::spawn(move || {
                let start = Instant::now();
                let doc = ingest_file_with_progress(&pdf, &NoProgress);
                (i, doc, start.elapsed())
            })
        })
        .collect();

    let mut char_counts = Vec::new();
    for h in handles {
        let (i, res, dur) = h.join().expect("worker panicked");
        match res {
            Ok(doc) => {
                println!("[thread {i}] OK: {} chars in {dur:?}", doc.text.len());
                char_counts.push(doc.text.len());
            }
            Err(e) => {
                eprintln!("[thread {i}] FAILED in {dur:?}: {e}");
                return Err(format!("thread {i} failed: {e}").into());
            }
        }
    }

    println!("\n=== 总耗时 {:?} ===", t.elapsed());
    // 三次解析同一文件，字符数必须一致
    let (min, max) = (
        *char_counts.iter().min().unwrap(),
        *char_counts.iter().max().unwrap(),
    );
    assert_eq!(min, max, "并发解析字符数不一致: {char_counts:?}");
    println!("✓ 3 线程并发解析全部成功，字符数一致: {max}");
    Ok(())
}
