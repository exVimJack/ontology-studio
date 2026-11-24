//! Turtle 属性/关系结构化编辑（文本手术，非重序列化）。
//!
//! 架构（决策：TTL 手动编辑 P0，只做属性/关系，类不动）：
//!   - 读取：token 化（带字节位置）→ 重建语句（subject … `.`）→ 提取谓语/宾语 → DTO
//!   - 写入：编辑 DTO → 重新生成该属性块文本 → 原位替换 → validate → import overwrite
//!
//! 为什么文本手术而不是 parse→改→序列化：
//!   - 落库存原文（见 ttl.rs `import_ttl`），注释/排版/prefix 组织是用户资产，重序列化会静默丢失
//!   - `legalize_double_hash_iris`（ttl.rs）已证明状态机路线可行，本模块是其同构延伸
//!
//! 关键设计：
//!   - 读写都在**原文**上做（双 # IRI 如 `<...#Policy#endDate>` 原样识别为 IRI token，
//!     不经 oxigraph 合法化路径，避免读写映射错位）
//!   - 语句 = 深度 0 的 token 序列到 `.`；嵌套 `[]`/`()`（如 owl:Restriction）整体为
//!     单个 token，不拆
//!   - 同主语散块（Turtle 允许属性定义散在多处）合并为一块写回首块位置、删掉其余
//!   - 不可编辑块（类声明、owl:Ontology 头、SWRL 规则等）字节级不动

use crate::error::{StoreError, StoreResult};

// ════════════════════════════════════════════════════════════════
// DTO
// ════════════════════════════════════════════════════════════════

/// 属性（字段/关系）的读写视图。IRI 为原文形态（可能含双 #）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Default)]
pub struct TtlPropertyEdit {
    /// 属性 IRI 全形（原文，如 `https://x.cn/ont#Policy#endDate`）。不可改（身份）。
    pub iri: String,
    /// "datatype" | "object"（owl:DatatypeProperty / owl:ObjectProperty）。
    pub kind: String,
    /// rdfs:label（去引号去语言标签后的纯文本；空 = 未设置）。
    #[serde(default)]
    pub label: String,
    /// rdfs:comment（同上）。
    #[serde(default)]
    pub comment: String,
    /// rdfs:domain 全 IRI（空 = 未设置）。
    #[serde(default)]
    pub domain: String,
    /// rdfs:range 全 IRI（xsd 类型或类；空 = 未设置）。
    #[serde(default)]
    pub range: String,
}

/// 类的读写视图（版本 diff 用：label/comment 修改检测，属性编辑不碰类块）。
/// 与 `TtlPropertyEdit` 同字段语义（kind 恒为 "class"）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Default)]
pub struct TtlClassMeta {
    /// 类 IRI 全形（原文，可能含双 #）。
    pub iri: String,
    /// rdfs:label（去引号去语言标签后的纯文本；空 = 未设置）。
    #[serde(default)]
    pub label: String,
    /// rdfs:comment（同上）。
    #[serde(default)]
    pub comment: String,
}

/// 类的编辑视图（概念图谱编辑用）：label/comment/subClassOf 受管，其余谓词原样保留。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Default)]
pub struct TtlClassEdit {
    /// 类 IRI 全形（原文，可能含双 #）。不可改（身份）。
    pub iri: String,
    /// rdfs:label（纯文本；空 = 删除该谓词）。
    #[serde(default)]
    pub label: String,
    /// rdfs:comment（同上）。
    #[serde(default)]
    pub comment: String,
    /// 父类 IRI 列表（rdfs:subClassOf，可多个；空 = 无父类）。
    #[serde(default)]
    pub parents: Vec<String>,
}

// ════════════════════════════════════════════════════════════════
// 1. Token 化（带字节位置）
// ════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    /// `<...>`（text = 尖括号内文）
    Iri,
    /// `pfx:local` / `:local`（text = 原文短形）
    Pname,
    /// `a`（rdf:type 缩写）
    KeywordA,
    /// `PREFIX`/`BASE`/`@prefix`/`@base` 指令头（text = 原文）
    Directive,
    /// 字面量（text = 原文含引号与语言标签，如 `"xxx"@zh`）
    Literal,
    /// `;` `,` `.`（仅深度 0 产出；text = 单字符）
    Separator(char),
    /// `[ ... ]` 匿名节点 / `( ... )` 集合（text = 原文整体）
    Group,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    text: String,
    start: usize,
    end: usize,
}

/// Turtle 文本 token 化。
///
/// - 跳过空白与 `#` 行注释；字符串（短/长）与 `<...>` IRI 内的 `#`/`;`/`.` 不拆
/// - 深度 0 的 `;` `,` `.` 产出 Separator；`[ ... ]`/`( ... )` 整体为一个 Group token
///   （嵌套内部不产出 token——属性块顶层结构不被嵌套内容干扰）
/// - `@prefix`/`@base`/`PREFIX`/`BASE` 产出 Directive（供语句重建时跳过；
///   `@zh` 语言标签并入前导 Literal 的 text）
fn tokenize(ttl: &str) -> Vec<Token> {
    #[derive(PartialEq)]
    enum St {
        Normal,
        LineComment,
        ShortString { start: usize },
        LongString { start: usize },
        Iri { start: usize },
    }
    let bytes = ttl.as_bytes();
    let mut st = St::Normal;
    let mut i = 0;
    let mut tokens: Vec<Token> = Vec::new();
    // 正在积累的裸词（pname / a / PREFIX / 数字）起点
    let mut word_start: Option<usize> = None;

    /// 结束裸词积累，产出 token（若构成 pname / a / 指令头）。
    /// 裸数字/布尔等非 pname 词直接丢弃（属性块顶层不作为宾语提取）。
    macro_rules! flush_word {
        ($end:expr) => {
            if let Some(ws) = word_start.take() {
                let end = ($end).min(bytes.len());
                if end > ws {
                    let text = &ttl[ws..end];
                    let lower = text.to_ascii_lowercase();
                    if text == "a" {
                        tokens.push(Token {
                            kind: TokenKind::KeywordA,
                            text: text.to_string(),
                            start: ws,
                            end,
                        });
                    } else if lower == "prefix" || lower == "base" {
                        tokens.push(Token {
                            kind: TokenKind::Directive,
                            text: text.to_string(),
                            start: ws,
                            end,
                        });
                    } else if text.contains(':') {
                        tokens.push(Token {
                            kind: TokenKind::Pname,
                            text: text.to_string(),
                            start: ws,
                            end,
                        });
                    }
                }
            }
        };
    }

    while i < bytes.len() {
        let b = bytes[i];
        match st {
            St::LineComment => {
                if b == b'\n' {
                    st = St::Normal;
                }
                i += 1;
            }
            St::ShortString { start } => {
                if b == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else if b == b'"' {
                    // 字面量闭引号；吞掉紧随的语言标签 `@xx`（若有）
                    let mut end = i + 1;
                    if end < bytes.len() && bytes[end] == b'@' {
                        let mut j = end + 1;
                        while j < bytes.len()
                            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-')
                        {
                            j += 1;
                        }
                        end = j;
                    } else if ttl[end..].starts_with("^^") {
                        // `^^<datatype>`：吞到 `>` 或空白
                        let mut j = end + 2;
                        while j < bytes.len()
                            && !bytes[j].is_ascii_whitespace()
                            && bytes[j] != b'.'
                            && bytes[j] != b';'
                            && bytes[j] != b','
                        {
                            j += 1;
                        }
                        end = j;
                    }
                    tokens.push(Token {
                        kind: TokenKind::Literal,
                        text: ttl[start..end].to_string(),
                        start,
                        end,
                    });
                    st = St::Normal;
                    i = end;
                } else {
                    i += 1;
                }
            }
            St::LongString { start } => {
                if bytes[i..].starts_with(b"\"\"\"") {
                    tokens.push(Token {
                        kind: TokenKind::Literal,
                        text: ttl[start..i + 3].to_string(),
                        start,
                        end: i + 3,
                    });
                    st = St::Normal;
                    i += 3;
                } else {
                    i += 1;
                }
            }
            St::Iri { start } => {
                if b == b'>' {
                    tokens.push(Token {
                        kind: TokenKind::Iri,
                        text: ttl[start + 1..i].to_string(),
                        start,
                        end: i + 1,
                    });
                    st = St::Normal;
                }
                i += 1;
            }
            St::Normal => {
                if b == b'#' {
                    flush_word!(i);
                    st = St::LineComment;
                    i += 1;
                } else if b == b'"' && bytes[i..].starts_with(b"\"\"\"") {
                    flush_word!(i);
                    st = St::LongString { start: i };
                    i += 3;
                } else if b == b'"' {
                    flush_word!(i);
                    st = St::ShortString { start: i };
                    i += 1;
                } else if b == b'<' {
                    // 完整 `<...>` 才是 IRI（同 legalize 状态机：中途遇空白/引号视为裸 `<`）
                    let mut j = i + 1;
                    while j < bytes.len() {
                        let c = bytes[j];
                        if c == b'>'
                            || c.is_ascii_whitespace()
                            || c == b'"'
                            || c == b'<'
                            || c == b'\''
                            || c == b'{'
                            || c == b'}'
                        {
                            break;
                        }
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'>' {
                        flush_word!(i);
                        st = St::Iri { start: i };
                        // Iri 态在 `>` 处产出 token；i 不动，交给下一轮
                    } else {
                        i += 1;
                    }
                } else if b == b'@' {
                    // `@prefix`/`@base` 指令；`@lang` 已并入 Literal，不该在此出现
                    flush_word!(i);
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    let word = ttl[i + 1..j].to_ascii_lowercase();
                    if word == "prefix" || word == "base" {
                        tokens.push(Token {
                            kind: TokenKind::Directive,
                            text: ttl[i..j].to_string(),
                            start: i,
                            end: j,
                        });
                    }
                    i = j;
                } else if b == b'[' || b == b'(' {
                    flush_word!(i);
                    // 匹配的闭合（跳过字符串/注释/IRI 内部）
                    let (open, close) = if b == b'[' { ('[', ']') } else { ('(', ')') };
                    let mut d = 1;
                    let mut j = i + 1;
                    let mut inner = St::Normal;
                    while j < bytes.len() && d > 0 {
                        let c = bytes[j];
                        match inner {
                            St::Normal => match c {
                                c if c == open as u8 => d += 1,
                                c if c == close as u8 => d -= 1,
                                b'"' if bytes[j..].starts_with(b"\"\"\"") => {
                                    inner = St::LongString { start: j };
                                    j += 2;
                                }
                                b'"' => inner = St::ShortString { start: j },
                                b'<' => {
                                    let mut k = j + 1;
                                    while k < bytes.len() && bytes[k] != b'>' {
                                        k += 1;
                                    }
                                    j = k; // 内部 IRI 一次跳过
                                }
                                b'#' => inner = St::LineComment,
                                _ => {}
                            },
                            St::LineComment => {
                                if c == b'\n' {
                                    inner = St::Normal;
                                }
                            }
                            St::ShortString { .. } => {
                                if c == b'\\' {
                                    j += 1;
                                } else if c == b'"' {
                                    inner = St::Normal;
                                }
                            }
                            St::LongString { .. } => {
                                if bytes[j..].starts_with(b"\"\"\"") {
                                    inner = St::Normal;
                                    j += 2;
                                }
                            }
                            St::Iri { .. } => {
                                if c == b'>' {
                                    inner = St::Normal;
                                }
                            }
                        }
                        j += 1;
                    }
                    tokens.push(Token {
                        kind: TokenKind::Group,
                        text: ttl[i..j.min(bytes.len())].to_string(),
                        start: i,
                        end: j.min(bytes.len()),
                    });
                    i = j;
                } else if b == b';' || b == b',' || b == b'.' {
                    flush_word!(i);
                    tokens.push(Token {
                        kind: TokenKind::Separator(b as char),
                        text: (b as char).to_string(),
                        start: i,
                        end: i + 1,
                    });
                    i += 1;
                } else if b.is_ascii_whitespace() {
                    flush_word!(i);
                    i += 1;
                } else {
                    // 裸词字符（pname/keyword/数字/`]`/`)` 等）
                    // 注意 `]` `)` 在深度 0 出现属非法 Turtle，作裸词跳过即可
                    if word_start.is_none() {
                        word_start = Some(i);
                    }
                    i += 1;
                }
            }
        }
    }
    flush_word!(bytes.len());
    tokens
}

// ════════════════════════════════════════════════════════════════
// 2. 语句重建（subject … `.`）
// ════════════════════════════════════════════════════════════════

/// 一条语句（Turtle statement）：主语 token + 到 `.` 为止的全部 token。
#[derive(Debug, Clone)]
struct Statement {
    /// 主语 token（Iri/Pname；Directive 语句为 None 且跳过）。
    subject: Option<Token>,
    /// 语句首 token 起点（含前导 Directive 时的 directive 起点）。
    start: usize,
    /// `.` 之后 + 一个换行（若有）——删块连换行一起删。
    end: usize,
    /// 语句内（除主语外）的 token（谓语/宾语/分隔符/Group）。
    rest: Vec<Token>,
}

/// 从 token 流重建语句列表。Directive 语句（prefix/base）跳过。
fn statements(ttl: &str, tokens: &[Token]) -> Vec<Statement> {
    let bytes = ttl.as_bytes();
    let mut out = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        let first = &tokens[idx];
        match first.kind {
            TokenKind::Directive => {
                // 跳到本语句的 `.`（prefix 声明也以 `.` 结尾）
                let mut j = idx;
                while j < tokens.len()
                    && !matches!(tokens[j].kind, TokenKind::Separator('.'))
                {
                    j += 1;
                }
                idx = if j < tokens.len() { j + 1 } else { tokens.len() };
            }
            TokenKind::Iri | TokenKind::Pname | TokenKind::Group => {
                // 主语语句。Group 主语（匿名节点主语）保留结构但不作属性编辑对象。
                let subject = Some(first.clone());
                let start = first.start;
                let mut rest = Vec::new();
                let mut j = idx + 1;
                while j < tokens.len() {
                    match tokens[j].kind {
                        TokenKind::Separator('.') => break,
                        _ => rest.push(tokens[j].clone()),
                    }
                    j += 1;
                }
                // end = `.` 之后 + 换行（若有）
                let dot_end = if j < tokens.len() {
                    tokens[j].end
                } else {
                    ttl.len()
                };
                let mut end = dot_end;
                if end < bytes.len() && bytes[end] == b'\r' {
                    end += 1;
                }
                if end < bytes.len() && bytes[end] == b'\n' {
                    end += 1;
                }
                out.push(Statement {
                    subject,
                    start,
                    end,
                    rest,
                });
                idx = if j < tokens.len() { j + 1 } else { tokens.len() };
            }
            _ => {
                // 孤立 token（语法异常），跳过
                idx += 1;
            }
        }
    }
    out
}

