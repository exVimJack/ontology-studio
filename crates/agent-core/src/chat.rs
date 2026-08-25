//! 流式对话服务。
//!
//! 封装 Rig 0.41 的 agent + streaming，对外暴露统一的 `StreamChunk` 流。
//! 对齐 ARCHITECTURE.md §13.2 的 StreamChunk 契约（一期子集：TextDelta/ReasoningDelta/Done/Error）。
//!
//! 设计说明（provider 类型分歧）：
//! Rig 的 `Agent<M>` / `Client<Ext,H>` 是高阶泛型，不同 provider 的具体类型不同，
//! 无法直接 trait-object 化。采用 **`ProviderRuntime` trait** 方案：定义一个 trait 把
//! 「构造 agent + 设采样参数 + 流式/prompt」封装为返回已擦除类型（`StreamChunk` 流 /
//! `String`）的 method，每个 provider kind 在 `with_tools` 时构造具体 client 并立即
//! 包成 `Box<dyn ProviderRuntime>`。`ChatService` 持 trait object，所有调用点零 match 分支。
//! 新增 provider 只需：① with_tools 加构造分支 ② impl ProviderRuntime 一块。

use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use rig::client::AgentClientExt;
use specta_typescript::Number;
use rig::agent::{MultiTurnStreamItem, StreamingError};
use rig::tool::server::ToolServerHandle;
use rig::completion::message::{AssistantContent, Message, UserContent};
use rig::providers::{anthropic, cohere, deepseek, gemini, groq, mistral, moonshot, ollama, openai, openrouter, perplexity, xai, zai};
use rig::streaming::{StreamedAssistantContent, StreamingChat, StreamingPrompt};
use serde::{Deserialize, Serialize};

use crate::error::{AgentError, AgentResult};
use crate::provider::{
    reasoning_to_params, ProviderConfig, ReasoningLevel,
};

/// Agent 多轮工具调用的最大模型调用次数（max_turns = total model-call budget）。
///
/// Rig 0.41 在未配 `default_max_turns` 时隐式预算 = 1 次模型调用（仅发 prompt，
/// 不进多轮循环），带 tool 的对话第一轮工具调用后即撞 MaxTurnsError。
///
/// 语义（rig 官方文档）：max_turns 含初始调用 + 每次重试/续轮。
/// - 2 = 1 次工具调用 + 1 次最终回答（最简单场景）
/// - 3 = rig 官方 typed 输出示例用的值
/// - 100 = 本项目默认。联邦探索 + agentic search + MCP 工具链都可能多轮试错，
///   100 给模型充足预算自主探索（list → describe → query → 修正 → 聚合 → 回答）。
///   参考：OpenAI Assistants run max 128、Claude tool use 实践常 20–50 轮。
///   无硬上限，100 平衡“几乎不会撞限”与“防失控”。
const MAX_TURNS: usize = 100;
// ── 对外 chunk 契约（§13.2 一期子集） ──────────────────────────

/// 流式 chunk 种类。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// 文本增量
    TextDelta,
    /// 思考/reasoning 增量（reasoning 模型）
    ReasoningDelta,
    /// 工具调用开始（模型决定调用某工具，即将执行）
    /// `text` = 工具名；`tool_call` 附带调用详情
    ToolCallStart,
    /// 工具调用结果（执行完成）
    /// `text` = 结果文本（模型可见）；`tool_call` 附带调用详情与结果
    ToolCallResult,
    /// provider 报告的真实 token usage（二期 B1）
    /// `usage` 附带输入/输出/总 token；可在 Done 前多次发出（多轮工具调用）
    Usage,
    /// 流式正常结束
    Done,
    /// 出错结束
    Error,
}

/// 工具调用详情（随 ToolCallStart/ToolCallResult 一起发出）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ToolCallInfo {
    /// 工具名（MCP server 注册的 name）
    pub name: String,
    /// 模型传入的 JSON 参数（原始字符串）
    pub arguments: String,
    /// Rig 内部调用 ID（同一轮多个调用可关联）
    pub call_id: String,
    /// 工具结果（仅 ToolCallResult 时有意义，ToolCallStart 为 None）
    pub result: Option<String>,
    /// 是否出错（工具报告 is_error 或执行异常）
    pub is_error: bool,
}

/// provider 报告的真实 token usage（二期 B1）。
///
/// 随 `StreamChunk::Usage` 发出，供上层落库到 assistant 消息，
/// 下次发消息前用于压缩判定（替代 chars/4 估算）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, specta::Type)]
pub struct TokenUsage {
    #[specta(type = Number)]
    pub input_tokens: u64,
    #[specta(type = Number)]
    pub output_tokens: u64,
    #[specta(type = Number)]
    pub total_tokens: u64,
}

/// 流式 chunk（对齐 §13.2 StreamChunk）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct StreamChunk {
    pub kind: StreamKind,
    /// TextDelta: 增量文本；ReasoningDelta: 增量思考；Done/Error: 可空；
    /// ToolCallStart/Result: 工具结果文本（Result）或空（Start）
    pub text: String,
    /// 工具调用详情（仅 ToolCallStart/Result 有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallInfo>,
    /// token usage（仅 Usage 有值）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

impl StreamChunk {
    pub fn text_delta(s: impl Into<String>) -> Self {
        Self { kind: StreamKind::TextDelta, text: s.into(), tool_call: None, usage: None }
    }
    pub fn reasoning_delta(s: impl Into<String>) -> Self {
        Self { kind: StreamKind::ReasoningDelta, text: s.into(), tool_call: None, usage: None }
    }
    pub fn tool_call_start(info: ToolCallInfo) -> Self {
        Self { kind: StreamKind::ToolCallStart, text: String::new(), tool_call: Some(info), usage: None }
    }
    pub fn tool_call_result(text: impl Into<String>, info: ToolCallInfo) -> Self {
        Self { kind: StreamKind::ToolCallResult, text: text.into(), tool_call: Some(info), usage: None }
    }
    pub fn usage(u: TokenUsage) -> Self {
        Self { kind: StreamKind::Usage, text: String::new(), tool_call: None, usage: Some(u) }
    }
    pub fn done() -> Self {
        Self { kind: StreamKind::Done, text: String::new(), tool_call: None, usage: None }
    }
    pub fn error(s: impl Into<String>) -> Self {
        Self { kind: StreamKind::Error, text: s.into(), tool_call: None, usage: None }
    }
}

// ── ChatService ───────────────────────────────────────────────

/// 强类型 client 容器。按 provider kind 分支。
/// 构造 agent 时的可选参数（采样 + preamble + reasoning + memory + tool handle）。
/// 传给 `ProviderRuntime` 的各方法，避免每个 method 重复传参。
struct AgentOpts {
    preamble: Option<String>,
    reasoning_params: Option<serde_json::Value>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
}

impl Clone for AgentOpts {
    fn clone(&self) -> Self {
        Self {
            preamble: self.preamble.clone(),
            reasoning_params: self.reasoning_params.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
        }
    }
}

