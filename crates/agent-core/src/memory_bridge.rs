//! Memory 桥接：把项目的 `Memory`（SQLite）适配为 rig 的 `ConversationMemory`，
//! 并提供 `CompactingMemory` 组合（TokenWindow 裁剪 + LLM 摘要压缩）。
//!
//! 设计（见 ARCHITECTURE.md 二期 B1/B2 重构）：
//!   - `Memory` impl `ConversationMemory`：load=list_messages→rig::Message，
//!     append/clear 写回 SQLite。**但实际 send_message 流程中 append 为 no-op**——
//!     消息由 send_message 手动建（前端需预生成 message_id 做 patch），
//!     rig 的 turn-end append 被忽略，避免重复写入。
//!   - load 内部用 `list_messages_limited` 取最近 MESSAGE_LOAD_LIMIT 条消息
//!     （避免超长会话从 SQLite 拉全量 content 阻塞几秒）。
//!     限额 200 条远大于 TokenWindow 裁剪窗口——足够窗口判定+压缩选则，
//!     同时保证 SQLite 扫描量可控（单表再大也只取 200 行）。
//!   - `CompactingMemory` 包裹 Memory + `AdaptiveWindowPolicy` + `LlmCompactor`：
//!     load 时自动裁剪超预算的旧历史并生成滚动摘要（carry_over），splice 进返回 history。
//!     预算按分档比例（75%/50%/85%，留刹车距离）而非 100% 窗口；
//!     前 3 条消息（首轮任务指令）永不逐出（借鉴 Hermes 压缩机制）。
//!     摘要仅存进程内存 state（不落 DB），重启后重新 compact（可接受）。
//!   - `LlmCompactor` impl `Compactor`：调 LLM 把 evicted 旧历史压缩成 `TextSummary`。
//!     因 ChatService 的 client 是泛型 enum，用 `SummaryFn` trait object 擦除。
//!
//! 这让我们直接复用 rig 的上下文管理基础设施，零手写 trim/compact 逻辑。

/// memory::load 取消息的上限：最近 200 条。
///
/// 200 条远大于 TokenWindow 裁剪窗口（128k token ≈ 几百条短消息），
/// 足够压缩判定。同时限制 SQLite 扫描量——单表再大每次 load 也只取 200 行，
/// 消灭“几秒拉取全量 content”的加载卡顿。
const MESSAGE_LOAD_LIMIT: usize = 200;

use std::sync::Arc;

use rig::completion::message::{AssistantContent, Message, UserContent};
use rig::id::ConversationId;
use rig::memory::{CompactingMemory, ConversationMemory, HeuristicTokenCounter, MemoryError, TokenWindowMemory};
use rig::wasm_compat::WasmBoxedFuture;

use memory::{Memory, MessageRole, MessageStatus};

// ── newtype 包装：绕过孤儿规则（不能为外部类型 Arc<Memory> impl 外部 trait） ──

/// rig `ConversationMemory` 的 SQLite 后端适配器。
///
/// 包装 `Arc<Memory>`，实现 load/append/clear。
/// - `load`：读 DB 全量消息 → rig Message
/// - `append`：**no-op**（消息由 send_message 手动建，前端需预生成 message_id）
/// - `clear`：删会话所有消息（保留会话本身）
#[derive(Clone)]
pub struct SqliteMemory(pub Arc<Memory>);

impl From<Arc<Memory>> for SqliteMemory {
    fn from(m: Arc<Memory>) -> Self {
        Self(m)
    }
}

impl ConversationMemory for SqliteMemory {
    fn load<'a>(
        &'a self,
        conversation_id: &'a ConversationId,
    ) -> WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            let rows = self
                .0
                .list_messages_limited(conversation_id.as_str(), Some(MESSAGE_LOAD_LIMIT))
                .map_err(|e| {
                    MemoryError::Backend(Box::new(std::io::Error::other(e.to_string())))
                })?;
            Ok(rows_to_messages(&rows))
        })
    }

    fn append<'a>(
        &'a self,
        conversation_id: &'a ConversationId,
        messages: Vec<Message>,
    ) -> WasmBoxedFuture<'a, Result<(), MemoryError>> {
        // no-op：消息由 send_message 手动建（前端需预生成 message_id 做 patch）。
        // rig turn 结束调用 append 时忽略，避免与手动建消息重复写入。
        // load 仍读 DB 全量，CompactingMemory 在 load 时裁剪——此机制不依赖 append。
        let _ = (conversation_id, messages);
        Box::pin(async move { Ok(()) })
    }

    fn clear<'a>(
        &'a self,
        conversation_id: &'a ConversationId,
    ) -> WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.0
                .delete_conversation_messages(conversation_id.as_str())
                .map_err(|e| MemoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))
        })
    }
}

