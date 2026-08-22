# AGENTS.md — onto-studio 项目指令

> 本文件是给 AI coding agent 的项目级约束，继承自 `ARCHITECTURE.md`。所有改码、选库、写测试前必读。
> 架构详情、ADR 理由、依赖版本以 `ARCHITECTURE.md` 为准；本文件是可执行摘要。

## 项目定位

基于 **Tauri 2 + Rust** 的跨端（桌面优先，移动端适配）知识工作台。三大核心能力：

1. **Agent core**：Rig 驱动的 agent loop + MCP 工具系统 + 多模态理解 + 文件检索工具
2. **多模态文件读写**：覆盖 PDF / Office / eBook / 文本 / 压缩包 / 图片的统一摄取管道
3. **本体设计 + 联邦查询**：数据源注册（MySQL/PG/CSV/Excel）→ 本体建模（ObjectType/LinkType/ActionType）→ DataFusion 联邦查询 → TextQL 自然语言转 SQL（三期）

## 五大核心原则（贯穿所有决策，优先级最高）

### 原则 1：Rust 纯栈，运行时零额外原生依赖

**目标机器零安装**——用户拿到二进制即可运行，无需装任何系统库/运行时/办公软件。

- 所有原生代码（SQLite）随 crate 源码**静态编译**进二进制
- 除 Tauri WebView（平台自带）和构建期 C 编译器（仅 SQLite 系）外，运行时无任何系统依赖
- **绝对否决**：任何依赖 LibreOffice / FFmpeg / Tesseract / Poppler / mupdf / OpenSSL / 独立向量库的方案
- **PDFium 破例**（见 `ARCHITECTURE.md` 决策 5）：纯 Rust（lopdf/pdfsink）对中文 CID CMap 解码失败，改用 pdfium-render + 预编译 PDFium 动态库（随 Tauri 资源打包、运行时由程序加载、用户零安装）。这是纯 Rust 文本提取能力不足的工程妥协，仅限 PDF 解析，不开放其他否决项
- bundled SQLite（cc 编译 C 源码）可接受，因为它静态链接、用户无感；但需系统安装的独立应用一律否决

### 原则 2：轻量化

- 不内嵌本地模型权重（OCR / Whisper / VLM 一律不走本地重型模型）
- 本地推理能力通过用户自配（Ollama 等）接入，应用本身保持小体积
- 重型能力（多模态理解）走 API，按需调用

### 原则 3：许可证友好

全部 MIT / Apache-2.0 / Unlicense / BSD-3，**排除 GPL**。引入任何新 crate 前先核实许可证。（PDFium 库本身为 BSD-3-Clause，属宽松许可，不冲突。）

### 原则 4：业务核心与平台解耦

- 业务逻辑全部在 `crates/`（平台无关），可独立 `cargo test`
- Tauri 只做 IPC 薄层 + 平台能力
- `DocumentParser` trait 抽象底层解析库，可平滑替换

### 原则 5：外部服务最小化且显式

仅两类外部依赖（按定义不可避免，且显式可知）：

- **模型 API**（云端 LLM/VLM，或用户自配本地 Ollama）
- **MCP server**（用户按需接入的工具服务）

## 工程结构（硬约束）

```
onto-studio/
├── src-tauri/        # Tauri 壳：IPC 薄层 + 平台能力，不含业务逻辑
├── crates/           # 全 Rust 业务核心，平台无关，可独立 cargo test
│   ├── agent-core/   # Rig + MCP + provider + 多模态
│   ├── ingest/       # 多模态摄取管道 + VLM 增强
│   ├── memory/       # SQLite + jieba FTS5（会话/消息/文档全文/元数据同一 .db）
│   ├── agent-core/   # Rig + MCP + provider + 多模态 + 文件检索工具
│   ├── ontology/     # 三期：本体建模（ObjectType/LinkType/ActionType）+ TextQL 编译器
│   └── federation/   # 三期：DataFusion 联邦查询 + TableProvider（MySQL/PG/CSV/Excel）
├── src/              # React 前端
└── package.json
```

- 业务逻辑禁止放进 `src-tauri/`；`src-tauri/` 只做 `#[tauri::command]` 薄封装
- `crates/` 之间可互相依赖，但不得反向依赖 `src-tauri/` 或 `src/`

## 关键技术决策（不可擅自偏离）

