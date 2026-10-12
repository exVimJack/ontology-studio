//! 流式对话服务。
//!
//! 封装 Rig 0.44 的 agent + streaming，对外暴露统一的 `StreamChunk` 流。
//! 对齐 ARCHITECTURE.md §13.2 的 StreamChunk 契约（一期子集：TextDelta/ReasoningDelta/Done/Error）。
//!
//! 设计说明（provider 类型分歧）：
//! Rig 0.44 各 provider 的 `Model<Wire>` 具体类型不同，无法直接 trait-object 化。
//! 采用 **`ModelFactory` trait** 方案：定义一个 trait 把「按名构造已擦除模型
//! （`DynModel<Completion>`）」封装为唯一方法，每个 provider kind 在 `with_tools`
//! 时构造具体 client 并立即包成 `Box<dyn ModelFactory>`。`ChatService` 持 trait
//! object，所有调用点零 match 分支；agent 构建/流式/采样等全走同一 helper，
//! 消除 0.41 的泛型宏样板。新增 provider 只需：① build_runtime 加构造分支
//! ② impl ModelFactory 一块。

use std::sync::Arc;

use futures_util::{Stream, StreamExt};
use specta_typescript::Number;
use rig::agent::tool::server::ToolServerHandle;
use rig::agent::MultiTurnStreamItem;
use rig::completion::message::{AssistantContent, Message, UserContent};
use rig::providers::{anthropic, cohere, gemini, ollama, openai};
use rig::streaming::StreamEvent;
use serde::{Deserialize, Serialize};

use crate::error::{AgentError, AgentResult};
use crate::provider::{
    reasoning_to_params, ProviderConfig, ReasoningLevel,
};

/// Agent 多轮工具调用的最大模型调用次数（max_turns = total model-call budget）。
///
/// Rig 在未配 `default_max_turns` 时隐式预算 = 1 次模型调用（仅发 prompt，
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

/// 模型工厂 trait：按模型名构造已擦除的 rig 模型（`DynModel<Completion>`）。
///
/// 0.44 起 AgentBuilder::new 直收模型（client.agent() 已删），client→model 由
/// 各 provider 自己路由（Responses/Chat/Messages/GenerateContent 各异）。
/// 擦除后 agent 构建/流式/prompt 全走同一 helper，消除 0.41 的泛型宏样板。
trait ModelFactory: Send + Sync {
    fn build_model(&self, name: &str) -> rig::DynModel<rig::operation::Completion>;
}

/// 保留 0.41 命名：运行时对象 = 模型工厂（方法已合并）。
trait ProviderRuntime: ModelFactory {}

impl<T: ModelFactory> ProviderRuntime for T {}

/// 已擦除类型的流式输出（`StreamChunk` 流）。
type ChatStream = std::pin::Pin<Box<dyn Stream<Item = StreamChunk> + Send>>;

/// 构造带 preamble 的 AgentOpts（run_llm_prompt_streamed 用）。
fn opts_from(preamble: &str, config: &ProviderConfig) -> AgentOpts {
    let mut opts = make_opts(None, config);
    opts.preamble = Some(preamble.to_string());
    opts
}


/// helper：为已擦除模型构造 agent 并流式对话。
///
/// 调用方传入 `DynModel<Completion>`（由 ModelFactory 按名构造）+ 配置，
/// 函数内部构造 `AgentBuilder`、设采样参数/preamble/reasoning/memory/
/// conversation/tool handle，build 后调 stream()/await，返回已擦除类型的流/字符串。
mod helper {
    use super::*;