/// 主语 token 文本 → 全 IRI（`<...>` 内文已是全形；短形用 prefix 表展开）。
fn subject_full_iri(subject: &Token, prefixes: &[(String, String)]) -> String {
    match subject.kind {
        TokenKind::Iri => subject.text.clone(),
        _ => expand_pname(&subject.text, prefixes)
            .unwrap_or_else(|| subject.text.clone()),
    }
}

// ════════════════════════════════════════════════════════════════
// 3. 语句内谓语/宾语提取
// ════════════════════════════════════════════════════════════════

/// 一对谓语/宾语（宾语为原文 token 文本；`,` 多值共享谓语）。
#[derive(Debug, Clone)]
struct PredObj {
    /// 谓词原文（`a` 或 `rdfs:label`；`<...>` 形为内文）。
    pred: String,
    /// 宾语原文（IRI 内文 / 字面量含引号 / Group 原文）。
    obj: String,
    /// 宾语原文全形（IRI 带 `<>`；非 IRI 同 obj）——重新生成块时保真用。
    obj_raw: String,
    is_iri: bool,
    is_literal: bool,
}

/// 解析语句 rest token：`pred obj (, obj)* (; pred ...)*`。
fn parse_pred_objs(rest: &[Token]) -> Vec<PredObj> {
    let mut out = Vec::new();
    let mut cur_pred: Option<String> = None;
    for tok in rest {
        match &tok.kind {
            TokenKind::KeywordA => cur_pred = Some("a".to_string()),
            TokenKind::Iri => {
                if cur_pred.is_none() {
                    cur_pred = Some(tok.text.clone());
                } else {
                    out.push(PredObj {
                        pred: cur_pred.clone().unwrap_or_default(),
                        obj: tok.text.clone(),
                        obj_raw: format!("<{}>", tok.text),
                        is_iri: true,
                        is_literal: false,
                    });
                }
            }
            TokenKind::Pname => {
                if cur_pred.is_none() {
                    cur_pred = Some(tok.text.clone());
                } else {
                    out.push(PredObj {
                        pred: cur_pred.clone().unwrap_or_default(),
                        obj: tok.text.clone(),
                        obj_raw: tok.text.clone(),
                        is_iri: true,
                        is_literal: false,
                    });
                }
            }
            TokenKind::Literal => {
                if let Some(p) = &cur_pred {
                    out.push(PredObj {
                        pred: p.clone(),
                        obj: tok.text.clone(),
                        obj_raw: tok.text.clone(),
                        is_iri: false,
                        is_literal: true,
                    });
                }
            }
            TokenKind::Group => {
                if let Some(p) = &cur_pred {
                    out.push(PredObj {
                        pred: p.clone(),
                        obj: tok.text.clone(),
                        obj_raw: tok.text.clone(),
                        is_iri: false,
                        is_literal: false,
                    });
                }
            }
            TokenKind::Separator(';') => cur_pred = None,
            TokenKind::Separator(_) | TokenKind::Directive => {}
        }
    }
    out
}

/// 谓词原文 → 全 IRI（`a` → rdf:type）。
fn pred_full(pred: &str, prefixes: &[(String, String)]) -> String {
    if pred == "a" {
        return RDF_TYPE.to_string();
    }
    if let Some(inner) = pred.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return inner.to_string();
    }
    expand_pname(pred, prefixes).unwrap_or_else(|| pred.to_string())
}

/// 宾语原文（IRI 形）→ 全 IRI。
fn obj_full_iri(obj: &str, prefixes: &[(String, String)]) -> String {
    if let Some(inner) = obj.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return inner.to_string();
    }
    expand_pname(obj, prefixes).unwrap_or_else(|| obj.to_string())
}

// ════════════════════════════════════════════════════════════════
// 4. Prefix 映射
// ════════════════════════════════════════════════════════════════

/// 从 Turtle 文本提取 prefix 映射（`@prefix pfx: <iri>` / `PREFIX pfx: <iri>`）。
/// 空前缀（`@prefix : <...>`）短形为 `:`。
pub(crate) fn extract_prefixes(ttl: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in ttl.lines() {
        let t = line.trim_start();
        let lower = t.to_ascii_lowercase();
        // 去掉指令关键字，剩余形如 `pfx: <iri> .`
        // strip_prefix 返回剩余串——切片起点应是「原长 - 剩余长」即前缀长，不是 r.len()
        let rest = if lower.starts_with("@prefix ") {
            &t["@prefix ".len()..]
        } else if lower.starts_with("prefix ") {
            &t["prefix ".len()..]
        } else if lower.starts_with("@base ") || lower.starts_with("base ") {
            // @base 不产生 pname 映射（base 用于相对 IRI 解析，pname 不引用它）
            continue;
        } else {
            continue;
        };
        let rest = rest.trim_start();
        // rest 形如 `pfx: <iri> .`——pfx 到首个 `:` 为止
        if let Some(c) = rest.find(':') {
            let after = rest[c + 1..].trim_start();
            if let (Some(l), Some(g)) = (after.find('<'), after.find('>')) {
                if l < g {
                    let pfx = rest[..c].trim().to_ascii_lowercase();
                    out.push((format!("{pfx}:"), after[l + 1..g].to_string()));
                }
            }
        }
    }
    out
}

/// 短形（`pfx:local` / `:local`）→ 全 IRI。无匹配返回 None。
pub(crate) fn expand_pname(pname: &str, prefixes: &[(String, String)]) -> Option<String> {
    let colon = pname.find(':')?;
    let pfx = &pname[..colon + 1];
    let local = &pname[colon + 1..];
    for (p, iri) in prefixes {
        if p.eq_ignore_ascii_case(pfx) {
            return Some(format!("{iri}{local}"));
        }
    }
    None
}

