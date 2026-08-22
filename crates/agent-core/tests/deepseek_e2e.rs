//! 端到端验证：用 DeepSeek API 跑一次流式对话。
//!
//! 运行：DEEPSEEK_API_KEY=sk-... cargo test -p agent-core --test deepseek_e2e -- --nocapture --ignored
//! （标记 #[ignore] 避免普通 CI 触发真实网络调用。）

use std::env;

use agent_core::{text_prompt, ChatService, ProviderConfig, ProviderKind, StreamKind};
use futures_util::StreamExt;

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
#[ignore = "真实网络调用，手动验证用"]
async fn deepseek_stream_chat_e2e() {
    let config = e2e_config();

    let service = ChatService::new(config).expect("ChatService::new");

    let prompt = text_prompt("Reply with exactly the word: hello");
    let mut stream = service
        .stream(prompt, vec![])
        .await
        .expect("stream start");

    let mut full_text = String::new();
    let mut got_done = false;

    while let Some(chunk) = stream.next().await {
        eprintln!("[chunk] {:?} {:?}", chunk.kind, if chunk.text.is_empty() { "" } else { &chunk.text });
        match chunk.kind {
            StreamKind::TextDelta => full_text.push_str(&chunk.text),
            StreamKind::ReasoningDelta => {
                eprintln!("[reasoning] {}", chunk.text);
            }
            StreamKind::Done => got_done = true,
            StreamKind::Error => {
                panic!("stream error: {}", chunk.text);
            }
            StreamKind::ToolCallStart | StreamKind::ToolCallResult => {
                eprintln!("[tool] {:?}", chunk.tool_call);
            }
            StreamKind::Usage => {
                eprintln!("[usage] {:?}", chunk.usage);
            }
        }
    }

    assert!(got_done, "应收到 Done chunk");
    assert!(
        full_text.to_lowercase().contains("hello"),
        "回复应包含 hello，实际：{full_text}"
    );
    eprintln!("✅ 端到端验证通过，完整回复：{full_text}");
}

#[tokio::test]
#[ignore = "真实网络调用，手动验证用"]
async fn deepseek_multi_turn_with_history() {
    let config = e2e_config();

    let service = ChatService::new(config).expect("ChatService::new");

    // 第一轮
    let prompt1 = text_prompt("My name is Alice. Remember it.");
    let mut s1 = service.stream(prompt1, vec![]).await.expect("stream1");
    let mut reply1 = String::new();
    while let Some(chunk) = s1.next().await {
        if chunk.kind == StreamKind::TextDelta {
            reply1.push_str(&chunk.text);
        }
    }
    eprintln!("[turn1] {reply1}");

    // 第二轮带历史：问"我叫什么名字？"
    use agent_core::text_history_to_messages;
    use memory::{MessageRole, MessageRow, MessageStatus};

    let history = vec![
        MessageRow {
            id: "h1".into(),
            conversation_id: "c1".into(),
            role: MessageRole::User,
            content: "My name is Alice. Remember it.".into(),
            reasoning: None,
            status: MessageStatus::Complete,
            error: None,
            model: None,
            created_at: memory::Timestamp::now(),
            updated_at: memory::Timestamp::now(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
        },
        MessageRow {
            id: "h2".into(),
            conversation_id: "c1".into(),
            role: MessageRole::Assistant,
            content: reply1,
            reasoning: None,
            status: MessageStatus::Complete,
            error: None,
            model: Some(MODEL.into()),
            created_at: memory::Timestamp::now(),
            updated_at: memory::Timestamp::now(),
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
        },
    ];
    let history_msgs = text_history_to_messages(&history);
    let prompt2 = text_prompt("What is my name? Reply with just the name.");

    let mut s2 = service.stream(prompt2, history_msgs).await.expect("stream2");
    let mut reply2 = String::new();
    while let Some(chunk) = s2.next().await {
        if chunk.kind == StreamKind::TextDelta {
            reply2.push_str(&chunk.text);
        }
        if chunk.kind == StreamKind::Error {
            panic!("turn2 error: {}", chunk.text);
        }
    }
    eprintln!("[turn2] {reply2}");
    assert!(
        reply2.to_lowercase().contains("alice"),
        "第二轮应记住名字 Alice，实际：{reply2}"
    );
    eprintln!("✅ 多轮对话历史验证通过");
}
