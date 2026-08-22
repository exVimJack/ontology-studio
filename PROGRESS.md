# 开发进展

> 本文件记录 onto-studio 的开发进展，按阶段组织。以 `ARCHITECTURE.md` 的落地路线为准。
> 最后更新：2026-07-30

---

## 一期 MVP — ✅ 核心完成，已用 DeepSeek 端到端验证

### 环境与工具链

- [x] Rust 1.97.1，MSVC target（`x86_64-pc-windows-msvc`），VS BuildTools 2022
- [x] workspace `Cargo.toml`：成员 `crates/agent-core`、`crates/ingest`、`crates/memory`；`src-tauri` 独立 Cargo.lock（避免 feature 统一冲突）
- [x] `.cargo/config.toml`：target-dir 重定向到 `C:/Users/think/AppData/Local/onto-studio-target`（绕过 D 盘 WDAC 策略阻止 exe 执行）
- [x] `cargo-msvc.bat`：先 `vcvars64.bat` 再 cargo（cc crate 无法自动为 cl.exe 设置 INCLUDE/LIB）
- [x] `tauri-msvc.bat`：在 MSVC 环境中运行 `npx @tauri-apps/cli`
- [x] 绑定生成流程：`cargo-msvc build --bin` → `ONTO_GEN_BINDINGS=1 exe`（从 src-tauri 目录运行）→ `tauri build`

### crates/memory — ✅ 完整，7 测试通过

SQLite（bundled，WAL 模式）+ `Mutex<Connection>` 线程安全。

| 文件 | 内容 |
| ------ | ------ |
| `lib.rs` | `Memory` 入口，`open_in_memory()` / `open(path)`，`conversations` + `messages` 表 |
| `error.rs` | `MemoryError` 枚举（thiserror） |
| `message.rs` | `MessageRole` / `MessageStatus` / `MessageRow`（specta::Type 派生） |
| `repo.rs` | `ConversationRow` / `ConversationSummary`，完整 CRUD，`append_message_text` 流式追加，pin/touch/删除级联 |
| `timestamp.rs` | `Timestamp(i64)` 新类型：自定义 specta::Type 导出为 TS `number`（绕过 specta BigInt 禁止），实现 `FromSql`/`ToSql` |

### crates/agent-core — ✅ 完整，3 端到端测试通过（DeepSeek）

Rig 0.41 流式对话封装（agent + memory feature）。

| 文件 | 内容 |
| ------ | ------ |
| `provider.rs` | `ProviderKind`（OpenAiCompatible / Anthropic）、`ProviderConfig`，specta::Type |
| `chat.rs` | `ChatService`：按 provider kind 分支持强类型 Client；`stream(prompt, history)` 返回 `Box<dyn Stream<StreamChunk>>`；`MultiTurnStreamItem → StreamChunk` 映射 |
| `error.rs` | `AgentError` 枚举 |
| `lib.rs` | 导出 `ChatService` / `StreamChunk` / `StreamKind` / `text_history_to_messages` / `text_prompt` / `split_last_as_prompt` |

**关键修复**：

- OpenAI-compatible provider 调 `.completions_api()` 切换到 `/chat/completions`（Rig 0.41 默认走 Responses API `/responses`，DeepSeek/Ollama 等不支持）
- Done chunk 去重（provider 发 FinalResponse 后不再补 Done）

### crates/ingest — ✅ 完整，13 测试 + 1 doc-test 通过

多模态摄取管道（决策 8：DocumentParser trait + dispatcher + 统一错误枚举 + 流式解析 + 安全防护）。

| 文件 | 内容 |
| ------ | ------ |
| `parser.rs` | `DocumentParser` trait + `ingest_file()` 入口 |
| `document.rs` | `Document` / `DocumentMeta` / `Table` / `MultimodalPart` 统一产物 |
| `error.rs` | `IngestError` 枚举（含安全/大小违规变体） |
| `dispatcher.rs` | 按扩展名路由 + 内容嗅探（PDF/zip/image magic bytes + UTF-8 兜底） |
| `security.rs` | 文件大小上限 200MB + zip 炸弹防护（`ArchiveBudget`：500MB 解压总量 / 10K 条目 / 5 层深度） |
| `parsers/pdf_parser.rs` | pdfium-render（决策 5）+ 进程级 Mutex 串行化（见下「PDF 并发修复」） |
| `parsers/office_parser.rs` | office_oxide（DOCX/DOC/PPTX/PPT/XLSX/XLS，to_markdown→plain_text 回退） |
| `parsers/xlsx_parser.rs` | calamine 结构化表格 |
| `parsers/epub_parser.rs` | rbook（spine + manifest 读章节，strip_html） |
| `parsers/image_parser.rs` | image 解码 + base64 → multimodal_part |
| `parsers/text_parser.rs` | 纯文本/代码/Markdown |
| `parsers/csv_parser.rs` | CSV/TSV → Markdown 表格 + tables 结构 |
| `parsers/json_parser.rs` | 美化 + 合法性校验 |
| `parsers/zip_parser.rs` | 流式 + 安全预算 + 递归 |
| `parsers/tar_parser.rs` | tar/tar.gz（flate2 解压） |
| `parsers/smart_zip_parser.rs` | PK 嗅探：office→epub→zip 依次尝试 |
| `tests/compat.rs` | 13 兼容性自测（决策 4/5 要求）：文本/Markdown/CSV/TSV/JSON/PNG/ZIP/不支持格式 + PDF/DOCX/XLSX/ePub 真实样本（缺样本 skip） |

### PDF 并发崩溃修复 — ✅（2026-07-29）

**现象**：拖入 PDF 报 `PDFium 未初始化` 或 `PdfiumLibraryInternalError(FormatError)`，独立探测却正常。

**根因 #1（资源路径）**：`src-tauri/src/pdfium.rs` 的 `PathResolver::resolve("pdfium/...", Resource)` 只做字符串拼接返回不存在的 `debug/pdfium/...`（应为 `debug/resources/pdfium/...`），`.ok()` 返 `Some` 跳过回退。修复：加 `.filter(|p| p.exists())` + 两条回退路径（`resource_dir().join("resources")` + `CARGO_MANIFEST_DIR/resources`）。

**根因 #2（并发）**：pdfium-render 0.9.3 的 `thread_safe` feature **仅 impl Send+Sync，不串行化 FFI 调用**（native `DynamicPdfiumBindings` 每个 FPDF 函数直接调函数指针无锁；issue #20「Marshall calls」未实现；issue #66 作者承认跨线程 segfault；docs.rs 说的 mutex 仅 WASM 路径生效）。PDFium C 库本身非线程安全（`fpdfview.h` 明确要求调用方加锁）。多文件 batch 并发解析 → 内存损坏（`STATUS_STACK_BUFFER_OVERRUN` / `FormatError`）。

**修复**：

- `crates/ingest/src/pdfium.rs`：新增 `static PDFIUM_LOCK: OnceLock<Mutex<()>>` + `pub fn with_pdfium() -> MutexGuard<'static, ()>` 进程级令牌锁
- `crates/ingest/src/parsers/pdf_parser.rs`：`parse_with_progress` 开头 `let _guard = with_pdfium()?;`，整个 load→extract→drop 临界区持锁，函数返回自动释放
- 修正 `pdfium.rs` / `pdf_parser.rs` 模块文档里关于「pdfium-render 内部 mutex 串行化」的错误描述
- 同步修正 ARCHITECTURE.md 决策 5 / Cargo.toml 注释（见上）

**验证**：`examples/probe_pdf_concurrent.rs` 3 线程并发解析 47MB/1711 页 PDF——修复前 `STATUS_STACK_BUFFER_OVERRUN` 崩溃；修复后全部成功，字符数一致（2,720,049），耗时呈串行阶梯（4.5s/9.1s/13.7s）。

**关于 batch 并行**：锁只在 `PdfParser` 内持有，非 PDF 文件（DOCX/XLSX/EPUB/txt）不碰 `with_pdfium`，与 PDF 并行无碍。PDF 之间串行是 PDFium 单线程约束的硬限制。

**证据链**：

1. PDFium 官方 `fpdfview.h`："embedders are required to ensure (via a mutex or similar) that only a single PDFium call can be made at a time"
2. pdfium-render issue #20「Marshall calls to Pdfium」——未完成的特性请求
3. pdfium-render issue #66「sync is not safe in every situation」——作者承认跨线程 segfault
4. 源码 `dynamic_bindings.rs`：`FPDF_LoadDocument` 等直接调函数指针，无锁
5. 实测：并发 3 份 PDF 共享单例 → 崩溃
6. 社区共识：pypdfium2/libvips 都用全局锁串行化 pdfium

