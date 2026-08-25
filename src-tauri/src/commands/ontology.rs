//! 本体建模 IPC 命令（三期：本体定义层存储 + import/export）。
//!
//! 薄封装 `crates/ontology-store`（AGENTS.md 工程结构硬约束：业务在 crates/，此处只 IPC）。
//! 五个命令：list / export / preview_import / import / delete。
//!
//! OntologyStore 不是 async 构造（rusqlite 同步），AppState 直接持有 Arc<OntologyStore>，
//! 命令内 clone 一份取出，不持锁跨 await。
//!
//! **BigInt 公约**：`OntologyPayload` / `ImportPreview` / `ImportResult` 含
//! `serde_json::Value` 字段（ActionTypeDef.parameters 等），specta 2.0-rc 的
//! `serde_json::Number` 实现硬编码 i64/u64 触发 BigIntForbidden。故这三个命令
//! 用 `String`（JSON 字符串）传输，前端 `JSON.parse` / 后端 `serde_json::from_str`。
//! `list_ontologies` 的 `OntologySummary` 无 Value 字段，直接结构化导出。

use tauri::State;

use ontology_store::{
    ImportRequest, OntologyChangelog, OntologyCharter, OntologyStore, OntologySummary,
    TtlChangelog, TtlCharter, TtlImportResult, TtlOntologySummary, TtlStore, TtlValidation,
};

use crate::commands::error::{AppError, AppResult};
use crate::state::AppState;

/// 取 OntologyStore 克隆（AppState 始终持有，不会为 None）。
fn get_store(state: &AppState) -> AppResult<std::sync::Arc<OntologyStore>> {
    Ok(state.ontology_store.clone())
}

/// 取 TtlStore 克隆（AppState 始终持有，不会为 None）。
fn get_ttl_store(state: &AppState) -> AppResult<std::sync::Arc<TtlStore>> {
    Ok(state.ttl_store.clone())
}

/// 列出所有已存储本体（前端列表页用）。
#[tauri::command]
#[specta::specta]
pub async fn list_ontologies(state: State<'_, AppState>) -> AppResult<Vec<OntologySummary>> {
    let store = get_store(&state)?;
    Ok(store.list_ontologies()?)
}

/// 列出指定本体下的全部数据集（决策 10 修订：按本体隔离）。
///
/// DatasetDef 含 `partition_config: Option<Value>`，serde_json::Value 触发
/// BigIntForbidden，故用 String（JSON）传输，前端 JSON.parse。
#[tauri::command]
#[specta::specta]
pub async fn list_ontology_datasets(
    state: State<'_, AppState>,
    ontology_api_name: String,
) -> AppResult<String> {
    let store = get_store(&state)?;
    let datasets = store.list_datasets(&ontology_api_name)?;
    serde_json::to_string(&datasets).map_err(|e| AppError::Ontology(format!("serialize: {e}")))
}

/// 列出指定本体下的全部数据源（决策 10 修订：按本体隔离）。
///
/// DataSourceDef 含 `connector_config: Value`，同上用 String（JSON）传输。
#[tauri::command]
#[specta::specta]
pub async fn list_ontology_data_sources(
    state: State<'_, AppState>,
    ontology_api_name: String,
) -> AppResult<String> {
    let store = get_store(&state)?;
    let sources = store.list_data_sources(&ontology_api_name)?;
    serde_json::to_string(&sources).map_err(|e| AppError::Ontology(format!("serialize: {e}")))
}

/// 导出本体为 OntologyPayload JSON 字符串（write-view）。
/// 前端 `JSON.parse` 后展示当前定义、或作为增量更新的起点。
#[tauri::command]
#[specta::specta]
pub async fn export_ontology(
    ontology_api_name: String,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let store = get_store(&state)?;
    let payload = store.export(&ontology_api_name)?;
    serde_json::to_string(&payload).map_err(|e| AppError::Ontology(format!("serialize: {e}")))
}

/// 删除本体及其全部子表（硬删，对齐 Gaia `hard_delete_ontology`）。
///
/// 依赖 DB `ON DELETE CASCADE` 级联清掉 object_types / properties / link_types /
/// action_types / object_type_groups 等。dataset / data_source 不删（物理资产
/// 跨本体共享，设计决策 10）。
///
/// 返回是否真的删除了（false=未找到该本体，幂等不报错）。
#[tauri::command]
#[specta::specta]
pub async fn delete_ontology(
    ontology_api_name: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let store = get_store(&state)?;
    Ok(store.delete(&ontology_api_name)?)
}

