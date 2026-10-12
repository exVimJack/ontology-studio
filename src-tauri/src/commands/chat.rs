//! 对话流式命令（见 ARCHITECTURE.md §13.2 / §14.1 主链路）。

//!

//! 流程（memory 模式，二期 B1/B2 重构后）：

//!   1. RAG 检索注入 + 构造本轮 prompt

//!   2. ChatService::stream_with_memory → agent 挂 CompactingMemory

//!      自动 load 历史 + 超预算裁剪 + 滚动摘要（carry_over）

//!   3. 通过 Channel 推 StreamChunk 给前端（前端 patch 内存，不逐 delta 落库）

//!   4. turn 结束整条落库 user + assistant 消息（预生成 id）

//!

//! Channel 优先于 Event（§13.1）：点对点、无噪声、fast-path。

//!

use agent_core::{

    multimodal_prompt, text_prompt,

    ContextImage, StreamChunk, StreamKind, TokenUsage, ToolCallInfo,

};

use memory::{MessageRole, MessageStatus};

use serde::{Deserialize, Serialize};

use specta::Type;

use tauri::ipc::Channel;

use tauri::{AppHandle, State};

use tokio::sync::oneshot;

use tracing::{error, info};



use super::error::{AppError, AppResult};

use crate::state::AppState;



/// IPC 用图片上下文块。

#[derive(Debug, Clone, Serialize, Deserialize, Type)]

pub struct ContextImageInput {

    pub mime: String,

    pub data_b64: String,

}



/// 发送消息的输入（对齐 §13.2 SendMessageRequest 一期子集）。

#[derive(Debug, Clone, Serialize, Deserialize, Type)]

pub struct SendMessageInput {

    pub conversation_id: String,

    /// 纯文本内容（一期；多模态图片输入下一步加 image 字段）

    pub content: String,

    /// 本次携带的挂载文档路径列表（用户在消息中 `@token` 引用的文档）。

    /// token 形态：`@文件夹/文件名`（知识库路径，决策 17 修订）/ `@文件名`（旧消息兜底）

    /// / `@skillName`。后端不从这些文档读全文注入 prompt——而是查得每篇的 id+name+path，

    /// 在 user message 尾部追加 `<conversation-scope>` 注脚，模型按需调

    /// `read_document(id)` 工具取全文（agentic search，cache 友好）；同时对库内文件

    /// 幂等补挂载（复制指令到新会话重放时文件未挂载的场景）。

    #[serde(default)]

    pub mounted_paths: Vec<String>,

    /// 本次携带的图片上下文（选中的已摄入图片，走 VLM）。

    #[serde(default)]

    pub context_images: Vec<ContextImageInput>,

    /// 是否开启深度思考（reasoning）。true 时按 provider kind 透传

    /// reasoning_effort/thinking 参数。默认 false。

    #[serde(default)]

    pub enable_reasoning: bool,

}



/// 流式 chunk（对齐 §13.2 StreamChunk，由 agent-core 的 StreamChunk 映射）。

///

/// 额外带 `message_id` 让前端定位 assistant 消息做增量 patch（§14.1 性能要点）。

#[derive(Debug, Clone, Serialize, Type)]

pub struct ChatStreamChunk {

    /// assistant 消息 ID（首个 chunk 发出后前端据此 patch）

    pub message_id: String,

    pub kind: StreamKind,

    pub text: String,

    /// 工具调用详情（仅 ToolCallStart/Result 有值）

    #[serde(skip_serializing_if = "Option::is_none")]

    pub tool_call: Option<ToolCallInfo>,

    /// token usage（仅 Usage 有值；二期 B1）

    #[serde(skip_serializing_if = "Option::is_none")]

    pub usage: Option<agent_core::TokenUsage>,

}



#[tauri::command]

#[specta::specta]