/// Provider 运行时 trait：擦除 rig client 的具体泛型类型。
///
/// 每个 provider kind 实现此 trait，把「构造 agent + 流式/prompt」封装为返回
/// 已擦除类型的方法。`ChatService` 持 `Box<dyn ProviderRuntime>`，调用点零分支。
trait ProviderRuntime: Send + Sync {
    /// 流式对话（带 history，无 memory）。
    fn stream_chat(
        &self,
        model: &str,
        prompt: Message,
        history: Vec<Message>,
        handle: Option<ToolServerHandle>,
        opts: &AgentOpts,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = AgentResult<ChatStream>> + Send>,
    >;

    /// 流式对话（memory 模式，按 conversation_id 挂 CompactingMemory）。
    fn stream_with_memory(
        &self,
        model: &str,
        prompt: Message,
        conversation_id: String,
        memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
        handle: Option<ToolServerHandle>,
        opts: &AgentOpts,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = AgentResult<ChatStream>> + Send>,
    >;

    /// 单次非流式 prompt（供摘要/标题生成用）。
    fn prompt_text(
        &self,
        model: &str,
        prompt: String,
        opts: &AgentOpts,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = AgentResult<String>> + Send>,
    >;
}

/// 已擦除类型的流式输出（`StreamChunk` 流）。
type ChatStream = std::pin::Pin<Box<dyn Stream<Item = StreamChunk> + Send>>;

/// 泛型 helper：为任意 `AgentClientExt` client 构造 agent 并流式对话。
///
/// 调用方传入 client 引用 + 配置，函数内部构造 `AgentBuilder`、设采样参数/
/// preamble/reasoning/memory/tool handle，build 后调 stream_chat/stream_prompt/prompt。
/// 返回已擦除类型的流/字符串，消除 trait impl 中的重复。
mod helper {
    use super::*;

    /// 构造 agent builder（设 preamble/memory/采样/reasoning）。
    /// 返回的 builder 尚未 build（调用方决定是否挂 tool_server_handle）。
    fn build_agent<C: AgentClientExt>(
        c: &C,
        model: &str,
        opts: &AgentOpts,
        memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
    ) -> rig::agent::AgentBuilder<C::CompletionModel>
    where
        <C as rig::client::CompletionClient>::CompletionModel: 'static,
    {
        let mut b = c.agent(model);
        if let Some(p) = &opts.preamble {
            if !p.is_empty() {
                b = b.preamble(p);
            }
        }
        if let Some(m) = memory {
            b = b.memory(m);
        }
        if let Some(t) = opts.temperature {
            b = b.temperature(t);
        }
        if let Some(m) = opts.max_tokens {
            b = b.max_tokens(m);
        }
        if let Some(p) = &opts.reasoning_params {
            b = b.additional_params(p.clone());
        }
        b
    }

    /// 流式对话（带 history，无 memory）。
    pub(crate) async fn stream_chat<C>(
        c: &C,
        model: &str,
        prompt: Message,
        history: Vec<Message>,
        handle: Option<ToolServerHandle>,
        opts: &AgentOpts,
    ) -> AgentResult<ChatStream>
    where
        C: AgentClientExt,
        <C as rig::client::CompletionClient>::CompletionModel: 'static,
        <<C as rig::client::CompletionClient>::CompletionModel as rig::completion::CompletionModel>::StreamingResponse:
            rig::completion::GetTokenUsage + Clone + Unpin + Send + 'static,
    {
        let b = build_agent(c, model, opts, None);
        let inner = if let Some(h) = handle {
            let agent = b.tool_server_handle(h).build();
            agent.stream_chat(prompt, history).max_turns(MAX_TURNS).await
        } else {
            let agent = b.build();
            agent.stream_chat(prompt, history).max_turns(MAX_TURNS).await
        };
        Ok(Box::pin(map_multi_turn_stream(inner)))
    }

    /// 流式对话（memory 模式）。
    pub(crate) async fn stream_with_memory<C>(
        c: &C,
        model: &str,
        prompt: Message,
        conversation_id: String,
        memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
        handle: Option<ToolServerHandle>,
        opts: &AgentOpts,
    ) -> AgentResult<ChatStream>
    where
        C: AgentClientExt,
        <C as rig::client::CompletionClient>::CompletionModel: 'static,
        <<C as rig::client::CompletionClient>::CompletionModel as rig::completion::CompletionModel>::StreamingResponse:
            rig::completion::GetTokenUsage + Clone + Unpin + Send + 'static,
    {
        let b = build_agent(c, model, opts, memory);
        let inner = if let Some(h) = handle {
            let agent = b.tool_server_handle(h).build();
            agent.stream_prompt(prompt).conversation(conversation_id).max_turns(MAX_TURNS).await
        } else {
            let agent = b.build();
            agent.stream_prompt(prompt).conversation(conversation_id).max_turns(MAX_TURNS).await
        };
        Ok(Box::pin(map_multi_turn_stream(inner)))
    }

    /// 单次非流式 prompt。
    pub(crate) async fn prompt_text<C>(
        c: &C,
        model: &str,
        prompt: String,
        opts: &AgentOpts,
    ) -> AgentResult<String>
    where
        C: AgentClientExt,
        <C as rig::client::CompletionClient>::CompletionModel: 'static,
    {
        use rig::completion::Prompt;
        use std::future::IntoFuture;
        let b = build_agent(c, model, opts, None);
        let agent = b.build();
        agent
            .prompt(&prompt)
            .into_future()
            .await
            .map_err(|e| AgentError::Provider(format!("prompt: {e}")))
    }
}

/// 按当前配置构造 `AgentOpts`（采样 + reasoning；preamble 由调用方注入）。
fn make_opts(reasoning_params: Option<serde_json::Value>, config: &ProviderConfig) -> AgentOpts {
    AgentOpts {
        preamble: None, // stream/stream_with_memory 各自注入完整 preamble
        reasoning_params,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
    }
}

// ── ProviderRuntime 实现：每个 provider kind 一块 ──────────────
// 用宏生成重复 impl，避免 13 个 provider × 3 方法的手写重复。
// 每个 impl 都是「调 helper::xxx(self.client, …)」的一行转发。
macro_rules! impl_provider_runtime {
    ($ty:ty) => {
        impl ProviderRuntime for $ty {
            fn stream_chat(
                &self,
                model: &str,
                prompt: Message,
                history: Vec<Message>,
                handle: Option<ToolServerHandle>,
                opts: &AgentOpts,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = AgentResult<ChatStream>> + Send>,
            > {
                let c = self.0.clone();
                let model = model.to_string();
                let opts = opts.clone();
                Box::pin(async move {
                    helper::stream_chat(&c, &model, prompt, history, handle, &opts).await
                })
            }
            fn stream_with_memory(
                &self,
                model: &str,
                prompt: Message,
                conversation_id: String,
                memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
                handle: Option<ToolServerHandle>,
                opts: &AgentOpts,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = AgentResult<ChatStream>> + Send>,
            > {
                let c = self.0.clone();
                let model = model.to_string();
                let opts = opts.clone();
                Box::pin(async move {
                    helper::stream_with_memory(
                        &c, &model, prompt, conversation_id, memory, handle, &opts,
                    )
                    .await
                })
            }
            fn prompt_text(
                &self,
                model: &str,
                prompt: String,
                opts: &AgentOpts,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = AgentResult<String>> + Send>,
            > {
                let c = self.0.clone();
                let model = model.to_string();
                let opts = opts.clone();
                Box::pin(async move { helper::prompt_text(&c, &model, prompt, &opts).await })
            }
        }
    };
}

/// 各 provider 的 client 包装（newtype 供宏 impl）。
struct OpenAiClient(openai::Client);          // Responses API
struct OpenAiCompletionsClient(openai::CompletionsClient); // Completions API（兼容端点）
struct AnthropicClient(anthropic::Client);
struct GeminiClient(gemini::Client);
struct DeepSeekClient(deepseek::Client);
struct XaiClient(xai::Client);
struct GroqClient(groq::Client);
struct OpenRouterClient(openrouter::Client);
struct OllamaClient(ollama::Client);
struct MoonshotClient(moonshot::Client);
struct ZaiClient(zai::Client);
struct MistralClient(mistral::Client);
struct CohereClient(cohere::Client);
struct PerplexityClient(perplexity::Client);

impl_provider_runtime!(OpenAiClient);
impl_provider_runtime!(OpenAiCompletionsClient);
impl_provider_runtime!(AnthropicClient);
impl_provider_runtime!(GeminiClient);
impl_provider_runtime!(DeepSeekClient);
impl_provider_runtime!(XaiClient);
impl_provider_runtime!(GroqClient);
impl_provider_runtime!(OpenRouterClient);
impl_provider_runtime!(OllamaClient);
impl_provider_runtime!(MoonshotClient);
impl_provider_runtime!(ZaiClient);
impl_provider_runtime!(MistralClient);
impl_provider_runtime!(CohereClient);
impl_provider_runtime!(PerplexityClient);

/// 对话服务。持有一个已配置好的 provider runtime（trait object，一期单 provider）
/// + 可选工具句柄 + 可选 memory/联邦/skill。
pub struct ChatService {
    runtime: Arc<dyn ProviderRuntime>,
    config: ProviderConfig,
    /// 可选 MCP 工具句柄。Some 时 agent 构建注入，模型可调用注册的工具。
    tool_handle: Option<ToolServerHandle>,
    /// 可选 memory backend（带 CompactingMemory 自动压缩）。
    /// set_memory 后构造，stream_with_memory 时挂到 agent。
    memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
    /// 原始 SQLite Memory 句柄（供文件检索工具访问 documents 表）。
    /// set_memory 时同步保存，stream_with_memory 时构造 document_tools。
    raw_memory: Option<Arc<memory::Memory>>,
    /// 可选联邦查询服务（三期阶段 1c：数据源查询作为 agent 工具）。
    /// set_federation 后保存，stream_with_memory 时构造 federation_tools。
    federation: Option<Arc<federation::FederationService>>,
    /// 可选本体存储（会话页面引用本体）。set_ontology_store 后保存，
    /// stream_with_memory 时构造 ontology_readonly_tools（5 个只读 drill-in）。
    ontology_store: Option<Arc<ontology_store::OntologyStore>>,
    /// 可选 W3C Turtle 本体存储（ontology-modeling-w3c skill 闭环）。
    /// set_ttl_store 后保存，stream_with_memory 时构造 ontology_ttl_tools
    /// （validate_ontology_ttl / import_ontology_ttl / export_ontology_ttl /
    /// list_ontology_ttl / query_ontology_sparql）。与 Palantir 工具组共存——
    /// skill preamble 渐进式披露控制模型用哪组（不做工具级动态过滤）。
    ttl_store: Option<Arc<ontology_store::TtlStore>>,
    /// 可选 Skill 管理器（决策 20）。set_skill_manager 后保存，
    /// stream_with_memory 时构造 preamble Tier 1 + active skill doc_paths。
    skill_manager: Option<Arc<crate::skill::SkillManager>>,
}

impl ChatService {
    /// 按配置构造 provider client（无工具）。
    pub fn new(config: ProviderConfig) -> AgentResult<Self> {
        Self::with_tools(config, None)
    }

    /// 构造时注入共享工具句柄（来自 McpManager）。
    pub fn with_tools(
        config: ProviderConfig,
        tool_handle: Option<ToolServerHandle>,
    ) -> AgentResult<Self> {
        let runtime: Arc<dyn ProviderRuntime> = build_runtime(&config)?;
        Ok(Self {
            runtime,
            config,
            tool_handle,
            memory: None,
            raw_memory: None,
            federation: None,
            ontology_store: None,
            ttl_store: None,
            skill_manager: None,
        })
    }

    /// 更新工具句柄（运行时动态增减 MCP server 后调用）。
    pub fn set_tool_handle(&mut self, handle: Option<ToolServerHandle>) {
        self.tool_handle = handle;
    }

    /// 注入 memory backend（带 CompactingMemory 自动压缩）。
    ///
    /// 接收 SQLite Memory + context_window，内部构造 CompactingMemory
    /// （TokenWindow 裁剪 + LLM 摘要），缓存为 trait object。
    /// 之后 `stream_with_memory` 调用会挂到 agent。
    ///
    /// context_window 由调用方解析（四层 fallback，见 context_window.rs）。
    pub fn set_memory(&mut self, memory: std::sync::Arc<memory::Memory>, context_window: usize) {
        self.raw_memory = Some(memory.clone());
        let summarize = self.build_summarize_fn();
        let compacting = crate::memory_bridge::build_compacting_memory(
            memory,
            context_window,
            summarize,
        );
        self.memory = Some(std::sync::Arc::new(compacting));
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// 注入联邦查询服务（三期阶段 1c）。
    ///
    /// 接收全局 `Arc<FederationService>`，`stream_with_memory` 时构造
    /// `federation_tools`（list_data_sources / describe_table / execute_sql）
    /// 与 document_tools 合并注入同一 ToolServerHandle。agent 自主决定是否调用。
    pub fn set_federation(&mut self, svc: std::sync::Arc<federation::FederationService>) {
        self.federation = Some(svc);
    }

    /// 注入本体存储（会话页面引用本体）。
    ///
    /// 接收全局 `Arc<OntologyStore>`，`stream_with_memory` 时构造
    /// `ontology_readonly_tools`（5 个只读 drill-in：describe_ontology /
    /// list_object_types / describe_object_type / list_link_types / describe_link_type）
    /// 与 document_tools / federation_tools 合并注入同一 ToolServerHandle。
    /// 会话模式不挂建模组（export/preview/import）——体积大 + 会话不需写库。
    /// 本体工具始终可用（无激活集过滤，会话引用场景下 agent 按需钻取）。
    pub fn set_ontology_store(&mut self, store: std::sync::Arc<ontology_store::OntologyStore>) {
        self.ontology_store = Some(store);
    }

    /// 注入 W3C Turtle 本体存储（ontology-modeling-w3c skill 闭环）。
    ///
    /// 接收全局 `Arc<TtlStore>`，`stream_with_memory` 时构造 `ontology_ttl_tools`
    /// （validate_ontology_ttl / import_ontology_ttl / export_ontology_ttl /
    /// list_ontology_ttl / query_ontology_sparql）与 Palantir 工具组合并注入同一
    /// ToolServerHandle。agent 按 skill preamble 指引用哪组工具。始终可用（无激活
    /// 集过滤）。
    pub fn set_ttl_store(&mut self, store: std::sync::Arc<ontology_store::TtlStore>) {
        self.ttl_store = Some(store);
    }

    /// 注入 Skill 管理器（决策 20）。
    ///
    /// 接收全局 `Arc<SkillManager>`，`stream_with_memory` 时：
    ///   1. 调 `build_preamble_section(conv_id)` 生成 Skill Tier 1 XML，
    ///      与 ProviderConfig.preamble 拼接为完整 preamble 注入 AgentBuilder
    ///   2. 调 `active_skill_doc_paths(conv_id)` 把 skill doc path 合并进
    ///      doc_paths_set，供 read_document 工具读取 skill 全文（Tier 2）
    pub fn set_skill_manager(&mut self, sm: std::sync::Arc<crate::skill::SkillManager>) {
        self.skill_manager = Some(sm);
    }

    /// 发起流式对话。
    ///
    /// - `prompt`: 用户本轮输入（纯文本 Message；多模态图片一期由调用方组装 UserContent::Image）
    /// - `history`: 之前的消息（Rig Message），由调用方从 memory 取出组装
    ///
    /// 返回一个 async stream，逐个产出 `StreamChunk`。类型已擦除（Box<dyn Stream>）。
    pub async fn stream(
        &self,
        prompt: Message,
        history: Vec<Message>,
    ) -> AgentResult<std::pin::Pin<Box<dyn Stream<Item = StreamChunk> + Send>>> {
        let handle = self.tool_handle.clone();
        let opts = make_opts(self.reasoning_params(), &self.config);
        self.runtime
            .stream_chat(&self.config.model, prompt, history, handle, &opts)
            .await
    }

    /// 发起流式对话（memory 模式）。
    ///
    /// 不传 history——agent 挂的 `CompactingMemory` 自动 load 该会话历史，
    /// 并在超 token 预算时自动裁剪 + 生成滚动摘要（carry_over）。
    /// turn 结束后 rig 会调 memory.append（我们的实现是 no-op，消息由调用方手动建）。
    ///
    /// 需先调 `set_memory` 注入 backend，否则退化为无历史对话。
    ///
    /// `enable_reasoning`：开启深度思考。按 provider kind 透传对应参数到
    /// provider 请求体（OpenAI: `reasoning_effort`; Anthropic: `thinking`）。
    /// rig 的 `additional_params` 经 `#[serde(flatten)]` 展开到 body 顶层。
    pub async fn stream_with_memory(
        &self,
        prompt: Message,
        conversation_id: &str,
        enable_reasoning: bool,
    ) -> AgentResult<std::pin::Pin<Box<dyn Stream<Item = StreamChunk> + Send>>> {
        let memory = self.memory.clone();
        let handle = self.tool_handle.clone();
        let conv_id = conversation_id.to_string();
        let reasoning_params = self.reasoning_params_runtime(enable_reasoning);
        // 文件检索工具（一期收尾：agentic search）。需 memory 句柄访问 documents 表。
        // 联邦查询工具（三期阶段 1c：数据源查询作为 agent 工具）。
        // 两者与 MCP 工具合并进同一 ToolServerHandle（PHASE3-FEDERATION.md §2.1 范式）。
        // 注入方式：复用 MCP tool_handle（add_dynamic_tool），或无 handle 时新建空 ToolServer。
        // 这样文件工具 + 联邦工具 + MCP 工具共存（rig 0.41 builder typestate 不允许同时
        // dynamic_tools + tool_server_handle，但 ToolServerHandle 运行时可动态加）。
        //
        // 会话激活集过滤（CONVERSATION-SCOPE.md §4）：
        //   - 解析本会话激活的 doc_paths + source_names
        //   - doc_tools 只查激活的文件、fed_tools 只查激活的数据源
        //   - 激活集为空 → 两者都不挂（模型按通用能力回答）
        // raw_memory 不在（未注入 Memory 句柄）时退化为不挂文档工具。
        let (doc_paths, source_names): (Vec<String>, Vec<String>) = match &self.raw_memory {
            Some(m) => match (
                m.resolve_active_doc_paths(&conv_id),
                m.get_active_sources(&conv_id),
            ) {
                (Ok(p), Ok(s)) => (p, s),
                (e1, e2) => {
                    tracing::warn!(conv_id = %conv_id, doc_err = ?e1.err(), src_err = ?e2.err(), "resolve active scope failed, falling back to empty");
                    (Vec::new(), Vec::new())
                }
            },
            None => (Vec::new(), Vec::new()),
        };
        // Skill doc_paths 合并（决策 20）：把本会话激活的 skill doc path
        // （`skill://<name>`）加入 doc_paths_set，供 read_document 工具读取
        // skill 全文（Tier 2）。skill body 入库 documents 表，与文件检索统一路径。
        let mut doc_paths = doc_paths;
        if let Some(sm) = &self.skill_manager {
            match sm.active_skill_doc_paths(&conv_id) {
                Ok(skill_paths) => doc_paths.extend(skill_paths),
                Err(e) => {
                    tracing::warn!(conv_id = %conv_id, error = %e, "active skill doc paths failed, skipping");
                }
            }
        }
        let doc_paths_set = Arc::new(doc_paths.into_iter().collect::<std::collections::HashSet<_>>());
        let source_names_set = Arc::new(source_names.into_iter().collect::<std::collections::HashSet<_>>());
        tracing::info!(conv_id = %conv_id, active_docs = doc_paths_set.len(), active_sources = source_names_set.len(), "stream_with_memory: active scope");
        let doc_tools: Vec<_> = if !doc_paths_set.is_empty() {
            match &self.raw_memory {
                Some(m) => crate::document_tools::document_tools(m.clone(), doc_paths_set),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let fed_tools: Vec<_> = if !source_names_set.is_empty() {
            match &self.federation {
                Some(f) => crate::federation_tools::federation_tools(f.clone(), source_names_set),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        // 本体工具（决策：会话页面引用本体 + 会话内建模/删除）。
        // 只读 drill-in 5 件套（describe_ontology/list_object_types/describe_object_type/
        // list_link_types/describe_link_type）始终挂；建模组（export/preview/import）
        // + 实体删除组（delete_object_type/delete_link_type/delete_action_type/
        // delete_dataset/delete_data_source）也一并挂上——工具可见性靠
        // ontology-modeling skill 的 preamble 渐进式披露控制（对齐决策 20：
        // skill 文档告诉模型何时用哪组工具），不做工具级动态过滤。
        // 无激活集过滤：本体工具始终可用（会话引用场景下 agent 按需钻取）。
        let ont_tools: Vec<_> = match &self.ontology_store {
            Some(s) => {
                let mut tools = crate::ontology_tools::ontology_readonly_tools(s.clone());
                tools.extend(crate::ontology_tools::ontology_modeling_tools(s.clone()));
                tools.extend(crate::ontology_tools::ontology_delete_tools(s.clone()));
                tools.extend(crate::ontology_tools::ontology_changelog_tools(s.clone()));
                tools
            }
            None => Vec::new(),
        };
        // W3C Turtle 工具组（ontology-modeling-w3c skill 闭环：validate/import/
        // export/list/query_sparql）。与 Palantir 工具组共存——skill preamble 控制
        // 模型用哪组。始终挂（无激活集过滤）。
        let ttl_tools: Vec<_> = match &self.ttl_store {
            Some(s) => crate::ontology_ttl_tools::ontology_ttl_tools(s.clone()),
            None => Vec::new(),
        };
        let all_tools: Vec<_> = doc_tools.into_iter().chain(fed_tools).chain(ont_tools).chain(ttl_tools).collect();
        let handle = if !all_tools.is_empty() {
            // 复用已有 MCP handle，或新建空 ToolServer。两者都能 add_dynamic_tool。
            let h = handle.clone().unwrap_or_else(|| {
                rig::tool::server::ToolServer::new().run()
            });
            for tool in all_tools {
                h.add_dynamic_tool(tool).await;
            }
            Some(h)
        } else {
            handle
        };
        // 构建 preamble（系统人设 + Skill Tier 1）。onto-studio 首次引入 preamble（决策 20）。
        // 系统人设来自 ProviderConfig.preamble，skill 段来自 SkillManager（若有）。
        // 拼接顺序：系统人设在前（不变前缀，保 prefix cache），skill 段在后（后缀追加）。
        let base_preamble = self.config.preamble.as_deref().unwrap_or("").to_string();
        let skill_section = match &self.skill_manager {
            Some(sm) => match sm.build_preamble_section(&conv_id) {
                Ok(s) if !s.is_empty() => s,
                _ => String::new(),
            },
            None => String::new(),
        };
        let preamble = match (base_preamble.is_empty(), skill_section.is_empty()) {
            (true, true) => String::new(),
            (false, true) => base_preamble,
            (true, false) => skill_section,
            (false, false) => format!("{base_preamble}\n\n{skill_section}"),
        };
        let mut opts = make_opts(reasoning_params.clone(), &self.config);
        opts.preamble = if preamble.is_empty() { None } else { Some(preamble) };
        self.runtime
            .stream_with_memory(
                &self.config.model,
                prompt,
                conv_id,
                memory.clone(),
                handle,
                &opts,
            )
            .await
    }

    /// 按 provider kind 构造开启深度思考的 additional_params。
    ///
    /// - OpenAI 兼容（含 DeepSeek/OpenRouter/o1 系等）：`reasoning_effort: "high"`
    ///   （DeepSeek thinking 模式默认 high；low/medium 兼容映射 high）
    /// - Anthropic：`thinking: { type: "enabled", budget_tokens: 4096 }`
    ///   （Claude extended thinking）
    ///
    /// 关闭时返回 None（不注入任何参数，模型用默认行为）。
    /// 注：rig 0.41 未在 agent builder 暴露 reasoning 选项，`additional_params`
    /// 是官方留的 provider 透传口子（见 rig-agent builder.rs:274）。
    /// 计算当前配置的 reasoning additional_params。
    ///
    /// 按 `ProviderConfig.reasoning`（或 `enable_reasoning` 兼容旧调用）映射：
    ///   - OpenAI 官方 / DeepSeek（支持 reasoning_effort）：`{ reasoning_effort }`
    ///   - Anthropic：`{ thinking: { type, budget_tokens } }`
    ///   - Gemini：`{ generation_config: { thinking_config } }`
    ///   - 不支持的 provider / Off：None
    fn reasoning_params(&self) -> Option<serde_json::Value> {
        let level = self.config.reasoning_level();
        reasoning_to_params(
            self.config.kind,
            level,
            self.config.effective_supports_reasoning_effort(),
        )
    }

    /// 计算运行时 toggle 的 reasoning params（用户在 Composer 开关「深度思考」）。
    ///
    /// `enable=true` 映射 `ReasoningLevel::High`（覆盖 config 默认）；
    /// `enable=false` 映射 `Off`。
    fn reasoning_params_runtime(&self, enable: bool) -> Option<serde_json::Value> {
        let level = if enable { ReasoningLevel::High } else { ReasoningLevel::Off };
        reasoning_to_params(
            self.config.kind,
            level,
            self.config.effective_supports_reasoning_effort(),
        )
    }

    /// 构造 LLM 摘要闭包（供 LlmCompactor 调用）。
    ///
    /// 闭包 clone `Arc<dyn ProviderRuntime>` 句柄，调单次非流式 prompt 生成摘要。
    /// 失败返回 Err(String)，Compactor 转为 MemoryError::Backend。
    fn build_summarize_fn(&self) -> crate::memory_bridge::SummaryFn {
        let runtime = self.runtime.clone();
        let model = self.config.model.clone();
        let opts = make_opts(None, &self.config);
        std::sync::Arc::new(move |text: String| {
            let runtime = runtime.clone();
            let model = model.clone();
            let opts = opts.clone();
            Box::pin(async move {
                runtime
                    .prompt_text(&model, text, &opts)
                    .await
                    .map_err(|e| format!("summarize: {e}"))
            }) as _
        })
    }

    /// 生成摘要文本（二期 B2 历史压缩用）。
    ///
    /// 用同一 provider/model 做单次非流式补全：把传入的 `text`（被裁掉的旧历史）
    /// 压缩成要点摘要。不带 history、不带工具，纯 prompt 模式。
    ///
    /// 返回摘要文本。失败时上层应降级为「直接丢弃」（不阻断主对话）。
    pub async fn summarize_text(&self, text: &str) -> AgentResult<String> {
        let prompt = format!(
            "请将以下对话历史压缩成简洁的要点摘要，保留关键事实、决定与未完成事项。\n\
             用中文，不超过 300 字。只输出摘要正文，不要任何前后缀。\n\n\
             --- 对话历史 ---\n{text}"
        );
        let opts = make_opts(None, &self.config);
        self.runtime
            .prompt_text(&self.config.model, prompt, &opts)
            .await
    }

    /// 用首条用户消息生成会话标题（LLM 概括，非原文截断）。
    ///
    /// 对齐主流对话产品（ChatGPT / Claude.ai）的做法：发首条消息后，
    /// 起一次独立、轻量的 LLM 补全，把用户意图概括成简短标题。
    /// 不带历史、不带工具、不进会话 memory，纯 prompt 模式。
    ///
    /// Prompt 关键约束：
    ///   - 语言跟随用户输入（中文问→中文标题，英文问→英文标题）
    ///   - 简洁，通常 ≤8 词 / 16 个汉字
    ///   - 只输出标题正文，无引号 / markdown / 尾标点
    ///
    /// 失败时上层应降级为截断式兜底（见前端 deriveTitle），不阻断主流程。
    pub async fn generate_title(&self, first_user_message: &str) -> AgentResult<String> {
        // 超长输入截断，避免无谓的 token 消耗（标题只需把握意图）
        const MAX_INPUT_CHARS: usize = 1000;
        let trimmed = if first_user_message.chars().count() > MAX_INPUT_CHARS {
            let mut s: String = first_user_message.chars().take(MAX_INPUT_CHARS).collect();
            s.push_str("…(truncated)");
            s
        } else {
            first_user_message.to_string()
        };
        let prompt = format!(
            "根据以下用户的首条消息，生成一个简短的对话标题。\n\
             要求：\n\
             - 使用与用户消息相同的语言（用户用中文则标题用中文，英文则英文，以此类推）\n\
             - 简洁，通常不超过 8 个词或 16 个汉字\n\
             - 只输出标题正文，不要引号、不要 markdown 格式、不要结尾标点\n\
             - 不要解释，直接输出标题\n\
             \n\
             用户消息：\n{trimmed}"
        );
        let opts = make_opts(None, &self.config);
        let raw = self
            .runtime
            .prompt_text(&self.config.model, prompt, &opts)
            .await
            .map_err(|e| AgentError::Provider(format!("generate_title: {e}")))?;
        Ok(clean_title(&raw))
    }
}

/// 按配置构造 provider runtime（trait object）。
///
/// 内部按 `kind` 路由到 rig 原生 provider client：
///   - OpenAI 官方走 `responses_api()`，其余 OpenAI 兼容走 `completions_api()`
///   - DeepSeek/XAI/Groq/OpenRouter/Ollama/Moonshot/Mistral/Cohere/Perplexity 各走 rig 原生 provider
///     （自动声明 capability：SUPPORTS_TOOLS / EMITS_COMPLETE_SINGLE_CHUNK_TOOL_CALLS 等）
///   - Gemini 走 Google Generative AI 协议
///   - Anthropic 走 Messages API
///   - OpenAiCompatible 兆底：用 openai::Client + completions_api + 用户自定义 base_url
///
/// `extra_headers` 合并进 client builder；`base_url` 覆盖 rig 默认。
/// `supports_developer_role=false` 对 OpenAI 兼容端点调 `with_system_instructions_as_messages()`
/// （rig 0.41 提供，把 system 指令作为 user message 发送而非 top-level instructions）。
fn build_runtime(config: &ProviderConfig) -> AgentResult<Arc<dyn ProviderRuntime>> {
    use http::HeaderMap;
    use crate::provider::ProviderKind as K;

    // 构造自定义 headers（如有）
    let make_headers = |extra: &Option<std::collections::HashMap<String, String>>| {
        if let Some(map) = extra {
            let mut h = HeaderMap::new();
            for (k, v) in map {
                if let (Ok(name), Ok(val)) = (
                    http::HeaderName::from_bytes(k.as_bytes()),
                    http::HeaderValue::from_str(v),
                ) {
                    h.insert(name, val);
                }
            }
            Some(h)
        } else {
            None
        }
    };
    let headers = make_headers(&config.extra_headers);
    let api_key = &config.api_key;
    let base_url = config.base_url.as_deref();
    let kind = config.kind;

    macro_rules! apply_common {
        ($b:expr) => {{
            let mut b = $b;
            if let Some(url) = base_url {
                b = b.base_url(url);
            }
            if let Some(h) = &headers {
                b = b.http_headers(h.clone());
            }
            b
        }};
    }

    // OpenAI 协议族的 client 构造（统一返回 openai::Client，再按 kind 切 completions/responses）
    let build_openai = |ext_kind: K| -> AgentResult<openai::Client> {
        let builder = apply_common!(openai::Client::builder().api_key(api_key.clone()));
        let client = builder
            .build()
            .map_err(|e| AgentError::Provider(format!("{} client build: {e}", ext_kind.as_str())))?;
        // OpenAI 兼容端点不支持 developer role 时，降级为 system messages
        if !config.effective_supports_developer_role() {
            return Ok(client.with_system_instructions_as_messages());
        }
        Ok(client)
    };

    let runtime: Arc<dyn ProviderRuntime> = match kind {
        K::OpenAi => {
            // 官方端点走 Responses API（rig 默认）
            let c = build_openai(K::OpenAi)?;
            Arc::new(OpenAiClient(c))
        }
        K::DeepSeek => {
            let c = apply_common!(deepseek::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("deepseek client build: {e}")))?;
            Arc::new(DeepSeekClient(c))
        }
        K::Xai => {
            let c = apply_common!(xai::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("xai client build: {e}")))?;
            Arc::new(XaiClient(c))
        }
        K::Groq => {
            let c = apply_common!(groq::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("groq client build: {e}")))?;
            Arc::new(GroqClient(c))
        }
        K::OpenRouter => {
            let c = apply_common!(openrouter::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("openrouter client build: {e}")))?;
            Arc::new(OpenRouterClient(c))
        }
        K::Ollama => {
            let c = apply_common!(ollama::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("ollama client build: {e}")))?;
            Arc::new(OllamaClient(c))
        }
        K::Moonshot => {
            let c = apply_common!(moonshot::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("moonshot client build: {e}")))?;
            Arc::new(MoonshotClient(c))
        }
        K::Zhipu => {
            // 智谱 GLM：rig zai provider，默认 base https://api.z.ai/api/paas/v4
            // 用户可改 base_url 指向国内 bigmodel.cn 兼容端点
            let c = apply_common!(zai::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("zhipu/zai client build: {e}")))?;
            Arc::new(ZaiClient(c))
        }
        K::Mistral => {
            let c = apply_common!(mistral::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("mistral client build: {e}")))?;
            Arc::new(MistralClient(c))
        }
        K::Cohere => {
            let c = apply_common!(cohere::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("cohere client build: {e}")))?;
            Arc::new(CohereClient(c))
        }
        K::Perplexity => {
            let c = apply_common!(perplexity::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("perplexity client build: {e}")))?;
            Arc::new(PerplexityClient(c))
        }
        K::Anthropic => {
            let c = apply_common!(anthropic::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("anthropic client build: {e}")))?;
            Arc::new(AnthropicClient(c))
        }
        K::Gemini => {
            let c = apply_common!(gemini::Client::builder().api_key(api_key.clone()))
                .build()
                .map_err(|e| AgentError::Provider(format!("gemini client build: {e}")))?;
            Arc::new(GeminiClient(c))
        }
        K::OpenAiCompatible => {
            // 兆底：OpenAI 兼容端点，走 Completions API
            let c = build_openai(K::OpenAiCompatible)?.completions_api();
            Arc::new(OpenAiCompletionsClient(c))
        }
    };
    Ok(runtime)
}

