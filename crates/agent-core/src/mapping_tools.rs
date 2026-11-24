//! 元素映射绑定层的 Agent 工具组（决策 25）。
//!
//! 四个 DynamicTool：`suggest_element_mappings` / `save_element_mappings` /
//! `list_element_mappings` / `export_element_mapping`。
//!
//! 设计要点：
//! - **标准推断**：`standard` 省略时按已注入的 store 推断（只有一侧可用就用它，
//!   两侧都有默认 palantir）。这样同一个工具组对两套标准都可用。
//! - **快照动态取**：物理结构快照由 `MappingSnapshotProvider` 每次调用时取
//!   （装配层从 `FederationService` 内存缓存 + `evidence_sources` 汇合），
//!   避免为「数据源刚注册」而重建 `ChatService`。
//! - **不新增 store 句柄**：映射是 `OntologyStore` / `TtlStore` 上的方法，
//!   工具组只拿既有的 `Arc`。
//! - **建议不落库**：`suggest` 只读；落库必须显式 `save`（建议→确认语义边界）。

use std::sync::Arc;

use ontology_store::mapping::{
    ElementMappingItem, MappingElementRef, MappingEndpoint, MappingExportScope, STANDARD_PALANTIR,
    STANDARD_W3C,
};
use ontology_store::mapping_verify::{PhysicalSchemaSnapshot, PhysicalSource, PhysicalTable};
use ontology_store::{OntologyStore, TtlStore};
use rig::tool::{DynamicTool, ToolOutput};
use serde_json::json;

/// 物理结构快照提供者：每次调用时取一次，反映最新的数据源注册状态。
pub type MappingSnapshotProvider = Arc<dyn Fn() -> PhysicalSchemaSnapshot + Send + Sync>;

/// 固定快照（测试与非动态装配场景）。
pub fn fixed_snapshot(snapshot: PhysicalSchemaSnapshot) -> MappingSnapshotProvider {
    Arc::new(move || snapshot.clone())
}

fn snapshot_now(provider: &MappingSnapshotProvider) -> PhysicalSchemaSnapshot {
    provider()
}

/// 本体标准（两套标准共用同一张映射表，靠此字段区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingStandard {
    Palantir,
    W3c,
}

impl MappingStandard {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            STANDARD_PALANTIR => Some(MappingStandard::Palantir),
            STANDARD_W3C => Some(MappingStandard::W3c),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MappingStandard::Palantir => STANDARD_PALANTIR,
            MappingStandard::W3c => STANDARD_W3C,
        }
    }
}

/// 工具组持有的两套本体存储句柄（都可空；至少一个可用才挂工具）。
#[derive(Clone, Default)]
pub struct MappingStores {
    pub palantir: Option<Arc<OntologyStore>>,
    pub w3c: Option<Arc<TtlStore>>,
}

impl MappingStores {
    pub fn new(palantir: Option<Arc<OntologyStore>>, w3c: Option<Arc<TtlStore>>) -> Self {
        Self { palantir, w3c }
    }

    pub fn is_empty(&self) -> bool {
        self.palantir.is_none() && self.w3c.is_none()
    }

    /// 解析标准：显式传入优先，缺省时按可用 store 推断。
    ///
    /// 两侧都可用时默认 palantir（会话页面默认打开 Palantir 本体）；
    /// 模型需要 W3C 时显式传 `standard`。
    fn resolve(&self, raw: &str) -> Result<MappingStandard, String> {
        if !raw.is_empty() {
            return MappingStandard::parse(raw)
                .ok_or_else(|| format!("未知的本体标准 '{raw}'，请用 palantir 或 w3c"));
        }
        match (&self.palantir, &self.w3c) {
            (Some(_), None) => Ok(MappingStandard::Palantir),
            (None, Some(_)) => Ok(MappingStandard::W3c),
            (Some(_), Some(_)) => Ok(MappingStandard::Palantir),
            (None, None) => Err("未注入任何本体存储，映射工具不可用".to_string()),
        }
    }

    fn palantir(&self) -> Result<&Arc<OntologyStore>, String> {
        self.palantir
            .as_ref()
            .ok_or_else(|| "未注入 Palantir 本体存储".to_string())
    }

