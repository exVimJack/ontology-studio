# 三期设计：数据源联邦查询（DataFederation）

> 本文件是 onto-studio 三期「本体设计 + 联邦查询」的独立设计文档。
> 架构总纲见 `ARCHITECTURE.md`；本文件聚焦三期的技术决策、契约、落地路径。
> 三期不改 ARCHITECTURE.md 既有决策，仅在「落地路线」章节引用本文件。
>
> 创建：2026-07-30

---

## 一、目标与范围

### 1.1 目标

让用户注册异构数据源（MySQL/PG/CSV/Excel），通过 **Agent 自主查询**（NL→SQL→执行）或 **`/sources` 工作区手动 SQL** 跨源联邦查询。

### 1.2 三期范围（明确边界）

| 范畴 | 三期做 | 三期不做（留四期） |
|---|---|---|
| 数据源 | MySQL / PostgreSQL / CSV / Excel | SQLite/ClickHouse/DuckDB/MongoDB 等 |
| 查询 | **只读 SELECT/WITH**（含跨源 JOIN） | 写操作（INSERT/UPDATE/DELETE/DROP） |
| 本体建模 | ❌ 不实现 ObjectType/LinkType/ActionType | 三期纯联邦查询，本体留四期 |
| TextQL | agent agentic 查询（NL→工具调用→SQL） | 独立 TextQL 编译器（sqlparser 三段式） |
| ActionType | ❌ 不实现 | 留四期 |
| ER 图 | ❌ 不实现 | 留四期（react-flow） |
| 凭证存储 | 明文 SQLite（对齐决策 10） | keyring 加密（二期统一加密时做） |

> **关键决策（用户拍板）**：三期是**只读产品**。`execute_sql` 工具永远只执行 SELECT/WITH，写 SQL 在工具层 sqlparser 拦截 + SessionConfig 禁 DDL 双层防御。ActionType 三期不实现。

### 1.3 不改动的既有决策

- 决策 9（DataFusion 54.x）、决策 10（本体元数据存 SQLite）、决策 11（sqlparser 0.62）、决策 12（sqlx 0.9 + rustls）——均沿用，本文件补充落地细节。

---

## 二、核心架构决策

### 2.1 Agent 集成范式：DataFusion 作为 rig 的 DynamicTool

DataFusion 不是「外挂引擎」，而是 agent 的工具。联邦查询能力包成 `DynamicTool`，与现有 MCP 工具 / 文件检索工具走**同一条 `ToolServerHandle` 注入路径**：

```
MCP server 工具  ─┐
文件检索工具     ─┼─▶ DynamicTool ─▶ ToolServerHandle.add_dynamic_tool() ─▶ agent
联邦查询工具(新) ─┘
```

- `chat.rs` 的 `stream_with_memory` 已把 `document_tools` 和 MCP `tool_handle` 合并进同一 handle
- 三期新增 `federation_tools.rs`，与 `document_tools.rs` 同构（闭包捕获 `Arc<FederationService>`）
- `ChatService` 加 `federation: Option<Arc<FederationService>>` 字段，与 `memory`/`tool_handle` 注入方式对称

**rig 无原生 DataFusion/SQL 集成**（已确认 rig 仓库仅有 provider/vector store/RAG/MCP），DynamicTool 桥接是唯一路径，但代码量小（~120 行，见 §四）。

### 2.2 查询引擎：DataFusion 54.x（决策 9）

- 单进程内嵌，零 JVM，原生 Arrow 内存格式，Apache-2.0
- 内置 `information_schema` 统一视图（`information_schema.tables` / `.columns`），跨源 schema 探查统一为 ISO SQL，agent 不需记各源原生方言
- catalog 三级寻址（`catalog.schema.table`）：每个数据源注册为一个 catalog，跨源 JOIN 用三段式限定名，DataFusion 自动路由

### 2.3 SQL 连接层：sqlx 0.9 + rustls（决策 12）