/// 清洗 LLM 返回的标题：去引号 / 去首尾空白 / 去结尾标点 / 去多余换行。
///
/// 模型偶尔会带 “” 或结尾的 。.!！？? 等，统一清理保证侧栏显示干净。
/// 空结果返回空字符串，由调用方决定兜底（返回 “新会话” 或截断原文）。
fn clean_title(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    // 取首行（模型偶尔会输出多行解释）
    let first_line = t.lines().next().unwrap_or("").trim();
    let mut s = first_line.to_string();
    // 去掉成对包裹引号（中英文）
    let is_quote = |c: char| {
        matches!(c, '"' | '\'' | '\u{2018}' | '\u{2019}' | '\u{201c}' | '\u{201d}' | '`')
    };
    let mut chars = s.chars();
    if let (Some(first), Some(last)) = (chars.next(), s.chars().last()) {
        if first != last && is_quote(first) && is_quote(last) {
            // 首尾是不同引号（如 “…” 或 ‘…’）
            s = s.chars().skip(1).take(s.chars().count().saturating_sub(2)).collect();
        } else if first == last && is_quote(first) {
            // 首尾是同一引号（如 "…" 或 '…'）
            s = s.chars().skip(1).take(s.chars().count().saturating_sub(2)).collect();
        }
    }
    // 去掉结尾标点（中英文句号、问号、感叹号、冒号）
    while let Some(c) = s.chars().last() {
        if matches!(c, '。' | '．' | '.' | '？' | '?' | '！' | '!' | '：' | ':') {
            s.pop();
        } else {
            break;
        }
    }
    s.trim().to_string()
}

