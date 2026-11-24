//! 元素映射绑定层 IPC 命令（决策 25）。
//!
//! 薄封装 `crates/ontology-store` 的映射方法（AGENTS.md：业务在 crates/，此处只 IPC）。
//! 两套本体标准共用同一 command 组，靠 `standard` 参数分派：
//!   - `palantir`：`ontology_key` = 本体 api_name
//!   - `w3c`：`ontology_key` = 本体 IRI
//!
//! **物理结构快照装配**（Task 7.2）：校验与建议都需要「物理世界里现在有哪些表/列」，
//! 快照在此处汇合——联邦服务的内存探查结果（`cached_snapshots`，可信度最高）
//! 加上本体库里的结构证据源（用户贴的 schema 文本）。`ontology-store` 不依赖
//! `federation`，快照由装配层传入，保持 crate 解耦。
//!
//! **BigInt 约束**：计数字段用 `u32`（元素数/行数都远小于 2^32），避免 specta
//! 的 64 位整数导出禁令；`ElementMapping` 的时间戳字段已在 crate 内标注
//! `#[specta(type = Number)]`。

use tauri::State;

use ontology_store::mapping::{
    ElementMapping, ElementMappingItem, MappingCoverage, MappingElementRef,
    MappingExportScope, MappingIntegrityReport, MappingSuggestion, STANDARD_PALANTIR,
    STANDARD_W3C,
};
use ontology_store::mapping_verify::PhysicalSchemaSnapshot;

use crate::commands::error::{AppError, AppResult};
use crate::state::AppState;

/// 写入结果 DTO（`MappingWriteOutcome` 在 crate 内未实现 specta::Type）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct MappingWriteResult {
    /// 写入（含更新）的行数。
    #[specta(type = specta_typescript::Number)]
    pub written: u32,
    /// 整组替换时移除的旧行数。
    #[specta(type = specta_typescript::Number)]
    pub removed: u32,
    /// 聚合提示（面向用户，一句话一条；不阻断保存）。
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// 解析标准参数。未知值直接报错，不静默降级。
fn parse_standard(raw: &str) -> AppResult<&'static str> {
    match raw {
        STANDARD_PALANTIR => Ok(STANDARD_PALANTIR),
        STANDARD_W3C => Ok(STANDARD_W3C),
        other => Err(AppError::Ontology(format!(
            "未知的本体标准 '{other}'（应为 {STANDARD_PALANTIR} 或 {STANDARD_W3C}）"
        ))),
    }
}

fn parse_scope(raw: &str) -> MappingExportScope {
    MappingExportScope::parse(raw)
}

/// 装配当前物理结构快照：联邦内存快照 + 本体库结构证据源。
///
/// 两者都缺失时返回空快照——校验路径据此降级为「未核对」，不报错
/// （用户还没注册任何数据源是正常状态）。
async fn current_snapshot(state: &AppState) -> PhysicalSchemaSnapshot {
    // clone 出句柄，不持读锁跨后续调用
    let fed = state.federation.read().await.clone();
    let store = state.ontology_store.clone();
    let mut snap = agent_core::mapping_tools::live_snapshot(fed.as_ref(), Some(store.as_ref()));
    // W3C 本体的结构证据源存在同一张表（ontology_api_name 为空表示跨本体共享证据），
    // 上面按 None 取全量，已包含，无需再合并。
    if snap.sources.is_empty() {
        snap = PhysicalSchemaSnapshot::default();
    }
    snap
}

// ── 读取 ───────────────────────────────────────────────────────