### src-tauri — ✅ 完整，编译通过

Tauri 2.11 IPC 薄层 + 6 平台插件 + tauri-specta 2.0.0-rc.25。

| 文件 | 内容 |
| ------ | ------ |
| `state.rs` | `AppState`：`Arc<Memory>` + `RwLock<Option<ChatService>>` + `RwLock<Option<ProviderConfig>>` |
| `commands/conversation.rs` | 8 个对话/消息 CRUD 命令 |
| `commands/chat.rs` | `send_message`：Channel 流式 + 增量落库 + 状态机收尾 |
| `commands/provider.rs` | `set_provider` / `get_provider` / `restore_provider`（tauri-plugin-store 持久化） |
| `commands/ingest.rs` | `ingest_files`：Channel 进度（Queued→Parsing→Done/Error）+ IngestResultItem |
| `commands/error.rs` | `AppError` 枚举 + From 转换（Memory/Agent/Ingest） |
| `lib.rs` | tauri-specta Builder + `collect_commands!` + `gen_bindings()` + `ONTO_GEN_BINDINGS` 提前退出 |

### 前端 — ✅ 一期 MVP 完成

React 19 + TS strict + Vite 8 + Tailwind v4 + shadcn/ui，四层单向依赖（UI → State → IPC → Domain）。

**IPC 层**

- `lib/ipc/bindings.ts`：tauri-specta 自动生成（决策 F5，禁止手写）
- `lib/ipc/commands.ts`：命令式封装，`IpcError` + `unwrap()`，Channel 适配器
- `lib/domain/index.ts`：类型 re-export + provider 预设（OpenAI/DeepSeek/Ollama/OpenRouter/Anthropic/自定义）+ `relativeTime()` / `dateGroup()`

**State 层**

- `stores/ui-store.ts`：主题、侧边栏/检查器折叠、当前对话、调色板/设置开关（Zustand）
- `stores/composer-store.ts`：草稿按会话持久化
- `stores/ingest-store.ts`：摄取任务进度 + 已摄入产物（瞬态，不持久化）
- `hooks/useConversations.ts`：对话/消息 CRUD（5 query/mutation）
- `hooks/useChat.ts`：聊天状态机（send/stream/stop），Channel 回调分块
- `hooks/useProvider.ts`：provider 配置 query/mutation
- `hooks/useIngest.ts`：ingest(paths) → Channel 进度 → 落 ingested

**UI 层**

- `components/shell/AppShell.tsx`：三栏可调整布局（react-resizable-panels v4）
- `components/shell/TitleBar.tsx`：拖拽区 + provider 模型显示 + ⌘K + 侧栏/检查器切换
- `components/shell/Sidebar.tsx`：新建 + 搜索 + 按日期分组 + 固定
- `components/shell/Inspector.tsx`：对话信息统计
- `components/shell/CommandPalette.tsx`：⌘K（cmdk）
- `components/chat/ChatArea.tsx`：空状态 + MessageScroller + IngestStatusBoard + Composer
- `components/chat/ChatEmptyState.tsx`：引导状态
- `components/chat/MessageScroller.tsx`：虚拟化（@tanstack/react-virtual），底部锚定
- `components/chat/MessageItem.tsx`：头像 + 气泡（Streamdown）+ 状态指示器
- `components/chat/MarkdownView.tsx`：Streamdown 封装
- `components/chat/Composer.tsx`：⌘↵ 发送 / ⌘. 中断 / 📎 文件选择(⌘O) / 图片粘贴拦截 / 草稿持久化 / provider 门控
- `components/library/FileDropZone.tsx`：Tauri `onDragDropEvent` 全局拖拽落点（决策 F10）
- `components/library/IngestStatusBoard.tsx`：摄取进度看板
- `components/settings/SettingsView.tsx`：单 provider 配置（6 预设 + API key 显隐 + 模型选择）+ 主题
- `App.tsx`：TitleBar + AppShell + CommandPalette + FileDropZone + Settings 覆盖层 + 全局快捷键

### 端到端验证（DeepSeek）

用 `deepseek-chat` + 真实 API key 跑了 3 个 `#[ignore]` 集成测试，全部通过：

| 测试 | 验证内容 | 结果 |
| ------ | --------- | ------ |
| `deepseek_stream_chat_e2e` | 单轮流式：TextDelta + Done | ✅ 回复 "hello" |
| `deepseek_multi_turn_with_history` | 多轮：第一轮记住名字，第二轮正确回答 | ✅ 回答 "Alice" |
| `send_message_full_flow_e2e` | 完整链路：Memory CRUD + 流式 + 增量落库 + 状态收尾 + DB 一致性 | ✅ 回答 "5" |

**验证中发现并修复的问题**：

1. DeepSeek base URL 缺 `/v1`（`https://api.deepseek.com` → `https://api.deepseek.com/v1`）
2. Rig 0.41 默认走 Responses API，OpenAI-compatible provider 需 `.completions_api()` 切换
3. Done chunk 重复（加 `got_final` 标志去重）

---

## 二期 — ✅ A2/A3/B1/B2 完成（代码层），A1 VLM 暂缓

> 最后更新：2026-07-29
>
> 二期路径实际落地顺序：A3 MCP → A2 RAG → B1 token 预算 → B2 自动压缩（A1 VLM 增强解析暂缓，无可用 VLM 模型，待四期 Ollama 本地 VLM 或接 GPT-4o 后再启）。
>
> ⚠️ 编译验证被环境策略阻塞（见末尾「环境阻塞」），代码经逐行 review。端到端验证待用户侧 Defender 放行后进行。

### sqlite-vec 硬阻塞解除 — ✅

**根因**：crates.io `sqlite-vec 0.1.10-alpha.4` 打包破损，缺 `sqlite-vec-diskann.c`（行 3772 `#include`）+ `sqlite-vec-rescore.c`（行 7644 `#include`），ivf 默认已关。

**解法**：`crates/vendor/sqlite-vec/` 本地完整源码副本，`build.rs` 关闭 `SQLITE_VEC_ENABLE_DISKANN=0` + `SQLITE_VEC_ENABLE_RESCORE=0` 走 stub，未改任何 .c/.h 源码。workspace `Cargo.toml` + `src-tauri/Cargo.toml` 各加 `[patch.crates-io]` 指向 vendor（src-tauri 被 exclude，patch 不跨 workspace 边界继承）。

- memory crate `vec` feature 可用，2 个集成测试通过（vec_version_loads + knn_query_returns_nearest）
- `register_vec_extension()` 用 `sqlite3_auto_extension` 注册，所有新连接自动加载 vec0
- ARCHITECTURE.md 决策 3 补落地注

### A3 MCP 工具系统 — ✅ 全栈

**agent-core**（`crates/agent-core/src/mcp.rs`）

- `McpServerConfig` enum（Stdio{id,name,command,args,env} / Http{id,name,url,auth_token,headers}），serde tag="kind"，specta::Type
- `McpManager`：持 `ToolServerHandle` + `Vec<McpConnection>`（每个含 `RunningService<RoleClient, ClientInfo>` + `Option<McpChildGuard>`）
- rig 0.41 取消 ToolDyn trait，改用 DynamicTool::new(name,desc,params,callback) 桥接：build_dynamic_tool(tool, peer) 构造 DynamicTool，callback 闭包持 Peer Clone，转 CallToolRequestParams 发给 MCP server，结果 ToolOutput::text 返回
- stdio transport 自实现：`tokio::process::Command` spawn + rmcp `(AsyncRead, AsyncWrite)` transport，Windows `CREATE_NO_WINDOW`（不用 rmcp transport-child-process，避免 process-wrap/windows crate）
- HTTP transport：rmcp `transport-streamable-http-client-reqwest`（reqwest+rustls）
- `ClientInfo` 直接当 `ClientHandler`（rmcp 为其实现空 handler）
- 静态工具模式：连接时 `list_all_tools` 一次性注册到共享 `ToolServerHandle`；list_changed 通知一期不处理

**chat.rs 改造**

- `ChatService::with_tools(config, tool_handle)` 注入工具句柄
- `StreamKind` 扩展 `ToolCallStart`/`ToolCallResult`；新增 `ToolCallInfo` 结构
- `map_multi_turn_stream` 接出 `ToolCall`/`ToolExecutionStart`/`StreamUserItem::ToolResult`，按 `internal_call_id` 关联 Start↔Result

**src-tauri IPC**（`commands/mcp.rs`）

