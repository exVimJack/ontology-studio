//! 证据链纯逻辑层：Evidence 结构 + quote 归一化匹配 + 闸门校验。
//!
//! 设计文档：`docs/EVIDENCE-CHAIN.md`（决策 24 后续，本体建模证据链）。
//! 核心哲学：**不信任 LLM 的自我声明，只信逐字匹配**——
//! evidence 引用的原文必须在登记的源内容中机械匹配到，匹配失败 = 落库阻断。
//!
//! 三级匹配（容忍格式抖动、不容忍改写）：
//!   1. Exact      —— quote 逐字出现在源内容中
//!   2. Normalized —— 双方做 normalize()（空白折叠 + 全半角统一）后子串匹配
//!   3. None       —— 匹配失败（改写/臆想，阻断）
//!
//! 不做词干/同义/模糊匹配：LLM 把「客户下单后三天未付款」改写成
//! 「用户逾期三天」必须匹配失败——这是本机制的底线。

use std::collections::HashMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::StoreResult;

/// inferred 多段 quote 的拼接分隔符（校验器逐段匹配）。
pub const SEGMENT_SEP: &str = " ⟪…⟫ ";

/// 证据来源登记记录（`evidence_sources` 表的读取视图）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct EvidenceSource {
    pub source_id: String,
    /// 归属本体 api_name；空串 = 全局共享（会话场景源先于本体建立时）。
    #[serde(default)]
    pub ontology_api_name: String,
    /// "scenario" | "document" | "db_table" | "db_column"
    pub kind: String,
    /// 人类可读出处：会话时间 / 文件名+chunk 路径 / 库.表.列。
    pub title: String,
    /// 源文本快照（登记时刻副本，不可变）。
    pub content: String,
    /// 内容指纹（FNV-1a 64，幂等登记判重）。
    pub content_hash: String,
    /// 关联既有设施：document_id / dataset_id / conversation_message_id（可空）。
    #[serde(default)]
    pub origin_ref: String,
    /// unix ms
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

/// 单条证据：元素 ← 源原文的引用。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct Evidence {
    /// 必须指向已登记的 evidence_sources（E1 校验）。
    pub source_id: String,
    /// 原文逐字引用，≤200 字符；inferred 多段时用 SEGMENT_SEP 拼接。
    pub quote: String,
    /// 在源 content 中的字符偏移（可选；提供则做二次确认，不一致按失败处理）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<(u32, u32)>,
    /// direct = 直接原文；inferred = 多段组合推断（需人工确认）。
    #[serde(default)]
    pub role: EvidenceRole,
}

/// 证据角色。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, specta::Type, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceRole {
    #[default]
    Direct,
    Inferred,
}

/// quote 匹配等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchLevel {
    Exact,
    Normalized,
    None,
}

/// FNV-1a 64 位内容指纹（无第三方依赖，跨版本稳定）。
pub fn content_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// CJK 字符判定（用于空格折叠：CJK 文本内/旁的空格是排版抖动，非词间分隔）。
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303F}'   // CJK 标点（。、《》等）
        | '\u{4E00}'..='\u{9FFF}' // CJK 统一表意文字
        | '\u{FF00}'..='\u{FFEF}' // 全角形式（，！？等）
    )
}

/// 单字符归一化映射：全角→半角；空白→空格；其余原样。
fn map_char(ch: char) -> Option<char> {
    match ch {
        '\u{3000}' => Some(' '),
        c @ '\u{FF01}'..='\u{FF5E}' => Some(char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)),
        c if c.is_whitespace() => Some(' '),
        c => Some(c),
    }
}

/// 归一化：trim + 连续空白折叠为单空格 + 全半角统一 + CJK 相邻空格剔除。
///
/// 刻意不做 NFKC 全量折叠/大小写转换——保持「改写即失败」的底线，
/// 只消除 LLM 复制原文时的空白/宽度抖动。
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_ws = false;
    for ch in s.trim().chars() {
        if let Some(c) = map_char(ch) {
            let is_ws = c == ' ';
            // CJK 文本内/旁的空格视为排版抖动直接剔除
            if is_ws && (last_ws || is_cjk(ch)) {
                continue;
            }
            out.push(c);
            last_ws = is_ws;
        }
    }
    out
}