/// 预演导入（dry-run）。返回 ImportPreview JSON 字符串。
/// **落库前必调**：前端解析后检查 errors 非空则阻止提交 import。
#[tauri::command]
#[specta::specta]
pub async fn preview_ontology_import(
    payload_json: String,
    overwrite_object_types: Vec<String>,
    overwrite_data_sources: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let t0 = std::time::Instant::now();
    let store = get_store(&state)?;
    let payload: ontology_store::OntologyPayload = serde_json::from_str(&payload_json)
        .map_err(|e| AppError::Ontology(format!("deserialize payload: {e}")))?;
    let t_parse = t0.elapsed();
    let req = ImportRequest {
        payload,
        overwrite_object_types,
        overwrite_data_sources,
    };
    let preview = store.preview_import(&req)?;
    let t_store = t0.elapsed() - t_parse;
    let out = serde_json::to_string(&preview)
        .map_err(|e| AppError::Ontology(format!("serialize: {e}")))?;
    let t_ser = t0.elapsed() - t_parse - t_store;
    tracing::info!(
        payload_bytes = payload_json.len(),
        result_bytes = out.len(),
        parse_us = t_parse.as_micros(),
        store_us = t_store.as_micros(),
        serialize_us = t_ser.as_micros(),
        total_us = t0.elapsed().as_micros(),
        "preview_ontology_import"
    );
    Ok(out)
}

/// 执行导入（DAG 顺序落库，best-effort 部分失败）。
/// 返回 ImportResult JSON 字符串，前端解析后检查 per-entity 状态。
#[tauri::command]
#[specta::specta]
pub async fn import_ontology(
    payload_json: String,
    overwrite_object_types: Vec<String>,
    overwrite_data_sources: Vec<String>,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let t0 = std::time::Instant::now();
    let store = get_store(&state)?;
    let payload: ontology_store::OntologyPayload = serde_json::from_str(&payload_json)
        .map_err(|e| AppError::Ontology(format!("deserialize payload: {e}")))?;
    let t_parse = t0.elapsed();
    let req = ImportRequest {
        payload,
        overwrite_object_types,
        overwrite_data_sources,
    };
    let result = store.import(&req)?;
    let t_store = t0.elapsed() - t_parse;
    let out = serde_json::to_string(&result)
        .map_err(|e| AppError::Ontology(format!("serialize: {e}")))?;
    let t_ser = t0.elapsed() - t_parse - t_store;
    tracing::info!(
        payload_bytes = payload_json.len(),
        result_bytes = out.len(),
        parse_us = t_parse.as_micros(),
        store_us = t_store.as_micros(),
        serialize_us = t_ser.as_micros(),
        total_us = t0.elapsed().as_micros(),
        "import_ontology"
    );
    Ok(out)
}