| 领域 | 决策 | 禁止项 |
|---|---|---|
| Agent 框架 | `rig = "0.41"`（启用 `agent`+`memory`+`rustls`+`derive`+`reqwest` feature）+ `rig-memory = "0.41"`，不用 loaders | pi_agent_rust、官方 pi-agent、LangChain-rust |
| MCP 工具 | `rmcp ^1.8`（锁定与 rig 0.41 一致，不用 3.0-beta）+ 自实现 `DynamicTool` 桥接（rig 0.41 取消 `ToolDyn` trait，改用 `DynamicTool::new(name,desc,params,callback)`），不启用 rig 的 rmcp feature；stdio 用 tokio::process 自实现 transport | rig 官方 tool::rmcp（rmcp 3.0 API + macros/darling resolver bug）、rmcp transport-child-process（process-wrap/windows crate 编译失败） |
| 文件检索 | `crates/agent-core/document_tools.rs` 三个 DynamicTool（list/search/read_documents）；ingest 全文存 memory SQLite（`documents` 表 + `documents_fts` FTS5 虚拟表，jieba 分词 + BM25 + snippet）；agent 工具按需检索（agentic search），不自动注入切片 | 向量 RAG（sqlite-vec KNN + embedding，留四期）、rig EmbeddingModel、本地 embedding 模型（原则 2）、独立向量库 |
| `@` 挂载 + prompt cache | `@fileName` 文本原位保留（位置语义）；后端查 id+name 在 **user message 尾部**追加 `<mounted-documents>` 注脚（不进 system prompt，不破坏 prefix cache）；模型按需调 `read_document(id)` 取全文（与工具检索统一路径）；不注入全文/摘要。详见 ARCHITECTURE.md 决策 17 | `@` 挂载走全文注入（撑爆 context、每轮重发、与工具检索割裂，已推翻）、system prompt 塞文件清单/摘要（破坏 prefix cache）、预生成摘要（batch 延迟高）、强制 `[n]` Citation（agentic search 下无编号） |
| Agent Skill | `agent-skills = "0.2"`（SKILL.md frontmatter + body）+ `crates/agent-core/skill/`（三层 disable + 四类来源 + 渐进式披露 Tier1/2/3）；skill body 入库 documents 表走 `skill://<name>` doc path（与 `@` 挂载统一 read_document 路径）；preamble 段拼在系统人设之后（保 prefix cache）。详见决策 20 | 自造 SKILL.md 解析器、skill 全文注入 preamble（撑爆 context）、skill 段进 system prompt 前部（破 prefix cache）、独立向量库存 skill |
| 上下文预算+压缩 | rig 0.41 原生 `ConversationMemory` + `CompactingMemory`：`crates/agent-core/memory_bridge.rs` 把 `Memory`(SQLite) 适配为 `ConversationMemory`（load=list_messages，append=no-op，消息由 send_message 手动建），挂 `TokenWindowMemory` policy（`HeuristicTokenCounter::openai()`=chars/4）+ 自实现 `LlmCompactor`（调同 provider 生成滚动摘要，carry_over）；agent 构造 `.memory(CompactingMemory).conversation(id)`，load 时自动裁剪+压缩，不手写 trim/compact | 手写 trim_history/compact_history（已废弃）、tiktoken-rs（原则 2）、摘要落 DB（摘要仅存进程内存 state） |
| 存储 | `rusqlite`(bundled) + 自实现分句版 jieba FTS5 tokenizer（jieba-rs + rusqlite-ext，分句避免 DAG O(n²) 退化），会话/消息/文档全文/元数据同一 `.db` | qdrant/lancedb、redb、`sqlite-jieba-tokenizer` 0.6（整段 cut 致 600s 退化） |
| PDF | `pdfium-render` + 预编译 PDFium 动态库 | lopdf/pdfsink-rs（中文 CID CMap 解码失败）、poppler、mupdf |
| Office | `office_oxide`（DOCX 读写/PPTX/老格式），`calamine`（XLSX 读），`rust_xlsxwriter`（XLSX 写） | LiteParse（依赖 LibreOffice）、docx-rs、pptx-to-md |
| eBook | `rbook` | `epub`（danigm，GPL-3.0） |
| 多模态 | Rig 原生 `UserContent::Image`，走 VLM API | 本地 Tesseract/whisper-candle |
| 网络 TLS | `rustls` | OpenSSL |
| 架构隔离 | `DocumentParser` trait + dispatcher（按 MIME 路由）+ 统一错误枚举 + 流式解析 | — |
| 联邦查询 | `datafusion` 54.x 单进程内嵌，`TableProvider` trait 统一各数据源 | Trino（JVM+独立进程）、DuckDB（federation 生态弱） |
| 本体存储 | 复用 `memory` 的 SQLite，新增 ontology 表族 | PostgreSQL（独立进程，违反原则 1） |
| TextQL | `sqlparser` 0.62 生成 SQL，NL→意图走 LLM（复用 agent-core） | Python sqlglot（需 Python 运行时） |
| 数据源连接 | `sqlx` 0.9 纯 Rust + rustls，连 MySQL/PG | SQLAlchemy（Python）、OpenSSL |
| 上下文体积管控 | 前端字节预算截断 + 图片降采样，Rust 兜底字节校验（见决策 13）；流式不逐 delta 落库（turn 结束整条写） | tokenizer 精确计数（二期）、逐 delta 写 SQLite（性能差） |