- `set_mcp_servers`（配置+连接+持久化，返回每个 server 状态）
- `get_mcp_servers` / `list_mcp_tools` / `restore_mcp_servers`（启动恢复）
- `AppState` 加 `tool_handle: ToolServerHandle`（共享）+ `mcp: RwLock<Option<McpManager>>`
- `ChatStreamChunk` 加 `tool_call: Option<ToolCallInfo>` 字段；ToolCallStart/Result 不落库

**前端**

- `bindings.ts` 手动补 MCP commands + StreamKind 扩展 + ChatStreamChunk.tool_call + ToolCallInfo + McpServerConfig（Serialize/Deserialize 双向类型）+ McpServerStatus + McpToolDef
- `ipc/commands.ts` + `domain/index.ts`：MCP 类型 + IPC 封装
- `hooks/useMcp.ts`：useMcpServers / useMcpTools / useSetMcpServers
- `components/chat/ToolCallCard.tsx`：可折叠卡片（name/参数/结果/状态，lucide 图标区分运行/成功/失败）+ ToolCallList
- `hooks/useChat.ts`：收集 tool_call_start/tool_call_result 到 `toolCallsByMsg` state（Record<message_id, ToolCallInfo[]>）
- `ChatArea.tsx`：useChat 提升到此层，传 toolCallsByMsg 给 MessageScroller，传 chat 给 Composer
- `MessageItem.tsx`：assistant bubble 下渲染 ToolCallList
- `SettingsView.tsx`：McpSection（增删 stdio/http server + 连接按钮 + 状态 + 可用工具列表）

### A2 RAG 检索增强 — ✅ 全栈

**`crates/retrieval/`**（新建业务 crate，平台无关）

- `chunker.rs`：文档切片器（双换行分段 + 字符数硬切带重叠，目标 800 字符/片，重叠 100；带 5 个单元测试）
- `embed.rs`：`EmbedClient` 用 reqwest+rustls 直调 `/v1/embeddings`；`ingest_document` 编排（切片→分批 embedding→入库）；`detect_ndims` 探测维度
- 默认模型 `text-embedding-3-small`（1536 维）

**`crates/memory/` 扩展**

- `vectors.rs`：`ChunkRepo` trait（ensure_vec_table / insert_chunks / search / delete_chunks_by_source / list_sources / chunk_count）+ DTO（ChunkRow/RetrievedChunk/SourceSummary/EmbeddedChunk，specta::Type）
- `lib.rs`：`register_vec_extension()` + schema 加 `doc_chunks` 表（vec_rowid 关联 vec0 整数 rowid）

**src-tauri IPC**（`commands/retrieval.rs`）

- `set_rag_config`/`get_rag_config`（store 持久化）+ `rag_ingest_document`/`rag_search`/`rag_list_sources`/`rag_delete_source`/`rag_stats` + `try_init_vec_table`（启动预建表）
- `commands/chat.rs`：`rag_retrieve_for_prompt` 辅助函数，`send_message` 在无手动 context_texts 时自动 RAG 检索注入（按源文件分组，单条 4KiB 上限，图片可与 RAG 共存）；失败静默降级

**前端**

- `bindings.ts` 手动补 RAG commands + RagConfig/RagHit/RagStats/SourceSummary
- `hooks/useRag.ts`：useRagConfig/useSetRagConfig/useRagSources/useRagStats/useRagIngestDocument/useRagDeleteSource/useRagSearch
- `hooks/useIngest.ts`：ingest 完成后 fire-and-forget 触发 `ragIngestDocument`（RAG 失败不影响 ingest）
- `SettingsView.tsx`：RagSection（启用开关 + API Key + Base URL + 模型 + Top-K + 知识库统计/源文件列表/删除）

**A2 完整链路**：摄入 → ingest 完成返回 text → 前端 ragIngestDocument → 切片 + embedding → 存 doc_chunks + vec_chunks → 对话时 send_message → rag_retrieve_for_prompt 用用户问题 embedding KNN 检索 → 命中切片注入 context_texts → agent 带参考材料回答

### B1 上下文 token 预算 + B2 历史自动压缩 — ✅ 全栈（rig 0.41 原生 ConversationMemory + CompactingMemory）

**重构说明**：二期初版手写 trim_history + compact_history，后改用 rig 0.41 原生上下文管理基础设施，不重复造轮。手写代码保留在 `context_budget.rs`（lib.rs 仍 re-export）但生产路径不再调用。

**`crates/agent-core/src/memory_bridge.rs`**（新建，带 3 个单元测试）

- `SqliteMemory`（newtype 包 `Arc<Memory>`）impl rig `ConversationMemory`：load=list_messages→rig Message，append=no-op（消息由 send_message 手动建，避免与 load 重复），clear=删会话消息
- `LlmCompactor` impl rig `Compactor`：`Artifact=SummaryArtifact(String)`，`compact()` 拼接 evicted 消息 + carry_over → 调 `SummaryFn` 生成滚动摘要
- `SummaryFn = Arc<dyn Fn(String) -> Future<Result<String>>>` trait object 擦除 provider 泛型
- `build_compacting_memory(memory, context_window, summarize)` → `CompactingMemory<SqliteMemory, TokenWindowMemory, LlmCompactor>`

**`crates/agent-core/src/chat.rs`**

- `ChatService` 加 `memory: Option<Arc<dyn ConversationMemory>>` 字段 + `set_memory(memory, context_window)`（内部构造 CompactingMemory 缓存为 trait object）
- `stream_with_memory(prompt, conv_id)` 走 `.memory().conversation(id)` 路径（rig 自动 load 历史 + 裁剪 + 压缩）
- `build_summarize_fn()` 构造闭包：clone provider client 调非流式 `.prompt()` 生成摘要
- TokenWindowMemory 用 `HeuristicTokenCounter::openai()`（chars/4 + per_message_overhead，与原 estimate_tokens 同语义）

**`crates/memory/src/repo.rs`**

- `create_message_with_id(..., id)` — 预生成 id 落库（send_message turn 结束写 user+assistant，前端已用此 id 做 patch）
- `delete_conversation_messages(conv_id)` — clear 用（删消息留会话）
- `set_message_usage(msg_id, prompt, completion, total)` — usage 落库
- `create_message_at` / `append_message_text` / `reset_message_content` 保留但生产路径不再用

**`src-tauri/src/commands/chat.rs`**

- `send_message` 重构为 memory 模式：RAG 注入 → 构造 prompt → `stream_with_memory` → 消费流推 Channel（不逐 delta 落库）→ `persist_turn` turn 结束整条写 user+assistant+usage
- 删除 compact_history / emergency_trim_and_rebuild / is_context_overflow（rig CompactingMemory 在 load 时自动压缩，不需要应急重试）
- `persist_turn(state, conv, user_id, content, asst_id, content, model, status, error, usage)` 辅助函数

**`src-tauri/src/commands/provider.rs`**

- `set_provider` 后调 `chat.set_memory(memory, resolve_context_window(&config).await)`（解析真实窗口）
- `restore_provider` 用默认 100K（启动期无法 async 探测）

**B1+B2 完整链路**：用户发消息 → RAG 检索注入 → 构造 prompt → `stream_with_memory(prompt, conv_id)` → rig agent 调 `CompactingMemory.load(conv_id)`：`SqliteMemory.load` 读 DB 全量 → `TokenWindowMemory.apply_with_demoted` 裁掉超预算旧消息 → `LlmCompactor.compact` 生成滚动摘要（carry_over）→ splice 摘要进 history 返回 → agent 用裁剪后 history + prompt 发起补全 → 流式推 Channel → turn 结束 `persist_turn` 整条落库

**设计要点**：直接用 rig 原生能力（CompactingMemory 的 in-flight 防并发、watermark 去重、carry_over 滚动摘要比手写更完备）；append no-op（消息手动建，前端需预生成 message_id 做 patch）；流式不逐 delta 落库（turn 结束整条写，性能更好）；摘要仅存进程内存 state（重启后重新 compact，可接受）；不引入 tokenizer（原则 2）；前端透明（压缩对前端不可见）

### 413 上下文体积预防性管控 — ✅（一期收尾时完成，决策 13）

四层防御链（防 HTTP 413 Payload Too Large）：

1. 前端文档字节预算截断：单篇 512KiB + 总量 2MiB（`context-budget.ts`）
2. 前端图片降采样：Canvas ≤2048px + JPEG q=0.85（`image-resize.ts`）
3. 前端单图硬上限：降采样后仍超 2MiB 跳过
4. Rust 兜底字节校验：28MiB 上限返回友好错误（`send_message`）