/// 归一化并记录每个输出字符在原文中的字节偏移（用于失败时回溯原文上下文）。
fn normalize_with_map(s: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len());
    let mut last_ws = false;
    for (byte_idx, ch) in s.trim().char_indices() {
        if let Some(c) = map_char(ch) {
            let is_ws = c == ' ';
            if is_ws && (last_ws || is_cjk(ch)) {
                continue;
            }
            out.push(c);
            map.push(byte_idx);
            last_ws = is_ws;
        }
    }
    (out, map)
}

/// quote ↔ 源内容三级匹配。
///
/// `span` 若提供则要求精确匹配窗口与 quote 一致（防止 LLM 给出「指向别处」的假定位）。
pub fn match_quote(content: &str, quote: &str, span: Option<(u32, u32)>) -> MatchLevel {
    if quote.trim().is_empty() {
        return MatchLevel::None;
    }
    // span 二次确认：窗口内容必须原样等于 quote
    if let Some((start, end)) = span {
        let ok = content
            .chars()
            .enumerate()
            .skip(start as usize)
            .take(end.saturating_sub(start) as usize)
            .map(|(_, c)| c)
            .collect::<String>()
            == quote;
        if !ok {
            return MatchLevel::None;
        }
    }
    if content.contains(quote) {
        return MatchLevel::Exact;
    }
    let (nc, _) = normalize_with_map(content);
    let nq = normalize(quote);
    if !nq.is_empty() && nc.contains(&nq) {
        return MatchLevel::Normalized;
    }
    MatchLevel::None
}

/// 匹配失败时提取源原文上下文（指导性打回，docs/EVIDENCE-CHAIN.md 5.3）。
///
/// 在归一化文本中定位 quote 的最佳近似位置，映射回原文字节偏移，
/// 截取前后各 `radius` 字符。agent 下一轮直接从真实原文重新摘录，
/// 而不是在看不到源内容时编造「看起来像原文」的引用。
pub fn quote_context(content: &str, quote: &str, radius: usize) -> Option<String> {
    let (nc, map) = normalize_with_map(content);
    let nq = normalize(quote);
    if nq.is_empty() {
        return None;
    }
    let start = nc.find(&nq)?;
    let start_char = nc[..start].chars().count();
    let begin_byte = map.get(start_char).copied().unwrap_or(0);
    let end_char = start_char + nq.chars().count() - 1;
    let end_byte = map
        .get(end_char)
        .map(|b| b + content[*b..].chars().next().map_or(1, |c| c.len_utf8()))
        .unwrap_or(content.len());
    // 按字符边界扩展半径
    let from = floor_char_boundary(content, begin_byte.saturating_sub(radius));
    let to = ceil_char_boundary(content, (end_byte + radius).min(content.len()));
    let mut ctx = String::new();
    if from > 0 {
        ctx.push('…');
    }
    ctx.push_str(&content[from..to]);
    if to < content.len() {
        ctx.push('…');
    }
    Some(ctx)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// quote 切段（inferred 多段拼接 → 逐段匹配）。
pub fn quote_segments(quote: &str) -> Vec<String> {
    if quote.contains(SEGMENT_SEP) {
        quote
            .split(SEGMENT_SEP)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![quote.to_string()]
    }
}

/// 单条 evidence 的校验结果：返回 (errors, warnings)。
///
/// - E1 源不存在 → error
/// - E2/E3 quote（含 inferred 逐段）匹配失败 → error（附 ±200 字原文上下文）
/// - E3' span 与 quote 不一致 → error（match_quote 内统一按 None 处理，同一 E2 文案）
/// - S1 无 evidence 且 strict → error；非 strict → W1 warning
/// - W2 全部 inferred → warning（非 strict 才产生；strict 下由 S1/匹配兜住）
fn check_one(
    target: &str,
    evidence: &[Evidence],
    sources: &HashMap<String, EvidenceSource>,
    strict: bool,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    if evidence.is_empty() {
        if strict {
            errors.push(format!("「{target}」无任何证据引用（strict 模式阻断）——请补充对应输入原文的 evidence，或向用户确认该元素确属抽象设计"));
        } else {
            warnings.push(format!("「{target}」ungrounded：无证据引用，未经输入佐证"));
        }
        return;
    }
    let mut all_inferred = true;
    for ev in evidence {
        if ev.role == EvidenceRole::Direct {
            all_inferred = false;
        }
        // 豁免引用（与 W3C 注解 evidence: none 对齐）：用户显式确认的人工设计，
        // quote 携带确认理由，闸门放行，落库 matched_by='manual'（persist 层处理）
        if ev.source_id == "none" {
            continue;
        }
        let Some(src) = sources.get(&ev.source_id) else {
            errors.push(format!(
                "「{target}」的 evidence 引用了未登记的源 {}（E1）",
                ev.source_id
            ));
            continue;
        };
        let segments = quote_segments(&ev.quote);
        for seg in &segments {
            if seg.chars().count() > 200 {
                errors.push(format!(
                    "「{target}」的 evidence quote 超长（{} > 200 字符）——请摘录关键句而非整段",
                    seg.chars().count()
                ));
                continue;
            }
            if match_quote(&src.content, seg, ev.span) == MatchLevel::None {
                let ctx = quote_context(&src.content, seg, 200)
                    .unwrap_or_else(|| src.content.chars().take(200).collect());
                errors.push(format!(
                    "「{target}」的 evidence 引用原文不存在于源「{}」，疑似臆想（E2）：\"{}\"。\n\
                     源「{}」中的相关原文片段：{}",
                    src.title,
                    truncate_head(seg, 50),
                    src.title,
                    ctx
                ));
            }
        }
    }
    if all_inferred && !strict {
        warnings.push(format!(
            "「{target}」unconfirmed：证据均为组合推断（inferred），需人工确认"
        ));
    }
}

fn truncate_head(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}…")
}