// ── 流转换：MultiTurnStreamItem → StreamChunk ──────────────────

/// 把 Rig 的 `MultiTurnStreamItem` 流映射为统一的 `StreamChunk` 流。
///
/// 一期只关心：StreamAssistantItem(Text/Reasoning delta) / FinalResponse(→Done)。
/// ToolCall/ToolExecutionStart 属二期 ToolCallCard，当前忽略。
fn map_multi_turn_stream<S, R>(stream: S) -> impl Stream<Item = StreamChunk> + Send
where
    S: Stream<Item = Result<MultiTurnStreamItem<R>, StreamingError>> + Send + 'static,
    R: Clone + Unpin + Send + 'static,
{
    async_stream::stream! {
        let mut s = Box::pin(stream);
        let mut got_final = false;
        let mut errored = false;
        // 记录 internal_call_id → (name, arguments)，供 ToolResult 关联
        let mut pending: std::collections::HashMap<String, ToolCallInfo> = std::collections::HashMap::new();
        while let Some(item) = s.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(content)) => {
                    match content {
                        StreamedAssistantContent::Text(t) => {
                            yield StreamChunk::text_delta(t.text);
                        }
                        StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
                            yield StreamChunk::reasoning_delta(reasoning);
                        }
                        StreamedAssistantContent::Reasoning(r) => {
                            for c in &r.content {
                                if let Some(t) = reasoning_text(c) {
                                    yield StreamChunk::reasoning_delta(t);
                                }
                            }
                        }
                        StreamedAssistantContent::ToolCall { tool_call, internal_call_id } => {
                            // 模型决定调用工具（即将执行）。记录以便 Result 关联。
                            let info = ToolCallInfo {
                                name: tool_call.function.name.clone(),
                                arguments: tool_call.function.arguments.to_string(),
                                call_id: internal_call_id.clone(),
                                result: None,
                                is_error: false,
                            };
                            pending.insert(internal_call_id.clone(), info.clone());
                            yield StreamChunk::tool_call_start(info);
                        }
                        // ToolCallDelta（参数增量）/ Final / Unknown：不向外发
                        _ => {}
                    }
                }
                Ok(MultiTurnStreamItem::ToolExecutionCommitted { tool_call, internal_call_id, .. }) => {
                    // Rig 已执行并提交工具调用（0.41 重命名 ToolExecutionStart → ToolExecutionCommitted）。
                    // 对前端而言，Start 信号已在 ToolCall 时发出，这里不重复；
                    // 但若 ToolCall 未收到（某些 provider 路径），补发。
                    if !pending.contains_key(&internal_call_id) {
                        let info = ToolCallInfo {
                            name: tool_call.function.name.clone(),
                            arguments: tool_call.function.arguments.to_string(),
                            call_id: internal_call_id.clone(),
                            result: None,
                            is_error: false,
                        };
                        pending.insert(internal_call_id.clone(), info.clone());
                        yield StreamChunk::tool_call_start(info);
                    }
                }
                Ok(MultiTurnStreamItem::StreamUserItem(user_content)) => {
                    // 工具执行结果（StreamedUserContent 当前仅 ToolResult 变体）
                    use rig::streaming::StreamedUserContent;
                    let StreamedUserContent::ToolResult { tool_result, internal_call_id } = user_content;
                    let result_text = tool_result_content_to_text(&tool_result.content);
                    if let Some(mut info) = pending.remove(&internal_call_id) {
                        info.result = Some(result_text.clone());
                        yield StreamChunk::tool_call_result(result_text, info);
                    } else {
                        // 未匹配到 Start（异常），仍发出结果
                        let info = ToolCallInfo {
                            name: String::new(),
                            arguments: String::new(),
                            call_id: internal_call_id.clone(),
                            result: Some(result_text.clone()),
                            is_error: false,
                        };
                        yield StreamChunk::tool_call_result(result_text, info);
                    }
                }
                Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                    // 二期 B1：provider 报告的真实 token usage。
                    // rig 的 Usage 以 0 为“未报告”哨兵（has_values() 判定），
                    // 仅在 provider 真的报了用量时才向外发，避免落库伪 0 值。
                    if call.usage.has_values() {
                        yield StreamChunk::usage(TokenUsage {
                            input_tokens: call.usage.input_tokens,
                            output_tokens: call.usage.output_tokens,
                            total_tokens: call.usage.total_tokens,
                        });
                    }
                }
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    got_final = true;
                    yield StreamChunk::done();
                }
                Ok(_) => {
                    // 其他未知变体（未来 rig 新增），安全忽略
                }
                Err(e) => {
                    errored = true;
                    yield StreamChunk::error(e.to_string());
                    break;
                }
            }
        }
        if !errored && !got_final {
            // 某些 provider 不发 FinalResponse，补一个 Done 保证前端状态机收尾
            yield StreamChunk::done();
        }
    }
}

