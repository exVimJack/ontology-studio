//! 上下文 token 预算与历史裁剪（二期 B1）。
//!
//! 设计要点（见 ARCHITECTURE.md 决策 16）：
//! - **不引入 tokenizer**（原则 2 轻量化）。用 `chars / 4` 启发式估算 token，
//!   偏松但安全（Claude Code 热路径同款）。
//! - **分层防御**：前端字节预算防 413（决策 13）；本模块防 `context_length_exceeded`
//!   ——即便单次请求 body 没超 413，历史过长仍会超模型上下文窗口被 provider 拒绝。
//! - **B2 压缩衔接**：裁剪掉的旧消息不直接丢弃，由上层（src-tauri）压缩成摘要
//!   注入 history 开头（见 `compact_history`）。本模块只负责「判定裁剪边界」，
//!   不负责生成摘要（摘要需 chat service，属平台层编排）。
//!
//! 一期常量集中在此，便于调参。

use rig::completion::message::Message;

/// 默认上下文窗口预算（token）。保守取 100K，覆盖多数主流模型
/// （GPT-4o 128K、Claude 200K、DeepSeek V4 1M）。真实窗口由
/// context_window.rs 探测/内置已知模型表解析，这里是未知模型的兜底值。
pub const DEFAULT_MAX_CONTEXT_TOKENS: usize = 100_000;

/// 默认保留的最近对话轮数（一轮 = user + assistant）。
/// 裁剪时永远保留最近这几轮的完整上下文，更早的才进入压缩候选。
pub const DEFAULT_KEEP_RECENT_TURNS: usize = 6;

/// 摘要预留 token 预算（压缩后的旧历史摘要最多占这么多 token）。
pub const SUMMARY_TOKEN_BUDGET: usize = 2_000;

/// 启发式 token 估算：`chars / 4`。
///
/// 对中英混合文本偏松（中文 1 字 ≈ 1-2 token，英文 4 char ≈ 1 token），
/// 估算偏高意味着会更早触发裁剪——安全侧倾向。
pub fn estimate_tokens(text: &str) -> usize {
    // 向上取整，避免极短文本估为 0
    text.chars().count().div_ceil(4)
}

/// 估算一条 Rig `Message` 的 token 数（仅文本 part；图片按固定开销估算）。
pub fn estimate_message_tokens(msg: &Message) -> usize {
    use rig::completion::message::{AssistantContent, UserContent};
    let mut total = 0usize;
    match msg {
        Message::User { content } => {
            for p in content.iter() {
                match p {
                    UserContent::Text(t) => total += estimate_tokens(&t.text),
                    UserContent::Image(_) => total += 768, // 单图固定开销（保守）
                    _ => {}
                }
            }
        }
        Message::Assistant { content, .. } => {
            for c in content.iter() {
                if let AssistantContent::Text(t) = c {
                    total += estimate_tokens(&t.text);
                }
            }
        }
        Message::System { content } => {
            // System 可能是 String 或结构化，统一按字符串估算
            total += estimate_tokens(content.as_ref());
        }
    }
    // 每条消息固定开销（角色标记、分隔符等，保守估算）
    total + 4
}

/// 估算一组消息的 token 总数。
pub fn estimate_messages_tokens(msgs: &[Message]) -> usize {
    msgs.iter().map(estimate_message_tokens).sum()
}

/// 预算配置。
#[derive(Debug, Clone, Copy)]
pub struct BudgetConfig {
    /// 上下文窗口 token 上限（含历史 + prompt + 注入上下文）。
    pub max_context_tokens: usize,
    /// 永远保留的最近对话轮数（一轮 = user + assistant）。
    pub keep_recent_turns: usize,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
            keep_recent_turns: DEFAULT_KEEP_RECENT_TURNS,
        }
    }
}

