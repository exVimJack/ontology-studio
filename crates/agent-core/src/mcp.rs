//! MCP（Model Context Protocol）工具系统集成（二期 A3）。
//!
//! 封装 rmcp 1.8 客户端 + Rig 0.40 的 ToolServer，把外部 MCP server 的工具
//! 注入 agent 的 ToolSet，agent loop 自动发现并调用。
//!
//! 架构（见 ARCHITECTURE.md 决策 1 / §九 二期 A3）：
//!
//! ```text
//! MCP server (stdio/http) ─┐
//!                          ├─▶ rmcp client ─▶ McpTool(ToolDyn) ─▶ ToolServerHandle (共享)
//! MCP server (stdio/http) ─┘                                         │
//!                                                                     ▼
//!                                           ChatService.stream() 用此 handle 构建 agent
//! ```
//!
//! 关键设计：
//! - **自实现 ToolDyn 桥接**：不使用 rig 的 `tool::rmcp` 模块（它针对 rmcp 3.0-beta API，
//!   且开启会触发 rmcp-macros → darling 的 cargo resolver bug）。我们直接用 rmcp 1.8
//!   的 `Peer::list_all_tools` / `Peer::call_tool`，把每个 MCP 工具包装成 `ToolDyn`
//!   注册到 Rig 的 `ToolServerHandle`。
//! - **静态工具模式**：连接时一次性拉取工具列表注册；list_changed 通知暂不处理
//!   （一期够用，后续可加 `ClientHandler::on_tool_list_changed`）。
//! - **stdio transport 自实现**：不用 rmcp 的 `transport-child-process`（依赖
//!   process-wrap 拉入 windows crate 重型包，当前 toolchain 编译失败），改用
//!   `tokio::process::Command` spawn 子进程 + rmcp 的 `(AsyncRead, AsyncWrite)` transport。
//! - **HTTP transport**：rmcp 的 `transport-streamable-http-client-reqwest`（reqwest+rustls）。
//! - **连接生命周期**：`RunningService` 持于 `McpManager`，drop 自动断开。

use std::collections::HashMap;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, Content,
    Implementation, JsonObject, RawContent, Tool,
};
use rmcp::service::{Peer, RunningService, RoleClient};
use rmcp::ServiceExt;
use rig::tool::server::ToolServerHandle;
use rig::tool::{ToolContext, ToolExecutionError, ToolOutput};
use rig::tool::DynamicTool;
use serde::{Deserialize, Serialize};
use tokio::process::{ChildStdin, ChildStdout};

use crate::error::{AgentError, AgentResult};

// ── 配置（持久化到设置，specta::Type 供前端 IPC 类型生成） ──────