pub async fn send_message(

    _app: AppHandle,

    state: State<'_, AppState>,

    input: SendMessageInput,

    on_chunk: Channel<ChatStreamChunk>,

) -> AppResult<String> {

    // 0. 校验 chat service 已配置

    let chat_guard = state.chat.read().await;

    let chat = chat_guard

        .as_ref()

        .ok_or_else(|| AppError::Provider("未配置模型提供商，请先在设置页添加".into()))?;

    let model_name = chat.config().model.clone();

    drop(chat_guard);



    let conv_id = &input.conversation_id;



    // 0.5 兜底体积校验：防止图片 base64 撑爆 provider 网关被 413。
    // 挂载文档不再注入全文（agentic search，模型按需调 read_document），
    // 只需校验图片体积。上限 28 MiB，留余量给历史消息与 JSON 包装。
    const MAX_ESTIMATED_BODY_BYTES: usize = 28 * 1024 * 1024;
    let mut estimated: usize = 0;
    for img in &input.context_images {
        estimated += img.data_b64.len();
    }
    if estimated > MAX_ESTIMATED_BODY_BYTES {
        let mb = (estimated as f64 / 1024.0 / 1024.0).floor() as u64;
        let limit = (MAX_ESTIMATED_BODY_BYTES / 1024 / 1024) as u64;
        return Err(AppError::Provider(format!(
            "请求体过大（约 {mb} MiB，上限 {limit} MiB）。请减少附件数量、拆分对话或压缩图片后重试。"
        )));
    }

    // 预生成 user_id + assistant_id：前端用它们做乐观更新/流式 patch，
    // turn 结束后用这些 id 落库（不预先建消息——避免 rig memory.load 读到本轮
    // user 消息导致与 prompt 重复）。
    let user_id = uuid::Uuid::new_v4().to_string();
    let assistant_id = uuid::Uuid::new_v4().to_string();

    // 1. 解析 mounted_paths：查得 id + name + path，并对库内文件幂等补挂载。

    //    - 幂等补挂载（mount_document INSERT OR IGNORE）：指令复制到新会话重放时

    //      文件未挂载，补上后进入激活集（resolve_active_doc_paths），

    //      search_documents / ScopeChip 均可见——修复「知识库中没有检索到这个附件」。

    //    - 模型看到 `@文件夹/文件名` / `@skillName` 文本（位置语义保留）+ 注脚里的 id+path，

    //      可按需调 read_document(id) 取全文。注脚在 user message（动态尾部），

    //      不破坏 system prompt prefix cache。

    //    - skill path（`skill://<name>`）与文件 path 统一走 read_document_by_path，

    //      注脚分两组呈现（[技能] / [挂载文档]），语义更清晰。

    let mut mounted_refs: Vec<(String, String, String)> = Vec::new(); // (id, name, path) 文件

    let mut skill_refs: Vec<(String, String)> = Vec::new(); // (id, name) skill

    for raw_path in &input.mounted_paths {

        let path = memory::canonicalize_path(raw_path);

        match state.memory.read_document_by_path(&path) {

            Ok(Some((id, name, format, _text, _char_count))) => {

                if format == "skill-md" {

                    skill_refs.push((id, name));

                } else {

                    // 幂等补挂载（已挂载则 no-op）；失败不阻断发送（注脚仍给 id+path）

                    if let Err(e) = state.memory.mount_document(conv_id, &path) {

                        tracing::warn!(path = %path, error = %e, "send_message: auto mount failed");

                    }

                    mounted_refs.push((id, name, path));

                }

            }
            Ok(None) => {
                tracing::warn!(path = %path, "send_message: mounted doc not found in documents table");
            }
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "send_message: read mounted doc failed");
            }
        }
    }

    // 1.5 构造本轮 user text：原文 + 会话知识范围注脚。
    //     注脚给最小范围元信息（激活的数据源名+表清单、激活的文件夹、挂载文档 id+name），
    //     不给全文/摘要（避免 batch 摘要延迟 + cache 失效 + context 膨胀）。
    //     模型凭此知道 `@xxx` token 指向什么，再按需调工具检索（agentic search，决策 17）。
    //     会话激活集为空时无注脚（模型按通用能力回答）。
    let active_folders = state.memory.get_active_folders(conv_id).unwrap_or_default();
    let active_sources = state.memory.get_active_sources(conv_id).unwrap_or_default();
    // 本体引用（决策：会话页面 @OntologyName）：存 ontology api_name 列表。
    // 注脚只给 api_name（不给 schema），模型用 describe_ontology 等只读工具按需钻取。
    let active_ontologies = state.memory.get_active_ontologies(conv_id).unwrap_or_default();
    // 激活数据源的表清单（agent 写 SQL 需要 catalog 名 + 表名；注脚给一次，避免首轮 list 工具往返）。
    let source_tables: Vec<(String, Vec<String>)> = if !active_sources.is_empty() {
        let fed_guard = state.federation.read().await;
        if let Some(fed) = fed_guard.as_ref() {
            let mut out = Vec::with_capacity(active_sources.len());
            for name in &active_sources {
                let tables = fed
                    .browse_schema(name)
                    .await
                    .map(|snap| snap.tables.into_iter().map(|t| t.name).collect())
                    .unwrap_or_default();
                out.push((name.clone(), tables));
            }
            out
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let has_scope =
        !mounted_refs.is_empty() || !skill_refs.is_empty() || !source_tables.is_empty()
            || !active_folders.is_empty() || !active_ontologies.is_empty();
    let user_text = if !has_scope {
        input.content.clone()
    } else {
        // charter 查询以闭包注入（Palantir / W3C 两套 store 同形，映射统一摘要结构），
        // 注脚渲染抽为纯函数 render_conversation_scope（无状态、可单测）。
        let palantir_charter = |ont: &str| {
            state
                .ontology_store
                .get_charter(ont)
                .ok()
                .map(|c| CharterSummary {
                    business_scenario: c.business_scenario.clone(),
                    business_essence: c.business_essence.clone(),
                })
        };
        let ttl_charter = |iri: &str| {
            state
                .ttl_store
                .get_charter(iri)
                .ok()
                .map(|c| CharterSummary {
                    business_scenario: c.business_scenario.clone(),
                    business_essence: c.business_essence.clone(),
                })
        };
        let mut buf = input.content.clone();
        buf.push_str(&render_conversation_scope(
            &source_tables,
            &active_folders,
            &skill_refs,
            &mounted_refs,
            &active_ontologies,
            &palantir_charter,
            &ttl_charter,
        ));
        buf
    };

    // 2. 构造本轮 prompt（多模态 / 纯文本；挂载文档不注入全文，仅 user_text 尾部注脚）
    let prompt = if !input.context_images.is_empty() {
        let imgs: Vec<ContextImage> = input
            .context_images
            .iter()
            .map(|i| ContextImage { mime: i.mime.clone(), data_b64: i.data_b64.clone() })
            .collect();
        multimodal_prompt(&user_text, &imgs)
    } else {
        text_prompt(&user_text)
    };


    info!(conv_id = %conv_id, assistant_id = %assistant_id, "stream chat started (memory mode)");



    // 3. 发起流式（memory 模式）：agent 挂的 CompactingMemory 自动 load 历史 +

    //    超预算时裁剪 + 滚动摘要。prompt 即本轮输入，history 由 rig 从 DB load。

    let chat_guard = state.chat.read().await;

    let chat = chat_guard

        .as_ref()

        .ok_or_else(|| AppError::Provider("provider 已失效".into()))?;

    let stream_result = chat.stream_with_memory(prompt, conv_id, input.enable_reasoning).await;

    drop(chat_guard);



    let mut stream = match stream_result {

        Ok(s) => s,

        Err(e) => {

            let msg = e.to_string();

            error!(assistant_id = %assistant_id, error = %msg, "stream init failed");

            // 流都没启动：落 user + error assistant 消息（保持历史可追溯）

            persist_turn(

                &state, conv_id, &user_id, &input.content, &assistant_id, "", None,

                Some(&model_name), MessageStatus::Error, Some(&msg), None,

            )?;

            return Err(e.into());

        }

    };



    // 4. 消费流，推 channel；累积 assistant content + reasoning（不逐 delta 落库，turn 结束整条写）

    use futures_util::StreamExt;

    // 注册取消信号：cancel_stream 命令触发时，recv 收到 → 流循环退出。
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    {
        let mut map = state.cancel_signals.lock().unwrap();
        map.insert(assistant_id.clone(), cancel_tx);
    }
    let mut cancel_rx = cancel_rx;

    let mut errored: Option<String> = None;

    let mut content_buf = String::new();

    let mut reasoning_buf = String::new();

    let mut last_usage: Option<TokenUsage> = None;

    let mut cancelled = false;

    loop {
        // select!：流 chunk 与取消信号竞速；取消到达 → 中断
        tokio::select! {
            _ = &mut cancel_rx => {
                cancelled = true;
                break;
            }
            chunk = stream.next() => {
                let chunk = match chunk {
                    Some(c) => c,
                    None => break, // 流结束
                };
                let StreamChunk { kind, text, tool_call, usage } = chunk;

        // 累积文本（TextDelta 入 content；ReasoningDelta 入 reasoning，独立于正文）

        match kind {

            StreamKind::TextDelta => content_buf.push_str(&text),

            StreamKind::ReasoningDelta => reasoning_buf.push_str(&text),

            StreamKind::Usage => {

                if let Some(u) = usage { last_usage = Some(u); }

            }

            _ => {}

        }

        // 推给前端

        let _ = on_chunk.send(ChatStreamChunk {

            message_id: assistant_id.clone(),

            kind: kind.clone(),

            text: text.clone(),

            tool_call: tool_call.clone(),

            usage,

        });

        if matches!(kind, StreamKind::Error) {

            errored = Some(text);

        }
            }
        }
    }

    // 移除取消信号注册（清理）
    {
        let mut map = state.cancel_signals.lock().unwrap();
        map.remove(&assistant_id);
    }

    // 中断时落库为 cancelled（保留已累积的 partial content）
    if cancelled {
        let reasoning_opt: Option<&str> =
            if reasoning_buf.is_empty() { None } else { Some(&reasoning_buf) };
        persist_turn(
            &state, conv_id, &user_id, &input.content, &assistant_id, &content_buf, reasoning_opt,
            Some(&model_name), MessageStatus::Cancelled, None, last_usage,
        )?;
        // 不推 Error chunk（前端 stop() 已标 cancelled；后端落库 Cancelled，\        // 命令返回后前端 invalidate 用 DB 状态为准）。
        return Ok(assistant_id);
    }



    // 5. 收尾：turn 结束整条落库（user + assistant）

    // reasoning 非空时传给 persist_turn，写入 messages.reasoning 列

    let reasoning_opt: Option<&str> = if reasoning_buf.is_empty() { None } else { Some(&reasoning_buf) };

    match errored {

        Some(err_msg) => {

            error!(assistant_id = %assistant_id, error = %err_msg, "stream ended with error");

            persist_turn(

                &state, conv_id, &user_id, &input.content, &assistant_id, &content_buf, reasoning_opt,

                Some(&model_name), MessageStatus::Error, Some(&err_msg), last_usage,

            )?;

            // 已通过 channel 推过 Error chunk，前端会处理；命令本身返回 Ok（消息已创建）

            Ok(assistant_id)

        }

        None => {

            persist_turn(

                &state, conv_id, &user_id, &input.content, &assistant_id, &content_buf, reasoning_opt,

                Some(&model_name), MessageStatus::Complete, None, last_usage,

            )?;

            Ok(assistant_id)

        }

    }

}



/// 落库本轮对话（user + assistant 消息，turn 结束整条写入）。

///

/// memory 模式下消息不预先建（避免 rig load 读到本轮 user 与 prompt 重复），

/// 流式结束后一次性写入。assistant 消息用预生成的 id（前端已用它做 patch）。

/// `assistant_reasoning`：reasoning 模型的思考链，非空时写入 messages.reasoning 列。

#[allow(clippy::too_many_arguments)]

fn persist_turn(

    state: &State<'_, AppState>,

    conversation_id: &str,

    user_id: &str,

    user_content: &str,

    assistant_id: &str,

    assistant_content: &str,

    assistant_reasoning: Option<&str>,

    model: Option<&str>,

    assistant_status: MessageStatus,

    error: Option<&str>,

    usage: Option<TokenUsage>,

) -> AppResult<()> {

    // user 消息（无 reasoning）

    state.memory.create_message_with_id(

        conversation_id,

        MessageRole::User,

        MessageStatus::Complete,

        user_content,

        None,

        user_id,

    )?;

    // assistant 消息（预生成 id，带 reasoning）

    state.memory.create_message_with_id_reasoning(

        conversation_id,

        MessageRole::Assistant,

        assistant_status,

        assistant_content,

        assistant_reasoning,

        model,

        assistant_id,

    )?;

    // 错误信息单独写

    if let Some(e) = error {

        state.memory.set_message_status(assistant_id, assistant_status, Some(e))?;

    }

    // usage 落库（二期 B1）

    if let Some(u) = usage {

        state.memory.set_message_usage(

            assistant_id,

            Some(u.input_tokens),

            Some(u.output_tokens),

            Some(u.total_tokens),

        )?;

    }

    Ok(())

}

/// 中断指定 assistant 消息的流式生成。
/// 触发 send_message 注册的 oneshot cancel sender → 流循环 select! 退出 → 落库 Cancelled。
#[tauri::command]
#[specta::specta]
pub async fn cancel_stream(
    message_id: String,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let tx = state.cancel_signals.lock().unwrap().remove(&message_id);
    if let Some(tx) = tx {
        let _ = tx.send(());
        info!(message_id = %message_id, "cancel_stream: signal sent");
    } else {
        tracing::warn!(message_id = %message_id, "cancel_stream: no active stream found");
    }
    Ok(())
}

/// 把本体设计宪章（不变点）摘要追加到 conversation-scope 注脚。
///
/// Palantir `OntologyCharter` 与 W3C `TtlCharter` 字段一致（业务场景/业务本质），
/// 调用方取其字段 String 传入即可，避免泛型/重复代码。
/// 完整 charter 走只读工具获取。
/// 宪章两字段全空 = 未填写（手工导入的本体常见）
fn charter_is_empty(c: &impl CharterFields) -> bool {
    c.business_scenario().trim().is_empty()
        && c.business_essence().trim().is_empty()

}

/// 两套 charter 结构字段同形，trait 统一访问（避免依赖 ontology-store 具体类型路径）
trait CharterFields {
    fn business_scenario(&self) -> &str;
    fn business_essence(&self) -> &str;
}
impl CharterFields for ontology_store::ttl::TtlCharter {
    fn business_scenario(&self) -> &str { &self.business_scenario }
    fn business_essence(&self) -> &str { &self.business_essence }
}
impl CharterFields for ontology_store::payload::OntologyCharter {
    fn business_scenario(&self) -> &str { &self.business_scenario }
    fn business_essence(&self) -> &str { &self.business_essence }
}

/// 宪章缺失注入：agent 第一轮就看到，直接知会用户补全，避免做完分析才在
/// import 撞宪章空检测闸门（白白浪费十几分钟工作）
fn append_charter_missing_notice(buf: &mut String) {
    buf.push_str("  ⚠️ 该本体的设计宪章为空（多为此前手工导入）。设计宪章是建模的业务意图基线：\n");
    buf.push_str("  **若本轮任务涉及修改/迭代该本体，请先知会用户在「设计宪章」面板补全业务场景/业务本质两字段\n");
    buf.push_str("  （可起草建议稿供用户修改确认，写入由用户操作），不要开始建模工作。**\n");
    buf.push_str("  （import 时宪章空检测闸门会拒绝导入。）\n");
}

fn append_charter_summary(buf: &mut String, business_essence: &str) {
    if !business_essence.is_empty() {
        buf.push_str(&format!("  业务本质：{business_essence}
"));
    }
}

/// charter 统一摘要：Palantir `OntologyCharter` / W3C `TtlCharter` 同形字段映射，
/// `render_conversation_scope` 不依赖两套具体 charter 类型。
struct CharterSummary {
    business_scenario: String,
    business_essence: String,
}

impl CharterFields for CharterSummary {
    fn business_scenario(&self) -> &str { &self.business_scenario }
    fn business_essence(&self) -> &str { &self.business_essence }
}

/// 构造 `<conversation-scope>` 注脚（纯函数，可单测）。
///
/// 输入为本轮解析出的全部激活范围数据。决策 17：注脚只给最小元信息（不给全文/摘要），
/// 模型按需调工具检索；挂载文档每行带 path（决策 17 修订：模型可反查知识库定位，
/// 重名文件靠路径区分）。charter 查询以闭包注入（Palantir / W3C 两套 store 同形），
/// 保持本函数无状态可测。语义约定（与 send_message 调用点一致）：
/// - charter 闭包返回 None = store 查询 Err（不追加任何内容）
/// - 返回 Some 且两字段全空白 = 宪章未填写（追加缺失知会）
/// - 返回 Some 且 business_essence 非空 = 追加业务本质摘要
fn render_conversation_scope(
    source_tables: &[(String, Vec<String>)],
    active_folders: &[String],
    skill_refs: &[(String, String)],
    mounted_refs: &[(String, String, String)],
    active_ontologies: &[String],
    palantir_charter: &dyn Fn(&str) -> Option<CharterSummary>,
    ttl_charter: &dyn Fn(&str) -> Option<CharterSummary>,
) -> String {
    let mut buf = String::new();
    buf.push_str("\n\n<conversation-scope>\n");
    buf.push_str("以下是本会话激活的知识范围（`@xxx` token 指向这些资源）。用对应工具按需检索，不必全部读取：\n");
    // 激活数据源 + 表清单
    if !source_tables.is_empty() {
        buf.push_str("\n[数据源] 可用 execute_sql 工具查询（表名用三段式 `source.schema.table`）：\n");
        for (name, tables) in source_tables {
            if tables.is_empty() {
                buf.push_str(&format!("- {name}（表清单待 list_data_sources 工具获取）\n"));
            } else {
                buf.push_str(&format!("- {name}：{} 张表 — {}\n", tables.len(), tables.join(", ")));
            }
        }
    }
    // 激活文件夹（范围标记，模型用 search_documents 在该范围检索）
    if !active_folders.is_empty() {
        buf.push_str("\n[文件夹] 可用 search_documents 工具在这些目录下检索文档：\n");
        for f in active_folders {
            buf.push_str(&format!("- {f}\n"));
        }
    }
    // 技能（@skillName 引用，skill body 入库 documents，统一 read_document 路径）
    if !skill_refs.is_empty() {
        buf.push_str("\n[技能] 可用 read_document 工具按 id 读取技能全文（SKILL.md body）：\n");
        for (id, name) in skill_refs {
            buf.push_str(&format!("- id: {id}, name: {name}\n"));
        }
    }
    // 挂载文档（可用 read_document 工具按 id 取全文；path 供模型反查知识库定位）
    if !mounted_refs.is_empty() {
        buf.push_str("\n[挂载文档] 可用 read_document 工具按 id 读取全文：\n");
        for (id, name, path) in mounted_refs {
            buf.push_str(&format!("- id: {id}, name: {name}, path: {path}\n"));
        }
    }
    // 本体引用（混存两种：Palantir api_name 或 W3C 本体 IRI，二者不冲突）。
    //  - W3C：IRI 含 `://`（如 https://...#Ont），用 export_ontology_ttl / query_ontology_sparql 钻取
    //  - Palantir：PascalCase api_name，用 describe_ontology / describe_object_type 钻取
    // 注脚附 charter 摘要（业务本质）——让 AI 无需调工具
    // 就知道本体是干什么的、有哪些红线（决策：本体不变点，向 AI 说明业务本质）。
    if !active_ontologies.is_empty() {
        let (mut ttl_refs, mut palantir_refs) = (Vec::new(), Vec::new());
        for ont in active_ontologies {
            if ont.contains("://") {
                ttl_refs.push(ont);
            } else {
                palantir_refs.push(ont);
            }
        }
        if !palantir_refs.is_empty() {
            buf.push_str("\n[本体·平台私有] 可用 describe_ontology 工具获取 OT 目录，describe_object_type 看单个 OT 详情，list_link_types/describe_link_type 看关系：\n");
            for ont in palantir_refs {
                buf.push_str(&format!("- {ont}\n"));
                match palantir_charter(ont) {
                    Some(c) if charter_is_empty(&c) => append_charter_missing_notice(&mut buf),
                    Some(c) => append_charter_summary(&mut buf, &c.business_essence),
                    None => {}
                }
            }
        }
        if !ttl_refs.is_empty() {
            buf.push_str("\n[本体·W3C 标准] 可用 export_ontology_ttl(ontology_iri) 取 Turtle 全文，query_ontology_sparql(ontology_iri, sparql) 跑 SPARQL 查询（类/属性/规则/实例）：\n");
            for iri in ttl_refs {
                buf.push_str(&format!("- {iri}\n"));
                match ttl_charter(iri) {
                    Some(c) if charter_is_empty(&c) => append_charter_missing_notice(&mut buf),
                    Some(c) => append_charter_summary(&mut buf, &c.business_essence),
                    None => {}
                }
            }
        }
    }
    buf.push_str("</conversation-scope>");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无 charter 的空查询闭包（多数用例用不到本体注脚）。
    fn no_charter(_: &str) -> Option<CharterSummary> {
        None
    }

    #[test]
    fn scope_footer_mounted_doc_line_includes_path() {
        // 决策 17 修订核心断言：挂载文档注脚每行带 path（模型可反查知识库定位）
        let mounted = &[(
            "doc_1".to_string(),
            "报告.pdf".to_string(),
            "/kb/试驾/报告.pdf".to_string(),
        )];
        let out = render_conversation_scope(
            &[], &[], &[], mounted, &[],
            &no_charter, &no_charter,
        );
        assert!(out.starts_with("\n\n<conversation-scope>\n"));
        assert!(out.contains("\n[挂载文档] 可用 read_document 工具按 id 读取全文：\n"));
        assert!(out.contains("- id: doc_1, name: 报告.pdf, path: /kb/试驾/报告.pdf\n"));
        assert!(out.ends_with("</conversation-scope>"));
    }

    #[test]
    fn scope_footer_mounted_doc_paths_distinguish_same_name() {
        // 重名文件（不同文件夹）：path 原样输出，模型可区分
        let mounted = &[
            ("doc_1".to_string(), "a.md".to_string(), "/kb/A/a.md".to_string()),
            ("doc_2".to_string(), "a.md".to_string(), "/kb/B/a.md".to_string()),
        ];
        let out = render_conversation_scope(
            &[], &[], &[], mounted, &[],
            &no_charter, &no_charter,
        );
        assert!(out.contains("path: /kb/A/a.md"));
        assert!(out.contains("path: /kb/B/a.md"));
    }

    #[test]
    fn scope_footer_renders_sources_folders_skills() {
        let sources = &[("ds1".to_string(), vec!["t1".to_string(), "t2".to_string()])];
        let folders = &["/曾国藩专题".to_string()];
        let skills = &[("sk_1".to_string(), "建模助手".to_string())];
        let out = render_conversation_scope(
            sources, folders, skills, &[], &[],
            &no_charter, &no_charter,
        );
        assert!(out.contains("\n[数据源]"));
        assert!(out.contains("- ds1：2 张表 — t1, t2\n"));
        assert!(out.contains("\n[文件夹]"));
        assert!(out.contains("- /曾国藩专题\n"));
        assert!(out.contains("\n[技能]"));
        assert!(out.contains("- id: sk_1, name: 建模助手\n"));
        // 未挂载文档时不输出 [挂载文档] 段
        assert!(!out.contains("[挂载文档]"));
    }

    #[test]
    fn scope_footer_no_scope_data_renders_header_only() {
        let out = render_conversation_scope(
            &[], &[], &[], &[], &[],
            &no_charter, &no_charter,
        );
        assert!(out.contains("<conversation-scope>"));
        assert!(!out.contains("[数据源]"));
        assert!(!out.contains("[文件夹]"));
        assert!(!out.contains("[技能]"));
        assert!(!out.contains("[挂载文档]"));
        assert!(!out.contains("[本体"));
    }

    #[test]
    fn scope_footer_ontologies_split_by_scheme_with_charter_summary() {
        // Palantir api_name（无 ://）与 W3C IRI（含 ://）分组；charter 有值时附业务本质
        let with_charter = |ont: &str| {
            let _ = ont;
            Some(CharterSummary {
                business_scenario: "二手车试驾场景".to_string(),
                business_essence: "成交意愿的结构化表达".to_string(),
            })
        };
        let out = render_conversation_scope(
            &[], &[], &[], &[],
            &["CarOntology".to_string(), "https://example.com#Ont".to_string()],
            &with_charter, &no_charter,
        );
        assert!(out.contains("\n[本体·平台私有]"));
        assert!(out.contains("- CarOntology\n"));
        assert!(out.contains("  业务本质：成交意愿的结构化表达\n"));
        assert!(out.contains("\n[本体·W3C 标准]"));
        assert!(out.contains("- https://example.com#Ont\n"));
    }

    #[test]
    fn scope_footer_empty_charter_appends_missing_notice() {
        // 宪章两字段全空白 = 未填写 → 追加缺失知会（不追加业务本质行）
        let empty_charter = |ont: &str| {
            let _ = ont;
            Some(CharterSummary {
                business_scenario: "  ".to_string(),
                business_essence: String::new(),
            })
        };
        let out = render_conversation_scope(
            &[], &[], &[], &[],
            &["Ont".to_string()],
            &empty_charter, &no_charter,
        );
        assert!(out.contains("设计宪章为空"));
        assert!(!out.contains("业务本质："));
    }

    #[test]
    fn scope_footer_charter_lookup_error_renders_nothing_extra() {
        // 闭包返回 None（对应 store 查询 Err）：不追加 charter 内容
        let out = render_conversation_scope(
            &[], &[], &[], &[],
            &["Ont".to_string()],
            &no_charter, &no_charter,
        );
        assert!(out.contains("- Ont\n"));
        assert!(!out.contains("设计宪章为空"));
        assert!(!out.contains("业务本质："));
    }
}



