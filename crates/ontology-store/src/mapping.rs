//! 元素映射绑定层：本体元素（类/属性/关系）→ 物理表字段。
//!
//! 设计文档：`openspec/changes/element-mapping-binding-layer/design.md`（决策 25）。
//!
//! 核心约束：
//! 1. **概念层与物理层分离**——映射不写进 Turtle 文本、不进 `OntologyPayload` 契约字段。
//!    与 W3C 自身的答案一致（R2RML 是独立于本体的映射文档）。
//! 2. **两套标准共用一张表**——与 `element_evidence` 同构的四元定位
//!    （`standard` + `ontology_key` + `element_type` + `element_id`），
//!    一条读写/校验/导出路径，不为第二条标准复制实现。
//! 3. **元素可多行**——唯一约束覆盖到物理位置，允许一个属性对应多个候选列、
//!    一个类对应视图或多表。重复保存完全相同的组合天然幂等。
//! 4. **核对状态由系统判定**——写入时对物理位置在已登记 schema 源中查找，
//!    调用方无法自行声明"已核对"（否则闸门会被 LLM 一律声明为通过）。
//! 5. **不阻断**——映射缺失/未核对不阻断导入与发布；孤儿与失效只进报告，不级联删除。
//!
//! `ontology_key` 取值：Palantir = 本体 api_name；W3C = 本体 IRI。
//! `element_id` 取值：Palantir = 元素 api_name；W3C = 元素 IRI 原文形态（可能含双 `#`）。

use std::collections::HashSet;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::StoreResult;

// ════════════════════════════════════════════════════════════════
// 常量
// ════════════════════════════════════════════════════════════════

/// 本体标准：平台 payload（object_type / property / link）。
pub const STANDARD_PALANTIR: &str = "palantir";
/// 本体标准：W3C Turtle（class / datatype_property / object_property）。
pub const STANDARD_W3C: &str = "w3c";

/// 元素类型：类（Palantir `object_type` / W3C `class`）。
pub const ETYPE_OBJECT_TYPE: &str = "object_type";
pub const ETYPE_CLASS: &str = "class";
/// 元素类型：属性（Palantir `property` / W3C `datatype_property`）。
pub const ETYPE_PROPERTY: &str = "property";
pub const ETYPE_DATATYPE_PROPERTY: &str = "datatype_property";
/// 元素类型：关系（Palantir `link` / W3C `object_property`）。
pub const ETYPE_LINK: &str = "link";
pub const ETYPE_OBJECT_PROPERTY: &str = "object_property";

/// 来源类别：从真实联邦数据源探查得到（最可信）。
pub const SOURCE_KIND_FEDERATION: &str = "federation";
/// 来源类别：从已登记的证据源（kind=`db_table`/`db_column`）解析得到。
pub const SOURCE_KIND_EVIDENCE: &str = "evidence";
/// 来源类别：人工/模型显式填写，未经任何 schema 源确认。
pub const SOURCE_KIND_MANUAL: &str = "manual";

/// 元素大类（覆盖率按此分桶统计；类与属性互不推导）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Class,
    Property,
    Relation,
    Other,
}

impl ElementKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ElementKind::Class => "class",
            ElementKind::Property => "property",
            ElementKind::Relation => "relation",
            ElementKind::Other => "other",
        }
    }
}

/// 元素类型 → 元素大类（两套标准共用同一映射表，分桶口径必须一致）。
pub fn element_kind(standard: &str, element_type: &str) -> ElementKind {
    match (standard, element_type) {
        (STANDARD_PALANTIR, ETYPE_OBJECT_TYPE) | (STANDARD_W3C, ETYPE_CLASS) => ElementKind::Class,
        (STANDARD_PALANTIR, ETYPE_PROPERTY) | (STANDARD_W3C, ETYPE_DATATYPE_PROPERTY) => {
            ElementKind::Property
        }
        (STANDARD_PALANTIR, ETYPE_LINK) | (STANDARD_W3C, ETYPE_OBJECT_PROPERTY) => {
            ElementKind::Relation
        }
        _ => ElementKind::Other,
    }
}

/// 元素类型是否被该标准接受（防止 W3C 元素写成 Palantir 类型）。
pub fn is_valid_element_type(standard: &str, element_type: &str) -> bool {
    match standard {
        STANDARD_PALANTIR => matches!(
            element_type,
            ETYPE_OBJECT_TYPE | ETYPE_PROPERTY | ETYPE_LINK
        ),
        STANDARD_W3C => matches!(
            element_type,
            ETYPE_CLASS | ETYPE_DATATYPE_PROPERTY | ETYPE_OBJECT_PROPERTY
        ),
        _ => false,
    }
}

// ════════════════════════════════════════════════════════════════
// 物理位置
// ════════════════════════════════════════════════════════════════