/// 单个 MCP server 的连接配置。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpServerConfig {
    /// stdio 子进程传输（本地 MCP server，如 `npx @modelcontextprotocol/server-filesystem`）。
    Stdio {
        /// 唯一标识（前端展示与去重用）。
        id: String,
        /// 展示名。
        name: String,
        /// 可执行命令（如 "npx" / "node" / "uvx"）。
        command: String,
        /// 命令参数。
        args: Vec<String>,
        /// 额外环境变量（追加到进程继承的环境）。
        #[serde(default)]
        env: HashMap<String, String>,
    },
    /// HTTP / Streamable HTTP 传输（远程 MCP server）。
    Http {
        /// 唯一标识。
        id: String,
        /// 展示名。
        name: String,
        /// server URL（如 "https://example.com/mcp"）。
        url: String,
        /// 可选 Bearer token（发送 `Authorization: Bearer <token>`）。
        #[serde(default)]
        auth_token: Option<String>,
        /// 可选自定义请求头。
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

impl McpServerConfig {
    pub fn id(&self) -> &str {
        match self {
            McpServerConfig::Stdio { id, .. } | McpServerConfig::Http { id, .. } => id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            McpServerConfig::Stdio { name, .. } | McpServerConfig::Http { name, .. } => name,
        }
    }
}
// ── Rig DynamicTool 桥接：把单个 MCP 工具适配为 Rig 工具（0.41 API） ──

/// 把一个 MCP server 工具包装成 Rig 的 `DynamicTool`。
///
/// 持有 server 的 `Peer`（用于 `call_tool`）与工具定义（name/desc/schema）。
/// agent loop 调用 DynamicTool 的 callback 时，转成 `CallToolRequestParams`
/// 发给 MCP server，把返回的 `CallToolResult.content` 拼成文本交给模型。
///
/// 0.41 起 rig 取消 `ToolDyn` trait，改用 `DynamicTool::new(name, desc, params, callback)`。
/// callback 签名 `Fn(&mut ToolContext, Value) -> Future<Result<ToolOutput, ToolExecutionError>>`。
fn build_dynamic_tool(tool: &Tool, peer: Peer<RoleClient>) -> DynamicTool {
    let name = tool.name.to_string();
    let description = tool
        .description
        .as_ref()
        .map(|c| c.to_string())
        .unwrap_or_default();
    let parameters = schema_as_json_value(&tool.input_schema);
    let tool_name = name.clone();
    DynamicTool::new(
        name,
        description,
        parameters,
        move |_ctx: &mut ToolContext, args: serde_json::Value| {
            let tool_name = tool_name.clone();
            let peer = peer.clone();
            Box::pin(async move {
                let arguments = parse_arguments_value(args)?;
                let mut req = CallToolRequestParams::new(tool_name);
                if let Some(obj) = arguments {
                    req = req.with_arguments(obj);
                }
                let result = peer
                    .call_tool(req)
                    .await
                    .map_err(|e| ToolExecutionError::provider(format!("MCP call failed: {e}")))?;
                Ok(ToolOutput::text(call_result_to_text(&result)))
            })
        },
    )
}





/// 把 `CallToolResult` 的 content 列表拼成模型可见文本。
///
/// - Text → 原文
/// - Image → `data:{mime};base64,{data}` 占位（模型可识别但一期不渲染）
/// - Resource → `uri:text` 形式
/// - 工具报告错误（is_error=true）→ 内容前缀 `[tool error]`
fn call_result_to_text(result: &CallToolResult) -> String {
    let mut buf = String::new();
    if result.is_error == Some(true) {
        buf.push_str("[tool error] ");
    }
    for content in &result.content {
        let text = content_to_text(content);
        if !buf.is_empty() && !text.is_empty() {
            buf.push('\n');
        }
        buf.push_str(&text);
    }
    buf
}

fn content_to_text(content: &Content) -> String {
    match &content.raw {
        RawContent::Text(t) => t.text.clone(),
        RawContent::Image(img) => {
            format!("data:{};base64,{}", img.mime_type, img.data)
        }
        RawContent::Resource(r) => match &r.resource {
            rmcp::model::ResourceContents::TextResourceContents { uri, text, .. } => {
                format!("{uri}:{text}")
            }
            rmcp::model::ResourceContents::BlobResourceContents { uri, blob, .. } => {
                format!("{uri}:{blob}")
            }
        },
        RawContent::Audio(_) => "[unsupported audio content]".into(),
        RawContent::ResourceLink(_) => "[resource link]".into(),
    }
}

/// MCP 工具的 input_schema（Arc<JsonObject>）→ JSON Value（ToolDefinition.parameters 用）。
fn schema_as_json_value(schema: &Arc<JsonObject>) -> serde_json::Value {
    serde_json::Value::Object(schema.as_ref().clone())
}

/// 解析 agent 传入的 JSON 参数为 MCP JsonObject。
/// 空/Null/非对象 → None（无参数）；对象 → Some。
fn parse_arguments_value(value: serde_json::Value) -> Result<Option<JsonObject>, ToolExecutionError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(_) => Ok(Some(serde_json::from_value(value).map_err(
            |e| ToolExecutionError::invalid_args(format!("arguments parse: {e}")),
        )?)),
        _ => Ok(None),
    }
}

/// 旧字符串接口（测试用）：先解析 JSON 再走 value 版本。
#[cfg(test)]
fn parse_arguments(args: &str) -> Result<Option<JsonObject>, ToolExecutionError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| ToolExecutionError::invalid_args(format!("invalid JSON arguments: {e}")))?;
    parse_arguments_value(value)
}