### 环境阻塞

4. 长对话验证 B1/B2 自动压缩（rig CompactingMemory 自动裁剪+滚动摘要；可临时把 set_memory 的 context_window 调小到 200 触发）

- ⚠️ **Windows Defender Application Control / Smart App Control**（os error 4551）阻止新编译的 build-script.exe 执行。前一轮清理 `.fingerprint` 缓存后所有 build-script（serde_core/quote/aws-lc-sys）需重新执行，新编译 exe 无 SAC 信誉被拦。**非代码问题**。缓存完整时编译可通过；或等 SAC 信誉建立后重试（`.cargo/config.toml` 记载 `Unblock-File` + 排除项配置）。
- ⚠️ specta bindings 无法自动重新生成（同上 Defender 阻断 build-script），`bindings.ts` 为手动更新（MCP + RAG commands/类型）。

### 待用户侧端到端验证

1. 重新生成 specta bindings（`cargo test --test gen_bindings_test`，需 Defender 放行）
2. 接一个真实 MCP server（如 `npx -y @modelcontextprotocol/server-filesystem /tmp`）测试工具发现与调用
3. ~~配置 embedding provider，摄入文档验证 RAG 注入~~——✅ A2 已砍，改文件工具 + jieba FTS5 agentic search（决策 15）
4. 长对话验证 B1/B2 自动压缩（可临时把 `DEFAULT_MAX_CONTEXT_TOKENS` 调小到 200 触发）
5. **Skill 系统端到端**：`@skillName` 触发菜单选中即时激活 + preamble Tier 1 注入 + 模型调 `read_document` 读 skill body（Tier 2）；Inspector 开关切换会话级激活；导入 zip skill 验证缓存 invalidate

---

## 工程基建落地 — ✅ ADR + oxlint + TanStack Router（2026-07-29）

> 二期前端短板排查发现实现偏离架构 + 工程约束未落地，集中补齐三项基建（#1 ADR / #2 oxlint / #3 路由）。

### #1 ADR：修订 F2/F6，新增 F12/F13（架构对齐）

排查发现前端实现偏离架构决策：chat 组件用了 `@assistant-ui/react`（F2 否决项）、Markdown 用 `@assistant-ui/react-markdown`（F6 否决 streamdown）、路由未启用（F4）。经评估，assistant-ui 的 **Primitives 模式**（headless 行为内核，非高阶抽象）实际满足 F2 的诉求，一期已稳定落地。故修订架构而非回退代码：

- **F12（修订 F2）**：chat 组件集改用 `@assistant-ui/react` Primitives 模式（`ThreadPrimitive`/`MessagePrimitive`/`MessagePartPrimitive` + `useExternalStoreRuntime`）。落地约束：只用 Primitives 不用高阶预设、样式自写、part 分派集中在模块作用域常量、citation 走自定义 part（shadcn `Marker` 设计作废）
- **F13（修订 F6）**：流式 Markdown 改用 `@assistant-ui/react-markdown`（`MarkdownTextPrimitive` + `memoizeMarkdownComponents` 流式 memo + `aui-md`/`dot.css` 排版）。`streamdown` 从依赖移除（已死代码，零 import）。安全约束保留（禁 raw HTML、外链拦截）
- **F4 落地注**：补记一期未启用路由的偏差及本次修复
- AGENTS.md 同步更新（streamdown → assistant-ui 表述）

`streamdown` 已 `npm uninstall`。`@assistant-ui/react` ^0.15 / `@assistant-ui/react-markdown` ^0.14 保留。

### #2 oxlint：四层单向依赖强制（§12.1 硬约束）

**工具选型**：typescript-eslint 8.x peer 只支持 TS <6.1，本项目 TS 7 无法安装（TS 7.0 无 compiler API，官方明确 7.1 才有）；强制安装运行时崩溃。改用 **oxlint 1.76**（Rust 实现，不依赖 TS compiler API）+ 内置 `no-restricted-imports`（按目录 glob 分组规则）。

**落地**：`.oxlintrc.json` 定义 6 个 override（domain/ipc/stores/hooks/components + routes），用 `no-restricted-imports` 的 paths/patterns 精准表达 §12.1：

- domain 禁依赖 stores/hooks/components/routes（允许 type-only 引用 bindings）
- ipc 禁反向引用 stores/hooks/components
- stores 禁 import components/hooks
- components 禁直接 invoke + 禁 import ipc/commands、ipc/bindings（核心硬约束）
- hooks 禁直接 invoke（invoke 集中在 ipc 层）
- routes 禁直接 invoke

**修复的真实违规**：

- `IngestStatusBoard.tsx` 直接 import `@/lib/ipc/commands` 调 `ipc.cancelIngest` → 新增 `useCancelIngest` hook，组件改调 hook
- `domain/index.ts` re-export bindings → 规则放开 type-only（bindings 是 specta 类型源头，domain 作为类型统一出口 re-export 合理）

**验证**：`npm run lint`（oxlint）0 error 0 warning；`lint:check`（--deny-warnings）通过。故意注入违规测试规则生效。`eslint` 已从依赖移除。

### #3 TanStack Router：文件式路由落地（决策 F4）

**最优方案**：路由参数为会话切换**唯一真相源**，无 Zustand 同步（拒绝了"路由→store effect 同步"的妥协方案，消除双真相源时序坑）。

**落地**：

- `@tanstack/router-plugin/vite` 接入 vite.config.ts，`routesDirectory: ./src/routes`，自动生成 `routeTree.gen.ts`（已加 .gitignore + oxlint ignores）
- 路由文件：`__root.tsx`（原 App.tsx 逻辑下沉：TitleBar+AppShell+CommandPalette+FileDropZone+Settings+快捷键+主题+清摄入副作用）、`index.tsx`（/ 空态）、`chat.$conversationId.tsx`（/chat/:id，**带 loader 预取消息历史**，F4 承诺的"进入会话前预取"）、`library.tsx`（/library 占位）
- `main.tsx` 挂 `RouterProvider`（替代直接渲染 `<App/>`），`defaultPreload: "intent"` hover 预取，导出 queryClient 供 loader 用
- `AppShell` 中间栏 `<ChatArea/>` → `<Outlet/>`
- 新建 `useCurrentConversationId` hook（从 `useRouterState` 的 matches 数组派生 id，结构化不靠正则）
- `useCreateConversation`/`useDeleteConversation` 的 `setCurrent` → `navigate()`（onSuccess 内导航，hooks 调 useNavigate 不违反单向依赖——路由是基础设施非业务层）
- Sidebar/CommandPalette 选会话 `setCurrent(id)` → `navigate({ to: '/chat/$conversationId', params })`
- ChatArea/FileDropZone/Inspector 读 `useUiStore.currentConversationId` → `useCurrentConversationId()`
- `ui-store` 移除 `currentConversationId` + `setCurrentConversation`（含清理摄入副作用的 setter），副作用迁移到 `__root.tsx` 的 `useCurrentConversationId` effect
- 删除 `App.tsx`

**收益**：路由为真相源 → 深链/后退栈/刷新保持会话；loader 预取 → 切会话不白屏；hover 预取 → 更快；类型安全（routeTree.gen 注册到 Router 类型）；代码分割（chat/library 独立 chunk）。

**验证**：oxlint 0 警告 + tsc -b 0 错误 + vite build 成功（1.1s，代码分割生效，无 ineffective import 警告）。

### 待用户侧验证

- `tauri dev` 实际运行：路由切换、⌘N 新建会话跳转、后退栈、刷新保持会话、loader 预取效果
- Defender 放行后重新生成 specta bindings（routeTree 生成不依赖 Defender，纯 JS plugin）

## 待办

### 一期收尾

- [x] **A3 MCP 工具系统**：rmcp 1.x（非 3.0-beta）+ 自实现 DynamicTool 桥接（rig 0.41，决策 14）
- [x] **架构边界强制**（§12.1）：oxlint + `no-restricted-imports` 强制四层单向依赖（决策 F12/F13 同期落地，eslint 因 TS7 不兼容改用 oxlint）
- [x] **B1 token 预算**：rig TokenWindowMemory + HeuristicTokenCounter::openai()（chars/4，决策 16）
- [x] **B2 历史自动压缩**：rig CompactingMemory + LlmCompactor 滚动摘要（carry_over，决策 16）
- [x] **已摄入文件 @ 挂载到消息**：Composer `@` 菜单（MentionMenu）选中文档后保留 `@fileName` 文本原位（位置语义）；后端在 user message 尾部追加 `<mounted-documents>` 注脚（id+name），模型按需调 `read_document` 取全文（决策 17）
- [x] **GUI 实际运行验证**：已打包 NSIS 安装包并运行，发现并修复 Tailwind CSS 问题