/// 一个物理位置（表或列）。`column` 为空表示"表级"定位（类映射）。
///
/// 三段式（catalog/schema/table）与 `BackingMapping` 一致，便于与 payload 契约互转；
/// 空段不参与拼接，避免产出 `..db.t.x` 这种噪声定位串。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq, Default, Hash)]
pub struct MappingEndpoint {
    #[serde(default)]
    pub catalog: String,
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub table: String,
    #[serde(default)]
    pub column: String,
}

impl MappingEndpoint {
    /// 是否为空（未使用）。关系只填一端时另一端为空。
    pub fn is_empty(&self) -> bool {
        self.catalog.is_empty()
            && self.schema.is_empty()
            && self.table.is_empty()
            && self.column.is_empty()
    }

    /// 是否有表级定位（决定能否被 schema 快照校验）。
    pub fn has_table(&self) -> bool {
        !self.table.is_empty()
    }

    /// 人类可读定位串（跳过空段）：`db.schema.table.column` / `schema.table`。
    pub fn locator(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(4);
        for seg in [&self.catalog, &self.schema, &self.table, &self.column] {
            if !seg.is_empty() {
                parts.push(seg);
            }
        }
        parts.join(".")
    }

    /// 表级定位串（不含列）：用于"类映射到表"的展示与同表先验匹配。
    pub fn table_locator(&self) -> String {
        let mut parts: Vec<&str> = Vec::with_capacity(3);
        for seg in [&self.catalog, &self.schema, &self.table] {
            if !seg.is_empty() {
                parts.push(seg);
            }
        }
        parts.join(".")
    }
}

// ════════════════════════════════════════════════════════════════
// 读写 DTO
// ════════════════════════════════════════════════════════════════

/// 写入一条映射的入参（不含核对状态——核对由系统判定，调用方无权声明）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct ElementMappingInput {
    /// 来源端物理位置；属性/类只用这一端。
    pub source: MappingEndpoint,
    /// 目标端物理位置；仅关系（JOIN 条件）使用，其余留空。
    #[serde(default)]
    pub target: MappingEndpoint,
    /// 来源类别：federation / evidence / manual（写错按 manual 处理）。
    #[serde(default)]
    pub source_kind: String,
    /// 来源引用（数据源名 / evidence source_id / 空）。
    #[serde(default)]
    pub source_ref: String,
    /// 自由备注（复合列说明、连接条件补充；导出到 CSV 备注列）。
    #[serde(default)]
    pub note: String,
}

impl ElementMappingInput {
    /// 便捷构造：只给表 + 列（人工填写的最常见形态）。
    pub fn manual(table: &str, column: &str) -> Self {
        Self {
            source: MappingEndpoint {
                table: table.to_string(),
                column: column.to_string(),
                ..Default::default()
            },
            source_kind: SOURCE_KIND_MANUAL.to_string(),
            ..Default::default()
        }
    }

    /// 规范化来源类别（未知值一律降级为 manual，避免污染来源统计）。
    pub fn normalized_source_kind(&self) -> &str {
        match self.source_kind.as_str() {
            SOURCE_KIND_FEDERATION => SOURCE_KIND_FEDERATION,
            SOURCE_KIND_EVIDENCE => SOURCE_KIND_EVIDENCE,
            _ => SOURCE_KIND_MANUAL,
        }
    }
}

/// 按元素整组替换的入参（对齐 `persist_element_evidence` 的替换语义）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct ElementMappingItem {
    /// object_type | property | link（palantir）/ class | datatype_property | object_property（w3c）
    pub element_type: String,
    /// palantir: api_name；w3c: 元素 IRI 原文形态
    pub element_id: String,
    /// 该元素的**全部**映射；空集合 = 删除该元素的全部映射。
    #[serde(default)]
    pub mappings: Vec<ElementMappingInput>,
}

/// 一条映射记录（读视图）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ElementMapping {
    pub id: String,
    pub standard: String,
    pub ontology_key: String,
    pub element_type: String,
    pub element_id: String,
    #[serde(default)]
    pub source: MappingEndpoint,
    #[serde(default)]
    pub target: MappingEndpoint,
    /// 系统判定的核对状态（写入时校验，调用方不可声明）。
    pub verified: bool,
    /// 最近核对时刻（unix ms；0 = 从未核对）。
    #[specta(type = specta_typescript::Number)]
    pub verified_at: i64,
    pub source_kind: String,
    #[serde(default)]
    pub source_ref: String,
    #[serde(default)]
    pub note: String,
    /// 创建时刻（unix ms）
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
    /// 最近更新时刻（unix ms）
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