/// 把 `ToolResult.content`（OneOrMany<ToolResultContent>）拼成模型可见文本。
fn tool_result_content_to_text(
    content: &rig::OneOrMany<rig::completion::message::ToolResultContent>,
) -> String {
    use rig::completion::message::ToolResultContent;
    let mut buf = String::new();
    for c in content.iter() {
        match c {
            ToolResultContent::Text(t) => {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&t.text);
            }
            ToolResultContent::Image(_) => {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str("[image]");
            }
            ToolResultContent::Json { value } => {
                // 0.41 新增变体：结构化 JSON 结果。序列化为字符串拼入。
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&value.to_string());
            }
        }
    }
    buf
}

/// 从 `ReasoningContent` 提取文本（Text/Summary 变体）。
fn reasoning_text(content: &rig::completion::message::ReasoningContent) -> Option<String> {
    use rig::completion::message::ReasoningContent;
    match content {
        ReasoningContent::Text { text, .. } => Some(text.clone()),
        ReasoningContent::Summary(s) => Some(s.clone()),
        _ => None,
    }
}

// ── 便利：memory MessageRow → rig Message ──────────────────────

/// 纯文本历史 → Rig Message 列表（跳过 system 与未完成消息）。
///
/// 一期：纯文本。多模态图片输入由调用方单独构造 `Message::User` with `UserContent::Image`。
///
/// **例外**：`model == "__summary__"` 的 system 消息是 B2 历史压缩摘要，
/// 保留并作为 history 开头注入（让模型知晓早期对话要点）。
pub fn text_history_to_messages(rows: &[memory::MessageRow]) -> Vec<Message> {
    use memory::{MessageRole, MessageStatus};
    const SUMMARY_MODEL_TAG: &str = "__summary__";
    rows.iter()
        .filter(|m| {
            m.status == MessageStatus::Complete && !m.content.is_empty() && match m.role {
                MessageRole::User | MessageRole::Assistant => true,
                // 仅保留压缩摘要 system 消息
                MessageRole::System => m.model.as_deref() == Some(SUMMARY_MODEL_TAG),
            }
        })
        .map(|m| match m.role {
            MessageRole::User => Message::User {
                content: rig::OneOrMany::one(UserContent::Text(m.content.clone().into())),
            },
            MessageRole::Assistant => Message::Assistant {
                id: None,
                content: rig::OneOrMany::one(AssistantContent::Text(m.content.clone().into())),
            },
            MessageRole::System => Message::System {
                content: m.content.clone(),
            },
        })
        .collect()
}

