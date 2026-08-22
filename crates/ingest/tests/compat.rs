//! ingest 兼容性自测（决策 4/5 要求：新库 office_oxide / pdfium-render 落地需自测）。
//!
//! 文本/CSV/JSON/图片类用内存生成的样本；PDF/Office/ePub 需真实样本（见同目录 fixtures/，缺则跳过）。
//!
//! PDF 测试需 PDFium 动态库：通过环境变量 `ONTO_PDFIUM_LIB` 指定库文件路径
//! （如 `pdfium.dll` / `libpdfium.dylib` / `libpdfium.so`），未设置则跳过 PDF 用例。
//! CI 可注入该变量；本地开发指向 src-tauri/resources/pdfium/<platform>/ 下的库。

#![allow(clippy::needless_borrow)]

use ingest::{ingest_file, IngestError};
use std::io::Write;
use std::path::PathBuf;

/// 创建临时文件写入内容，返回路径。
fn write_tmp(name: &str, content: impl AsRef<[u8]>) -> PathBuf {
    let path = std::env::temp_dir().join(format!("onto-studio-test-{name}"));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_ref()).unwrap();
    path
}

#[test]
fn text_parser_basic() {
    let path = write_tmp("sample.txt", "Hello, onto-studio!\n第二行中文。");
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "txt");
    assert!(doc.text.contains("Hello, onto-studio!"));
    assert!(doc.text.contains("第二行中文"));
    assert_eq!(doc.tables.len(), 0);
}

#[test]
fn markdown_parser_keeps_content() {
    let path = write_tmp("sample.md", "# Title\n\nSome **bold** text.\n");
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "md");
    assert!(doc.text.contains("# Title"));
}

#[test]
fn csv_parser_outputs_table() {
    let csv = "name,age\nAlice,30\nBob,25\n";
    let path = write_tmp("data.csv", csv.as_bytes());
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "csv");
    assert_eq!(doc.tables.len(), 1);
    assert_eq!(doc.tables[0].rows.len(), 3); // header + 2
    assert_eq!(doc.tables[0].rows[0], vec!["name", "age"]);
    assert!(doc.text.contains("Alice"));
}

#[test]
fn tsv_parser_delimiter() {
    let tsv = "a\tb\n1\t2\n";
    let path = write_tmp("data.tsv", tsv.as_bytes());
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.tables[0].rows[1], vec!["1", "2"]);
}

#[test]
fn json_parser_pretty_prints() {
    let path = write_tmp("data.json", b"{\"k\":\"v\"}");
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "json");
    assert!(doc.text.contains("\"k\""));
    assert!(doc.text.contains("\"v\""));
}

#[test]
fn json_parser_rejects_invalid() {
    let path = write_tmp("bad.json", b"{not json");
    let err = ingest_file(&path).unwrap_err();
    assert!(matches!(err, IngestError::Json(_)), "got {err:?}");
}

#[test]
fn unsupported_format_errors() {
    // 非 UTF-8 二进制 + 未知扩展名 → UnsupportedFormat
    let path = write_tmp("weird.xyz123", &[0xFF, 0xFE, 0x00, 0x01, 0x80, 0x9F]);
    let err = ingest_file(&path).unwrap_err();
    assert!(matches!(err, IngestError::UnsupportedFormat { .. }));
}

#[test]
fn png_image_decodes_and_produces_multimodal_part() {
    // 用 image crate 生成 2x2 红 PNG（避免手写字节 CRC 出错）
    let img = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]));
    let path = std::env::temp_dir().join("onto-studio-test-pixel.png");
    img.save_with_format(&path, image::ImageFormat::Png).unwrap();
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "png");
    assert_eq!(doc.multimodal_parts.len(), 1);
    assert_eq!(doc.multimodal_parts[0].mime, "image/png");
    assert!(!doc.multimodal_parts[0].data_b64.is_empty());
}

#[test]
fn zip_parser_extracts_text_entries() {
    use std::io::Write;
    // 用 zip crate 构造一个含 txt 的 zip
    let path = std::env::temp_dir().join("onto-studio-test-sample.zip");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::SimpleFileOptions = Default::default();
        zip.start_file("hello.txt", opts).unwrap();
        zip.write_all(b"hello from zip").unwrap();
        zip.start_file("nested.md", opts).unwrap();
        zip.write_all(b"# nested title").unwrap();
        zip.finish().unwrap();
    }
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "zip");
    assert!(doc.text.contains("hello from zip"));
    assert!(doc.text.contains("nested title"));
}