> `office_oxide` / `pdfium-render` 是新库，落地需做兼容性自测（见 `ARCHITECTURE.md` 决策 4/5）。
> Rig 0.41 发布节奏快（约每月 2 个 minor），涉及 Rig 具体 API 名称时以 0.41 官方文档为准核对。

## 前端约束（src/）

- **技术栈**：React 19 + TS strict + Vite 8 + Tailwind v4 + shadcn/ui（复制源码模式，非 npm）
- **状态三层分离**：服务端状态 → TanStack Query；全局 UI 状态 → Zustand；组件本地 → useState。禁止用单一 Zustand 管全部，禁止用 Redux
- **四层单向依赖**：UI → State → IPC → Domain。硬约束：`components/` 不得直接 `invoke`；`stores/` 不得 import 组件；`ipc/` 不得 import `stores/`。用 ESLint `no-restricted-imports` 在 CI 强制
- **IPC 类型安全**：`tauri-specta` 从 Rust `#[tauri::command]` + event 自动生成 TS 绑定，禁止手写 TS 类型
- **流式渲染**：chat 原语用 `@assistant-ui/react` Primitives 模式(修订 F2→F12)；Markdown 用 `@assistant-ui/react-markdown`(修订 F6→F13,streamdown 已移除)；消息列表滚动契约由 `ThreadPrimitive.Viewport` 原生处理
- **持久化分层**：业务数据走 Rust 侧 SQLite；UI 偏好用 `@tauri-store/zustand`；**禁止用 localStorage / IndexedDB**
- **流式 IPC**：优先用 `Channel<T>` 而非 `Event`（点对点、无噪声、fast-path）
- **多窗口**：主窗 / Quick Prompt / 设置窗；跨窗口靠 Event 广播 + TanStack Query invalidate
- **移动端**：单套代码 + Tailwind 断点 + shadcn Dialog/Drawer 自适应，不做双套代码

## specta 版本陷阱

`tauri-specta 1.0.2` 锁定 `specta ^1.0.3`，但 `specta-typescript 0.0.12` 用 `=2.0.0-rc.25` 硬定版本。两者同时存在时，`Cargo.lock` 里会是 `specta 2.0.0-rc.25`（被 specta-typescript 强制拉入）。crates.io 上 specta 同时存在 `1.0.5`(stable) 和 `2.0.0-rc.25`(rc)：若项目只用 `tauri-specta 1.0.2` 路径，`cargo add` 时显式指定 `specta@1.0.5`，误拉 2.0-rc 会在 1.x API 下编译失败；若用了 `specta-typescript 0.0.12`，则必须接受 `specta 2.0.0-rc.25`（API 已变，见下方 BigInt 公约）。两者不可混用——确认项目走哪条版本线后再加依赖。

## 工作守则

1. **改码前先读 `ARCHITECTURE.md`** 对应章节，确认不违背 ADR
2. **引入新依赖前先查许可证**（原则 3）和**运行时依赖**（原则 1）
3. **业务逻辑写到 `crates/`**，`src-tauri/` 只做薄封装；写完用 `cargo test` 在 `crates/` 内验证
4. **前端不直接调业务库**，全部经 IPC 契约；新增 command 同步加 `#[derive(specta::Type)]`
5. **LLM 输出按不可信内容处理**：禁 raw HTML、代码块不执行、外链经确认
6. **遇疑难先搜索再动手**：遇到库的报错/限制/版本陷阱时，**先读该库源码的文档注释和官方文档，再 web_search 确认业界方案**，不要凭局部信息（如只读了一个函数）就下"无解"结论并自造 workaround。历史教训：specta-typescript 0.0.12 的 `BigIntForbidden` 错误，其 `error.rs` 顶部官方文档明确列了 5 种迁移路径（含 `#[specta(type = specta_typescript::Number)]` 一行注解方案），但 agent 只读了 `primitives.rs` 一个硬编码 `return Err` 就断定"无配置开关"，转而手写 5 个 newtype，浪费大量时间——正确做法是先读 `error.rs` 顶部注释 + 搜"specta bigint workaround"
7. **编译检查必须覆盖 src-tauri**：根 `Cargo.toml` 明确 `exclude = ["src-tauri"]`（Tauri crate 有独立 target/feature 流程），**`cargo check --workspace` 只检查 `crates/*`，永远不碰 src-tauri**。验证 src-tauri 必须用 `cargo check --manifest-path src-tauri/Cargo.toml`（或 `cargo run` 本身）。`ONTO_GEN_BINDINGS=1 cargo run` 会编译 src-tauri，但其 warning 易被输出截断/忽略——验证时显式 grep `warning`。历史教训：agent 整个会话反复用 `cargo check --workspace` 自称"零 warning"，实际 src-tauri 的 unused import 一直存在，直到用户 `cargo run` 才暴露。

