//! federation: 联邦查询服务（见 PHASE3-FEDERATION.md）。
//!
//! 架构（§2.1 DataFusion 作为 rig 的 DynamicTool）：
//!   - `FederationService` 持全局单例 `Arc<SessionContext>`（§2.5）
//!   - 数据源配置持久化到 memory 的 SQLite（决策 10），运行时热注册/注销到 catalog
//!   - Agent 工具（list_data_sources / describe_table / execute_sql）经
//!     `crates/agent-core/src/federation_tools.rs` 注入，走同一条 ToolServerHandle
//!
//! 三期范围（§1.2）：MySQL/PG/CSV/Excel，只读 SELECT/WITH，无本体建模。
//! 本文件含 SessionContext 构造（§2.5）+ 数据源持久化 CRUD + 热注册入口。

pub mod catalog;
pub mod error;
pub mod executor;
pub mod query;
pub mod schema;
pub mod source;

use std::sync::Arc;

use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::optimizer::Optimizer;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_federation::{FederatedQueryPlanner, FederationOptimizerRule};
use memory::{Memory, Timestamp};
use rusqlite::params;

pub use error::{FederationError, FederationResult};
pub use source::{
    ColumnMeta, ConnectionConfig, DataSourceConfig, DataSourceKind, DataSourceSummary,
    DbConnection, FileConnection, QueryResult, SchemaSnapshot, TableMeta,
};

/// 联邦查询服务：全局单例 SessionContext + 数据源持久化。
///
/// 对齐 `Arc<Memory>` 单例模式（§2.5）：应用启动构造一次，注入 ChatService。
/// 复用 sqlx 连接池 + schema 缓存，避免每会话重连重探。
pub struct FederationService {
    ctx: Arc<SessionContext>,
    memory: Arc<Memory>,
    /// 每个已注册数据源的 schema 快照（register 时生成，避免 browse_schema 再查联邦
    /// information_schema——后者会合并相同 compute_context 的多 catalog 表名）。
    snapshots: Arc<std::sync::RwLock<std::collections::HashMap<String, Arc<crate::source::SchemaSnapshot>>>>,
}

impl Clone for FederationService {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            memory: self.memory.clone(),
            snapshots: Arc::clone(&self.snapshots),
        }
    }
}

impl FederationService {
    /// 构造服务：初始化 SessionContext（含联邦规则）+ 数据源表 + 恢复已注册源。
    pub async fn new(memory: Arc<Memory>) -> FederationResult<Self> {
        let ctx = build_session_context()?;
        let svc = Self { ctx, memory, snapshots: Default::default() };
        svc.init_storage()?;
        // 恢复热状态：从 SQLite 读所有数据源，重新注册到 catalog（§2.5 配套设计 2）
        svc.restore_sources().await;
        Ok(svc)
    }

    /// 测试用：内存库 + 不恢复源（避免文件不存在报错）。
    pub async fn new_for_test(memory: Arc<Memory>) -> FederationResult<Self> {
        let ctx = build_session_context()?;
        let svc = Self { ctx, memory, snapshots: Default::default() };
        svc.init_storage()?;
        Ok(svc)
    }

    /// 暴露 SessionContext（agent 工具 / IPC run_query 用）。
    pub fn ctx(&self) -> &Arc<SessionContext> {
        &self.ctx
    }

    pub fn memory(&self) -> &Arc<Memory> {
        &self.memory
    }

    /// 浏览某 catalog 的 schema 快照（register 时缓存，单源精确，避免跨 catalog 合并）。
    /// 未命中缓存时回退到 schema::browse_schema（兼容旧路径，但可能合并）。
    pub async fn browse_schema(&self, catalog: &str) -> FederationResult<crate::source::SchemaSnapshot> {
        if let Ok(m) = self.snapshots.read() {
            if let Some(snap) = m.get(catalog) {
                return Ok((**snap).clone());
            }
        }
        tracing::warn!(catalog = %catalog, "schema 快照未命中缓存，回退到联邦 information_schema 查询（可能跨 catalog 合并）");
        crate::schema::browse_schema(&self.ctx, catalog).await
    }

    // ── 数据源持久化 CRUD（落 memory SQLite，data_sources 表） ──────