// ── stdio transport（tokio::process + (stdout, stdin)） ─────────

/// spawn 子进程取 stdout/stdin 作为 MCP transport。
///
/// 子进程 stderr 丢弃（后续可接 tracing）。子进程句柄保活于 `McpChildGuard`，
/// 连接断开时由 tokio Child 的 drop 行为处理（默认不 kill，但 stdin 关闭会
/// 触发 server 优雅退出）。
struct StdioTransportParts {
    read: ChildStdout,
    write: ChildStdin,
    _guard: McpChildGuard,
}

/// 持有子进程句柄，保活。drop 时不主动 kill（让 stdin 关闭触发退出）。
#[allow(dead_code)]
struct McpChildGuard(tokio::process::Child);

impl StdioTransportParts {
    fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> std::io::Result<Self> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        apply_creation_flags(&mut cmd);
        let mut child = cmd.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("子进程未产生 stdout"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("子进程未产生 stdin"))?;
        Ok(Self {
            read: stdout,
            write: stdin,
            _guard: McpChildGuard(child),
        })
    }
}

// ── McpManager ─────────────────────────────────────────────────

/// MCP 连接管理器。持有所有已连接 server 的 running service（保持连接存活）
/// 与共享的 `ToolServerHandle`（注入 agent）。
///
/// 一期：静态工具模式——连接时拉取工具列表注册到共享 handle。
/// 工具列表变更通知暂不处理（需实现 ClientHandler）。
pub struct McpManager {
    tool_handle: ToolServerHandle,
    /// 所有保持存活的连接 + 子进程 guard。drop 即断开。
    connections: Vec<McpConnection>,
}

struct McpConnection {
    /// RunningService 保活连接。Peer 从中取（注册工具后不再需要 service，但
    /// 必须保持 service 存活否则连接断开）。
    _service: RunningService<RoleClient, ClientInfo>,
    _guard: Option<McpChildGuard>,
}

impl McpManager {
    /// 构造空管理器（无 MCP server）。tool_handle 通常由 ChatService 共享。
    pub fn new(tool_handle: ToolServerHandle) -> Self {
        Self {
            tool_handle,
            connections: Vec::new(),
        }
    }

    /// 暴露共享工具句柄，供 `ChatService` 注入 agent。
    pub fn tool_handle(&self) -> &ToolServerHandle {
        &self.tool_handle
    }

    /// 并发连接所有配置的 MCP server，把工具注册到共享 handle。
    ///
    /// 单个 server 失败不影响其他——失败项记入返回列表，成功项正常注册。
    pub async fn connect_all(
        &mut self,
        configs: &[McpServerConfig],
    ) -> Vec<(String, AgentError)> {
        let mut errors: Vec<(String, AgentError)> = Vec::new();
        for cfg in configs {
            let result = match cfg {
                McpServerConfig::Stdio { command, args, env, .. } => {
                    self.connect_stdio(command, args, env).await
                }
                McpServerConfig::Http { url, auth_token, headers, .. } => {
                    self.connect_http(url, auth_token.clone(), headers).await
                }
            };
            match result {
                Ok(count) => {
                    tracing::info!(
                        server = cfg.name(),
                        tools = count,
                        "MCP server 连接成功，工具已注册"
                    );
                }
                Err(e) => {
                    tracing::warn!(server = cfg.name(), error = %e, "MCP server 连接失败");
                    errors.push((cfg.name().to_string(), e));
                }
            }
        }
        errors
    }

    /// 当前已连接的 server 数量。
    pub fn connected_count(&self) -> usize {
        self.connections.len()
    }