impl ElementMapping {
    /// 关系是否只映射了单端（导出与界面需明确标为不完整）。
    pub fn is_partial(&self) -> bool {
        let kind = element_kind(&self.standard, &self.element_type);
        kind == ElementKind::Relation
            && (!self.target.has_table() || !self.source.has_table())
    }
}

/// 精简物理引用（嵌入 Agent 只读视图 `PropertyReadView.backing`）。
///
/// 独立于 `ElementMapping`：只读视图只需要"绑到哪儿 + 是否核对过"，
/// 不需要 id/时间戳/来源类别——避免把整张映射表的字段灌进每个属性的返回体。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq, Default)]
pub struct MappingRef {
    #[serde(default)]
    pub catalog: String,
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub table: String,
    #[serde(default)]
    pub column: String,
    /// 系统判定的核对状态。
    #[serde(default)]
    pub verified: bool,
}

impl MappingRef {
    pub fn from_endpoint(ep: &MappingEndpoint, verified: bool) -> Self {
        Self {
            catalog: ep.catalog.clone(),
            schema: ep.schema.clone(),
            table: ep.table.clone(),
            column: ep.column.clone(),
            verified,
        }
    }

    pub fn endpoint(&self) -> MappingEndpoint {
        MappingEndpoint {
            catalog: self.catalog.clone(),
            schema: self.schema.clone(),
            table: self.table.clone(),
            column: self.column.clone(),
        }
    }

    pub fn locator(&self) -> String {
        self.endpoint().locator()
    }
}

/// 元素（含其语义上下文）。
///
/// 前五个字段是候选建议的输入；`owner_*` / `target_*` / 端点属性 / 基数 / 描述
/// 是为「多工作表映射导出」补的语义上下文（见
/// `openspec/changes/mapping-export-multisheet/design.md` D3）。
/// 属性只填 `owner_*`，关系填 `owner_*` + `target_*` + 两端属性 + 基数，类全空。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct MappingElementRef {
    pub element_type: String,
    pub element_id: String,
    /// 元素人类可读名（label / display_name）；名称归一的输入之一。
    #[serde(default)]
    pub display_name: String,
    /// 属性声明的 DataType（类为空）；类型兼容性打分的输入。
    #[serde(default)]
    pub data_type: String,
    /// 已知同表线索（该元素所属类已映射到的物理表定位串；空 = 无线索）。
    #[serde(default)]
    pub same_table_hint: String,

    // ── 语义上下文（导出用；不参与候选建议打分）──
    /// 所属对象标识（属性所属 object_type / class；关系为源对象）；类为空。
    #[serde(default)]
    pub owner_id: String,
    /// 所属对象中文描述（导出表用；空 = 未设置）。
    #[serde(default)]
    pub owner_name: String,
    /// 关系目标对象标识；非关系为空。
    #[serde(default)]
    pub target_id: String,
    /// 关系目标对象中文描述；非关系为空。
    #[serde(default)]
    pub target_name: String,
    /// 关系源端属性标识（外键所在属性）；未声明或非关系为空。
    #[serde(default)]
    pub endpoint_property: String,
    /// 关系目标端属性标识（通常为目标对象主键）；未声明或非关系为空。
    #[serde(default)]
    pub target_property: String,
    /// 关系基数（ONE / MANY）；非关系为空。
    #[serde(default)]
    pub cardinality: String,
    /// 元素自身中文描述（rdfs:comment / description）。
    #[serde(default)]
    pub description: String,
}

/// 一条候选建议（只读产物，不落库）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct MappingSuggestion {
    pub element_type: String,
    pub element_id: String,
    /// 候选物理列。
    pub candidate: MappingEndpoint,
    /// 置信度（0.0–1.0，各维度权重之和，见 D6）。
    pub score: f64,
    /// 命中依据（人类可读，如「名称归一相等」「类型兼容」「同表」）。
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// 单个桶的覆盖率（已映射元素数 / 元素总数）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq, Default)]
pub struct MappingCoverageBucket {
    /// 已映射元素数（有至少一条映射的元素）。注意是**元素**数，不是映射行数。
    #[specta(type = specta_typescript::Number)]
    pub mapped: u32,
    /// 元素总数（来自本体内容，不是映射行数）。
    #[specta(type = specta_typescript::Number)]
    pub total: u32,
}

/// 映射覆盖率（类 / 属性 / 关系分别统计，互不推导）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Eq, Default)]
pub struct MappingCoverage {
    pub standard: String,
    pub ontology_key: String,
    #[serde(default)]
    pub classes: MappingCoverageBucket,
    #[serde(default)]
    pub properties: MappingCoverageBucket,
    #[serde(default)]
    pub relations: MappingCoverageBucket,
}