/// 全 IRI → 尽量短形（最长匹配 prefix）。
/// 残段含 `#`（双 # IRI 被 prefix 吞并会产生非法 pname）时保持 `<iri>` 全形。
pub(crate) fn abbreviate_iri(iri: &str, prefixes: &[(String, String)]) -> String {
    let mut best: Option<(usize, &str)> = None;
    for (p, piri) in prefixes {
        if iri.starts_with(piri.as_str()) {
            let l = piri.len();
            if best.map(|(bl, _)| l > bl).unwrap_or(true) {
                best = Some((l, p));
            }
        }
    }
    match best {
        Some((len, p)) => {
            let local = &iri[len..];
            if local.contains('#') || local.is_empty() {
                // 残段含 #（双 # 场景）或 prefix 恰好等于整个 IRI：用全形
                format!("<{iri}>")
            } else {
                format!("{p}{local}")
            }
        }
        None => format!("<{iri}>"),
    }
}

// ════════════════════════════════════════════════════════════════
// 5. 属性块读取（TtlPropertyEdit / 类列表）
// ════════════════════════════════════════════════════════════════

/// 新增属性的默认 IRI 前缀（前端表单预填用）。
///
/// 优先级：
///   1. 已声明属性里出现最多的 namespace（取最长公共前缀到 `#`/`/`）——
///      和本体既有属性形态一致（如 `https://.../AutoDealerRetail#`）
///   2. 回退：`PREFIX ns:` 声明的 namespace
///   3. 兜底：本体 IRI 的 `#` 前缀（旧行为）
///
/// 这样新建的属性 IRI 与全文既有属性同 namespace，导出后形态统一。
pub(crate) fn default_property_namespace(
    ttl: &str,
    ontology_iri: &str,
) -> String {
    // 1. 已声明属性 IRI 的 namespace 投票（去尾段，得 `...#` / `.../` 前缀）
    let prefixes = extract_prefixes(ttl);
    let mut votes: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for p in list_properties(ttl) {
        if let Some(ns) = namespace_of(&p.iri) {
            *votes.entry(ns).or_insert(0) += 1;
        }
    }
    if let Some((ns, _)) = votes.iter().max_by_key(|(_, c)| **c) {
        return ns.clone();
    }
    // 2. ns: 前缀声明（`ns:` 是工具导出的惯例命名）
    for (p, iri) in &prefixes {
        if p == "ns:" {
            return iri.clone();
        }
    }
    // 3. 本体 IRI 兜底（旧行为）
    ontology_iri
        .rsplit_once('#')
        .map(|(base, _)| format!("{base}#"))
        .unwrap_or_else(|| format!("{ontology_iri}#"))
}

/// IRI 的 namespace：尾段分隔符（`#` 优先，无则最后一个 `/`）之前含分隔符的部分。
/// `https://x.cn/ont#Policy#endDate`（双 # 原文）→ `https://x.cn/ont#Policy#`。
fn namespace_of(iri: &str) -> Option<String> {
    let cut = iri.rfind('#').or_else(|| iri.rfind('/'))?;
    Some(iri[..=cut].to_string())
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_SUBCLASSOF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_RESTRICTION: &str = "http://www.w3.org/2002/07/owl#Restriction";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_EQUIVALENT_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const SKOS_EDITORIAL_NOTE: &str = "http://www.w3.org/2004/02/skos/core#editorialNote";

/// 完整字面量解析：`"text"@zh` → (text, Some("zh"))；`"text"` → (text, None)。
fn parse_literal(lit: &str) -> String {
    let s = lit.trim();
    if let Some(rest) = s.strip_prefix("\"\"\"") {
        if let Some(pos) = rest.rfind("\"\"\"") {
            return rest[..pos].to_string();
        }
        return rest.to_string();
    }
    if let Some(rest) = s.strip_prefix('"') {
        let bytes = rest.as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            if bytes[j] == b'\\' {
                j += 2;
                continue;
            }
            if bytes[j] == b'"' {
                return rest[..j].to_string();
            }
            j += 1;
        }
        return rest.to_string();
    }
    s.to_string()
}

/// 属性判断：语句的 rdf:type 是 Datatype/ObjectProperty → Some(kind)。
fn property_kind(stmt: &Statement, prefixes: &[(String, String)]) -> Option<&'static str> {
    for po in parse_pred_objs(&stmt.rest) {
        if pred_full(&po.pred, prefixes) == RDF_TYPE && po.is_iri {
            let obj = obj_full_iri(&po.obj, prefixes);
            if obj == OWL_DATATYPE_PROPERTY {
                return Some("datatype");
            }
            if obj == OWL_OBJECT_PROPERTY {
                return Some("object");
            }
        }
    }
    None
}

/// 从原文提取全部属性（DatatypeProperty / ObjectProperty）的结构化视图。
/// 散块聚合（同主语多语句合并）；label/comment/domain/range 取首见值。
pub fn list_properties(ttl: &str) -> Vec<TtlPropertyEdit> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out: Vec<TtlPropertyEdit> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        let full_iri = subject_full_iri(subject, &prefixes);
        // 散块聚合：主语已建条目 → 字段空位填充；新主语 → 必须是属性声明块
        // （散块本身无 `a owl:*Property`，靠主语匹配进已有条目）
        let kind = property_kind(stmt, &prefixes);
        let is_scatter = index.contains_key(&full_iri);
        if kind.is_none() && !is_scatter {
            continue;
        }
        let entry = match index.get(&full_iri) {
            Some(&i) => out[i].clone(),
            None => TtlPropertyEdit {
                iri: full_iri.clone(),
                kind: kind.unwrap_or_default().to_string(),
                ..Default::default()
            },
        };
        let mut entry = entry;
        // kind 以声明块为准（散块不覆盖）
        if let Some(k) = kind {
            entry.kind = k.to_string();
        }
        for po in parse_pred_objs(&stmt.rest) {
            let pf = pred_full(&po.pred, &prefixes);
            match pf.as_str() {
                RDFS_LABEL if po.is_literal && entry.label.is_empty() => {
                    entry.label = parse_literal(&po.obj);
                }
                RDFS_COMMENT if po.is_literal && entry.comment.is_empty() => {
                    entry.comment = parse_literal(&po.obj);
                }
                RDFS_DOMAIN if po.is_iri && entry.domain.is_empty() => {
                    entry.domain = obj_full_iri(&po.obj, &prefixes);
                }
                RDFS_RANGE if po.is_iri && entry.range.is_empty() => {
                    entry.range = obj_full_iri(&po.obj, &prefixes);
                }
                _ => {}
            }
        }
        if let Some(&i) = index.get(&full_iri) {
            out[i] = entry;
        } else {
            index.insert(full_iri, out.len());
            out.push(entry);
        }
    }
    out
}

/// 提取本本体已声明的全部类 IRI（domain/range 下拉选项用）。
/// 只认 `a owl:Class`（对齐 validate 第 2 类规范）。
pub fn list_class_iris(ttl: &str) -> Vec<String> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out = Vec::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        for po in parse_pred_objs(&stmt.rest) {
            if pred_full(&po.pred, &prefixes) == RDF_TYPE
                && po.is_iri
                && obj_full_iri(&po.obj, &prefixes) == "http://www.w3.org/2002/07/owl#Class"
            {
                out.push(subject_full_iri(subject, &prefixes));
            }
        }
    }
    out
}

/// 提取全部 `rdfs:subClassOf` 边（child, parent），全形 IRI；去重。
/// 与 `list_class_metas` 同一原文 tokenize 坐标系（双 # 保留）。
pub fn list_subclass_of_edges(ttl: &str) -> Vec<(String, String)> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut seen = std::collections::HashSet::<(String, String)>::new();
    let mut out = Vec::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        let child = subject_full_iri(subject, &prefixes);
        for po in parse_pred_objs(&stmt.rest) {
            if pred_full(&po.pred, &prefixes) != RDFS_SUBCLASSOF || !po.is_iri {
                continue;
            }
            let parent = obj_full_iri(&po.obj, &prefixes);
            if parent.is_empty() {
                continue;
            }
            if seen.insert((child.clone(), parent.clone())) {
                out.push((child.clone(), parent));
            }
        }
    }
    out
}

/// 提取本本体已声明的全部类（含 label/comment 元数据，版本 diff 用）。
/// 只认 `a owl:Class`（对齐 list_class_iris）；散块聚合：同一类的 label/comment
/// 散在多处时按 IRI 合并。与 `list_properties` 同一原文 tokenize 坐标系。
pub(crate) fn list_class_metas(ttl: &str) -> Vec<TtlClassMeta> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out: Vec<TtlClassMeta> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        let full_iri = subject_full_iri(subject, &prefixes);
        // 只认 a owl:Class 的声明块（或已入索引的散块）
        let is_class_decl = parse_pred_objs(&stmt.rest).iter().any(|po| {
            pred_full(&po.pred, &prefixes) == RDF_TYPE
                && po.is_iri
                && obj_full_iri(&po.obj, &prefixes) == "http://www.w3.org/2002/07/owl#Class"
        });
        if !is_class_decl && !index.contains_key(&full_iri) {
            continue;
        }
        let mut entry = match index.get(&full_iri) {
            Some(&i) => out[i].clone(),
            None => TtlClassMeta {
                iri: full_iri.clone(),
                ..Default::default()
            },
        };
        for po in parse_pred_objs(&stmt.rest) {
            let pf = pred_full(&po.pred, &prefixes);
            match pf.as_str() {
                RDFS_LABEL if po.is_literal && entry.label.is_empty() => {
                    entry.label = parse_literal(&po.obj);
                }
                RDFS_COMMENT if po.is_literal && entry.comment.is_empty() => {
                    entry.comment = parse_literal(&po.obj);
                }
                _ => {}
            }
        }
        if let Some(&i) = index.get(&full_iri) {
            out[i] = entry;
        } else {
            index.insert(full_iri, out.len());
            out.push(entry);
        }
    }
    out
}