## IPC 边界 BigInt 公约（specta-typescript 0.0.12+）

`specta-typescript 0.0.12`（依赖 `specta =2.0.0-rc.25`）**硬编码禁止** `u64/i64/usize/isize/i128/u128/f128` 导出为 TS 类型（`primitives.rs` 无条件 `return Err(bigint_forbidden)`），理由是 JS `number` 只有 53 位精度。这是**全有或全无**约束：只要任何一个 `#[derive(specta::Type)]` 的命令/DTO 含裸 64 位整数字段，整个 `Builder.export()` 就失败。

**解决方案（官方推荐，方案 4）**：逐字段加 `#[specta(type = specta_typescript::Number)]` 注解，明示"此值 < 2^53，接受 number 降级"。底层 `Number` 是 `specta_typescript` 内置 OpaqueReference，走 bypass 路径输出 `number`，不触发 bigint 检查。

```rust
use specta_typescript::Number;  // 仅 derive 上下文需要，字段类型保持原样

#[derive(Serialize, Deserialize, specta::Type)]
pub struct MessageRow {
    #[specta(type = Number)]
    pub prompt_tokens: Option<u64>,   // Rust 仍 u64，serde 传 number，specta 导出 number
}
```

- **不要手写 newtype 绕 bigint**（除非该 newtype 有独立领域语义，如 `memory::Timestamp` 表"unix ms 时间戳"）。newtype 方案能工作但冗余、非官方，维护成本高。
- `specta_util::Remapper` 全局重映射仅用于无法逐字段改的场景（如 `serde_json::Value` 内含 bigint），不推荐常规使用。
- 时间戳类字段：优先用 `memory::Timestamp` newtype（有领域语义 + 已实现），新增时间戳字段复用之；纯计数/大小类字段用 `#[specta(type = Number)]` 注解。
- **`#[specta(type = Number)]` 只能用于 struct/enum 字段，不能用于 `#[tauri::command]` 的函数参数**（specta derive macro 不处理函数参数）。命令参数的 bigint 类型（如 `offset`/`limit`/`usize`）：值域小时直接用 `u32`/`i32`（不触发禁令），调用处 `as usize` 转换；值域大时用 newtype 包装。
6. **新库（office_oxide/pdfium-render 等）需写兼容性自测**，放 `crates/ingest/tests/`
7. **流式解析防 OOM**：大文件走流式，不在内存持有全部内容；zip 炸弹防护见 `crates/ingest/src/security.rs`
8. **改动涉及 ADR 时**：先在 `ARCHITECTURE.md` 更新决策记录，再改码

## 落地路线（对齐进度时参考）

- **一期 MVP**：脚手架 → Rig 接云端多模态 → 基础对话+流式 → ingest 核心 parser（PDF/Office/文本）→ 图片输入 → SQLite 会话；前端 AppShell 三栏 + 对话流式 + 拖拽摄入 + ⌘K 基础导航
- **二期**：~~VLM 增强解析~~（A1 暂缓，无 VLM 模型）→ ~~向量检索 RAG（A2，已砍）~~ → ✅ MCP 工具系统（A3）→ ✅ token 预算（B1）+ 历史自动压缩（B2）→ ✅ 文件检索工具（jieba FTS5 + agent 工具，agentic search）→ ✅ `@` 挂载统一 agentic search（决策 17：位置语义 + user message 注脚 + 工具按需读，不注入全文/不破 prefix cache）；上下文管理改用 rig 0.41 原生 `ConversationMemory`+`CompactingMemory`（`memory_bridge.rs`），不手写 trim/compact。前端 ✅ ToolCallCard + MCP 配置区 + Inspector 挂载文档面板，Citation 去强制化。实际落地顺序 A3→~~A2~~→B1→B2→文件工具→`@` 挂载统一→✅ **Agent Skill 系统**（决策 20：agentskills.io 规范，三层 disable + 四类来源 + 渐进式披露，`agent-skills 0.2` + `crates/agent-core/skill/` + 3 个内置 skill）→✅ **会话消息操作**（决策 21：ActionBarPrimitive Copy/ExportMarkdown/Reload + Edit 行内编辑态，后端 `delete_message_and_after` 截断重发）（见 PROGRESS.md）
- **三期（本体设计）**：数据源注册（MySQL/PG/CSV/Excel）→ 本体建模（ObjectType/LinkType/ActionType）→ DataFusion 联邦查询 → TextQL NL→SQL → Agent 工具化（联邦查询作为 MCP tool）；前端 ER 图（react-flow）+ TextQL 编辑器 + 查询结果表格
- **四期（可选）**：本地 Ollama + Qwen2.5-VL 离线高精度 → 移动端适配