/// 完整性报告中的一条问题。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct MappingIntegrityItem {
    pub mapping_id: String,
    pub element_type: String,
    pub element_id: String,
    /// 物理位置定位串（便于用户直接找到那条绑定）。
    #[serde(default)]
    pub locator: String,
    /// 人类可读说明（面向用户，不暴露内部实现）。
    pub detail: String,
}

/// 映射完整性报告：三类问题，**均不自动删除、不阻断任何操作**。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct MappingIntegrityReport {
    pub standard: String,
    pub ontology_key: String,
    /// 元素在本体当前内容中已不存在（改名/删除的残留）。
    #[serde(default)]
    pub orphans: Vec<MappingIntegrityItem>,
    /// 曾被核对，但来源物理结构其后发生变化（仅按结构差异判定，连接失败不算）。
    #[serde(default)]
    pub stale: Vec<MappingIntegrityItem>,
    /// 从未核对成功（冷启动阶段正常，仅是提示）。
    #[serde(default)]
    pub unverified: Vec<MappingIntegrityItem>,
}

impl MappingIntegrityReport {
    /// 问题总数（概览卡与告警聚合用）。
    pub fn issue_count(&self) -> usize {
        self.orphans.len() + self.stale.len() + self.unverified.len()
    }

    pub fn is_clean(&self) -> bool {
        self.issue_count() == 0
    }
}

/// 导出范围。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MappingExportScope {
    /// 全部元素（已映射 + 未映射）。
    #[default]
    All,
    /// 仅缺口：未映射的元素 + 未核对的映射行。
    Gap,
}

impl MappingExportScope {
    pub fn parse(s: &str) -> Self {
        match s {
            "gap" => MappingExportScope::Gap,
            _ => MappingExportScope::All,
        }
    }
}

// ════════════════════════════════════════════════════════════════
// Schema（与 element_evidence 同构；两 store 共用同一 db 文件）
// ════════════════════════════════════════════════════════════════

/// 建 `element_mapping` 表与索引（幂等）。
///
/// **两处调用**：`schema.rs::init_schema`（Palantir store）与
/// `ttl/mod.rs::init_ttl_schema`（W3C store）。生产环境两 store 共用同一 `.db`
/// 文件，任一侧先打开都要建出空表；建表语句只写在本函数里，禁止复制粘贴
/// （历史教训：`element_evidence` 双处建表靠人工同步，改一处漏一处）。
pub fn init_element_mapping_schema(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS element_mapping (
            id             TEXT PRIMARY KEY,
            standard       TEXT NOT NULL,              -- 'palantir' | 'w3c'
            ontology_key   TEXT NOT NULL,              -- palantir: 本体 api_name；w3c: 本体 IRI
            element_type   TEXT NOT NULL,              -- 见 mapping::ETYPE_*
            element_id     TEXT NOT NULL,              -- palantir: api_name；w3c: 元素 IRI 原文形态
            source_catalog TEXT NOT NULL DEFAULT '',
            source_schema  TEXT NOT NULL DEFAULT '',
            source_table   TEXT NOT NULL DEFAULT '',
            source_column  TEXT NOT NULL DEFAULT '',
            target_catalog TEXT NOT NULL DEFAULT '',
            target_schema  TEXT NOT NULL DEFAULT '',
            target_table   TEXT NOT NULL DEFAULT '',
            target_column  TEXT NOT NULL DEFAULT '',
            source_kind    TEXT NOT NULL DEFAULT 'manual', -- 'federation' | 'evidence' | 'manual'
            source_ref     TEXT NOT NULL DEFAULT '',
            verified       INTEGER NOT NULL DEFAULT 0,     -- 0/1，由系统写入时判定
            verified_at    INTEGER NOT NULL DEFAULT 0,     -- unix ms；0 = 从未核对
            note           TEXT NOT NULL DEFAULT '',
            created_at     INTEGER NOT NULL,
            updated_at     INTEGER NOT NULL,
            -- 唯一约束覆盖到物理位置（不是 element_id）：一个元素允许多行
            --（多候选列 / 视图 / 多表副本），重复保存相同组合天然幂等。
            UNIQUE(standard, ontology_key, element_type, element_id,
                   source_catalog, source_schema, source_table, source_column,
                   target_catalog, target_schema, target_table, target_column)
        );
        CREATE INDEX IF NOT EXISTS idx_em_lookup
            ON element_mapping(standard, ontology_key);
        CREATE INDEX IF NOT EXISTS idx_em_element
            ON element_mapping(standard, ontology_key, element_type, element_id);
        "#,
    )?;
    Ok(())
}

/// 判断某库是否已存在 `element_mapping` 表（迁移/回填前置检查与单测用）。
pub fn element_mapping_table_exists(conn: &Connection) -> StoreResult<bool> {
    let mut stmt = conn.prepare(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='element_mapping'",
    )?;
    let n: i64 = stmt.query_row([], |r| r.get(0))?;
    Ok(n > 0)
}