/// 把 `MessageRow` 列表转为 rig `Message`（跳过未完成/空/非摘要 system）。
///
/// 与 `chat::text_history_to_messages` 同语义，但这里供 memory load 用。
/// 摘要 system 消息（model=="__summary__"）保留——但 CompactingMemory 场景下
/// 摘要由 compactor 在内存生成并 splice，DB 里不再存摘要消息（旧 B2 的摘要表已废弃）。
fn rows_to_messages(rows: &[memory::MessageRow]) -> Vec<Message> {
    rows.iter()
        .filter(|m| {
            m.status == MessageStatus::Complete
                && !m.content.is_empty()
                && matches!(
                    m.role,
                    MessageRole::User | MessageRole::Assistant | MessageRole::System
                )
        })
        .map(|m| match m.role {
            MessageRole::User => Message::User {
                content: vec![UserContent::text(m.content.clone())],
            },
            MessageRole::Assistant => Message::Assistant(
                rig::completion::message::AssistantMessage::new(vec![AssistantContent::text(m.content.clone())]),
            ),
            MessageRole::System => Message::System {
                content: m.content.clone(),
            },
        })
        .collect()
}

// ── LlmCompactor ──────────────────────────────────────────────

/// LLM 摘要函数（trait object 擦除 provider 泛型）。
///
/// 输入待压缩文本（旧历史拼接 + 可选 carry_over），返回摘要文本。
pub type SummaryFn =
    Arc<dyn Fn(String) -> WasmBoxedFuture<'static, Result<String, String>> + Send + Sync>;

/// 用 LLM 把被裁掉的旧历史压缩成滚动摘要。
///
/// 实现 rig 的 `Compactor` trait：`compact(conv_id, evicted, carry_over)` 把
/// evicted 消息拼接成文本，连同上一轮 carry_over 摘要，调 LLM 生成新摘要。
/// 失败时返回 `MemoryError::Backend`（CompactingMemory 会传播，上层降级）。
pub struct LlmCompactor {
    summarize: SummaryFn,
}

impl LlmCompactor {
    pub fn new(summarize: SummaryFn) -> Self {
        Self { summarize }
    }
}

/// LLM 摘要产物（Compactor::Artifact）。
///
/// 自定义类型而非复用 rig-memory 的 `TextSummary`——后者字段私有，
/// 外部 crate 无法构造。本类型满足 `Into<Message> + Clone + Send + Sync`。
#[derive(Clone)]
pub struct SummaryArtifact(pub String);

impl From<SummaryArtifact> for Message {
    fn from(s: SummaryArtifact) -> Self {
        Message::System { content: s.0 }
    }
}

impl rig::memory::Compactor for LlmCompactor {
    type Artifact = SummaryArtifact;

    fn compact<'a>(
        &'a self,
        _conversation_id: &'a ConversationId,
        evicted: &'a [Message],
        carry_over: Option<&'a Self::Artifact>,
    ) -> WasmBoxedFuture<'a, Result<Self::Artifact, MemoryError>> {
        Box::pin(async move {
            let text = messages_to_compaction_text(evicted);
            let input = match carry_over {
                Some(prev) => format!(
                    "之前的对话摘要：\n{}\n\n--- 新增的早期对话 ---\n{text}",
                    prev.0
                ),
                None => text,
            };
            if input.trim().is_empty() {
                return Ok(SummaryArtifact(String::new()));
            }
            let prompt = format!(
                "请将以下对话历史压缩成简洁的要点摘要，保留关键事实、决定与未完成事项。\n\
                 用中文，不超过 300 字。只输出摘要正文，不要任何前后缀。\n\n\
                 --- 对话历史 ---\n{input}"
            );
            let summary = (self.summarize)(prompt)
                .await
                .map_err(|e| MemoryError::Backend(Box::new(std::io::Error::other(e))))?;
            Ok(SummaryArtifact(summary))
        })
    }
}

/// 把 evicted 消息列表拼接为压缩用纯文本（标注角色）。
fn messages_to_compaction_text(messages: &[Message]) -> String {
    let mut buf = String::new();
    for m in messages {
        let (role, text) = match m {
            Message::User { content } => ("用户", first_text(content)),
            Message::Assistant(a) => ("助手", first_text_assistant(&a.content)),
            Message::System { content } => ("系统", content.clone()),
        };
        if text.is_empty() {
            continue;
        }
        buf.push_str(role);
        buf.push('：');
        buf.push_str(&text);
        buf.push_str("\n\n");
    }
    buf
}