    /// 初始化 data_sources 表（幂等）。
    fn init_storage(&self) -> FederationResult<()> {
        let conn = self.memory.lock();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS data_sources (
                id          TEXT PRIMARY KEY,           -- UUID v4
                name        TEXT NOT NULL UNIQUE,       -- catalog 名（三段式寻址用）
                kind        TEXT NOT NULL,              -- mysql|postgres|csv|excel
                connection  TEXT NOT NULL,              -- JSON: ConnectionConfig
                color       TEXT,
                created_at  INTEGER NOT NULL            -- unix ms
            );
            "#,
        )
        .map_err(|e| FederationError::Storage(e.to_string()))?;
        Ok(())
    }

    /// 注册数据源：落 SQLite + 注册到 catalog（热生效）。
    /// 返回摘要（含连接探测结果）。
    pub async fn register(&self, config: DataSourceConfig) -> FederationResult<DataSourceSummary> {
        // 校验 catalog 名合法（DataFusion 约束：不含点/空格）
        validate_catalog_name(&config.name)?;

        let conn_json = serde_json::to_string(&config.connection)?;
        {
            let conn = self.memory.lock();
            conn.execute(
                "INSERT INTO data_sources(id, name, kind, connection, color, created_at)
                 VALUES(?, ?, ?, ?, ?, ?)
                 ON CONFLICT(name) DO UPDATE SET
                    kind=excluded.kind, connection=excluded.connection, color=excluded.color",
                params![
                    config.id.to_string(),
                    config.name,
                    config.connection.kind().as_str(),
                    conn_json,
                    config.color,
                    config.created_at,
                ],
            )
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        }

        // 注册到 catalog + 探测连接
        let summary = match catalog::register_source(&self.ctx, &config).await {
            Ok(snapshot) => {
                let table_count = snapshot.tables.len();
                // 缓存 schema 快照（供 browse_schema 直接返回，避免跨 catalog 合并）
                if let Ok(mut m) = self.snapshots.write() {
                    m.insert(config.name.clone(), Arc::new(snapshot));
                }
                DataSourceSummary {
                    id: config.id,
                    name: config.name.clone(),
                    kind: config.connection.kind(),
                    connected: true,
                    table_count: Some(table_count),
                    last_error: None,
                }
            },
            Err(e) => {
                tracing::warn!(source = %config.name, error = %e, "register source connect failed");
                DataSourceSummary {
                    id: config.id,
                    name: config.name.clone(),
                    kind: config.connection.kind(),
                    connected: false,
                    table_count: None,
                    last_error: Some(e.to_string()),
                }
            }
        };
        Ok(summary)
    }

    /// 删除数据源：从 catalog 注销 + 删 SQLite 记录。
    ///
    /// 注：DataFusion 54 的 SessionContext 无直接 deregister_catalog，
    /// catalog 在进程内驻留至下次重建（三期单用户可接受；重启后从 SQLite 恢复时不会重载已删源）。
    /// 若需立即生效，调用方需重建 FederationService（罕见场景）。
    pub async fn deregister(&self, id: &str) -> FederationResult<()> {
        // 先查出 name（catalog 名）记录，删除 SQLite 记录
        let name = self.find_name_by_id(id)?;
        if let Some(ref n) = name {
            catalog::deregister_source(&self.ctx, n);
            if let Ok(mut m) = self.snapshots.write() {
                m.remove(n);
            }
        }
        {
            let conn = self.memory.lock();
            conn.execute("DELETE FROM data_sources WHERE id = ?", params![id])
                .map_err(|e| FederationError::Storage(e.to_string()))?;
        }
        Ok(())
    }

    /// 列出所有已注册数据源（含连接状态探测，§3.1 list_data_sources）。
    pub async fn list_sources(&self) -> FederationResult<Vec<DataSourceSummary>> {
        let rows = self.load_all_configs()?;
        let mut out = Vec::with_capacity(rows.len());
        for config in rows {
            let (connected, table_count, last_error) =
                catalog::probe_source(&self.ctx, &config).await;
            out.push(DataSourceSummary {
                id: config.id,
                name: config.name.clone(),
                kind: config.connection.kind(),
                connected,
                table_count,
                last_error,
            });
        }
        Ok(out)
    }

    /// 测试连接（不落库，仅探测 + 返回 schema 快照）。
    /// 用于前端「测试连接」按钮（§5.4）。
    pub async fn test_connection(
        &self,
        config: &DataSourceConfig,
    ) -> FederationResult<SchemaSnapshot> {
        let t0 = std::time::Instant::now();
        // 临时注册到独立 catalog 名探测，再注销
        let probe_name = format!("__probe_{}", uuid::Uuid::new_v4().simple());
        let mut probe_config = config.clone();
        probe_config.name = probe_name.clone();
        // 临时注册不落库
        tracing::info!(target: "federation::test_connection", source = %probe_name, "register_source begin");
        let snapshot = catalog::register_source(&self.ctx, &probe_config).await?;
        tracing::info!(target: "federation::test_connection", source = %probe_name, n = snapshot.tables.len(), elapsed = ?t0.elapsed(), "register_source done (含 schema 快照)");
        catalog::deregister_source(&self.ctx, &probe_name);
        Ok(snapshot)
    }

    /// 按 id 取配置。
    pub fn get_config(&self, id: &str) -> FederationResult<Option<DataSourceConfig>> {
        let conn = self.memory.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, kind, connection, color, created_at FROM data_sources WHERE id = ?")
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(params![id])
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        match rows.next().map_err(|e| FederationError::Storage(e.to_string()))? {
            Some(r) => Ok(Some(row_to_config(r)?)),
            None => Ok(None),
        }
    }

    /// 按 name 取 catalog（agent 工具用 name 寻址）。
    pub fn find_config_by_name(&self, name: &str) -> FederationResult<Option<DataSourceConfig>> {
        let conn = self.memory.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, kind, connection, color, created_at FROM data_sources WHERE name = ?")
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(params![name])
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        match rows.next().map_err(|e| FederationError::Storage(e.to_string()))? {
            Some(r) => Ok(Some(row_to_config(r)?)),
            None => Ok(None),
        }
    }

    fn find_name_by_id(&self, id: &str) -> FederationResult<Option<String>> {
        let conn = self.memory.lock();
        let mut stmt = conn
            .prepare("SELECT name FROM data_sources WHERE id = ?")
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(params![id])
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        match rows.next().map_err(|e| FederationError::Storage(e.to_string()))? {
            Some(r) => Ok(Some(r.get::<_, String>(0).map_err(|e| FederationError::Storage(e.to_string()))?)),
            None => Ok(None),
        }
    }

    fn load_all_configs(&self) -> FederationResult<Vec<DataSourceConfig>> {
        let conn = self.memory.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, kind, connection, color, created_at FROM data_sources ORDER BY created_at ASC")
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map([], row_to_config)
            .map_err(|e| FederationError::Storage(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| FederationError::Storage(e.to_string()))?);
        }
        Ok(out)
    }

    /// 启动时恢复：从 SQLite 读所有源，重新注册到 catalog。
    /// 文件型源（CSV/Excel）文件不存在时标记 ⚠️ 但不阻断启动。
    async fn restore_sources(&self) {
        match self.load_all_configs() {
            Ok(configs) => {
                for config in configs {
                    match catalog::register_source(&self.ctx, &config).await {
                        Ok(snapshot) => {
                            let n = snapshot.tables.len();
                            if let Ok(mut m) = self.snapshots.write() {
                                m.insert(config.name.clone(), Arc::new(snapshot));
                            }
                            tracing::info!(source = %config.name, tables = n, "restored data source");
                        }
                        Err(e) => {
                            tracing::warn!(source = %config.name, error = %e, "restore data source failed (will retry on next register)");
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "load data sources from storage failed");
            }
        }
    }
}

/// 构造全局 SessionContext（§2.5 完整构造代码）。
///
/// 关键：FederationOptimizerRule + FederatedQueryPlanner 必须同时注册，
/// 否则 federation 不生效（官方 example df-csv-advanced.rs 实证）。
/// RuntimeEnv 设 512MB 内存上限（防 OOM，桌面应用必须设）。
fn build_session_context() -> FederationResult<Arc<SessionContext>> {
    let runtime = RuntimeEnvBuilder::new()
        .with_memory_limit(512 * 1024 * 1024, 0.8)
        .build_arc()
        .map_err(|e| FederationError::Other(format!("runtime env: {e}")))?;

    let config = SessionConfig::new()
        .with_target_partitions(4) // 桌面 4 核
        .with_information_schema(true); // 启用 information_schema 统一视图（§2.2）

    // 取默认优化规则 + 追加联邦规则
    let mut rules = Optimizer::new().rules;
    rules.push(Arc::new(FederationOptimizerRule::new()));

    let state = SessionStateBuilder::new()
        .with_config(config)
        .with_runtime_env(runtime)
        .with_optimizer_rules(rules)
        .with_query_planner(Arc::new(FederatedQueryPlanner::new()))
        .with_default_features()
        .build();

    // datafusion 54 用 From<SessionState> 构造 SessionContext
    Ok(Arc::new(SessionContext::from(state)))
}

/// 校验 catalog 名合法（DataFusion catalog 名约束）。
/// 禁止点（破坏三段式寻址）、空格、特殊字符。允许字母数字下划线。
fn validate_catalog_name(name: &str) -> FederationResult<()> {
    if name.is_empty() {
        return Err(FederationError::InvalidConfig("数据源名不能为空".into()));
    }
    if name.contains('.') {
        return Err(FederationError::InvalidConfig(
            "数据源名不能含点（破坏三段式 catalog.schema.table 寻址）".into(),
        ));
    }
    let valid = name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return Err(FederationError::InvalidConfig(
            "数据源名只能含字母数字、下划线、连字符".into(),
        ));
    }
    Ok(())
}

/// rusqlite Row → DataSourceConfig。
fn row_to_config(
    row: &rusqlite::Row<'_>,
) -> Result<DataSourceConfig, rusqlite::Error> {
    let id_str: String = row.get(0)?;
    let name: String = row.get(1)?;
    let _kind: String = row.get(2)?; // kind 已编码在 connection JSON 内，此处仅校验
    let conn_str: String = row.get(3)?;
    let color: Option<String> = row.get(4)?;
    let created_at: Timestamp = row.get(5)?;

    let connection: ConnectionConfig =
        serde_json::from_str(&conn_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?;
    Ok(DataSourceConfig {
        id: id_str,
        name,
        connection,
        color,
        created_at,
    })
}