#[test]
fn gzip_parser_extracts_single_file() {
    use flate2::write::GzEncoder;
    use flate2::Compression;

    let path = std::env::temp_dir().join("onto-studio-test-sample.txt.gz");
    {
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = GzEncoder::new(file, Compression::default());
        enc.write_all(b"hello from gzip").unwrap();
        enc.finish().unwrap();
    }
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "gzip");
    assert!(doc.text.contains("hello from gzip"));
}

#[test]
fn jsonl_parser_reads_lines() {
    let path = write_tmp("data.jsonl", "{\"a\":1}\n{\"b\":2}\n");
    let doc = ingest_file(&path).unwrap();
    assert_eq!(doc.meta.format, "jsonl");
    assert!(doc.text.contains("\"a\""));
    assert!(doc.text.contains("\"b\""));
}

// ── 需真实样本的格式（缺样本则跳过，不阻断 CI） ──

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    if p.exists() { Some(p) } else { None }
}

#[test]
fn pdf_compat() {
    // PDFium 库路径（决策 5）：未提供则跳过，避免测试因缺库而失败
    let lib = match std::env::var("ONTO_PDFIUM_LIB") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => {
            eprintln!("[skip] 未设置 ONTO_PDFIUM_LIB，跳过 PDF 兼容性自测");
            return;
        }
    };
    ingest::init_pdfium(&lib).expect("init_pdfium 失败");

    let Some(p) = fixture("sample.pdf") else {
        eprintln!("[skip] 无 tests/fixtures/sample.pdf，跳过 PDF 兼容性自测");
        return;
    };
    let doc = ingest_file(&p).unwrap();
    assert_eq!(doc.meta.format, "pdf");
    assert!(!doc.text.trim().is_empty(), "PDF 应提取到文本");
}

#[test]
fn docx_compat() {
    let Some(p) = fixture("sample.docx") else {
        eprintln!("[skip] 无 tests/fixtures/sample.docx，跳过 DOCX 兼容性自测");
        return;
    };
    let doc = ingest_file(&p).unwrap();
    assert!(!doc.text.trim().is_empty(), "DOCX 应提取到文本");
}

#[test]
fn xlsx_compat() {
    let Some(p) = fixture("sample.xlsx") else {
        eprintln!("[skip] 无 tests/fixtures/sample.xlsx，跳过 XLSX 兼容性自测");
        return;
    };
    let doc = ingest_file(&p).unwrap();
    assert!(!doc.text.trim().is_empty(), "XLSX 应提取到文本");
}

#[test]
fn epub_compat() {
    let Some(p) = fixture("sample.epub") else {
        eprintln!("[skip] 无 tests/fixtures/sample.epub，跳过 ePub 兼容性自测");
        return;
    };
    let doc = ingest_file(&p).unwrap();
    assert_eq!(doc.meta.format, "epub");
    assert!(!doc.text.trim().is_empty(), "ePub 应提取到文本");
}

/// 中文 CID PDF 回归测试（决策 5 破例引入 pdfium-render 的核心动机）。
///
/// 纯 Rust（lopdf/pdfsink-rs）对 Type0 + CIDFontType2 + ToUnicode CMap 的中文 PDF
/// 解码失败，输出乱码/空。此测试用真实中文 PDF 验证 pdfium 能正确提取中文。
///
/// 样本路径由环境变量 `ONTO_PDF_CN_FIXTURE` 指定（如《曾国藩合集》）；
/// 未设置则跳过——因中文 PDF 样本不便入库（体积大、版权）。
#[test]
fn pdf_chinese_cid_compat() {
    let lib = match std::env::var("ONTO_PDFIUM_LIB") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => {
            eprintln!("[skip] 未设置 ONTO_PDFIUM_LIB，跳过中文 PDF 回归测试");
            return;
        }
    };
    ingest::init_pdfium(&lib).expect("init_pdfium 失败");

    let p = match std::env::var("ONTO_PDF_CN_FIXTURE") {
        Ok(p) => std::path::PathBuf::from(p),
        Err(_) => {
            eprintln!("[skip] 未设置 ONTO_PDF_CN_FIXTURE，跳过中文 PDF 回归测试");
            return;
        }
    };
    let doc = ingest_file(&p).unwrap();
    assert!(!doc.text.trim().is_empty(), "中文 PDF 应提取到文本");
    // 关键断言：提取的文本含 CJK 字符（非乱码）
    let has_cjk = doc.text.chars().any(|c| {
        let cp = c as u32;
        // CJK 统一表意文字范围
        (0x4E00..=0x9FFF).contains(&cp) || (0x3400..=0x4DBF).contains(&cp)
    });
    assert!(has_cjk, "提取文本应包含中文字符（非乱码）——此断言失败说明 CID CMap 解码有问题");
    eprintln!("中文 PDF 回归测试通过：{} 字符", doc.text.len());
}