    /// 构造 agent builder（设 preamble/memory/conversation/采样/reasoning）。
    /// 返回的 builder 尚未 build（调用方决定是否挂 tool_server_handle）。
    fn build_agent(
        model: rig::DynModel<rig::operation::Completion>,
        opts: &AgentOpts,
        memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
        conversation_id: Option<String>,
    ) -> rig::agent::AgentBuilder {
        let mut b = rig::agent::AgentBuilder::new(model);
        if let Some(p) = &opts.preamble {
            if !p.is_empty() {
                b = b.preamble(p);
            }
        }
        if let Some(m) = memory {
            b = b.memory(m);
        }
        if let Some(id) = conversation_id {
            // 0.44：conversation 从每次 prompt 调用上移到 builder
            b = b.conversation(id);
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

    /// build（可选挂共享 tool server）。
    fn finish(b: rig::agent::AgentBuilder, handle: Option<ToolServerHandle>) -> rig::agent::Agent {
        if let Some(h) = handle {
            b.tool_server_handle(h).build()
        } else {
            b.build()
        }
    }

    /// 流式对话（带 history，无 memory）。
    pub(crate) async fn stream_chat(
        model: rig::DynModel<rig::operation::Completion>,
        prompt: Message,
        history: Vec<Message>,
        handle: Option<ToolServerHandle>,
        opts: &AgentOpts,
    ) -> AgentResult<ChatStream> {
        let agent = finish(build_agent(model, opts, None, None), handle);
        let inner = agent.prompt(prompt).history(history).max_turns(MAX_TURNS).stream();
        Ok(Box::pin(map_multi_turn_stream(inner)))
    }

    /// 流式对话（memory 模式）。
    pub(crate) async fn stream_with_memory(
        model: rig::DynModel<rig::operation::Completion>,
        prompt: Message,
        conversation_id: String,
        memory: Option<Arc<dyn rig::memory::ConversationMemory>>,
        handle: Option<ToolServerHandle>,
        opts: &AgentOpts,
    ) -> AgentResult<ChatStream> {
        let agent = finish(
            build_agent(model, opts, memory, Some(conversation_id)),
            handle,
        );
        // memory 模式不传 history：CompactingMemory 在 load 时裁剪 + 压缩；
        // 显式 history 会绕过 memory（rig 语义），二者互斥。
        let inner = agent.prompt(prompt).max_turns(MAX_TURNS).stream();
        Ok(Box::pin(map_multi_turn_stream(inner)))
    }

    /// 单次非流式 prompt。
    pub(crate) async fn prompt_text(
        model: rig::DynModel<rig::operation::Completion>,
        prompt: String,
        opts: &AgentOpts,
    ) -> AgentResult<String> {
        let agent = finish(build_agent(model, opts, None, None), None);
        let resp = agent
            .prompt(prompt)
            .await
            .map_err(|e| AgentError::Provider(format!("prompt: {e}")));
        resp.map(|r| r.output())
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

// ── 各 provider 的 ModelFactory 实现 ─────────────────────────────

/// OpenAI 形协议 runtime（官方 + 一切兼容端点）。
///
/// `responses=true` 走官方 Responses API；其余兼容端点走 Chat Completions。
/// dialect 决定 base_url/鉴权/capability quirks（DeepSeek 单 chunk tool call 等）。
struct OpenAiShaped {
    client: openai::OpenAI,
    responses: bool,
}

impl ModelFactory for OpenAiShaped {
    fn build_model(&self, name: &str) -> rig::DynModel<rig::operation::Completion> {
        if self.responses {
            self.client.responses(name).erase()
        } else {
            self.client.chat(name).erase()
        }
    }
}

/// Anthropic（Messages API 原生协议）。
struct AnthropicClient(anthropic::Anthropic);
impl ModelFactory for AnthropicClient {
    fn build_model(&self, name: &str) -> rig::DynModel<rig::operation::Completion> {
        self.0.completion(name).erase()
    }
}

/// Gemini（Generative AI 原生协议）。
struct GeminiClient(gemini::Gemini);
impl ModelFactory for GeminiClient {
    fn build_model(&self, name: &str) -> rig::DynModel<rig::operation::Completion> {
        self.0.completion(name).erase()
    }
}

/// Cohere（原生协议）。
struct CohereClient(cohere::Cohere);
impl ModelFactory for CohereClient {
    fn build_model(&self, name: &str) -> rig::DynModel<rig::operation::Completion> {
        self.0.completion(name).erase()
    }
}

/// Ollama（本地；OpenAI 兼容 chat 路由）。
struct OllamaClient(ollama::Ollama);
impl ModelFactory for OllamaClient {
    fn build_model(&self, name: &str) -> rig::DynModel<rig::operation::Completion> {
        self.0.completion(name).erase()
    }
}

/// 对话服务。持有一个已配置好的 provider runtime（trait object，一期单 provider）
/// + 可选工具句柄 + 可选 memory/联邦/skill。
#[derive(Clone)]
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
    /// 当前压缩策略使用的上下文窗口（token）。set_memory 时记录，
    /// 供 UI 展示用量百分比（「28k/128k tok」）。0 = 未注入 memory（未知）。
    context_window: usize,
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
            context_window: 0,
        })
    }