/// 从 TTL **原文**提取建模元素（类 + 两类属性）及其证据注解。
///
/// 与 `ttl.rs::ttl_evidence_extraction`（oxigraph 版）等价，但 element_id 用
/// **原文 token 形态**（无尖括号、双 # 保留，如 `<...#Policy#endDate>` → `...#Policy#endDate`），
/// 与 `list_properties` / `list_class_iris` 输出一致——`element_evidence` 落库后
/// 前端 `statusOf` 的 `r.element_id === iri` 精确匹配才能命中（EVIDENCE-CHAIN L4）。
///
/// oxigraph 版的问题：`Term::to_string()` 带尖括号，且双 # 在喂 oxigraph 前已被
/// `legalize_double_hash_iris` 合法化成点号 → element_id 与前端原文形态对不上，
/// 证据行永远匹配不到（⚪ 而非 🟢）。本函数绕开 oxigraph，直接 tokenize 原文。
///
/// 返回 `(elements, notes_by_element)`：element = (element_id, element_type)，
/// element_type ∈ "class" | "datatype_property" | "object_property"。
pub(crate) fn evidence_extraction_from_text(
    ttl: &str,
) -> (
    Vec<(String, String)>,
    std::collections::HashMap<String, Vec<String>>,
) {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut elements: Vec<(String, String)> = Vec::new();
    let mut notes_by_element: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        let full_iri = subject_full_iri(subject, &prefixes);
        let mut etype: Option<&'static str> = None;
        for po in parse_pred_objs(&stmt.rest) {
            let pf = pred_full(&po.pred, &prefixes);
            if pf == RDF_TYPE && po.is_iri {
                let obj = obj_full_iri(&po.obj, &prefixes);
                if obj == OWL_CLASS {
                    etype = Some("class");
                } else if obj == OWL_DATATYPE_PROPERTY {
                    etype = Some("datatype_property");
                } else if obj == OWL_OBJECT_PROPERTY {
                    etype = Some("object_property");
                }
            }
            if pf == SKOS_EDITORIAL_NOTE && po.is_literal {
                let note = parse_literal(&po.obj);
                if note.starts_with("evidence:") {
                    notes_by_element.entry(full_iri.clone()).or_default().push(note);
                }
            }
        }
        if let Some(t) = etype {
            if !elements.iter().any(|(id, _)| id == &full_iri) {
                elements.push((full_iri.clone(), t.to_string()));
            }
        }
    }
    (elements, notes_by_element)
}

// ════════════════════════════════════════════════════════════════
// 6. 属性块生成 + 写路径（edit / add / delete）
// ════════════════════════════════════════════════════════════════

/// 生成属性块文本（`pref 4 空格缩进`，末行 `.`）。
/// `kept`：非受管谓词的原文段（如 `rdfs:seeAlso <x>`），原样保留在块尾。
fn render_property_block(
    edit: &TtlPropertyEdit,
    kept: &[String],
    prefixes: &[(String, String)],
) -> String {
    let subj = abbreviate_iri(&edit.iri, prefixes);
    let kind_abbr = abbreviate_iri(
        if edit.kind == "object" {
            OWL_OBJECT_PROPERTY
        } else {
            OWL_DATATYPE_PROPERTY
        },
        prefixes,
    );
    let mut lines = vec![format!("{subj} a {kind_abbr} ;")];
    if !edit.label.is_empty() {
        lines.push(format!("    rdfs:label {} ;", escape_literal(&edit.label)));
    }
    if !edit.comment.is_empty() {
        lines.push(format!("    rdfs:comment {} ;", escape_literal(&edit.comment)));
    }
    if !edit.domain.is_empty() {
        lines.push(format!(
            "    rdfs:domain {} ;",
            abbreviate_iri(&edit.domain, prefixes)
        ));
    }
    if !edit.range.is_empty() {
        lines.push(format!(
            "    rdfs:range {} ;",
            abbreviate_iri(&edit.range, prefixes)
        ));
    }
    for k in kept {
        lines.push(format!("    {k} ;"));
    }
    if let Some(last) = lines.last_mut() {
        if last.ends_with(';') {
            last.pop();
            last.push('.');
        }
    }
    lines.join("\n") + "\n"
}

/// 纯文本 → Turtle 字面量（加引号 + 转义；含中文加 `@zh`——对齐 validate
/// 第 3 类「中文标签必须带 @zh」；纯 ASCII 不加，保持库内既有风格）。
fn escape_literal(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let has_cjk = text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
    if has_cjk {
        format!("\"{escaped}\"@zh")
    } else {
        format!("\"{escaped}\"")
    }
}

/// 收集目标属性全部散块的下标（按出现顺序）。
/// 删除 / 改写类块（`target_statement_indices` 定位全部同主语语句）。
fn target_statement_indices(ttl: &str, iri: &str) -> Vec<usize> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    stmts
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.subject
                .as_ref()
                .map(|sub| subject_full_iri(sub, &prefixes) == iri)
                .unwrap_or(false)
        })
        .map(|(i, _)| i)
        .collect()
}

/// 删除属性时一并清理引用它的 `owl:Restriction` 约束。
///
/// 两种形态都要覆盖（真实本体里都存在）：
/// 1. 内联空白节点：`C rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :p ; ... ]`
/// 2. 具名约束：`:R a owl:Restriction ; owl:onProperty :p ; ...`（整块删）
///
/// 关系删除后，针对它的基数/限定约束已无意义；留着会变成悬空引用，被
/// `check_dangling_property_refs` 判为 error 而阻断整笔编辑落库。
///
/// 实现走原文扫描（`[...]` 是一个 Group token，内部结构不产出 token，无法
/// 用谓词/宾语模型直接摘值）：先按语句切分，再在语句原文里定位 `onProperty`
/// 命中的 `[ ... ]` 区间，连同其前面的谓词一起删除。
fn drop_restrictions_on_property(ttl: &str, prop_iri: &str) -> StoreResult<String> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let targets = |iri: &str| iri_loose_eq(iri, prop_iri);

    // ① 具名约束块：主语自身 rdf:type owl:Restriction 且 onProperty 命中 → 整块删
    let mut named_hits: Vec<usize> = Vec::new();
    for (i, s) in stmts.iter().enumerate() {
        let Some(subject) = &s.subject else { continue };
        let _ = subject;
        let pos = parse_pred_objs(&s.rest);
        let is_restriction = pos.iter().any(|po| {
            pred_full(&po.pred, &prefixes) == RDF_TYPE
                && po.is_iri
                && obj_full_iri(&po.obj, &prefixes) == OWL_RESTRICTION
        });
        if !is_restriction {
            continue;
        }
        if pos.iter().any(|po| {
            pred_full(&po.pred, &prefixes) == OWL_ON_PROPERTY
                && po.is_iri
                && targets(&obj_full_iri(&po.obj, &prefixes))
        }) {
            named_hits.push(i);
        }
    }

    // ② 逐语句重建：先跳过具名块，再在内联约束语句上做值级摘除
    let mut out = String::with_capacity(ttl.len());
    let mut pos = 0usize;
    for (i, s) in stmts.iter().enumerate() {
        if named_hits.contains(&i) {
            out.push_str(&ttl[pos..s.start]);
            pos = s.end;
            continue;
        }
        let raw = &ttl[s.start..s.end];
        let Some(cleaned) = strip_inline_restrictions(raw, prop_iri, &prefixes) else {
            continue;
        };
        out.push_str(&ttl[pos..s.start]);
        // 摘完若已不剩任何「谓词-宾语」，整条丢弃；否则写入处理后原文。
        // （只判非空会把 `:C .` 这种无谓词残句留下来，解析器报
        //   ". is not a valid predicate"，整笔编辑被拒。）
        if statement_has_predicate_object(&cleaned) {
            out.push_str(&cleaned);
        }
        pos = s.end;
    }
    out.push_str(&ttl[pos..]);
    Ok(out)
}

/// 判断一段语句原文是否仍含至少一个 `谓词 宾语`（用于丢弃被摘空的语句）。
fn statement_has_predicate_object(stmt_raw: &str) -> bool {
    let tokens = tokenize(stmt_raw);
    let stmts = statements(stmt_raw, &tokens);
    let Some(s) = stmts.first() else { return false };
    parse_pred_objs(&s.rest).iter().any(|po| {
        // 只留下孤立的谓词（有 pred 无 obj）也不算合法三元组
        !po.obj.trim().is_empty()
    })
}

/// 在一条语句原文里摘掉所有 `onProperty` 命中 prop_iri 的内联 `[ ... ]` 限制值。
///
/// 返回 `None` = 该语句不含目标约束（原样保留）；返回 `Some(文本)` = 处理后的
/// 语句原文（可能已不含任何三元组，调用方据此决定是否整条丢弃）。
fn strip_inline_restrictions(
    stmt_raw: &str,
    prop_iri: &str,
    prefixes: &[(String, String)],
) -> Option<String> {
    if !stmt_raw.contains("onProperty") {
        return None;
    }
    // 找出所有 `[ ... ]` 区间（括号配平，跳过字符串/IRI 内部）
    let ranges = group_bracket_ranges(stmt_raw);
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for (bs, be) in ranges {
        let inner = &stmt_raw[bs + 1..be - 1];
        for po in parse_pred_objs(&tokenize_rest_of(inner)) {
            if pred_full(&po.pred, prefixes) == OWL_ON_PROPERTY
                && po.is_iri
                && iri_loose_eq(&obj_full_iri(&po.obj, prefixes), prop_iri)
            {
                hits.push((bs, be));
                break;
            }
        }
    }
    if hits.is_empty() {
        return None;
    }

    let mut working = stmt_raw.to_string();
    // 从后往前删，避免字节偏移失效
    for (bs, be) in hits.into_iter().rev() {
        // 往前吃掉谓词名（`rdfs:subClassOf` / `owl:equivalentClass` 等）与空白
        let before = &working[..bs];
        let mut start = bs;
        let bb = before.as_bytes();
        while start > 0 && matches!(bb[start - 1], b' ' | b'\t' | b'\r' | b'\n') {
            start -= 1;
        }
        let word_end = start;
        while start > 0 && !matches!(bb[start - 1], b' ' | b'\t' | b'\r' | b'\n' | b';' | b',') {
            start -= 1;
        }
        let word = &before[start..word_end];
        let drop_pred = matches!(
            pred_full(word, prefixes).as_str(),
            RDFS_SUBCLASSOF | OWL_EQUIVALENT_CLASS
        );
        if !drop_pred {
            start = word_end;
        }
        // 往后吃掉空白与分隔符
        let mut end = be;
        let bytes = working.as_bytes();
        while end < bytes.len() && matches!(bytes[end], b' ' | b'\t' | b'\r' | b'\n' | b',' | b';')
        {
            end += 1;
        }
        // 往前再吃掉紧邻的 `;`（避免留下空谓词）
        let mut s2 = start;
        while s2 > 0 && matches!(working.as_bytes()[s2 - 1], b' ' | b'\t' | b'\r' | b'\n') {
            s2 -= 1;
        }
        if s2 > 0 && working.as_bytes()[s2 - 1] == b';' {
            start = s2 - 1;
        }
        let mut rep = String::with_capacity(working.len());
        rep.push_str(&working[..start]);
        rep.push_str(&working[end..]);
        working = rep;
    }
    Some(working)
}