    fn w3c(&self) -> Result<&Arc<TtlStore>, String> {
        self.w3c
            .as_ref()
            .ok_or_else(|| "未注入 W3C 本体存储".to_string())
    }

    fn store_for(&self, standard: MappingStandard) -> Result<MappingStore, String> {
        match standard {
            MappingStandard::Palantir => self.palantir().map(|s| MappingStore::Palantir(s.clone())),
            MappingStandard::W3c => self.w3c().map(|s| MappingStore::W3c(s.clone())),
        }
    }
}

/// 统一 store 调用面（两套 store 的方法签名一致，只是持有类型不同）。
#[derive(Clone)]
enum MappingStore {
    Palantir(Arc<OntologyStore>),
    W3c(Arc<TtlStore>),
}

impl MappingStore {
    fn list(&self, key: &str) -> Result<Vec<ontology_store::ElementMapping>, String> {
        match self {
            MappingStore::Palantir(s) => s.list_element_mappings(key).map_err(|e| e.to_string()),
            MappingStore::W3c(s) => s.list_element_mappings(key).map_err(|e| e.to_string()),
        }
    }

    fn list_for(
        &self,
        key: &str,
        etype: &str,
        eid: &str,
    ) -> Result<Vec<ontology_store::ElementMapping>, String> {
        match self {
            MappingStore::Palantir(s) => s
                .list_element_mappings_for(key, etype, eid)
                .map_err(|e| e.to_string()),
            MappingStore::W3c(s) => s
                .list_element_mappings_for(key, etype, eid)
                .map_err(|e| e.to_string()),
        }
    }

    fn save(
        &self,
        key: &str,
        items: Vec<ElementMappingItem>,
        snapshot: Option<&PhysicalSchemaSnapshot>,
    ) -> Result<ontology_store::mapping::MappingWriteOutcome, String> {
        match self {
            MappingStore::Palantir(s) => s
                .save_element_mappings(key, items, snapshot)
                .map_err(|e| e.to_string()),
            MappingStore::W3c(s) => s
                .save_element_mappings(key, items, snapshot)
                .map_err(|e| e.to_string()),
        }
    }

    /// 装配导出数据集（Agent 侧只消费文本预览，不落盘）。
    fn dataset(
        &self,
        key: &str,
        scope: MappingExportScope,
        snapshot: Option<&PhysicalSchemaSnapshot>,
        exported_at: &str,
    ) -> Result<ontology_store::mapping_export::ExportDataset, String> {
        match self {
            MappingStore::Palantir(s) => s
                .mapping_dataset(key, scope, snapshot, exported_at)
                .map_err(|e| e.to_string()),
            MappingStore::W3c(s) => s
                .mapping_dataset(key, scope, snapshot, exported_at)
                .map_err(|e| e.to_string()),
        }
    }
}

// ════════════════════════════════════════════════════════════════
// 快照装配
// ════════════════════════════════════════════════════════════════

/// 汇合物理结构快照：联邦已注册数据源（可信度最高，覆盖同表名）+ 证据源文本快照。
///
/// 同表名时**联邦优先**：联邦是真实探查结果（有类型与可空），证据源是用户贴的文本，
/// 可能过期。证据源只补充联邦没覆盖到的表。
pub fn build_schema_snapshot(
    federation_sources: impl IntoIterator<Item = (String, Vec<PhysicalTable>)>,
    evidence_sources: impl IntoIterator<Item = ontology_store::EvidenceSource>,
) -> PhysicalSchemaSnapshot {
    let mut sources: Vec<PhysicalSource> = Vec::new();
    for (name, tables) in federation_sources {
        if tables.is_empty() {
            continue;
        }
        sources.push(PhysicalSource {
            name,
            kind: ontology_store::mapping::SOURCE_KIND_FEDERATION.to_string(),
            tables,
        });
    }
    let evidence = ontology_store::mapping_verify::parse_evidence_physical(evidence_sources);
    let covered: std::collections::HashSet<String> = sources
        .iter()
        .flat_map(|s| s.tables.iter().map(|t| t.name.to_ascii_lowercase()))
        .collect();
    for src in evidence.sources {
        let tables: Vec<PhysicalTable> = src
            .tables
            .into_iter()
            .filter(|t| !covered.contains(&t.name.to_ascii_lowercase()))
            .collect();
        if tables.is_empty() {
            continue;
        }
        sources.push(PhysicalSource {
            name: src.name,
            kind: src.kind,
            tables,
        });
    }
    PhysicalSchemaSnapshot { sources }
}