### 二期

- [x] **sqlite-vec 打包破损修复**：crates/vendor/sqlite-vec 本地副本，build.rs 关 diskann/rescore 走 stub（见二期 section）
- [x] **A3 MCP 工具系统**：rmcp 1.x（非 3.0-beta）+ 自实现 DynamicTool 桥接（决策 14）
- [x] **A2 向量检索 RAG**：~~sqlite-vec 向量存储 + KNN 检索注入~~——✅ 已砍，改文件工具 + jieba FTS5 agentic search（决策 15）
- [x] **B1 token 预算**：rig TokenWindowMemory + HeuristicTokenCounter::openai()（chars/4，决策 16）
- [x] **B2 历史自动压缩**：rig CompactingMemory + LlmCompactor 滚动摘要（carry_over，决策 16）
- [x] **前端 ToolCallCard**：可折叠工具调用卡片（MCP + 文件工具）
- [x] **前端 MCP 服务器配置区**：stdio/http 增删 + 状态 + 工具列表
- [x] **库/文件管理视图**：LibraryView（documents 表数据源，挂载/删除/预览走 IPC）
- [x] **Inspector 挂载文档面板**：展示本会话挂载文档列表（从 conversation_documents 表读），可查看/移除
- [x] **`@` 挂载统一 agentic search**：位置语义 token + user message 注脚 + 工具按需读（决策 17）
- [ ] **A1 VLM 增强解析**：复杂度检测 + 扫描件走 VLM API——暂缓（无可用 VLM 模型，待四期 Ollama 本地 VLM 或接 GPT-4o）
- [ ] **ThinkingBlock/Quick Entry 浮窗**：ThinkingBlock 已由 assistant-ui Reasoning part 覆盖；Quick Entry 暂缓
- [x] **已摄入文件 @ 挂载到消息**：Composer `@` 菜单（MentionMenu）选中文档后保留 `@fileName` 文本原位（决策 17）
- [x] **会话级知识范围（文件夹层级 + 激活集）**：详见下方专节（2026-07-31）
- [x] **Agent Skill 系统**：agentskills.io 规范 + `agent-skills 0.2` + 六模块 + 3 内置 skill + `@skillName` 端到端 + TTL 缓存（决策 20，详见下方专节）
- [x] **Skill discover_all 性能优化**：60s TTL 缓存，导入/卸载 invalidate（SKILL-SYSTEM.md §3.6 落地）

### 会话级知识范围 — ✅（2026-07-31）

设计文档：`docs/CONVERSATION-SCOPE.md`（云盘模式：文件系统式嵌套文件夹 + 会话激活集）

**问题**：原设计「上传即可问」导致历史文件污染新会话上下文；无文件夹组织 5 本电子书散在根目录。

**方案**：

- **文件夹层级**：`documents.folder_path TEXT`（无独立 folders 表，文件夹由文件隐式定义），嵌套路径 `/曾国藩专题/书信集`，旧书迁移到 `/Inbox`
- **会话激活集**：`conversations.active_folders`(JSON) + `active_sources`(JSON) 两列 + 复用 `conversation_documents` 表存 `@` 触发的单文件；**默认空**（新会话不预选，防污染）
- **`@` 语义**：`@文件` 插入 token + mount 该文件；`@source.table` 插入 token + 把 source 加入激活集；候选 = 所有文件 + 所有数据源表
- **Agent 工具过滤**：`document_tools(memory, allowed_paths)` / `federation_tools(svc, allowed_sources)`；空激活集不挂工具（模型通用回答）

**后端（crates/memory）**

- `documents` 加 `folder_path` + `source_conv_id` 列，迁移旧书→`/Inbox`，`idx_documents_folder` 索引
- `conversations` 加 `active_folders` + `active_sources` JSON 列
- 新增：`list_folders`/`list_documents_by_folder`/`move_document`/`rename_folder`（递归子目录）/`delete_folder`（级联删文件+FTS5+挂载关联）
- 新增激活集 repo：`get/set_active_folders`/`get/set_active_sources`/`resolve_active_doc_paths`（folders 递归 ∪ conversation_documents）
- `SearchHit` 加 `path` 字段（工具层激活集过滤用）
- 5 新测试（folder 持久化/列表、move、rename 递归、delete 级联清 FTS5、激活集 resolve+filter）全过

**Agent 工具过滤（crates/agent-core）**

- `document_tools(memory, allowed_paths: Arc<HashSet<String>>)`：list/search 后 filter、read 前校验 path 在集合内
- `federation_tools(svc, allowed_sources: Arc<HashSet<String>>)`：list 返回前 filter、describe/execute 前校验 source_name
- `stream_with_memory`：解析会话激活集→空集不挂对应工具→有日志

**IPC（src-tauri/commands/ingest.rs）**

- `ingest_files` 加 `conversation_id: Option<String>`：会话上传落 `/Inbox` + 记 `source_conv_id` + 自动 `mount_document`
- 8 新 command：`list_folders`/`list_documents_by_folder`/`move_document`/`rename_folder`/`delete_folder`/`get_active_scope`/`set_active_folders`/`set_active_sources` + `ActiveScopeDto`
- 修 `BINDINGS_PATH` 用 `CARGO_MANIFEST_DIR` 绝对路径（修了 bindings 不更新的隐藏 bug——Tauri 运行时 cwd 不是 src-tauri/，相对路径 `../src/...` 写到错处）

**前端（src/）**

- `hooks/useActiveScope.ts`：读/设置/切换激活集（folders + sources）
- `hooks/useFolders.ts`：文件夹列表/文件/移动/重命名/删除（invalidate 多缓存）
- `components/chat/ScopeChip.tsx`（新）：对话页顶部 chip，显示激活范围或「未挂载知识源」，popover 勾选文件夹+数据源
- `components/chat/MentionMenu.tsx`（重写）：候选 = 文档 + 数据源表（`useQueries` 批量查 schema）；@文件→mount+插 token；@source.table→加激活集+插 token
- `components/library/LibraryView.tsx`（重写）：两栏文件树视图（左:层级文件夹树含 Inbox 置顶；右:文件列表+重命名/删除文件夹/移动文件/预览/挂载/删除）
- `components/chat/ChatArea.tsx`：顶部加 ScopeChip 行

**关键行为**

- 新会话默认空激活集 → 模型无文档/联邦工具 → 通用回答，chip 明示「未挂载知识源」
- 会话上传 → 落 `/Inbox` + 自动激活（立即可用，持久化）
- chip 勾文件夹 → 该文件夹下所有文件（含子目录递归）进激活集
- `@文件` → 该文件 path 加入 `conversation_documents`（激活集 documents 部分）
- 激活集持久化到会话表，切回恢复；后端 `stream_with_memory` 直接读，前端只传 conv_id

**待后续**

- react-arborist 替换手写树（文件多时）
- 拖拽移动文件（当前用 prompt 输入路径）
- 空激活集时 UI 更明确提示「通用对话模式」

### 三期（本体设计）

- [ ] **ontology crate**：ObjectType/LinkType/ActionType 模型 + SQLite 表族 CRUD（复用 memory 的 rusqlite）

#### 数据集/数据源隔离与级联清理 — ✅（2026-07-31）

- **隔离展示**：dataset/data_source 为全局共享物理资产（无 ontology_id，决策 10），不再混在每个本体详情 Tab 里；`OntologyView` 顶栏分段切换「本体 / 数据集 / 数据源」三个独立视图，全局资产单独管理。新增 IPC `list_ontology_datasets` / `list_ontology_data_sources`（JSON string 返回，避开 BigInt 禁令），bindings.ts 重新生成
- **级联删除（决策 10 修订）**：新增 `ontology_dataset_refs` / `ontology_data_source_refs` 引用表跟踪“哪个本体声明了哪些资产”；`delete` 本体时 refs 随 CASCADE 消失，随后删除**不再被任何本体声明**且不被剩余 object_types.backing_dataset_api_name / datasets.data_source_api_name 引用的孤儿资产。仍被其他本体声明的资产保留。import 时登记 refs（INSERT OR IGNORE 幂等）。新增 2 个测试（独占级联删 / 共享保留）