- `mysql` + `postgres` + `tls-rustls-ring` feature 组合，纯 rustls 无 native-tls，符合原则 1
- MySQL/PG 用强类型 `MySqlPool`/`PgPool`，按 `DataSourceKind` 分发（不用 AnyPool，避免类型擦除损耗）
- **不引入 `datafusion-table-providers`**：其 leaf crate 硬编码 native-tls（`mysql_async`/`tokio-postgres` 的 `native-tls-tls`/`postgres-native-tls`），违反原则 1，且 facade 无 rustls feature，需 fork 维护——总工作量大于自实现

### 2.4 联邦下推：datafusion-federation 0.5.5（方案 D 核心）

**不手写 JOIN 拆分逻辑**，用 `datafusion-federation` 自动处理跨源查询下推（Spice AI 生产同款架构）。

集成方式（官方 example `df-csv-advanced.rs` 实证）：

```rust
// 1. 实现 SQLExecutor trait（每源一个，~80 行）
let executor = Arc::new(MysqlExecutor::new(pool));
// 2. 包成 federation provider
let fed_provider = Arc::new(SQLFederationProvider::new(executor));
// 3. 包成 schema provider（声明该源有哪些表）
let schema_provider = Arc::new(SQLSchemaProvider::new_with_tables(fed_provider, tables).await?);
// 4. 注册到 SessionState（加 FederationOptimizerRule + FederatedQueryPlanner）
// 5. federation analyzer 自动识别子计划、生成方言 SQL 下推、本地合并跨源结果
```

我只需实现 `SQLExecutor` trait 的 6 个方法（docs.rs 实证，trait 是 dyn compatible）：

```rust
pub trait SQLExecutor: Sync + Send {
    fn name(&self) -> &str;
    fn compute_context(&self) -> Option<String>;  // 区分多个源，返回 None 会导致同名源错误联邦
    fn dialect(&self) -> Arc<dyn Dialect>;         // 'mysql'|'postgres' 等（sqlparser Dialect）
    fn execute(&self, query: &str, schema: SchemaRef,
               filters: &[Arc<dyn PhysicalExpr>]) -> Result<SendableRecordBatchStream>;
    fn table_names(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
    fn get_table_schema(&self, table_name: &str) -> impl Future<Output = Result<SchemaRef>> + Send;
}
```

**实现要点**（每源一个 Executor，~80 行）：
- `execute`：federation 把下推的方言 SQL（已由 unparser 生成）传入，用 sqlx 执行，结果转 `SendableRecordBatchStream`（用 `RecordBatchStreamAdapter` 包 sqlx 行流→Arrow RecordBatch）。`filters` 含运行时物理表达式（如 DynamicFilter），可安全忽略
- `table_names`：查源原生 `information_schema.tables`（MySQL/PG）或 SQLite `sqlite_master`
- `get_table_schema`：查源原生 `information_schema.columns`，构造 Arrow `SchemaRef`
- `dialect`：返回 `MySqlDialect` / `PostgreSqlDialect`（sqlparser 自带，federation unparser 用它生成方言 SQL）
- `compute_context`：必须返回唯一字符串（如 `mysql:localhost:3306:mydb`），返回 None 会导致同名源错误联邦（docs.rs 警告）

方言 SQL 由 federation 的 unparser 自动生成（datafusion 54 默认启用 `unparser` feature），**不手写方言翻译**。

### 2.5 SessionContext 生命周期：全局单例（用户拍板）

- `Arc<FederationService>`（内含 `Arc<SessionContext>`）应用启动时构造一次，注入 `ChatService`，全程共享
- 与 `Arc<Memory>` 单例模式完全对称（既定架构模式，零新增概念）
- 复用 sqlx 连接池 + schema 缓存，避免每会话重连重探

**配套设计（避免踩坑）**：