/// 列出本体的变更历史（git commit log 式，revision 倒序）。
/// 返回结构化 `Vec<OntologyChangelog>`（created_at 已标 Number，无 BigInt 问题）。
#[tauri::command]
#[specta::specta]
pub async fn list_ontology_changelog(
    ontology_api_name: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<OntologyChangelog>> {
    let store = get_store(&state)?;
    Ok(store.list_changelog(&ontology_api_name)?)
}

/// 读取本体设计宪章（不变点：业务场景 / 本质 / 设计意图 / 补充说明）。
///
/// charter 不随历史变化——它记录本体的业务本质说明，不随实体增删改而变更。
/// 由独立命令写入，不进 import 流程。前端详情页头部常驻展示。
#[tauri::command]
#[specta::specta]
pub async fn get_ontology_charter(
    ontology_api_name: String,
    state: State<'_, AppState>,
) -> AppResult<OntologyCharter> {
    let store = get_store(&state)?;
    Ok(store.get_charter(&ontology_api_name)?)
}

/// 写入/更新本体设计宪章（不变点）。
///
/// **只有用户明确要求调整 charter 时才调用**；常规增量更新不应触碰 charter。
/// `updated_by` 为 "agent" | "user"。写后不触发 ontology-changed（charter 不影响实体定义）。
#[tauri::command]
#[specta::specta]
pub async fn set_ontology_charter(
    ontology_api_name: String,
    charter: OntologyCharter,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let store = get_store(&state)?;
    store.set_charter(&ontology_api_name, &charter)?;
    Ok(())
}

// ════════════════════════════════════════════════════════════════
// W3C Turtle 本体 IPC 命令（对齐 skill `ontology-modeling-w3c`）
// 薄封装 `state.ttl_store`（crates/ontology-store/src/ttl.rs 已全就绪）。
// 与 Palantir `OntologyStore` 命令并列，主键改为 ontology_iri（完整 URI）。
// ════════════════════════════════════════════════════════════════

/// 列出所有已存 W3C Turtle 本体（前端列表页用）。
#[tauri::command]
#[specta::specta]
pub async fn list_ontology_ttl(state: State<'_, AppState>) -> AppResult<Vec<TtlOntologySummary>> {
    let store = get_ttl_store(&state)?;
    Ok(store.list_ontologies()?)
}

/// 导出指定本体 IRI 的 Turtle 文本。
#[tauri::command]
#[specta::specta]
pub async fn export_ontology_ttl(
    ontology_iri: String,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let store = get_ttl_store(&state)?;
    Ok(store.export_ttl(&ontology_iri)?)
}

/// 校验 Turtle（dry-run，不写库）。返回 TtlValidation（errors 非空则禁止 import）。
#[tauri::command]
#[specta::specta]
pub async fn validate_ontology_ttl(
    ttl_content: String,
    state: State<'_, AppState>,
) -> AppResult<TtlValidation> {
    let store = get_ttl_store(&state)?;
    Ok(store.validate_ttl(&ttl_content)?)
}

/// 导入 Turtle 到存储（best-effort）。返回 TtlImportResult。
#[tauri::command]
#[specta::specta]
pub async fn import_ontology_ttl(
    ttl_content: String,
    overwrite: bool,
    state: State<'_, AppState>,
) -> AppResult<TtlImportResult> {
    let store = get_ttl_store(&state)?;
    Ok(store.import_ttl(&ttl_content, overwrite)?)
}

/// 删除指定本体 IRI（幂等，级联清 charter/changelog）。
#[tauri::command]
#[specta::specta]
pub async fn delete_ontology_ttl(
    ontology_iri: String,
    state: State<'_, AppState>,
) -> AppResult<bool> {
    let store = get_ttl_store(&state)?;
    Ok(store.delete_ontology(&ontology_iri)?)
}

/// 执行 SPARQL 查询（第 7 类 CRUD）。返回 SPARQL Results JSON 字符串。
#[tauri::command]
#[specta::specta]
pub async fn query_ontology_sparql(
    ontology_iri: String,
    sparql: String,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let store = get_ttl_store(&state)?;
    Ok(store.query_sparql(&ontology_iri, &sparql)?)
}

/// 读取 W3C 本体设计宪章（不变点）。
#[tauri::command]
#[specta::specta]
pub async fn get_ontology_ttl_charter(
    ontology_iri: String,
    state: State<'_, AppState>,
) -> AppResult<TtlCharter> {
    let store = get_ttl_store(&state)?;
    Ok(store.get_charter(&ontology_iri)?)
}

/// 写入/更新 W3C 本体设计宪章（不变点）。只有用户明确要求调整时才调用。
#[tauri::command]
#[specta::specta]
pub async fn set_ontology_ttl_charter(
    ontology_iri: String,
    charter: TtlCharter,
    state: State<'_, AppState>,
) -> AppResult<()> {
    let store = get_ttl_store(&state)?;
    store.set_charter(&ontology_iri, &charter)?;
    Ok(())
}

/// 列出 W3C 本体变更历史（revision 倒序）。
#[tauri::command]
#[specta::specta]
pub async fn list_ontology_ttl_changelog(
    ontology_iri: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<TtlChangelog>> {
    let store = get_ttl_store(&state)?;
    Ok(store.list_changelog(&ontology_iri)?)
}

/// 提交一条 W3C 本体变更日志（每次 import/delete 后调用）。
/// 返回分配的 revision 序号。
#[tauri::command]
#[specta::specta]
pub async fn commit_ontology_ttl_change(
    ontology_iri: String,
    title: String,
    body: String,
    change_summary: String,
    conversation_id: Option<String>,
    state: State<'_, AppState>,
) -> AppResult<u32> {
    let store = get_ttl_store(&state)?;
    Ok(store.commit_change(
        &ontology_iri,
        &title,
        &body,
        &change_summary,
        conversation_id.as_deref(),
        "user",
    )?)
}