/// 构造映射工具组（4 个）。两 store 都未注入时返回空集。
pub fn mapping_tools(
    stores: MappingStores,
    snapshot: MappingSnapshotProvider,
) -> Vec<DynamicTool> {
    if stores.is_empty() {
        return Vec::new();
    }
    vec![
        suggest_tool(stores.clone(), snapshot.clone()),
        save_tool(stores.clone(), snapshot.clone()),
        list_tool(stores.clone()),
        export_tool(stores, snapshot),
    ]
}

// ════════════════════════════════════════════════════════════════
// JSON schema
// ════════════════════════════════════════════════════════════════

fn standard_and_key_props() -> serde_json::Value {
    json!({
        "standard": {
            "type": "string",
            "enum": ["palantir", "w3c"],
            "description": "本体标准：palantir 或 w3c。省略时按会话已加载的本体标准自动判断",
            "default": "",
        },
        "ontology_key": {
            "type": "string",
            "description": "本体标识：palantir 用本体 api_name；w3c 用本体 IRI",
        },
    })
}

fn with_required(mut props: serde_json::Value, required: &[&str]) -> serde_json::Value {
    if let Some(obj) = props.as_object_mut() {
        obj.insert("required".to_string(), json!(required));
        obj.insert("additionalProperties".to_string(), json!(false));
    }
    props
}

fn schema_of(props: serde_json::Value) -> serde_json::Value {
    json!({ "type": "object", "properties": props })
}

fn endpoint_props(table_required: bool) -> serde_json::Value {
    let mut props = json!({
        "catalog": { "type": "string", "default": "" },
        "schema": { "type": "string", "default": "" },
        "table": { "type": "string" },
        "column": { "type": "string", "default": "" },
    });
    if let Some(obj) = props.as_object_mut() {
        obj.insert("additionalProperties".to_string(), json!(false));
        if table_required {
            obj.insert("required".to_string(), json!(["table"]));
        }
    }
    props
}

fn arg_str(args: &serde_json::Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

// ════════════════════════════════════════════════════════════════
// 工具
// ════════════════════════════════════════════════════════════════

fn suggest_tool(stores: MappingStores, snapshot: MappingSnapshotProvider) -> DynamicTool {
    let mut props = standard_and_key_props();
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            "elements".to_string(),
            json!({
                "type": "array",
                "description": "需要候选映射的本体元素列表",
                "items": {
                    "type": "object",
                    "properties": {
                        "element_type": { "type": "string" },
                        "element_id": { "type": "string" },
                        "display_name": { "type": "string", "default": "" },
                        "data_type": { "type": "string", "default": "" },
                        "same_table_hint": {
                            "type": "string",
                            "description": "该元素所属类已绑定的物理表定位串；已知时传入可优先推荐同表列",
                            "default": "",
                        },
                    },
                    "required": ["element_type", "element_id"],
                    "additionalProperties": false,
                },
            }),
        );
        obj.insert(
            "top_n".to_string(),
            json!({
                "type": "integer",
                "description": "每个元素返回的候选数量上限，默认 3",
                "default": 3,
            }),
        );
    }
    crate::dynamic_tool(
        "suggest_element_mappings",
        "为指定本体元素推荐物理表字段候选（只读，不保存）。\n\
         返回每个元素的候选列表，含物理位置、置信度与命中依据。\n\
         建议由名称归一、类型兼容、同表优先三条确定性规则产生，不会凭空编造列名。\n\
         候选需要用户或你确认后，再调用 save_element_mappings 落库。\n\
         若返回为空，说明已登记的数据源里没有可用物理结构——请先确认数据源已注册，\n\
         或已用 register_evidence_source 登记 schema，再重试。\n\
         结果仅供决策，不写入本体，不改动任何映射。",
        schema_of(with_required(props, &["ontology_key", "elements"])),
        move |args| {
            let stores = stores.clone();
            let snapshot = snapshot.clone();
            Box::pin(async move {
                let standard = match stores.resolve(&arg_str(&args, "standard")) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法确定标准：{e}"))),
                };
                let key = arg_str(&args, "ontology_key");
                if key.is_empty() {
                    return Ok(ToolOutput::text("缺少 ontology_key 参数"));
                }
                let elements: Vec<MappingElementRef> = args
                    .get("elements")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                if elements.is_empty() {
                    return Ok(ToolOutput::text("缺少 elements 参数"));
                }
                let top_n = args.get("top_n").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                let store = match stores.store_for(standard) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法访问本体存储：{e}"))),
                };
                let known = store.list(&key).unwrap_or_default();
                let snap = snapshot_now(&snapshot);
                let out = ontology_store::mapping_suggest::suggest_with_top_n(
                    &elements, &snap, &known, top_n,
                );
                if out.is_empty() {
                    return Ok(ToolOutput::text(
                        "没有找到可用的物理结构候选。请确认数据源已注册并探测到 schema，\n\
                         或已登记结构证据源（kind 为 db_table 或 db_column）。\n\
                         若是属性名与列名风格差异过大，可直接指定物理位置后调用 save_element_mappings。",
                    ));
                }
                Ok(ToolOutput::json(json!(out)))
            })
        },
    )
}