1. **SessionContext 完整构造**（开发人员可直接参考）：
   ```rust
   use datafusion::execution::runtime_env::RuntimeEnvBuilder;
   use datafusion::execution::session_state::SessionStateBuilder;
   use datafusion::optimizer::Optimizer;
   use datafusion::prelude::{SessionConfig, SessionContext};
   use datafusion_federation::{FederatedQueryPlanner, FederationOptimizerRule};

   let runtime = RuntimeEnvBuilder::new()
       .with_memory_limit(512 * 1024 * 1024, 0.8)  // 512MB 软上限，80% spill
       .build_arc()?;
   let config = SessionConfig::new().with_target_partitions(4);  // 桌面 4 核
   let mut rules = Optimizer::new().rules;  // 取默认优化规则
   rules.push(Arc::new(FederationOptimizerRule::new()));  // 加联邦规则
   let state = SessionStateBuilder::new()
       .with_config(config)
       .with_runtime_env(runtime)
       .with_optimizer_rules(rules)
       .with_query_planner(Arc::new(FederatedQueryPlanner::new()))  // 联邦查询规划器
       .with_default_features()
       .build();
   let ctx = Arc::new(SessionContext::with_state(state));  // 全局单例
   ```
   关键：`FederationOptimizerRule` + `FederatedQueryPlanner` 必须同时注册，否则 federation 不生效（官方 example `df-csv-advanced.rs` 实证）。

2. **RuntimeEnv 内存上限**（防 OOM）：DataFusion 默认 RuntimeEnv 无内存上限（有用户报告吃 2GB）。桌面应用必须设（见上 `with_memory_limit`）。
2. **数据源热注册/注销**：「全局」不等于「静态」。数据源配置持久化到 SQLite（决策 10），运行时可热增删：
   - 启动时从 SQLite 读所有数据源 → 注册到 SessionContext（恢复热状态）
   - 前端注册新源 → 写 SQLite + `ctx.register_catalog()` 热生效
   - 删除源 → 从 SQLite 删 + `ctx.deregister_catalog()`
3. **CSV/Excel 失效检测**：`list_data_sources` 时对文件型源做 `std::fs::exists` 检查，不存在标记 ⚠️

会话级 context 只在多租户隔离/临时表污染场景才需要，三期单用户只读 SELECT，无此需求。

---

## 三、Agent 工具设计

### 3.1 工具粒度：温和三工具（用户拍板）

不走 Text2SqlAgent 的激进路线（单 `execute_sql` 让 agent 自己查 information_schema），原因：
- 多源场景下源清单必须显式（agent 要知道有哪些 catalog 可查）
- list/describe 返回精简结构化数据，比 information_schema 原始行集省 token
- describe_table 不碰数据，天然安全

| 工具 | 输入 | 实现 | 作用 |
|---|---|---|---|
| `list_data_sources` | 无 | 查 SQLite data_sources 表 + DataFusion `catalog_names()` | 返回源清单 + 每个源的表列表 + 连接状态 |
| `describe_table` | source_id, table | DataFusion `information_schema.columns` 或 `TableProvider.schema()` | 返回列名/类型/主键 + 前 5 行样本 |
| `execute_sql` | sql, limit? | DataFusion `ctx.sql(sql).collect()` + 只读校验 + 行数上限 | 执行 SELECT，返回 JSON 行集 + 涉及源 + 耗时 |

三个工具内部都用 DataFusion 统一 API，不直接碰各源原生 information_schema。方言适配是 DataFusion + SQLExecutor 的职责，对 agent 透明。agent 永远只看到 DataFusion 的 ISO SQL + 三段式 catalog 限定名。

### 3.2 安全护栏（三层防御）

1. **sqlparser 前置拦截**（`execute_sql` 工具内）：复用 datafusion 自带的 `sqlparser 0.62.0`（datafusion 54 已依赖，**不需单独加 sqlparser 依赖**）。解析 SQL 后检查 `Statement` 枚举，只放行 `Statement::Query`（SELECT/WITH），遇 `Insert`/`Update`/`Delete`/`Drop`/`Alter`/`Truncate` 等直接拒，返回「只读模式，已拦截」
2. **SessionConfig 禁 DDL**：`SessionContext` 默认支持 DDL/DML（`CREATE TABLE`/`CREATE VIEW`/`INSERT`）。配置层用 `SessionConfig::new()` 不启用 DDL 相关选项（datafusion 的 `sql` feature 解析后，DDL 走 `SessionContext::sql` 的默认实现，可通过自定义 catalog/schema provider 拒绝注册来限制）
3. **行数硬上限**：`execute_sql` 默认 `limit=200`，最大 1000，防 `SELECT *` 爆内存。在 SQL 末尾自动追加 `LIMIT`（若用户 SQL 未含）
4. **超时**：`tokio::time::timeout(30s)` 包裹 `ctx.sql().collect()`。DataFusion 49+ 内置 Tokio task budget 协作式取消（官方 cancellation 博客实证），stream drop 即停；sqlx 连接在 future drop 时释放回池