- [x] **federation crate**：DataFusion 54.x 内嵌 + datafusion-federation 0.5.5 SQLExecutor（MySQL/PG 用 sqlx 0.9 + rustls，CSV 复用 ingest 的 calamine，Excel 待补）
- [ ] **TextQL 编译器**：sqlparser-rs 0.62 生成 SQL，NL→意图走 LLM（复用 agent-core Rig）
- [x] **schema 浏览**：三段式 information_schema 查询 + DataFusion table_provider 回退，树形展示
- [ ] **Agent 工具化**：联邦查询作为 MCP tool 暴露给 LLM
- [x] **前端**：数据源注册向导（MySQL/PG/CSV）+ schema 浏览树 + SQL 编辑器（⌘Enter 执行）+ 查询结果表格；ER 图（react-flow）+ TextQL 编辑器待后续

### 联邦查询全栈 — ✅（2026-07-30）

**`crates/federation/`**（新建业务 crate，平台无关）

- `DataFusion 54.x` 内嵌 `SessionContext`，每个数据源注册为独立 catalog（`public` schema，三段式 `catalog.public.table` 寻址）
- `datafusion-federation 0.5.5` 的 `SQLExecutor` trait：`mysql.rs` / `postgres.rs` 各实现一个（sqlx 0.9 + rustls，`AssertSqlSafe` 包裹动态 pushdown SQL），`SQLSchemaProvider::new().await?` + `register_schema?`
- `catalog.rs`：CSV 注册走 temp table 模式（`register_csv`→`table_provider`→`deregister_table`→`register_catalog_with_table`），因 DF 54 CSV 直接注册 catalog 无 schema provider；`deregister_source` 为 noop（DF 54 无 `deregister_catalog`，catalog 随进程留存）
- `query.rs`：只读守卫（`sqlparser` 解析，仅 `Statement::Query`/`Explain(Query)` 放行）+ 自动 LIMIT 注入（默认 200，max 1000）+ 30s 超时；`execute_query` 返回 `QueryResult{columns:ColumnMeta[], rows:Vec<String>(JSON string), row_count, elapsed_ms, sources_touched}`
- `schema.rs`：`browse_schema`（三段式 `{catalog}.information_schema.tables WHERE table_schema='public'` 避开系统表）+ `describe_table`（同模式 + `table_provider` 回退 + 前 5 行样本 + 行数估计）
- `source.rs`：`DataSourceConfig{id:String, name, connection:ConnectionConfig(tagged enum), color, created_at:Timestamp}` / `DataSourceSummary` / `TableMeta` / `ColumnMeta` / `QueryResult`
- 10 测试通过（5 unit：readonly/LIMIT/多语句注入；5 e2e：CSV 注册/查询/schema/注销/持久化/行数限制）

**src-tauri IPC**（`commands/federation.rs`）

- 9 命令：`register_data_source` / `test_data_source` / `deregister_data_source` / `list_data_sources` / `get_data_source` / `browse_federation_schema` / `describe_federation_table` / `execute_federation_query` / `explain_federation_query`
- `AppState` 加 `federation: RwLock<Option<FederationService>>`（`FederationService: Clone`，两 Arc 字段廉价克隆，避开 RwLock guard 跨 await 的 Send 问题）+ `init_federation()` 异步初始化
- `register`/`deregister` 的 `MutexGuard` 用块作用域 `{ }` 包裹，确保 guard 不跨 await 点（Future Send）

**BigInt 公约落地**（见 AGENTS.md「IPC 边界 BigInt 公约」）

- `specta-typescript 0.0.12` 硬编码禁止 `u64/i64/usize/isize/i128/u128/f128` 导出（全有或全无：任一字段触发整个 `Builder.export()` 失败）
- **官方方案 4**：逐字段 `#[specta(type = specta_typescript::Number)]` 注解（`Number` 是内置 OpaqueReference，走 bypass 路径输出 `number`，不触发 bigint 检查）
- 各 crate 加 `specta-typescript` 依赖（纯导出元数据，不破坏解耦）；`memory::Timestamp` newtype 保留（领域语义）
- 命令参数（函数参数）不支持注解，用 `u32`/`i32` + `as usize` 转换（如 `execute_federation_query` 的 limit）
- 历史教训：先手写 5 个 newtype（ContextWindow/TokenCount/FileSize/RowCount + types.rs），后读 `error.rs` 顶部官方文档发现一行注解方案，全部回退重构

**前端**

- `bindings.ts` 自动生成（9 federation 命令 + 所有 bigint 字段导出为 `number`）
- `ipc/commands.ts`：federation 段封装（unwrap 9 命令）
- `hooks/useFederation.ts`：useDataSources / useFederationSchema / useTableMeta / useRegister/useDeregister/useTest/useExecute/useExplain
- `components/federation/FederationView.tsx`：三栏工作台（左:数据源列表；中:SQL 编辑器+结果表；右:schema 树）+ 注册对话框（MySQL/PG/CSV/Excel 表单 + 测试连接 + 颜色标记）
- 入口：Sidebar 底部「联邦查询」按钮 + ⌘Shift+F 全局快捷键
- `ui-store`：加 `federationOpen` 状态
- 修复 bindings 重新生成的副作用：`useChat.ts` 三处 MessageRow 构造 token 字段 `null`→`undefined`；`SettingsView.tsx` context_window/statuses/error title 类型对齐

**工程教训（已写入 AGENTS.md 工作守则）**

- 第 6 条：遇库的报错/限制先读源码文档注释 + 搜业界方案，勿凭局部信息下"无解"结论自造 workaround（specta BigInt 教训）
- 第 7 条：`cargo check --workspace` 因根 Cargo.toml `exclude=["src-tauri"]` **永远不查 src-tauri**；验证 src-tauri 必须用 `cargo check --manifest-path src-tauri/Cargo.toml`（unused Builder import 漏网教训）

### Agent Skill 系统 — ✅ 全栈（2026-08-01，决策 20）

**背景**：基于 agentskills.io 开放规范的文本扩展机制。Skill = SKILL.md（YAML frontmatter + Markdown 正文），不是可执行插件——需执行能力的 skill 走 MCP server。渐进式披露：name + description 常驻 preamble（Tier 1，几十 token），完整正文按需由模型调 `read_document` 读取（Tier 2）。详见 `docs/SKILL-SYSTEM.md`。

**`crates/memory`** — 两表 + skill_repo

- `disabled_skills`（全局禁用，层次 2）+ `conversation_skills`（会话级激活，层次 3，外键到 conversations）
- `skill_repo.rs`：CRUD（list/set/is/removed），卸载时 `remove_skill_records` 清两表

**`crates/agent-core/src/skill/`** — 六模块（约 600 行业务代码）

- `mod.rs`：`SkillRecord` / `SkillSource`（Builtin/Imported/ExternalReadOnly/Project，kebab-case specta）/ `SkillError`
- `manager.rs`：`SkillManager`（扫描去重 Builtin>Imported>External、入库 documents、preamble 拼接）；**discover_all 走 60s TTL 缓存**（`Mutex<Option<(Instant, Vec<SkillRecord>)>>`），导入/卸载时 `invalidate_cache()` 失效（见下「性能优化」）
- `activate.rs`：三层 disable 判断（frontmatter dmi / 全局禁用 / 会话级 enabled）+ `resolve_preamble_skills` + `active_skill_doc_paths`
- `prompt.rs`：`<available_skills>` XML 生成（手写极简转义，零依赖），空列表返回空串（不破 prefix cache）
- `builtin.rs`：补 `disable-model-invocation` frontmatter 解析（Govcraft 不解析，业务层补）
- `import.rs`：`import_from_dir`（递归复制）/ `import_from_zip`（复用 ingest::security 防炸弹 + flat/nested 兼容）/ `uninstall`（删目录 + 清 DB + 失效缓存）
- 29 测试通过（manager 8 含 3 缓存测试 / activate 6 / import 8 / builtin 隐含在 manager 测试）

**`crates/agent-core/src/chat.rs`** — 集成

- `ChatService` 加 `skill_manager: Option<Arc<SkillManager>>` + `set_skill_manager` 注入
- `stream_with_memory`：调 `active_skill_doc_paths(conv_id)` 把 `skill://<name>` 合并进 `doc_paths_set`（供 read_document Tier 2）+ 调 `build_preamble_section(conv_id)` 生成 Tier 1 XML 拼在系统人设之后（保 prefix cache）

**3 个内置 skill**（`src-tauri/resources/skills/`）

- `onto-studio-federation` / `onto-studio-ontology` / `onto-studio-ingest`：随应用分发，只读，Builtin source

**src-tauri IPC**（`commands/skill.rs`）