fn save_tool(stores: MappingStores, snapshot: MappingSnapshotProvider) -> DynamicTool {
    let mut props = standard_and_key_props();
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            "items".to_string(),
            json!({
                "type": "array",
                "description": "按元素整组替换的映射列表",
                "items": {
                    "type": "object",
                    "properties": {
                        "element_type": { "type": "string" },
                        "element_id": { "type": "string" },
                        "mappings": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "source": {
                                        "type": "object",
                                        "properties": endpoint_props(true),
                                    },
                                    "target": {
                                        "type": "object",
                                        "properties": endpoint_props(false),
                                    },
                                    "source_kind": {
                                        "type": "string",
                                        "enum": ["federation", "evidence", "manual"],
                                        "default": "manual",
                                    },
                                    "source_ref": { "type": "string", "default": "" },
                                    "note": { "type": "string", "default": "" },
                                },
                                "required": ["source"],
                                "additionalProperties": false,
                            },
                        },
                    },
                    "required": ["element_type", "element_id", "mappings"],
                    "additionalProperties": false,
                },
            }),
        );
    }
    crate::dynamic_tool(
        "save_element_mappings",
        "保存本体元素的物理表字段映射（按元素整组替换）。\n\
         传入某元素的全部映射即覆盖该元素原有映射；传入空集合等于清空该元素的全部映射。\n\
         映射与本体定义分离保存，不改变类、属性、关系的定义本身。\n\
         目标物理位置不存在时不会拒绝保存，只会标记为未核对并给出提示。\n\
         属性与类只需填 source；关系可填 source 与 target 表达连接两端。\n\
         保存成功后返回写入与移除的行数与提示。",
        schema_of(with_required(props, &["ontology_key", "items"])),
        move |args| {
            let stores = stores.clone();
            let snapshot = snapshot.clone();
            Box::pin(async move {
                let standard = match stores.resolve(&arg_str(&args, "standard")) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法确定标准：{e}"))),
                };
                let key = arg_str(&args, "ontology_key");
                if key.is_empty() {
                    return Ok(ToolOutput::text("缺少 ontology_key 参数"));
                }
                let items = parse_mapping_items(&args);
                if items.is_empty() {
                    return Ok(ToolOutput::text("缺少 items 参数"));
                }
                let store = match stores.store_for(standard) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法访问本体存储：{e}"))),
                };
                let snap = snapshot_now(&snapshot);
                let snap_opt = if snap.is_empty() { None } else { Some(&snap) };
                match store.save(&key, items, snap_opt) {
                    Ok(outcome) => {
                        let mut msg = format!(
                            "映射已保存：写入 {} 条，移除 {} 条。",
                            outcome.written, outcome.removed
                        );
                        if !outcome.warnings.is_empty() {
                            msg.push_str(&format!(
                                "\n\n提示（{} 条）：\n- {}",
                                outcome.warnings.len(),
                                outcome.warnings.join("\n- ")
                            ));
                        }
                        Ok(ToolOutput::text(msg))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("保存映射失败：{e}"))),
                }
            })
        },
    )
}

