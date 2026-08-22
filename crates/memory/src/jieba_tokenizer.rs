//! 分句版 Jieba FTS5 tokenizer。
//!
//! ## 为什么不用 `sqlite-jieba-tokenizer` 0.6
//! 该 crate 的 `tokenize` 直接对整段文本 `JIEBA.cut(text, true)`。jieba 的 DAG
//! （有向无环图）算法对超长连续文本 O(n²) 退化：1.4M 字符（整本 PDF）需 600s+，
//! 而 1k 字符仅 0.005s。实测吞吐从 1k 的 214k chars/s 掉到 200k 的 18k chars/s。
//!
//! jieba 的设计是按句子分词（中文自然有句号/逗号/换行分割）。本 tokenizer 在
//! `tokenize` 里先按标点/换行分句，逐句 `JIEBA.cut`，offset 跨句按 byte 精确累加。
//! 实测 1.4M 字符 **600s → 1s**。
//!
//! 其余逻辑（stopword 过滤、英文 stemmer、小写归一化）与 sqlite-jieba-tokenizer
//! 0.6 完全一致，保证索引兼容。

use jieba_rs::Jieba;
use rusqlite_ext::{TokenizeReason, Tokenizer};
use sqlite_chinese_stopword::STOPWORD;
use sqlite_english_stemmer::{EN_STEMMER, is_space_or_ascii_punctuation_str, make_lowercase};
use std::ffi::CStr;
use std::ops::Range;
use std::sync::LazyLock;

static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);

/// 句子分隔符：中文标点 + 换行 + ASCII 句末标点。
/// 在这些字符处断句，逐段 jieba.cut（每段一般 < 100 字符，DAG 线性）。
fn is_sentence_boundary(ch: char) -> bool {
    matches!(ch,
        '。' | '，' | '！' | '？' | '；' | '：' | '\n' | '\r' | '\t'
        | '．' | '…'
        | '“' | '”' | '‘' | '’' | '（' | '）' | '《' | '》' | '【' | '】'
        | '.' | ',' | '!' | '?' | ';' | ':'
    )
}

/// 分句版 jieba tokenizer（stopword + stemmer 与原版一致）。
pub struct JiebaSentenceTokenizer {
    enable_stopword: bool,
}

impl Default for JiebaSentenceTokenizer {
    fn default() -> Self {
        Self { enable_stopword: true }
    }
}

impl JiebaSentenceTokenizer {
    pub fn disable_stopword(&mut self) {
        self.enable_stopword = false;
    }
}

impl Tokenizer for JiebaSentenceTokenizer {
    type Global = ();

    fn name() -> &'static CStr {
        c"jieba"
    }

    fn new(_global: &Self::Global, args: Vec<String>) -> Result<Self, rusqlite::Error> {
        let mut t = Self::default();
        for arg in args {
            if arg == "disable_stopword" {
                t.disable_stopword();
            }
        }
        Ok(t)
    }

    fn tokenize<TKF>(
        &mut self,
        _reason: TokenizeReason,
        text: &[u8],
        push_token: TKF,
    ) -> Result<(), rusqlite::Error>
    where
        TKF: FnMut(&[u8], Range<usize>, bool) -> Result<(), rusqlite::Error>,
    {
        let text = String::from_utf8_lossy(text);
        // 按标点/换行分句后逐句 jieba.cut，避免对大全文 DAG O(n²) 退化。
        self.tokenize_precise(&text, push_token)
    }
}

impl JiebaSentenceTokenizer {
    /// 按 byte 偏移精确切分句子，保证 token range 正确。
    fn tokenize_precise<TKF>(
        &mut self,
        text: &str,
        mut push_token: TKF,
    ) -> Result<(), rusqlite::Error>
    where
        TKF: FnMut(&[u8], Range<usize>, bool) -> Result<(), rusqlite::Error>,
    {
        let mut word_buf = String::new();
        let bytes = text.as_bytes();
        let mut start = 0_usize; // 当前句子的 byte 起点
        let mut i = 0_usize;
        while i < bytes.len() {
            let ch_len = utf8_char_len(bytes[i]);
            let ch = text[i..i + ch_len].chars().next().unwrap_or('\0');
            i += ch_len;
            if is_sentence_boundary(ch) {
                if i - ch_len > start {
                    self.cut_sentence(&text[start..i - ch_len], start, &mut word_buf, &mut push_token)?;
                }
                start = i;
            }
        }
        if start < text.len() {
            self.cut_sentence(&text[start..], start, &mut word_buf, &mut push_token)?;
        }
        Ok(())
    }

    fn cut_sentence<TKF>(
        &self,
        sent: &str,
        base: usize,
        word_buf: &mut String,
        push_token: &mut TKF,
    ) -> Result<(), rusqlite::Error>
    where
        TKF: FnMut(&[u8], Range<usize>, bool) -> Result<(), rusqlite::Error>,
    {
        let mut local = 0_usize;
        for word in JIEBA.cut(sent, true) {
            let range = base + local..base + local + word.len();
            local += word.len();
            if is_space_or_ascii_punctuation_str(word) {
                continue;
            }
            let need_stem = make_lowercase(word, word_buf);
            if self.enable_stopword && STOPWORD.contains(word_buf.as_str()) {
                continue;
            }
            if need_stem {
                let stemmed = EN_STEMMER.stem(word_buf.as_str()).into_owned();
                push_token(stemmed.as_bytes(), range, false)?;
            } else {
                push_token(word_buf.as_bytes(), range, false)?;
            }
        }
        Ok(())
    }
}

/// 取 UTF-8 首 byte 推断 char 长度。
fn utf8_char_len(first: u8) -> usize {
    if first < 0x80 { 1 }
    else if first >> 5 == 0b110 { 2 }
    else if first >> 4 == 0b1110 { 3 }
    else if first >> 3 == 0b11110 { 4 }
    else { 1 }
}