### 3.3 大结果集处理（防撑爆上下文）

agentic 模式下 agent 可能连调 4-5 次，每次结果进上下文。对策：
- `execute_sql` 默认 200 行，列多时只回前 10 行 + 行数统计，全文让 agent 再调工具翻页（同 `read_document` 模式）
- 二期 `CompactingMemory` + `TokenWindowMemory` 自动压缩兜底

### 3.4 TextQL 交互：ToolCallCard 内确认（用户拍板选项 b）

- agent 生成 SQL → 回显在对话的 `ToolCallCard`
- ToolCallCard 显示 SQL + 「执行」按钮
- 用户点「执行」→ 工具执行 SQL（三层护栏兜底只读）
- 写 SQL 在 ToolCallCard 显示「只读模式，已拦截」，无执行路径
- 不做跨视图联动（不回显到 `/sources` 编辑器），实现简单

---

## 四、工程结构

### 4.1 新增 crate：`crates/federation`

```
crates/federation/
├── Cargo.toml
├── src/
│   ├── lib.rs              ← FederationService 入口（Arc<SessionContext> + Arc<Memory> + 连接池缓存）
│   ├── source.rs           ← DataSourceConfig / DataSourceKind / DataSourceSummary（specta::Type）
│   ├── executor/
│   │   ├── mod.rs          ← SQLExecutor trait 约定 + 共用辅助
│   │   ├── mysql.rs        ← MysqlExecutor（sqlx MySqlPool + rustls）
│   │   ├── postgres.rs     ← PostgresExecutor（sqlx PgPool + rustls）
│   │   ├── csv.rs          ← DataFusion 内置 register_csv（零 SQLExecutor）
│   │   └── excel.rs        ← calamine → MemTable（复用 ingest）
│   ├── schema.rs           ← browse_schema（information_schema → SchemaSnapshot）
│   ├── query.rs            ← execute_sql（只读校验 + DataFusion 执行 + Arrow→JSON）
│   ├── catalog.rs          ← 注册/注销数据源到 SessionContext（热增删）
│   └── error.rs            ← FederationError 枚举
└── tests/
    └── csv_e2e.rs          ← CSV provider 端到端（零外部依赖，验证全链路）
```

### 4.2 依赖（Cargo.toml）

```toml
[dependencies]
# datafusion 54.0.0 自带 arrow 58.3.0 + sqlparser 0.62.0（见 docs.rs/crate/datafusion/54.0.0/source/Cargo.toml 实证）
# unparser feature 默认启用（federation 生成方言 SQL 需要）
datafusion = "54"                          # Apache-2.0，单进程内嵌查询引擎
datafusion-federation = { version = "0.5.5", features = ["sql"] }  # Apache-2.0，联邦下推（与 datafusion 54 兼容，datafusion-table-providers 0.13 workspace 共存实证）
sqlx = { version = "0.9", default-features = false, features = [
    "mysql", "postgres", "tls-rustls-ring", "runtime-tokio", "json", "chrono"
    # 不启用 "any"：MySQL/PG 用强类型 MySqlPool/PgPool 按 kind 分发（AnyPool 有类型擦除损耗）
] }                                         # MIT/Apache-2.0，纯 rustls（原则 1，禁 native-tls）
arrow = "58"                               # 与 datafusion 54 一致（datafusion 依赖 arrow 58.3.0），RecordBatch/Schema
async-trait = "0.1"                         # SQLExecutor trait
# 复用 ingest 的 calamine（Excel→MemTable）
ingest.workspace = true
memory.workspace = true
# workspace 统一
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
uuid.workspace = true
specta = { version = "2.0.0-rc.25", features = ["derive"] }  # IPC 类型桥（决策 F5）
```

