//! 端到端验证：模拟 send_message 命令的完整流程（memory 模式）。
//!
//! 流程（对齐 src-tauri/src/commands/chat.rs 的 send_message，二期 B1/B2 重构后）：
//! 1. Memory + ChatService.set_memory（挂 CompactingMemory）
//! 2. 预建历史消息（验证 rig load 能读到）
//! 3. ChatService::stream_with_memory(prompt, conv_id)
//! 4. 收集 StreamChunk → turn 结束整条落库
//!
//! 运行：DEEPSEEK_API_KEY=sk-... cargo test -p agent-core --test send_message_e2e -- --nocapture --ignored

use std::env;
use std::sync::Arc;

use agent_core::{ChatService, ProviderConfig, ProviderKind, StreamKind};
use futures_util::StreamExt;
use memory::{Memory, MessageRole, MessageStatus};

/// API Key 从环境变量读取（敏感信息不硬编码进仓库）。
fn api_key() -> String {
    env::var("DEEPSEEK_API_KEY")
        .expect("请先设置 DEEPSEEK_API_KEY 环境变量再运行 e2e 测试")
}
const BASE_URL: &str = "https://api.deepseek.com/v1";
const MODEL: &str = "deepseek-chat";

/// 构造 ProviderConfig（用工厂函数补全默认字段，避免 struct 新增字段时初始化漂移）。
fn e2e_config() -> ProviderConfig {
    let mut config = ProviderConfig::openai_compatible(api_key(), MODEL);
    config.base_url = Some(BASE_URL.to_string());
    config
}

#[tokio::test]
#[ignore = "真实网络调用"]
async fn send_message_memory_mode_e2e() {
    // ── 1. 初始化 Memory + ChatService（挂 memory） ──
    let mem = Arc::new(Memory::open_in_memory().expect("open_in_memory"));
    let conv = mem
        .create_conversation(Some("E2E Test"))
        .expect("create_conversation");
    eprintln!("[memory] 对话已创建: {} ({})", conv.id, conv.title);

    let config = e2e_config();
    let mut service = ChatService::new(config).expect("ChatService::new");
    // 挂 memory：context_window 给个大值，避免本测试触发压缩
    service.set_memory(mem.clone(), 100_000);

    // ── 2. 预建一条历史 user 消息（验证 rig load 能读到历史） ──
    mem.create_message(
        &conv.id,
        MessageRole::User,
        MessageStatus::Complete,
        "My name is Alice. Remember it.",
        None,
    )
    .expect("create history user");
    mem.create_message(
        &conv.id,
        MessageRole::Assistant,
        MessageStatus::Complete,
        "Got it, Alice.",
        Some(MODEL),
    )
    .expect("create history assistant");

    // ── 3. 发起流式（memory 模式） ──
    // prompt = 本轮输入（不手动建消息，避免与 load 重复）
    let prompt = agent_core::text_prompt("What is my name? Reply with just the name.");
    let mut stream = service
        .stream_with_memory(prompt, &conv.id, false)
        .await
        .expect("stream_with_memory");

    // ── 4. 收集 chunk ──
    let mut full_text = String::new();
    let mut chunk_count = 0u32;
    while let Some(chunk) = stream.next().await {
        chunk_count += 1;
        match chunk.kind {
            StreamKind::TextDelta => full_text.push_str(&chunk.text),
            StreamKind::ReasoningDelta => eprintln!("[reasoning] {}", chunk.text),
            StreamKind::Usage => eprintln!("[usage] {:?}", chunk.usage),
            StreamKind::Done => {
                eprintln!("[done] ({} chunks)", chunk_count);
                break;
            }
            StreamKind::Error => panic!("stream error: {}", chunk.text),
            StreamKind::ToolCallStart | StreamKind::ToolCallResult => {
                eprintln!("[tool] {:?}", chunk.tool_call);
            }
        }
    }

    // ── 5. turn 结束整条落库（user + assistant） ──
    let user_id = uuid::Uuid::new_v4().to_string();
    let asst_id = uuid::Uuid::new_v4().to_string();
    mem.create_message_with_id(
        &conv.id, MessageRole::User, MessageStatus::Complete,
        "What is my name? Reply with just the name.", None, &user_id,
    ).expect("create user");
    mem.create_message_with_id(
        &conv.id, MessageRole::Assistant, MessageStatus::Complete,
        &full_text, Some(MODEL), &asst_id,
    ).expect("create assistant");

    eprintln!("[result] 回复内容: {full_text}");

    // ── 6. 断言 ──
    // 模型应从历史记住 Alice
    assert!(
        full_text.to_lowercase().contains("alice"),
        "应回答 Alice，实际：{full_text}"
    );

    // 从 DB 读回验证：2 历史 + 2 本轮 = 4
    let msgs = mem.list_messages(&conv.id).expect("list_messages");
    assert_eq!(msgs.len(), 4, "应有 4 条消息（2 历史 + 2 本轮），实际 {}", msgs.len());

    eprintln!("✅ memory 模式 send_message 验证通过");
}
