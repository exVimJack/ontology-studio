//! Payload 模型（对齐 Gaia `OntologyExportPayload` / `ObjectTypeBatchCreate` 等）。
//!
//! 这是 export/import 端点的 JSON 形态——agent 产出和消费的就是这个。
//! 字段严格对齐 Gaia pydantic schema，不自创字段。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta_typescript::Number;

/// 本体列表项（轻量摘要，用于 list_ontologies command）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct OntologySummary {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
}

// ════════════════════════════════════════════════════════════════
// 只读 summary / drill-in DTO（对齐 Gaia describe_ontology / list_object_types /
// describe_object_type 的 read-view，区别于上面 export/import 的 write-view payload）
// ════════════════════════════════════════════════════════════════
//
// 设计动机（决策：会话页面引用本体）：
// - `OntologyPayload`（export 产物）是完整 write-view，100+ OT 的本体可达 100KB+，
//   直接塞进 agent 上下文会撑爆预算。
// - Gaia 平台的 `describe_ontology` 已有 `summary=True` 投影 + `describe_object_type`
//   分层 drill-in 解决此问题。本地 store 复刻这套只读链路，会话引用场景走 summary →
//   按需 describe 单个 OT，总上下文增量 5-6KB（vs 整包 100KB+）。
// - 建模/导入场景仍用 `OntologyPayload`（export/import），两套语义分离。

/// 本体设计宪章（不变点，1:1 关联本体）。
///
/// 与 changelog（变化点）分离：changelog 记每次变更随历史增长，charter 记业务
/// 本质说明不随历史变化。由独立命令 `set_ontology_charter` 写入，不进 import 流程。
///
/// 四字段语义（决策：本体不变点）：
/// - `business_scenario`：业务场景（服务于什么业务目标、谁用、解决什么问题）
/// - `business_essence`：业务本质（核心业务对象/状态/关系/动态行为的一句话本质概括）
/// - `design_intent`：设计意图（为什么这样建模、够用且可扩展的取舍、可扩展方向）
/// - `invariants`：补充说明（自由文本，记录不可违反的业务约束、边界条件等）
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct OntologyCharter {
    #[serde(default)]
    pub business_scenario: String,
    #[serde(default)]
    pub business_essence: String,
    #[serde(default)]
    pub design_intent: String,
    #[serde(default)]
    pub invariants: String,
    /// "agent" | "user"
    #[serde(default)]
    pub updated_by: String,
    /// unix ms
    #[specta(type = Number)]
    pub updated_at: i64,
}

/// 本体 summary（对齐 Gaia `describe_ontology(summary=True)`）。
///
/// 轻量目录视图：每个 ObjectType 只给 api_name/display_name/primary_key/
/// storage_type/property_count，不含 properties[]/links[]/actions[]。
/// 供 agent 第一跳拿到 OT 概览，需要详情时调 `describe_object_type` drill-in。
///
/// 附带 `charter`（本体设计宪章，不变点）——agent 拿 OT 目录时同步拿到业务
/// 场景/本质/意图/补充说明，建立结构化业务认知后再钻取 OT 细节。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct OntologySummaryFull {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// 本体设计宪章（不变点）。无 charter 时为空结构体（各字段空串）。
    #[serde(default)]
    pub charter: OntologyCharter,
    #[serde(default)]
    pub object_types: Vec<ObjectTypeSummary>,
    /// LinkType 总数（不展开定义，仅计数）。
    #[serde(default)]
    pub link_type_count: usize,
    /// ActionType 总数。
    #[serde(default)]
    pub action_type_count: usize,
}

/// ObjectType 摘要项（对齐 Gaia summary 投影的 OT 条目）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ObjectTypeSummary {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub primary_key: String,
    #[serde(default)]
    pub storage_type: String,
    /// 属性数量（含主键）。
    pub property_count: usize,
}

/// ObjectType 列表项（对齐 Gaia `list_object_types`，比 summary 更轻）。
///
/// 仅 api_name/display_name/description/storage_type，无 primary_key/property_count。
/// 用于「只想知道有哪些 OT」的极轻量场景。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ObjectTypeBrief {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub storage_type: String,
}