    /// 更新工具句柄（运行时动态增减 MCP server 后调用）。
    pub fn set_tool_handle(&mut self, handle: Option<ToolServerHandle>) {
        self.tool_handle = handle;
    }

    /// 单次非流式 LLM 调用（B/A 评审共用管道，即用即弃）。
    ///
    /// 对应 VALIDATION-DESIGN.md「编排驱动 + 多 LLM 路由」：对抗流程的每一步
    /// 是独立上下文的一次调用——不同 role（Generator/Verifier）只换 model + prompt，
    /// 无常驻 agent、无对话历史（保证 B 看不到 A 的思考过程）。
    ///
    /// `model`: provider 内的模型名（如 "glm-5.3-flash" / "deepseek-r1"）。
    /// `preamble`: 角色系统提示词（B 用独立审计视角，不含建模方法论）。
    /// `prompt`: 用户侧输入（评审任务 / 修订任务）。
    pub async fn run_llm_prompt(
        &self,
        model: &str,
        preamble: &str,
        prompt: &str,
    ) -> AgentResult<String> {
        let mut opts = make_opts(None, &self.config);
        opts.preamble = Some(preamble.to_string());
        // 校验对抗的非流式长请求（整本 Turtle 进出）曾长期挂死——
        // 改为流式接收 + 本地聚合：连接上持续有数据流动，网关不会掐；
        // read_timeout 只要求「每 2 分钟内要有新数据」，总上限 30min。
        self.run_llm_prompt_streamed(model, preamble, prompt).await
    }

    /// 流式单轮 prompt：与 run_llm_prompt 同参，但底层走 SSE 流式，
    /// 逐 chunk 聚合文本后一次性返回。 reasoning 增量丢弃。
    pub async fn run_llm_prompt_streamed(
        &self,
        model: &str,
        preamble: &str,
        prompt: &str,
    ) -> AgentResult<String> {
        let stream = helper::stream_chat(
            self.runtime.build_model(model),
            text_prompt(prompt),
            Vec::new(),
            None,
            &opts_from(preamble, &self.config),
        )
        .await?;
        let mut s = stream;
        let mut out = String::new();
        while let Some(chunk) = s.next().await {
            match chunk.kind {
                StreamKind::TextDelta => out.push_str(&chunk.text),
                StreamKind::Error => {
                    return Err(AgentError::Provider(if chunk.text.is_empty() {
                        "provider stream error".to_string()
                    } else {
                        chunk.text
                    }))
                }
                _ => {}
            }
        }
        Ok(out)
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
        self.context_window = context_window;
    }

    /// 当前压缩策略使用的上下文窗口（token）。0 = 未注入 memory（未知）。
    pub fn context_window(&self) -> usize {
        self.context_window
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
        helper::stream_chat(
            self.runtime.build_model(&self.config.model),
            prompt,
            history,
            handle,
            &opts,
        )
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
                rig::agent::tool::server::ToolServer::new().run()
            });
            for tool in all_tools {
                // 0.44：add_dynamic_tool 同步（不再 async）
                h.add_dynamic_tool(tool);
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
        helper::stream_with_memory(
            self.runtime.build_model(&self.config.model),
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
                helper::prompt_text(runtime.build_model(&model), text, &opts)
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
        helper::prompt_text(self.runtime.build_model(&self.config.model), prompt, &opts).await
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
        let raw = helper::prompt_text(
            self.runtime.build_model(&self.config.model),
            prompt,
            &opts,
        )
        .await
        .map_err(|e| AgentError::Provider(format!("generate_title: {e}")))?;
        Ok(clean_title(&raw))
    }
}