/// 语句原文中所有顶层/嵌套 `[ ... ]` 的字节区间（含方括号本身）。
fn group_bracket_ranges(s: &str) -> Vec<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut i = 0usize;
    let mut in_str: Option<u8> = None;
    let mut in_iri = false;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        if in_iri {
            if c == b'>' {
                in_iri = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => in_str = Some(c),
            b'<' => in_iri = true,
            b'[' => stack.push(i),
            b']' => {
                if let Some(s0) = stack.pop() {
                    out.push((s0, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// 取一段空白节点内容的谓语/宾语序列（用于判断 onProperty 目标）。
fn tokenize_rest_of(inner: &str) -> Vec<Token> {
    tokenize(inner)
}
/// 块内非受管谓词（label/comment/domain/range/a 之外的）原文保留。
pub(crate) fn edit_property(ttl: &str, edit: &TtlPropertyEdit) -> StoreResult<String> {
    let targets = target_statement_indices(ttl, &edit.iri);
    if targets.is_empty() {
        return Err(StoreError::NotFound(format!(
            "属性 '{}' 未找到（无法编辑）",
            edit.iri
        )));
    }
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    // 受管谓词全丢弃由 edit 重生成；其余（rdfs:seeAlso 等）原文段保留
    let mut kept: Vec<String> = Vec::new();
    for &ti in &targets {
        for po in parse_pred_objs(&stmts[ti].rest) {
            let pf = pred_full(&po.pred, &prefixes);
            let managed = matches!(
                pf.as_str(),
                RDF_TYPE | RDFS_LABEL | RDFS_COMMENT | RDFS_DOMAIN | RDFS_RANGE
            );
            if !managed {
                kept.push(format!("{} {}", po.pred, po.obj_raw));
            }
        }
    }
    let rendered = render_property_block(edit, &kept, &prefixes);
    // 手术：首块替换，其余块区间剔除
    let first = &stmts[targets[0]];
    let mut out = String::with_capacity(ttl.len() + rendered.len());
    out.push_str(&ttl[..first.start]);
    out.push_str(&rendered);
    let mut cursor = first.end;
    for &ti in &targets[1..] {
        let b = &stmts[ti];
        out.push_str(&ttl[cursor..b.start]);
        cursor = b.end;
    }
    out.push_str(&ttl[cursor..]);
    Ok(out)
}

/// 新增属性块。插入点：最后一个属性块之后（同类聚集）；无属性块时
/// 插在最后一个主语块之后；空文件报错。
pub(crate) fn add_property(ttl: &str, edit: &TtlPropertyEdit) -> StoreResult<String> {
    if edit.iri.is_empty() {
        return Err(StoreError::Other(anyhow::anyhow!("新增属性缺少 IRI")));
    }
    let existing = list_properties(ttl);
    if existing.iter().any(|p| p.iri == edit.iri) {
        return Err(StoreError::Other(anyhow::anyhow!(
            "属性 '{}' 已存在（应走编辑而非新增）",
            edit.iri
        )));
    }
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let last = stmts
        .last()
        .ok_or_else(|| StoreError::Other(anyhow::anyhow!("原文无任何主语块，无法插入")))?;
    // 插入点：从后往前找第一个属性块；没有则最后一个语句块
    let mut insert_at = last.end;
    for stmt in stmts.iter().rev() {
        if property_kind(stmt, &prefixes).is_some() {
            insert_at = stmt.end;
            break;
        }
    }
    let rendered = render_property_block(edit, &[], &prefixes);
    let mut out = String::with_capacity(ttl.len() + rendered.len());
    out.push_str(&ttl[..insert_at]);
    out.push_str(&rendered);
    out.push_str(&ttl[insert_at..]);
    Ok(out)
}

/// 删除属性（同主语散块全删）。针对它的 `owl:Restriction` 约束一并摘除——
/// 否则留下悬空 `owl:onProperty`，校验会拒绝整笔落库（删类/删属性都被挡住）。
pub(crate) fn delete_property(ttl: &str, iri: &str) -> StoreResult<String> {
    let targets = target_statement_indices(ttl, iri);
    if targets.is_empty() {
        return Err(StoreError::NotFound(format!(
            "属性 '{iri}' 未找到（无法删除）"
        )));
    }
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out = String::with_capacity(ttl.len());
    let mut cursor = 0;
    for &ti in &targets {
        let b = &stmts[ti];
        out.push_str(&ttl[cursor..b.start]);
        cursor = b.end;
    }
    out.push_str(&ttl[cursor..]);
    drop_restrictions_on_property(&out, iri)
}

/// 渲染类声明块：`subj a owl:Class ; label? comment? subClassOf? .` + kept 原文谓词。
fn render_class_block(
    edit: &TtlClassEdit,
    kept: &[String],
    prefixes: &[(String, String)],
) -> String {
    let subj = abbreviate_iri(&edit.iri, prefixes);
    let class_abbr = abbreviate_iri(OWL_CLASS, prefixes);
    // 首行同样带 `;`：否则后面接 label/comment/subClassOf 时缺谓词-宾语分隔符，
    // 产出非法 Turtle（surgery 校验会失败/静默丢字段）。
    let mut lines = vec![format!("{subj} a {class_abbr} ;")];
    if !edit.label.is_empty() {
        lines.push(format!("    rdfs:label {} ;", escape_literal(&edit.label)));
    }
    if !edit.comment.is_empty() {
        lines.push(format!("    rdfs:comment {} ;", escape_literal(&edit.comment)));
    }
    if !edit.parents.is_empty() {
        let objs = edit
            .parents
            .iter()
            .map(|p| abbreviate_iri(p, prefixes))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("    rdfs:subClassOf {objs} ;"));
    }
    for k in kept {
        lines.push(format!("    {k} ;"));
    }
    // 收尾：最后一个 `;` 换成 `.`（kept 为空且 parents/label/comment 全空时也要闭合）
    let mut out = lines.join("\n");
    if let Some(stripped) = out.strip_suffix(" ;") {
        out = format!("{stripped} .");
    } else {
        out.push_str(" .");
    }
    out.push('\n');
    out
}

/// 编辑类声明块（label/comment/subClassOf 受管重写，散块合并回首块；其余谓词原样保留）。
/// 与 edit_property 同构：目标按 subject IRI 匹配，要求目标确为 `a owl:Class` 声明。
pub(crate) fn edit_class(ttl: &str, edit: &TtlClassEdit) -> StoreResult<String> {
    let targets = target_statement_indices(ttl, &edit.iri);
    if targets.is_empty() {
        return Err(StoreError::NotFound(format!(
            "类 '{}' 未找到（无法编辑）",
            edit.iri
        )));
    }
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let is_class = parse_pred_objs(&stmts[targets[0]].rest).iter().any(|po| {
        pred_full(&po.pred, &prefixes) == RDF_TYPE
            && po.is_iri
            && obj_full_iri(&po.obj, &prefixes) == OWL_CLASS
    });
    if !is_class {
        return Err(StoreError::Other(anyhow::anyhow!(
            "'{}' 不是 owl:Class 声明（类编辑仅作用于类块）",
            edit.iri
        )));
    }
    let mut kept: Vec<String> = Vec::new();
    for &ti in &targets {
        for po in parse_pred_objs(&stmts[ti].rest) {
            let pf = pred_full(&po.pred, &prefixes);
            let managed =
                matches!(pf.as_str(), RDF_TYPE | RDFS_LABEL | RDFS_COMMENT | RDFS_SUBCLASSOF);
            if !managed {
                kept.push(format!("{} {}", po.pred, po.obj_raw));
            }
        }
    }
    let rendered = render_class_block(edit, &kept, &prefixes);
    let first = &stmts[targets[0]];
    let mut out = String::with_capacity(ttl.len() + rendered.len());
    out.push_str(&ttl[..first.start]);
    out.push_str(&rendered);
    let mut cursor = first.end;
    for &ti in &targets[1..] {
        let b = &stmts[ti];
        out.push_str(&ttl[cursor..b.start]);
        cursor = b.end;
    }
    out.push_str(&ttl[cursor..]);
    Ok(out)
}

/// 提取某主语在 TTL 中的全部原始语句块（含结尾 `.`），按出现顺序拼接。
/// 用于「从生效版恢复已删元素」时原样迁回工作区。
pub(crate) fn extract_subject_raw_blocks(ttl: &str, iri: &str) -> StoreResult<String> {
    let targets = target_statement_indices(ttl, iri);
    if targets.is_empty() {
        return Err(StoreError::NotFound(format!(
            "主语 '{iri}' 在对照文本中未找到"
        )));
    }
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out = String::new();
    for &ti in &targets {
        let b = &stmts[ti];
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(ttl[b.start..b.end].trim());
        if !out.ends_with('.') {
            out.push('.');
        }
    }
    Ok(out)
}

/// 将原始语句块追加到 TTL 末尾（用于恢复已删元素）。
pub(crate) fn append_raw_blocks(ttl: &str, blocks: &str) -> String {
    let trimmed = ttl.trim_end();
    if blocks.trim().is_empty() {
        return trimmed.to_string();
    }
    format!("{trimmed}\n\n{}\n", blocks.trim())
}

/// 删除类声明块（同主语散块全删）。引用检查在 store 层做（删除前）。
pub(crate) fn delete_class(ttl: &str, iri: &str) -> StoreResult<String> {
    let targets = target_statement_indices(ttl, iri);
    if targets.is_empty() {
        return Err(StoreError::NotFound(format!(
            "类 '{iri}' 未找到（无法删除）"
        )));
    }
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out = String::with_capacity(ttl.len());
    let mut cursor = 0;
    for &ti in &targets {
        let b = &stmts[ti];
        out.push_str(&ttl[cursor..b.start]);
        cursor = b.end;
    }
    out.push_str(&ttl[cursor..]);
    Ok(out)
}

/// IRI 末段（local name）；缩写形（`:Route` / `ns:Route`）再剥一层前缀。
fn iri_local_name(iri: &str) -> &str {
    let tail = iri.rsplit(['#', '/']).next().unwrap_or(iri);
    tail.rsplit(':').next().unwrap_or(tail)
}

/// 松散 IRI 相等（末段 local name 相同）；空串一律不相等。
fn iri_loose_eq(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let sa = iri_local_name(a);
    let sb = iri_local_name(b);
    !sa.is_empty() && sa == sb
}

/// 收集属性全部 `rdfs:domain`（多定义域；与 list_properties 首见值不同）。
pub(crate) fn collect_property_domains(ttl: &str, prop_iri: &str) -> Vec<String> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::<String>::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        if subject_full_iri(subject, &prefixes) != prop_iri {
            continue;
        }
        for po in parse_pred_objs(&stmt.rest) {
            if pred_full(&po.pred, &prefixes) != RDFS_DOMAIN || !po.is_iri {
                continue;
            }
            let d = obj_full_iri(&po.obj, &prefixes);
            if !d.is_empty() && seen.insert(d.clone()) {
                out.push(d);
            }
        }
    }
    out
}

/// 重写属性定义域列表（可多个）；保留 label/comment/range/非受管谓词。
fn set_property_domains(ttl: &str, prop_iri: &str, domains: &[String]) -> StoreResult<String> {
    let props = list_properties(ttl);
    let Some(mut edit) = props.into_iter().find(|p| p.iri == prop_iri) else {
        return Err(StoreError::NotFound(format!(
            "属性 '{prop_iri}' 未找到（无法改定义域）"
        )));
    };
    let prefixes = extract_prefixes(ttl);
    let targets = target_statement_indices(ttl, prop_iri);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut kept: Vec<String> = Vec::new();
    for &ti in &targets {
        for po in parse_pred_objs(&stmts[ti].rest) {
            let pf = pred_full(&po.pred, &prefixes);
            let managed = matches!(
                pf.as_str(),
                RDF_TYPE | RDFS_LABEL | RDFS_COMMENT | RDFS_DOMAIN | RDFS_RANGE
            );
            if !managed {
                kept.push(format!("{} {}", po.pred, po.obj_raw));
            }
        }
    }
    edit.domain = domains.first().cloned().unwrap_or_default();
    for d in domains.iter().skip(1) {
        kept.push(format!("rdfs:domain {}", abbreviate_iri(d, &prefixes)));
    }
    let rendered = render_property_block(&edit, &kept, &prefixes);
    let first = &stmts[targets[0]];
    let mut out = String::with_capacity(ttl.len() + rendered.len());
    out.push_str(&ttl[..first.start]);
    out.push_str(&rendered);
    let mut cursor = first.end;
    for &ti in &targets[1..] {
        let b = &stmts[ti];
        out.push_str(&ttl[cursor..b.start]);
        cursor = b.end;
    }
    out.push_str(&ttl[cursor..]);
    Ok(out)
}

/// 卡片硬删类：按业务规则级联后删类（不因属性/关系引用拒绝）。
/// - datatype：唯一定义域为 C → 删属性；多定义域含 C → 摘掉 C
/// - object：domain 或 range 触及 C → 删整条关系
/// - 其他类以 C 为父的 subClassOf → 从父类列表去掉 C
/// - 最后删类块；若仍有无法清理的引用（如实例 rdf:type）→ 人话错误
pub(crate) fn delete_class_cascade(ttl: &str, class_iri: &str) -> StoreResult<String> {
    let metas = list_class_metas(ttl);
    // 规范 IRI：前端给的就是文档里的全形 IRI 时严格比较（不同命名空间的同名类
    // 不被误删）；只有拿到缩写/短名时才退回 local name 兜底匹配。
    let strict = metas.iter().any(|m| m.iri == class_iri);
    let class_iri: String = if strict {
        class_iri.to_string()
    } else {
        metas
            .iter()
            .find(|m| iri_loose_eq(&m.iri, class_iri))
            .map(|m| m.iri.clone())
            .unwrap_or_else(|| class_iri.to_string())
    };
    let is_c = |iri: &str| {
        if strict {
            iri == class_iri
        } else {
            iri_loose_eq(iri, &class_iri)
        }
    };

    let mut ws = ttl.to_string();
    for p in list_properties(&ws) {
        let domains = collect_property_domains(&ws, &p.iri);
        let domain_hits = domains.iter().any(|d| is_c(d));
        if p.kind == "object" {
            if domain_hits || is_c(&p.range) {
                ws = delete_property(&ws, &p.iri)?;
            }
            continue;
        }
        // datatype（及其他非 object）：独占则删，共享则仅摘掉 C
        if !domain_hits {
            continue;
        }
        let others: Vec<String> = domains.into_iter().filter(|d| !is_c(d)).collect();
        if others.is_empty() {
            ws = delete_property(&ws, &p.iri)?;
        } else {
            ws = set_property_domains(&ws, &p.iri, &others)?;
        }
    }

    // 其他类以 C 为父：从各自父类列表去掉 C（保留其余父类）
    let children_of_c: Vec<String> = list_subclass_of_edges(&ws)
        .into_iter()
        .filter(|(_, parent)| is_c(parent))
        .map(|(child, _)| child)
        .filter(|child| !is_c(child))
        .collect();
    for child in children_of_c {
        let meta = metas.iter().find(|m| m.iri == child);
        let parents: Vec<String> = list_subclass_of_edges(&ws)
            .into_iter()
            .filter(|(c, _)| c == &child)
            .map(|(_, p)| p)
            .filter(|p| !is_c(p))
            .collect();
        let edit = TtlClassEdit {
            iri: child,
            label: meta.map(|m| m.label.clone()).unwrap_or_default(),
            comment: meta.map(|m| m.comment.clone()).unwrap_or_default(),
            parents,
        };
        ws = edit_class(&ws, &edit)?;
    }

    // 属性/关系/继承已清；剩余（实例数据、等价类等）需用户自行处理，
    // 错误只给业务语言，不列出引用方 IRI。
    if !find_class_references(&ws, &class_iri).is_empty() {
        return Err(StoreError::Other(anyhow::anyhow!(
            "该类仍被其他定义引用，这里无法自动清理，请先移除相关引用后再删除"
        )));
    }
    delete_class(&ws, &class_iri)
}

/// 类引用检查：全原文扫描 iri 被谁使用（排除它自己的声明块）。
/// 返回人类可读引用描述（非空则删除应被拒绝）。
pub(crate) fn find_class_references(ttl: &str, iri: &str) -> Vec<String> {
    let prefixes = extract_prefixes(ttl);
    let tokens = tokenize(ttl);
    let stmts = statements(ttl, &tokens);
    let mut refs: Vec<String> = Vec::new();
    for stmt in &stmts {
        let Some(subject) = &stmt.subject else { continue };
        let subj_iri = subject_full_iri(subject, &prefixes);
        if subj_iri == iri {
            continue; // 自身声明块不算引用
        }
        for po in parse_pred_objs(&stmt.rest) {
            let pf = pred_full(&po.pred, &prefixes);
            let used = match pf.as_str() {
                p if p == RDFS_DOMAIN || p == RDFS_RANGE || p == RDFS_SUBCLASSOF => {
                    po.is_iri && obj_full_iri(&po.obj, &prefixes) == iri
                }
                p if p == RDF_TYPE => po.is_iri && obj_full_iri(&po.obj, &prefixes) == iri,
                _ => false,
            };
            if used {
                let what = match pf.as_str() {
                    p if p == RDFS_DOMAIN => "rdfs:domain",
                    p if p == RDFS_RANGE => "rdfs:range",
                    p if p == RDFS_SUBCLASSOF => "rdfs:subClassOf（子类）",
                    _ => "rdf:type（实例）",
                };
                refs.push(format!("{subj_iri} 的 {what}"));
            }
        }
    }
    refs
}

// ════════════════════════════════════════════════════════════════
// 测试
// ════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/ontology/test#> .

:TestOntology a owl:Ontology ;
    rdfs:label "测试本体"@zh ;
    owl:versionInfo "1.0.0" .

:Supplier a owl:Class ;
    rdfs:label "供应商"@zh ;
    rdfs:comment "采购供应商 [confirmed]"@zh .

:supplierId a owl:DatatypeProperty ;
    rdfs:domain :Supplier ;
    rdfs:range xsd:string ;
    rdfs:label "供应商编号"@zh .

# 散块：同主语的第二段
:supplierId rdfs:comment "编号备注" .

:supplies a owl:ObjectProperty ;
    rdfs:domain :Supplier ;
    rdfs:range :Supplier ;
    rdfs:label "供货"@zh .
"#
    }

    /// 双 # IRI 原文（外部导出形态，落库存原文）。
    fn sample_double_hash() -> &'static str {
        r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Ont> a owl:Ontology .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy> a owl:Class ;
    rdfs:label "保单"@zh .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate>
        rdf:type      owl:DatatypeProperty;
        rdfs:comment  "结束日期 [tentative]";
        rdfs:domain   <https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy>;
        rdfs:label    "结束日期";
        rdfs:range    xsd:dateTime .
"#
    }

    #[test]
    fn list_properties_basic() {
        let props = list_properties(sample());
        assert_eq!(props.len(), 2, "props: {props:?}");
        let sid = props
            .iter()
            .find(|p| p.iri.ends_with("#supplierId"))
            .unwrap();
        assert_eq!(sid.kind, "datatype");
        assert_eq!(sid.label, "供应商编号");
        assert_eq!(sid.comment, "编号备注", "散块 comment 应合并");
        assert_eq!(sid.domain, "https://example.org/ontology/test#Supplier");
        assert_eq!(sid.range, "http://www.w3.org/2001/XMLSchema#string");
        let sup = props
            .iter()
            .find(|p| p.iri.ends_with("#supplies"))
            .unwrap();
        assert_eq!(sup.kind, "object");
    }

    #[test]
    fn list_properties_double_hash_iri() {
        let props = list_properties(sample_double_hash());
        assert_eq!(props.len(), 1, "props: {props:?}");
        assert_eq!(
            props[0].iri,
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate"
        );
        assert_eq!(props[0].label, "结束日期");
        assert_eq!(
            props[0].domain,
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy"
        );
    }

    #[test]
    fn list_class_iris_basic() {
        let classes = list_class_iris(sample());
        assert_eq!(classes, vec!["https://example.org/ontology/test#Supplier"]);
    }

    #[test]
    fn default_namespace_prefers_existing_properties() {
        // 属性投票多数派：两个 ns: 属性 vs 本体 IRI 在另一 namespace
        // → 默认前缀应与既有属性一致，而非本体 IRI 兜底
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ns: <https://jslc.auto-retail.cn/ontology/AutoDealerRetail#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
ns:a a owl:DatatypeProperty .
ns:b a owl:DatatypeProperty .
"#;
        let ns = default_property_namespace(ttl, "urn:something#TestOntology");
        assert_eq!(
            ns,
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#"
        );
    }

    #[test]
    fn default_namespace_falls_back_to_ontology_iri() {
        // 无已声明属性、无 ns: 前缀 → 本体 IRI 的 # 前缀（旧行为）
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class .
"#;
        let ns = default_property_namespace(ttl, "urn:jslc.auto-retail.cn#TestOntology");
        assert_eq!(ns, "urn:jslc.auto-retail.cn#");
    }

    #[test]
    fn edit_property_rewrites_block_and_keeps_rest() {
        let ttl = sample();
        let edited = edit_property(
            ttl,
            &TtlPropertyEdit {
                iri: "https://example.org/ontology/test#supplierId".into(),
                kind: "datatype".into(),
                label: "供应商编号2".into(),
                comment: "新备注".into(),
                domain: "https://example.org/ontology/test#Supplier".into(),
                range: "http://www.w3.org/2001/XMLSchema#string".into(),
            },
        )
        .unwrap();
        // 类块不动（字节级）
        assert!(edited.contains(":Supplier a owl:Class ;"));
        assert!(edited.contains("采购供应商 [confirmed]"));
        // 关系块不动
        assert!(edited.contains(":supplies a owl:ObjectProperty ;"));
        // 新块内容
        assert!(edited.contains(":supplierId a owl:DatatypeProperty ;"));
        assert!(edited.contains("供应商编号2"));
        assert!(edited.contains("新备注"));
        // 散块被合并（原散块 comment 消失）
        assert!(!edited.contains("编号备注"));
        // 重新 list 应读到新值
        let props = list_properties(&edited);
        let sid = props
            .iter()
            .find(|p| p.iri.ends_with("#supplierId"))
            .unwrap();
        assert_eq!(sid.label, "供应商编号2");
        assert_eq!(sid.comment, "新备注");
    }

    #[test]
    fn edit_property_keeps_unknown_predicates() {
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .

:p a owl:DatatypeProperty ;
    rdfs:label "p"@zh ;
    rdfs:seeAlso <https://example.org/see> .
"#;
        let edited = edit_property(
            ttl,
            &TtlPropertyEdit {
                iri: "https://example.org/ontology/test#p".into(),
                kind: "datatype".into(),
                label: "p2".into(),
                comment: String::new(),
                domain: String::new(),
                range: String::new(),
            },
        )
        .unwrap();
        assert!(
            edited.contains("rdfs:seeAlso <https://example.org/see>"),
            "未知谓词应保留: {edited}"
        );
    }

    #[test]
    fn edit_property_not_found() {
        let err = edit_property(
            sample(),
            &TtlPropertyEdit {
                iri: "https://example.org/ontology/test#nope".into(),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn add_property_inserts_after_last_property() {
        let ttl = sample();
        let added = add_property(
            ttl,
            &TtlPropertyEdit {
                iri: "https://example.org/ontology/test#note".into(),
                kind: "datatype".into(),
                label: "备注".into(),
                comment: String::new(),
                domain: "https://example.org/ontology/test#Supplier".into(),
                range: "http://www.w3.org/2001/XMLSchema#string".into(),
            },
        )
        .unwrap();
        // 新块出现在 supplies 块之后（末尾）
        let pos_new = added.find(":note a").unwrap();
        let pos_sup = added.find(":supplies a").unwrap();
        assert!(pos_new > pos_sup);
        // 原有内容全保留
        assert!(added.contains(":TestOntology a owl:Ontology ;"));
        assert!(added.contains(":Supplier a owl:Class ;"));
        // 重新 list 三条
        assert_eq!(list_properties(&added).len(), 3);
    }

    #[test]
    fn add_property_duplicate_rejected() {
        let err = add_property(
            sample(),
            &TtlPropertyEdit {
                iri: "https://example.org/ontology/test#supplierId".into(),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn delete_property_removes_all_scattered_blocks() {
        let ttl = sample();
        let deleted = delete_property(ttl, "https://example.org/ontology/test#supplierId").unwrap();
        assert!(!deleted.contains("supplierId"));
        assert!(!deleted.contains("编号备注"));
        // 其他块保留
        assert!(deleted.contains(":Supplier a owl:Class ;"));
        assert!(deleted.contains(":supplies a owl:ObjectProperty ;"));
        assert_eq!(list_properties(&deleted).len(), 1);
    }

    #[test]
    fn delete_property_not_found() {
        assert!(delete_property(sample(), "https://example.org/ontology/test#nope").is_err());
    }

    #[test]
    fn edit_double_hash_iri_roundtrip() {
        let ttl = sample_double_hash();
        let edited = edit_property(
            ttl,
            &TtlPropertyEdit {
                iri: "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate"
                    .into(),
                kind: "datatype".into(),
                label: "保单结束日期".into(),
                comment: String::new(),
                domain: "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy".into(),
                range: "http://www.w3.org/2001/XMLSchema#dateTime".into(),
            },
        )
        .unwrap();
        // 双 # IRI 保持全形（不缩写为非法 pname）
        assert!(
            edited.contains("<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate>"),
            "双 # IRI 应保持全形: {edited}"
        );
        // 类块不动
        assert!(edited.contains("<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy> a owl:Class ;"));
        // 重新读回
        let props = list_properties(&edited);
        assert_eq!(props[0].label, "保单结束日期");
    }

    #[test]
    fn tokenizer_handles_tricky_content() {
        // 字符串含 . ; # "、嵌套 []、多行字符串
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .

:p a owl:DatatypeProperty ;
    rdfs:comment "含.;#\"符号"@zh ;
    rdfs:comment """多行
注释""" ;
    :nested [ a owl:Thing ; :deep "v" ] .

:q a owl:ObjectProperty .
"#;
        let props = list_properties(ttl);
        assert_eq!(props.len(), 2);
        let p = props.iter().find(|x| x.iri.ends_with("#p")).unwrap();
        // parse_literal 不解转义（读层保持原文转义形态；写回时 escape_literal 再转义）
        assert_eq!(p.comment, "含.;#\\\"符号", "首个 comment 生效");
        // 顶层谓词 :nested 的 Group 宾语不干扰 label/comment 提取
    }

    #[test]
    fn prefix_extraction_and_abbreviation() {
        let prefixes = extract_prefixes(sample());
        assert!(prefixes.contains(&(
            ":".to_string(),
            "https://example.org/ontology/test#".to_string()
        )));
        let abbr = abbreviate_iri("https://example.org/ontology/test#Supplier", &prefixes);
        assert_eq!(abbr, ":Supplier");
        // 双 # IRI 不缩写
        let full = abbreviate_iri(
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate",
            &[(
                "auto:".to_string(),
                "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#".to_string(),
            )],
        );
        assert_eq!(
            full,
            "<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate>"
        );
    }

    #[test]
    fn evidence_extraction_from_text_double_hash_and_type() {
        // 双 # IRI + evidence 注解：element_id 必须保持原文形态（无尖括号、双 #），
        // element_type 正确判定 datatype_property——这是前端 🟢 匹配的前提。
        let ttl = r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Ont> a owl:Ontology .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy> a owl:Class ;
    rdfs:label "保单"@zh ;
    skos:editorialNote "evidence: src-abc | direct | 「保单对象」" .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate>
        rdf:type      owl:DatatypeProperty;
        rdfs:comment  "结束日期 [tentative]";
        rdfs:domain   <https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy>;
        rdfs:label    "结束日期";
        rdfs:range    xsd:dateTime ;
        skos:editorialNote "evidence: src-abc | direct | 「结束日期」" .
"#;
        let (elements, notes_by_element) = evidence_extraction_from_text(ttl);
        // 两个元素：类 + datatype_property
        let cls = elements
            .iter()
            .find(|(id, _)| id.ends_with("AutoDealerRetail#Policy"))
            .expect("类应在 elements 中");
        assert_eq!(cls.1, "class");
        let prop = elements
            .iter()
            .find(|(id, _)| id.contains("#Policy#endDate"))
            .expect("属性应在 elements 中");
        assert_eq!(prop.1, "datatype_property");
        // element_id 必须是原文形态：无尖括号、双 # 保留
        assert_eq!(
            prop.0,
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Policy#endDate"
        );
        // 注解挂在对应元素下
        assert_eq!(
            notes_by_element.get(&prop.0).map(|v| v.len()),
            Some(1),
            "属性证据注解应被提取"
        );
        assert!(notes_by_element.get(&prop.0).unwrap()[0].starts_with("evidence:"));
    }
    #[test]
    fn edit_class_rewrites_managed_and_keeps_rest() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
:Policy a owl:Class ;
    rdfs:label "保单" ;
    rdfs:seeAlso <https://ext.example/ref> .
:Company a owl:Class .
"#;
        let edited = edit_class(
            ttl,
            &TtlClassEdit {
                iri: "https://x.org/t#Policy".into(),
                label: "保险单".into(),
                comment: "核心业务对象".into(),
                parents: vec!["https://x.org/t#Contract".into()],
            },
        )
        .unwrap();
        assert!(edited.contains("rdfs:label \"保险单\""));
        assert!(edited.contains("rdfs:comment \"核心业务对象\""));
        assert!(edited.contains("rdfs:subClassOf :Contract"));
        // 未受管谓词原样保留
        assert!(edited.contains("rdfs:seeAlso <https://ext.example/ref>"));
        // 其他类块不动
        assert!(edited.contains(":Company a owl:Class"));
        // 原有的空 label/comment/subClassOf 不再重复
        assert!(!edited.contains("\"保单\""));
        // 块结构合法：`a owl:Class ;` 起头，后续谓词可被重新解析回来
        assert!(edited.contains(":Policy a owl:Class ;"), "edited: {edited}");
        let metas = list_class_metas(&edited);
        let policy = metas
            .iter()
            .find(|m| m.iri == "https://x.org/t#Policy")
            .expect("Policy 元数据应可回读");
        assert_eq!(policy.label, "保险单");
        assert_eq!(policy.comment, "核心业务对象");
        assert!(
            list_subclass_of_edges(&edited)
                .iter()
                .any(|(c, p)| c == "https://x.org/t#Policy"
                    && p == "https://x.org/t#Contract"),
            "父类边应可回读: {edited}"
        );
    }

    #[test]
    fn edit_class_rejects_non_class_subject() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
:holders a owl:ObjectProperty ;
    rdfs:domain :Policy .
:Policy a owl:Class .
"#;
        let err = edit_class(
            ttl,
            &TtlClassEdit {
                iri: "https://x.org/t#holders".into(),
                ..Default::default()
            },
        );
        assert!(err.is_err());
    }

    #[test]
    fn find_class_references_covers_domain_type_subclass() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
:Policy a owl:Class .
:holder a owl:Class ; rdfs:subClassOf :Party .
:insures a owl:ObjectProperty ; rdfs:domain :Policy ; rdfs:range :Party .
:alice a :Party .
"#;
        let refs = find_class_references(ttl, "https://x.org/t#Party");
        assert_eq!(refs.len(), 3, "subClassOf + range + rdf:type：{refs:?}");
        // 自身声明块不算引用
        assert!(find_class_references(ttl, "https://x.org/t#Policy")
            .iter()
            .all(|r| !r.contains("Policy a") && !r.contains("Policy 的")));
    }

    #[test]
    fn delete_class_removes_block_only() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
:Policy a owl:Class ;
    rdfs:label "保单" .
:Company a owl:Class .
"#;
        let deleted = delete_class(ttl, "https://x.org/t#Policy").unwrap();
        assert!(!deleted.contains(":Policy"));
        assert!(deleted.contains(":Company a owl:Class"));
    }

    #[test]
    fn delete_property_drops_named_restriction_block() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:TestDrive a owl:Class .
:convertsLead a owl:ObjectProperty ;
    rdfs:domain :TestDrive ;
    rdfs:range :Lead .
:R a owl:Restriction ;
    owl:onProperty :convertsLead ;
    owl:minCardinality "1"^^xsd:nonNegativeInteger .
"#;
        let out = delete_property(ttl, "https://x.org/t#convertsLead").unwrap();
        assert!(!out.contains(":convertsLead a owl:ObjectProperty"));
        assert!(!out.contains("owl:onProperty"), "具名约束块应一并删除: {out}");
        assert!(out.contains(":TestDrive a owl:Class"));
    }

    #[test]
    fn delete_property_drops_inline_restriction_value() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:TestDrive a owl:Class .
:Lead a owl:Class .
:convertsLead a owl:ObjectProperty ;
    rdfs:domain :TestDrive ;
    rdfs:range :Lead .
:TestDrive rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :convertsLead ; owl:minCardinality "1"^^xsd:nonNegativeInteger ] .
:TestDrive rdfs:subClassOf :OtherClass .
"#;
        let out = delete_property(ttl, "https://x.org/t#convertsLead").unwrap();
        assert!(!out.contains("convertsLead"), "内联约束值应摘除: {out}");
        assert!(!out.contains("onProperty"), "不应留下悬空 onProperty: {out}");
        // 其他 subClassOf 值必须保留
        assert!(out.contains(":OtherClass"), "无关的 subClassOf 值应保留: {out}");
        assert!(out.contains(":TestDrive a owl:Class"));
    }

    /// 语句只剩被删约束时，整条语句必须丢弃——否则留下 `:C .` 残句，
    /// 解析器报 ". is not a valid predicate"，整笔编辑被拒（真实库踩过）。
    #[test]
    fn delete_property_removes_statement_left_without_predicate() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:TestDrive a owl:Class .