fn list_tool(stores: MappingStores) -> DynamicTool {
    let mut props = standard_and_key_props();
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            "element_type".to_string(),
            json!({
                "type": "string",
                "description": "只看某一类元素（如 property 或 class）；省略则返回全部",
                "default": "",
            }),
        );
        obj.insert(
            "element_id".to_string(),
            json!({
                "type": "string",
                "description": "只看某个元素（需与 element_type 同时给出）",
                "default": "",
            }),
        );
    }
    crate::dynamic_tool(
        "list_element_mappings",
        "读取本体已保存的物理表字段映射（只读）。\n\
         不传 element_type 返回该本体全部映射；传 element_type 与 element_id 只看单个元素。\n\
         用于确认某个属性当前绑定了哪些物理列，或核对保存结果。",
        schema_of(with_required(props, &["ontology_key"])),
        move |args| {
            let stores = stores.clone();
            Box::pin(async move {
                let standard = match stores.resolve(&arg_str(&args, "standard")) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法确定标准：{e}"))),
                };
                let key = arg_str(&args, "ontology_key");
                if key.is_empty() {
                    return Ok(ToolOutput::text("缺少 ontology_key 参数"));
                }
                let etype = arg_str(&args, "element_type");
                let eid = arg_str(&args, "element_id");
                let store = match stores.store_for(standard) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法访问本体存储：{e}"))),
                };
                let result = if !etype.is_empty() && !eid.is_empty() {
                    store.list_for(&key, &etype, &eid)
                } else {
                    store.list(&key)
                };
                match result {
                    Ok(rows) => {
                        let filtered: Vec<_> = rows
                            .into_iter()
                            .filter(|m| {
                                (etype.is_empty() || m.element_type == etype)
                                    && (eid.is_empty() || m.element_id == eid)
                            })
                            .collect();
                        if filtered.is_empty() {
                            return Ok(ToolOutput::text("该本体暂无符合条件的映射。"));
                        }
                        Ok(ToolOutput::json(json!(filtered)))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("读取映射失败：{e}"))),
                }
            })
        },
    )
}

