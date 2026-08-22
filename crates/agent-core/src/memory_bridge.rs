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
//!   - `CompactingMemory` 包裹 Memory + `TokenWindowMemory` policy + `LlmCompactor`：
//!     load 时自动裁剪超预算的旧历史并生成滚动摘要（carry_over），splice 进返回 history。
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
use rig::memory::{ConversationMemory, MemoryError};
use rig::wasm_compat::WasmBoxedFuture;
use rig_memory::{CompactingMemory, HeuristicTokenCounter, TokenWindowMemory};

use memory::{MessageRole, MessageStatus, Memory};

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
        conversation_id: &'a str,
    ) -> WasmBoxedFuture<'a, Result<Vec<Message>, MemoryError>> {
        Box::pin(async move {
            let rows = self
                .0
                .list_messages_limited(conversation_id, Some(MESSAGE_LOAD_LIMIT))
                .map_err(|e| MemoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))?;
            Ok(rows_to_messages(&rows))
        })
    }

    fn append<'a>(
        &'a self,
        conversation_id: &'a str,
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
        conversation_id: &'a str,
    ) -> WasmBoxedFuture<'a, Result<(), MemoryError>> {
        Box::pin(async move {
            self.0
                .delete_conversation_messages(conversation_id)
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
                && matches!(m.role, MessageRole::User | MessageRole::Assistant | MessageRole::System)
        })
        .map(|m| match m.role {
            MessageRole::User => Message::User {
                content: rig::OneOrMany::one(UserContent::text(m.content.clone())),
            },
            MessageRole::Assistant => Message::Assistant {
                id: None,
                content: rig::OneOrMany::one(AssistantContent::text(m.content.clone())),
            },
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
        _conversation_id: &'a str,
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
            Message::User { content } => {
                ("用户", first_text(content))
            }
            Message::Assistant { content, .. } => {
                ("助手", first_text_assistant(content))
            }
            Message::System { content } => {
                ("系统", content.clone())
            }
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

fn first_text(content: &rig::OneOrMany<UserContent>) -> String {
    for c in content.iter() {
        if let UserContent::Text(t) = c {
            return t.text.clone();
        }
    }
    String::new()
}

fn first_text_assistant(content: &rig::OneOrMany<AssistantContent>) -> String {
    for c in content.iter() {
        if let AssistantContent::Text(t) = c {
            return t.text.clone();
        }
    }
    String::new()
}

// ── 构造：Memory + CompactingMemory 组合 ───────────────────────

/// 构造一个带自动压缩的 `ConversationMemory`。
///
/// - `memory`: SQLite 后端（Arc 共享）
/// - `context_window`: 模型上下文窗口 token 数（token 预算）
/// - `summarize`: LLM 摘要函数（provider client 提供）
///
/// 返回的 `CompactingMemory` 可直接挂到 `AgentBuilder::memory()`。
/// load 时自动裁剪超 `context_window` 的旧历史并滚动摘要。
pub fn build_compacting_memory(
    memory: Arc<Memory>,
    context_window: usize,
    summarize: SummaryFn,
) -> CompactingMemory<SqliteMemory, TokenWindowMemory, LlmCompactor> {
    let policy = TokenWindowMemory::new(context_window, HeuristicTokenCounter::openai());
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
            "nonexistent",
            vec![Message::User {
                content: rig::OneOrMany::one(UserContent::text("test")),
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
                content: rig::OneOrMany::one(UserContent::text("你好")),
            },
            Message::Assistant {
                id: None,
                content: rig::OneOrMany::one(AssistantContent::text("在的")),
            },
        ];
        let text = messages_to_compaction_text(&msgs);
        assert!(text.contains("用户：你好"));
        assert!(text.contains("助手：在的"));
    }
}