> **版本实证**（docs.rs/crate/datafusion/54.0.0/source/Cargo.toml）：datafusion 54.0.0 → arrow 58.3.0 + sqlparser 0.62.0。**sqlparser 不需单独依赖**——datafusion 自带，只读护栏复用 `datafusion::sql::sqlparser`。workspace 零既有 arrow 依赖（ingest/memory 均未用），无版本冲突。
>
> **datafusion-federation 0.5.5 兼容性**：datafusion-table-providers 0.13 的 workspace Cargo.toml 同时锁 `datafusion = "54.0"` + `datafusion-federation = "0.5.5"`，证明兼容。federation 0.5.5 是 alpha（README 标注），三期单用户桌面可接受，兜底见 §八。

### 4.3 `crates/agent-core` 新增

```
crates/agent-core/src/
├── federation_tools.rs    ← 新增：联邦查询 DynamicTool 集合（~120 行，同构 document_tools.rs）
└── chat.rs                ← 修改：ChatService 加 federation 字段 + stream_with_memory 注入
```

**ChatService 改动**（对齐现有 `memory`/`raw_memory`/`tool_handle` 注入模式）：
```rust
// chat.rs ChatService 结构体新增字段
pub struct ChatService {
    // ... 现有字段 ...
    federation: Option<Arc<FederationService>>,  // 新增
}

// stream_with_memory 里，与 document_tools 并列注入
let doc_tools: Vec<_> = match &self.raw_memory {
    Some(m) => crate::document_tools::document_tools(m.clone()),
    None => Vec::new(),
};
let fed_tools: Vec<_> = match &self.federation {
    Some(f) => crate::federation_tools::federation_tools(f.clone()),
    None => Vec::new(),
};
let all_tools: Vec<_> = doc_tools.into_iter().chain(fed_tools).collect();
let handle = if !all_tools.is_empty() {
    let h = handle.clone().unwrap_or_else(|| rig::tool::server::ToolServer::new().run());
    for tool in all_tools { h.add_dynamic_tool(tool).await; }
    Some(h)
} else { handle };
```

**federation_tools.rs**（参照 `document_tools.rs` 的 `DynamicTool::new(name, desc, params, callback)` 模式，闭包捕获 `Arc<FederationService>`，见 §3.1 三工具）。

### 4.4 `src-tauri` IPC 薄层（4 个 command）

```rust
#[tauri::command] #[specta::specta]
pub async fn register_data_source(input: DataSourceConfig) -> Result<DataSourceSummary, AppError>;
#[tauri::command] #[specta::specta]
pub async fn test_data_source(input: DataSourceConfig) -> Result<SchemaSnapshot, AppError>;
#[tauri::command] #[specta::specta]
pub async fn browse_schema(source_id: Uuid) -> Result<SchemaSnapshot, AppError>;
// execute_sql 不暴露为 IPC command（agent 工具内部调用，不直接给前端）
// /sources 工作区手动 SQL 走独立 command（见 §五）
#[tauri::command] #[specta::specta]
pub async fn run_query(sql: String, source_id: Option<Uuid>) -> Result<QueryResult, AppError>;
```

---

## 五、前端设计

### 5.1 设置页瘦身重构（前置，不依赖三期后端）

当前 `SettingsView.tsx`（380 行）问题：长表单滚动、单文件膨胀、保存模型混乱。**重构为左导航 + 右面板**（VSCode/Beekeeper 范式）：

```
src/components/settings/
├── SettingsView.tsx        ← 骨架：左导航 + 右面板切换（~80 行）
├── ProviderPanel.tsx       ← 从现 SettingsView 抽出
├── McpPanel.tsx            ← 从现 McpSection 抽出
├── AppearancePanel.tsx     ← 从现 SettingsView 抽出外观
└── shared.tsx              ← Section/Field/input 样式提取（现有内联 <style>）
```

设置页只留：模型提供商 / MCP 工具 / 外观（**数据源不进设置页**）。
当前选中分类存 `ui-store.settingsTab`（与 `theme`/`sidebarCollapsed` 同层），打开记住位置。

### 5.2 `/sources` 数据源工作区（新建路由，对标 `/library`）

参考 **DBX**（github.com/t8y2/dbx，Tauri 2 + Rust(sqlx) + shadcn-vue + AI SQL，技术栈 90% 重合）。DBX 的设计哲学：**数据源是工作区一等公民，不是设置项**。