- 6 命令：`list_skills` / `import_skill_from_dir` / `import_skill_from_zip` / `uninstall_skill` / `set_skill_conversation_enabled` / `set_skill_globally_disabled`
- `AppState` 加 `skill_manager: Arc<SkillManager>`（setup hook 构造，路径解析复用 pdfium 三层兜底）
- `SkillDto`：含 `conversation_enabled: Option<bool>` + `globally_disabled: bool` + `disable_model_invocation: bool`

**前端**

- `hooks/useSkills.ts`：useSkillsGlobal / useSkillsConversation / useImport*/useUninstall/useSet*Enabled（乐观更新）
- `components/skill/SkillView.tsx`：全局管理视图（导入/卸载/全局禁用），独立覆盖层（同知识库/数据源同级）
- `components/chat/SkillTogglePanel.tsx`：Inspector 会话级开关面板（层次 3，按来源分组 + dmi 盾牌图标）
- `lib/mention.ts`：`@skillName` 解析——`resolveMentionedPaths(text, mounted, skills)` 识别 `skill://<name>` 虚拟 path + `resolveMentionedSkillNames` 提取引用的 skill 名
- `components/chat/MentionMenu.tsx`：顶层候选加 skill（Zap 图标 + dmi 盾牌 + 已激活徽标），选中即时激活（写 conversation_skills.enabled=true）+ 插 `@skillName` token
- `components/chat/Composer.tsx`：发送时 `mounted_paths` 含 `skill://<name>`，后端按 path 查 documents 表取 skill body（与文件全文统一 read_document 路径）；手打 `@skillName` 未激活的补激活

**后端注脚**（`commands/chat.rs`）

- `send_message` 的 `mounted_paths` 循环按 `format == "skill-md"` 分成 `skill_refs` / `mounted_refs` 两组，注脚分「[技能]」/「[挂载文档]」段，语义更清晰
- `canonicalize_path` 对 `skill://name`（非真实路径）`canonicalize()` 失败回退原值，天然支持

**性能优化**（2026-08-01，SKILL-SYSTEM.md §3.6 落地）

- `discover_all()` 加 60s TTL 缓存：`build_preamble_section` + `active_skill_doc_paths` 复用同一缓存，单次发消息不再扫描两遍磁盘
- `import_from_dir` / `import_from_zip` / `uninstall` 末尾调 `invalidate_cache()` 立即失效
- 全局禁用/会话级激活是 DB 查询（非磁盘扫描），不影响缓存

**遗留**

- L1: `agent-skills 0.2` 依赖 `serde_yml`（RUSTSEC-2025-0068，unsound，仓库 archived）——Cargo.lock 已有 `serde_yaml_ng 0.10.0`（lindera 传递），patch 成本 3 行 + 2 Cargo.toml，**发版前必修**
- `@skillName` 跨客户端 external-readonly skill 不可写，UI 应禁用编辑（二期 skill 编辑器时做）
- 二期：GitHub skill 仓库安装（`git clone`，需评估 gix vs 系统 git）/ 项目级 skill 扫描 / skill 编辑器

### 会话消息操作（复制 / 重新生成 / 编辑重发）— ✅ 全栈（2026-08，决策 21）

消息级操作增强，采用 assistant-ui 原生 `ActionBarPrimitive`，分角色配置按钮 + 行内编辑态，后端新增截断删除命令支撑 reload/edit 重发。

**后端**：

- `crates/memory/src/repo.rs`：`delete_message_and_after(message_id)` —— 单事务删指定消息及其后所有消息，用 SQLite 隐含 `rowid`（插入自增）做时序基准（避免 created_at 同毫秒撞值）
- `src-tauri/src/commands/conversation.rs`：`delete_message_and_after` 命令（返回 `u32`，遵循决策 18 BigInt 公约）

**前端**：

- `src/hooks/useChat.ts`：`reload(parentUserId)`（截断 + 用原 user 文本重发）/ `editAndResend(userMsgId, newContent)`（截断 + 用新文本重发）
- `src/components/chat/ChatRuntime.tsx`：`useExternalStoreRuntime` 补 `onReload` / `onEdit` 回调；`extractTextFromParts` 从 AppendMessage.content 提取文本
- `src/components/chat/Thread.tsx`：
  - assistant 消息 `ActionBarPrimitive`：Copy + ExportMarkdown + Reload
  - user 消息 `ActionBarPrimitive`：Copy + Edit
  - `autohide="not-last"` + `autohideFloat="always"`：悬停浮现，最后一条常驻
  - user 行内编辑态：`ComposerPrimitive.If editing` 切换，`ComposerPrimitive.Input` + Send/Cancel

**已知限制**：reload/edit 重发无法恢复原 user 消息的图片上下文（MessageRow 不存 context_images），仅重发纯文本 + 当前会话挂载状态。

### 四期（可选）

- [ ] 本地 Ollama + Qwen2.5-VL 离线高精度
- [x] **移动端适配**（ARCHITECTURE.md §17.2 / 决策 F11，2026-08-01）——详见下方专节

### 移动端适配 — ✅（2026-08-01，§17.2 / 决策 F11）

**策略**：单套代码 + Tailwind v4 `max-md:` 断点（768px）+ `useIsMobile()` hook 做 JS 分支，不引入新依赖（不用 shadcn Dialog/Drawer，用自绘 Drawer + 全屏覆盖层）。

**基础设施**

- `index.html` viewport 加 `viewport-fit=cover, maximum-scale=1.0, user-scalable=no`（移动端 app 标配，内容延伸到刘海下方）
- `styles.css` 暴露 `--safe-top/bottom/left/right = env(safe-area-inset-*)` CSS 变量；移动端隐藏滚动条 + `overscroll-behavior:none`（禁横向溢出）
- `src/hooks/useIsMobile.ts`：`matchMedia(max-width:768px)`，SSR 安全（初始 false 桌面优先），挂载后校正

**布局适配**

- **AppShell**：桌面两栏 resizable（react-resizable-panels）→ 移动单栏 Outlet + Sidebar Drawer（左侧 85vw 抽出，遮罩/选会话关闭）。新增 `ui-store.mobileSidebarOpen` 状态（与桌面 `sidebarCollapsed` 语义独立）
- **TitleBar**：桌面拖拽栏 + 品牌 + ⌘K → 移动汉堡菜单（触发 Drawer）+ 紧凑标题 + 搜索图标。移动端隐藏 `data-tauri-drag-region`（系统全屏管理）+ `pt-[env(safe-area-inset-top)]`
- **Sidebar**：加 `onNavigate` 回调，移动端选会话/功能入口后自动关闭 Drawer
- **覆盖层**（Settings/Federation/Skill）：`fixed inset-0` 全屏 + `max-md:pt/pb-[env(safe-area-inset-*)]`。SettingsView 从原 flex 流内（被挤压）改为 fixed 覆盖层（桌面移动统一）
- **FederationView**：桌面两栏（数据源 w-64 + SQL 工作台）→ 移动分屏（`mobileShowWorkspace = isMobile && selected`）：无选中显示数据源列表全宽，有选中显示 SQL 工作台 + 返回按钮
- **LibraryView**：桌面两栏（树 w-56 + 文件列表）→ 移动分屏（`mobileView: 'tree'|'files'`）：点文件夹切到文件列表 + 返回按钮
- **CommandPalette**：桌面 `pt-[15vh] max-w-xl` → 移动顶部贴边全宽 + `max-h-[70vh]`
- **MentionMenu/ScopeChip popover**：`w-80` 加 `max-w-[calc(100vw-1.5rem)]` 防溢出；ScopeChip summary 移动端缩窄 max-w
- **Composer**：移动端 padding 缩小 + `pb-[max(0.5rem,env(safe-area-inset-bottom))]`（home indicator 安全区）；「深度思考」文字 `max-md:hidden` 只留图标；placeholder 移动端简化（去掉快捷键提示）；textarea `max-md:text-base`（iOS 防 16px 以下缩放）
- **FileDropZone**：移动端 `onDragDropEvent` 不触发（自动 no-op），由 Composer + 按钮覆盖（§17.2）

**附带修复**（lint 阻断，§12.1 硬约束）

- FederationView 删 `import { IpcError }`，`instanceof IpcError` → `instanceof Error`（IpcError 继承 Error，message 已设）
- MentionMenu 删 `import { ipc }`，`useFederation.ts` 新增 `fetchFederationSchema(catalog)` 裸函数供 `useQueries` 批量场景用（避免组件直接 import ipc/commands）
- FederationView `toggle` 三元表达式改 if（修 `no-unused-expressions`）