/// 裁剪结果。
#[derive(Debug, Clone)]
pub struct TrimmedHistory {
    /// 保留并发给模型的最近消息（不含被裁掉的旧消息）。
    pub kept: Vec<Message>,
    /// 被裁掉的旧消息（按时间升序，最旧在前），供上层压缩成摘要。
    /// 若为空，表示无需裁剪。
    pub evicted: Vec<Message>,
    /// 是否发生了裁剪。
    pub trimmed: bool,
}

/// 按预算裁剪历史。
///
/// 算法：
/// 1. 估算 prompt + 已注入上下文（`prompt_tokens`）的 token 数。
/// 2. 从历史末尾向前累计，保留最近 `keep_recent_turns` 轮（最少 1 轮）。
/// 3. 剩余预算 = `max_context_tokens` - prompt_tokens - kept_tokens - SUMMARY_TOKEN_BUDGET`。
/// 4. 从保留区之前继续向前纳入更多消息，直到预算耗尽；更早的进入 `evicted`。
/// 5. 若总历史本就在预算内，`evicted` 为空，`kept` 为全部历史。
///
/// `prompt_tokens` 应包含本轮 user prompt + RAG/手动 context 的 token 估算，
/// 由调用方算好传入（本模块不重复估算 prompt，避免与上下文构造逻辑耦合）。
pub fn trim_history(
    history: Vec<Message>,
    prompt_tokens: usize,
    config: BudgetConfig,
) -> TrimmedHistory {
    if history.is_empty() {
        return TrimmedHistory {
            kept: vec![],
            evicted: vec![],
            trimmed: false,
        };
    }

    let history_tokens = estimate_messages_tokens(&history);
    let total = history_tokens.saturating_add(prompt_tokens);

    if total <= config.max_context_tokens {
        // 无需裁剪
        return TrimmedHistory {
            kept: history,
            evicted: vec![],
            trimmed: false,
        };
    }

    // 需要裁剪。从末尾向前保留最近 keep_recent_turns 轮（每轮 = 2 条：user+assistant，
    // 但 Tool/System 消息按 1 条计）。保留区至少 2 条（1 轮）。
    let keep_count = (config.keep_recent_turns.saturating_mul(2)).max(2).min(history.len());
    let split = history.len().saturating_sub(keep_count);
    let (old, recent) = history.split_at(split);

    // recent 区一定保留。剩余预算决定 old 区能纳入多少。
    let recent_tokens = estimate_messages_tokens(recent);
    let budget_for_old = config
        .max_context_tokens
        .saturating_sub(prompt_tokens)
        .saturating_sub(recent_tokens)
        .saturating_sub(SUMMARY_TOKEN_BUDGET);

    // 从 old 区末尾（较新）向前纳入，尽量保留更多近期上下文
    let mut kept_old: Vec<Message> = Vec::new();
    let mut used = 0usize;
    for msg in old.iter().rev() {
        let t = estimate_message_tokens(msg);
        if used + t > budget_for_old {
            break;
        }
        used += t;
        kept_old.push(msg.clone());
    }
    kept_old.reverse(); // 恢复时间升序

    let kept: Vec<Message> = kept_old
        .into_iter()
        .chain(recent.iter().cloned())
        .collect();

    // evicted = old 区中未被纳入的（最旧的部分）
    let kept_old_len = kept.len() - recent.len();
    let evicted: Vec<Message> = old[..old.len().saturating_sub(kept_old_len)].to_vec();

    TrimmedHistory {
        kept,
        evicted,
        trimmed: true,
    }
}

// ── 基于 MessageRow 的裁剪（供 src-tauri 直接操作持久化行） ──────

/// 裁剪结果（行版本，保留 id 供删除/更新）。
#[derive(Debug, Clone)]
pub struct TrimmedRows {
    /// 保留并发给模型的最近消息行（时间升序）。
    pub kept: Vec<memory::MessageRow>,
    /// 被裁掉的旧消息行（时间升序，最旧在前），供上层压缩成摘要后删除。
    pub evicted: Vec<memory::MessageRow>,
    /// 是否发生了裁剪。
    pub trimmed: bool,
}