:Lead a owl:Class .
:convertsLead a owl:ObjectProperty ;
    rdfs:domain :TestDrive ;
    rdfs:range :Lead .
:TestDrive rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :convertsLead ; owl:minCardinality "1"^^xsd:nonNegativeInteger ] .
"#;
        let out = delete_property(ttl, "https://x.org/t#convertsLead").unwrap();
        assert!(!out.contains("onProperty"), "约束应整条移除: {out}");
        // 不得留下只有主语 + `.` 的残句
        for line in out.lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            assert!(
                !t.ends_with(" .") || t.matches(char::is_whitespace).count() > 1,
                "出现无谓词残句: {t:?}\n全部输出:\n{out}"
            );
        }
        // 必须仍能被 oxigraph 解析
        use oxigraph::io::{RdfFormat, RdfParser};
        let temp = oxigraph::store::Store::new().unwrap();
        temp.load_from_reader(RdfParser::from_format(RdfFormat::Turtle), out.as_bytes())
            .unwrap_or_else(|e| panic!("输出不是合法 Turtle: {e}\n{out}"));
    }

    #[test]
    fn delete_class_cascade_removes_restrictions_on_deleted_relation() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:TestDrive a owl:Class .
:Lead a owl:Class .
:convertsLead a owl:ObjectProperty ;
    rdfs:domain :TestDrive ;
    rdfs:range :Lead .