fn first_text(content: &[UserContent]) -> String {
    for c in content.iter() {
        if let UserContent::Text(t) = c {
            return t.text.clone();
        }
    }
    String::new()
}

fn first_text_assistant(content: &[AssistantContent]) -> String {
    for c in content.iter() {
        if let AssistantContent::Text(t) = c {
            return t.text.clone();
        }
    }
    String::new()
}

// ── 构造：Memory + CompactingMemory 组合 ───────────────────────────

/// head 保护条数：前 3 条消息（含首轮任务指令）永不逐出（借鉴 Hermes
/// `compression.protect_first_n`）。
///
/// 本体建模会话的首条用户消息（建表 DDL + 遵守 charter 指令）是全链路最重要的
/// 上下文，被摘要掉后 agent 会丢失任务约束。head 不计入裁剪预算——消息数固定
/// 且为纯文本，体量有界，不会失控。
const PROTECT_HEAD_N: usize = 3;

/// 极小窗口阈值（token）：窗口很小 → 尽量用满再压（85%），避免频繁压缩。
const TINY_WINDOW: usize = 32_768;

/// 大窗口阈值（token）：≥ 此值 → 50% 即压，窗口足够大时更早压缩换稳定性。
const LARGE_WINDOW: usize = 512 * 1024;

/// 分档压缩触发比例（借鉴 Hermes 三档阈值，含“刹车距离”）。
///
/// 压缩本身要调一次 LLM 生成摘要，这个请求自己也占窗口——等 100% 满再压，
/// 压缩请求自身就可能超限（实测：12.9 万 token 请求打到托管端点 429）。
/// 预留 15%~50% 的刹车距离确保「当前 token + 新消息 + 压缩请求 < 100%」。
///
/// - 极小窗口（≤32K）：85% —— 窗口紧张，尽量用满
/// - 大窗口（≥512K）：50% —— 窗口充裕，早压换稳定
/// - 其他（主流 128K~200K）：75% —— 安全平衡点
pub fn compression_ratio(context_window: usize) -> f64 {
    if context_window <= TINY_WINDOW {
        0.85
    } else if context_window >= LARGE_WINDOW {
        0.50
    } else {
        0.75
    }
}

/// 压缩预算（token）= 窗口 × 分档比例。取整并至少保留 1024，防御 0 预算。
pub fn compression_budget(context_window: usize) -> usize {
    let budget = (context_window as f64 * compression_ratio(context_window)) as usize;
    budget.max(1_024).min(context_window)
}

/// 分档阈值 + head 保护的窗口裁剪策略（包 rig `TokenWindowMemory`）。
///
/// - 预算 = `compression_budget(window)`（非 100%，留刹车距离，见分档注释）
/// - 前 `PROTECT_HEAD_N` 条消息绕过预算直接保留，永不进 demoted
///   （孤立 tool-result 清理等 rig 原生行为在余下消息上照常生效）
pub struct AdaptiveWindowPolicy {
    inner: TokenWindowMemory,
}

impl AdaptiveWindowPolicy {
    pub fn new(context_window: usize) -> Self {
        Self {
            inner: TokenWindowMemory::new(
                compression_budget(context_window),
                // bytes/4 与 0.41 HeuristicTokenCounter::openai() 同参数（0.44 删该 ctor）
                HeuristicTokenCounter::new(4.0, 4, 0),
            ),
        }
    }
}

impl rig::memory::MemoryPolicy for AdaptiveWindowPolicy {
    fn apply(&self, messages: Vec<Message>) -> Result<Vec<Message>, MemoryError> {
        Ok(self.apply_with_demoted(messages)?.0)
    }

    fn apply_with_demoted(
        &self,
        messages: Vec<Message>,
    ) -> Result<(Vec<Message>, Vec<Message>), MemoryError> {
        let n = messages.len();
        if n <= PROTECT_HEAD_N {
            // 消息数不多于保护条数：全保留，不触发压缩
            return Ok((messages, Vec::new()));
        }
        let (head, rest) = messages.split_at(PROTECT_HEAD_N);
        let (kept, demoted) = self.inner.apply_with_demoted(rest.to_vec())?;
        let mut window = head.to_vec();
        window.extend(kept);
        Ok((window, demoted))
    }
}