/// 统一 HTTP 后端（带超时 + 可选默认 headers）→ rig 传输对象。
///
/// 整本体修订/八维度评审等非流式长请求单次可跑数分钟，默认 reqwest::Client
/// 无超时——被网关/代理掐掉时表现为 `error sending request`。显式配：
/// 连接 30s、读间隔 120s（流式下每个 chunk 重置）、总时长 30min。
/// `extra_headers` 作为 reqwest 默认头合并（OpenRouter X-Title、anthropic-beta 等）。
fn provider_transport(
    extra_headers: Option<http::HeaderMap>,
) -> AgentResult<rig::http_client::ReqwestClient> {
    let mut b = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(120))
        .timeout(std::time::Duration::from_secs(1800));
    if let Some(h) = extra_headers {
        b = b.default_headers(h);
    }
    let client = b
        .build()
        .map_err(|e| AgentError::Provider(format!("http transport build: {e}")))?;
    Ok(rig::http_client::ReqwestClient::from(client))
}

fn extra_header_map(
    extra: &Option<std::collections::HashMap<String, String>>,
) -> Option<http::HeaderMap> {
    use http::HeaderMap;
    let map = extra.as_ref()?;
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
}

/// 按配置构造 provider runtime（trait object）。
///
/// 内部按 `kind` 路由到 rig 0.44 原生 provider client：
///   - OpenAI 官方走 Responses API（`.responses()`），其余 OpenAI 兼容走 Chat
///     Completions（`.chat()`）；OpenAI 形 vendor 全部经 `OpenAIConfig::with_key(dialect)`
///     构造（0.44 的 vendor = openai::OpenAI + Dialect，capability quirks 由 dialect 声明）
///   - Anthropic 走 Messages API；Gemini 走 Generative AI；Cohere 原生；Ollama 本地
///   - OpenAiCompatible 兆底：OPENAI dialect + 用户自定义 base_url + Chat 路由
///
/// `extra_headers` 并入 reqwest 默认头；`base_url` 覆盖 dialect 默认。
/// `supports_developer_role=false` 时对 OpenAIConfig 调
/// `with_system_instructions_as_messages()`（0.44 语义同 0.41：system 指令作为
/// messages 发送而非 top-level instructions）。
fn build_runtime(config: &ProviderConfig) -> AgentResult<Arc<dyn ProviderRuntime>> {
    use crate::provider::ProviderKind as K;
    use rig::providers::openai::wire as oai_wire;

    let http = provider_transport(extra_header_map(&config.extra_headers))?;
    let api_key = config.api_key.clone();
    let base_url = config.base_url.clone();

    // OpenAI 形协议：dialect + base_url 覆盖 + developer-role 降级。
    let openai_shaped = |dialect: &'static oai_wire::Dialect,
                         responses: bool|
     -> AgentResult<OpenAiShaped> {
        let mut cfg = oai_wire::OpenAIConfig::with_key(dialect, api_key.clone());
        if let Some(url) = &base_url {
            cfg = cfg.with_base_url(url);
        }
        if !config.effective_supports_developer_role() {
            cfg = cfg.with_system_instructions_as_messages();
        }
        Ok(OpenAiShaped {
            client: cfg.connect(http.clone()),
            responses,
        })
    };

    let runtime: Arc<dyn ProviderRuntime> = match config.kind {
        // 官方端点走 Responses API（rig 默认路由）
        K::OpenAi => Arc::new(openai_shaped(&oai_wire::OPENAI, true)?),
        K::DeepSeek => Arc::new(openai_shaped(&oai_wire::DEEPSEEK, false)?),
        K::Xai => Arc::new(openai_shaped(&rig::providers::xai::DIALECT, false)?),
        K::Groq => Arc::new(openai_shaped(&oai_wire::GROQ, false)?),
        K::OpenRouter => Arc::new(openai_shaped(&oai_wire::OPENROUTER, false)?),
        K::Moonshot => Arc::new(openai_shaped(&oai_wire::MOONSHOT, false)?),
        // 智谱 GLM：rig ZAI dialect，默认 base https://api.z.ai/api/paas/v4，
        // 用户可改 base_url 指向国内 bigmodel.cn 兼容端点
        K::Zhipu => Arc::new(openai_shaped(&oai_wire::ZAI, false)?),
        K::Mistral => Arc::new(openai_shaped(&oai_wire::MISTRAL, false)?),
        K::Perplexity => Arc::new(openai_shaped(&oai_wire::PERPLEXITY, false)?),
        // 兆底：OpenAI 兼容端点，走 Chat Completions + 自定义 base_url
        K::OpenAiCompatible => Arc::new(openai_shaped(&oai_wire::OPENAI, false)?),
        K::Ollama => {
            // OllamaConfig { base_url, api_key }；默认本地 daemon，base_url 可覆盖
            let mut cfg = ollama::OllamaConfig::new();
            if let Some(url) = &base_url {
                cfg.base_url = url.clone();
            }
            cfg.api_key = api_key.into();
            Arc::new(OllamaClient(cfg.connect(http)))
        }
        K::Anthropic => {
            let mut cfg = anthropic::AnthropicConfig::new(api_key);
            if let Some(url) = &base_url {
                cfg = cfg.with_base_url(url);
            }
            Arc::new(AnthropicClient(cfg.connect(http)))
        }
        K::Gemini => {
            let mut cfg = gemini::GeminiConfig::new(api_key);
            if let Some(url) = &base_url {
                cfg = cfg.with_base_url(url);
            }
            Arc::new(GeminiClient(cfg.connect(http)))
        }
        K::Cohere => {
            let mut cfg = cohere::CohereConfig::new(api_key);
            if let Some(url) = &base_url {
                cfg = cfg.with_base_url(url);
            }
            Arc::new(CohereClient(cfg.connect(http)))
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

// ── 流转换：MultiTurnStreamItem → StreamChunk ──────────────────

/// 把 Rig 0.44 的 `MultiTurnStreamItem` 流映射为统一的 `StreamChunk` 流。
///
/// 关注：
///   - StreamAssistantItem(Item<StreamEvent>)：Text/Reasoning 增量（Text/Reasoning
///     事件）；工具调用的最终形态在 End{content: AssistantContent::ToolCall} 一次性
///     到达（Start 只带名字，Arguments 增量我们不外发）
///   - ToolCall{tool_call}：turn 提交时模型已完成的调用（与 End 去重后补发 Start）
///   - ToolExecutionCommitted / ToolResult：工具执行与结果（0.44 以 call id 关联，
///     ToolResult 自带 is_error，不再依赖 "[tool error]" 前缀还原）
///   - CompletionCall：provider 报告的真实 token usage
///   - FinalResponse：正常收尾
fn map_multi_turn_stream<S>(stream: S) -> impl Stream<Item = StreamChunk> + Send
where
    S: Stream<Item = Result<MultiTurnStreamItem, rig::completion::PromptError>> + Send + 'static,
{
    async_stream::stream! {
        use rig::completion::message::AssistantContent as AC;
        let mut s = Box::pin(stream);
        let mut got_final = false;
        let mut errored = false;
        // call_id → info，供 ToolResult 关联（Start 补发去重）
        let mut pending: std::collections::HashMap<String, ToolCallInfo> = std::collections::HashMap::new();
        while let Some(item) = s.next().await {
            match item {
                Ok(MultiTurnStreamItem::StreamAssistantItem(item)) => {
                    let rig::streaming::Item::Event(ev) = item else { continue };
                    match ev {
                        StreamEvent::Text { text, .. } => {
                            yield StreamChunk::text_delta(text);
                        }
                        StreamEvent::Reasoning { text, .. } => {
                            yield StreamChunk::reasoning_delta(text);
                        }
                        StreamEvent::End { content, .. } => {
                            // 文本/思考的完整内容已通过增量发出，这里只取工具调用的最终形态
                            if let AC::ToolCall(tool_call) = content {
                                let info = ToolCallInfo {
                                    name: tool_call.function.name.to_string(),
                                    arguments: serde_json::Value::Object(tool_call.function.arguments.clone()).to_string(),
                                    call_id: tool_call.id.to_string(),
                                    result: None,
                                    is_error: false,
                                };
                                if !pending.contains_key(&info.call_id) {
                                    pending.insert(info.call_id.clone(), info.clone());
                                    yield StreamChunk::tool_call_start(info);
                                }
                            }
                        }
                        // Start（part 开启，仅名字）/ Arguments（参数 JSON 增量）：不外发
                        _ => {}
                    }
                }
                Ok(MultiTurnStreamItem::ToolCall { tool_call }) => {
                    // turn 提交时模型完成的调用（End 已发过则去重）
                    let id = tool_call.id.to_string();
                    if !pending.contains_key(&id) {
                        let info = ToolCallInfo {
                            name: tool_call.function.name.to_string(),
                            arguments: serde_json::Value::Object(tool_call.function.arguments.clone()).to_string(),
                            call_id: id,
                            result: None,
                            is_error: false,
                        };
                        pending.insert(info.call_id.clone(), info.clone());
                        yield StreamChunk::tool_call_start(info);
                    }
                }
                Ok(MultiTurnStreamItem::ToolExecutionCommitted { tool_call, .. }) => {
                    // Rig 已执行并提交该调用。Start 信号已在 End/ToolCall 时发出，这里只补漏。
                    let id = tool_call.id.to_string();
                    if !pending.contains_key(&id) {
                        let info = ToolCallInfo {
                            name: tool_call.function.name.to_string(),
                            arguments: serde_json::Value::Object(tool_call.function.arguments.clone()).to_string(),
                            call_id: id,
                            result: None,
                            is_error: false,
                        };
                        pending.insert(info.call_id.clone(), info.clone());
                        yield StreamChunk::tool_call_start(info);
                    }
                }
                Ok(MultiTurnStreamItem::ToolResult { tool_result }) => {
                    // 工具执行结果（0.44 以 call id 关联，替代 0.41 的 StreamUserItem）
                    let result_text = tool_result_content_to_text(&tool_result.content);
                    // 0.44 ToolResult 自带 is_error，前端据此渲染红色错误卡片
                    let is_error = tool_result.is_error;
                    let id = tool_result.call.to_string();
                    if let Some(mut info) = pending.remove(&id) {
                        info.result = Some(result_text.clone());
                        info.is_error = is_error;
                        yield StreamChunk::tool_call_result(result_text, info);
                    } else {
                        // 未匹配到 Start（异常），仍发出结果
                        let info = ToolCallInfo {
                            name: tool_result.name.to_string(),
                            arguments: String::new(),
                            call_id: id,
                            result: Some(result_text.clone()),
                            is_error,
                        };
                        yield StreamChunk::tool_call_result(result_text, info);
                    }
                }
                Ok(MultiTurnStreamItem::CompletionCall(call)) => {
                    // 二期 B1：provider 报告的真实 token usage（0.44 为 Option，None=未报告）。
                    // 仅在 provider 真的报了用量时才向外发，避免落库伪 0 值。
                    if call.usage.input_tokens.is_some() || call.usage.output_tokens.is_some() {
                        yield StreamChunk::usage(TokenUsage {
                            input_tokens: call.usage.input_tokens.unwrap_or(0),
                            output_tokens: call.usage.output_tokens.unwrap_or(0),
                            total_tokens: call.usage.total_tokens.unwrap_or(
                                call.usage.input_tokens.unwrap_or(0) + call.usage.output_tokens.unwrap_or(0),
                            ),
                        });
                    }
                }
                Ok(MultiTurnStreamItem::FinalResponse(_)) => {
                    got_final = true;
                    yield StreamChunk::done();
                }
                Ok(_) => {
                    // 其他未知变体（ToolResult 已单列；ModelTurnRetried 等），安全忽略
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

/// 把 `ToolResult.content`（Vec<ToolResultContent>）拼成模型可见文本。
fn tool_result_content_to_text(content: &[rig::completion::message::ToolResultContent]) -> String {
    use rig::completion::message::ToolResultContent;
    let mut buf = String::new();
    for c in content {
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
                // 结构化 JSON 结果。序列化为字符串拼入。
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&value.to_string());
            }
        }
    }
    buf
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
        content: vec![UserContent::text(content)],
    }
}

/// 构造一条多模态 user prompt（文本 + 图片）。
///
/// Rig 0.44 的 `Message::User.content` 是 `Vec<UserContent>`。
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
    let mut parts: Vec<UserContent> = Vec::with_capacity(1 + images.len());
    // 首项：文本（用户问题）
    parts.push(UserContent::text(content));
    for img in images {
        let media_type = <ImageMediaType as MimeType>::from_mime_type(&img.mime);
        parts.push(UserContent::image_base64(
            img.data_b64.clone(),
            media_type,
            None::<ImageDetail>,
        ));
    }
    Message::User { content: parts }
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