/** 图片上下文块（一期：base64 + MIME，走 VLM）。 */
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextImage {
    /// MIME，如 "image/png"。
    pub mime: String,
    /// base64 编码的原始字节。
    pub data_b64: String,
}

/// 构造一条纯文本 user prompt（避免上层直接依赖 rig）。
pub fn text_prompt(content: impl Into<String>) -> Message {
    Message::User {
        content: rig::OneOrMany::one(UserContent::Text(content.into().into())),
    }
}

/// 构造一条多模态 user prompt（文本 + 图片）。
///
/// Rig 的 `Message::User.content` 是 `OneOrMany<UserContent>`，至少含一项。
/// 本函数按 [文本, 图片...] 顺序组装；无图片时退化为纯文本 prompt。
pub fn multimodal_prompt(
    content: impl Into<String>,
    images: &[ContextImage],
) -> Message {
    use rig::completion::message::{ImageDetail, ImageMediaType, MimeType};
    let content = content.into();
    if images.is_empty() {
        return text_prompt(content);
    }
    // 首项：文本（用户问题）
    let first = UserContent::text(content);
    let mut parts: Vec<UserContent> = Vec::with_capacity(images.len());
    for img in images {
        let media_type = ImageMediaType::from_mime_type(&img.mime);
        parts.push(UserContent::image_base64(
            img.data_b64.clone(),
            media_type,
            None::<ImageDetail>,
        ));
    }
    // OneOrMany::one + 多次 push 得到 [文本, 图1, 图2, ...]
    let mut one = rig::OneOrMany::one(first);
    for p in parts {
        one.push(p);
    }
    Message::User { content: one }
}