    async fn connect_stdio(
        &mut self,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> AgentResult<usize> {
        let parts = StdioTransportParts::spawn(command, args, env)
            .map_err(|e| AgentError::Mcp(format!("stdio spawn '{command}': {e}")))?;
        let guard = parts._guard;
        let svc = self
            .client_info()
            .serve((parts.read, parts.write))
            .await
            .map_err(|e| AgentError::Mcp(format!("stdio connect '{command}': {e}")))?;
        let count = self.register_tools(&svc).await?;
        self.connections.push(McpConnection {
            _service: svc,
            _guard: Some(guard),
        });
        Ok(count)
    }

    async fn connect_http(
        &mut self,
        url: &str,
        auth_token: Option<String>,
        headers: &HashMap<String, String>,
    ) -> AgentResult<usize> {
        use rmcp::transport::streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        };
        let mut config = StreamableHttpClientTransportConfig::with_uri(url);
        if let Some(token) = auth_token {
            config = config.auth_header(token);
        }
        if !headers.is_empty() {
            config = config.custom_headers(http_header_map(headers));
        }
        let transport = StreamableHttpClientTransport::<reqwest::Client>::with_client(
            reqwest::Client::new(),
            config,
        );
        let svc = self
            .client_info()
            .serve(transport)
            .await
            .map_err(|e| AgentError::Mcp(format!("http connect '{url}': {e}")))?;
        let count = self.register_tools(&svc).await?;
        self.connections.push(McpConnection {
            _service: svc,
            _guard: None,
        });
        Ok(count)
    }

    /// 拉取 server 的工具列表，每个包装成 McpTool 注册到共享 handle。
    async fn register_tools(
        &self,
        svc: &RunningService<RoleClient, ClientInfo>,
    ) -> AgentResult<usize> {
        let peer = svc.peer().clone();
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|e| AgentError::Mcp(format!("list_tools: {e}")))?;
        let count = tools.len();
        for tool in tools {
            let dyn_tool = build_dynamic_tool(&tool, peer.clone());
            self.tool_handle.add_dynamic_tool(dyn_tool).await;
        }
        Ok(count)
    }

    fn client_info(&self) -> ClientInfo {
        ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("onto-studio", env!("CARGO_PKG_VERSION")),
        )
    }
}

/// 把 `HashMap<String,String>` 转为 `HashMap<HeaderName,HeaderValue>`。
fn http_header_map(
    headers: &HashMap<String, String>,
) -> HashMap<http::HeaderName, http::HeaderValue> {
    let mut map = HashMap::new();
    for (k, v) in headers {
        if let (Ok(name), Ok(val)) = (
            http::HeaderName::from_bytes(k.as_bytes()),
            http::HeaderValue::from_str(v),
        ) {
            map.insert(name, val);
        }
    }
    map
}

// ── 平台适配：Windows CREATE_NO_WINDOW ───────────────────────

#[cfg(windows)]
fn apply_creation_flags(cmd: &mut tokio::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn apply_creation_flags(_cmd: &mut tokio::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_serde_stdio_roundtrip() {
        let cfg = McpServerConfig::Stdio {
            id: "fs".into(),
            name: "filesystem".into(),
            command: "npx".into(),
            args: vec!["@modelcontextprotocol/server-filesystem".into(), "/tmp".into()],
            env: HashMap::from([("FOO".into(), "bar".into())]),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert!(json.contains(r#""kind":"stdio""#));
    }

    #[test]
    fn config_serde_http_roundtrip() {
        let cfg = McpServerConfig::Http {
            id: "remote".into(),
            name: "remote-mcp".into(),
            url: "https://example.com/mcp".into(),
            auth_token: Some("tok".into()),
            headers: HashMap::new(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert!(json.contains(r#""kind":"http""#));
    }

    #[test]
    fn config_accessors() {
        let cfg = McpServerConfig::Stdio {
            id: "x".into(),
            name: "X".into(),
            command: "c".into(),
            args: vec![],
            env: HashMap::new(),
        };
        assert_eq!(cfg.id(), "x");
        assert_eq!(cfg.name(), "X");
    }

    #[test]
    fn parse_arguments_classifies() {
        assert!(parse_arguments("").unwrap().is_none());
        assert!(parse_arguments("   ").unwrap().is_none());
        assert!(parse_arguments("null").unwrap().is_none());
        assert!(parse_arguments("[1,2]").unwrap().is_none());
        assert!(parse_arguments("{}").unwrap().is_some());
        let obj = parse_arguments("{\"a\":1}").unwrap().unwrap();
        assert_eq!(obj.get("a"), Some(&serde_json::json!(1)));
        assert!(parse_arguments("{bad json").is_err());
    }
}
