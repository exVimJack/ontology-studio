//! 分发器：按文件扩展名（辅以 MIME 探测）路由到具体 parser。
//!
//! 优先扩展名（快、零读），未知扩展名回退到内容嗅探（读 magic bytes）。

use crate::error::{IngestError, IngestResult};
use crate::parser::DocumentParser;
use std::path::Path;
use std::sync::Arc;

/// 按路径选 parser。返回 `Arc<dyn DocumentParser>` 以便复用。
pub fn pick_parser(path: &Path) -> IngestResult<Arc<dyn DocumentParser>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    use crate::parsers::*;
    match ext.as_str() {
        // PDF
        "pdf" => Ok(Arc::new(PdfParser)),
        // Office（office_oxide 统一处理 DOCX/PPTX/老格式，XLSX 也支持）
        "docx" | "doc" | "pptx" | "ppt" | "xlsx" | "xls" => {
            Ok(Arc::new(OfficeParser::new()))
        }
        // XLSX 结构化表格（calamine，保留表格语义）
        // 注：office_oxide 已覆盖 xlsx 文本，calamine 作为表格保真补充。
        // 一期用 office_oxide 统一路径；如需表格语义二期切 calamine。
        // eBook
        "epub" => Ok(Arc::new(EpubParser)),
        // 图片
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" => Ok(Arc::new(ImageParser)),
        // 文本类
        "txt" | "log" | "rs" | "go" | "py" | "js" | "ts" | "tsx" | "jsx" | "java" | "c" | "cpp"
        | "h" | "hpp" | "cs" | "rb" | "php" | "swift" | "kt" | "sh" | "bash" | "zsh" | "fish"
        | "ps1" | "bat" | "cmd" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "xml"
        | "html" | "css" | "scss" | "sql" | "dockerfile" | "gitignore" | "env" | "md"
        | "markdown" | "vue" | "svelte" | "astro" | "less" | "styl" | "tex" | "latex"
        | "lua" | "scala" | "dart" | "r" | "m" | "mm" | "pl" | "erl" | "ex" | "exs"
        | "clj" | "cljs" | "cljc" | "fs" | "fsx" | "groovy" | "gradle" | "v" | "vhdl" | "sv"
        | "asm" | "s" | "zig" | "nim" | "jl" | "sol" | "graphql" | "gql" | "proto"
        | "rst" | "adoc" | "asciidoc" | "org" | "textile" | "properties" | "editorconfig"
        | "diff" | "patch" | "lock" | "svg" | "rss" | "atom" => Ok(Arc::new(TextParser)),
        "csv" | "tsv" => Ok(Arc::new(CsvParser)),
        "json" | "jsonl" | "ndjson" => Ok(Arc::new(JsonParser)),
        // 压缩包
        "zip" => Ok(Arc::new(ZipParser)),
        "gz" => Ok(Arc::new(GzipParser)),
        "tar" | "tgz" | "tbz2" | "txz" => {
            Ok(Arc::new(TarParser))
        }
        _ => {
            // 无扩展名或未知：尝试内容嗅探
            sniff(path).ok_or_else(|| IngestError::UnsupportedFormat {
                ext: ext.clone(),
                path: path.to_path_buf(),
            })
        }
    }
}

/// 读前几个字节判断格式（无扩展名文件兜底）。
fn sniff(path: &Path) -> Option<Arc<dyn DocumentParser>> {
    use crate::parsers::*;
    let mut buf = [0u8; 8];
    let mut f = std::fs::File::open(path).ok()?;
    use std::io::Read;
    let n = f.read(&mut buf).ok()?;
    let head = &buf[..n];

    // PDF: %PDF
    if head.starts_with(b"%PDF") {
        return Some(Arc::new(PdfParser));
    }
    // ZIP 类（docx/xlsx/epub/zip 都是 zip）：交给 office_oxide / epub / zip 尝试
    // PK\x03\x04
    if head.starts_with(b"PK\x03\x04") {
        // 先试 office_oxide（docx/xlsx），再试 epub，再 zip
        return Some(Arc::new(SmartZipParser::new()));
    }
    // PNG / JPEG / GIF / BMP magic
    if head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(b"\xff\xd8\xff")
        || head.starts_with(b"GIF8")
        || head.starts_with(b"BM")
    {
        return Some(Arc::new(ImageParser));
    }
    // 兜底：无扩展名但能读为 UTF-8 → 当作纯文本
    // （zip/tar 递归解出的临时文件无扩展名，多为文本）
    if std::fs::read(path)
        .ok()
        .and_then(|b| String::from_utf8(b.clone()).ok().map(|_| b))
        .is_some()
    {
        return Some(Arc::new(TextParser));
    }
    None
}