:TestDrive rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :convertsLead ; owl:minCardinality "1"^^xsd:nonNegativeInteger ] .
"#;
        let out = delete_class_cascade(ttl, "https://x.org/t#TestDrive").unwrap();
        assert!(!out.contains(":TestDrive a owl:Class"));
        assert!(!out.contains("convertsLead"), "关系删除时其约束应一并清理: {out}");
        assert!(!out.contains("onProperty"), "不得留下悬空 onProperty: {out}");
        assert!(out.contains(":Lead a owl:Class"));
    }

    #[test]
    fn delete_class_cascade_removes_exclusive_attrs_and_relations() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:Route a owl:Class .
:Store a owl:Class .
:name a owl:DatatypeProperty ;
    rdfs:domain :Route ;
    rdfs:range xsd:string .
:belongsTo a owl:ObjectProperty ;
    rdfs:domain :Route ;
    rdfs:range :Store .
"#;
        let out = delete_class_cascade(ttl, "https://x.org/t#Route").unwrap();
        assert!(!out.contains(":Route a owl:Class"));
        assert!(!out.contains(":name a owl:DatatypeProperty"));
        assert!(!out.contains(":belongsTo a owl:ObjectProperty"));
        assert!(out.contains(":Store a owl:Class"));
    }

    #[test]
    fn delete_class_cascade_strips_shared_multi_domain() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:Route a owl:Class .