```
/routes/sources.tsx（新）
├─ 左栏（~280px）：连接列表 + [新建连接] 按钮 + schema 树
│   ├─ 连接卡片（名称 + 类型 + 状态徽标 + 颜色标记）
│   └─ schema 树（react-arborist）：Connection > Schema > Table > Column
│       右键菜单（@radix-ui/react-context-menu）：刷新 / 编辑连接 / 断开
├─ 中栏：SQL 编辑器（CodeMirror 6）+ 执行按钮 + 结果表格
│   ├─ 编辑器补全（表名/列名，数据来自 information_schema）
│   ├─ ⌘+Enter 执行（DBX 范式）
│   ├─ 写 SQL 按钮灰掉 + 提示「只读模式」
│   └─ 结果表格（@tanstack/react-table）+ 执行信息（耗时/涉及源/行数）
└─ 右栏（~300px）：表结构详情 + 前 5 行样本
```

入口：Sidebar 加「数据源」导航项（与「文件库」对称）+ 快捷键。

### 5.3 组件选型

| 组件 | 选型 | npm 包 | 理由 |
|---|---|---|---|
| SQL 编辑器 | CodeMirror 6 | `codemirror` + `@codemirror/lang-sql` | DBX 同款，轻量，与 Tauri 小体积理念一致（Monaco 重） |
| schema 树 | react-arborist | `react-arborist` | ARCHITECTURE.md §17.2 已指定 |
| 右键菜单 | shadcn ContextMenu | `@radix-ui/react-context-menu`（shadcn 复制源码模式） | 与 react-arborist 叠加 |
| 结果表格 | TanStack Table | `@tanstack/react-table` | TanStack 生态一致 |

> **SQL 补全**：`@codemirror/lang-sql` 自带 SQL 语法补全，补全数据源（表名/列名）通过 `SQLConfig.tables` 注入，数据来自 `browse_schema` IPC。无需额外 autocomplete 扩展包。

### 5.4 DataSourcePanel 不单独建（在 `/sources` 内）

数据源注册/编辑/测试连接表单**在 `/sources` 工作区内**（DBX 范式：连接管理在侧栏，不在设置页）：
- [新建连接] → 展开/弹窗表单（类型选择 + host/port/db/user/pass + SSL + 测试连接 + 保存）
- CSV/Excel 类型：隐藏网络字段，显文件路径选择器（`@tauri-apps/plugin-dialog`）
- 连接表单字段参考 Beekeeper（连接类型下拉 / TCP 模式 / SSL 三档 disable-require-verify）

---

## 六、数据契约（对齐 ARCHITECTURE.md §13.5）

```rust
// ── 数据源（federation crate 定义，specta::Type 供 IPC） ──
#[derive(Serialize, Deserialize, Type, Clone)]
pub struct DataSourceConfig {
    pub id: Uuid,
    pub kind: DataSourceKind,          // MySQL | PostgreSQL | CSV | Excel
    pub name: String,                  // catalog 名（三段式寻址用）
    pub connection: serde_json::Value, // {host,port,db,user,pass} 或 {path}；凭证字段单独建模，预留 keyring 迁移
    pub color: Option<String>,         // 连接颜色标记（DBX 范式：红=生产/蓝=测试/绿=本地）
}

#[derive(Serialize, Deserialize, Type)]
pub struct DataSourceSummary {
    pub id: Uuid,
    pub name: String,
    pub kind: DataSourceKind,
    pub connected: bool,               // 连接状态
    pub table_count: Option<usize>,    // 表数（已连接时）
    pub last_error: Option<String>,    // 连接失败原因
}

#[derive(Serialize, Deserialize, Type)]
pub struct SchemaSnapshot {
    pub tables: Vec<TableMeta>,        // {name, columns: Vec<ColumnMeta>, row_count_estimate}
}

#[derive(Serialize, Deserialize, Type)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<serde_json::Value>,  // Arrow → JSON
    pub row_count: usize,
    pub elapsed_ms: u64,
    pub sources_touched: Vec<Uuid>,    // 透明性：查询涉及哪些源
}
```

> 凭证明文存 SQLite（用户拍板，对齐决策 10）。`connection` 字段凭证单独建模，二期迁 keyring 时只改存储后端不改结构。

---

## 七、落地路径