/// 列出某本体的全部映射。
#[tauri::command]
#[specta::specta]
pub async fn list_element_mappings(
    standard: String,
    ontology_key: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<ElementMapping>> {
    let std = parse_standard(&standard)?;
    let rows = match std {
        STANDARD_PALANTIR => state.ontology_store.list_element_mappings(&ontology_key)?,
        _ => state.ttl_store.list_element_mappings(&ontology_key)?,
    };
    Ok(rows)
}

/// 某元素的映射列表（属性卡 / 关系卡渲染用）。
#[tauri::command]
#[specta::specta]
pub async fn list_element_mappings_for(
    standard: String,
    ontology_key: String,
    element_type: String,
    element_id: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<ElementMapping>> {
    let std = parse_standard(&standard)?;
    let rows = match std {
        STANDARD_PALANTIR => state
            .ontology_store
            .list_element_mappings_for(&ontology_key, &element_type, &element_id)?,
        _ => state
            .ttl_store
            .list_element_mappings_for(&ontology_key, &element_type, &element_id)?,
    };
    Ok(rows)
}

/// 本体元素全集（缺口清单 / 候选建议的输入；不含映射状态）。
#[tauri::command]
#[specta::specta]
pub async fn list_mapping_elements(
    standard: String,
    ontology_key: String,
    state: State<'_, AppState>,
) -> AppResult<Vec<MappingElementRef>> {
    let std = parse_standard(&standard)?;
    let rows = match std {
        STANDARD_PALANTIR => state.ontology_store.list_mapping_elements(&ontology_key)?,
        _ => state.ttl_store.list_mapping_elements(&ontology_key)?,
    };
    Ok(rows)
}

/// 覆盖率（类 / 属性 / 关系分别统计，互不推导）。
#[tauri::command]
#[specta::specta]
pub async fn mapping_coverage(
    standard: String,
    ontology_key: String,
    state: State<'_, AppState>,
) -> AppResult<MappingCoverage> {
    let std = parse_standard(&standard)?;
    let coverage = match std {
        STANDARD_PALANTIR => state.ontology_store.mapping_coverage(&ontology_key)?,
        _ => state.ttl_store.mapping_coverage(&ontology_key)?,
    };
    Ok(coverage)
}

/// 完整性报告（孤儿 / 失效 / 未核对；均不自动删除、不阻断操作）。
#[tauri::command]
#[specta::specta]
pub async fn mapping_integrity(
    standard: String,
    ontology_key: String,
    state: State<'_, AppState>,
) -> AppResult<MappingIntegrityReport> {
    let std = parse_standard(&standard)?;
    let snap = current_snapshot(&state).await;
    let snap_opt = if snap.is_empty() { None } else { Some(&snap) };
    let report = match std {
        STANDARD_PALANTIR => state
            .ontology_store
            .mapping_integrity(&ontology_key, snap_opt)?,
        _ => state.ttl_store.mapping_integrity(&ontology_key, snap_opt)?,
    };
    Ok(report)
}

// ── 变更 ───────────────────────────────────────────────────────

/// 保存映射（按元素整组替换语义；空集合 = 清空该元素全部映射）。
#[tauri::command]
#[specta::specta]
pub async fn save_element_mappings(
    standard: String,
    ontology_key: String,
    items: Vec<ElementMappingItem>,
    state: State<'_, AppState>,
) -> AppResult<MappingWriteResult> {
    let std = parse_standard(&standard)?;
    let snap = current_snapshot(&state).await;
    let snap_opt = if snap.is_empty() { None } else { Some(&snap) };
    let outcome = match std {
        STANDARD_PALANTIR => state
            .ontology_store
            .save_element_mappings(&ontology_key, items, snap_opt)?,
        _ => state
            .ttl_store
            .save_element_mappings(&ontology_key, items, snap_opt)?,
    };
    Ok(MappingWriteResult {
        written: outcome.written as u32,
        removed: outcome.removed as u32,
        warnings: outcome.warnings,
    })
}

/// 删除某元素的全部映射（等价于保存空集合）。
#[tauri::command]
#[specta::specta]
pub async fn delete_element_mappings(
    standard: String,
    ontology_key: String,
    element_type: String,
    element_id: String,
    state: State<'_, AppState>,
) -> AppResult<u32> {
    let std = parse_standard(&standard)?;
    let n = match std {
        STANDARD_PALANTIR => state.ontology_store.delete_element_mappings_for_element(
            &ontology_key,
            &element_type,
            &element_id,
        )?,
        _ => state.ttl_store.delete_element_mappings_for_element(
            &ontology_key,
            &element_type,
            &element_id,
        )?,
    };
    Ok(n as u32)
}

/// 清理孤儿映射（**仅在用户显式确认后调用**；不做后台自动删除）。
#[tauri::command]
#[specta::specta]
pub async fn purge_orphan_mappings(
    standard: String,
    ontology_key: String,
    state: State<'_, AppState>,
) -> AppResult<u32> {
    let std = parse_standard(&standard)?;
    let snap = current_snapshot(&state).await;
    let snap_opt = if snap.is_empty() { None } else { Some(&snap) };
    let n = match std {
        STANDARD_PALANTIR => state
            .ontology_store
            .purge_orphan_mappings(&ontology_key, snap_opt)?,
        _ => state
            .ttl_store
            .purge_orphan_mappings(&ontology_key, snap_opt)?,
    };
    Ok(n as u32)
}

// ── 建议与导出 ─────────────────────────────────────────────────

/// 候选建议（只读，不落库）。未提供 elements 时用本体元素全集。
#[tauri::command]
#[specta::specta]
pub async fn suggest_element_mappings(
    standard: String,
    ontology_key: String,
    elements: Option<Vec<MappingElementRef>>,
    state: State<'_, AppState>,
) -> AppResult<Vec<MappingSuggestion>> {
    let std = parse_standard(&standard)?;
    let snap = current_snapshot(&state).await;
    let refs = match elements {
        Some(list) if !list.is_empty() => list,
        _ => match std {
            STANDARD_PALANTIR => state.ontology_store.list_mapping_elements(&ontology_key)?,
            _ => state.ttl_store.list_mapping_elements(&ontology_key)?,
        },
    };
    let out = match std {
        STANDARD_PALANTIR => {
            state
                .ontology_store
                .suggest_element_mappings(&ontology_key, &refs, &snap)?
        }
        _ => state
            .ttl_store
            .suggest_element_mappings(&ontology_key, &refs, &snap)?,
    };
    Ok(out)
}

/// 导出映射为多工作表工作簿并落盘到用户选定路径，返回实际写入路径。
///
/// 之所以由后端落盘而非把 `Vec<u8>` 传回前端：工作簿是二进制，走 IPC 需要
/// 整体序列化成 number 数组（大本体可达数 MB，specta/serde 开销与内存翻倍），
/// 且前端拿到后仍需经插件写文件。路径由前端 `save()` 对话框选定（用户显式授权），
/// 后端只写入该路径，不提供任意路径泛化。
#[tauri::command]
#[specta::specta]
pub async fn export_element_mapping(
    standard: String,
    ontology_key: String,
    scope: String,
    target_path: String,
    state: State<'_, AppState>,
) -> AppResult<String> {
    let std = parse_standard(&standard)?;
    let path = target_path.trim();
    if path.is_empty() {
        return Err(AppError::Ontology("请选择保存位置".to_string()));
    }
    let snap = current_snapshot(&state).await;
    let snap_opt = if snap.is_empty() { None } else { Some(&snap) };
    let exported_at = ontology_store::mapping_export::format_export_time(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    );
    let bytes = match std {
        STANDARD_PALANTIR => state.ontology_store.mapping_workbook(
            &ontology_key,
            parse_scope(&scope),
            snap_opt,
            &exported_at,
        )?,
        _ => state.ttl_store.mapping_workbook(
            &ontology_key,
            parse_scope(&scope),
            snap_opt,
            &exported_at,
        )?,
    };
    std::fs::write(path, &bytes)
        .map_err(|e| AppError::Ontology(format!("写入文件失败：{e}")))?;
    Ok(path.to_string())
}

/// 回填：把历史的属性级来源字段迁入映射表（幂等，仅在本体映射为空时执行）。
///
/// 供「升级后首次打开旧本体」与导入路径调用；已存在映射时不覆盖用户数据。
#[tauri::command]
#[specta::specta]
pub async fn backfill_element_mappings(
    ontology_key: String,
    state: State<'_, AppState>,
) -> AppResult<u32> {
    let n = state
        .ontology_store
        .backfill_mappings_from_properties(&ontology_key)?;
    Ok(n as u32)
}