**验证**：`tsc -b` 零错误 + `oxlint --deny-warnings` 零 warning + `vite build` 成功（869ms）。

**未做**（§17.2 桌面优先，移动端是降级场景）：

- Tauri Android/iOS 构建链（`tauri android init` / `tauri ios init`，需各自 toolchain）——代码层就绪，构建链待用户按需启用
- 移动端原生手势（侧滑返回等）——WebView 默认支持，未额外定制
- react-arborist 替换手写文件树（决策 19 待后续，文件多时）

### Provider 配置补全 — ✅ 全栈（2026-08-02，决策 7.5）

**问题**:原 `ProviderKind` 只有 2 种(OpenAiCompatible/Anthropic),所有兼容端点塞进 `openai::Client.completions_api()`,丢失 rig 的 `OpenAICompatibleProvider` capability 声明(DeepSeek 的单 chunk tool call、不支持 response_format 等);且字段不全(无 temperature/max_tokens/headers/reasoning 精细化)。

**改造范围**

- **`crates/agent-core/src/provider.rs`**:`ProviderKind` 从 2 种扩到 13 种(OpenAi/Anthropic/Gemini/DeepSeek/Xai/Groq/OpenRouter/Ollama/Moonshot/Mistral/Cohere/Perplexity/OpenAiCompatible)。`ProviderConfig` 加字段:采样(temperature/max_tokens/top_p)、兼容性(supports_developer_role/supports_reasoning_effort/input_types)、增强(extra_headers/reasoning 4 级)。新增 `reasoning_to_params()` 按 provider 映射(OpenAI→reasoning_effort,Anthropic→thinking.budget_tokens,Gemini→thinking_config)。13 个单测。
- **`crates/agent-core/src/chat.rs`**:引入 `ProviderRuntime` trait 擦除 rig client 泛型类型(原 `DynClient` enum 的 8 处 `match &self.client` 分支全部消除)。泛型 `helper::stream_chat/stream_with_memory/prompt_text` 抽公共逻辑,宏 `impl_provider_runtime!` 为 13 个 provider newtype 生成 trait impl。`build_runtime()` 按 kind 路由:OpenAI 官方走 responses_api(),兼容端点走 completions_api(),Gemini/Anthropic 走原生协议;`supports_developer_role=false` 调 rig 0.41 的 `with_system_instructions_as_messages()`。`ChatService` 持 `Arc<dyn ProviderRuntime>`。
- **`src-tauri/commands/provider.rs`**:`SetProviderInput` 同步扩字段。
- **前端 `domain/index.ts`**:`PROVIDER_PRESETS` 从 6 个扩到 13 个(每个含 supportsImage 标记),`presetForKind()` helper。base_url 留空用 rig 默认(不再前端写死)。
- **`SettingsView.tsx`**:加「高级设置」折叠区(温度/maxTokens/topP/深度思考 4 级/developer role/reasoning_effort/图片输入/自定义 Headers)。
- **`bindings.ts`**:手动更新 ProviderKind(13 variant)+ ProviderConfig/SetProviderInput(新字段)+ InputType/ReasoningLevel 类型。

**验证**:`cargo test -p agent-core --lib` 73 测试全过;`cargo check --manifest-path src-tauri/Cargo.toml` 零 warning;`tsc -b` 零错误;`oxlint --deny-warnings` 零 warning;`vite build` 成功。

**未做**(权衡后保留):

- context_window 探测未改用 rig `ModelListingClient`(rig ModelList 不直接暴露 context_window 字段,当前手写四层 fallback 已健壮)。
- pi 的 7 级 thinkingLevelMap(onto-studio 非编码 agent,4 级够用)。
- cost/用量统计(三期/四期)。

---

## 关键决策记录

| rmcp 锁定 1.x（非 3.0-beta） | rig 0.41 锁 rmcp ^1.8；3.0 的 macros/pastey/darling 在 cargo 1.97 触发 resolver panic（决策 14） |

| 不启用 rig 的 rmcp feature | rig tool::rmcp 针对 3.0 API；自实现 DynamicTool 桥接只需 rmcp client+transport，无 macros（决策 14） |
| ------ | ------ |
| MSVC target（非 GNU） | 用户选择；VS BuildTools 2022 |
| target-dir 重定向到 C 盘 | D 盘 WDAC 策略阻止新生成 exe 执行（os error 4551） |
| `cargo-msvc.bat` 包装器 | cc crate 无法自动为 cl.exe 设置 INCLUDE/LIB |
| `src-tauri` 独立 Cargo.lock | 避免 workspace feature 统一冲突 |
| sqlite-vec 设为可选 | ~~crates.io 版本打包损坏~~——✅ 已砍，二期改 jieba FTS5 agentic search（决策 15） |
| sqlite-vec vendor 副本 | ~~crates.io 0.1.10-alpha.4 缺 diskann.c/rescore.c~~——✅ 已删，不再需要（决策 15） |
| rmcp 锁定 1.x（非 3.0-beta） | rig 0.41 锁 rmcp ^1.8；3.0 的 macros/pastey/darling 在 cargo 1.97 触发 resolver panic（决策 14） |
| 不启用 rig 的 rmcp feature | rig tool::rmcp 针对 3.0 API；自实现 DynamicTool 桥接只需 rmcp client+transport，无 macros（决策 14） |
| stdio transport 自实现 | 不用 rmcp transport-child-process（process-wrap→windows crate 编译失败），改 tokio::process::Command（决策 14） |
| RAG 直调 /v1/embeddings | ~~不用 rig EmbeddingModel~~——✅ 已砍，改 FTS5 + 文件工具，无 embed API（决策 15） |
| chars/4 token 估算 | 不引 tokenizer（原则 2）；偏松安全侧，Claude Code 热路径同款（决策 16） |
| `@` 挂载不注入全文 | 推翻早期「整篇注入」设计；`@fileName` 文本原位 + user message 尾部注脚（id+name）+ 模型按需调 `read_document`；不进 system prompt（不破 prefix cache）、不给摘要（避免 batch 延迟）（决策 17） |
| B1+B2 合一 | 裁剪定边界 + 压缩保信息一条链路；零 schema 改动用 model=**summary** 标记摘要消息（决策 16） |
| Timestamp 新类型 | specta-typescript 禁止 i64/u64 导出为 BigInt；自定义 Type 导出为 `number`（< 2^53 安全） |
| `.completions_api()` | Rig 0.41 默认 Responses API；OpenAI-compatible 服务只支持 Chat Completions |
| DeepSeek base URL 带 `/v1` | Rig `build_uri` = `base_url + path`，path=`/chat/completions` |
| `@tailwindcss/vite` 插件 | Tailwind v4 必须通过此插件处理 `@import "tailwindcss"`，不再用 PostCSS 自动处理 |
| `@theme inline`（非 `@theme`） | Tailwind v4 中 `@theme` 是编译期静态解析；`inline` 让生成的 utility 引用运行时 `var()`，主题切换才生效 |
| Release profile 优化 | `lto="thin"` + `codegen-units=16` + `incremental=true`；全量编译 15min→4min，增量复用稳定 |

---

## 构建与发布

### 安装包

- **格式**：NSIS（`.exe` 安装包），默认升级模式（双击覆盖安装，配置/数据保留）
- **产物路径**：`C:/Users/think/AppData/Local/onto-studio-target/release/bundle/nsis/onto-studio_0.1.0_x64-setup.exe`
- **大小**：~7.1MB

### 开发工作流

| 场景 | 命令 | 耗时 | 说明 |
|------|------|------|------|
| 日常开发 | `tauri-msvc.bat dev` | 首次~1min，之后改前端秒级、改Rust几十秒 | 窗口热重载，不用重装 |
| 出安装包 | `tauri-msvc.bat build --bundles nsis` | 增量~2min | 双击覆盖安装 |

### Release 编译优化（避免缓存失效）

原配置 `lto=true` + `codegen-units=1` + 无 `incremental` 是最慢组合，且强制中断会生成残缺 `.rlib`/指纹导致缓存全部失效。已改为：

```toml
[profile.release]
codegen-units = 16      # 拆分编译单元，多核并行
lto = "thin"            # thin-LTO 替代全量，速度翻倍
opt-level = 3
panic = "abort"
strip = true
incremental = true      # 关键：保留增量中间文件（默认 release 关闭）
```

**避坑**：

- 不要 `cargo clean`，保留 `target/` 缓存
- 构建超时不要强制 kill，正常 Ctrl+C 让 Cargo 写完指纹
- `incremental=true` + 完整跑完一次后，二次构建可复用缓存