### 7.1 顺序（用户拍板：设置页重构 → 后端 → 前端工作区）

```
阶段 0：设置页瘦身重构（前置，不依赖三期后端，可立即做）
  └─ 拆 SettingsView 为左导航 + 三 Panel，数据源 tab 占位待移除

阶段 1：后端 federation crate（CSV 先行验证全链路）
  ├─ 1a. crates/federation 骨架 + Cargo.toml + datafusion/sqlx 依赖
  ├─ 1b. CSV provider（DataFusion 内置 register_csv，零 SQLExecutor）
  │       + execute_sql + 只读护栏 + Arrow→JSON → cargo test 跑通
  ├─ 1c. federation_tools.rs + ChatService 注入 → agent 端到端验证
  ├─ 1d. MySQL/PG SQLExecutor（sqlx + rustls + datafusion-federation）
  └─ 1e. Excel provider（calamine → MemTable，复用 ingest）

阶段 2：IPC 薄层（src-tauri）
  └─ register_data_source / test_data_source / browse_schema / run_query + specta 绑定生成

阶段 3：前端 /sources 工作区
  ├─ 3a. 路由 + 连接列表 + 新建连接表单（含测试连接）
  ├─ 3b. schema 树（react-arborist + 右键菜单）
  ├─ 3c. SQL 编辑器（CodeMirror 6 + 补全）+ 结果表格
  └─ 3d. ToolCallCard 加 SQL 确认执行交互（§3.4）
```

### 7.2 验证标准

- `crates/federation` 独立 `cargo test` 通过（原则 4：平台无关，不依赖 src-tauri）
- **CSV 端到端**（阶段 1b，零外部依赖）：注册 CSV → `list_data_sources` → `describe_table` → `execute_sql` 返回行集。测试样本放 `crates/federation/tests/data/`
- **MySQL/PG**（阶段 1d，需测试库）：
  ```bash
  docker run --name pg-test -e POSTGRES_PASSWORD=password -e POSTGRES_DB=test -p 5432:5432 -d postgres:16-alpine
  docker run --name mysql-test -e MYSQL_ROOT_PASSWORD=password -e MYSQL_DATABASE=test -p 3306:3306 -d mysql:9
  ```
  测试用 `#[ignore]` 标注需 docker 的用例，CI 默认跳过本地手跑
- **跨源 JOIN 下推验证**：用 `EXPLAIN SELECT ... FROM pg.t1 JOIN mysql.t2 ...`，检查计划含 federation 节点（而非全量拉取）
- 前端：设置页三 tab 切换正常；`/sources` 注册→浏览→查询全流程

### 7.3 实现完成状态（2026-07-30 更新）

> 本设计文档的落地偏离了原计划的「设置页重构 + `/sources` 路由」路径，改为「独立 FederationView 覆盖层 + Sidebar 入口」。后端基本对齐，前端架构调整。

**✅ 已完成**:
- **阶段 1 后端**（`crates/federation/`）：DataFusion 54 + datafusion-federation 0.5.5 SQLExecutor（MySQL/PG 用 sqlx 0.9 + rustls）；CSV 走 temp table 模式（DF 54 无 schema provider 限制）；只读守卫 + 自动 LIMIT + 30s 超时；schema 浏览（三段式 information_schema）；10 测试通过
- **阶段 2 IPC**（`src-tauri/src/commands/federation.rs`）：9 命令（原设计 4 个，扩展为 register/test/deregister/list/get + browse/describe + execute/explain）+ specta 绑定生成
- **阶段 3 前端**：`FederationView.tsx` 三栏工作台（左:数据源列表；中:SQL 编辑器+结果表；右:schema 树）+ 注册对话框；`hooks/useFederation.ts`（TanStack Query）；Sidebar 底部入口 + ⌘Shift+F
- **BigInt 公约**（新决策 18）：`#[specta(type = specta_typescript::Number)]` 逐字段注解方案落地