/// ObjectType 完整元数据（对齐 Gaia `ObjectTypeFullMetadata` 的 read-view）。
///
/// Gaia 的 ObjectTypeFullMetadata：api_name/display_name/description/primary_key/
/// title_property/storage_type/visibility/status/backing_dataset_api_name/properties[]/
/// inbound_links(list[str])/outbound_links(list[str])/actions(list[str])/id。
///
/// inbound_links/outbound_links/actions 是 **api_name 字符串列表**（不是完整对象），
/// 对齐 Gaia——详情用 describe_link_type / 单独查 action。properties 是完整 `PropertyDef`
/// （含 backing_mapping 等物理映射，供 agent 理解列语义）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ObjectTypeFullMetadata {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub primary_key: String,
    #[serde(default)]
    pub title_property: String,
    pub storage_type: String,
    #[serde(default)]
    pub visibility: String,
    /// v5.2 lifecycle：ACTIVE/DEPRECATED。默认 ACTIVE。
    #[serde(default = "default_active_status")]
    pub status: String,
    /// 主 backing dataset（便利引用；权威绑定是 per-property backing_mapping）。None=未绑定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_dataset_api_name: Option<String>,
    #[serde(default)]
    pub properties: Vec<PropertyReadView>,
    /// 以本 OT 为 source 的 link api_name 列表(outgoing)。
    #[serde(default)]
    pub outbound_links: Vec<String>,
    /// 以本 OT 为 target 的 link api_name 列表（incoming）。
    #[serde(default)]
    pub inbound_links: Vec<String>,
    /// 作用于本 OT 的 action api_name 列表。
    #[serde(default)]
    pub actions: Vec<String>,
}

// ── LinkType 只读 drill-in（对齐 Gaia list_link_types / describe_link_type）──
//
// 与 OT 侧的 ObjectTypeBrief / ObjectTypeFullMetadata 对称：
// - LinkTypeBrief：名单视图（source/target/cardinality/fk），对应 Gaia list_link_types
// - LinkTypeFull：单条 link 完整定义，对应 Gaia describe_link_type
//
// 会话场景：agent 想看某条关系详情时，不用先 describe 承载它的 OT（outbound_links
// 里能拿到），可直接按 link api_name 查。适合「我只关心一条关系」的细粒度场景。

/// LinkType 列表项（对齐 Gaia `list_link_types`）。
///
/// 名单视图（对齐 Gaia `list_link_types`）：api_name + source/target OT +
/// cardinality + foreign_key。5 字段，无 display_name。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct LinkTypeBrief {
    pub api_name: String,
    /// source OT 的 api_name（关系发出方）。对齐 Gaia 字段名 `source_object_type`。
    pub source_object_type: String,
    pub target_object_type: String,
    pub cardinality: String,
    /// source 侧 FK 属性（m2m 时为 None）。可空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_key_property_api_name: Option<String>,
}

/// 单条 link 完整定义（对齐 Gaia `describe_link_type` 的 read-view）。
///
/// Gaia 的 describe_link_type 返回 9 字段：api_name/display_name/description/
/// source_object_type/target_object_type/foreign_key_property_api_name/cardinality/
/// directional/has_properties。不含 weight_property/temporal（那些是 graph-reasoning
/// 扩展，在 write-view `LinkDef` 里，不在 read-view 返回）。
///
/// - `directional`：link 是否有方向性（反向遍历是否有意义）。当前固定 true
///   （与 Gaia Sprint 1 行为一致——所有 link 默认有方向性，source→target 是正向）。
/// - `has_properties`：link 是否自带属性。当前固定 false（本地 schema 未实现 link 属性表）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct LinkTypeFull {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub source_object_type: String,
    pub target_object_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_key_property_api_name: Option<String>,
    pub cardinality: String,
    /// link 是否有方向性（反向遍历是否有意义）。固定 true（对齐 Gaia Sprint 1）。
    pub directional: bool,
    /// link 是否自带属性。固定 false（本地未实现 link 属性表）。
    pub has_properties: bool,
}

/// Export 产物 / Import 请求的 payload（对齐 Gaia `OntologyExportPayload`）。
///
/// `object_types` 用 `ObjectTypeBatchCreate` 形态（properties + links 内嵌），
/// `action_types` 用 `ActionTypeCreate` 形态。link 的 target 和 action 的 affected
/// 都用 api_name（不是 UUID），由 import 侧解析。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct OntologyPayload {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub object_types: Vec<ObjectTypeDef>,
    #[serde(default)]
    pub action_types: Vec<ActionTypeDef>,
    #[serde(default)]
    pub datasets: Vec<DatasetDef>,
    #[serde(default)]
    pub data_sources: Vec<DataSourceDef>,
    #[serde(default)]
    pub object_type_groups: Vec<ObjectTypeGroupDef>,
}

/// ObjectType 定义（对齐 Gaia `ObjectTypeBatchCreate`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ObjectTypeDef {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub primary_key: String,
    #[serde(default)]
    pub title_property: String,
    pub storage_type: String,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
    #[serde(default)]
    pub links: Vec<LinkDef>,
    /// SKILL 扩展字段，非 Gaia schema——import 时剥离，仅标注产物质量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

