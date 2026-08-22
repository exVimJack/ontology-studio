# 跨端 Agent 应用架构文档(最终版)

## 一、项目定位

基于 **Tauri 2 + Rust** 的跨端(桌面优先,移动端适配)知识工作台,三大核心能力:

1. **Agent core**:Rig 驱动的 agent loop + MCP 工具系统 + 多模态理解
2. **多模态文件读写**:覆盖 PDF / Office / eBook / 文本 / 压缩包 / 图片的统一摄取管道
3. **本体设计 + 联邦查询**:数据源注册(MySQL/PG/CSV/Excel)→ 本体建模(ObjectType/LinkType/ActionType)→ DataFusion 联邦查询 → TextQL 自然语言转 SQL

> 第三能力对标「数据知识图谱」场景:用户注册多个异构数据源,在本体层定义实体/关系/动作的语义模型,通过 TextQL(自然语言)或 SQL 编辑器跨源联邦查询。全程单进程,零外部依赖(无 Trino/PostgreSQL/Docker)。

---

## 二、核心原则(贯穿所有决策)

### 原则 1:Rust 纯栈,运行时零额外原生依赖

**目标机器零安装**——用户拿到二进制即可运行,无需装任何系统库/运行时/办公软件。

- 所有原生代码(SQLite)随 crate 源码**静态编译**进二进制
- 除 Tauri WebView(平台自带)和构建期 C 编译器(仅为 SQLite 系)外,运行时无任何系统依赖
- **明确否决**:任何依赖 LibreOffice / FFmpeg / Tesseract 等外部应用或 C++ 库的方案

> 这条原则在讨论中经历过两次精确化:
>
> 1. 从"纯 Rust 零 C 代码"放宽为"运行时对用户零安装"——bundled SQLite(cc 编译 C 源码)可接受,因为它静态链接、用户无感。但 LibreOffice 这类需系统安装的独立应用绝对否决。
> 2. **PDFium 破例(见决策 5)**:纯 Rust 生态(lopdf / pdfsink-rs / pdf-extract)对中文 PDF 的 CIDFontType2 + ToUnicode CMap 解码存在系统性缺陷,实测对《曾国藩合集》等真实中文 PDF 输出纯乱码或空。改用 pdfium-render(Chrome PDFium C++ 库的 FFI 绑定),其预编译动态库随 Tauri 资源打包、运行时由程序自身加载,**用户无需系统安装**——符合"运行时对用户零安装"的精神。这是纯 Rust 文本提取能力不足的工程妥协,仅限 PDF 解析一处,不构成对 LibreOffice/FFmpeg/Tesseract 等否决项的开放。

### 原则 2:轻量化

- 不内嵌本地模型权重(OCR/Whisper/VLM 一律不走本地重型模型)
- 本地推理能力通过用户自配(Ollama 等)接入,应用本身保持小体积
- 重型能力(多模态理解)走 API,按需调用

### 原则 3:许可证友好

全部 MIT / Apache-2.0 / Unlicense,**排除 GPL**(曾否决 GPL 的 `epub` crate)。

### 原则 4:业务核心与平台解耦

- 业务逻辑全部在 `crates/`(平台无关),可独立 `cargo test`
- Tauri 只做 IPC 薄层 + 平台能力
- `DocumentParser` trait 抽象底层解析库,可平滑替换(为未来留演进空间)

### 原则 5:外部服务最小化且显式

仅两类外部依赖(按定义不可避免,且显式可知):

- **模型 API**(云端 LLM/VLM,或用户自配本地 Ollama)
- **MCP server**(用户按需接入的工具服务)

---

## 三、关键决策记录(ADR 式)

### 决策 1:Agent 框架选 Rig

**决策**:`rig-core` 作为 agent 核心。
**理由**:Rust 生态最完整的嵌入式 agent 库,单 crate 覆盖 completion/embedding/tool/RAG/multi-agent;原生支持多模态输入;20+ provider 统一抽象。
**否决项**:

- `pi_agent_rust`(Dicklesworthstone)——是 CLI 产品非库,工具写死编码场景,方向不符
- 官方 `pi-agent` crate——可行但 RAG/向量弱,生态窄;Rig 更通用
- LangChain-rust 等——成熟度不及 Rig

### 决策 2:不启用 Rig 的 loaders 模块

**决策**:Rig 仅用 agent loop / provider / vector store / tool,**关闭 `pdf` 和 `epub` feature**。
**理由**:

- Rig 的 `PdfFileLoader` 是基础文本抽取,会与 pdfsink-rs 形成冗余双栈
- Rig 的 `EpubFileLoader` 底层可能拉 **GPL 的 `epub` crate**,有许可证风险
- 自建 `crates/ingest` 是 Rig loaders 的超集,且每项按纯 Rust + 许可证精选

### 决策 3:存储统一为 SQLite(会话 + 文档全文 + FTS5 索引)

**决策**:`rusqlite`(bundled)+ 自实现分句版 jieba FTS5 tokenizer(基于 jieba-rs + rusqlite-ext),会话/消息/文档全文/元数据同在一个 `.db` 文件。
**理由**:

- 存储层单一,运维极简
- bundled SQLite 默认编译 FTS5,无需额外依赖即支持中文全文检索(jieba 分词)
- 有 SQL 表达力,全文检索 + 元数据过滤顺手
**否决项**:独立向量库(qdrant/lancedb)、纯 Rust 的 redb——前者增依赖,后者失 SQL。

**落地注**:FTS5 由 bundled SQLite 默认提供(`-DSQLITE_ENABLE_FTS5`)。jieba 中文分词通过自实现 `JiebaSentenceTokenizer`(`crates/memory/src/jieba_tokenizer.rs`,基于 jieba-rs + rusqlite-ext),分句后逐句 cut 避免 DAG O(n²) 退化。tokenizer 是 per-connection 注册,`Memory::open`(含 `open_indexer_connection`)统一调 `register_fts5_tokenizers`。详见决策 15。

### 决策 4:Office 解析用 office_oxide 统一(除 XLSX 读)

**决策**:

- DOCX 读/写、PPTX、老格式 DOC/XLS/PPT → `office_oxide = "0.1.8"`
- XLSX 读 → `calamine`(保留,类型/公式保真更优)
- XLSX 写 → `rust_xlsxwriter`(保留,领域标准)

**理由**(基于解析质量对比,非性能):

- office_oxide 的统一 IR(标题/段落/列表/表格/页眉页脚/图片,保留交错顺序)对 agent/RAG 场景更顺手
- PPTX 的 slide 分节 + 表格内联 + 备注保留,优于 pptx-to-md 的扁平 MD
- **老格式 DOC/XLS/PPT 是独家能力**(纯 Rust 无其他解)
- XLSX 保留 calamine:office_oxide 的 IR 把单元格扁平为 string,丢失类型/公式,精细读 Excel 是损失

**约束**:office_oxide 极新(2026年4月生,0.1.x),`DocumentParser` trait 隔离可平滑回退;落地需做兼容性自测。

**否决项**:

- `pdfium-render`——pdfium 是 C++ 库,违背纯 Rust
- LiteParse v2——**决定性否决**:Office 解析依赖 **LibreOffice 系统安装**,无法对用户隐藏,违背原则 1;且 Office 转 PDF 再解析丢失原生结构
- docx-rs / pptx-to-md——被 office_oxide 在结构化保真上超越

### 决策 5:PDF 解析用 pdfium-render(预编译 PDFium 动态库)

**决策**:`pdfium-render 0.9.3`(纯 FFI 绑定)+ 预编译 PDFium 动态库(bblanchon/pdfium-binaries,chromium/7881),随 Tauri 资源打包、运行时由程序加载。`lopdf` / `pdfsink-rs` 弃用。

**理由(破例原则 1 的 ADR)**:

- 纯 Rust 路线(lopdf / pdfsink-rs)对中文 PDF 解析失败。实测《曾国藩合集》(1371 页,Type0 + CIDFontType2 + SimSun 子集 + ToUnicode CMap):lopdf 输出纯乱码(第2页仅 42 字符拉丁符号),pdfsink-rs(底层 pdf-extract)输出空。两者都无法正确解码该类 CMap。
- pdfium 是 Chrome 内核,渲染级文本提取,CJK/CID/ToUnicode 零乱码。实测同文件:1371 页全量提取 4.15s,中文完美、无字间空格。
- **不编译 PDFium C++ 源码**:全程使用社区预编译二进制(bblanchon/pdfium-binaries),`pdfium-render` 仅做 FFI 绑定,`cargo build` 只编译 Rust 代码,无 GN/Ninja/Chromium 编译。
- **运行时对用户零安装**:动态库(`pdfium.dll`/`libpdfium.dylib`/`libpdfium.so`)随 Tauri `bundle.resources` 打包进安装包,程序启动时从资源目录加载,用户无需任何系统安装——符合原则 1 的精神(区别于 LibreOffice 需用户自行安装)。

**集成方式**:

- `crates/ingest` 暴露进程级单例 `init_pdfium(lib_path)`,`PdfParser` 通过单例借用 `Pdfium`(thread_safe feature,`Pdfium` 实现 Send+Sync,可跨线程共享借用)。**关键:PDFium C 库本身非线程安全**(官方 `fpdfview.h` 明确要求调用方用 mutex 串行化所有调用),而 pdfium-render 0.9.3 的 `thread_safe` feature **仅 impl Send+Sync,不串行化 FFI 调用**(native 路径 `DynamicPdfiumBindings` 的每个 FPDF 函数直接调函数指针,无锁;issue #20「Marshall calls」是未完成的特性请求,issue #66 作者承认跨线程 segfault;官方文档说的 mutex 仅 WASM 路径生效)。因此本项目在 `crates/ingest/src/pdfium.rs` 用进程级 `Mutex<()>` 令牌锁(`with_pdfium()`)串行化所有 PDFium 操作——`PdfParser::parse_with_progress` 在整个 load→extract→drop 临界区持锁,任意时刻只有一个线程在调用 PDFium。多文件 batch 实际串行执行 PDF(非 PDF 文件不受锁影响、仍并行),性能依赖单文件速度(实测 1711 页 4.5s,可接受)。
- `src-tauri` 启动时(setup hook)定位资源目录中的平台库,调 `ingest::init_pdfium`。
- 库版本与 `pdfium-render` 的 `pdfium_xxxx` feature 严格一致(当前 7881),不匹配会 FFI 崩溃。
- dev 环境:`src-tauri/resources/pdfium/<platform>/` 本地存放;CI 构建前用脚本拉取。

**已知限制**:扫描型 PDF(无文本层)仍无法解析——由决策 7 的 VLM 增强层解决。

**弃用项**:lopdf(对 CID CMap 解码缺陷)、pdfsink-rs(底层 pdf-extract 同样失败,且全量几何解析对大 PDF 较慢)、pdfium 的 static/bundled feature(不存在;pdfium-render 本就是纯 FFI,无编译)。

### 决策 6:eBook 用 rbook

**决策**:`rbook` 替换 `epub-parser`。
**理由**:更成熟(0.7.x)、Apache-2.0、ePub2/3 读+构建+编辑、持续活跃。
**否决项**:`epub`(danigm)——**GPL-3.0,许可证传染,排除**。

### 决策 7:多模态能力通过 VLM API 接入,不引入本地模型

**决策**:图片输入支持 + 复杂排版增强解析,均通过 Rig 的多模态 `CompletionModel`(云端 API 或用户自配 Ollama)实现。
**理由**:

- Rig 原生支持 `UserContent::Image` 等多模态消息,无需自建抽象层
- 多模态模型统一了"图片理解 + OCR + 复杂排版解析"三个问题,一个能力三用
- 符合轻量化:不带模型权重,按需调 API
- VLM 解析质量研究证实优于传统工具(olmOCR 等)

**分阶段**:

- 一期:图片输入支持(ingest 解码 → base64 → Rig Message)
- 二期:VLM 增强解析(复杂度检测 → 页面渲染图 → VLM 重解析),解决扫描件死区 + 提升复杂排版
- 三期(可选):本地 Ollama + Qwen2.5-VL 支持离线高精度

**否决项**:本地 Tesseract/whisper-candle 等——重,需模型权重,违背轻量化。

### 决策 7.5:Provider 配置补全——按 kind 路由 rig 原生 provider(2026-08-02)

**决策**:`ProviderKind` 从 2 种(OpenAiCompatible/Anthropic)扩到 13 种(OpenAi/Anthropic/Gemini/DeepSeek/Xai/Groq/OpenRouter/Ollama/Moonshot/Mistral/Cohere/Perplexity/OpenAiCompatible),按 kind 路由到 rig 原生 provider client,而非全部塞进 `openai::Client.completions_api()`。`ProviderConfig` 字段对齐 pi-coding-agent 的 `models.json` schema(采样/兼容性/input_types/headers/reasoning)。
**理由**:

- **P0 正确性**:原方案把 DeepSeek 塞进通用 openai::Client,丢失 rig 的 `OpenAICompatibleProvider` capability 声明(DeepSeek 的 `EMITS_COMPLETE_SINGLE_CHUNK_TOOL_CALLS=true`、`SUPPORTS_RESPONSE_FORMAT=false`)——可能导致流式 tool call 渲染异常、response_format 被错误发送触发 400。按 kind 路由后各 provider 的 capability 由 rig 正确声明。
- **能力扩展**:Gemini(非 OpenAI 协议)首次可用;rig 原生支持 24+ provider,onto-studio 只用了 2 个。
- **架构重构**:引入 `ProviderRuntime` trait 擦除 rig client 泛型类型(原 `DynClient` enum 的 8 处 `match &self.client` 分支全部消除),新增 provider 只需 ① `build_runtime` 加构造分支 ② 宏 `impl_provider_runtime!` 生成 trait impl。`ChatService` 持 `Arc<dyn ProviderRuntime>`。
- **字段对齐 pi**:采样(temperature/max_tokens/top_p)、兼容性(supports_developer_role/supports_reasoning_effort,input_types)、增强(extra_headers/reasoning 4 级)。降低用户从 pi 迁移成本。
- **OpenAI 官方走 Responses API**:`OpenAi` kind 用 rig 默认的 responses_api(),其余兼容端点走 completions_api()。`supports_developer_role=false` 调 `with_system_instructions_as_messages()`(rig 0.41 原生)。
- **context_window 探测保留手写**:rig `ModelListingClient::list_models()` 的 `Model` struct 虽有 `context_length` 字段,但 Anthropic lister 丢弃了官方 `context_window`(只取 id/name),OpenAI 官方/DeepSeek 响应里本来就没有该字段——手写按 kind 分派(OpenAI 兼容 /models、Anthropic /v1/models、Gemini models.get、Ollama /api/show)反而覆盖更全。当前五层 fallback(用户配置 > 官方元数据探测 > Ollama /api/show > 内置已知模型表 > 默认 100K)已健壮,不为了用 rig 而用 rig。
**落地**:`crates/agent-core/src/provider.rs`(ProviderKind 扩 13 种 + ProviderConfig 扩字段 + reasoning_to_params)、`chat.rs`(ProviderRuntime trait + helper 泛型函数 + build_runtime + 宏)、`src-tauri/commands/provider.rs`(SetProviderInput 扩字段)、前端 `domain/index.ts`(PROVIDER_PRESETS 13 个 + presetForKind)、`SettingsView.tsx`(高级设置折叠区)、`bindings.ts`(手动更新)。
**否决项**:① 全部 provider 走 openai::Client 一刀切(丢失 capability,已废弃);② 用 rig ModelListingClient 替换手写 context_window 探测(收益不足);③ pi 的 7 级 thinkingLevelMap(onto-studio 非编码 agent,4 级够用,后续可扩)。

### 决策 8:分层架构,trait 隔离底层库

**决策**:`DocumentParser` trait 统一接口,文件分发层按 MIME 路由,统一错误枚举,流式解析防 OOM。
**理由**:底层库(尤其 office_oxide/pdfsink-rs 等新库)可能换,trait 隔离使迁移成本限于 ingest 内部,不影响上层。

### 决策 9:联邦查询引擎选 DataFusion(替代 Trino)

**决策**:用 `datafusion` 54.x 作为单进程内嵌的联邦查询引擎,替代原 Python+Docker 方案中的 Trino(JVM,~2GB)。
**理由**:

- 单进程内嵌,零 JVM 开销,符合原则 1(运行时零外部依赖)
- 原生 Arrow 内存格式,跨源查询零拷贝
- `TableProvider` trait 统一抽象各数据源,新增数据源只需实现一个 trait(约 150 行/源)
- Apache-2.0 许可证,符合原则 3
- 查询能力对等 Trino(过滤/聚合/JOIN/子查询),单用户桌面场景无需 Trino 的分布式调度
**否决项**:Trino(JVM+独立进程,违反原则 1)、DuckDB(C++ 嵌入,虽可接受但 federation 生态弱于 DataFusion)

**落地状态(2026-07-30,三期)**:`crates/federation/` 已实现全栈。

- DataFusion 54.x + `datafusion-federation 0.5.5` 的 `SQLExecutor` trait(MySQL/PG 用 sqlx 0.9 + rustls,CSV 走 temp table 模式);Excel 待补
- 每个数据源注册为独立 catalog(`public` schema,三段式 `catalog.public.table` 寻址)
- 只读守卫(sqlparser 解析,仅 `Query`/`Explain(Query)` 放行)+ 自动 LIMIT 注入(默认 200,max 1000)+ 30s 超时
- schema 浏览:三段式 `{catalog}.information_schema.tables WHERE table_schema='public'` 避开系统表 + `table_provider` 回退
- 10 测试通过;src-tauri 9 IPC 命令 + 前端三栏工作台(SQL 编辑器+结果表+schema 树+注册对话框)
- DF 54 已知限制:`deregister_catalog` 不存在,catalog 随进程留存(重启不恢复);CSV 无法直接注册为带 schema provider 的 catalog,改走 temp table 模式
- 详见 `PROGRESS.md`「联邦查询全栈」节

### 决策 10:本体元数据存 SQLite(替代 PostgreSQL)

**决策**:本体元数据(ObjectType/LinkType/ActionType/数据源注册/凭证)全部存入 `memory` crate 的同一 SQLite `.db` 文件,新增 `ontology` 表族。
**理由**:

- 本体元数据是 CRUD + JSON 字段,单用户桌面,不需要服务器进程(原方案用 PostgreSQL 16)
- 复用 `memory` crate 的 `rusqlite`(bundled),零新增原生依赖
- 单文件 `~/.onto-studio/onto-studio.db` 便携备份
- 凭证一期明文存(同 provider API key,§20.9),二期加密
**否决项**:PostgreSQL(独立进程,违反原则 1)、嵌入式 PostgreSQL(体积过大)

### 决策 11:TextQL 编译器用 sqlparser-rs(替代 Python sqlglot)

**决策**:用 `sqlparser` 0.62 作为 SQL 生成/解析后端,TextQL→SQL 的语义映射层用 Rust 重写(约 400 行)。
**理由**:

- 原方案用 Python sqlglot,需 Python 运行时(违反原则 1)
- sqlparser-rs 是 sqlglot 的 Rust 对等物,ANSI SQL:2011,支持 MySQL/PG 方言
- TextQL→逻辑计划→各源方言 SQL 的三段式编译,逻辑层与 DataFusion 的逻辑计划自然衔接
- Apache-2.0 许可证
**注**:TextQL 的自然语言→逻辑计划部分走 LLM(复用 agent-core 的 Rig),非硬编码规则。LLM 产出结构化意图,sqlparser-rs 负责语法正确的 SQL 生成。

### 决策 12:数据源连接用 sqlx(替代 SQLAlchemy)

**决策**:用 `sqlx` 0.9 连接 MySQL/PostgreSQL,纯 Rust(rustls),schema 浏览走 `information_schema`。
**理由**:

- 原方案用 Python SQLAlchemy(async),需 Python 运行时
- sqlx 纯 Rust + rustls(符合原则 1,禁 OpenSSL),编译期 SQL 校验
- MySQL/PG 统一 trait,DataFusion 的 `TableProvider` 在其上封装
- MIT/Apache-2.0 许可证
**注**:CSV/Excel 复用 ingest crate 的 calamine + datafusion 内置 CSV provider,不额外连接。

### 决策 13:上下文体积预防性管控(防 413 Payload Too Large)

**决策**:发送前对注入单次请求的上下文做分层体积管控,不引入 tokenizer(二期 RAG 范畴),用字节预算 + 图片降采样覆盖一期场景:

1. **文档文本字节预算截断**(前端 `context-budget.ts`):单篇 512 KiB + 总量 2 MiB,超限尾部截断并标注原文字符数,保留文件名让模型知存在哪些文件。静默不打断用户。
2. **图片降采样**(前端 `image-resize.ts`):Canvas 降采样到长边≤2048px、短边≤768px(OpenAI vision 官方建议值,模型内部亦降采样至此),重编码 JPEG q=0.85。base64 膨胀约 33% 是 body 杀手,降采样显著缩小体积。
3. **单图字节硬上限**(前端):降采样后仍超 2 MiB 的罕见图直接跳过并内联提示。
4. **Rust 兜底字节校验**(`send_message`):拼好 prompt 后估算 body 字节,超 28 MiB 返回友好错误(留余量给历史+JSON 包装,Anthropic Messages API 硬上限 32 MiB),不让 provider 网关(openresty 等)直接 413 黑屏。

**理由**:

- 413 与 token 用量无关,是 HTTP body 字节数超 provider 网关限制(openresty/aiohttp 默认 1 MiB,Anthropic 32 MiB,OpenAI 512 MiB)。base64 图片是主犯(OpenCode/Claude Code/Cherry Studio 同款 bug)。
- 业界共识:长上下文不取代 RAG,一期“stuff what you can”场景用字节预算足够;token 级精确预算与 413 后自动压缩恢复(stripMedia + compaction)留二期 RAG 一起做。
- 字节预算取 char/4 启发式(Claude Code 热路径同款),偏松但安全;常量集中便于调参。
- 降采样是 ROI 最高的一条:原图直发纯属浪费 body,模型用不到更高分辨率。

### 决策 14:MCP 工具系统——rmcp 1.x + 自实现 DynamicTool 桥接(二期 A3)

**决策**:用 `rmcp ^1.8`(而非 3.0-beta)接 MCP server,自实现 rig 0.41 的 `DynamicTool` 适配器(0.41 取消了 `ToolDyn` trait,改用 `DynamicTool::new(name,desc,params,callback)`)把 MCP 工具注册到 Rig `ToolServerHandle`,不启用 rig 的 rmcp feature。

**理由**:

- **版本锁定 1.x**:rig 0.41 依赖 `rmcp ^1.8`。若直接用 rmcp 3.0-beta 会与 rig 拉的 1.x 共存,且 rmcp 3.x 的 macros feature 依赖 pastey/darling,在当前 toolchain(rustc 1.97)+ cargo feature unification 下触发 cargo resolver panic(`did not find features for darling`)。保持 1.x 统一版本绕开。
- **不启用 rig 的 rmcp feature**:rig 的 `tool::rmcp` 模块针对 rmcp 3.0-beta API,开启会拉 rmcp default features(macros/server)触发上述 resolver bug。自实现 `DynamicTool` 桥接只需 rmcp `client` + 两个 transport feature,无 macros。
- **0.41 DynamicTool API**:rig 0.41 取消 `ToolDyn` trait,改用 `DynamicTool::new(name, description, parameters, callback)`,callback 签名 `Fn(&mut ToolContext, Value) -> Future<Result<ToolOutput, ToolExecutionError>>`。`ToolError` 被 `ToolExecutionError` 取代。`ToolServerHandle::add_dynamic_tool(DynamicTool)` 替代 `add_tool(impl ToolDyn)`。
- **stdio transport 自实现**:不用 rmcp 的 `transport-child-process`(依赖 process-wrap 拉入 windows crate 重型包,当前 toolchain 编译失败),改用 `tokio::process::Command` spawn + rmcp 的 `(AsyncRead, AsyncWrite)` transport,零额外原生依赖(符合原则 1)。Windows 下设 `CREATE_NO_WINDOW` 避免弹控制台。
- **HTTP transport**:rmcp `transport-streamable-http-client-reqwest`,复用 reqwest + rustls(原则 1)。
- **静态工具模式**:连接时一次性 `list_all_tools` 注册到共享 `ToolServerHandle`,agent 构建时注入。工具列表变更通知(`on_tool_list_changed`)一期不处理。
- **ClientHandler**:`ClientInfo` 自身实现了 `ClientHandler`(空实现),直接 `.serve(ClientInfo::new(...))` 无需自定义 handler。

**落地**:`crates/agent-core/src/mcp.rs` 的 `McpManager`(连接管理 + 工具注册)+ `build_dynamic_tool`(DynamicTool 适配器)+ `McpServerConfig`(Stdio/Http 配置,specta::Type)。

**否决项**:

- rig 官方 `tool::rmcp` 集成——版本冲突 + resolver bug
- rmcp 3.0-beta——同上
- `transport-child-process`——process-wrap/windows crate 编译失败

---

### 决策 15:文件检索——agentic search(FTS5 + jieba 分词 + agent 工具,一期收尾)

**决策**:砍掉二期向量 RAG(原 sqlite-vec KNN + embedding 方案),改用「文件工具 + FTS5 全文搜索」的 agentic search 模式。ingest 摄取的全文持久化到 memory crate 的 `documents` 表,建 FTS5 虚拟表(jieba 中文分词 + BM25 排序 + snippet 高亮,external content table 模式),三个 agent 工具(`list_documents`/`search_documents`/`read_document`)挂到 agent,模型按需检索而非自动注入切片。

**理由**:

- **业界共识(2026)**:小语料文件工具优于 RAG(Anthropic 在 Claude Code 用 agentic search 替换 RAG,+2.7% 准确率)。onto-studio 是个人知识工作台(几十到几百文档),适合文件工具模式。向量检索留到四期(本地 Ollama embedding 时再考虑混合检索)。
- **FTS5 是白送的能力**:bundled SQLite 默认编译了 `-DSQLITE_ENABLE_FTS5`,无需加任何 feature 或依赖。支持 BM25 排序、snippet 高亮、external content table(全文只存一份)。
- **口语化 query 问题在工具路径下不存在**:用户口语化长 query 直接做 FTS5 MATCH 会翻车(噪声词 + AND 语义),向量检索有语义泛化优势。但走文件工具路径,query 来自模型的 `search_documents` 调用而非用户原话——模型天然会把口语化意图转成搜索关键词,FTS5 完美适配。
- **符合原则 2 + 原则 5**:无需 embed API(外部服务最小化)、无本地 embedding 模型(不内嵌权重)、无 sqlite-vec(少一个原生扩展)。
- **砍掉 retrieval crate + sqlite-vec**:移除 `crates/retrieval/`(切片器 + EmbedClient)、`crates/vendor/sqlite-vec/`、memory `vectors.rs`、`doc_chunks`/`vec_chunks` 表、所有 RAG IPC 命令 + 前端 useRag。架构大幅简化。

**jieba 分词**:

- **自实现分句版 JiebaSentenceTokenizer**(`crates/memory/src/jieba_tokenizer.rs`),替代 `sqlite-jieba-tokenizer` 0.6。原因:该 crate 的 `tokenize` 直接对整段文本 `JIEBA.cut(text, true)`,jieba DAG 对超长连续文本 O(n²) 退化——1.4M 字符 PDF 需 600s+(吞吐从 1k 的 214k chars/s 掉到 200k 的 18k chars/s)。分句版先按标点/换行分句再逐句 cut,offset 按 byte 精确累加,实测 **600s → 2.91s**(提升 200 倍)。stopword/stemmer/小写归一化逻辑与原版一致,索引兼容。
- jieba-rs 内嵌词典(dict.txt + idf.txt,~11MB)编进二进制,已确认接受(词典非模型权重,性质上是数据文件)。
- tokenizer 是 per-connection 注册(基于 `rusqlite-ext::register_tokenizer`),每个新 `Connection::open`(含 `open_indexer_connection`)后调 `register_fts5_tokenizers`。Memory::open/init_schema 统一注册。
- 曾评估 `sqlite-simple-tokenizer`(拼音单字,微信方案),但拼音能力对 agent 工具场景无用(模型给中文关键词),单字分词噪音比 jieba 大,故不采用。

**落地**:

- `crates/memory/src/documents.rs`:`documents` 表(id/path/name/format/text/char_count/created_at/indexed_at)+ `documents_fts` FTS5 虚拟表(content='documents' external content table + jieba 分词)**无触发器**。Repo 方法:upsert/list/read(分页)/search/delete/index_document。
  - **异步索引**(取代同步触发器):jieba 对大全文(1M+ 字符 PDF)分词建索引耗时 10 分钟+,同步触发器会阻塞 `ingest_files` 返回(前端表现为“解析完成后卡住”)。改为:upsert 只写主行(毫秒级,用旧值删旧 FTS5 索引保证一致性,置 indexed_at=0)→ `spawn_blocking` fire-and-forget 调 `index_document(id)` 在**独立连接**上建 FTS5 索引(WAL 模式下不抢主连接锁)→ indexed_at 置完成时间戳。search WHERE indexed_at>0 过滤未索引文档。大文档索引构建期间搜不到,其他功能不受影响。
  - external content 模式('delete' 需原文):upsert 更新时先用旧 SELECT 的旧值删旧 FTS5 索引,再更新主行;delete 同理。contentless 模式不支持 DELETE,不可用。detail=none 不支持 phrase 查询(中文多字词被 jieba 拆多 token 后报错),不可用——用默认 detail=full。
- `crates/agent-core/src/document_tools.rs`:三个 `DynamicTool`(闭包捕获 `Arc<Memory>`)。`list_documents()` 列清单、`search_documents(query, limit?)` FTS5 MATCH + BM25 + snippet、`read_document(id, offset?, limit?)` 分页读全文。
- 工具注入:`stream_with_memory` 复用 MCP `ToolServerHandle` + `add_dynamic_tool()`(rig 0.41 builder typestate 不允许同时 `dynamic_tools()` + `tool_server_handle()`,但 ToolServerHandle 运行时可动态加,同名替换幂等,MCP 工具与文件工具共存)。
- ChatService 加 `raw_memory: Option<Arc<Memory>>` 字段(set_memory 时保存原始句柄,供工具访问 documents 表;`self.memory` 是 `Arc<dyn ConversationMemory>` trait object,工具需具体类型)。
- ingest 摄取成功后 `upsert_document` 持久化全文(同 path 幂等替换)。前端 `useIngest` 不再调 RAG 入库。
- `@` 挂载语义(一期收尾后改为工具按需读,见决策 17):用户在消息中 `@fileName` 引用文档 → 文本原位保留 token(位置语义)→ 后端查 id+name 在 user message 尾部追加 `<mounted-documents>` 注脚 → 模型按需调 `read_document(id)` 取全文。不再整篇注入、不再发 Sources chunk。

**否决项**:

- 向量 RAG(sqlite-vec KNN + embedding)——一期不需要,留四期
- rig `EmbeddingModel` trait——client 泛型纠缠,且不再需要
- 本地 embedding 模型(原则 2)
- 独立向量库(qdrant/lancedb)——违反决策 3 单一 .db
- sqlite-simple-tokenizer——拼音对工具场景无用
- rig-sqlite 全量迁移——不再需要(无向量存储)

---

### 决策 16:上下文管理——rig 0.41 原生 ConversationMemory + CompactingMemory(二期 B1 + B2)

**决策**:直接复用 rig 0.41 的上下文管理基础设施,不手写 trim/compact。`crates/agent-core/memory_bridge.rs` 把项目 `Memory`(SQLite)适配为 rig `ConversationMemory`(newtype `SqliteMemory` 绕过孤儿规则),再挂 `CompactingMemory` 组合 `TokenWindowMemory` policy(超预算裁剪)+ 自实现 `LlmCompactor`(LLM 生成滚动摘要)。agent 构造 `.memory(CompactingMemory).conversation(id)`,load 时自动裁剪+压缩,turn 结束 rig 自动 append(我们的实现是 no-op,消息由 send_message 手动建)。

**理由**:

- **不重复造轮**:rig 0.41 + rig-memory 0.41 提供完整上下文管理(`ConversationMemory` trait + `Compactor` trait + `CompactingMemory`/`TokenWindowMemory`/`HeuristicTokenCounter`)。手写 trim/compact 是重复实现已有能力,且 rig 的实现更完备(in-flight 防并发、watermark 去重、carry_over 滚动摘要)。
- **不引入 tokenizer**(原则 2 轻量化)。`HeuristicTokenCounter::openai()` = chars/4 + per_message_overhead,与原手写 `estimate_tokens` 同语义,偏松安全侧。
- **模型上下文窗口动态获取**:替代硬编码 100K,`context_window.rs` 五层 fallback(用户配置 > 官方元数据探测(按 kind 分派:OpenAI 兼容 /models 读 context_length、Anthropic /v1/models 读 context_window、Gemini models.get 读 inputTokenLimit、Ollama /api/show 读 GGUF) > 内置已知模型表(DeepSeek V4 1M 等无官方元数据的模型,按 kind+模型名前缀匹配) > 默认 100K),作为 `TokenWindowMemory` 的预算参数。探测全程超时+错误保护,1h 缓存,配置变更 `invalidate_cache()` 清空。
- **真实 usage 落库**:rig 流式上报 `CompletionCall`(含 `Usage`),`has_values()` 过滤全 0 哨兵后落库到 assistant 消息(prompt_tokens/completion_tokens/total_tokens 三列,幂等迁移)。供下次 context_window 判定参考。
- **append no-op 的设计权衡**:消息由 `send_message` 预生成 id 手动建(前端需 message_id 做流式 patch),rig turn 结束的 append 被忽略避免重复写入。load 仍读 DB 全量,CompactingMemory 在 load 时裁剪——此机制不依赖 append。代价:摘要仅存进程内存 state(carry_over),重启后重新 compact(可接受)。
- **流式不逐 delta 落库**:旧方案每个 TextDelta 都 `UPDATE messages SET content = content || ?`(性能差)。新方案 turn 结束整条写入,前端靠 Channel 推 TextDelta patch 内存实时渲染。代价:进行中的流式消息刷新页面会消失(turn 未结束 DB 无此条),但这是正常 chat 应用行为。

**落地**:

- `crates/agent-core/src/memory_bridge.rs`:`SqliteMemory`(newtype 包 `Arc<Memory>`)impl `ConversationMemory`(load=list_messages→rig Message,append=no-op,clear=删会话消息);`LlmCompactor` impl `Compactor`(Artifact=`SummaryArtifact(String)`,调 `SummaryFn` trait object 生成摘要);`build_compacting_memory(memory,window,summarize)` 构造 `CompactingMemory`(带 3 个单元测试)
- `crates/agent-core/src/chat.rs`:`ChatService` 加 `memory: Option<Arc<dyn ConversationMemory>>` 字段 + `set_memory(memory,context_window)`(内部构造 CompactingMemory 缓存);`stream_with_memory(prompt,conv_id)` 走 `.memory().conversation(id)` 路径;`build_summarize_fn()` 构造闭包调同 provider 非流式 prompt
- `crates/memory/src/repo.rs`:`create_message_with_id`(预生成 id 落库,供 send_message turn 结束写 user+assistant);`delete_conversation_messages`(clear 用);`set_message_usage`(usage 落库)
- `src-tauri/commands/chat.rs`:`send_message` 重构为 RAG 注入→构造 prompt→`stream_with_memory`→消费流推 Channel(不落库)→`persist_turn` turn 结束整条写 user+assistant+usage
- `src-tauri/commands/provider.rs`:`set_provider` 后调 `chat.set_memory(memory, resolve_context_window)`;`restore_provider` 用同步 `resolve_known_or_default`(用户配置 > 已知模型表 > 100K;启动期无法 async 探测)

**否决项**:

- 手写 trim_history/compact_history——重复造轮,已被 rig 原生方案取代(代码保留在 context_budget.rs 但生产路径不再调用)
- tiktoken-rs——二期不引入 tokenizer(原则 2)
- 摘要落 DB——CompactingMemory 设计上摘要存内存 state,落 DB 会破坏其 watermark 语义
- 逐 delta 落库——性能差,turn 结束整条写更合理

---

### 决策 17:`@` 挂载语义 + prompt cache 友好的上下文注入(一期收尾)

**决策**:`@` 挂载文档**不注入全文**,改为「位置语义 token + user message 尾部注脚 + 工具按需读」的 agentic search 模式。推翻决策 15 早期版本的「`@` 挂载走 `context_texts` 整篇注入」设计。

**核心原则(prompt cache 友好的上下文架构)**:

prompt caching 是 prefix match——前缀任意一字节变化,后面全失效(Anthropic 官方)。渲染顺序 `tools → system → messages`,cache key 是前缀字节哈希。因此:

1. **静态前缀冻结**:tools 定义、system prompt 在会话期间**永不变化**。动态状态(当前时间、文件清单、会话状态)绝不进 system prompt——放 user message 尾部(动态区,变化不破坏 prefix cache)。
2. **挂载信息进 user message,不进 system prompt**:用户 `@` 的文档清单作为 `<mounted-documents>` 注脚追加到**本轮 user message 尾部**,每轮变化但不影响已缓存的 prefix。
3. **不给摘要**:注脚只给 `id + name`,不给全文/摘要。避免 batch 上传多文档时生成摘要的延迟,且文件名已足够模型决策「是否调 read_document 取全文」。
4. **不注入全文**:挂载文档全文不进 prompt。模型按需调 `read_document(id)` 工具分页读取(大文档安全,不撑爆 context window)。这与未挂载文档走 `search_documents` → `read_document` 的路径**完全统一**。

**`@` mention 文本处理(位置语义保留)**:

用户输入 `@` 触发 MentionMenu,选中文档后**在光标位置插入 `@fileName` 文本**(不删 `@`,不替换为 chip)。业界共识(Cursor/AgenticX/shogo-ai/chamber):`@fileName` 作为可读 token 留在文本原位,既保留位置语义(「对比 @A 和 @B 的架构」),又让模型在自然语言里就能看到用户引用了哪些文件。发送时前端解析文本里所有 `@<name>` token,匹配已挂载文档得 path 列表(`resolveMentionedPaths`)。

**注脚格式**(追加到 user message 尾部):

```
用户原文…

<mounted-documents>
用户在本消息中 @ 引用了以下文档(可用 read_document 工具按需读取全文):
- id: doc_abc, name: 架构设计.pdf
- id: doc_def, name: 实施方案.docx
</mounted-documents>
```

**理由**:

- **业界实践(2026 调研)**:
  - ChatGPT 5K 字符规则:粘贴超 5000 字符自动转附件,多数消费级聊天工具**不把大文档塞进 context**,而是索引后按需检索喂相关 chunk。
  - kindatechnical 决策树:语料 < 150K tokens 才用 long context 全文注入;超 200K tokens 用 Files API / RAG 按需检索。中文文档 100k 字 ≈ 150k tokens,一篇就接近上限,全文注入不合理。
  - OpenAI File Search / Claude Files API / agno FileTools / DevoxxGenie Agent Mode:system prompt **不**提具体文件,只描述工具能力,模型按需调 `list_files`/`file_search`/`read_file` 自己发现。OpenAI 自动注入的「the user has uploaded files…」提示被社区广泛吐槽为 bad pattern(导致模型不停说「我看到你上传了文件」、过度调用 file_search、干扰其他工具)。
  - Anthropic static-first 架构(Claude Code 实现):system prompt ~4k tokens 全局冻结,动态状态(git status/当前文件)通过 `<system-reminder>` 标签注入 user message,绝不碰 system prompt。达成 90-96% cache hit rate。
- **prefix cache 经济性**:18k tokens 静态前缀,50 轮会话:无缓存 $13.50 vs 有缓存 $1.35(差 10 倍,仅前缀成本)。system prompt 塞文件清单会每轮失效,成本飙升。
- **大文档安全**:全文注入时 100k 中文字 ≈ 150k tokens,一篇就快顶 long context 上限,多轮叠加历史必然溢出。工具按需读天然支持分页,大文档不撑爆。
- **统一检索路径**:`@` 挂载 = 模型可检索的预选集(注脚告知 id),与未挂载文档走同一套 `read_document` 工具。不再有两套路径(挂载注入 vs 工具检索)。
- **位置语义价值**:用户写「对比 @A 和 @B」时,A/B 在句中位置就是语义。`@fileName` 留原位让模型理解引用语境,比「全文堆前面 + 问题在后面」更自然。

**落地**:

- `src-tauri/src/commands/chat.rs`:`send_message` 删 `context_texts` 全文读取 + `text_prompt_with_context` 调用 + Sources chunk + 28MiB 文档体积兜底(改为只校验图片体积)。新逻辑:`mounted_paths` → 查 `documents` 表得 `(id, name)` → user message 尾部追加 `<mounted-documents>` 注脚。`persist_turn` 落库仍存原文(不含注脚)。
- `crates/memory/src/documents.rs`:`read_document_by_path` 返回值加 `id`(5 元组),供后端构建注脚。
- `crates/agent-core/src/chat.rs`:删 `ContextText` struct + `text_prompt_with_context` 函数 + `SourceRef` struct + `StreamKind::Sources` 变体(死代码清理)。
- `src/components/chat/MentionMenu.tsx`:选中后保留 `@fileName` 文本在原位(不删 `@`)。
- `src/lib/mention.ts`:`resolveMentionedPaths(text, mounted)` 解析文本 `@<name>` token 匹配已挂载文档得 path 列表(以文本实际出现的 @ 为准,用户删掉 `@fileName` 即不引用)。
- `src/components/chat/Composer.tsx`:`send` 时调 `resolveMentionedPaths`(不再传全部挂载的 path)。
- Citation 去强制化:prompt 不再要求 `[n]` 标注;`remarkCitations.ts` 保留(模型可自愿用 `[n]`),`MarkdownText` 的 `CitationMark` title 改为「挂载文档 n」。
- Inspector 侧栏:展示「本会话挂载的文档列表」(从 `conversation_documents` 表读),不再展示 Sources。工具检索记录在会话区 ToolCallCard 展示。

**否决项**:

- `@` 挂载走全文注入(`context_texts` + `text_prompt_with_context`)——撑爆 context、每轮重发、与工具检索割裂,已推翻
- system prompt 塞文件清单/摘要——破坏 prefix cache(用户每上传新文件就失效整个前缀)
- 给挂载文档预生成摘要——batch 上传多文档时延迟高,且文件名已足够模型决策
- OpenAI 式自动注入「用户上传了文件」提示——被业界吐槽为 bad pattern,过度触发工具
- 强制 `[n]` Citation 角标——agentic search 下工具检索的文档无编号,强制 `[n]` 会让模型困惑;改为自然语言引用文档名

### 决策 18:IPC 边界 BigInt 公约——`#[specta(type = specta_typescript::Number)]` 逐字段注解(三期)

**决策**:`#[derive(specta::Type)]` 的 struct/enum 字段含 `u64/i64/usize/isize/i128/u128/f128` 时,逐字段加 `#[specta(type = specta_typescript::Number)]` 注解导出为 TS `number`,不手写 newtype 绕行(除非该 newtype 有独立领域语义,如 `memory::Timestamp`)。

**背景**:`specta-typescript 0.0.12`(依赖 `specta =2.0.0-rc.25`)**硬编码禁止**这些 64 位整数导出为 TS 类型(`primitives.rs` 无条件 `return Err(bigint_forbidden)`),理由是 JS `number` 只有 53 位精度。这是**全有或全无**约束:任一字段触发,整个 `Builder.export()` 失败。

**官方方案**(0.0.12 `error.rs` 顶部文档列了 5 种迁移路径,按推荐度):

1. 用支持 BigInt 的框架(暂无)
2. 改用更小整数类型(u32/i32,值域允许时)
3. 序列化为 string(`#[specta(type=String)]` + `#[serde(with=...)]`,无损但需胶水)
4. **逐字段接受精度损失:`#[specta(type = specta_typescript::Number)]`**(✅ 采用)
5. `specta_util::Remapper` 全局重映射(仅 `serde_json::Value` 等无法逐字段改时)

**方案 4 原理**:`specta_typescript::Number` 是内置 OpaqueReference,`primitives.rs` 对其走 bypass 路径直接输出 `"number"`,不触发 bigint 检查。Rust 字段类型保持 `u64`(serde 传 JSON number),仅导出元数据声明「此值 < 2^53,接受 number 降级」。

**落地**:

- 各 crate(`memory`/`agent-core`/`federation`)加 `specta-typescript = "0.0.12"` 依赖(纯导出元数据,不破坏业务核心解耦)
- 计数/大小类字段用注解:`MessageRow.prompt_tokens`(`Option<u64>`)、`TokenUsage.{input,output,total}_tokens`、`ProviderConfig.context_window`、`DataSourceSummary.{table_count,row_count_estimate}`、`QueryResult.{row_count,elapsed_ms}`、`IngestProgress.file_size`、`McpServerStatus.tool_count` 等
- `memory::Timestamp(i64)` newtype 保留(领域语义:unix ms 时间戳,已实现 ToSql/FromSql),不降级为注解
- **命令参数不支持注解**(specta derive macro 不处理函数参数):`#[tauri::command]` 的 bigint 参数用 `u32`/`i32` + `as usize` 转换(如 `read_document` 的 offset/limit、`execute_federation_query` 的 limit)

**否决项**:

- 手写 newtype 绕 bigint(ContextWindow/TokenCount/FileSize/RowCount 等)——能工作但冗余、非官方,曾误用后全部回退(历史教训:未读 `error.rs` 顶部官方文档就断定"无配置开关",凭 `primitives.rs` 单点信息自造 workaround)
- `specta_util::Remapper` 全局重映射——仅 `serde_json::Value` 内含 bigint 时用,常规场景过度
- JS BigInt 类型——webview 兼容性差(tauri-specta issue #6)

---

## 四、最终架构

```
┌─────────────────────────────────────────────────────────┐
│  表现层 (WebView)                                         │
│  React 19 + TS + Tailwind + shadcn/ui + Zustand          │
├─────────────────────────────────────────────────────────┤
│  IPC (Tauri command + event)                             │
├─────────────────────────────────────────────────────────┤
│  crates/ (全 Rust 业务核心,平台无关)                      │
│                                                         │
│  agent-core/                                            │
│    ├─ rig-core agent loop + AgentTool trait              │
│    │  (关闭 pdf/epub feature,不用 loaders)              │
│    ├─ rmcp (MCP client)                                 │
│    ├─ reqwest + rustls-tls                              │
│    └─ 多模态:Rig 原生 UserContent::Image/Audio          │
│                                                         │
│  ingest/  (统一摄取管道)                                  │
│    ├─ DocumentParser trait + dispatcher(按 MIME 路由)    │
│    ├─ Document{ text, chunks, tables, multimodal_parts,  │
│    │            meta } + 统一错误枚举                     │
│    ├─ parsers/                                          │
│    │    PDF:      pdfium-render(FFI 绑定 + 预编译动态库)  │
│    │    Office:   office_oxide(DOCX/PPTX/老格式)         │
│    │    XLSX读:   calamine                              │
│    │    XLSX写:   rust_xlsxwriter                       │
│    │    eBook:    rbook                                 │
│    │    图片:     image + base64(输出 multimodal_part)   │
│    │    文本/MD:  pulldown-cmark + std                  │
│    │    CSV:      csv                                   │
│    │    JSON:     serde_json                            │
│    │    压缩包:   zip + tar(递归+深度限制+流式)           │
│    └─ enhance/(二期)                                    │
│         ├─ 复杂度检测(cheap pass)                       │
│         ├─ 页面渲染→图片                                │
│         └─ VLM 重解析(调 Rig 多模态)                    │
│                                                         │
│  memory/  (统一存储)                                     │
│    └─ rusqlite(bundled) + jieba FTS5                     │
│       会话 / 向量 / 元数据 / 本体 同一 .db 文件            │
│                                                         │
│  agent-core 文件检索工具(一期收尾,决策 15)              │
│    ├─ document_tools(list/search/read,DynamicTool)      │
│    └─ 复用 MCP ToolServerHandle + add_dynamic_tool      │
│                                                         │
│  agent-core MCP(二期 A3)                                │
│    ├─ McpManager(rmcp 1.x client + 连接管理)           │
│    ├─ build_dynamic_tool(自实现 rig 0.41 DynamicTool 桥接)          │
│    └─ memory_bridge(ConversationMemory + CompactingMemory 自动压缩)│
│                                                         │
│  ontology/  (本体建模 + 元数据,三期)                    │
│    ├─ ObjectType / LinkType / ActionType 模型           │
│    ├─ SQLite 表族(复用 memory 的 rusqlite)             │
│    └─ TextQL→SQL 编译器(sqlparser-rs)                  │
│                                                         │
│  federation/  (联邦查询,三期)                            │
│    ├─ DataFusion 引擎(单进程内嵌)                      │
│    ├─ TableProvider:MySQL/PG(sqlx) / CSV / Excel       │
│    ├─ schema 浏览(information_schema)                 │
│    └─ 查询结果→Arrow RecordBatch→JSON/表格渲染         │
├─────────────────────────────────────────────────────────┤
│  平台层 (Tauri 官方 plugins,纯 Rust)                     │
│  fs / dialog / clipboard / notification / global-shortcut│
└─────────────────────────────────────────────────────────┘

外部依赖(显式、按定义不可避免):
  · 模型 API(云端 LLM/VLM,或用户自配 Ollama)
  · MCP server(用户按需接入)
  · 数据源(用户注册的 MySQL/PG 等,三期联邦查询)
```

---

## 五、最终依赖清单

> 版本号经 crates.io 实时核实(2026-07),`^` 锁主版本区间。`rig 0.41` 发布节奏快(约每月 2 个 minor),第三节 ADR 中涉及 rig-core 的具体 API 名称(如 `UserContent::Image`、loaders feature 等)落地时以 0.41 官方文档为准核对。

```toml
# crates/agent-core
[dependencies]
rig = { version = "0.41", default-features = false, features = ["reqwest","derive","rustls","memory","agent"] }  # agent 框架(启用 agent+memory feature)
rig-memory = { version = "0.41", default-features = false }  # 上下文管理策略(SlidingWindow/TokenWindow/CompactingMemory,见决策 16)
rmcp = "1.8"                # MCP client(锁定 1.x;见决策 14,不用 3.0-beta)
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.13", default-features = false, features = ["rustls-tls", "json", "stream"] }
serde = { version = "1", features = ["derive"] }

# crates/ingest
[dependencies]
# PDF
pdfium-render = { version = "0.9.3", default-features = false, features = ["thread_safe", "pdfium_7881"] }
                            # Chrome PDFium 的 FFI 绑定(MIT/Apache-2.0);thread_safe 仅 impl Send+Sync
                            # 不串行化 FFI 调用,本项目用进程级 Mutex 串行化(见决策 5)
                            # pdfium_7881 必须与预编译 PDFium 版本严格一致(chromium/7881)

# Office
office_oxide = "0.1.8"      # DOCX 读/写 + PPTX + 老格式 DOC/XLS/PPT(统一 IR)
calamine = "0.36"           # XLSX 读(类型/公式保真)
rust_xlsxwriter = "0.96"    # XLSX 写

# eBook
rbook = "0.7"               # ePub 2/3

# 图片(多模态输入)
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "webp", "bmp", "gif"] }
base64 = "0.23"

# 文本结构化
csv = "1.4"
pulldown-cmark = "0.13"     # Markdown 结构
serde_json = "1.0"

# 压缩包(递归 + 深度限制 + 流式)
zip = "8"
tar = "0.4"

# 通用
serde = { version = "1", features = ["derive"] }
thiserror = "2"

# crates/memory
[dependencies]
rusqlite = { version = "0.39", features = ["bundled", "load_extension"] }
jieba-rs = "0.9"               # FTS5 中文分词(jieba 词语分词,见决策 15)
rusqlite-ext = "0.39"           # FTS5 tokenizer 注册(自实现分句版,见决策 15)
sqlite-chinese-stopword = "0.1" # jieba 停词表(自实现 tokenizer 用)
sqlite-english-stemmer = "0.1"  # 英文词干提取(自实现 tokenizer 用)

# crates/ontology(三期:本体建模 + TextQL)
[dependencies]
sqlparser = "0.62"          # ANSI SQL:2011 解析/生成,TextQL→SQL 后端
serde = { version = "1", features = ["derive"] }
serde_json = "1.0"           # 本体属性 JSON 字段
thiserror = "2"
memory.workspace = true     # 复用 rusqlite 连接

# crates/federation(三期:联邦查询)
[dependencies]
datafusion = "54"           # 单进程内嵌查询引擎(Apache-2.0)
sqlx = { version = "0.9", default-features = false, features = ["mysql", "postgres", "runtime-tokio-rustls", "json"] }
                            # 纯 Rust + rustls,连 MySQL/PG
arrow = "56"                # DataFusion 内存格式(查询结果)
async-trait = "0.1"         # TableProvider trait
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tokio.workspace = true
ontology.workspace = true   # 本体元数据驱动查询重写
memory.workspace = true     # 数据源连接信息
```

> 落地时以 crates.io 最新稳定版为准;`office_oxide`/`pdfsink-rs` 等新库需做兼容性自测(见决策 4/5)。

---

## 六、工程结构

```
onto-studio/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs          # tauri::Builder + plugin 注册
│   │   ├── commands/        # #[tauri::command] 薄封装
│   │   ├── state.rs         # AppState
│   │   └── events.rs        # 流式事件 emit
│   ├── Cargo.toml
│   └── tauri.conf.json
├── crates/
│   ├── agent-core/          # Rig + MCP + provider + 多模态
│   ├── ingest/              # 多模态摄取管道 + VLM 增强
│   │   ├── src/
│   │   │   ├── lib.rs       # DocumentParser trait + ingest()
│   │   │   ├── dispatcher.rs
│   │   │   ├── document.rs  # Document 结构
│   │   │   ├── error.rs     # 统一错误枚举
│   │   │   ├── parsers/     # 各格式实现
│   │   │   ├── enhance/     # 二期:VLM 增强
│   │   │   └── security.rs  # zip 炸弹防护等
│   │   └── tests/           # 各格式样本回归 + 兼容性自测
│   ├── memory/              # SQLite + jieba FTS5(会话/消息/文档全文)
│   │   ├── src/
│   │   │   ├── lib.rs       # Memory 句柄 + schema + jieba tokenizer 注册
│   │   │   ├── documents.rs # documents 表 + FTS5 索引 + Repo(upsert/search/read/index_document)
│   │   │   ├── jieba_tokenizer.rs # 分句版 jieba FTS5 tokenizer(避免 DAG O(n²) 退化)
│   │   │   ├── repo.rs      # 会话/消息 CRUD
│   │   │   └── ...
│   │   └── tests/
│   ├── ontology/            # 三期:本体建模 + TextQL 编译器
│   │   ├── src/
│   │   │   ├── lib.rs       # OntologyService 入口
│   │   │   ├── model.rs     # ObjectType/LinkType/ActionType
│   │   │   ├── store.rs     # SQLite 表族 CRUD
│   │   │   └── textql.rs    # TextQL→SQL 编译(sqlparser-rs)
│   └── federation/          # 三期:DataFusion 联邦查询
│       ├── src/
│       │   ├── lib.rs       # FederationService 入口
│       │   ├── provider/    # TableProvider:MySQL/PG/CSV/Excel
│       │   ├── schema.rs    # information_schema 浏览
│       │   └── error.rs
│       └── tests/           # 各数据源连接回归
├── src/                     # React 前端
│   ├── components/
│   ├── hooks/
│   └── stores/
└── package.json
```

---

## 七、支持的输入格式(最终)

| 格式 | 库 | 能力 | 许可证 |
| --- | --- | --- | --- |
| PDF | pdfium-render + 预编译 PDFium 动态库 | 文本/坐标/渲染(扫描件二期走 VLM);CJK/CID 零乱码 | MIT/Apache-2.0(pdfium-render)/BSD-3-Clause(PDFium 库本身) |
| DOCX | office_oxide | IR:标题/段落/列表/表格/页眉页脚/图片 | MIT/Apache |
| XLSX 读 | calamine | 单元格类型/公式/合并 | MIT |
| XLSX 写 | rust_xlsxwriter | 新建写 | MIT/Apache |
| PPTX | office_oxide | slide 分节/表格/备注 | MIT/Apache |
| DOC/XLS/PPT(老格式) | office_oxide | 独家支持 | MIT/Apache |
| ePub | rbook | ePub 2/3 读/构建/编辑 | Apache |
| 图片 | image + base64 | 解码→多模态 part(交 VLM 理解) | MIT |
| CSV | csv | 结构化 | Unlicense/MIT |
| Markdown | pulldown-cmark | 标题/列表/代码块 | MIT |
| JSON | serde_json | — | MIT/Apache |
| 纯文本/code | 标准库 | — | — |
| 压缩包 | zip + tar | 递归(深度限制+流式) | MIT |

---

## 八、隐含依赖复查(最终状态)

| 项 | 状态 |
| --- | --- |
| Tesseract / OCR | ✅ 不纳入(由 VLM API 替代) |
| FFmpeg / 视频 | ✅ 不纳入 |
| LibreOffice | ✅ 否决 LiteParse,不引入 |
| PDFium | ⚠️ 破例引入(决策 5):预编译动态库随 Tauri 资源打包,运行时由程序加载,用户零安装。纯 Rust(lopdf/pdfsink)对中文 CID CMap 解码失败的工程妥协 |
| Poppler / mupdf | ✅ 不引入(由 pdfium 替代) |
| OpenSSL | ✅ 用 rustls 替代 |
| 独立向量库 | ✅ 用 sqlite-vec 替代 |
| 本地 VLM/Whisper 权重 | ✅ 不纳入,走 API/用户自配 |
| SQLite(libsqlite3) | ✅ bundled 静态编译(含 FTS5) |
| WebView2/WKWebView/WebKitGTK | ⚠️ Tauri 基础,平台自带,无法消除 |
| 构建期 C 编译器 | ⚠️ 仅 SQLite 需要,随源码编译 |

**结论**:除 Tauri WebView(平台自带)、构建期 C 编译器(仅 SQLite)、PDFium 预编译动态库(随安装包分发,用户零安装,见决策 5 破例)外,运行时零额外依赖。符合原则 1 的精神。

---

## 九、落地路线

1. **一期 MVP**:脚手架 → Rig 接云端多模态 provider → 基础对话+流式 → ingest 核心 parser(PDF/Office/文本)→ 图片输入 → SQLite 会话
2. **二期**:~~VLM 增强解析(复杂度检测+扫描件)~~ → ~~向量检索 RAG(已砍,改文件工具+FTS5,见决策 15)~~ → ✅ MCP 工具系统(决策 14) → ✅ 上下文管理(rig 0.41 原生 ConversationMemory+CompactingMemory,决策 16) → ✅ 文件检索工具(决策 15,jieba FTS5 + agent 工具)
   - VLM 增强解析(A1)暂缓:无可用 VLM 模型,待四期 Ollama 本地 VLM 或接 GPT-4o 后再启
   - 向量 RAG(A2)砍掉:一期改用文件工具 + FTS5(agentic search),向量检索留四期
   - 实际落地顺序:A3 MCP → ~~A2 RAG~~ → B1 token 预算 → B2 自动压缩 → 文件工具(见 PROGRESS.md)
3. **三期(本体设计)**:数据源注册(MySQL/PG/CSV/Excel)→ 本体建模(ObjectType/LinkType/ActionType)→ DataFusion 联邦查询 → TextQL 自然语言转 SQL → Agent 工具化(让 LLM 调用联邦查询)
4. **四期(可选)**:本地 Ollama + Qwen2.5-VL 离线高精度 → 移动端适配(砍 sidecar,改纯库+云端)

> 三期「本体设计」是 onto-studio 从「Agent 工作台」进化为「数据知识图谱工作台」的关键阶段。核心价值:用户注册异构数据源 → 本体层定义语义模型 → Agent 通过 TextQL 自主跨源查询。前置依赖二期 MCP 工具系统(联邦查询作为 MCP tool 暴露给 Agent)。

---

# 第二部分:前端架构设计

> 目标:给用户最好的体验与易用性,对标 Claude Desktop / Cursor / Perplexity 等成熟产品。本部分补全第一节架构图里仅一行带过的「表现层 (WebView)」,与后端的 ADR 风格保持一致。

## 十、前端设计目标与对标

### 10.1 体验北极星

一款「打开即用、键鼠行云、移动端顺手」的本地 Agent 工作台。具体可衡量的体验指标:

- **首屏 < 200ms**:冷启动后主窗口可交互时间(Tauri + Vite + 路由级代码分割)
- **流式首字 < 300ms**:从用户回车到第一个 token 渲染
- **万条消息不卡**:单会话 1 万条消息滚动稳定 60fps(虚拟列表 + 流式增量渲染)
- **零鼠标完成常用动作**:新建会话 / 切换会话 / 发送 / 中断 / 重试 / 搜索 / 摄入文件 全部有快捷键
- **拖即摄**:文件拖入窗口任意位置即触发摄取,无需先点上传按钮
- **离线可用**:无网络时仍可浏览历史会话、编辑草稿、管理文件库

### 10.2 对标产品与借鉴点

| 产品 | 借鉴点 | 在本应用的落地 |
| --- | --- | --- |
| **Claude Desktop** | Quick Entry 全局快捷键双击 Option 唤起;三模式(Chat/Cowork/Code)分场景 | 全局快捷键唤起 Quick Prompt 浮窗;主窗口三视图(对话/文件库/设置) |
| **Cursor** | `@` 唤起上下文选择器挂载文件/文件夹;Agent 工具调用过程可视化 | 输入框 `@` 触发上下文挂载菜单;tool call 可折叠卡片 + reasoning 折叠块 |
| **Perplexity** | 内联编号 citation + hover 预览卡 + 底部/侧栏 source 列表 | RAG 回答内联 `[1][2]` marker,hover 出源文档片段卡,侧栏 Sources 全审计 |
| **Lobe Chat** | 多 provider 配置中心 + 会话侧栏 + 命令面板 ⌘K | 设置页 provider 矩阵;⌘K 命令面板统一导航/动作/搜索 |
| **ChatGPT Desktop** | 多 webview 独立窗口、Quick Entry、canvas/代码块富交互 | Tauri 多窗口(主窗 + Quick Prompt + 设置独立窗);代码块复制/语言切换/下载 |

### 10.3 与后端原则的对齐

前端同样遵循第二部分的核心原则,具体映射:

- **原则 4(业务核心与平台解耦)**:前端不直接调 Rust 业务库,全部经 IPC 契约;前端自身也分层,UI / state / ipc / domain 四层单向依赖
- **原则 5(外部服务最小化且显式)**:前端除加载模型 API 配置和 MCP server 列表外,不内置任何网络请求逻辑——所有 LLM/VLM 调用走 Rust agent-core,前端只消费流式事件
- **轻量化**:前端打包产物目标 < 1.5MB(gzipped),不内嵌大依赖;Markdown 渲染按需加载语法高亮语言包

---

## 十一、技术栈选型(前端 ADR)

### 决策 F1:React 19 + TypeScript + Vite

**决策**:React 19(并发特性 + use 钩子)+ TS 严格模式 + Vite 8。
**理由**:React 19 的 `use()` 与 transition 让流式增量渲染更顺;Vite 是 Tauri 官方推荐模板;TS strict 保 IPC 类型安全。
**否决项**:Svelte/Solid——生态与 shadcn/ui 不匹配,团队迁移成本高。

### 决策 F2:Tailwind v4 + shadcn/ui(含 2026 新 chat 组件集)

> ⚠️ **已被决策 F12 修订**(2026-07-29):chat 组件集改用 `@assistant-ui/react`(Primitives 模式),见 F12。本决策的 Tailwind v4 + shadcn/ui(非 chat 部分)仍然有效。

**决策**:Tailwind CSS v4 + shadcn/ui,**优先采用 shadcn 2026-06 发布的 chat 组件集**(`MessageScroller` / `Message` / `Bubble` / `Attachment` / `Marker`)。
**理由**:

- `MessageScroller` 内建流式跟随、新 turn 锚定、历史 prepend 不跳屏、跳转任意消息——正是万条消息场景的核心需求,自研成本高
- `Marker` 用于内联 citation 标记,与 Perplexity 式引用渲染天然契合
- shadcn 复制源码模式(非 npm 黑盒),便于深度定制 agent 场景的 tool call / reasoning 扩展
**否决项**:assistant-ui(功能全但抽象重、定制成本高)、MUI X Chat(商业授权)、自研聊天原语(重复造轮子)。

### 决策 F3:状态管理三层分离(Zustand + TanStack Query + 组件本地 state)

**决策**:严格区分三类状态,各归其库:

| 状态类型 | 归属 | 例子 |
| --- | --- | --- |
| **服务端状态**(Rust 返回的数据) | TanStack Query | 会话列表、消息历史、文件库、provider 配置 |
| **全局 UI 状态**(跨页面、跨组件) | Zustand | 当前会话 ID、侧栏折叠、主题、Quick Prompt 开关、输入草稿 |
| **组件本地状态** | useState/useReducer | 输入框聚焦、下拉开关、拖拽悬停态 |

**理由**:TanStack Query + Zustand 是 Tauri 社区验证的成熟组合(tauri-desktop-starter / argus 等模板均采用),职责清晰避免状态蔓延;TanStack Query 的缓存/失效/乐观更新天然适配 IPC command 的 request-response 语义。
**否决项**:Redux(样板重)、单一 Zustand 管全部(服务端缓存与 UI 状态混杂,失效逻辑难写)。

### 决策 F4:路由用 TanStack Router(文件式 + 全类型)

**决策**:TanStack Router,文件式路由,全类型推断,代码分割开箱即用。
**理由**:Tauri 桌面应用无 SSR,纯客户端路由,TanStack Router 的类型安全与 `loader` 模式契合「进入会话前预取历史」场景;与 TanStack Query 同生态共享类型。
**否决项**:React Router(类型推断弱)、不用路由(单页状态机切换,会话深链与后退栈丢失)。

**落地注**(2026-07-29):一期实际用 `App.tsx` 状态机切换未启用路由,与决策矛盾。现已落地文件式路由(`src/routes/` + `@tanstack/router-plugin` 自动生成路由树),主窗路由 `/` `/chat/$id` `/library` `/settings`,Quick Prompt 独立窗 `/quick`(见 §17.1)。历史 `App.tsx` 状态机逻辑下沉到 `routes/__root.tsx` 的快捷键 effect + 路由 layout。

### 决策 F5:IPC 类型安全桥用 tauri-specta

**决策**:`tauri-specta` 从 Rust `#[tauri::command]` 与 event 自动生成 TypeScript 绑定,前端 `invoke`/`listen` 全类型。
**理由**:与后端原则 4「Tauri 只做薄层」一致——薄层也要类型安全;Rust 侧 struct 加 `#[derive(specta::Type)]`,构建期生成 `bindings.ts`,杜绝手写类型漂移;event 也可生成类型(Claude/ChatGPT 流式场景刚需)。
**否决项**:手写 TS 类型(易腐化)、ts-rs(需额外胶水,不及 tauri-specta 与 Tauri 命令系统深度集成)。
**版本与 BigInt 公约**:项目用 `tauri-specta 2.0.0-rc.25` + `specta 2.0.0-rc.25` + `specta-typescript 0.0.12`。0.0.12 硬编码禁止 64 位整数导出 TS(全有或全无约束),逐字段用 `#[specta(type = specta_typescript::Number)]` 注解绕行——详见决策 18。命令参数(函数参数)不支持注解,改用 `u32`/`i32` + `as usize`。

### 决策 F6:流式渲染用 Streamdown(替代 react-markdown)

**决策**:Markdown 渲染用 `streamdown`(Vercel 出品,专为流式设计),内置 Shiki 代码高亮 + KaTeX + Mermaid + 流式光标 + 未闭合块容错。
**理由**:传统 react-markdown 每来一个 token 全量重解析,长回复性能崩塌;streamdown 针对不完整 Markdown 块做了流式容错,正是 agent 流式输出痛点。与 shadcn chat 组件可组合(`@assistant-ui/react-streamdown` 已验证集成路径)。
> ⚠️ **已被决策 F13 修订**(2026-07-29):流式 Markdown 改用 `@assistant-ui/react-markdown`(react-markdown 基座 + 官方调优的紧凑排版 + dot.css 流式光标),见 F13。`streamdown` 从依赖移除。

**否决项**:react-markdown + 手拼 remark/rehype 插件(流式体验差、需自己处理未闭合语法)、MDX(安全面大、不适合渲染不可信 LLM 输出)。

### 决策 F7:长列表虚拟化用 @tanstack/react-virtual(chat 反向锚定模式)

**决策**:消息列表用 `@tanstack/react-virtual` 的 **end-anchored / reverse** 模式。
**理由**:TanStack Virtual 在 2025 专门为 chat 场景重构了反向锚定语义——新消息追加在末尾且视口跟随、历史 prepend 时视口钉住当前行不跳屏、流式输出增长时仅在用户已在底部才跟随。这正是 Claude/ChatGPT 滚动契约,自研极易出 bug。
**否决项**:react-virtuoso(可用但 API 较黑盒)、不虚拟化(万条消息必卡)。

### 决策 F8:持久化分层——服务端数据进 SQLite,UI 偏好进 tauri-plugin-store

**决策**:

- **会话/消息/文件库/向量**:走 IPC 落 Rust 侧 SQLite(单一真相源,跨窗口一致)
- **UI 偏好**(侧栏宽度、主题、最近会话 ID、输入草稿):用 `@tauri-store/zustand` 持久化到磁盘(Rust 侧管理,跨窗口同步)
- **不使用 localStorage**:容量小(3-5MB)、仅 JS 可访问、多 webview 不同步

**理由**:桌面应用多窗口(主窗 + Quick Prompt + 设置)需共享状态,localStorage 各 webview 隔离会撕裂;tauri-plugin-store 由 Rust 持有,天然跨窗口一致并支持 saveOnChange。
**否决项**:localStorage(前述缺陷)、IndexedDB(浏览器 API,在 Tauri 多窗口同样隔离、且与 Rust 侧 SQLite 形成双存储)。

### 决策 F9:命令面板 ⌘K 用 cmdk

**决策**:`cmdk`(Paco Coursey)构建全局命令面板,统一「导航 / 动作 / 内容搜索」三类入口。
**理由**:Linear/Notion/Raycast/Vercel 验证的键盘主义标配;cmdk 无样式可组合,与 shadcn 的 `Command` 组件同源(shadcn Command 即 cmdk 封装)。内容搜索(搜历史会话/消息)走 IPC 全文检索,结果在面板内分组呈现。
**否决项**:自研(Paco 的实现已足够好且无障碍达标)。

### 决策 F10:文件拖拽用 react-dropzone + Tauri 原生 drop 事件双通道

**决策**:`react-dropzone` 提供 UI 层拖拽视觉反馈,Tauri 窗口级 `onDragDropEvent` 提供真实文件路径(浏览器 drop 事件拿不到本地路径)。
**理由**:WebView 的 HTML5 drop 出于安全只能拿 File 对象(需先读入内存),而 Tauri 的 `tauri://drag-drop` 事件直接给本地路径,可交给 Rust 流式摄取避免前端 OOM。两者结合:dropzone 管视觉,Tauri 事件管路径。移动端无拖拽,退化为文件选择器(`@tauri-apps/plugin-dialog`)。

### 决策 F11:移动端用响应式断点 + 底部 Sheet,不做双套代码

**决策**:单套 React 代码,通过 Tailwind 断点 + shadcn 的 Dialog/Drawer 自适应原语(桌面 Dialog → 移动 Drawer)适配。
**理由**:桌面优先原则下,移动端是降级场景;双套代码维护成本翻倍且易行为不一致。会话列表在移动端从侧栏变 Drawer 抽出,设置页从居中 Dialog 变底部 Sheet,命令面板在移动端改为顶部下拉。文件摄入在移动端仅保留选择器入口。

### 决策 F12:chat 组件集改用 @assistant-ui/react(修订 F2)

**决策**:对话流式渲染的 chat 原语改用 `@assistant-ui/react ^0.15` 的 **Primitives 模式**(`ThreadPrimitive` / `MessagePrimitive` / `MessagePartPrimitive` / `AssistantRuntimeProvider` + `useExternalStoreRuntime`),而非 F2 原定的 shadcn 2026 chat 组件集(`MessageScroller`/`Message`/`Bubble`/`Marker`)。

**理由**(对 F2 否决理由的反转,基于一期实际落地经验):

- **Primitives 模式非黑盒**:assistant-ui 的 Primitives 是 headless 行为内核(类似 Radix),样式完全自写,定制成本不高于 shadcn 复制源码。F2 当初否决的「抽象重」针对的是 assistant-ui 的高阶 Runtime/Thread 预设组件,Primitives 层不在此列。
- **滚动契约零成本**:`ThreadPrimitive.Viewport` 原生处理流式跟随、新 turn 锚定、历史 prepend 不跳屏、回到最新——F2 原本指望 shadcn `MessageScroller` 提供的能力,assistant-ui 已内置且更成熟(流式 memo、part 级增量渲染)。
- **part 模型天然适配 agent 场景**:`MessagePartPrimitive` 按 part 类型分派(text/reasoning/tool-call/image),reasoning 自动可折叠、tool-call 有 fallback 卡片——正是 F2 想用 shadcn 定制的能力,assistant-ui 已抽象好。
- **ExternalStoreRuntime 与现有架构契合**:`useExternalStoreRuntime({messages, convertMessage, isRunning, onCancel})` 把 TanStack Query 缓存 + useChat 流式状态桥接成 runtime,发送逻辑保留在 Composer(调 useChat),runtime 只管渲染——与 F3 三层状态分离不冲突。
- **shadcn chat 组件集 2026-06 发布时 API 仍在变动**(`Marker`/`Attachment` 接口未稳定),assistant-ui 0.15 已是稳定 GA。

**落地约束**(避免重蹈 F2 否决的高阶抽象坑):

- **只用 Primitives + ExternalStoreRuntime**,不用 assistant-ui 的预设 `Thread`/`Composer` 高阶组件(那些才是 F2 否决的「抽象重」部分)。
- **样式自写**:所有视觉用 Tailwind + 自定义 className,不依赖 assistant-ui 的内置主题,保持与 shadcn 其余组件视觉一致。
- **part 分派集中在 `Thread.tsx` 的 `ASSISTANT_PARTS_COMPONENTS`/`USER_PARTS_COMPONENTS` 模块作用域常量**,避免流式时重建引用导致 memo 失效(官方明确要求)。
- **citation(二期补)用自定义 part 类型 + `MessagePartPrimitive` 渲染**,不依赖 shadcn `Marker`(F2 原设计作废,citation 改走 assistant-ui part 模型,见后续 Citation 落地)。

**否决项**:

- shadcn 2026 chat 组件集——API 未稳定,且 Primitives 已覆盖其能力
- 自研聊天原语——重复造轮(滚动契约/part 分派/memo 都自写成本高)

**影响**:F2 中「shadcn chat 组件集」部分作废,shadcn/ui 其余原语(Dialog/Command/DropdownMenu 等)仍继续用。§20.1 的 shadcn chat 组件 API 表作废,改为以 assistant-ui Primitives API 为准。§20.7 MessageItem 解剖图改为 assistant-ui part 模型(reasoning/text/tool-call/image parts)。

### 决策 F13:流式 Markdown 改用 @assistant-ui/react-markdown(修订 F6)

**决策**:LLM 输出的 Markdown 渲染改用 `@assistant-ui/react-markdown ^0.14`(react-markdown 基座 + `MarkdownTextPrimitive` + `unstable_memoizeMarkdownComponents`),移除 `streamdown` 依赖。理由见 `src/components/chat/MarkdownText.tsx` 顶部注释(一期落地时已记录)。

**理由**(对 F6 的修订):

- **流式 memo 已具备**:`MarkdownTextPrimitive` 的 `defer` + `memoizeMarkdownComponents` 对已完成块缓存、仅未完成块重解析,与 streamdown 的 `remend` 引擎同语义,F6 担心的「react-markdown 全量重解析」在此方案下不成立。
- **官方调优排版**:`aui-md` + `dot.css` 提供紧凑间距与流式脉动光标,无需手写 CSS;streamdown 的排版需自行调。
- **与 F12 同生态**:assistant-ui-markdown 是 assistant-ui 的官方配套,part 模型无缝衔接;若用 streamdown 需走 `@assistant-ui/react-streamdown` 桥接,多一层。
- **代码块复制/语言标签内置**:`CodeHeader` + `useIsMarkdownCodeBlock` 开箱即用。

**落地约束**:

- **安全**(F6 原约束保留):LLM 输出按不可信内容,默认禁 raw HTML(react-markdown `rehype-raw` 不启用);外链 `rel=\"noopener noreferrer\"`,拦 `file://` 与钓鱼;代码块不执行;Mermaid/KaTeX 按需(当前未启用,后续需要时加 remark/rehype 插件)。
- **components 全部模块作用域定义**(`memoizeMarkdownComponents` 缓存),避免流式时子树重渲染。
- **Shiki 代码高亮暂未启用**(streamdown 内置 Shiki,本方案需自行接 `rehype-highlight` 或 `shiki` runner);一期代码块用纯 CSS,二期需要高精度高亮时再加。

**否决项**:

- streamdown——同生态不契合 F12,且排版需自调;从依赖移除
- react-markdown 裸用——缺流式 memo 与官方排版

**影响**:F6 作废。§20.2 的 Streamdown props 示例作废,改为 `MarkdownTextPrimitive` API。`streamdown` 从 `package.json` 依赖移除。

---

## 十二、前端分层架构

### 12.1 四层单向依赖

```
┌─────────────────────────────────────────────────────────┐
│  UI 层 (src/components, src/routes)                       │
│  React 组件 + shadcn 原语,只读 state/hooks,不含业务逻辑   │
├─────────────────────────────────────────────────────────┤
│  State 层 (src/stores, src/hooks)                         │
│  Zustand 全局 UI 状态 + TanStack Query 服务端状态缓存      │
│  + useChat 等领域 hook(封装状态机)                        │
├─────────────────────────────────────────────────────────┤
│  IPC 层 (src/lib/ipc)                                     │
│  tauri-specta 生成的类型化 invoke/listen 封装              │
│  + 流式 Channel 适配器(把 Tauri Channel 转 RxJS/AsyncIter)│
├─────────────────────────────────────────────────────────┤
│  Domain 层 (src/lib/domain)                               │
│  纯 TS 类型 + 工具函数(Message/ToolCall/Citation 建模)     │
│  与 Rust 侧 serde struct 一一对应(由 specta 生成)          │
└─────────────────────────────────────────────────────────┘
         ↑ 单向依赖:上层依赖下层,下层不反向引用
```

**硬约束**:`components/` 不得直接 `invoke`;`stores/` 不得直接 import 组件;`ipc/` 不得 import `stores/`。用 ESLint `no-restricted-imports` 规则在 CI 强制。

### 12.2 目录结构(前端部分细化)

```
src/
├── routes/                    # TanStack Router 文件式路由
│   ├── __root.tsx             # 主壳:三栏布局 + 命令面板挂载
│   ├── chat.$conversationId.tsx  # 对话视图(流式渲染主战场)
│   ├── library.tsx            # 文件库视图(已摄入文件管理)
│   └── settings.tsx           # 设置(provider/MCP/外观)
│
├── components/
│   ├── chat/                  # 对话相关(基于 shadcn chat 组件集扩展)
│   │   ├── ChatThread.tsx     # MessageScroller + 虚拟化
│   │   ├── MessageItem.tsx    # Message + Bubble + 角色/avatar
│   │   ├── MarkdownView.tsx   # Streamdown 封装
│   │   ├── CitationMarker.tsx # 内联 [1] marker + hover 卡
│   │   ├── SourcesPanel.tsx   # 侧栏/底部 source 审计列表
│   │   ├── ToolCallCard.tsx   # MCP 工具调用可折叠卡片
│   │   ├── ThinkingBlock.tsx  # reasoning 折叠块
│   │   └── Composer.tsx       # 输入框 + @ 上下文菜单 + 附件
│   ├── library/
│   │   ├── FileDropZone.tsx   # 全局拖拽落点 + Tauri 事件桥
│   │   ├── IngestStatusBoard.tsx # 摄取进度(队列/解析中/完成/失败)
│   │   └── FilePreview.tsx    # 文档预览(文本/表格/图片)
│   ├── shell/
│   │   ├── AppShell.tsx       # 三栏 resizable 布局
│   │   ├── Sidebar.tsx        # 会话列表 + 新建 + 搜索
│   │   ├── TitleBar.tsx       # 自定义标题栏(无边框窗)
│   │   └── CommandPalette.tsx # ⌘K 命令面板
│   ├── quick-prompt/          # Quick Entry 浮窗(独立 webview)
│   │   └── QuickPrompt.tsx
│   └── ui/                    # shadcn 原语(复制源码,非 npm)
│
├── stores/                    # Zustand(全局 UI 状态)
│   ├── ui-store.ts            # 侧栏/主题/当前会话
│   ├── composer-store.ts      # 输入草稿(按会话 ID 分键)
│   └── quick-prompt-store.ts
│
├── hooks/
│   ├── useChat.ts             # 对话状态机(send/stream/stop/retry)
│   ├── useIngest.ts           # 摄取进度订阅
│   ├── useConversations.ts    # TanStack Query:会话列表 CRUD
│   ├── useLibrary.ts          # TanStack Query:文件库
│   └── useGlobalShortcut.ts   # 全局快捷键注册
│
├── lib/
│   ├── ipc/                   # tauri-specta 生成 + 适配
│   │   ├── bindings.ts        # (自动生成,勿手改)
│   │   ├── commands.ts        # 命令式 IPC 封装
│   │   ├── channels.ts        # Tauri Channel → AsyncIterable 适配
│   │   └── events.ts          # 事件 listen 封装
│   ├── domain/                # 纯类型与工具
│   │   ├── message.ts         # Message/Part/Role 建模
│   │   ├── tool.ts            # ToolCall/ToolResult
│   │   └── citation.ts
│   └── markdown/              # Streamdown 插件配置
│
└── styles/
    └── globals.css           # Tailwind v4 + 主题 token
```

---

## 十三、IPC 契约设计(前后端边界)

### 13.1 三种通信通道与适用场景

Tauri v2 提供三种 IPC 机制,前端按语义选用:

| 机制 | 语义 | 适用 | 本应用用法 |
| --- | --- | --- | --- |
| **Command**(`invoke`) | request-response | 一次性取数/动作 | 列会话、发消息(非流式)、删文件、存配置 |
| **Channel**(`Channel<T>`) | 单向流式,Rust→JS 多次推送 | 流式产出、进度更新 | **LLM 流式 token、摄取进度、tool call 步骤** |
| **Event**(`emit`/`listen`) | 广播 pub/sub,多消费者 | 后端主动通知、跨窗口 | 摄取完成广播、MCP server 状态变更、Quick Prompt 与主窗同步 |

**关键**:流式对话**优先用 Channel 而非 Event**。Channel 是点对点(一次 invoke 返回一个 channel),无全局监听器噪声,且 Tauri 对小包(≤8KB)走 fast-path(eval 直注)性能优于 event。Event 仅留给真正需要广播的场景。

### 13.2 核心命令契约(示例,Rust 侧定义、specta 生成)

```rust
// src-tauri/src/commands/chat.rs
#[derive(Serialize, Type)] pub struct SendMessageRequest {
    pub conversation_id: Uuid,
    pub content: UserContent,      // text / image[] / attached_file_ids[]
    pub model: ModelRef,
}

#[derive(Serialize, Type)] pub struct StreamChunk {
    pub message_id: Uuid,
    pub kind: StreamKind,          // TextDelta | ToolCallStart | ToolCallDelta
                                  // | ToolResult | ReasoningDelta | Citation | Done | Error
    pub payload: StreamPayload,
}

#[tauri::command] #[specta::specta]
pub async fn send_message(
    req: SendMessageRequest,
    on_chunk: tauri::ipc::Channel<StreamChunk>,
) -> Result<Uuid, AppError>;  // 返回 message_id,流式走 channel
```

前端 `useChat` 把 `Channel<StreamChunk>` 适配成 `AsyncIterable`,消费并写入 TanStack Query 缓存(乐观更新 + 增量 patch)。

### 13.3 摄取进度流

```rust
#[derive(Serialize, Type)] pub struct IngestProgress {
    pub job_id: Uuid,
    pub stage: IngestStage,        // Queued | Parsing{pct} | Chunking | Embedding | Done | Failed
    pub file_name: String,
    pub current: usize,
    pub total: usize,
    pub error: Option<String>,
}

#[tauri::command] #[specta::specta]
pub async fn ingest_files(paths: Vec<PathBuf>, on_progress: Channel<IngestProgress>)
    -> Result<Vec<Uuid>, AppError>;  // 返回生成的 document_ids
```

前端 `IngestStatusBoard` 订阅该 channel,展示 kanban 式状态板(队列/解析中/完成/失败),失败项可重试。

### 13.3a MCP 与 RAG 命令契约(二期新增)

```rust
// ── MCP 工具系统(决策 14) ──
#[derive(Serialize, Deserialize, Type)]
#[serde(tag = "kind")]
pub enum McpServerConfig {
    Stdio { id, name, command, args, env },
    Http { id, name, url, auth_token, headers },
}

#[tauri::command] #[specta::specta]
pub async fn set_mcp_servers(servers: Vec<McpServerConfig>) -> Result<Vec<McpServerStatus>, AppError>;
pub async fn get_mcp_servers() -> Result<Vec<McpServerConfig>, AppError>;
pub async fn list_mcp_tools() -> Result<Vec<McpToolDef>, AppError>;

// 工具调用走流式 StreamChunk 的 ToolCallStart/ToolCallResult 变体(决策 14),
// 不独立 command;tool_calls 为运行时状态不落库,前端 useChat state 收集。

// ── 文件库 + 挂载关联（决策 15 + 17）──
#[derive(Serialize, Deserialize, Type)]
pub struct DocumentSummaryDto { pub id: String, pub path: String, pub name: String, pub format: String, pub char_count: u32, pub created_at: i64 }

#[derive(Serialize, Deserialize, Type)]
pub struct MountedDocDto { pub path: String, pub name: String, pub format: String, pub char_count: u32, pub mounted_at: i64 }

// 文件库 + 挂载关联（决策 15 + 17）
#[tauri::command] #[specta::specta]
pub async fn list_all_documents() -> Result<Vec<DocumentSummaryDto>, AppError>;
pub async fn read_document(id: String, offset: Option<usize>, limit: Option<usize>) -> Result<Option<DocumentContentDto>, AppError>;
pub async fn delete_document(path: String) -> Result<bool, AppError>;
pub async fn mount_document(conversation_id: String, path: String) -> Result<(), AppError>;
pub async fn unmount_document(conversation_id: String, path: String) -> Result<usize, AppError>;
pub async fn list_mounted_documents(conversation_id: String) -> Result<Vec<MountedDocDto>, AppError>;

// `@` 挂载由 send_message 接收 mounted_paths，后端查 id+name 在 user message
// 尾部追加 <mounted-documents> 注脚（决策 17）。模型按需调 read_document 取全文。
// 文件工具（list/search/read_documents）挂到 agent，模型 agentic search。
```

### 13.4 错误统一建模

```rust
#[derive(Serialize, Type, thiserror::Error)]
pub enum AppError {
    #[error("ingest: {0}")] Ingest(IngestError),
    #[error("agent: {0}")] Agent(AgentError),
    #[error("memory: {0}")] Memory(MemoryError),
    #[error("provider: {0}")] Provider(ProviderError),  // 含 401/欠费/超时等可前端区分
    #[error("cancelled")] Cancelled,
}
```

前端按变体做差异化 UX:`Provider(401)` → 弹设置页引导补 key;`Cancelled` → 静默(用户主动中断);其他 → toast + 重试按钮。

### 13.5 本体与联邦查询契约(三期)

```rust
// ── 数据源 ──
#[derive(Serialize, Deserialize, Type)]
pub struct DataSourceConfig {
    pub id: Uuid,
    pub kind: DataSourceKind,        // MySQL | PostgreSQL | CSV | Excel
    pub name: String,
    pub connection: serde_json::Value, // {host,port,db,user} 凭证不返回前端
}

#[tauri::command] #[specta::specta]
pub async fn register_data_source(input: DataSourceConfig) -> Result<DataSourceConfig, AppError>;

#[tauri::command] #[specta::specta]
pub async fn test_data_source(input: DataSourceConfig) -> Result<SchemaSnapshot, AppError>;

#[tauri::command] #[specta::specta]
pub async fn browse_schema(source_id: Uuid) -> Result<SchemaSnapshot, AppError>;

// ── 本体 ──
#[derive(Serialize, Deserialize, Type)]
pub struct ObjectType {
    pub id: Uuid,
    pub name: String,
    pub source_id: Uuid,
    pub table: String,
    pub properties: Vec<PropertyMapping>, // {name, column, type, is_primary}
}

#[derive(Serialize, Deserialize, Type)]
pub struct LinkType {
    pub id: Uuid,
    pub name: String,
    pub from_type_id: Uuid,
    pub to_type_id: Uuid,
    pub join: JoinSpec,              // {from_col, to_col, source_id} 跨源 JOIN
}

#[derive(Serialize, Deserialize, Type)]
pub struct ActionType {
    pub id: Uuid,
    pub name: String,
    pub object_type_id: Uuid,
    pub sql_template: String,        // 参数化 SQL,如 UPDATE {table} SET is_vip=true WHERE id = $1
    pub params: Vec<ParamSpec>,
}

#[tauri::command] #[specta::specta]
pub async fn create_object_type(input: ObjectType) -> Result<ObjectType, AppError>;
#[tauri::command] #[specta::specta]
pub async fn create_link_type(input: LinkType) -> Result<LinkType, AppError>;
#[tauri::command] #[specta::specta]
pub async fn create_action_type(input: ActionType) -> Result<ActionType, AppError>;
#[tauri::command] #[specta::specta]
pub async fn list_ontology() -> Result<OntologyGraph, AppError>; // 返回完整 ER 图

// ── 联邦查询 ──
#[derive(Serialize, Deserialize, Type)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<serde_json::Value>, // Arrow → JSON
    pub row_count: usize,
    pub elapsed_ms: u64,
    pub sources_touched: Vec<Uuid>,  // 查询涉及的数据源(透明性)
}

#[tauri::command] #[specta::specta]
pub async fn execute_sql(sql: String) -> Result<QueryResult, AppError>;

#[tauri::command] #[specta::specta]
pub async fn execute_textql(natural_language: String) -> Result<TextqlResult, AppError>;
// TextqlResult 含 { generated_sql, logical_plan, result: QueryResult, explanation }
// LLM 生成 SQL 后先回显给用户确认(不可信输出,§工作守则 5),用户点「执行」才走 execute_sql
```

---

## 十四、核心交互流时序

### 14.1 发送消息 + 流式响应(主链路)

```
用户回车
  │
  ▼
useChat.send()
  ├─ 1. 乐观:Zustand composer 清空草稿;TanStack Query 缓存追加 user msg + 空 assistant msg(status=streaming)
  ├─ 2. invoke('send_message', { req, onChunk }) → 拿到 message_id
  ├─ 3. onChunk 循环(Channel → AsyncIterable):
  │     ├─ TextDelta    → patch assistant msg.content(增量,触发 Streamdown 增量渲染)
  │     ├─ ReasoningDelta → append 到 ThinkingBlock(默认折叠)
  │     ├─ ToolCallStart → 追加 ToolCallCard(status=running)
  │     ├─ ToolCallDelta → patch 卡片参数(流式 JSON)
  │     ├─ ToolResult    → 卡片 status=done,展开结果
  │     ├─ Citation      → 收集,渲染内联 [n] marker
  │     ├─ Done          → status=complete,失效该会话 query
  │     └─ Error         → status=error,toast + 暴露 Retry
  ├─ 4. MessageScroller:若用户在底部则跟随,否则显示「↓ 新消息」浮标
  └─ 5. 中断:Esc 或停止按钮 → invoke('cancel_message', message_id) → 收到 Cancelled chunk 收尾
```

**性能要点**:Streamdown 对增量 delta 做了 memo,仅未完成块重解析,已完成块缓存;TanStack Query 用 `setQueriesData` 做 O(1) patch 而非全量替换;虚拟列表只渲染视口内消息。

### 14.2 文件摄入流

```
拖文件入窗口(FileDropZone 捕获 Tauri drop 事件拿路径)
  │
  ▼
useIngest.ingest(paths)
  ├─ 1. 立即在 IngestStatusBoard 追加占位卡片(status=queued)
  ├─ 2. invoke('ingest_files', { paths, onProgress }) → 拿 job_ids
  ├─ 3. onProgress 订阅:更新卡片 stage/percent;失败项标红 + Retry
  ├─ 4. 全部完成:emit('ingest:batch-done') 广播 → 文件库 query 失效刷新
  └─ 5. 用户可从文件库把已摄入文件 @ 挂载进新消息(走 document_id,不重复摄取)
```

### 14.3 Quick Entry 浮窗(对标 Claude Desktop)

```
全局快捷键(双击 Option / Ctrl+Space,由 tauri-plugin-global-shortcut 注册)
  │
  ▼
Rust 创建/聚焦 Quick Prompt 独立小窗(无边框 + 居中浮层)
  ├─ 1. 该窗口加载独立路由(轻量 bundle,不含文件库/设置)
  ├─ 2. 用户输入 → 选择「在此浮窗继续」或「展开到主窗」
  │     ├─ 浮窗继续:走正常 send_message 流,流式渲染在浮窗内
  │     └─ 展开到主窗:event('quick-prompt:promote', { draft }) → 主窗监听后新建会话并填充
  └─ 3. Esc 关闭浮窗;草稿 persist 到 tauri-store 防丢失
```

### 14.4 数据源注册与 schema 浏览(三期)

```
用户在「数据源」视图点「添加数据源」
  │
  ▼
DataSourceDialog 填写连接信息(host/port/db/user/pass)
  ├─ 1. invoke('test_data_source', config) → 连接测试(不落库)
  ├─ 2. 测试通过 → invoke('register_data_source', config) → 落 SQLite(凭证明文,二期加密)
  ├─ 3. invoke('browse_schema', source_id) → sqlx 查 information_schema
  │     返回 { schemas: [{ tables: [{ columns: [{ name, type, nullable }] }] }] }
  └─ 4. 前端树形展示;用户勾选表 → 标记为「可查询」
```

### 14.5 本体建模(三期)

```
用户在「本体」视图定义语义模型
  │
  ▼
三种建模对象:
  ├─ ObjectType: 映射到数据源表(如 customer 表 → Customer 类型)
  │     ├─ invoke('create_object_type', { name, source_id, table, properties: [{name, column, type}] })
  │     └─ 属性可跨源(如 Customer.orders 映射到另一数据源的 orders 表)
  ├─ LinkType: 定义对象间关系(如 Customer --places--> Order)
  │     └─ invoke('create_link_type', { name, from_type, to_type, join: {from_col, to_col, source_id} })
  └─ ActionType: 定义可执行动作(如「标记 VIP」更新 customer.is_vip)
        └─ invoke('create_action_type', { name, object_type, sql_template, params })
本体元数据全部落 SQLite,前端实时渲染 ER 图(react-flow)
```

### 14.6 TextQL 联邦查询(三期)

```
用户输入自然语言:「查询上月消费超 1 万的 VIP 客户及其订单」
  │
  ▼
TextQL 编译流水线:
  ├─ 1. LLM 意图解析(复用 agent-core Rig):NL → 结构化意图
  │     { object: Customer, filters: [{spent: >10000, period: last_month}],
  │       expand: [orders], condition: customer.is_vip = true }
  ├─ 2. 本体重写(ontology crate):意图 → 逻辑计划
  │     ├─ Customer → 数据源 A 的 customer 表
  │     ├─ orders → 数据源 B 的 orders 表(JOIN on customer.id = orders.cust_id)
  │     └─ spent → SUM(orders.amount) GROUP BY customer.id
  ├─ 3. SQL 生成(sqlparser-rs):逻辑计划 → 各源方言 SQL
  ├─ 4. DataFusion 执行:注册 TableProvider → 跨源 JOIN → Arrow RecordBatch
  ├─ 5. 结果渲染:Arrow → JSON → 前端表格(react-arborist)或图表
  └─ 6. Agent 工具化:联邦查询作为 MCP tool 暴露,LLM 可自主调用(二期 MCP 先行)
```

---

## 十五、对话状态机(前端单会话)

每条 assistant message 是一个显式状态机,避免隐式转换导致的重复消息/幽灵草稿:

```
                    ┌─────────┐
         send()     │         │  TextDelta/ToolCall...
        ──────────▶ │STREAMING│ ─────────────────▶ (loop, patch content)
                    │         │
                    └────┬────┘
                         │
              ┌──────────┼───────────┬──────────────┐
        Done  │     Error │     Cancel│ (user)       │ timeout
              ▼          ▼           ▼              ▼
         ┌────────┐ ┌────────┐  ┌──────────┐   ┌─────────┐
         │COMPLETE│ │ ERROR  │  │CANCELLED │   │ ERROR   │
         └────────┘ └────┬───┘  └────┬─────┘   └────┬────┘
                       retry()│   resume()│             │
              ◀────────────────┴──────────┴─────────────┘
              (retry 从该 msg 起重发;resume 续传流式)
```

- **id 稳定**:每条消息有稳定 `message_id`(Rust 生成),流式 patch 永远按 id 定位,不靠数组下标
- **草稿隔离**:每会话输入草稿独立键,切会话不丢
- **乐观回滚**:发送失败时 user msg 保留、assistant msg 转 error 并附 Retry,而非整条删除
- **中止保留**:中断后已产出内容保留为 partial,标记 CANCELLED 可 resume

---

## 十六、性能与无障碍策略

### 16.1 性能预算与手段

| 场景 | 预算 | 手段 |
| --- | --- | --- |
| 冷启动首屏 | < 200ms | Vite 代码分割;路由级懒加载文件库/设置;主窗 bundle 仅含 chat |
| 流式首字 | < 300ms | Channel fast-path;乐观渲染 placeholder 再 patch |
| 长会话滚动 | 60fps | TanStack Virtual 反向锚定;Streamdown memo;非视口消息卸载 |
| 大文件摄取 | 不阻塞 UI | Rust 侧流式解析;前端仅收进度事件;不在 JS 持有文件内容 |
| 切换会话 | < 100ms | TanStack Query 预取相邻会话;MessageScroller 恢复滚动位置 |

### 16.2 Markdown 安全

LLM 输出按**不可信内容**处理:Streamdown/Shiki 默认禁用 raw HTML;代码块不执行;Mermaid 沙箱渲染;链接 rel="noopener noreferrer" 且外链经确认(防 `file://`/钓鱼)。

### 16.3 无障碍(WCAG 2.1 AA)

- 所有动作有键盘等价(⌘K 面板 + 快捷键);焦点环可见
- 流式输出用 `aria-live="polite"` 播报新消息(可配置关闭)
- 颜色对比 ≥ 4.5:1;不止靠颜色传达状态(图标 + 文字)
- 屏幕阅读器友好:ToolCallCard 用 `role="group"` + `aria-label`;citation marker 有 `aria-describedby` 指向源片段

---

## 十七、跨窗口与移动端策略

### 17.1 多窗口(Tauri webview)

| 窗口 | 用途 | 路由 |
| --- | --- | --- |
| 主窗 | 三栏工作台 | `/chat/:id` `/library` `/settings` |
| Quick Prompt | 全局唤起浮窗 | `/quick`(独立轻量 bundle) |
| 设置窗(可选) | 独立设置面板,可从主窗分离 | `/settings`(独立实例) |

跨窗口同步靠 Event 广播(会话新建、配置变更、摄取完成),各窗口 TanStack Query 收到事件后 `invalidateQueries`。UI 偏好走 tauri-store 自动跨窗口一致。

### 17.2 移动端降级(三期)

| 桌面能力 | 移动端处理 |
| --- | --- |
| 拖拽摄入 | 退化为「+」按钮 → 文件选择器 |
| 侧栏常驻 | 改 Drawer 抽出 |
| 设置 Dialog | 改底部 Sheet |
| ⌘K 命令面板 | 改顶部下拉搜索 |
| Quick Entry 浮窗 | 不可用(无全局快捷键)→ 依赖应用内快速入口 |
| 自定义标题栏 | 用系统原生 |

布局统一用 Tailwind 断点(`sm:`/`md:`/`lg:`)+ shadcn 的 responsive Dialog/Drawer,单套代码适配。

---

## 十八、前端依赖清单(补充第五节)

> 版本号经 npm registry 实时核实(2026-07)。注意几个主版本跃升:`streamdown` 已 2.x(非 0.x)、`lucide-react` 已 1.x(非 0.4xx)、`typescript` 已 7(非 5)、`vite` 已 8、`react-resizable-panels` 已 4、`react-dropzone` 已 19。

```jsonc
// package.json(前端部分)
{
  "dependencies": {
    "react": "^19.2",
    "react-dom": "^19.2",
    "@tanstack/react-router": "^1.170",   // 文件式路由,全类型
    "@tanstack/react-query": "^5.101",    // 服务端状态缓存
    "@tanstack/react-virtual": "^3.14",   // chat 反向锡定虚拟列表
    "zustand": "^5.0",                    // 全局 UI 状态
    "@tauri-apps/api": "^2.11",
    "@tauri-apps/plugin-dialog": "^2.7",
    "@tauri-apps/plugin-fs": "^2.5",
    "@tauri-apps/plugin-store": "^2.4",
    "@tauri-apps/plugin-global-shortcut": "^2.3",  // Quick Entry 全局键
    "@tauri-apps/plugin-clipboard-manager": "^2.3",
    "@tauri-apps/plugin-notification": "^2.3",
    "@tauri-store/zustand": "^1.2",       // UI 偏好跨窗口持久化(Rust 侧)
    "streamdown": "^2.5",                  // 流式 Markdown(含 Shiki/KaTeX/Mermaid)
    "cmdk": "^1.1",                        // 命令面板(shadcn Command 底层)
    "react-dropzone": "^19.1",             // 拖拽视觉层(Tauri 事件提供路径)
    "react-resizable-panels": "^4.12",     // 三栏可调布局
    "lucide-react": "^1.27",               // 图标(已 1.x 主版本)
    "tailwindcss": "^4.3",
    "// shadcn/ui": "复制源码进 components/ui,非 npm 依赖(用 npx shadcn@latest add ...)"
  },
  "devDependencies": {
    "typescript": "^7.0",                  // 已 7.x 正式版
    "vite": "^8.1",                        // 已 8.x 正式版
    "@vitejs/plugin-react": "^6.0",
    "eslint": "^9",
    "// eslint-plugin-boundaries": "强制四层单向依赖(§12.1)"
  }
}
```

> `tauri-specta` 在 Rust 侧(`src-tauri/Cargo.toml`)引入,构建期生成 `src/lib/ipc/bindings.ts`,前端不直接安装。Rust 侧版本见下表。

### 18.1 src-tauri 侧依赖(Tauri + 插件 + specta,2026-07 核实)

```toml
# src-tauri/Cargo.toml
[build-dependencies]
tauri-build = "2.6"

[dependencies]
tauri = "2.11"
tauri-plugin-dialog = "2.7"
tauri-plugin-fs = "2.5"
tauri-plugin-store = "2.4"
tauri-plugin-global-shortcut = "2.3"
tauri-plugin-clipboard-manager = "2.3"
tauri-plugin-notification = "2.3"

# IPC 类型安全桥(见决策 F5 / 决策 18 BigInt 公约)
specta = "2.0.0-rc.25"        # 被 specta-typescript 0.0.12 硬锁(=2.0.0-rc.25)
specta-typescript = "0.0.12"  # 禁止 64 位整数导出;逐字段 #[specta(type = Number)] 绕行
tauri-specta = "2.0.0-rc.25"  # rc 版,API 已变(与 1.x 不兼容)

# 业务 crates(workspace 成员)
agent-core = { path = "../crates/agent-core" }   # 含 rig 0.41 agent + memory_bridge + rmcp 1.x(二期)
federation = { path = "../crates/federation" }  # 三期联邦查询(DataFusion 54 + sqlx 0.9)
ingest = { path = "../crates/ingest" }
memory = { path = "../crates/memory" }           # jieba FTS5,无 sqlite-vec
http = "1"                    # rmcp streamable-http transport 需要
```

> **specta 版本陷阱**:crates.io 上 specta 同时存在 `1.0.5`(stable)和 `2.0.0-rc.25`(rc)。`specta-typescript 0.0.12` 用 `=2.0.0-rc.25` 硬定版本,会强制拉入 2.0-rc(API 与 1.x 不兼容)。若只用 `tauri-specta` 而不用 `specta-typescript`,可走 1.x 线;两者不可混用。本项目用 `specta-typescript 0.0.12`,必须接受 `specta 2.0.0-rc.25`。见决策 18 BigInt 公约。

---

## 十九、前端落地路线(对齐第九节)

| 阶段 | 前端交付 |
| --- | --- |
| **一期 MVP** | AppShell 三栏 + 对话流式(Streamdown + 虚拟列表)+ 拖拽摄入 PDF/Office/文本 + 图片粘贴 + SQLite 会话 + ⌘K 基础导航 + 设置页(单 provider) |
| **二期** | ✅ ToolCallCard(MCP,可折叠卡片)+ ✅ RAG 知识库面板(设置页:配置+源文件列表)+ ✅ MCP 服务器配置区(stdio/http)+ ✅ 摄取后自动入库 + 上下文体积管控(内联提示条)。待补:Citation 渲染 + Sources 面板 + ThinkingBlock + Quick Entry 浮窗 + 命令面板全文搜索 |
| **三期(本体设计)** | 数据源注册向导 + schema 浏览树 + 本体建模 ER 图(react-flow,ObjectType/LinkType/ActionType 三色节点)+ TextQL 编辑器(SQL 预览 + 执行确认)+ 查询结果表格/图表(react-arborist) |
| **四期(可选)** | 移动端响应式适配 + 多 provider 切换矩阵 + 本地 Ollama 接入 UI + 离线模式(历史可读、草稿可编辑) |

---

## 二十、界面与菜单设计细则

> 本章把 §10–§19 的架构层下沉到「可据此画 mockup、写组件 props、定菜单项」的颗粒度。所有组件名沿用 shadcn 2026-06 chat 组件集的真实 API。

### 20.1 shadcn chat 组件集真实 API(落地锚点)

通过 `npx shadcn@latest add message-scroller message bubble attachment marker` 引入,组合关系固定:

| 组件 | 职责边界(官方语义,勿越界) | 在本应用的用法 |
| --- | --- | --- |
| `MessageScroller` + `MessageScrollerProvider` | **拥有**滚动状态:开屏定位、流式跟随、新 turn 锚定、历史 prepend 不跳屏、跳转最新。`Provider` 是 headless 行为内核,`MessageScroller` 是带样式的壳 | ChatThread 根容器,内嵌虚拟化(走 scroller 的 hooks 逃逸口接入 @tanstack/react-virtual) |
| `Message` + `MessageAvatar` + `MessageContent` + `MessageHeader` + `MessageFooter` | 单条消息的**行布局**:头像、对齐、header(名字/时间)、footer(操作按钮)。不放内容本体 | 每条 user/assistant 消息的外壳;footer 放复制/重试/引用 |
| `Bubble` + `BubbleContent` | 消息**内容表面**:变体、对齐、分组、折叠、reaction。不放头像/名字 | 承载 Streamdown 渲染的 Markdown;tool call 用 Bubble 的 collapsible 变体 |
| `Attachment` | 文件/图片附件卡 | 用户消息内的附件预览;Composer 的待发附件条 |
| `Marker` | 内联标记(系统注记、分隔、citation 角标) | citation `[1][2]` 内联角标;会话内的「日期分隔」「模型切换」分隔条 |

**硬约束**:头像/名字/时间戳放 `Message`,不放 `Bubble`;Bubble 只管内容表面。这是官方明确分工,违反会导致 streaming/grouping 行为错乱。

### 20.2 Streamdown 真实 props(流式渲染锚点)

```tsx
import { Streamdown } from "streamdown"

<Streamdown
  isAnimating={isStreaming}      // true 时按流式处理(未闭合块容错 + 光标)
  animated                       // 启用逐字动画(可关)
  // 安全:LLM 输出按不可信处理,默认禁 raw HTML
  // 链接安全:外链弹确认,拦 file:// 与钓鱼
>
  {message.content}
</Streamdown>
```

- `mode="streaming"`(默认):内部用 `remend` 引擎补全未闭合语法 → 切块 → 仅未完成块重解析,已完成块 memo 缓存。这是长回复不卡的关键。
- 内置 Shiki(代码高亮,按需加载语言)、KaTeX(公式)、Mermaid(沙箱图)。
- 与 shadcn 组合:Bubble 内放 Streamdown;`@assistant-ui/react-streamdown` 是已验证的集成参考。

### 20.3 应用菜单栏(native menu,Tauri Menu)

桌面端用 Tauri 原生菜单(macOS 顶部全局栏 / Win-Linux 窗口内栏),不自绘 HTML 菜单——原生菜单支持系统快捷键、无障碍、托盘集成。结构对标 Claude Desktop + Cursor + 通用 macOS 约定:

```
{App}                          (macOS only: 关于 / 偏好设置⌘, / 服务 / 隐藏 / 退出⌘Q)
├─ 文件 File
│   ├─ 新建会话            ⌘N
│   ├─ 新建会话(浮窗)     ⌘ShiftN      ← Quick Entry 独立窗
│   ├─ 打开文件…           ⌘O           ← 摄入文件入口(对话框)
│   ├─ 打开文件夹…         ⌘ShiftO      ← 批量摄入
│   ├─ ─────────
│   ├─ 导出会话为 Markdown ⌘E
│   └─ 关闭会话            ⌘W
├─ 编辑 Edit
│   ├─ 撤销/重做            ⌘Z / ⌘ShiftZ
│   ├─ ─────────
│   ├─ 复制                 ⌘C
│   ├─ 粘贴                 ⌘V           (含图片粘贴自动转附件)
│   └─ 查找会话内…          ⌘F           (会话内全文搜索,区别于全局 ⌘K)
├─ 视图 View
│   ├─ 切换侧栏             ⌘\\
│   ├─ 切换文件库面板       ⌘B
│   ├─ ─────────
│   ├─ 放大/缩小            ⌘+ / ⌘-
│   └─ 主题:浅色/深色/跟随系统
├─ 会话 Conversation        (本应用特有,放 agent 动作)
│   ├─ 发送                 ⌘↵          (与回车换行区分)
│   ├─ 中断生成             ⌘.           ← stop streaming
│   ├─ 重试上一条           ⌘R
│   ├─ ─────────
│   ├─ 引用上一条为上下文   ⌘ShiftR
│   └─ 切换模型             ⌘⌘M          (或输入框模型选择器)
├─ 窗口 Window               (macOS 标准:最小化/缩放/全部置前;多会话窗列表)
└─ 帮助 Help
    ├─ 快捷键参考           ⌘/
    ├─ 文档
    └─ 检查更新…
```

**设计原则**:

- macOS 用 `app` 子菜单 + 标准角色(About/Preferences/Quit),Win/Linux 省略 app 菜单,Quit 并入文件
- 「会话」菜单是本应用区别于通用编辑器的核心——把 agent 交互动作(发送/中断/重试/切模型)提到菜单层,确保键盘可达
- 所有菜单项同步注册为全局快捷键监听(菜单关闭时也生效,如 ⌘. 中断)

### 20.4 命令面板(⌘K)分组结构

对标 Linear/Raycast/Lobe Chat:单一输入、分组结果、键盘优先。用 cmdk + shadcn Command 实现,三类入口:

```
⌘K 输入框(模糊匹配全部命令 + 会话标题 + 消息全文)
│
├─ ▸ 导航 Navigation
│     • 切换到对话 → 列出最近会话(带时间/首句预览)
│     • 切换到文件库
│     • 切换到设置
│     • 打开 Quick Prompt 浮窗
├─ ▸ 动作 Actions(上下文相关)
│     • 新建会话
│     • 摄入文件…
│     • 中断当前生成
│     • 重试上一条
│     • 导出当前会话
│     • 切换模型 → 展开可用模型子列表
│     • 切换主题
├─ ▸ 搜索 Search(命中消息全文,走 Rust 侧 SQLite FTS)
│     • "关键词" in {会话标题} → 点击跳转到该消息并高亮
│     • 分组按会话聚合,显示片段预览 + citation 跳转
└─ ▸ 文件库 Library(命中已摄入文件名/内容)
      • {文件名} → 预览/重摄取/挂载到当前会话
```

**交互细则**:

- 首项自动高亮,↵ 执行,⌘↵ 执行次要动作(如「在浮窗打开」)
- 输入 `>` 前缀 → 只搜动作;`#` 前缀 → 只搜会话;`@` 前缀 → 只搜文件(与 Composer 的 `@` 语义一致)
- 空输入时显示「最近会话 + 推荐动作」,对标 Raycast Root Search
- 移动端:无 ⌘K,改为顶栏放大镜图标触发,布局改顶部下拉

### 20.5 三栏 AppShell 布局规格

```
┌──────────────────────────────────────────────────────────────┐
│ TitleBar(自绘无边框:拖拽区 + 窗口控制 + 模型选择器 + ⌘K入口)│
├──────────┬───────────────────────────────────┬───────────────┤
│ Sidebar  │  Chat Area                         │ Inspector     │
│ 240px    │  (flex-1,最小 480px)               │  320px        │
│ 可调宽   │  ┌─────────────────────────────┐  │ 可调宽/可折叠  │
│ 可折叠   │  │ MessageScroller              │  │               │
│          │  │  (虚拟化消息流)              │  │ ▸ Sources     │
│ 新建 ⌘N  │  │  user/assistant 气泡         │  │   (citation   │
│ 搜索框   │  │  tool call 卡片              │  │    审计列表)  │
│ ──────   │  │  thinking 折叠块             │  │               │
│ 会话列表 │  │  ↓ 新消息浮标(非底部时)     │  │ ▸ 上下文      │
│  (按日   │  └─────────────────────────────┘  │   (已挂载文件 │
│   分组)  │  ┌─────────────────────────────┐  │    + @ 引用)  │
│ • 今天   │  │ Composer                     │  │               │
│ • 昨天   │  │  [附件条] [输入区]            │  │ ▸ 消息信息   │
│ • 本周   │  │  [@上下文] [模型] [发送⌘↵]   │  │   (tokens/耗时│
│ • 更早   │  └─────────────────────────────┘  │    /模型)    │
└──────────┴───────────────────────────────────┴───────────────┘
```

- 用 `react-resizable-panels` 实现三栏可拖拽调宽;Sidebar 与 Inspector 都可折叠到图标条
- Inspector 默认折叠,仅在有 citation 或选中消息时展开(降低首屏认知负荷)
- 桌面最小窗口 720×480;小于 `lg`(1024px)时 Inspector 自动折叠为抽屉
- 移动端:Sidebar→Drawer,Inspector→底部 Sheet,Composer 置底固定

### 20.6 Composer(输入区)交互规格

对标 Cursor 的 `@` + Claude Desktop 的多模态附件:

```
┌──────────────────────────────────────────────────────────┐
│ [📎 file.pdf ✕] [🖼️ img.png ✕] [📄 doc.docx ✕]   ← 附件条 │
├──────────────────────────────────────────────────────────┤
│ 输入到这里…                                               │
│ 输入 @ 触发上下文菜单                                     │
│                                                          │
├──────────────────────────────────────────────────────────┤
│ [@ 上下文]  [模型: Claude 4.5 ▾]  [⚙]    [发送 ⌘↵]      │
└──────────────────────────────────────────────────────────┘
```

- **`@` 上下文菜单**(对标 Cursor):输入 `@` 弹出,分类:已摄入文件 / 文件夹 / 最近会话片段;选中后转为可见的引用 chip
- **附件入口**:拖拽落点(整窗口)、`@` 菜单选文件、粘贴图片自动转附件、`⌘O` 文件对话框
- **多行**:回车换行,`⌘↵` 发送(与菜单「会话→发送」一致)
- **草稿持久化**:按会话 ID 存草稿,切会话/重启不丢(走 tauri-store)
- **模型选择器**:输入框左下,记忆每会话上次选择;⌘⌘M 全局切换
- **状态联动**:流式中 Composer 禁用发送、显示「⌘. 中断」;有附件未摄入完成时禁用发送

### 20.7 消息项(MessageItem)解剖

```
┌─ Message (行布局:avatar + 对齐) ─────────────────────────┐
│ ┌Avatar┐  ┌─ MessageHeader ──────────────────┐          │
│ │  C   │  │ Claude 4.5 · 14:32  [复制][重试] │          │
│ └──────┘  └──────────────────────────────────┘          │
│           ┌─ ThinkingBlock (默认折叠) ──────────┐        │
│           │ ▸ 思考过程 (2.1s)                   │        │
│           └─────────────────────────────────────┘        │
│           ┌─ Bubble (内容表面) ──────────────────┐        │
│           │ <Streamdown> 根据文档[1],应该…       │        │
│           │  ```rust
 代码块 ```                │        │
│           │  [1] ← Marker (citation 角标)        │        │
│           └─────────────────────────────────────┘        │
│           ┌─ ToolCallCard (可折叠) ──────────────┐        │
│           │ 🔧 read_file  ✓ 1.2s  ▸             │        │
│           └─────────────────────────────────────┘        │
│           ┌─ MessageFooter ────────────────────┐         │
│           │ 👍 👎  · 引用此条 · 导出           │         │
│           └────────────────────────────────────┘         │
└──────────────────────────────────────────────────────────┘
```

- user 消息:无 avatar 或用用户头像,右对齐气泡;assistant:左对齐 + avatar
- **流式态**:Bubble 内 Streamdown `isAnimating=true` + 末尾光标;ToolCallCard 显示 spinner
- **error 态**:气泡边框转红 + footer 显眼 Retry;ThinkingBlock 保留已产出
- **citation marker**:用 `Marker` 渲染 `[1]`,hover 弹源文档片段卡(带页码/高亮),点击展开右侧 Inspector 的 Sources
- **grouping**:连续同角色消息用 Bubble 的 `grouping` 合并 avatar(参考官方 chat rules)

### 20.8 文件库视图(Library)规格

```
┌──────────────────────────────────────────────────────────┐
│ [拖拽到此处摄入文件]  ← 全局 FileDropZone(虚线区)      │
├──────────────────────────────────────────────────────────┤
│ 全部 | 文档 | 表格 | 演示 | 图片 | 其他    [搜索] [＋]  │
├──────────────────────────────────────────────────────────┤
│ ┌─ IngestStatusBoard(进行中任务) ──────────────────┐    │
│ │ 📄 report.pdf    解析中 ████████░░ 80%  [取消]   │    │
│ │ 📊 data.xlsx     排队中                    [×]   │    │
│ └──────────────────────────────────────────────────┘    │
├──────────────────────────────────────────────────────────┤
│ 文件列表(虚拟化)                                        │
│  📄 report.pdf     12页 · 2.3MB · 已索引 · 3天前 [⋯]    │
│  📊 data.xlsx      3表 · 480KB · 已索引 · 1周前 [⋯]     │
│  📚 novel.epub     已索引 · 5章 · 2天前 [⋯]             │
│  [⋯ 菜单: 预览 / 重摄取 / 挂载到会话 / 删除 / 导出]     │
└──────────────────────────────────────────────────────────┘
```

- 拖入即摄取(§14.2);IngestStatusBoard 用 kanban 思路按 stage 分组展示进行中任务
- 文件项的 `⋯` 菜单含「挂载到当前会话」——直接把 document_id 作为 @ 上下文塞进 Composer
- 预览:文本/Markdown 渲染;表格用 calamine 数据展示;图片直接显示;PDF/PPTX 显示首页缩略图 + 已抽取文本
- 失败项:红色标记 + Retry;显示错误码(映射 §13.4 AppError)

### 20.9 设置视图(Settings)规格

分区对标 Lobe Chat 的 provider 中心 + Claude Desktop 设置:

```
设置(Settings,可作独立窗口或主窗路由)
├─ 模型提供商 Providers
│   ├─ + 添加 provider(OpenAI/Anthropic/Google/DeepSeek/Ollama/自定义 OpenAI 兼容)
│   ├─ 每个 provider:API Key(密码框,加密存 SQLite)/ Base URL / 可用模型列表(自动拉取)
│   └─ 默认模型 + 默认多模态模型(图片理解用)
├─ MCP 服务器 MCP Servers
│   ├─ + 添加(stdio 命令 / SSE URL)
│   ├─ 每个 server:连接状态 / 工具列表(可逐个启停)/ 重连
│   └─ 工具调用需确认的开关(高危工具拦截)
├─ 知识库 Knowledge
│   ├─ 嵌入模型选择(走哪个 provider 的 embedding)
│   ├─ 分块策略(大小/重叠,高级)
│   └─ 重建索引
├─ 外观 Appearance
│   ├─ 主题(浅/深/系统)· 强调色
│   ├─ 字体大小 · 代码字体
│   └─ 语言(中/英)
├─ 快捷键 Shortcuts(列出 §20.3 全部,可重绑)
└─ 高级 Advanced
    ├─ 数据目录 / 导出全部会话
    ├─ Quick Entry 全局快捷键配置
    └─ 日志 / 检查更新 / 关于
```

- API Key 等敏感数据走 IPC 存 Rust 侧,前端只显示掩码;不落 localStorage
- MCP 工具的「需确认」开关对应 §13.4 的高危工具拦截,前端先弹确认再 invoke

### 20.10 Quick Prompt 浮窗规格

对标 Claude Desktop Quick Entry(双击 Option 唤起):

```
┌─ Quick Prompt(独立小窗,无边框,居中浮层,~520×96px) ─┐
│  [C]  输入问题…                              [⌘↵ 发送] │
│       [展开到主窗 ↗]  [Esc 关闭]                          │
└──────────────────────────────────────────────────────────┘
```

- 轻量 bundle:仅含 Composer + Streamdown,不含文件库/设置/侧栏
- 默认「发送后在此浮窗继续流式」;「展开到主窗」把会话 promote 到主窗(走 event 同步)
- 草稿 persist:Esc 关闭不丢,再唤起恢复
- 移动端无此浮窗(无全局快捷键),改为主窗的快速入口 FAB

### 20.11 状态可见性(状态指示器规范)

参考 GitHub PR 状态徽章的「不止靠颜色」原则(图标+文字+色):

| 场景 | 指示器 |
| --- | --- |
| 流式中 | 气泡内光标 + Composer「⌘. 中断」按钮 + 标题栏小转圈 |
| 工具调用中 | ToolCallCard spinner + 「正在调用 read_file…」文字 |
| 摄取中 | IngestStatusBoard 进度条 + 百分比文字 + 阶段名 |
| 离线 | 标题栏「离线」徽章(灰云图标);历史可读,发送禁用并提示 |
| Provider 401 | 发送后 toast「API Key 无效」+ 「前往设置」按钮 |
| MCP 断开 | 侧栏底部红点 + 「N 个 MCP server 已断开」 |

所有状态同时满足:图标 + 文字 + 颜色三重编码,符合 §16.3 无障碍要求。

---

### 决策 19:会话级知识范围——文件夹层级 + 激活集（二期收尾，2026-07-31）

**决策**：知识库文件用文件系统式嵌套文件夹组织；每个会话持有一份「激活集」——可见的文件夹 + 数据源 + `@` 触发的单文件。Agent 工具按激活集过滤：空集不挂工具（模型通用回答），非空按激活范围挂载 `document_tools(memory, allowed_paths)` / `federation_tools(svc, allowed_sources)`。

**背景**：原设计「上传即可问」导致历史文件污染新会话上下文——用户上传 5 本电子书后，任何新会话都默认能搜到全部，违反上下文洁净性。用户洞见：「上传就能问」不应意味着「不上传也能问」导致历史文件意外参与。同时需文件夹组织（5 本书散在根目录不可用）。

**数据模型**（修订 2026-08：双轨——独立 folders 表 + documents.folder_path）：

- `documents.folder_path TEXT`（默认 NULL → 迁移为 `/Inbox`）+ `source_conv_id TEXT`（会话上传溯源）
- `conversations.active_folders TEXT`（JSON `string[]`）+ `active_sources TEXT`（JSON `string[]`）
- 复用现有 `conversation_documents` 表存 `@` 触发的单文件（激活集 documents 部分）
- **独立 `folders` 表**（`path` PK + `parent_path` + `name` + `created_at`）：持久化空文件夹。
  - 文件夹树 = `folders.path` ∪ `DISTINCT documents.folder_path`（UNION 去重）
  - 修订背景：原「无 folders 表、纯隐式推导」无法持久化空文件夹（新建后刷新即丢），
    云盘体验不完整。业界标准（Stack Overflow）确认云盘式文件管理需独立 folders 表。
  - `list_folders` / `build_folder_tree` 从双轨 UNION 取路径；已有文件迁移的 `/Inbox`
    仍由 documents 推导，无需显式插入 folders 表

**激活集语义**：

- **默认空**：新会话不预选任何文件夹/数据源，防历史污染
- **`active_folders`**：勾选某文件夹 → 该文件夹下所有文件（含子目录递归 `LIKE '/folder/%'`）进激活集
- **`@文件`**：插入 `@fileName` token + `mount_document`（该文件 path 加入 `conversation_documents`）
- **`@source.table`**：插入 token + 把 source 加入 `active_sources`（不连带勾选该 source 所有表——`@` 只精确引用）
- **`active_sources`**：勾选某数据源 → 该 source 下所有表可被 agent 查询
- **持久化**：写入 `conversations` 表，`stream_with_memory` 时后端直接读（前端只传 conv_id）

**Agent 工具过滤**（`crates/agent-core`）：

- `document_tools(memory, allowed_paths: Arc<HashSet<String>>)`：`list_documents`/`search_documents` 返回前 filter（path 在集合内），`read_document` 前校验 path 在集合内否则拒
- `federation_tools(svc, allowed_sources: Arc<HashSet<String>>)`：`list_data_sources` 返回前 filter，`describe_table`/`execute_sql` 前校验 source_name 在集合内
- `stream_with_memory`：`resolve_active_doc_paths(conv_id)` ∪ `get_active_sources(conv_id)` → 空集不挂对应工具（模型无文件/联邦能力，按通用知识回答）

**上传行为**：

- 会话内上传：落 `/Inbox` + 记 `source_conv_id` + 自动 `mount_document`（加入激活集，立即可用，持久化）
- Library 上传到指定 `folder_path`，不自动激活（要用再 `@` 或 chip 勾选）

**文件夹操作**（双轨同步：folders 表 + documents.folder_path）：

- `create_folder(path)`：`INSERT OR IGNORE INTO folders`（幂等，空文件夹持久化）
- `move_document(path, target_folder)`：改 `folder_path`
- `rename_folder(old, new)`：递归更新 documents.folder_path（前缀替换）+ folders 表逐行更新 path/parent_path/name
- `delete_folder(folder)`：级联删 folders 表记录（含子文件夹）+ 删所有文件（含子目录）+ FTS5 索引 + `conversation_documents` 关联（用户明确确认：删除不移动到 Inbox）

**前端**：

- 对话页顶部 `ScopeChip`：显示激活范围或「未挂载知识源」，popover 勾选文件夹+数据源
- `MentionMenu`：候选 = 所有文件 + 所有数据源表（`useQueries` 批量查 schema）；分组渲染
- `LibraryView`：两栏文件树（左:层级文件夹含 Inbox 置顶；右:文件列表+操作）

**否决项**：

- NotebookLM 风格「笔记本作为独立实体」——用户细化为文件系统式文件夹（更符合云盘/文件树心智模型，无独立实体维护负担）
- `@` 候选只列激活集内文件——`@` 是显式动作，引用即激活，候选应为全量（方案 B）；chip 是「批量挂载」入口，`@` 是「精确引用即挂载」入口，两者互补
- `@文件` 连带激活所在文件夹——`@` 是精确引用，不连带；要批量见整个文件夹去 chip 勾选
- 独立 `folders` 表——文件夹由文件隐式定义，避免文件夹与文件 orphan 不一致
- 临时文件随会话删——用户明确要求持久化（回到历史会话需看到上传文件）；「临时」只是组织语义（未分类暂存区），非「用完即弃」

### 决策 20:Agent Skill 系统——agentskills.io 规范的文本扩展机制（二期收尾，2026-08）

**背景**：Agent 能力扩展需要标准化机制。Anthropic 推出的 agentskills.io 规范定义了 `SKILL.md` frontmatter + Markdown body 的渐进式披露格式，已被 Claude Code、pi、Cursor 等采纳。onto-studio 作为 agent 工作台，需原生支持 skill 作为 agent 能力的补充层（区别于 MCP 工具：skill 是「文本指令扩展模型行为」，MCP 是「可执行工具」）。

**核心机制**：

- **三层 disable 语义**（优先级从高到低）：
  1. `disable_model_invocation: true`（frontmatter，不可覆盖）：skill 永不进 preamble，但用户仍可 `@skill-name` 手动激活（仅注册 doc path，不进 preamble）
  2. 全局禁用（`disabled_skills` 表，用户在设置页关闭）：skill 不进 preamble、不可被模型激活
  3. 会话级 enabled（`conversation_skills` 表，用户在会话内开关）：精细控制单会话内哪些 skill 生效
- **四类 skill 来源**：
  - **Builtin**：`resources/skills/`（随应用分发，3 个内置：federation/ontology/ingest）
  - **Imported**：`~/.onto-studio/skills/`（用户导入，可卸载）
  - **External**：`~/.claude/skills/`、`~/.pi/agent/skills/`、`~/.agents/skills/`（跨客户端只读扫描）
  - **`@skill-name`**：用户在消息中显式激活（Tier 3，仅当未全局禁用且未 disable_model_invocation 时生效）
- **渐进式披露（三层）**：
  - **Tier 1（preamble）**：激活的 skill 的 name + description 注入 system preamble，模型感知「有哪些 skill 可用」
  - **Tier 2（doc path）**：skill body（全文）入库 `documents` 表，注册 `skill://<name>` doc path，模型按需调 `read_document` 取全文（与文件检索统一路径）
  - **Tier 3（手动激活）**：`@skill-name` 触发，确保 skill 进 preamble + doc path 可读

**实现要点**：

- `agent-skills = "0.2"` crate：`SkillDirectory::load` 解析 SKILL.md（frontmatter 强校验 name 匹配目录名、description ≤1024 字符）
- `crates/agent-core/skill/`：6 文件（mod/builtin/manager/prompt/activate/import）
  - `manager.rs`：扫描四类目录，去重（Builtin > Imported > External），ensure_skill_documented 入库
  - `prompt.rs`：build_preamble_section 生成 `<available_skills>` XML 段
  - `activate.rs`：三层 disable 判断 + active_skill_doc_paths
  - `import.rs`：import_from_dir/zip（复用 ingest::security 防炸弹）/ uninstall
- `crates/memory/skill_repo.rs`：`disabled_skills` + `conversation_skills` 两表
- `chat.rs` 集成：`stream_with_memory` 调 `build_preamble_section(conv_id)` 拼 ProviderConfig.preamble（系统人设在前保 prefix cache，skill 段在后）+ `active_skill_doc_paths(conv_id)` 合并进 doc_paths_set
- `src-tauri/skill.rs`：路径解析（builtin 复用 pdfium 三层兜底；user/external 用 dirs crate）
- `commands/skill.rs`：6 个 IPC 命令（list/import_dir/import_zip/uninstall/set_conv_enabled/set_global_disabled）

**否决项**：

- 自造 SKILL.md 解析器——`agent-skills` crate 已实现规范，避免重复造轮子（许可证 MIT 友好）
- skill body 自动注入 preamble——撑爆 context，改用 doc path + read_document 按需读（与决策 17 `@` 挂载统一 agentic search 路径）
- skill 段进 system prompt 前部——破坏 prefix cache，改放 preamble 尾部（系统人设在前不变前缀）
- 独立向量库存 skill——违反原则 1，skill 全文走 documents 表 + FTS5（与文件检索同库）
- External 目录可写——跨客户端目录只读扫描，避免污染其他 CLI

**与现有决策的关系**：

- 复用决策 17 的 doc path 机制（`skill://<name>` 与 `file://` 统一 read_document 路径）
- 复用决策 18 BigInt 公约（skill 无 64 位整数字段，无需注解）
- 复用决策 19 会话级激活集思路（conversation_skills 表与 conversation_documents 同模式）
- 不引入新原生依赖（agent-skills 纯 Rust，dirs 纯 Rust，符合原则 1/3）

**修订（2026-08-05）：补全资源披露层（references/assets/scripts 三子目录，Tier 2.5）**

原实现只把 SKILL.md body 入库为 `skill://<name>`，遗漏了 `references/` 子目录下的契约文档（agentskills.io 规范定义的 skill 目录结构：`SKILL.md` + 可选 `scripts/` + `references/` + `assets/`）。后果：模型读到 body 里的「详见 references/gaia-schema-contract.md」后，没有任何工具能按文件路径读到磁盘上的资源（read_document 只按已入库 id 读），只能在知识库里瞎找后放弃——这是过去一轮 Agent 产出本体包时跳过 schema 契约、凭描述猜字段名的根因。

补全方案（方案 A1：资源随父 skill 整体入库，模型按需读），覆盖规范全部三类子目录：

- `crates/agent-core/src/skill/mod.rs`：新增 `SkillSubdir` 枚举（References/Assets/Scripts）+ `resource_doc_path()` / `scan_subdir_files()` / `is_text_resource()` helper。`scan_subdir_files` 复用 `agent_skills::SkillDirectory` 的 `scripts()`/`references()`/`assets()` 枚举 API（避免手写 read_dir 与规范脱节）。文本扩展名识别 `.md/.txt/.json/.yaml/.yml/.toml/.csv/.xml/.sh/.py/.js/.ts/.rs`，二进制（图片/dll/压缩包）跳过（本期不做 MEDIA_REFERENCE）
- `manager.rs::ensure_skill_documented`：body 入库后遍历三类子目录，逐个入库为 `skill://<name>/<dir>/<filename>`（format=`skill-resource`），异步建 FTS5 索引，doc path 收集进 `SkillRecord.resource_doc_paths`
- `activate.rs::active_skill_doc_paths`：资源 doc path 随父 skill 一起进 `doc_paths_set`（受三层 disable 约束：全局禁用的 skill 其资源也不可读）
- `prompt.rs::format_available_skills`：`<location>` 文案重写——明确告诉模型 body + N 份资源均已入库、doc path 模式（`<子目录> ∈ {references, assets, scripts}`）、读取方式（search_documents 跨 body+资源搜关键词 / list_documents 拿 id 后 read_document 精读）、并提示「产出前务必先读 references 里的 schema 契约文档」
- `import.rs::uninstall`：卸载时除删 body 外，逐个删除资源文档（`delete_document_by_path` 精确匹配，不支持前缀）
- `memory::documents::list_documents`：取消 `WHERE format != 'skill-md'` 过滤——skill-md 与 skill-resource 现在都进 list_documents（供模型发现），由 `document_tools::list_documents_tool` 层的 `allowed_paths`（doc_paths_set）过滤，只返回本会话激活的 skill 文档，不泄露未激活 skill。`list_documents_by_folder`（前端 Library 用）仍排除 skill-md，skill 走独立 Inspector SkillTogglePanel 暴露
- 新增 10 个测试：三类子目录全部入库 / scripts 可读 / 二进制跳过（references+assets 混放）/ 无资源目录 / 资源随父 skill 进激活集 / 禁用排除资源 / 卸载清理资源 / preamble 文案有/无资源 / XML 转义

scripts 处理说明：onto-studio 无执行 skill 脚本的能力（skill 是文本扩展，可执行走 MCP），但 scripts 内容仍入库——模型需读到脚本内容以理解可用命令、在必要时指导用户。

语义不变：三层 disable、四类来源、prefix cache（skill 段仍拼 preamble 尾部）、agentic search（不注入全文）均保持。资源走与 body 同样的「doc path + read_document」路径，不引入新机制。

### 决策 21：会话消息操作——复制 / 重新生成 / 编辑重发（2026-08）

**背景**：会话窗口缺消息级操作。代码块已有复制按钮（MarkdownText CodeHeader），但整条 assistant 回复无法复制；无法重新生成不满意的回复；无法编辑已发 user 消息后重发。assistant-ui 原生 `ActionBarPrimitive` 提供 Copy / Reload / Edit / ExportMarkdown / Speak / Feedback 等按钮原语，自动处理禁用态（无内容/运行中不可复制、非 assistant 不可 reload、编辑中不可再编辑）和复制反馈（`data-copied` 属性）。

**方案**：采用 assistant-ui 原生 `ActionBarPrimitive`，分角色配置：

- **assistant 消息**：Copy（复制正文）+ ExportMarkdown（导出 .md）+ Reload（重新生成）
- **user 消息**：Copy + Edit（行内编辑态）
- `autohide="not-last"` + `autohideFloat="always"`：除最后一条外平时隐藏，悬停浮现（`data-[floating]` + `group-hover` 透明度过渡），不打扰阅读；最后一条常驻显示便于操作最新回复
- Edit 用 (a) 方案：assistant-ui 原生行内编辑态——`ComposerPrimitive.If editing` 切换，`ComposerPrimitive.Input` 在 Message 内自动绑定 edit composer runtime，提交时 assistant-ui 调 `onEdit(AppendMessage)`

**runtime 桥接**（`ChatRuntime.tsx` 的 `useExternalStoreRuntime` 补回调）：

- `onReload(parentId)`：parentId = 重新生成 assistant 的前序 user 消息 id。调 `chat.reload(parentId)`
- `onEdit(message)`：`message.parentId` = 被编辑 user 消息 id，从 `message.content` parts 提取文本。调 `chat.editAndResend(id, text)`

**截断删除**（`useChat.ts` reload/editAndResend + 后端 `delete_message_and_after`）：

- 语义：send_message 每次落新 user + 新 assistant，故 reload/edit 不能只删 assistant——会多出一条 user。正确做法是删「目标 user 及其后所有消息」（含对应 assistant），再用原/新 user 文本重发，历史等价于「重发这条 user」
- 后端新增 `delete_message_and_after(message_id)` 命令：单事务 `DELETE FROM messages WHERE conversation_id=? AND rowid >= (SELECT rowid ...)`。**用 SQLite 隐含 `rowid`（插入自增）而非 `created_at` 做时序基准**——连续创建消息可能同毫秒，`created_at >=` 会误删前序；`rowid` 单调递增无撞值风险
- 返回 `u32`（非 usize）——遵循决策 18 BigInt 公约：命令返回值不能用 64 位整数，计数类值域小用 u32

**已知限制**：reload/editAndResend 重发时无法恢复原 user 消息携带的图片上下文（MessageRow 不存 context_images），仅重发纯文本 + 当前会话挂载状态。多数 reload 场景不涉图片，可接受；如需完整上下文重发需后续扩展 MessageRow 存原始 SendVariables。

**暂不做的按钮**及理由：

- Speak / StopSpeaking：需 `SpeechSynthesisAdapter`（Web Speech API），且本地无 TTS 权重（原则 2），如需语音后续接 API
- Feedback 👍/👎：需后端反馈存储表 + runtime `onFeedback`，单独立项

**与现有决策的关系**：

- 复用决策 18 BigInt 公约（返回 u32）
- 复用 `delete_message` 既有的 NotFound 语义
- 不引入新原生依赖（纯 SQL + assistant-ui 原语，符合原则 1/3）

### 决策 22：本体不变点——设计宪章 charter（业务场景 / 本质 / 设计意图 / 补充说明）

**背景**：本体的会话式更新持续维护和更新，已有「历史」（git 式 changelog，记**变化点**），但缺「不变点」——业务意图、业务本质、设计意图等信息会随每次增量 import 被覆盖（`upsert_ontology` 的 UPDATE 分支会写 description），导致 AI 在多次增量更新后失去稳定的业务认知基线，建模逐渐漂移。

两条原则要求 charter 作为不变点：

1. 业务意图目标决定，遵循「够用且可扩展」——charter 记录建模的取舍边界，不为完备而完备
2. 本体始终扮演「向 AI 说明业务本质」的角色——charter 是 AI 自主性建立在结构化业务认知之上的载体

**方案**：新增 `ontology_charter` 表，与 `ontology_changelog`（变化点）物理分离、语义分离、写入路径分离。

```sql
CREATE TABLE IF NOT EXISTS ontology_charter (
    ontology_api_name  TEXT PRIMARY KEY,        -- 1:1 关联 ontologies.api_name
    business_scenario  TEXT NOT NULL DEFAULT '',  -- 业务场景
    business_essence   TEXT NOT NULL DEFAULT '',  -- 业务本质
    design_intent      TEXT NOT NULL DEFAULT '',  -- 设计意图
    invariants         TEXT NOT NULL DEFAULT '',  -- 补充说明（自由文本，非 JSON 数组）
    updated_at         INTEGER NOT NULL,
    updated_by         TEXT NOT NULL DEFAULT 'agent',
    FOREIGN KEY (ontology_api_name) REFERENCES ontologies(api_name) ON DELETE CASCADE
);
```

**关键设计决策**：

- **1:1 用 api_name 而非 ontology_id 作主键**：ontology 行被 delete+recreate 时 id 会变（`new_id()`），但 api_name 稳定——charter 跟着 api_name 走不丢
- **四字段拆分而非一坨 markdown**：`business_scenario`（场景）/`business_essence`（本质）/`design_intent`（意图）/`invariants`（补充说明，自由文本非数组）——让 agent 读取时能结构化理解「场景-本质-意图-约束」四元组
- **不在 `OntologyPayload` 里**：payload 是 Gaia 对齐的 write-view，charter 是 onto-studio 本地的「说明层」。export 不带 charter（保持 Gaia 兼容），import 不从 payload 读 charter（避免覆盖）
- **`upsert_ontology` 去掉 description 覆盖**：增量 import 的 UPDATE 分支只改 `display_name + updated_at`，description 不再被覆盖——description 归 charter 管理域。新建本体首导时仍用 payload.description 作初始值
- **invariants 用 text 不用数组**：自由文本存储补充内容，比 JSON 数组更灵活（可写自然语言约束、边界条件、多段说明）
- **charter 不进 Gaia 兼容 export payload**：charter 是本地增强，保持 payload 对齐 Gaia；export 出去的 JSON 给别处 Gaia 实例用时丢失 charter（可接受——charter 是本地业务认知，不属本体定义层）

**写入路径分离（核心：不随历史变化）**：

- charter 由独立命令 `set_ontology_charter` 写入，**不进 import 流程**
- 冷启动首导后调一次；增量更新时**只读不写**
- 只有用户明确要求调整不变点时才调 `set_ontology_charter` 修订
- `set_charter` 后不触发 `ontology-changed` 事件（charter 不影响实体定义，不需失效 payload 缓存）

**读取路径（向 AI 说明业务本质）**：

- `describe_ontology` 只读工具返回携带 `charter` 字段——agent 第一跳拿到 OT 目录时同步拿到业务场景/本质/意图/约束
- 会话 `@OntologyName` 注脚附加 charter 摘要（business_essence + design_intent + invariants 前 200 字）——让模型无需调工具就知道本体是干什么的、有哪些红线
- 增量更新前 SKILL 引导 agent 先调 `describe_ontology` 读 charter 作约束基线，对照 `invariants` 自检本次变更是否违反业务约束（软约束，不阻断落库）

**SKILL 流程引导**（`ontology-modeling` skill）：

- **冷启动场景**：先从历史对话/材料提取 charter 信息（不重复问用户已说过的）；信息不足时向用户确认；首导后调 `set_ontology_charter` 落库；然后开始详细实体建模
- **增量更新场景**：先调 `describe_ontology` 读 charter 作约束基线；对照 invariants 自检；不调 `set_ontology_charter`；除非用户明确要求调整不变点

**前端**：OntologyView 头部常驻 CharterPanel（四段只读展示 + 编辑切换），不进「历史」Tab 流——视觉强化「charter 是本体的根本说明，不是变更记录」。编辑入口带警示文案「只有用户明确要求调整不变点时才编辑」。

**BigInt 公约**：`OntologyCharter.updated_at` 是 i64，加 `#[specta(type = Number)]` 注解（与 `OntologyChangelog.created_at` 同模式）。

**与现有决策的关系**：

- 复用决策 10（本体元数据存 SQLite）——charter 同属本体定义层表族
- 复用决策 18 BigInt 公约（`updated_at` 用 Number 注解）
- 复用 Gaia 对齐原则——charter 不进 `OntologyPayload`，保持 export/import 对称
- 不引入新原生依赖（纯 SQL + 现有 rusqlite，符合原则 1/3）
