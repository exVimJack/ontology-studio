//! 联邦查询 IPC 命令（§三期：DataFusion 联邦查询）。
//!
//! 薄封装 `crates/federation`（AGENTS.md 工程结构硬约束：业务在 crates/，此处只 IPC）。
//! 数据源管理（注册/测试/注销/列表）+ schema 探查 + 只读 SQL 执行 + EXPLAIN。
//!
//! 只读护栏在 federation::query::assert_readonly（sqlparser 仅 Statement::Query/Explain），
//! 自动 LIMIT（默认 200/上限 1000）+ 30s 超时均在 federation 层。
//!
//! FederationService: Clone（两字段皆 Arc，克隆廉价），故命令内 clone 一份取出，
//! 避免持 RwLock read guard 跨 await。

use tauri::State;

use federation::{
    DataSourceConfig, DataSourceSummary, FederationService, QueryResult, SchemaSnapshot, TableMeta,
};

use crate::commands::error::{AppError, AppResult};
use crate::state::AppState;

/// 取已就绪的 FederationService 克隆。未初始化完成返回错误。
async fn get_svc(state: &AppState) -> AppResult<FederationService> {
    state
        .federation
        .read()
        .await
        .as_ref()
        .ok_or_else(|| AppError::Federation("联邦查询服务尚未初始化完成".into()))
        .cloned()
}

// ── 数据源管理 ─────────────────────────────────────────────────

/// 注册数据源（落 SQLite + 热注册到 SessionContext + 探测连接）。
#[tauri::command]
#[specta::specta]
pub async fn register_data_source(
    config: DataSourceConfig,
    state: State<'_, AppState>,
) -> AppResult<DataSourceSummary> {
    let t = std::time::Instant::now();
    let kind = format!("{:?}", config.connection);
    tracing::info!(name = %config.name, kind = %kind, "register_data_source invoked");
    let svc = get_svc(&state).await?;
    let r = svc.register(config).await;
    match &r {
        Ok(s) => tracing::info!(name = %s.name, tables = s.table_count, elapsed_ms = t.elapsed().as_millis() as u64, "register_data_source ok"),
        Err(e) => tracing::warn!(error = %e, elapsed_ms = t.elapsed().as_millis() as u64, "register_data_source failed"),
    }
    Ok(r?)
}

/// 测试连接（临时注册探查后注销，不落库）。返回表结构快照。
#[tauri::command]
#[specta::specta]
pub async fn test_data_source(
    config: DataSourceConfig,
    state: State<'_, AppState>,
) -> AppResult<SchemaSnapshot> {
    let svc = get_svc(&state).await?;
    Ok(svc.test_connection(&config).await?)
}

/// 注销数据源（删 SQLite 记录；SessionContext 内 catalog 随进程生命周期留存，
/// 重启不恢复——DF 54 无 deregister_catalog，见 catalog.rs）。
#[tauri::command]
#[specta::specta]
pub async fn deregister_data_source(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let svc = get_svc(&state).await?;
    svc.deregister(&id).await?;
    Ok(())
}

/// 列出所有已注册数据源（含连接状态/表数）。
#[tauri::command]
#[specta::specta]
pub async fn list_data_sources(state: State<'_, AppState>) -> AppResult<Vec<DataSourceSummary>> {
    let svc = get_svc(&state).await?;
    Ok(svc.list_sources().await?)
}

/// 取单个数据源配置（编辑用）。
#[tauri::command]
#[specta::specta]
pub async fn get_data_source(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<Option<DataSourceConfig>> {
    let svc = get_svc(&state).await?;
    Ok(svc.get_config(&id)?)
}

// ── schema 探查 ────────────────────────────────────────────────

/// 浏览 catalog 下所有表结构（不含样本行，list_tables 工具）。
#[tauri::command]
#[specta::specta]
pub async fn browse_federation_schema(
    catalog: String,
    state: State<'_, AppState>,
) -> AppResult<SchemaSnapshot> {
    let svc = get_svc(&state).await?;
    Ok(svc.browse_schema(&catalog).await?)
}

/// 描述单表：列/类型/可空 + 前 5 行样本 + 行数估计（describe_table 工具）。
#[tauri::command]
#[specta::specta]
pub async fn describe_federation_table(
    catalog: String,
    table: String,
    state: State<'_, AppState>,
) -> AppResult<TableMeta> {
    let svc = get_svc(&state).await?;
    Ok(federation::schema::describe_table(svc.ctx(), &catalog, &table).await?)
}

// ── 查询执行 ────────────────────────────────────────────────────

/// 执行只读 SQL（三段式寻址 catalog.public.table）。自动追加 LIMIT，30s 超时。
#[tauri::command]
#[specta::specta]
pub async fn execute_federation_query(
    sql: String,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> AppResult<QueryResult> {
    let t = std::time::Instant::now();
    tracing::info!(sql = %sql, limit = ?limit, "execute_federation_query invoked");
    let svc = get_svc(&state).await?;
    let r = federation::query::execute_query(svc.ctx(), &sql, limit.map(|n| n as usize)).await;
    match &r {
        Ok(qr) => tracing::info!(rows = qr.rows.len(), cols = qr.columns.len(), elapsed_ms = t.elapsed().as_millis() as u64, "execute_federation_query ok"),
        Err(e) => tracing::warn!(error = %e, sql = %sql, elapsed_ms = t.elapsed().as_millis() as u64, "execute_federation_query failed"),
    }
    Ok(r?)
}

/// EXPLAIN：生成执行计划摘要（调试/审计，本身只读）。
#[tauri::command]
#[specta::specta]
pub async fn explain_federation_query(
    sql: String,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let svc = get_svc(&state).await?;
    Ok(federation::query::explain_query(svc.ctx(), &sql).await?)
}