fn default_visibility() -> String {
    "NORMAL".to_string()
}

/// `ObjectTypeFullMetadata.status` 默认值——"ACTIVE"（对齐 Gaia v5.2 lifecycle）。
fn default_active_status() -> String {
    "ACTIVE".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub graph_indexing_enabled: bool,
    #[serde(default)]
    pub geotime_indexing_enabled: bool,
}

/// Property 定义（对齐 Gaia `PropertyInput`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct PropertyDef {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub data_type: String,
    #[serde(default = "default_true")]
    pub searchable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_primary_key: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_title_property: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_mapping: Option<BackingMapping>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector_config: Option<Value>,
    /// SKILL 扩展字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

/// Property 只读视图（对齐 Gaia MCP `describe_object_type` 返回的 7 字段）。
///
/// Gaia MCP 返回：api_name/display_name/data_type/is_primary_key/nullable/
/// filterable/sortable。不含 description/backing_mapping/vector_config/
/// is_title_property（那些是 write-view `PropertyDef` 的字段）。
///
/// 派生规则（对齐 Gaia `metadata.py`）：
/// - `nullable`：write-view 未收集，固定 true（对齐 Gaia ORM 默认 `nullable=True`）
/// - `filterable`：`searchable || is_primary_key`（Gaia 用 `indexed || is_primary_key`，
///   我们用 `searchable` 代替 `indexed`——语义一致：标记为可检索的列）
/// - `sortable`：data_type 为标量类型时 true，复杂类型（ARRAY/STRUCT/VECTOR/
///   GEOPOINT/GEOSHAPE/MEDIA_REFERENCE/ATTACHMENT）时 false
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct PropertyReadView {
    pub api_name: String,
    pub display_name: String,
    pub data_type: String,
    pub is_primary_key: bool,
    pub nullable: bool,
    pub filterable: bool,
    pub sortable: bool,
}

impl PropertyDef {
    /// 派生 `PropertyReadView`（对齐 Gaia MCP `describe_object_type` 的 property 返回）。
    pub fn to_read_view(&self) -> PropertyReadView {
        const UNSORTABLE: &[&str] = &[
            "ARRAY", "STRUCT", "VECTOR", "GEOPOINT", "GEOSHAPE",
            "MEDIA_REFERENCE", "ATTACHMENT",
        ];
        let is_pk = self.is_primary_key.unwrap_or(false);
        PropertyReadView {
            api_name: self.api_name.clone(),
            display_name: self.display_name.clone(),
            data_type: self.data_type.clone(),
            is_primary_key: is_pk,
            nullable: true,
            filterable: self.searchable || is_pk,
            sortable: !UNSORTABLE.contains(&self.data_type.to_uppercase().as_str()),
        }
    }
}

fn default_true() -> bool {
    true
}

/// backing_mapping（对齐 Gaia `BackingColumnRef`，五字段）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct BackingMapping {
    #[serde(default)]
    pub dataset_api_name: String,
    #[serde(default)]
    pub backing_catalog: String,
    #[serde(default)]
    pub backing_schema: String,
    #[serde(default)]
    pub backing_table: String,
    #[serde(default)]
    pub backing_column: String,
}

/// LinkType 定义（对齐 Gaia `LinkInput` / `LinkTypeDefCreate`）。
/// 字段命名说明：Gaia 的 `LinkInput` 用 `target_object_type_id`（历史命名怪癖，
/// 实际存的是 api_name 而非 UUID）。我们用 `target_object_type_api_name`
/// 作为 Rust 字段名（语义更准确），但 serde 同时接受 `target_object_type_id`
/// 以兼容 Gaia 导出 / Ascend 等第三方本体 JSON。
///
/// 对齐 Gaia schema：无 `direction` 字段。link 由 source 侧声明即隐含 outgoing，
/// 反向遍历是否有意义由派生属性 `directional` 表达（Gaia `describe_link_type` 返回）。
/// 导入时若 JSON 含 `direction` 字段，serde 默认忽略（无对应字段不报错）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct LinkDef {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// 用 api_name 引用（不是 UUID）。serde alias 兼容 Gaia 的 `target_object_type_id`。
    #[serde(alias = "target_object_type_id")]
    pub target_object_type_api_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign_key_property_api_name: Option<String>,
    pub cardinality: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_property: Option<String>,
    #[serde(default)]
    pub temporal: bool,
    /// SKILL 扩展字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

