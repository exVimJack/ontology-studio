//! source: 数据源配置与契约（见 PHASE3-FEDERATION.md §六 数据契约）。
//!
//! 设计说明：
//!   - 文档 §六 把 connection 建模为 `serde_json::Value`（预留 keyring 迁移），
//!     但 Rust 侧用强类型枚举更安全、specta 生成的 TS 类型更清晰。
//!     `ConnectionConfig` 按 `DataSourceKind` 分变体，序列化为 JSON 落 SQLite，
//!     反序列化时按 kind 还原——与文档语义等价，仅类型表达更强。
//!   - 凭证明文存 SQLite（用户拍板，对齐决策 10）；二期迁 keyring 时只改存储后端。

use memory::Timestamp;
use serde::{Deserialize, Serialize};

use specta_typescript::Number;
use specta::Type as SpectaType;

/// 数据源类型（三期范围：MySQL/PG/CSV/Excel）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, SpectaType)]
#[serde(rename_all = "lowercase")]
pub enum DataSourceKind {
    Mysql,
    Postgres,
    Csv,
    Excel,
}

impl DataSourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Postgres => "postgres",
            Self::Csv => "csv",
            Self::Excel => "excel",
        }
    }
}

/// 数据库连接配置（MySQL/PG）。CSV/Excel 走 FileConnection。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct DbConnection {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    /// 明文密码（三期，对齐决策 10）。二期迁 keyring 时改加密后端。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// SSL 模式：disable / require / verify（Beekeeper 三档范式）。
    /// 三期默认 require（rustls），verify 暂不校验 CA（桌面单用户）。
    #[serde(default = "default_ssl_mode")]
    pub ssl_mode: String,
}

fn default_ssl_mode() -> String {
    "require".to_string()
}

/// 文件型连接配置（CSV/Excel）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct FileConnection {
    /// 源文件绝对路径。CSV 可指向目录（注册目录下所有 .csv）。
    pub path: String,
    /// CSV 是否有表头（仅 CSV 有效）。
    #[serde(default = "default_has_header")]
    pub has_header: bool,
    /// CSV 分隔符（默认逗号）。TSV 用 "\t"。
    #[serde(default = "default_delimiter")]
    pub delimiter: String,
}

fn default_has_header() -> bool {
    true
}

fn default_delimiter() -> String {
    ",".to_string()
}

/// 连接配置（按 kind 分变体）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
#[serde(tag = "kind", content = "params", rename_all = "lowercase")]
pub enum ConnectionConfig {
    Mysql(DbConnection),
    Postgres(DbConnection),
    Csv(FileConnection),
    Excel(FileConnection),
}

impl ConnectionConfig {
    /// 对应的 DataSourceKind。
    pub fn kind(&self) -> DataSourceKind {
        match self {
            Self::Mysql(_) => DataSourceKind::Mysql,
            Self::Postgres(_) => DataSourceKind::Postgres,
            Self::Csv(_) => DataSourceKind::Csv,
            Self::Excel(_) => DataSourceKind::Excel,
        }
    }
}

/// 规范化主机名：`localhost` → `127.0.0.1`。
///
/// Windows 上 `localhost` 解析极慢（IPv6 `::1` 优先 + TCP 连接被拒后回退 IPv4），
/// sqlx 每次新建连接都走一次解析，57 表 schema 拉取会累积到 40+ 秒。
/// 规范化成 `127.0.0.1` 后连接耗时从 21s 降到 60ms（实测）。
/// 远程主机名不改动（用户显式填的域名/DNS，由系统解析器处理）。
pub fn normalize_host(host: &str) -> String {
    if host.trim().eq_ignore_ascii_case("localhost") {
        "127.0.0.1".to_string()
    } else {
        host.trim().to_string()
    }
}

/// 数据源完整配置（落 SQLite + 注册到 DataFusion catalog）。
///
/// `name` 同时作为 catalog 名（三段式 `catalog.schema.table` 寻址用），
/// 必须唯一且合法（DataFusion catalog 名约束：字母数字下划线，不含点）。
///
/// `id` 用 String 存 UUID v4（对齐 memory 的 conversation id 范式，
/// 避免 specta uuid feature 依赖）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct DataSourceConfig {
    pub id: String,
    pub name: String,
    pub connection: ConnectionConfig,
    /// 连接颜色标记（DBX 范式：红=生产/蓝=测试/绿=本地）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 创建时间（unix ms）。
    pub created_at: Timestamp,
}

/// 数据源摘要（list_data_sources 工具 + 前端连接列表）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct DataSourceSummary {
    pub id: String,
    pub name: String,
    pub kind: DataSourceKind,
    /// 当前连接状态（启动/注册时探测）。
    pub connected: bool,
    /// 表数（已连接时；CSV/Excel = 文件内 sheet 数）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Number)]
    pub table_count: Option<usize>,
    /// 最近一次连接/查询失败原因（前端展示 ⚠️）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// 列元信息（information_schema.columns 子集）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct ColumnMeta {
    pub name: String,
    /// Arrow 类型名（如 Utf8、Int64、Float64、Date32）。
    pub data_type: String,
    /// 是否可空。
    pub nullable: bool,
}

/// 表元信息。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct TableMeta {
    pub name: String,
    pub columns: Vec<ColumnMeta>,
    /// 行数估计（information_schema 或 COUNT(*)，可能为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(type = Number)]
    pub row_count_estimate: Option<i64>,
    /// 前 5 行样本（describe_table 返回；list 时不带）。
    /// 每行为 JSON 序列化字符串（对齐 mcp.rs 范式：serde_json::Value 不实现 specta::Type）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_rows: Vec<String>,
}

/// schema 快照（browse_schema / test_data_source 返回）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct SchemaSnapshot {
    pub tables: Vec<TableMeta>,
}

/// 查询结果（execute_sql / run_query 返回）。
#[derive(Debug, Clone, Serialize, Deserialize, SpectaType)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    /// Arrow 行 → JSON 字符串数组（每行一个 JSON 对象字符串，前端 JSON.parse）。
    /// 用 String 而非 serde_json::Value：对齐 mcp.rs 范式（Value 不实现 specta::Type）。
    pub rows: Vec<String>,
    #[specta(type = Number)]
    pub row_count: usize,
    #[specta(type = Number)]
    pub elapsed_ms: u64,
    /// 透明性：查询涉及的源（catalog 名）。
    pub sources_touched: Vec<String>,
    /// SQL 执行计划摘要（EXPLAIN，调试/审计用，可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<String>,
}