/// 取历史消息列表，拆分为 (不含最后一条的历史, 最后一条作为 prompt)。
///
/// 调用方通常刚写入 user 消息，需要把它从 history 取出作为本轮 prompt。
/// 返回的 prompt 保证是 user 文本消息；若最后一条非 user，则返回 None。
pub fn split_last_as_prompt(rows: Vec<memory::MessageRow>) -> Option<(Vec<Message>, Message)> {
    use memory::{MessageRole, MessageStatus};
    let mut rows = rows;
    let last = rows.pop()?;
    if !matches!(last.role, MessageRole::User) || last.status != MessageStatus::Complete {
        return None;
    }
    let prompt = text_prompt(last.content);
    let history = text_history_to_messages(&rows);
    Some((history, prompt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::completion::message::{Message, UserContent};

    fn extract_text(msg: &Message) -> String {
        match msg {
            Message::User { content } => {
                for p in content.iter() {
                    if let UserContent::Text(t) = p {
                        return t.text.clone();
                    }
                }
                String::new()
            }
            _ => String::new(),
        }
    }

    #[test]
    fn text_prompt_basic() {
        let m = text_prompt("hello");
        assert_eq!(extract_text(&m), "hello");
    }

    #[test]
    fn multimodal_prompt_no_images_degrades() {
        let m = multimodal_prompt("hi", &[]);
        assert_eq!(extract_text(&m), "hi");
    }

    #[test]
    fn multimodal_prompt_has_image_parts() {
        let imgs = vec![ContextImage {
            mime: "image/png".into(),
            data_b64: "iVBORw".into(),
        }];
        let m = multimodal_prompt("看图", &imgs);
        match m {
            Message::User { content } => {
                let mut has_text = false;
                let mut has_image = false;
                for p in content.iter() {
                    match p {
                        UserContent::Text(t) => {
                            assert_eq!(t.text, "看图");
                            has_text = true;
                        }
                        UserContent::Image(_) => has_image = true,
                        _ => {}
                    }
                }
                assert!(has_text, "应含文本 part");
                assert!(has_image, "应含图片 part");
            }
            _ => panic!("expected User message"),
        }
    }

    // ── clean_title 单元测试 ──

    #[test]
    fn clean_title_strips_wrapping_quotes() {
        assert_eq!(clean_title("\"关于 Rust 异步\""), "关于 Rust 异步");
        assert_eq!(clean_title("'hello world'"), "hello world");
        // 中文引号 \u{201c}…\u{201d}
        assert_eq!(clean_title("\u{201c}关于数据库设计\u{201d}"), "关于数据库设计");
    }

    #[test]
    fn clean_title_strips_trailing_punctuation() {
        assert_eq!(clean_title("数据库设计。"), "数据库设计");
        assert_eq!(clean_title("How to use async?"), "How to use async");
        assert_eq!(clean_title("Debug this!"), "Debug this");
        assert_eq!(clean_title("多轮工具调用："), "多轮工具调用");
    }

    #[test]
    fn clean_title_takes_first_line() {
        // 模型偶尔输出多行（标题+解释），只取首行
        assert_eq!(clean_title("Rust 异步\n这是解释"), "Rust 异步");
    }

    #[test]
    fn clean_title_empty_input() {
        assert_eq!(clean_title(""), "");
        assert_eq!(clean_title("   \n  "), "");
    }

    #[test]
    fn clean_title_preserves_normal_title() {
        assert_eq!(clean_title("Rust 异步编程"), "Rust 异步编程");
        assert_eq!(clean_title("How to center a div"), "How to center a div");
    }
}