fn export_tool(stores: MappingStores, snapshot: MappingSnapshotProvider) -> DynamicTool {
    let mut props = standard_and_key_props();
    if let Some(obj) = props.as_object_mut() {
        obj.insert(
            "scope".to_string(),
            json!({
                "type": "string",
                "enum": ["all", "gap"],
                "description": "导出范围：all 全部元素；gap 仅未映射与未核对项。默认 all",
                "default": "all",
            }),
        );
    }
    crate::dynamic_tool(
        "export_element_mapping",
        "生成并预览本体元素映射表。\n\
         scope 为 all 时输出全部元素（含未映射的）；为 gap 时只输出未映射项，便于逐项补齐。\n\
         返回内容预览与总行数，用户可在界面导出完整工作簿文件。",
        schema_of(with_required(props, &["ontology_key"])),
        move |args| {
            let stores = stores.clone();
            let snapshot = snapshot.clone();
            Box::pin(async move {
                let standard = match stores.resolve(&arg_str(&args, "standard")) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法确定标准：{e}"))),
                };
                let key = arg_str(&args, "ontology_key");
                if key.is_empty() {
                    return Ok(ToolOutput::text("缺少 ontology_key 参数"));
                }
                let scope = MappingExportScope::parse(&arg_str(&args, "scope"));
                let store = match stores.store_for(standard) {
                    Ok(s) => s,
                    Err(e) => return Ok(ToolOutput::text(format!("无法访问本体存储：{e}"))),
                };
                let snap = snapshot_now(&snapshot);
                let snap_opt = if snap.is_empty() { None } else { Some(&snap) };
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let exported_at = ontology_store::mapping_export::format_export_time(now);
                match store.dataset(&key, scope, snap_opt, &exported_at) {
                    Ok(ds) => {
                        let data_rows = ds.data_row_count();
                        if data_rows == 0 {
                            return Ok(ToolOutput::text(
                                "没有可导出的映射内容。\n\
                                 请先用 save_element_mappings 保存映射，或确认数据源已登记。",
                            ));
                        }
                        let text = ontology_store::mapping_export::render_preview_text(&ds);
                        let lines: Vec<&str> = text.lines().collect();
                        let preview: Vec<&str> = lines.iter().take(21).copied().collect();
                        let scope_label = if scope == MappingExportScope::Gap {
                            "缺口"
                        } else {
                            "全部"
                        };
                        let mut msg = format!(
                            "映射表已生成（{scope_label}，共 {data_rows} 行，含 1 行表头）\n\
                             用户可在界面导出完整文件。以下为前 {} 行预览：\n\n",
                            preview.len().saturating_sub(1),
                        );
                        msg.push_str(&preview.join("\n"));
                        if data_rows + 1 > preview.len() {
                            msg.push_str("\n……（后略）");
                        }
                        Ok(ToolOutput::text(msg))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("导出映射失败：{e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 辅助
// ════════════════════════════════════════════════════════════════

/// 从工具参数里解析整组替换列表（容错：字段缺失按空处理）。
pub fn parse_mapping_items(value: &serde_json::Value) -> Vec<ElementMappingItem> {
    value
        .get("items")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// 解析单个物理位置（容错：缺失返回空端点）。
pub fn parse_endpoint(value: Option<&serde_json::Value>) -> MappingEndpoint {
    value
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// 标准的展示标签（与 CSV 导出口径一致）。
pub fn standard_label(standard: MappingStandard) -> &'static str {
    standard.as_str()
}

/// 人工指定位置的入参便捷构造。
pub fn manual_input(table: &str, column: &str) -> ontology_store::mapping::ElementMappingInput {
    ontology_store::mapping::ElementMappingInput::manual(table, column)
}

/// 生产路径的快照装配：联邦内存快照（真实探查）+ 本体库里的结构证据源。
///
/// 两者都可缺失：没连数据源、也没登记 schema 时返回空快照，
/// 校验路径据此降级为「未核对」而不是报错。
pub fn live_snapshot(
    federation: Option<&federation::FederationService>,
    evidence_store: Option<&OntologyStore>,
) -> PhysicalSchemaSnapshot {
    let fed: Vec<(String, Vec<PhysicalTable>)> = federation
        .map(|f| {
            f.cached_snapshots()
                .into_iter()
                .map(|(name, snap)| {
                    let tables = snap
                        .tables
                        .into_iter()
                        .map(|t| PhysicalTable {
                            // 数据源名即 catalog 名（三段式寻址的第一段）
                            catalog: name.clone(),
                            schema: String::new(),
                            name: t.name,
                            columns: t
                                .columns
                                .into_iter()
                                .map(|c| ontology_store::PhysicalColumn {
                                    name: c.name,
                                    data_type: c.data_type,
                                    nullable: c.nullable,
                                })
                                .collect(),
                        })
                        .collect();
                    (name, tables)
                })
                .collect()
        })
        .unwrap_or_default();
    let evidence = evidence_store
        .and_then(|s| s.list_evidence_sources(None).ok())
        .unwrap_or_default();
    build_schema_snapshot(fed, evidence)
}
#[cfg(test)]
mod tests {
    use super::*;
    use ontology_store::PhysicalColumn;

    fn provider(snap: PhysicalSchemaSnapshot) -> MappingSnapshotProvider {
        fixed_snapshot(snap)
    }

    #[test]
    fn standard_labels() {
        assert_eq!(standard_label(MappingStandard::Palantir), "palantir");
        assert_eq!(standard_label(MappingStandard::W3c), STANDARD_W3C);
        assert_eq!(MappingStandard::parse("w3c"), Some(MappingStandard::W3c));
        assert_eq!(MappingStandard::parse("nope"), None);
    }

    #[test]
    fn default_standard_inferred_from_available_store() {
        let only_w3c =
            MappingStores::new(None, Some(Arc::new(TtlStore::open_in_memory().unwrap())));
        assert_eq!(only_w3c.resolve("").unwrap(), MappingStandard::W3c);

        let only_palantir =
            MappingStores::new(Some(Arc::new(OntologyStore::open_in_memory().unwrap())), None);
        assert_eq!(only_palantir.resolve("").unwrap(), MappingStandard::Palantir);

        let none = MappingStores::default();
        assert!(none.resolve("").is_err());
        assert!(none.store_for(MappingStandard::Palantir).is_err());
    }

    #[test]
    fn no_stores_yields_no_tools() {
        let tools =
            mapping_tools(MappingStores::default(), provider(PhysicalSchemaSnapshot::default()));
        assert!(tools.is_empty());
    }

    #[test]
    fn four_tools_are_registered() {
        let stores = MappingStores::new(
            Some(Arc::new(OntologyStore::open_in_memory().unwrap())),
            Some(Arc::new(TtlStore::open_in_memory().unwrap())),
        );
        assert_eq!(
            mapping_tools(stores, provider(PhysicalSchemaSnapshot::default())).len(),
            4
        );
    }

    #[test]
    fn parse_items_from_tool_args() {
        let args = json!({
            "items": [{
                "element_type": "property",
                "element_id": "orderNo",
                "mappings": [{
                    "source": { "table": "t_order", "column": "order_no" },
                    "note": "note"
                }]
            }]
        });
        let items = parse_mapping_items(&args);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].element_id, "orderNo");
        assert_eq!(items[0].mappings[0].source.table, "t_order");
        assert_eq!(items[0].mappings[0].source.column, "order_no");
        assert_eq!(items[0].mappings[0].note, "note");
        assert_eq!(items[0].mappings[0].normalized_source_kind(), "manual");
    }

    #[test]
    fn empty_items_defaults() {
        assert!(parse_mapping_items(&json!({})).is_empty());
        assert!(parse_endpoint(None).is_empty());
    }

    #[test]
    fn manual_input_helper() {
        let i = manual_input("t", "c");
        assert_eq!(i.source.table, "t");
        assert_eq!(i.source.column, "c");
        assert_eq!(i.normalized_source_kind(), "manual");
    }

    #[test]
    fn endpoint_roundtrip() {
        let ep = parse_endpoint(Some(&json!({ "table": "t", "column": "c" })));
        assert_eq!(ep.table, "t");
        assert_eq!(ep.column, "c");
    }

    #[test]
    fn snapshot_prefers_federation_over_evidence() {
        let fed = vec![(
            "sales".to_string(),
            vec![PhysicalTable {
                name: "t_order".into(),
                columns: vec![PhysicalColumn {
                    name: "fed_col".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        )];
        let evidence = vec![ontology_store::EvidenceSource {
            source_id: "s".into(),
            ontology_api_name: String::new(),
            kind: "db_column".into(),
            title: "t_order.evidence_col".into(),
            content: "t_order.evidence_col\nt_other.evidence_only".into(),
            content_hash: String::new(),
            origin_ref: String::new(),
            created_at: 0,
        }];
        let snap = build_schema_snapshot(fed, evidence);
        let names: Vec<&str> = snap.tables().map(|(_, t)| t.name.as_str()).collect();
        assert!(names.contains(&"t_order"));
        let (_, t_order) = snap
            .tables()
            .find(|(_, t)| t.name == "t_order")
            .expect("t_order present");
        assert!(t_order.columns.iter().any(|c| c.name == "fed_col"));
        assert!(!t_order.columns.iter().any(|c| c.name == "evidence_col"));
        assert!(names.contains(&"t_other"));
    }

    #[test]
    fn empty_federation_and_evidence_gives_empty_snapshot() {
        let snap = build_schema_snapshot(Vec::new(), Vec::new());
        assert!(snap.is_empty());
    }

    #[test]
    fn provider_is_called_per_invocation() {
        let cell = Arc::new(std::sync::Mutex::new(PhysicalSchemaSnapshot::default()));
        let c = cell.clone();
        let p: MappingSnapshotProvider = Arc::new(move || c.lock().unwrap().clone());
        assert!(snapshot_now(&p).is_empty());
        cell.lock().unwrap().sources.push(PhysicalSource {
            name: "db".into(),
            kind: ontology_store::mapping::SOURCE_KIND_FEDERATION.into(),
            tables: vec![PhysicalTable {
                name: "t_order".into(),
                ..Default::default()
            }],
        });
        assert!(!snapshot_now(&p).is_empty());
    }
}