/// ActionType 定义（对齐 Gaia `ActionTypeCreate`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ActionTypeDef {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub affected_object_type_api_name: String,
    #[serde(default)]
    pub parameters: Vec<Value>,
    #[serde(default)]
    pub rules: Vec<Value>,
    #[serde(default)]
    pub submission_criteria: Vec<Value>,
    #[serde(default)]
    pub effects: Vec<Value>,
    #[serde(default)]
    pub ontology_rules: Vec<Value>,
    #[serde(default = "default_risk")]
    pub risk_level: String,
    #[serde(default = "default_operation_kind")]
    pub operation_kind: String,
    #[serde(default)]
    pub batch_enabled: bool,
    /// SKILL 扩展字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

fn default_risk() -> String {
    "low".to_string()
}
fn default_operation_kind() -> String {
    "mixed".to_string()
}

/// Dataset 定义（对齐 Gaia `DatasetGovernanceCreate`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct DatasetDef {
    pub api_name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub storage_location: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_config: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_dataset_api_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source_api_name: Option<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub is_view: bool,
    /// SKILL 扩展字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

fn default_kind() -> String {
    "MANAGED".to_string()
}

/// DataSource 定义（对齐 Gaia `DataSourceCreate`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct DataSourceDef {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub connector_type: String,
    #[serde(default)]
    pub connector_config: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    /// SKILL 扩展字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

/// ObjectTypeGroup 定义（对齐 Gaia `ObjectTypeGroupCreate` + members）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ObjectTypeGroupDef {
    pub api_name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub members: Vec<String>,
    /// SKILL 扩展字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

// ═══ Import 请求 / 结果 / Preview（对齐 Gaia）═══

/// Import 请求（对齐 Gaia `OntologyImportRequest`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ImportRequest {
    pub payload: OntologyPayload,
    /// 用户选择覆写的 ObjectType api_name 列表；未列入的同名 OT 默认 skip。
    #[serde(default)]
    pub overwrite_object_types: Vec<String>,
    /// 用户选择覆写的 DataSource api_name 列表；未列入的同名 DS 默认 skip。
    /// DataSource 是全局共享物理资产，重导入默认不覆写；列入此项时
    /// 会用 payload 里的 connector_config 等字段 UPDATE 已有记录
    /// （用于从脱敏 `***` 升级到真实凭据等场景）。
    #[serde(default)]
    pub overwrite_data_sources: Vec<String>,
}

/// 单个实体的导入结果。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ImportItemResult {
    pub api_name: String,
    pub status: String, // created | skipped | overwritten | failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Import 总结果（对齐 Gaia `OntologyImportResult`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ImportResult {
    pub ontology_api_name: String,
    pub ontology_status: String, // created | existed
    #[serde(default)]
    pub object_types: Vec<ImportItemResult>,
    #[serde(default)]
    pub links_created: i32,
    #[serde(default)]
    pub links_skipped: i32,
    #[serde(default)]
    pub action_types: Vec<ImportItemResult>,
    #[serde(default)]
    pub datasets: Vec<ImportItemResult>,
    #[serde(default)]
    pub data_sources: Vec<ImportItemResult>,
    #[serde(default)]
    pub object_type_groups: Vec<ImportItemResult>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// Preview 单项（对齐 Gaia `ImportPreviewItem`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ImportPreviewItem {
    pub api_name: String,
    pub status: String, // create | skip | overwrite | fail
    #[serde(default)]
    pub reason: String,
}

/// Preview 总结果（对齐 Gaia `OntologyImportPreview`）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ImportPreview {
    pub ontology_api_name: String,
    pub ontology_status: String, // create | skip
    #[serde(default)]
    pub object_types: Vec<ImportPreviewItem>,
    #[serde(default)]
    pub links: Vec<ImportPreviewItem>,
    #[serde(default)]
    pub actions: Vec<ImportPreviewItem>,
    #[serde(default)]
    pub datasets: Vec<ImportPreviewItem>,
    #[serde(default)]
    pub data_sources: Vec<ImportPreviewItem>,
    #[serde(default)]
    pub object_type_groups: Vec<ImportPreviewItem>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub errors: Vec<String>,
}

/// 本体变更日志条目（git commit log 式）。
/// 每条记录一次 import/delete 后的设计说明：title+body 为人可读的 commit message,
/// change_summary 为机器可读的实体级 +/−/~ 摘要。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct OntologyChangelog {
    pub revision: u32,
    pub title: String,
    pub body: String,
    /// JSON 字符串：{"created":[...],"deleted":[...],"modified":[...]}
    pub change_summary: String,
    /// 来源会话 id（可空，手工导入无）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// "agent" | "user"
    pub author: String,
    /// unix ms
    #[specta(type = Number)]
    pub created_at: i64,
}