/// 收集一组元素 id（用于孤儿检测与覆盖率：本体内容侧的"元素全集"）。
pub fn collect_element_ids<'a>(
    elements: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HashSet<(String, String)> {
    elements
        .into_iter()
        .map(|(t, id)| (t.to_string(), id.to_string()))
        .collect()
}

// ════════════════════════════════════════════════════════════════
// CRUD
// ════════════════════════════════════════════════════════════════

/// 写入结果：成功写入的行数 + 聚合告警（告警不阻断写入）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MappingWriteOutcome {
    pub written: usize,
    /// 被整组替换移除的旧行数。
    pub removed: usize,
    /// 聚合告警（面向调用方拼成一句话，不逐条刷屏）。
    pub warnings: Vec<String>,
}

/// 物理位置的唯一键（不含 standard/ontology/element，用于组内判重与旧行保留判断）。
type PositionKey = (String, String, String, String, String, String, String, String);

fn position_key(source: &MappingEndpoint, target: &MappingEndpoint) -> PositionKey {
    (
        source.catalog.clone(),
        source.schema.clone(),
        source.table.clone(),
        source.column.clone(),
        target.catalog.clone(),
        target.schema.clone(),
        target.table.clone(),
        target.column.clone(),
    )
}

/// 某元素按整组替换写入映射（对齐 `persist_element_evidence` 的替换语义）。
///
/// - 入参是该元素的**全量**映射：未出现在入参中的旧行会被删除（`removed` 计数）。
/// - 完全相同的物理位置组合天然幂等（`ON CONFLICT DO UPDATE`，保留创建时间）。
/// - `snapshot` 为 `None` 时不改判已有行的核对状态（避免误把已核对记录降级）；
///   新写入的行一律 `verified=0`。
pub fn upsert_element_mappings(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
    items: impl IntoIterator<Item = ElementMappingItem>,
    snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
    now_ms: i64,
) -> StoreResult<MappingWriteOutcome> {
    let mut outcome = MappingWriteOutcome::default();
    let tx = conn.unchecked_transaction()?;
    for item in items {
        if !is_valid_element_type(standard, &item.element_type) {
            outcome.warnings.push(format!(
                "元素 '{}' 的类型 '{}' 与本体标准不符，已跳过",
                item.element_id, item.element_type
            ));
            continue;
        }
        // 旧行位置 → (verified, verified_at)，供"未变位置保留核对状态"使用
        let existing = {
            let mut stmt = tx.prepare(
                "SELECT source_catalog, source_schema, source_table, source_column,
                        target_catalog, target_schema, target_table, target_column,
                        verified, verified_at
                 FROM element_mapping
                 WHERE standard=?1 AND ontology_key=?2 AND element_type=?3 AND element_id=?4",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![standard, ontology_key, item.element_type, item.element_id],
                    |r| {
                        let key: PositionKey = (
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                        );
                        Ok((key, r.get::<_, i64>(8)? != 0, r.get::<_, i64>(9)?))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let desired: HashSet<PositionKey> = item
            .mappings
            .iter()
            .map(|m| position_key(&m.source, &m.target))
            .collect();

        // 整组替换：删掉本次未出现的旧行
        for (key, _, _) in &existing {
            if desired.contains(key) {
                continue;
            }
            let removed = tx.execute(
                "DELETE FROM element_mapping
                 WHERE standard=?1 AND ontology_key=?2 AND element_type=?3 AND element_id=?4
                   AND source_catalog=?5 AND source_schema=?6 AND source_table=?7
                   AND source_column=?8 AND target_catalog=?9 AND target_schema=?10
                   AND target_table=?11 AND target_column=?12",
                rusqlite::params![
                    standard,
                    ontology_key,
                    item.element_type,
                    item.element_id,
                    key.0,
                    key.1,
                    key.2,
                    key.3,
                    key.4,
                    key.5,
                    key.6,
                    key.7
                ],
            )?;
            outcome.removed += removed;
        }

        for m in &item.mappings {
            if !m.source.has_table() {
                outcome.warnings.push(format!(
                    "元素 '{}' 有一条映射缺少来源表，已跳过",
                    item.element_id
                ));
                continue;
            }
            let key = position_key(&m.source, &m.target);
            let prior = existing.iter().find(|(k, _, _)| *k == key);
            // 核对状态：有快照 → 系统判定；无快照 → 保留旧状态，新行为未核对
            let (verified, verified_at) = match (snapshot, prior) {
                (Some(snap), _) => {
                    let v = crate::mapping_verify::verify_mapping(&m.source, &m.target, snap);
                    match v {
                        crate::mapping_verify::MappingVerifyOutcome::Verified => (true, now_ms),
                        crate::mapping_verify::MappingVerifyOutcome::Unverified(reason) => {
                            outcome.warnings.push(reason);
                            (false, 0)
                        }
                        crate::mapping_verify::MappingVerifyOutcome::Skipped => {
                            (prior.map(|(_, v, _)| *v).unwrap_or(false), prior.map(|(_, _, at)| *at).unwrap_or(0))
                        }
                    }
                }
                (None, Some((_, v, at))) => (*v, *at),
                (None, None) => (false, 0),
            };
            tx.execute(
                "INSERT INTO element_mapping
                 (id, standard, ontology_key, element_type, element_id,
                  source_catalog, source_schema, source_table, source_column,
                  target_catalog, target_schema, target_table, target_column,
                  source_kind, source_ref, verified, verified_at, note, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
                 ON CONFLICT(standard, ontology_key, element_type, element_id,
                             source_catalog, source_schema, source_table, source_column,
                             target_catalog, target_schema, target_table, target_column)
                 DO UPDATE SET
                     source_kind = excluded.source_kind,
                     source_ref  = excluded.source_ref,
                     note        = excluded.note,
                     verified    = excluded.verified,
                     verified_at = excluded.verified_at,
                     updated_at  = excluded.updated_at",
                rusqlite::params![
                    format!("em-{}", uuid::Uuid::new_v4().simple()),
                    standard,
                    ontology_key,
                    item.element_type,
                    item.element_id,
                    m.source.catalog,
                    m.source.schema,
                    m.source.table,
                    m.source.column,
                    m.target.catalog,
                    m.target.schema,
                    m.target.table,
                    m.target.column,
                    m.normalized_source_kind(),
                    m.source_ref,
                    if verified { 1 } else { 0 },
                    verified_at,
                    m.note,
                    now_ms,
                    now_ms
                ],
            )?;
            outcome.written += 1;
        }
    }
    tx.commit()?;
    Ok(outcome)
}

/// 列出某本体的全部映射（两套标准共用；按元素类型/标识/创建时间稳定排序）。
pub fn list_element_mappings(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
) -> StoreResult<Vec<ElementMapping>> {
    let mut stmt = conn.prepare(
        "SELECT id, standard, ontology_key, element_type, element_id,
                source_catalog, source_schema, source_table, source_column,
                target_catalog, target_schema, target_table, target_column,
                verified, verified_at, source_kind, source_ref, note, created_at, updated_at
         FROM element_mapping
         WHERE standard=?1 AND ontology_key=?2
         ORDER BY element_type, element_id, created_at, id",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![standard, ontology_key], |r| {
            Ok(ElementMapping {
                id: r.get(0)?,
                standard: r.get(1)?,
                ontology_key: r.get(2)?,
                element_type: r.get(3)?,
                element_id: r.get(4)?,
                source: MappingEndpoint {
                    catalog: r.get(5)?,
                    schema: r.get(6)?,
                    table: r.get(7)?,
                    column: r.get(8)?,
                },
                target: MappingEndpoint {
                    catalog: r.get(9)?,
                    schema: r.get(10)?,
                    table: r.get(11)?,
                    column: r.get(12)?,
                },
                verified: r.get::<_, i64>(13)? != 0,
                verified_at: r.get(14)?,
                source_kind: r.get(15)?,
                source_ref: r.get(16)?,
                note: r.get(17)?,
                created_at: r.get(18)?,
                updated_at: r.get(19)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// 列出某单个元素的全部映射（Agent 只读视图注入用）。
pub fn list_element_mappings_for(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
    element_type: &str,
    element_id: &str,
) -> StoreResult<Vec<ElementMapping>> {
    Ok(list_element_mappings(conn, standard, ontology_key)?
        .into_iter()
        .filter(|m| m.element_type == element_type && m.element_id == element_id)
        .collect())
}

/// 删除某元素的全部映射（返回删除行数）。
pub fn delete_element_mappings_for_element(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
    element_type: &str,
    element_id: &str,
) -> StoreResult<usize> {
    Ok(conn.execute(
        "DELETE FROM element_mapping
         WHERE standard=?1 AND ontology_key=?2 AND element_type=?3 AND element_id=?4",
        rusqlite::params![standard, ontology_key, element_type, element_id],
    )?)
}

/// 按映射 id 批量删除（孤儿清理：**仅在用户确认后调用**，不做后台自动删除）。
pub fn delete_element_mappings_by_ids(
    conn: &Connection,
    ids: &[String],
) -> StoreResult<usize> {
    let mut removed = 0usize;
    let tx = conn.unchecked_transaction()?;
    for id in ids {
        removed += tx.execute("DELETE FROM element_mapping WHERE id=?1", rusqlite::params![id])?;
    }
    tx.commit()?;
    Ok(removed)
}

/// 删除某本体（key 粒度）的全部映射（本体删除时调用）。
pub fn delete_element_mappings(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
) -> StoreResult<usize> {
    Ok(conn.execute(
        "DELETE FROM element_mapping WHERE standard=?1 AND ontology_key=?2",
        rusqlite::params![standard, ontology_key],
    )?)
}

/// 某元素的"最佳"映射（模型侧 hint：verified 优先，同状态取最近更新）。
pub fn primary_mapping_for(
    conn: &Connection,
    standard: &str,
    ontology_key: &str,
    element_type: &str,
    element_id: &str,
) -> StoreResult<Option<ElementMapping>> {
    let mut rows = list_element_mappings_for(
        conn,
        standard,
        ontology_key,
        element_type,
        element_id,
    )?;
    rows.sort_by(|a, b| {
        b.verified
            .cmp(&a.verified)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    Ok(rows.into_iter().next())
}

// ════════════════════════════════════════════════════════════════
// 覆盖率与完整性（纯函数：无 IO，输入全部由调用方给出）
// ════════════════════════════════════════════════════════════════

/// 计算覆盖率：`elements` 是本体内容侧的**元素全集**（element_type, element_id），
/// `mappings` 是该本体全部映射行。类/属性/关系分桶，互不推导。
///
/// 只统计存在于 `elements` 中的元素——孤儿映射不计入"已映射"（否则删掉元素后
/// 覆盖率反而显得更好）。
pub fn compute_coverage(
    standard: &str,
    ontology_key: &str,
    elements: &[(String, String)],
    mappings: &[ElementMapping],
) -> MappingCoverage {
    let mut cov = MappingCoverage {
        standard: standard.to_string(),
        ontology_key: ontology_key.to_string(),
        ..Default::default()
    };
    // 已存在的元素集合：孤儿映射不计入"已映射"（否则删掉元素后覆盖率反而变好）
    let live: HashSet<(&str, &str)> = elements
        .iter()
        .map(|(t, id)| (t.as_str(), id.as_str()))
        .collect();
    let mapped: HashSet<(&str, &str)> = mappings
        .iter()
        .map(|m| (m.element_type.as_str(), m.element_id.as_str()))
        .filter(|k| live.contains(k))
        .collect();

    for (etype, eid) in elements {
        let bucket = match element_kind(standard, etype) {
            ElementKind::Class => &mut cov.classes,
            ElementKind::Property => &mut cov.properties,
            ElementKind::Relation => &mut cov.relations,
            ElementKind::Other => continue,
        };
        bucket.total += 1;
        if mapped.contains(&(etype.as_str(), eid.as_str())) {
            bucket.mapped += 1;
        }
    }
    cov
}

/// 映射完整性报告：孤儿（元素已不存在）+ 失效（结构漂移）+ 未核对。
///
/// `snapshot` 为 `None` 或空时**不判失效**（连接失败/未注册不是结构事实，见 D13）。
/// 所有问题只报告，**不删除、不阻断**。
pub fn compute_integrity(
    standard: &str,
    ontology_key: &str,
    elements: &[(String, String)],
    mappings: &[ElementMapping],
    snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
) -> MappingIntegrityReport {
    let mut report = MappingIntegrityReport {
        standard: standard.to_string(),
        ontology_key: ontology_key.to_string(),
        ..Default::default()
    };
    let has_snapshot = snapshot.map(|s| !s.is_empty()).unwrap_or(false);

    for m in mappings {
        let exists = elements
            .iter()
            .any(|(t, id)| t == &m.element_type && id == &m.element_id);
        let locator = m.source.locator();
        if !exists {
            report.orphans.push(MappingIntegrityItem {
                mapping_id: m.id.clone(),
                element_type: m.element_type.clone(),
                element_id: m.element_id.clone(),
                locator: locator.clone(),
                detail: format!("元素已不在本体中，该绑定指向 {locator}"),
            });
            continue;
        }
        if has_snapshot && m.verified {
            let snap = snapshot.expect("has_snapshot implies Some");
            use crate::mapping_verify::{verify_mapping, MappingVerifyOutcome};
            if let MappingVerifyOutcome::Unverified(_) = verify_mapping(&m.source, &m.target, snap)
            {
                report.stale.push(MappingIntegrityItem {
                    mapping_id: m.id.clone(),
                    element_type: m.element_type.clone(),
                    element_id: m.element_id.clone(),
                    locator: locator.clone(),
                    detail: format!(
                        "曾核对过的物理位置 {locator} 在数据源中已不存在，需重新核对"
                    ),
                });
                continue;
            }
        }
        if !m.verified {
            report.unverified.push(MappingIntegrityItem {
                mapping_id: m.id.clone(),
                element_type: m.element_type.clone(),
                element_id: m.element_id.clone(),
                locator: locator.clone(),
                detail: format!("绑定 {locator} 尚未与数据源核对"),
            });
        }
    }
    report
}

/// 从完整性报告里取出孤儿映射 id（供 UI"一键清理孤儿"在用户确认后调用）。
pub fn orphan_mapping_ids(report: &MappingIntegrityReport) -> Vec<String> {
    report.orphans.iter().map(|i| i.mapping_id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_locator_skips_empty_segments() {
        let ep = MappingEndpoint {
            catalog: "db".into(),
            schema: String::new(),
            table: "t_order".into(),
            column: "order_no".into(),
        };
        assert_eq!(ep.locator(), "db.t_order.order_no");
        assert_eq!(ep.table_locator(), "db.t_order");
        assert!(ep.has_table());
        assert!(!ep.is_empty());
        assert!(MappingEndpoint::default().is_empty());
    }

    #[test]
    fn element_kind_covers_both_standards() {
        assert_eq!(
            element_kind(STANDARD_PALANTIR, ETYPE_OBJECT_TYPE),
            ElementKind::Class
        );
        assert_eq!(
            element_kind(STANDARD_PALANTIR, ETYPE_PROPERTY),
            ElementKind::Property
        );
        assert_eq!(
            element_kind(STANDARD_PALANTIR, ETYPE_LINK),
            ElementKind::Relation
        );
        assert_eq!(element_kind(STANDARD_W3C, ETYPE_CLASS), ElementKind::Class);
        assert_eq!(
            element_kind(STANDARD_W3C, ETYPE_DATATYPE_PROPERTY),
            ElementKind::Property
        );
        assert_eq!(
            element_kind(STANDARD_W3C, ETYPE_OBJECT_PROPERTY),
            ElementKind::Relation
        );
        // 串味（w3c 用了 palantir 类型名）不算有效
        assert!(!is_valid_element_type(STANDARD_W3C, ETYPE_PROPERTY));
        assert!(!is_valid_element_type(STANDARD_PALANTIR, ETYPE_CLASS));
        assert!(!is_valid_element_type("other", ETYPE_CLASS));
    }

    #[test]
    fn source_kind_unknown_downgrades_to_manual() {
        let mut input = ElementMappingInput::manual("t_order", "order_no");
        assert_eq!(input.normalized_source_kind(), SOURCE_KIND_MANUAL);
        input.source_kind = "federation".into();
        assert_eq!(input.normalized_source_kind(), SOURCE_KIND_FEDERATION);
        input.source_kind = "evidence".into();
        assert_eq!(input.normalized_source_kind(), SOURCE_KIND_EVIDENCE);
        input.source_kind = "llm-guess".into();
        assert_eq!(input.normalized_source_kind(), SOURCE_KIND_MANUAL);
    }

    #[test]
    fn partial_relation_mapping_detected() {
        let base = ElementMapping {
            id: "em-1".into(),
            standard: STANDARD_W3C.into(),
            ontology_key: "https://x#Ont".into(),
            element_type: ETYPE_OBJECT_PROPERTY.into(),
            element_id: "https://x#hasCustomer".into(),
            source: MappingEndpoint {
                table: "t_order".into(),
                column: "customer_id".into(),
                ..Default::default()
            },
            target: MappingEndpoint::default(),
            verified: false,
            verified_at: 0,
            source_kind: SOURCE_KIND_MANUAL.into(),
            source_ref: String::new(),
            note: String::new(),
            created_at: 1,
            updated_at: 1,
        };
        assert!(base.is_partial(), "关系仅单端应标为不完整");
        let mut full = base.clone();
        full.target = MappingEndpoint {
            table: "t_customer".into(),
            column: "id".into(),
            ..Default::default()
        };
        assert!(!full.is_partial());
        // 属性/类不参与"不完整"判定
        let mut prop = full.clone();
        prop.element_type = ETYPE_DATATYPE_PROPERTY.into();
        prop.target = MappingEndpoint::default();
        assert!(!prop.is_partial());
    }

    #[test]
    fn schema_init_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_element_mapping_schema(&conn).unwrap();
        // 重复初始化不报错（两 store 先后打开同一库的真实路径）
        init_element_mapping_schema(&conn).unwrap();
        assert!(element_mapping_table_exists(&conn).unwrap());
    }

    #[test]
    fn export_scope_parse() {
        assert_eq!(MappingExportScope::parse("gap"), MappingExportScope::Gap);
        assert_eq!(MappingExportScope::parse("all"), MappingExportScope::All);
        assert_eq!(MappingExportScope::parse(""), MappingExportScope::All);
        assert_eq!(MappingExportScope::parse("weird"), MappingExportScope::All);
    }
}