**⏳ 未完成/偏离**:
- Excel provider（阶段 1e，calamine→MemTable）——未实现
- Agent 工具化（阶段 1c，`federation_tools.rs` + ChatService 注入 + ToolCallCard SQL 确认交互）——未实现，当前仅 IPC 直调，未作为 rig DynamicTool 暴露给 LLM
- 设置页瘦身重构（阶段 0）——未做，federation 独立成覆盖层而非并入设置页
- schema 树用 lucide 手写折叠（非 react-arborist）；SQL 编辑器用原生 textarea（非 CodeMirror 6 补全）
- 跨源 JOIN 下推验证（§7.2）——需 docker 测试库，未跑

详见 `PROGRESS.md`「联邦查询全栈」节。

---

## 八、风险与缓解

| 风险 | 缓解 |
|---|---|
| datafusion-federation 0.5.5 是 alpha（README 标注） | 三期单用户桌面， federation 失败可 fallback 到「全量拉取本地 JOIN」（SQLExecutor 不下推，scan 全表）。但大表爆内存——仅作兜底，主路径依赖 federation |
| sqlx 跨方言类型差异 | MySQL/PG 分别用强类型 `MySqlPool`/`PgPool`，按 `DataSourceKind` 分发（不用 AnyPool，避免类型擦除损耗）。§4.2 已不启用 `any` feature |
| CodeMirror 6 SQL 补全对接 information_schema | 补全 source 查 `information_schema.columns`，与 agent 工具同源，复用 `browse_schema` IPC |
| 凭证明文安全 | 三期对齐决策 10（同 provider API key 明文）；二期统一加密时 keyring 迁移，结构已预留 |

---

## 九、与既有架构的关系

- **不修订 ARCHITECTURE.md 既有决策**：决策 9/10/11/12 沿用，本文件是落地细化
- **ARCHITECTURE.md 落地路线三期章节**引用本文件：原「数据源注册→本体建模→DataFusion→TextQL→Agent 工具化」细化为本文件 §七
- **本文件优先级**：在三期范围内，本文件是权威；与 ARCHITECTURE.md 冲突时以 ARCHITECTURE.md 为准（但目前无冲突）

---

## 附录 A：关键实证来源（开发人员核查用）

所有技术声明可追溯，避免「文档说但实际不是」的坑：

| 声明 | 来源 |
|---|---|
| datafusion 54.0.0 依赖 arrow 58.3.0 + sqlparser 0.62.0 | docs.rs/crate/datafusion/54.0.0/source/Cargo.toml（源码实证） |
| datafusion 默认启用 `unparser` feature | docs.rs datafusion 54 feature 列表 |
| datafusion-federation 0.5.5 兼容 datafusion 54 | datafusion-table-providers 0.13 workspace Cargo.toml 同时锁两者 |
| `SQLExecutor` trait 6 个方法签名 | docs.rs/datafusion-federation/latest/datafusion_federation/sql/trait.SQLExecutor.html |
| federation 集成 5 步（FederationOptimizerRule + FederatedQueryPlanner） | datafusion-federation example `df-csv-advanced.rs` |
| DataFusion 49+ 协作式取消（task budget） | datafusion.apache.org/blog/2025/06/30/cancellation/ |
| DataFusion 内置 information_schema 统一视图 | datafusion.apache.org/user-guide/sql/information_schema.html |
| catalog/schema/table 三级层级 | datafusion.apache.org/library-user-guide/catalogs.html |
| sqlx 0.9 rustls feature 组合（mysql+postgres+tls-rustls-ring） | docs.rs/crate/sqlx/latest/features |
| `datafusion-table-providers` 硬编码 native-tls（不采用原因） | crates/mysql/Cargo.toml + crates/postgres/Cargo.toml 源码 |
| rig 无原生 DataFusion/SQL 集成 | github.com/0xPlaygrounds/rig（仅 provider/vector/RAG/MCP） |
| DBX（Tauri+sqlx+shadcn+AI SQL）前端范式 | github.com/t8y2/dbx + dbxio.com/en/docs |
| Beekeeper 连接表单字段范式（SSL 三档/SSH 隧道） | docs.beekeeperstudio.io/user_guide/connecting/connecting/ |
| Spice 生产联邦架构（federation 下推 + unparser 方言） | spice.ai/blog/how-we-use-apache-datafusion-at-spice-ai |
| Text2SqlAgent agentic 单工具范式（Spider 95%） | github.com/Text2SqlAgent/text2sql-framework |