:Lead a owl:Class .
:note a owl:DatatypeProperty ;
    rdfs:domain :Route ;
    rdfs:domain :Lead ;
    rdfs:range xsd:string .
"#;
        let domains = collect_property_domains(ttl, "https://x.org/t#note");
        assert_eq!(domains.len(), 2);
        let out = delete_class_cascade(ttl, "https://x.org/t#Route").unwrap();
        assert!(!out.contains(":Route a owl:Class"));
        assert!(out.contains(":note a owl:DatatypeProperty"));
        let left = collect_property_domains(&out, "https://x.org/t#note");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0], "https://x.org/t#Lead");
    }

    #[test]
    fn delete_class_cascade_removes_range_side_relation() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
:Lead a owl:Class .
:Route a owl:Class .
:fromLead a owl:ObjectProperty ;
    rdfs:domain :Lead ;
    rdfs:range :Route .
"#;
        let out = delete_class_cascade(ttl, "https://x.org/t#Route").unwrap();
        assert!(!out.contains(":Route a owl:Class"));
        // 关系仅 range 触及 C 也整条删除
        assert!(!out.contains(":fromLead"));
        assert!(out.contains(":Lead a owl:Class"));
    }

    #[test]
    fn delete_class_cascade_keeps_other_parents_of_children() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
:Base a owl:Class .
:Route a owl:Class ;
    rdfs:subClassOf :Base .
:Express a owl:Class ;
    rdfs:subClassOf :Route , :Base .
"#;
        let out = delete_class_cascade(ttl, "https://x.org/t#Route").unwrap();
        assert!(!out.contains(":Route a owl:Class"));
        assert!(!out.contains(":Route ;") && !out.contains(":Route ,"));
        let edges = list_subclass_of_edges(&out);
        assert!(edges
            .iter()
            .any(|(c, p)| c == "https://x.org/t#Express" && p == "https://x.org/t#Base"));
        assert!(!edges
            .iter()
            .any(|(_, p)| p == "https://x.org/t#Route"));
    }

    #[test]
    fn delete_class_cascade_reports_friendly_error_on_instance_refs() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
:Route a owl:Class .
:route1 a :Route .
"#;
        let err = delete_class_cascade(ttl, "https://x.org/t#Route").unwrap_err();
        let msg = err.to_string();
        // 人话错误：不堆引用方 IRI
        assert!(!msg.contains("https://x.org/t#route1"), "msg: {msg}");
        assert!(msg.contains("引用"), "msg: {msg}");
    }

    #[test]
    fn delete_class_cascade_accepts_abbreviated_iri() {
        let ttl = r#"@prefix : <https://x.org/t#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
:Route a owl:Class .
:name a owl:DatatypeProperty ;
    rdfs:domain :Route ;
    rdfs:range xsd:string .
"#;
        let out = delete_class_cascade(ttl, ":Route").unwrap();
        assert!(!out.contains(":Route a owl:Class"));
        assert!(!out.contains(":name"));
    }
}