/// 证据源登记（连接级，供 OntologyStore 与 TtlStore 共用——两者装配层同库文件打开）。
/// content 为登记时刻快照（不可变）；幂等：同 ontology+kind+content_hash 返回既有 id。
pub fn register_source(
    conn: &Connection,
    kind: &str,
    title: &str,
    content: &str,
    ontology_api_name: &str,
    origin_ref: &str,
    now_ms: i64,
) -> StoreResult<String> {
    if kind.is_empty() || title.trim().is_empty() || content.trim().is_empty() {
        return Err(crate::error::StoreError::InvalidInput(
            "kind/title/content 均不能为空".to_string(),
        ));
    }
    let hash = content_hash(content);
    let existing: Option<String> = conn
        .query_row(
            "SELECT source_id FROM evidence_sources
             WHERE ontology_api_name=?1 AND kind=?2 AND content_hash=?3",
            rusqlite::params![ontology_api_name, kind, hash],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing {
        return Ok(id);
    }
    let source_id = format!("src-{}", uuid::Uuid::new_v4().simple());
    conn.execute(
        "INSERT INTO evidence_sources
         (source_id, ontology_api_name, kind, title, content, content_hash, origin_ref, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        rusqlite::params![
            source_id,
            ontology_api_name,
            kind,
            title.trim(),
            content,
            hash,
            origin_ref,
            now_ms
        ],
    )?;
    Ok(source_id)
}

/// 连接级读取全部证据源映射（闸门校验用；旧库无表时返回空集）。
pub fn load_sources_map(conn: &Connection) -> StoreResult<HashMap<String, EvidenceSource>> {
    let mut stmt = match conn.prepare(
        "SELECT source_id, ontology_api_name, kind, title, content, content_hash, origin_ref, created_at
         FROM evidence_sources",
    ) {
        Ok(s) => s,
        Err(_) => return Ok(HashMap::new()),
    };
    let rows = stmt
        .query_map([], |r| {
            Ok(EvidenceSource {
                source_id: r.get(0)?,
                ontology_api_name: r.get(1)?,
                kind: r.get(2)?,
                title: r.get(3)?,
                content: r.get(4)?,
                content_hash: r.get(5)?,
                origin_ref: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?
        .filter_map(Result::ok)
        .map(|s| (s.source_id.clone(), s))
        .collect();
    Ok(rows)
}

/// 元素级证据行（`element_evidence` 表读取视图，docs/EVIDENCE-CHAIN.md L4）。
/// Palantir（standard='palantir'）与 W3C TTL（standard='w3c'）统一存储。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ElementEvidenceRow {
    pub element_type: String,
    pub element_id: String,
    pub source_id: String,
    /// 出处标题（JOIN evidence_sources 取，展示用）
    pub source_title: String,
    pub source_kind: String,
    pub quote: String,
    /// 'direct' | 'inferred'
    pub role: String,
    /// 'exact' | 'normalized' | ''（空 = 引用未通过校验：源缺失或 quote 不符，前端🔴态）
    pub matched_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<(u32, u32)>,
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

/// 持久化一批元素级证据（import/落库路径调用；幂等 UPSERT，按元素整组替换）。
///
/// `items`：(element_type, element_id, evidence)。matched_by 在此重算
/// （对源 content 机械匹配），不信任 payload 自我声明；
/// 引用不存在的源 / 匹配失败的行也落库（供前端展示🔴与审计），
/// matched_by 为空即「未通过校验」的显式信号。
pub fn persist_element_evidence(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
    items: impl IntoIterator<Item = (String, String, Evidence)>,
    now_ms: i64,
) -> StoreResult<usize> {
    let sources = load_sources_map(conn)?;
    let mut written = 0usize;
    for (element_type, element_id, ev) in items {
        // 整组替换：该元素旧证据清空（payload 是 write-view 全量语义）
        conn.execute(
            "DELETE FROM element_evidence
             WHERE standard=?1 AND ontology_key=?2 AND element_type=?3 AND element_id=?4",
            rusqlite::params![standard, ontology_key, element_type, element_id],
        )?;
        // 豁免注解（W3C：evidence: none | direct | 「理由」）：matched_by='manual'，
        // source_id 落空串——用户显式声明人工确认设计，无需源引用
        let is_manual = ev.source_id == "none";
        let matched_by = if is_manual {
            "manual".to_string()
        } else {
            match sources.get(&ev.source_id) {
                Some(src) => match match_quote(&src.content, &ev.quote, ev.span) {
                    MatchLevel::Exact => "exact",
                    MatchLevel::Normalized => "normalized",
                    MatchLevel::None => "",
                }
                .to_string(),
                None => String::new(),
            }
        };
        conn.execute(
            "INSERT INTO element_evidence
             (id, standard, ontology_key, element_type, element_id, source_id, quote, span, role, matched_by, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
             ON CONFLICT(standard, ontology_key, element_type, element_id, source_id, quote)
             DO UPDATE SET role=excluded.role, matched_by=excluded.matched_by, span=excluded.span",
            rusqlite::params![
                format!("ee-{}", uuid::Uuid::new_v4().simple()),
                standard,
                ontology_key,
                element_type,
                element_id,
                if is_manual { "" } else { &ev.source_id },
                ev.quote,
                ev.span
                    .map(|(a, b)| format!("[{a},{b}]"))
                    .unwrap_or_default(),
                match ev.role {
                    EvidenceRole::Direct => "direct",
                    EvidenceRole::Inferred => "inferred",
                },
                matched_by,
                now_ms
            ],
        )?;
        written += 1;
    }
    Ok(written)
}

/// 删除某本体（key 粒度）的全部元素级证据（本体删除/重建时调用）。
pub fn delete_element_evidence(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
) -> StoreResult<usize> {
    Ok(conn.execute(
        "DELETE FROM element_evidence WHERE standard=?1 AND ontology_key=?2",
        rusqlite::params![standard, ontology_key],
    )?)
}

/// 删除某本体下指定元素的证据（overwrite 单个 OT 时调用）。
pub fn delete_element_evidence_for(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
    element_type: &str,
    element_id: &str,
) -> StoreResult<usize> {
    Ok(conn.execute(
        "DELETE FROM element_evidence
         WHERE standard=?1 AND ontology_key=?2 AND element_type=?3 AND element_id=?4",
        rusqlite::params![standard, ontology_key, element_type, element_id],
    )?)
}

/// 列出某本体的元素级证据（含出处标题 JOIN；前端三态展示用）。
pub fn list_element_evidence(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
) -> StoreResult<Vec<ElementEvidenceRow>> {
    let mut stmt = conn.prepare(
        "SELECT e.element_type, e.element_id, e.source_id,
                COALESCE(s.title, '(源已删除)'), COALESCE(s.kind, ''),
                e.quote, e.role, e.matched_by, e.span, e.created_at
         FROM element_evidence e
         LEFT JOIN evidence_sources s ON s.source_id = e.source_id
         WHERE e.standard=?1 AND e.ontology_key=?2
         ORDER BY e.element_type, e.element_id, e.created_at",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![standard, ontology_key], |r| {
            let span_str: String = r.get(8)?;
            let span = if span_str.is_empty() {
                None
            } else {
                serde_json::from_str::<(u32, u32)>(&span_str).ok()
            };
            Ok(ElementEvidenceRow {
                element_type: r.get(0)?,
                element_id: r.get(1)?,
                source_id: r.get(2)?,
                source_title: r.get(3)?,
                source_kind: r.get(4)?,
                quote: r.get(5)?,
                role: r.get(6)?,
                matched_by: r.get(7)?,
                span,
                created_at: r.get(9)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 解析 TTL skos:editorialNote 证据注解（docs/EVIDENCE-CHAIN.md 5.4，W3C 路径）。
/// 格式：`evidence: {source_id} | {direct|inferred} | 「quote」`（quote 内 inferred 多段用 SEGMENT_SEP 拼接）。
/// 返回 (source_id, role, quote)；格式非法返回 None（调用方报格式错误）。
pub fn parse_evidence_note(note: &str) -> Option<(String, String, String)> {
    // oxigraph 返回的字面量可能带 Turtle 原始引号
    let note = note.trim();
    let note = note.strip_prefix('"').unwrap_or(note);
    let note = note.strip_suffix('"').unwrap_or(note);
    let body = note.trim().strip_prefix("evidence:")?.trim();
    let mut parts = body.splitn(3, '|');
    let source_id = parts.next()?.trim().to_string();
    if source_id.is_empty() {
        return None;
    }
    let role = parts.next()?.trim().to_string();
    if role != "direct" && role != "inferred" {
        return None;
    }
    let rest = parts.next()?.trim();
    let start = rest.find('「')?;
    let end = rest.rfind('」')?;
    if end <= start {
        return None;
    }
    let quote = rest[start + '「'.len_utf8()..end].trim().to_string();
    if quote.is_empty() {
        return None;
    }
    Some((source_id, role, quote))
}

/// 证据闸门校验器（docs/EVIDENCE-CHAIN.md L3 / 5.1）。
///
/// 对一批 (元素标签, evidence) 逐项校验，聚合 errors / warnings。
/// `strict` = true 时 ungrounded 升级为阻断错误（S1）。
pub fn validate_evidence_batch(
    items: impl IntoIterator<Item = (String, Vec<Evidence>)>,
    sources: &HashMap<String, EvidenceSource>,
    strict: bool,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for (target, evidence) in items {
        check_one(&target, &evidence, sources, strict, &mut errors, &mut warnings);
    }
    (errors, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "客户下单后，订单创建成功。若三天未付款，系统自动取消订单并通知仓库。";

    fn src_map() -> HashMap<String, EvidenceSource> {
        let s = EvidenceSource {
            source_id: "src-1".into(),
            ontology_api_name: String::new(),
            kind: "scenario".into(),
            title: "场景描述".into(),
            content: SRC.into(),
            content_hash: content_hash(SRC),
            origin_ref: String::new(),
            created_at: 0,
        };
        HashMap::from([("src-1".to_string(), s)])
    }

    fn ev(quote: &str, role: EvidenceRole) -> Evidence {
        Evidence {
            source_id: "src-1".into(),
            quote: quote.into(),
            span: None,
            role,
        }
    }

    #[test]
    fn exact_match_passes() {
        assert_eq!(
            match_quote(SRC, "三天未付款", None),
            MatchLevel::Exact
        );
    }

    #[test]
    fn normalized_match_tolerates_whitespace_and_width() {
        // 全角空格 + 多余空白抖动 → 归一化后匹配
        assert_eq!(
            match_quote(SRC, "客户下单后，　订单创建成功。", None),
            MatchLevel::Normalized
        );
    }

    #[test]
    fn paraphrase_fails() {
        // 改写：「用户逾期三天」≠ 原文「三天未付款」→ 必须失败（底线）
        assert_eq!(match_quote(SRC, "用户逾期三天", None), MatchLevel::None);
    }

    #[test]
    fn span_mismatch_fails_even_if_quote_exists() {
        // quote 真实存在，但 span 指向别处 → 假定位，按失败处理
        assert_eq!(
            match_quote(SRC, "三天未付款", Some((0, 5))),
            MatchLevel::None
        );
    }

    #[test]
    fn segment_separator_present() {
        // 整串含分隔符不直接匹配（校验器层切段后逐段匹配）
        assert_eq!(
            match_quote(SRC, "客户下单后 ⟪…⟫ 三天未付款", None),
            MatchLevel::None,
        );
    }

    #[test]
    fn validate_strict_blocks_ungrounded() {
        let sources = src_map();
        let (errors, _) = validate_evidence_batch(
            vec![("Order".to_string(), vec![])],
            &sources,
            true,
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("无任何证据引用"));
    }

    #[test]
    fn validate_lenient_warns_ungrounded() {
        let sources = src_map();
        let (errors, warnings) = validate_evidence_batch(
            vec![("Order".to_string(), vec![])],
            &sources,
            false,
        );
        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ungrounded"));
    }

    #[test]
    fn validate_rejects_unknown_source() {
        let sources = src_map();
        // src_map 用 src-1，这里改 id 模拟未登记
        let mut bad = ev("三天未付款", EvidenceRole::Direct);
        bad.source_id = "src-nope".into();
        let (errors, _) = validate_evidence_batch(
            vec![("Order".to_string(), vec![bad])],
            &sources,
            false,
        );
        assert!(errors[0].contains("未登记的源"));
    }

    #[test]
    fn validate_rejects_paraphrase_with_context() {
        let sources = src_map();
        let (errors, _) = validate_evidence_batch(
            vec![("Order".to_string(), vec![ev("用户逾期三天", EvidenceRole::Direct)])],
            &sources,
            false,
        );
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("疑似臆想"));
        // 指导性打回：错误信息附原文上下文
        assert!(errors[0].contains("相关原文片段"));
    }

    #[test]
    fn validate_inferred_segments_each_checked() {
        let sources = src_map();
        // 两段都真实 → 通过
        let (errors, warnings) = validate_evidence_batch(
            vec![(
                "OverdueOrder".to_string(),
                vec![ev("客户下单后 ⟪…⟫ 三天未付款", EvidenceRole::Inferred)],
            )],
            &sources,
            false,
        );
        assert!(errors.is_empty());
        assert!(warnings.iter().any(|w| w.contains("unconfirmed")));
        // 一段臆想 → 阻断
        let (errors, _) = validate_evidence_batch(
            vec![(
                "OverdueOrder".to_string(),
                vec![ev("客户下单后 ⟪…⟫ 用户逾期三天", EvidenceRole::Inferred)],
            )],
            &sources,
            false,
        );
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn fnv_hash_stable() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
    }

    #[test]
    #[test]
    fn check_one_allows_none_exemption() {
        let mut e = Evidence {
            source_id: "none".into(),
            quote: "用户在对话中确认为纯设计抽象".into(),
            span: None,
            role: EvidenceRole::Direct,
        };
        // 豁免引用不报 E1（未登记源），strict 下也不因「无引用」被拦
        let (errors, _) = validate_evidence_batch(
            [("X".to_string(), vec![e.clone()])],
            &src_map(),
            true,
        );
        assert!(errors.is_empty(), "{errors:?}");
        e.source_id = "src-missing".into();
        let (errors, _) = validate_evidence_batch([("X".to_string(), vec![e])], &src_map(), true);
        assert!(errors.iter().any(|x| x.contains("E1")));
    }

    fn parse_evidence_note_direct() {
        let (sid, role, quote) = parse_evidence_note(
            "evidence: src-abc | direct | 「订单创建后处于待支付状态」",
        )
        .unwrap();
        assert_eq!(sid, "src-abc");
        assert_eq!(role, "direct");
        assert_eq!(quote, "订单创建后处于待支付状态");
    }

    #[test]
    fn parse_evidence_note_inferred_multi_segment() {
        let (_, role, quote) = parse_evidence_note(
            "evidence: src-1 | inferred | 「客户下单后 ⟪…⟫ 三天未付款」",
        )
        .unwrap();
        assert_eq!(role, "inferred");
        assert_eq!(quote_segments(&quote).len(), 2);
    }

    #[test]
    fn parse_evidence_note_rejects_bad_format() {
        assert!(parse_evidence_note("随便写的一句话").is_none());
        assert!(parse_evidence_note("evidence: src-1 | maybe | 「x」").is_none());
        assert!(parse_evidence_note("evidence: src-1 | direct | 无引号").is_none());
    }

    #[test]
    fn quote_context_returns_original_window() {
        // 归一化可定位时返回原文窗口
        let ctx = quote_context(SRC, "订单创建成功", 10);
        assert!(ctx.is_some());
        assert!(ctx.unwrap().contains("订单创建成功"));
    }
}