/// 构造一个带自动压缩的 `ConversationMemory`。
///
/// - `memory`: SQLite 后端（Arc 共享）
/// - `context_window`: 模型上下文窗口 token 数（内部按分档比例留刹车距离）
/// - `summarize`: LLM 摘要函数（provider client 提供）
///
/// 返回的 `CompactingMemory` 可直接挂到 `AgentBuilder::memory()`。
/// load 时自动裁剪超预算的旧历史并滚动摘要；前 3 条永不被裁。
pub fn build_compacting_memory(
    memory: Arc<Memory>,
    context_window: usize,
    summarize: SummaryFn,
) -> CompactingMemory<SqliteMemory, AdaptiveWindowPolicy, LlmCompactor> {
    let policy = AdaptiveWindowPolicy::new(context_window);
    let compactor = LlmCompactor::new(summarize);
    CompactingMemory::new(SqliteMemory(memory), policy, compactor)
}

/// 解析模型上下文窗口的便捷包装（供 ChatService 构造 memory 时用）。
///
/// 与 send_message 的 resolve_context_window 一致，但这里只在构造 ChatService 时
/// 调一次（配置变更重建时）。运行时每次 load 用构造时的窗口值——若需动态更新，
/// 重建 ChatService 即可。
pub async fn resolve_window_or_default(config: &crate::provider::ProviderConfig) -> usize {
    crate::context_window::resolve_context_window(config).await as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::memory::MemoryPolicy as _; // apply_with_demoted 是 trait 方法

    // ── 分档压缩阈值 + head 保护 ──

    #[test]
    fn compression_ratio_tiers() {
        // 极小窗口：85%（尽量用满）
        assert_eq!(compression_ratio(8_192), 0.85);
        assert_eq!(compression_ratio(32_768), 0.85);
        // 主流窗口（128K~200K）：75%
        assert_eq!(compression_ratio(128_000), 0.75);
        assert_eq!(compression_ratio(200_000), 0.75);
        assert_eq!(compression_ratio(511_999), 0.75);
        // 大窗口（≥512K）：50%
        assert_eq!(compression_ratio(512 * 1024), 0.50);
        assert_eq!(compression_ratio(1_000_000), 0.50);
    }

    #[test]
    fn compression_budget_leaves_braking_distance() {
        // 128K 窗口 → 75% = 96K（非 100%：压缩请求自身也占窗口）
        assert_eq!(compression_budget(128_000), 96_000);
        // 1M 窗口 → 50%
        assert_eq!(compression_budget(1_000_000), 500_000);
        // 极小窗口防 0 预算
        assert_eq!(compression_budget(1), 1_024.min(1));
    }

    #[test]
    fn adaptive_policy_protects_head() {
        // 5 条消息、极小预算：前 3 条（head）永不逐出，其余被裁
        let policy = AdaptiveWindowPolicy::new(1); // 预算压到极小
        let msgs: Vec<Message> = (0..5)
            .map(|i| Message::User {
                content: vec![UserContent::text(format!("m{i}"))],
            })
            .collect();
        let (window, demoted) = policy.apply_with_demoted(msgs).unwrap();
        assert_eq!(window.len(), 3, "head 3 条保留");
        assert_eq!(demoted.len(), 2, "余下 2 条被逐出交压缩");
        assert!(matches!(&window[0], Message::User { content } if
            matches!(content.first(), Some(UserContent::Text(t)) if t.text == "m0")));
    }

    #[test]
    fn adaptive_policy_no_compact_when_short() {
        // 消息数 ≤ 保护条数：全保留不压缩
        let policy = AdaptiveWindowPolicy::new(1);
        let msgs: Vec<Message> = (0..3)
            .map(|i| Message::User {
                content: vec![UserContent::text(format!("m{i}"))],
            })
            .collect();
        let (window, demoted) = policy.apply_with_demoted(msgs).unwrap();
        assert_eq!(window.len(), 3);
        assert!(demoted.is_empty());
    }

    #[test]
    fn adaptive_policy_keeps_within_budget_when_head_small() {
        // 大预算下行为等价 TokenWindowMemory：全部保留
        let policy = AdaptiveWindowPolicy::new(100_000);
        let msgs: Vec<Message> = (0..10)
            .map(|i| Message::User {
                content: vec![UserContent::text(format!("m{i}"))],
            })
            .collect();
        let (window, demoted) = policy.apply_with_demoted(msgs).unwrap();
        assert_eq!(window.len(), 10);
        assert!(demoted.is_empty());
    }

    #[test]
    fn adaptive_policy_head_bypasses_budget() {
        // head 消息即使体量巨大也不逐出（任务指令保护优先于预算）：
        // 第 1 条 100KB 文本 + 后续小消息，预算极小 → head 全保 + 其余被裁
        let big = "任".repeat(50_000); // 150KB UTF-8 ≈ 37.5k token，远超预算
        let mut msgs = vec![Message::User {
            content: vec![UserContent::text(big)],
        }];
        msgs.extend((0..4).map(|i| Message::User {
            content: vec![UserContent::text(format!("m{i}"))],
        }));
        let policy = AdaptiveWindowPolicy::new(1);
        let (window, demoted) = policy.apply_with_demoted(msgs).unwrap();
        assert_eq!(window.len(), 3, "head 3 条（含巨大首条）保留");
        assert_eq!(demoted.len(), 2);
    }

    #[test]
    fn adaptive_policy_orphan_tool_result_cleanup_still_works() {
        // 交互回归：head splice 后内层 TokenWindowMemory 的孤立 tool-result
        // 清理照常生效——rig 会把「开头是 tool_result」的消息挪进 demoted。
        // 构造：head 3 条普通消息 + 1 条孤立 tool_result + 1 条普通（预算极小，
        // 除 head 外全部逐出 → 内层返回空 window，无孤儿可清理的退化路径），
        // 再构造预算恰好装下 tool_result 之外的一条，验证 rig 行为不被 head 破坏。
        let mk_tool_result = || Message::User {
            content: vec![UserContent::ToolResult(rig::completion::message::ToolResult {
                call: rig::completion::message::CallId::from_wire("call-1"),
                name: "t1".try_into().expect("非空工具名"),
                content: vec![rig::completion::message::ToolResultContent::text("result")],
                is_error: false,
            })],
        };
        // 路径 A：head 后紧跟孤立 tool_result，预算极小 → 除 head 全逐出，
        // demoted 含 tool_result（rig 从 window 头挪进 demoted 或本就逐出）
        let mut msgs: Vec<Message> = (0..3)
            .map(|i| Message::User {
                content: vec![UserContent::text(format!("h{i}"))],
            })
            .collect();
        msgs.push(mk_tool_result());
        msgs.push(Message::User {
            content: vec![UserContent::text("tail")],
        });
        let policy = AdaptiveWindowPolicy::new(1);
        let (window, demoted) = policy.apply_with_demoted(msgs).unwrap();
        // head 全保留；内层预算极小时 window 应为空或已被 rig 清理孤立对
        assert_eq!(window.len(), 3);
        assert_eq!(demoted.len(), 2);
        // demoted 中 tool_result 可存在（将被摘要吸收，不影响 head）
        let has_tool_result = demoted.iter().any(|m| {
            matches!(m, Message::User { content } if
                matches!(content.first(), Some(UserContent::ToolResult(_))))
        });
        assert!(has_tool_result, "孤立 tool_result 应在 demoted 侧而非 head");
    }

    #[test]
    fn rows_to_messages_skips_streaming_and_empty() {
        let rows = vec![
            memory::MessageRow {
                id: "1".into(),
                conversation_id: "c".into(),
                role: MessageRole::User,
                status: MessageStatus::Complete,
                content: "hi".into(),
                reasoning: None,
                error: None,
                model: None,
                created_at: memory::Timestamp::now(),
                updated_at: memory::Timestamp::now(),
                prompt_tokens: None,
                completion_tokens: None,
                total_tokens: None,
            },
            memory::MessageRow {
                id: "2".into(),
                conversation_id: "c".into(),
                role: MessageRole::Assistant,
                status: MessageStatus::Streaming,
                content: "".into(),
                reasoning: None,
                error: None,
                model: None,
                created_at: memory::Timestamp::now(),
                updated_at: memory::Timestamp::now(),
                prompt_tokens: None,
                completion_tokens: None,
                total_tokens: None,
            },
        ];
        let msgs = rows_to_messages(&rows);
        assert_eq!(msgs.len(), 1, "streaming 空消息应被跳过");
    }

    #[tokio::test]
    async fn append_is_noop() {
        let mem = SqliteMemory(Arc::new(Memory::open_in_memory().unwrap()));
        // append 应是 no-op，不写入
        <SqliteMemory as ConversationMemory>::append(
            &mem,
            &ConversationId::new("nonexistent"),
            vec![Message::User {
                content: vec![UserContent::text("test")],
            }],
        )
        .await
        .unwrap();
        let rows = mem.0.list_messages("nonexistent").unwrap();
        assert!(rows.is_empty(), "append no-op 不应写入");
    }

    #[test]
    fn compaction_text_formats_roles() {
        let msgs = vec![
            Message::User {
                content: vec![UserContent::text("你好")],
            },
            Message::Assistant(rig::completion::message::AssistantMessage::new(vec![AssistantContent::text(
                "在的",
            )])),
        ];
        let text = messages_to_compaction_text(&msgs);
        assert!(text.contains("用户：你好"));
        assert!(text.contains("助手：在的"));
    }
}