/// 估算一条 `MessageRow` 的 token 数（仅看 content 文本 + 角色开销）。
pub fn estimate_row_tokens(row: &memory::MessageRow) -> usize {
    estimate_tokens(&row.content) + 4
}

/// 按预算裁剪历史（行版本）。
///
/// 逻辑与 `trim_history` 一致，但直接操作 `MessageRow`，便于上层删除/压缩。
/// `prompt_tokens` 由调用方算好传入（含本轮 prompt + 注入上下文 token）。
pub fn trim_history_rows(
    history: Vec<memory::MessageRow>,
    prompt_tokens: usize,
    config: BudgetConfig,
) -> TrimmedRows {
    use memory::MessageStatus;
    if history.is_empty() {
        return TrimmedRows {
            kept: vec![],
            evicted: vec![],
            trimmed: false,
        };
    }

    // 只统计 complete 且非空的（与 text_history_to_messages 过滤一致）
    let history_tokens: usize = history
        .iter()
        .filter(|m| m.status == MessageStatus::Complete && !m.content.is_empty())
        .map(estimate_row_tokens)
        .sum();
    let total = history_tokens.saturating_add(prompt_tokens);

    if total <= config.max_context_tokens {
        return TrimmedRows {
            kept: history,
            evicted: vec![],
            trimmed: false,
        };
    }

    // 保留最近 keep_recent_turns 轮（每轮 2 条）
    let keep_count = (config.keep_recent_turns.saturating_mul(2)).max(2).min(history.len());
    let split = history.len().saturating_sub(keep_count);
    let (old, recent) = history.split_at(split);

    let recent_tokens: usize = recent.iter().map(estimate_row_tokens).sum();
    let budget_for_old = config
        .max_context_tokens
        .saturating_sub(prompt_tokens)
        .saturating_sub(recent_tokens)
        .saturating_sub(SUMMARY_TOKEN_BUDGET);

    let mut kept_old: Vec<memory::MessageRow> = Vec::new();
    let mut used = 0usize;
    for row in old.iter().rev() {
        let t = estimate_row_tokens(row);
        if used + t > budget_for_old {
            break;
        }
        used += t;
        kept_old.push(row.clone());
    }
    kept_old.reverse();

    let kept: Vec<memory::MessageRow> = kept_old
        .iter()
        .cloned()
        .chain(recent.iter().cloned())
        .collect();

    let kept_old_len = kept_old.len();
    let evicted: Vec<memory::MessageRow> = old[..old.len().saturating_sub(kept_old_len)].to_vec();

    TrimmedRows {
        kept,
        evicted,
        trimmed: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::completion::message::UserContent;

    fn user(text: &str) -> Message {
        Message::User {
            content: rig::OneOrMany::one(UserContent::text(text.to_string())),
        }
    }

    fn assistant(text: &str) -> Message {
        use rig::completion::message::AssistantContent;
        Message::Assistant {
            id: None,
            content: rig::OneOrMany::one(AssistantContent::text(text.to_string())),
        }
    }

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2); // 5 chars → (5+3)/4 = 2
        assert_eq!(estimate_tokens("你好世界"), 1); // 4 chars → 1
    }

    #[test]
    fn no_trim_when_within_budget() {
        let hist = vec![user("hi"), assistant("hello")];
        let cfg = BudgetConfig {
            max_context_tokens: 100_000,
            keep_recent_turns: 6,
        };
        let r = trim_history(hist.clone(), 10, cfg);
        assert!(!r.trimmed);
        assert_eq!(r.kept.len(), 2);
        assert!(r.evicted.is_empty());
    }

    #[test]
    fn trims_when_over_budget() {
        // 构造 20 轮（40 条），每条 ~10 token，总 ~400 token。
        let mut hist = Vec::new();
        for i in 0..20 {
            hist.push(user(&format!("user message number {i} with padding")));
            hist.push(assistant(&format!("assistant reply number {i} with padding")));
        }
        // 预算设很小：max=80, prompt=10, keep=2 轮(4条)。
        // recent 4 条 ~40 token，剩 80-10-40-2000 < 0 → old 全部 evict。
        let cfg = BudgetConfig {
            max_context_tokens: 80,
            keep_recent_turns: 2,
        };
        let r = trim_history(hist, 10, cfg);
        assert!(r.trimmed);
        assert!(r.kept.len() <= 4, "kept should be at most keep_recent_turns*2");
        assert!(!r.evicted.is_empty(), "should evict old messages");
    }

    #[test]
    fn keeps_recent_turns_minimum() {
        // 即便预算极小，也至少保留 keep_recent_turns 轮
        let mut hist = Vec::new();
        for i in 0..10 {
            hist.push(user(&format!("u{i}")));
            hist.push(assistant(&format!("a{i}")));
        }
        let cfg = BudgetConfig {
            max_context_tokens: 5, // 极小
            keep_recent_turns: 3,
        };
        let r = trim_history(hist, 1, cfg);
        assert!(r.trimmed);
        assert_eq!(r.kept.len(), 6, "should keep 3 turns = 6 messages");
        assert_eq!(r.evicted.len(), 14);
    }

    #[test]
    fn empty_history_no_trim() {
        let cfg = BudgetConfig::default();
        let r = trim_history(vec![], 100, cfg);
        assert!(!r.trimmed);
        assert!(r.kept.is_empty());
        assert!(r.evicted.is_empty());
    }

    // ── 行版本测试 ──
    fn row(i: usize) -> memory::MessageRow {
        memory::MessageRow {
            id: format!("u{i}"),
            conversation_id: "c".into(),
            role: memory::MessageRole::User,
            status: memory::MessageStatus::Complete,
            content: format!("user message {i} with some padding text"),
            reasoning: None,
            error: None,
            model: None,
            created_at: memory::Timestamp::from((i as i64) * 2),
            updated_at: memory::Timestamp::from((i as i64) * 2),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
        }
    }

    fn arow(i: usize) -> memory::MessageRow {
        memory::MessageRow {
            id: format!("a{i}"),
            conversation_id: "c".into(),
            role: memory::MessageRole::Assistant,
            status: memory::MessageStatus::Complete,
            content: format!("assistant reply {i} with some padding text"),
            reasoning: None,
            error: None,
            model: Some("test-model".into()),
            created_at: memory::Timestamp::from((i as i64) * 2 + 1),
            updated_at: memory::Timestamp::from((i as i64) * 2 + 1),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
        }
    }

    #[test]
    fn rows_no_trim_within_budget() {
        let hist = vec![row(0), arow(0)];
        let cfg = BudgetConfig::default();
        let r = trim_history_rows(hist, 10, cfg);
        assert!(!r.trimmed);
        assert_eq!(r.kept.len(), 2);
        assert!(r.evicted.is_empty());
    }

    #[test]
    fn rows_trims_and_evicts_old() {
        let mut hist = Vec::new();
        for i in 0..10 {
            hist.push(row(i));
            hist.push(arow(i));
        }
        let cfg = BudgetConfig {
            max_context_tokens: 60,
            keep_recent_turns: 2,
        };
        let r = trim_history_rows(hist, 5, cfg);
        assert!(r.trimmed);
        assert!(r.kept.len() <= 4);
        assert!(!r.evicted.is_empty());
        // evicted 的 id 应是最旧的（u0/a0/u1/a1...），kept 是最新的
        assert!(r.evicted[0].id.starts_with("u0"));
        assert!(r.kept.last().unwrap().id.starts_with("a9"));
    }

    #[test]
    fn rows_empty_no_trim() {
        let r = trim_history_rows(vec![], 100, BudgetConfig::default());
        assert!(!r.trimmed);
    }
}
