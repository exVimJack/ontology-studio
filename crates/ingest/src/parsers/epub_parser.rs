//! ePub parser：rbook（Apache-2.0，决策 6）。
//!
//! 通过 spine 顺序 + manifest 取章节资源，read_str() 读 XHTML，剥离标签得纯文本。

use crate::document::Document;
use crate::error::{IngestError, IngestResult};
use crate::parser::{make_meta, DocumentParser, ParseProgress};
use rbook::Epub;
use std::path::Path;

pub struct EpubParser;

impl DocumentParser for EpubParser {
    fn format_name(&self) -> &'static str {
        "epub"
    }

    fn parse_with_progress(&self, path: &Path, progress: &dyn ParseProgress) -> IngestResult<Document> {
        let bytes = std::fs::metadata(path)?.len();
        let meta = make_meta(path, "epub", bytes);

        progress.on_phase("打开 EPUB");
        let epub = Epub::open(path).map_err(|e| IngestError::Epub(format!("{e:?}")))?;

        // 元信息：标题/作者
        let md = epub.metadata();
        let mut header = String::new();
        if let Some(title) = md.title() {
            header.push_str(&format!("# {}\n\n", title.value()));
        }
        if let Some(creator) = md.creators().next() {
            header.push_str(&format!("作者: {}\n\n", creator.value()));
        }

        // 遍历 spine 顺序读章节
        progress.on_phase("提取章节文本");
        let spine = epub.spine();
        let manifest = epub.manifest();
        let total = spine.len();
        let mut body = String::new();

        for (i, item_ref) in spine.iter().enumerate() {
            if progress.is_cancelled() {
                return Err(IngestError::Cancelled);
            }
            let idref = item_ref.idref();
            if let Some(item) = manifest.by_id(idref) {
                if let Ok(content) = item.read_str() {
                    let plain = strip_html(&content);
                    if !plain.trim().is_empty() {
                        progress.on_chars(plain.len());
                        body.push_str(&plain);
                        body.push_str("\n\n");
                    }
                }
            }
            progress.on_progress(i + 1, Some(total));
        }

        let text = if body.trim().is_empty() {
            header
        } else {
            format!("{header}{body}")
        };

        Ok(Document::new_text(text, meta))
    }
}

/// 粗剥离 HTML 标签为纯文本（ePub 章节是 XHTML）。
/// 一期够用；二期可换更完整的 HTML→Markdown。
///
/// 性能：单次线性扫描，不反复切片/find，避免大章节 O(n²) 退化。
fn strip_html(html: &str) -> String {
    let bytes = html.as_bytes();
    let lower: Vec<u8> = html.as_bytes().iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut out = String::with_capacity(html.len() / 2);
    let mut i = 0;
    let n = bytes.len();
    while i < n {
        if bytes[i] == b'<' {
            // 判断是否 script/style 块
            let tag_end = lower[i..]
                .iter()
                .position(|&b| b == b' ' || b == b'>' || b == b'/' || b == b'\n')
                .map(|p| i + p)
                .unwrap_or(n);
            let tag_name = &lower[i + 1..tag_end];
            if tag_name == b"script" || tag_name == b"style" {
                // 找闭合标签 </script> 或 </style>
                let close: &[u8] = if tag_name == b"script" {
                    b"</script>"
                } else {
                    b"</style>"
                };
                // 在 lower 里从 i 开始线性找 close
                if let Some(pos) = find_subslice(&lower[i..], close) {
                    i += pos + close.len();
                } else {
                    break;
                }
                continue;
            }
            // 普通标签：跳到 '>'
            match lower[i..].iter().position(|&b| b == b'>') {
                Some(pos) => {
                    i += pos + 1;
                    out.push(' ');
                }
                None => break,
            }
        } else {
            // 原样输出（UTF-8 安全：'<' 是 ASCII，其余字节按 char 推进）
            // 为避免拆坏多字节字符，按 char 边界推进
            let ch_len = utf8_char_len(bytes[i]);
            if i + ch_len <= n {
                out.push_str(&html[i..i + ch_len]);
                i += ch_len;
            } else {
                i += 1;
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 返回 UTF-8 首字节指示的字符长度。
fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else if b >> 3 == 0b11110 {
        4
    } else {
        1 // 非法首字节，保守推进 1
    }
}

/// 在 haystack 中线性查找 needle 的首次出现（避免反复切片）。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let last = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}
