//! 映射导出：数据集装配 + 多工作表渲染。
//!
//! 设计见 `openspec/changes/mapping-export-multisheet/design.md`：
//! - **先装配、后渲染**（D2）：装配层与本体标准相关（查表 / 解析 TTL），
//!   渲染层与标准无关。三类元素列集不同，若继续用「一套 columns + 全行」的模式，
//!   列定义会退化成按 sheet 名分支判断，很快失控。
//! - **单一数据集**：xlsx 导出与 Agent 侧文本预览共用同一份 `ExportDataset`，
//!   避免两处各自维护列语义（旧实现里 CSV 是唯一出口，所以只有一份；新增 xlsx 后
//!   若各写一份，列口径必然漂移）。
//! - **业务可读名**（D3/规格）：元素以「技术标识 + 中文描述」成对分列呈现，
//!   不输出带命名空间的完整 IRI。
//! - **状态两态**（D7）：有映射 = 已映射，无映射 = 未映射。核对质量不进入交付文档。

use std::collections::HashMap;

use rust_xlsxwriter::{Format, FormatAlign, Workbook};
use serde::{Deserialize, Serialize};

use crate::error::{StoreError, StoreResult};
use crate::mapping::{
    element_kind, ElementKind, ElementMapping, MappingElementRef, MappingExportScope,
};
use crate::mapping_verify::PhysicalSchemaSnapshot;

// ════════════════════════════════════════════════════════════════
// 表头契约（导出规格的一部分，顺序固定）
// ════════════════════════════════════════════════════════════════

/// 概览页（字段名 / 值 两列）。
pub const OVERVIEW_SHEET: &str = "概览";
pub const CLASS_SHEET: &str = "类";
pub const PROPERTY_SHEET: &str = "属性";
pub const RELATION_SHEET: &str = "关系";

pub const CLASS_HEADERS: &[&str] = &["对象名称", "对象描述", "来源表", "映射状态", "备注"];

pub const PROPERTY_HEADERS: &[&str] = &[
    "对象名称",
    "对象描述",
    "属性名称",
    "属性描述",
    "属性数据类型",
    "是否主键",
    "来源表",
    "来源字段",
    "字段类型",
    "可空",
    "映射状态",
    "备注",
];

pub const RELATION_HEADERS: &[&str] = &[
    "关系名称",
    "关系描述",
    "源对象",
    "源属性",
    "目标对象",
    "目标属性",
    "基数",
    "连接条件",
    "映射状态",
    "备注",
];

/// 映射状态文案（对外只有两态，见 D7）。
pub const STATUS_MAPPED: &str = "已映射";
pub const STATUS_UNMAPPED: &str = "未映射";

// ════════════════════════════════════════════════════════════════
// 数据集
// ════════════════════════════════════════════════════════════════

/// 概览页内容：承接被移除的「本体标准 / 本体」两列（只出现一次，不逐行重复）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportOverview {
    /// 本体显示名（业务名；缺省时调用方传标识）。
    pub ontology_name: String,
    /// 本体标识（Palantir = api_name；W3C = IRI）。
    pub ontology_key: String,
    /// 本体标准展示名（`W3C Turtle` / `平台 payload`）。
    pub standard_label: String,
    /// 导出时刻（`YYYY-MM-DD HH:MM UTC`）。
    pub exported_at: String,
    pub class_total: u32,
    pub class_mapped: u32,
    pub property_total: u32,
    pub property_mapped: u32,
    pub relation_total: u32,
    pub relation_mapped: u32,
}

/// 类表一行。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClassRow {
    pub name: String,
    pub description: String,
    pub source_table: String,
    pub mapped: bool,
    pub note: String,
}

/// 属性表一行（映射交付主体）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropertyRow {
    pub owner_name: String,
    pub owner_desc: String,
    pub name: String,
    pub description: String,
    pub data_type: String,
    pub is_primary_key: bool,
    pub source_table: String,
    pub source_column: String,
    pub column_type: String,
    /// `是` / `否` / 空（快照缺失时留空，不伪造）。
    pub nullable: String,
    pub mapped: bool,
    pub note: String,
}

/// 关系表一行。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelationRow {
    pub name: String,
    pub description: String,
    pub source_object: String,
    pub source_property: String,
    pub target_object: String,
    pub target_property: String,
    pub cardinality: String,
    pub join_condition: String,
    pub mapped: bool,
    pub note: String,
}

/// 一次导出的全部内容。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportDataset {
    pub overview: ExportOverview,
    pub classes: Vec<ClassRow>,
    pub properties: Vec<PropertyRow>,
    pub relations: Vec<RelationRow>,
}

impl ExportDataset {
    /// 数据行总数（不含表头、不含概览页）。
    pub fn data_row_count(&self) -> usize {
        self.classes.len() + self.properties.len() + self.relations.len()
    }
}

// ════════════════════════════════════════════════════════════════
// 装配
// ════════════════════════════════════════════════════════════════

/// 装配入参（把长参数列表收进结构，避免调用处错位）。
pub struct DatasetInput<'a> {
    pub standard: &'a str,
    pub ontology_key: &'a str,
    /// 本体显示名；空则回退到标识。
    pub ontology_name: &'a str,
    pub scope: MappingExportScope,
    pub elements: &'a [MappingElementRef],
    pub mappings: &'a [ElementMapping],
    pub snapshot: Option<&'a PhysicalSchemaSnapshot>,
    /// `(对象标识, 属性标识)` → 是否主键。**必须是复合键**（见 D5）。
    pub primary_keys: &'a HashMap<(String, String), bool>,
    /// 导出时刻（已格式化）。
    pub exported_at: &'a str,
}

/// 把元素全集 + 映射行装配成导出数据集（纯函数，无 IO）。
///
/// - `elements` 是本体内容侧的元素全集（含未映射元素）
/// - `mappings` 是该本体全部映射行
/// - `snapshot` 给出时回填物理事实列（字段类型 / 可空）并重新判定核对状态
/// - `scope == Gap` 时只保留未映射元素
pub fn build_dataset(input: DatasetInput<'_>) -> ExportDataset {
    let DatasetInput {
        standard,
        ontology_key,
        ontology_name,
        scope,
        elements,
        mappings,
        snapshot,
        primary_keys,
        exported_at,
    } = input;

    // 按元素聚合映射（保持写入顺序，便于人读与 diff）
    let mut by_element: HashMap<(&str, &str), Vec<&ElementMapping>> = HashMap::new();
    for m in mappings {
        by_element
            .entry((m.element_type.as_str(), m.element_id.as_str()))
            .or_default()
            .push(m);
    }

    let mut ds = ExportDataset {
        overview: ExportOverview {
            ontology_name: ontology_display_or_key(ontology_name, ontology_key),
            ontology_key: ontology_key.to_string(),
            standard_label: standard_label(standard).to_string(),
            exported_at: exported_at.to_string(),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut emitted: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();

    for el in elements {
        let key = (el.element_type.as_str(), el.element_id.as_str());
        let rows = by_element.get(&key);
        let is_mapped = rows.map(|r| !r.is_empty()).unwrap_or(false);

        // 概览计数按「元素」口径（不是映射行数）
        let bucket = match element_kind(standard, &el.element_type) {
            ElementKind::Class => 0,
            ElementKind::Property => 1,
            ElementKind::Relation => 2,
            ElementKind::Other => continue,
        };
        match bucket {
            0 => {
                ds.overview.class_total += 1;
                if is_mapped {
                    ds.overview.class_mapped += 1;
                }
            }
            1 => {
                ds.overview.property_total += 1;
                if is_mapped {
                    ds.overview.property_mapped += 1;
                }
            }
            _ => {
                ds.overview.relation_total += 1;
                if is_mapped {
                    ds.overview.relation_mapped += 1;
                }
            }
        }

        emitted.insert(key);

        // Gap 只保留未映射元素（已映射即视为已覆盖，核对质量不进交付文档）
        if scope == MappingExportScope::Gap && is_mapped {
            continue;
        }

        match element_kind(standard, &el.element_type) {
            ElementKind::Class => push_class_rows(&mut ds.classes, el, rows),
            ElementKind::Property => {
                push_property_rows(&mut ds.properties, el, rows, snapshot, primary_keys)
            }
            ElementKind::Relation => push_relation_rows(&mut ds.relations, el, rows),
            ElementKind::Other => {}
        }
    }

    // 孤儿映射（元素已不在本体内容里）：Gap 范围输出，便于用户发现残留。
    // 全部范围也输出——静默丢弃会让用户以为映射丢了。
    for m in mappings {
        let key = (m.element_type.as_str(), m.element_id.as_str());
        if emitted.contains(&key) {
            continue;
        }
        let ghost = MappingElementRef {
            element_type: m.element_type.clone(),
            element_id: m.element_id.clone(),
            ..Default::default()
        };
        let single = vec![m];
        match element_kind(standard, &ghost.element_type) {
            ElementKind::Class => {
                let mut rows = Vec::new();
                push_class_rows(&mut rows, &ghost, Some(&single));
                for r in &mut rows {
                    append_note(&mut r.note, "元素已不在本体中");
                }
                ds.classes.extend(rows);
            }
            ElementKind::Property => {
                let mut rows = Vec::new();
                push_property_rows(&mut rows, &ghost, Some(&single), snapshot, primary_keys);
                for r in &mut rows {
                    append_note(&mut r.note, "元素已不在本体中");
                }
                ds.properties.extend(rows);
            }
            ElementKind::Relation => {
                let mut rows = Vec::new();
                push_relation_rows(&mut rows, &ghost, Some(&single));
                for r in &mut rows {
                    append_note(&mut r.note, "元素已不在本体中");
                }
                ds.relations.extend(rows);
            }
            ElementKind::Other => {}
        }
    }

    // 稳定排序：对象 → 元素名 → 物理位置（供人读，便于 diff）
    ds.classes.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.source_table.cmp(&b.source_table))
    });
    ds.properties.sort_by(|a, b| {
        a.owner_name
            .cmp(&b.owner_name)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.source_table.cmp(&b.source_table))
            .then_with(|| a.source_column.cmp(&b.source_column))
    });
    ds.relations.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.join_condition.cmp(&b.join_condition))
    });

    ds
}

fn push_class_rows(
    out: &mut Vec<ClassRow>,
    el: &MappingElementRef,
    rows: Option<&Vec<&ElementMapping>>,
) {
    let name = short_name(&el.element_id);
    let description = display_or_short(el);
    match rows {
        Some(list) if !list.is_empty() => {
            for m in list {
                out.push(ClassRow {
                    name: name.clone(),
                    description: description.clone(),
                    source_table: m.source.table_locator(),
                    mapped: true,
                    note: m.note.clone(),
                });
            }
        }
        _ => out.push(ClassRow {
            name,
            description,
            mapped: false,
            ..Default::default()
        }),
    }
}

fn push_property_rows(
    out: &mut Vec<PropertyRow>,
    el: &MappingElementRef,
    rows: Option<&Vec<&ElementMapping>>,
    snapshot: Option<&PhysicalSchemaSnapshot>,
    primary_keys: &HashMap<(String, String), bool>,
) {
    let name = short_name(&el.element_id);
    let is_pk = primary_keys
        .get(&(el.owner_id.clone(), el.element_id.clone()))
        .copied()
        .unwrap_or(false);
    let base = PropertyRow {
        // 对象名称列给业务短名（`https://x.cn/ont#Order` → `Order`），
        // 与「属性名称」列同一口径；完整 IRI 不进交付文档（规格 D3）。
        owner_name: short_name(&el.owner_id),
        owner_desc: el.owner_name.clone(),
        name,
        description: display_or_short(el),
        data_type: el.data_type.clone(),
        is_primary_key: is_pk,
        ..Default::default()
    };

    match rows {
        Some(list) if !list.is_empty() => {
            for m in list {
                // 物理事实列：有快照才回填，否则留空（不伪造）
                let (column_type, nullable) = physical_facts(m, snapshot);
                out.push(PropertyRow {
                    source_table: m.source.table_locator(),
                    source_column: m.source.column.clone(),
                    column_type,
                    nullable,
                    mapped: true,
                    note: m.note.clone(),
                    ..base.clone()
                });
            }
        }
        _ => out.push(PropertyRow {
            mapped: false,
            ..base
        }),
    }
}

fn push_relation_rows(
    out: &mut Vec<RelationRow>,
    el: &MappingElementRef,
    rows: Option<&Vec<&ElementMapping>>,
) {
    let name = short_name(&el.element_id);
    let base = RelationRow {
        name,
        description: display_or_short(el),
        // 两端对象列同样给业务短名（W3C 侧 owner_id/target_id 是完整 IRI，
        // 原样输出会让「源对象/目标对象」列出现整串命名空间——规格 D3 明确禁止）。
        source_object: short_name(&el.owner_id),
        source_property: el.endpoint_property.clone(),
        target_object: short_name(&el.target_id),
        target_property: el.target_property.clone(),
        cardinality: cardinality_label(&el.cardinality),
        ..Default::default()
    };

    match rows {
        Some(list) if !list.is_empty() => {
            for m in list {
                // 状态口径（规格 `Export status uses mapping-oriented wording`）：
                // 有映射即「已映射」，核对质量不进交付文档。唯一例外是单端映射——
                // 规格明确要求它不得标为已映射，并在连接条件列标出不完整。
                out.push(RelationRow {
                    join_condition: join_condition(m),
                    mapped: !m.is_partial(),
                    note: m.note.clone(),
                    ..base.clone()
                });
            }
        }
        _ => out.push(RelationRow {
            mapped: false,
            ..base
        }),
    }
}

/// 物理事实列（字段类型 / 可空）：有快照且能找到该列才回填。
fn physical_facts(m: &ElementMapping, snapshot: Option<&PhysicalSchemaSnapshot>) -> (String, String) {
    let Some(snap) = snapshot.filter(|s| !s.is_empty()) else {
        return (String::new(), String::new());
    };
    if m.source.column.is_empty() {
        return (String::new(), String::new());
    }
    let Some((_, table)) = snap.find_table(&m.source.catalog, &m.source.schema, &m.source.table)
    else {
        return (String::new(), String::new());
    };
    match PhysicalSchemaSnapshot::find_column(table, &m.source.column) {
        Some(c) => (
            c.data_type.clone(),
            if c.nullable { "是" } else { "否" }.to_string(),
        ),
        None => (String::new(), String::new()),
    }
}

/// 关系连接条件：两端物理位置的对应关系，单端时明确标注不完整。
fn join_condition(m: &ElementMapping) -> String {
    let src = m.source.locator();
    let tgt = m.target.locator();
    match (src.is_empty(), tgt.is_empty()) {
        (false, false) => format!("{src} = {tgt}"),
        (false, true) => format!("{src} = ?（目标端未映射，不完整）"),
        (true, false) => format!("? = {tgt}（来源端未映射，不完整）"),
        (true, true) => "两端均未映射".to_string(),
    }
}

fn append_note(note: &mut String, extra: &str) {
    if note.is_empty() {
        note.push_str(extra);
    } else {
        note.push('；');
        note.push_str(extra);
    }
}

/// 元素标识 → 业务短名：去掉命名空间前缀（`https://x.cn/ont#Policy#endDate` → `endDate`）。
///
/// 只用于**展示**；元素身份（`element_id`）仍是原文全形，不受影响。
///
/// 末尾分隔符要先剥掉：本体 IRI 常写成 `https://x.cn/ont#`（以 `#` 收尾，表示命名
/// 空间而非实体），直接 `rsplit(['#','/'])` 会取到空串——那会让「对象名称」列变空白。
pub fn short_name(id: &str) -> String {
    // 原文 token 可能带尖括号（`<https://x.cn/ont#Order>`），先归一
    let s = id.trim();
    let s = s
        .strip_prefix('<')
        .map(|rest| rest.strip_suffix('>').unwrap_or(rest))
        .unwrap_or(s);
    // 去掉末尾命名空间分隔符，再取末段
    let s = s.trim_end_matches(['#', '/']);
    s.rsplit(['#', '/']).next().unwrap_or(s).to_string()
}

/// 显示名优先，回退到短名（两者都空时为空）。
fn display_or_short(el: &MappingElementRef) -> String {
    if !el.display_name.trim().is_empty() {
        el.display_name.clone()
    } else if !el.description.trim().is_empty() {
        el.description.clone()
    } else {
        short_name(&el.element_id)
    }
}

/// 本体显示名的兜底：传入名称为空时给业务可读名，而不是整串 IRI。
///
/// W3C 侧标识常是 `https://x.cn/ont#`——直接回退成标识会让概览页「本体名称」
/// 显示一整串命名空间。与「本体标识」行（保留原值，供对账）区分开。
fn ontology_display_or_key(ontology_name: &str, ontology_key: &str) -> String {
    let name = ontology_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    let short = short_name(ontology_key);
    if short.is_empty() {
        ontology_key.to_string()
    } else {
        short
    }
}

fn standard_label(standard: &str) -> &'static str {
    match standard {
        crate::mapping::STANDARD_W3C => "W3C Turtle",
        crate::mapping::STANDARD_PALANTIR => "平台 payload",
        _ => "本体",
    }
}

fn cardinality_label(raw: &str) -> String {
    match raw {
        "ONE" => "一对一".to_string(),
        "MANY" => "一对多".to_string(),
        other => other.to_string(),
    }
}

fn status_label(mapped: bool) -> &'static str {
    if mapped {
        STATUS_MAPPED
    } else {
        STATUS_UNMAPPED
    }
}

// ════════════════════════════════════════════════════════════════
// 渲染：xlsx
// ════════════════════════════════════════════════════════════════

/// 渲染多工作表工作簿（字节交给调用方落盘）。
pub fn render_workbook(ds: &ExportDataset) -> StoreResult<Vec<u8>> {
    let mut workbook = Workbook::new();

    let header = Format::new()
        .set_bold()
        .set_background_color(rust_xlsxwriter::Color::RGB(0xEF_F2_F6))
        .set_align(FormatAlign::Center);
    let label = Format::new().set_bold();

    // ── 概览页 ──
    {
        let ws = workbook.add_worksheet();
        ws.set_name(OVERVIEW_SHEET)
            .map_err(xlsx_err)?;
        let o = &ds.overview;
        let rows: Vec<(&str, String)> = vec![
            ("本体名称", o.ontology_name.clone()),
            ("本体标识", o.ontology_key.clone()),
            ("本体标准", o.standard_label.clone()),
            ("导出时间", o.exported_at.clone()),
            ("类数量", o.class_total.to_string()),
            ("类已映射", format!("{} / {}", o.class_mapped, o.class_total)),
            (
                "属性已映射",
                format!("{} / {}", o.property_mapped, o.property_total),
            ),
            (
                "关系已映射",
                format!("{} / {}", o.relation_mapped, o.relation_total),
            ),
            (
                "映射状态说明",
                "按是否已绑定来源字段统计（未区分核对结果）".to_string(),
            ),
        ];
        for (i, (k, v)) in rows.iter().enumerate() {
            ws.write_string_with_format(i as u32, 0, *k, &label)
                .map_err(xlsx_err)?;
            ws.write_string(i as u32, 1, v).map_err(xlsx_err)?;
        }
        ws.set_column_width(0, 16.0).map_err(xlsx_err)?;
        ws.set_column_width(1, 46.0).map_err(xlsx_err)?;
    }

    // ── 类页 ──
    write_table(
        &mut workbook,
        CLASS_SHEET,
        CLASS_HEADERS,
        ds.classes.iter().map(|r| {
            vec![
                r.name.clone(),
                r.description.clone(),
                r.source_table.clone(),
                status_label(r.mapped).to_string(),
                r.note.clone(),
            ]
        }),
        &header,
    )?;

    // ── 属性页 ──
    write_table(
        &mut workbook,
        PROPERTY_SHEET,
        PROPERTY_HEADERS,
        ds.properties.iter().map(|r| {
            vec![
                r.owner_name.clone(),
                r.owner_desc.clone(),
                r.name.clone(),
                r.description.clone(),
                r.data_type.clone(),
                if r.is_primary_key { "是" } else { "否" }.to_string(),
                r.source_table.clone(),
                r.source_column.clone(),
                r.column_type.clone(),
                r.nullable.clone(),
                status_label(r.mapped).to_string(),
                r.note.clone(),
            ]
        }),
        &header,
    )?;

    // ── 关系页 ──
    write_table(
        &mut workbook,
        RELATION_SHEET,
        RELATION_HEADERS,
        ds.relations.iter().map(|r| {
            vec![
                r.name.clone(),
                r.description.clone(),
                r.source_object.clone(),
                r.source_property.clone(),
                r.target_object.clone(),
                r.target_property.clone(),
                r.cardinality.clone(),
                r.join_condition.clone(),
                status_label(r.mapped).to_string(),
                r.note.clone(),
            ]
        }),
        &header,
    )?;

    let buf = workbook.save_to_buffer().map_err(xlsx_err)?;
    Ok(buf)
}

fn write_table(
    workbook: &mut Workbook,
    sheet_name: &str,
    headers: &[&str],
    rows: impl Iterator<Item = Vec<String>>,
    header_fmt: &Format,
) -> StoreResult<()> {
    let ws = workbook.add_worksheet();
    ws.set_name(sheet_name).map_err(xlsx_err)?;
    // 表头
    for (c, h) in headers.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, header_fmt)
            .map_err(xlsx_err)?;
    }
    // 冻结表头，长表滚动时列名可见
    ws.set_freeze_panes(1, 0).map_err(xlsx_err)?;

    // 数据行 + 列宽（按显示宽度自适应）
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for (r, row) in rows.enumerate() {
        let excel_row = (r + 1) as u32;
        for (c, cell) in row.iter().enumerate() {
            if !cell.is_empty() {
                ws.write_string(excel_row, c as u16, cell).map_err(xlsx_err)?;
            }
            let w = display_width(cell);
            if let Some(slot) = widths.get_mut(c) {
                if w > *slot {
                    *slot = w;
                }
            }
        }
    }
    for (c, w) in widths.iter().enumerate() {
        // 留 2 个字符余量，上限 60 避免备注列把表撑爆
        let width = ((*w).min(60) + 2) as f64;
        ws.set_column_width(c as u16, width).map_err(xlsx_err)?;
    }
    Ok(())
}

/// 显示宽度：CJK / 全角字符按 2 计（Excel 列宽的经验单位）。
fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            let wide = (0x1100..=0x115F).contains(&cp)
                || (0x2E80..=0xA4CF).contains(&cp)
                || (0xAC00..=0xD7A3).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFE30..=0xFE6F).contains(&cp)
                || (0xFF00..=0xFF60).contains(&cp)
                || (0xFFE0..=0xFFE6).contains(&cp);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

fn xlsx_err(e: rust_xlsxwriter::XlsxError) -> StoreError {
    StoreError::InvalidInput(format!("生成工作簿失败: {e}"))
}

// ════════════════════════════════════════════════════════════════
// 渲染：Agent 侧文本预览（内部用途，非交付物）
// ════════════════════════════════════════════════════════════════

/// 预览表头（Agent 在会话里读，列数刻意压缩，和交付用 xlsx 不同）。
pub const PREVIEW_HEADERS: &[&str] = &[
    "元素类型",
    "对象",
    "名称",
    "描述",
    "来源表",
    "来源字段",
    "映射状态",
];

/// 渲染 Agent 侧扁平预览文本（制表符分隔，便于会话里读，不用于落盘交付）。
///
/// 存在理由：Agent 工具只能返回文本，无法消费二进制工作簿。它复用同一份
/// `ExportDataset`，因此与 xlsx 交付物口径一致，不会各写一套列语义。
pub fn render_preview_text(ds: &ExportDataset) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(ds.data_row_count() + 1);
    lines.push(PREVIEW_HEADERS.join("\t"));
    for r in &ds.classes {
        lines.push(
            [
                "类",
                short_name(&r.name).as_str(),
                r.name.as_str(),
                r.description.as_str(),
                r.source_table.as_str(),
                "",
                status_label(r.mapped),
            ]
            .join("\t"),
        );
    }
    for r in &ds.properties {
        lines.push(
            [
                "属性",
                format!("{}.{}", r.owner_name, r.name).as_str(),
                r.name.as_str(),
                r.description.as_str(),
                r.source_table.as_str(),
                r.source_column.as_str(),
                status_label(r.mapped),
            ]
            .join("\t"),
        );
    }
    for r in &ds.relations {
        lines.push(
            [
                "关系",
                r.name.as_str(),
                r.name.as_str(),
                r.description.as_str(),
                r.join_condition.as_str(),
                "",
                status_label(r.mapped),
            ]
            .join("\t"),
        );
    }
    lines.join("\n")
}

// ════════════════════════════════════════════════════════════════
// 文件名
// ════════════════════════════════════════════════════════════════

/// 导出文件名（纯函数）：`<本体短名>-元素映射.xlsx`。
///
/// 导出只有一种范围（全量），不再带范围后缀；W3C 的本体标识是 IRI，
/// 整串做文件名既长又难读，故先取末段。扩展名与非法字符替换规则
/// **必须**与前端 `mapping-display.ts::mappingExportFileName` 保持一致
/// （两处各需立刻拿到文件名，无法共享代码，靠测试锁一致性）。
pub fn export_file_name(ontology_display: &str) -> String {
    let raw = ontology_display.trim();
    let candidate = if raw.contains("://") {
        let no_scheme = raw
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(raw);
        let segs: Vec<&str> = no_scheme
            .split(['#', '/'])
            .filter(|s| !s.is_empty())
            .collect();
        segs.last().copied().unwrap_or(no_scheme)
    } else {
        raw
    };
    let mut safe: String = candidate
        .chars()
        .map(|c| {
            if c.is_whitespace() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    safe = safe.trim_matches('_').to_string();
    safe.truncate(80);
    let base = if safe.is_empty() {
        "ontology".to_string()
    } else {
        safe
    };
    format!("{base}-元素映射.xlsx")
}

/// 导出时刻格式化：unix ms → `YYYY-MM-DD HH:MM UTC`。
///
/// 用纯算术实现（civil-from-days 反解），不引入时间库——只为概览页一行文字，
/// 增加一个依赖不划算（原则 2）。
pub fn format_export_time(unix_ms: i64) -> String {
    let secs = unix_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02} UTC")
}

/// Howard Hinnant 的 civil_from_days（days 自 1970-01-01 起算）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{ElementMapping, MappingEndpoint, STANDARD_PALANTIR, STANDARD_W3C};

    fn el(etype: &str, id: &str, name: &str, dt: &str) -> MappingElementRef {
        MappingElementRef {
            element_type: etype.into(),
            element_id: id.into(),
            display_name: name.into(),
            data_type: dt.into(),
            ..Default::default()
        }
    }

    fn mapping(etype: &str, id: &str, table: &str, column: &str, verified: bool) -> ElementMapping {
        ElementMapping {
            id: format!("em-{id}-{column}"),
            standard: STANDARD_PALANTIR.into(),
            ontology_key: "Sales".into(),
            element_type: etype.into(),
            element_id: id.into(),
            source: MappingEndpoint {
                table: table.into(),
                column: column.into(),
                ..Default::default()
            },
            target: MappingEndpoint::default(),
            verified,
            verified_at: if verified { 1 } else { 0 },
            source_kind: "manual".into(),
            source_ref: String::new(),
            note: String::new(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn build(
        standard: &str,
        scope: MappingExportScope,
        elements: &[MappingElementRef],
        mappings: &[ElementMapping],
        pk: &HashMap<(String, String), bool>,
    ) -> ExportDataset {
        build_named(
            standard,
            "Sales",
            "销售本体",
            scope,
            elements,
            mappings,
            pk,
        )
    }

    /// 指定本体标识/名称的装配（概览页回退逻辑需要真实 IRI 形态）。
    #[allow(clippy::too_many_arguments)]
    fn build_named(
        standard: &str,
        ontology_key: &str,
        ontology_name: &str,
        scope: MappingExportScope,
        elements: &[MappingElementRef],
        mappings: &[ElementMapping],
        pk: &HashMap<(String, String), bool>,
    ) -> ExportDataset {
        build_dataset(DatasetInput {
            standard,
            ontology_key,
            ontology_name,
            scope,
            elements,
            mappings,
            snapshot: None,
            primary_keys: pk,
            exported_at: "2026-11-13 08:40 UTC",
        })
    }

    fn empty_pk() -> HashMap<(String, String), bool> {
        HashMap::new()
    }

    #[test]
    fn workbook_is_valid_zip_container() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[],
            &[],
            &empty_pk(),
        );
        let bytes = render_workbook(&ds).expect("渲染应成功");
        assert!(bytes.starts_with(b"PK\x03\x04"), "xlsx 必须是 zip 容器");
        assert!(bytes.len() > 1000, "工作簿不应为空壳");
    }

    #[test]
    fn overview_carries_context_once() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[el("object_type", "Dealership", "门店", "")],
            &[],
            &empty_pk(),
        );
        assert_eq!(ds.overview.ontology_name, "销售本体");
        assert_eq!(ds.overview.standard_label, "平台 payload");
        assert_eq!(ds.overview.class_total, 1);
        // 概览页存在（工作簿可渲染）
        let bytes = render_workbook(&ds).expect("渲染应成功");
        assert!(bytes.starts_with(b"PK\x03\x04"));
    }

    #[test]
    fn class_sheet_has_five_columns() {
        assert_eq!(CLASS_HEADERS.len(), 5);
        assert_eq!(PROPERTY_HEADERS.len(), 12);
        assert_eq!(RELATION_HEADERS.len(), 10);
    }

    #[test]
    fn unmapped_element_yields_one_empty_row() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[el("property", "amount", "金额", "DECIMAL")],
            &[],
            &empty_pk(),
        );
        assert_eq!(ds.properties.len(), 1);
        let r = &ds.properties[0];
        assert!(!r.mapped);
        assert!(r.source_table.is_empty());
        assert!(r.source_column.is_empty());
        assert!(r.column_type.is_empty(), "快照缺失时不得伪造字段类型");
        assert!(r.nullable.is_empty());
    }

    #[test]
    fn multiple_candidates_each_occupy_row() {
        let mut pk = HashMap::new();
        pk.insert(("Order".to_string(), "amount".to_string()), false);
        let mut e = el("property", "amount", "金额", "DECIMAL");
        e.owner_id = "Order".into();
        e.owner_name = "订单".into();

        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[e],
            &[
                mapping("property", "amount", "t_order", "amount", true),
                mapping("property", "amount", "t_order_bak", "amt", false),
            ],
            &pk,
        );
        assert_eq!(ds.properties.len(), 2, "两条候选必须各占一行");
        // 状态按两态口径：有映射即已映射（核对质量不进交付文档）
        assert!(ds.properties.iter().all(|r| r.mapped));
        assert_eq!(ds.overview.property_total, 1, "元素数不是行数");
        assert_eq!(ds.overview.property_mapped, 1);
    }

    #[test]
    fn gap_scope_drops_mapped_elements() {
        let elements = vec![
            el("property", "orderNo", "订单号", "STRING"),
            el("property", "amount", "金额", "DECIMAL"),
        ];
        let mappings = vec![mapping("property", "orderNo", "t_order", "order_no", true)];
        let all = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &elements,
            &mappings,
            &empty_pk(),
        );
        assert_eq!(all.properties.len(), 2);

        let gap = build(
            STANDARD_PALANTIR,
            MappingExportScope::Gap,
            &elements,
            &mappings,
            &empty_pk(),
        );
        assert_eq!(gap.properties.len(), 1);
        assert_eq!(gap.properties[0].name, "amount");
    }

    #[test]
    fn relation_carries_semantic_endpoints() {
        let mut e = el("link", "hasCustomer", "所属客户", "");
        e.owner_id = "SalesOrder".into();
        e.owner_name = "销售订单".into();
        e.target_id = "Customer".into();
        e.target_name = "客户".into();
        e.endpoint_property = "customerId".into();
        e.target_property = "customerId".into();
        e.cardinality = "MANY".into();

        let mut m = mapping("link", "hasCustomer", "t_order", "customer_id", true);
        m.target = MappingEndpoint {
            table: "t_customer".into(),
            column: "id".into(),
            ..Default::default()
        };
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[e],
            &[m],
            &empty_pk(),
        );
        let r = &ds.relations[0];
        assert_eq!(r.source_object, "SalesOrder");
        assert_eq!(r.target_object, "Customer");
        assert_eq!(r.source_property, "customerId");
        assert_eq!(r.target_property, "customerId");
        assert_eq!(r.cardinality, "一对多");
        assert_eq!(r.join_condition, "t_order.customer_id = t_customer.id");
        assert!(r.mapped);
    }

    #[test]
    fn partial_relation_is_not_marked_mapped() {
        let mut e = el("link", "hasCustomer", "所属客户", "");
        e.owner_id = "SalesOrder".into();
        e.target_id = "Customer".into();
        let m = mapping("link", "hasCustomer", "t_order", "customer_id", true);
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[e],
            &[m],
            &empty_pk(),
        );
        let r = &ds.relations[0];
        assert!(!r.mapped, "单端映射不得标为已映射");
        assert!(r.join_condition.contains("不完整"));
    }

    #[test]
    fn w3c_relation_leaves_endpoint_properties_empty() {
        let mut e = el("object_property", "https://x.cn/ont#Order#hasCustomer", "所属客户", "");
        e.owner_id = "https://x.cn/ont#Order".into();
        e.owner_name = "订单".into();
        e.target_id = "https://x.cn/ont#Customer".into();
        e.target_name = "客户".into();
        let ds = build(
            STANDARD_W3C,
            MappingExportScope::All,
            &[e],
            &[],
            &empty_pk(),
        );
        let r = &ds.relations[0];
        assert_eq!(r.name, "hasCustomer", "名称必须去掉 IRI 前缀");
        assert!(r.source_property.is_empty(), "TTL 未声明端点属性时留空");
        assert!(r.target_property.is_empty());
        assert!(r.cardinality.is_empty());
    }

    #[test]
    fn short_name_strips_iri_prefix() {
        assert_eq!(short_name("https://x.cn/ont#Policy#endDate"), "endDate");
        assert_eq!(short_name("http://ex/onto/Order"), "Order");
        assert_eq!(short_name("dealership_id"), "dealership_id");
        assert_eq!(short_name(""), "");
    }

    #[test]
    fn short_name_survives_trailing_namespace_separator() {
        // 本体 IRI 常以 `#`/`/` 收尾（命名空间形式）：旧实现取到空串，
        // 会让「对象名称」列整列空白（导出实测踩到）。
        assert_eq!(short_name("https://x.cn/ont#"), "ont");
        assert_eq!(short_name("https://x.cn/ont/"), "ont");
        assert_eq!(short_name("<https://x.cn/ont#Order>"), "Order");
        // 全分隔符/空串不退化成 panic 或残串
        assert_eq!(short_name("###"), "");
        assert_eq!(short_name("/"), "");
    }

    #[test]
    fn property_object_name_drops_iri_prefix() {
        // W3C 的属性 owner_id 是完整类 IRI，原样输出会让「对象名称」列出现整串
        // 命名空间；规格要求去前缀只留业务短名，中文描述另占一列。
        let mut e = el(
            "datatype_property",
            "https://x.cn/ont#Dealership#dealerId",
            "门店编号",
            "STRING",
        );
        e.owner_id = "https://x.cn/ont#Dealership".into();
        e.owner_name = "门店".into();
        let ds = build(
            STANDARD_W3C,
            MappingExportScope::All,
            &[e],
            &[],
            &empty_pk(),
        );
        let r = &ds.properties[0];
        assert_eq!(r.owner_name, "Dealership", "对象名称必须去掉 IRI 前缀");
        assert_eq!(r.owner_desc, "门店", "中文描述仍完整保留");
        assert_eq!(r.name, "dealerId");
    }

    #[test]
    fn relation_endpoints_drop_iri_prefix() {
        // 关系页的源/目标对象列同样是完整 IRI，必须去前缀（双 # 本体最容易踩）。
        let mut e = el(
            "object_property",
            "https://www.example.com/NEVDealershipRetailOntology#ChatRecord#mentions",
            "提及",
            "",
        );
        e.owner_id = "https://www.example.com/NEVDealershipRetailOntology#ChatRecord".into();
        e.owner_name = "微信聊天记录".into();
        e.target_id = "https://www.example.com/NEVDealershipRetailOntology#Dealership".into();
        e.target_name = "门店".into();
        let ds = build(
            STANDARD_W3C,
            MappingExportScope::All,
            &[e],
            &[],
            &empty_pk(),
        );
        let r = &ds.relations[0];
        assert_eq!(r.source_object, "ChatRecord", "源对象必须去掉 IRI 前缀");
        assert_eq!(r.target_object, "Dealership", "目标对象必须去掉 IRI 前缀");
        assert!(!r.source_object.contains("://"));
        assert!(!r.target_object.contains("://"));
    }

    #[test]
    fn overview_name_falls_back_to_short_name_not_full_iri() {
        // 名称为空时旧实现回退成整串标识（W3C = 完整 IRI），概览页会显示
        // 一大串命名空间；应回退成业务短名，而「本体标识」行保留原值供对账。
        let ds = build_named(
            STANDARD_W3C,
            "https://www.example.com/NEVDealershipRetailOntology#",
            "",
            MappingExportScope::All,
            &[],
            &[],
            &empty_pk(),
        );
        assert_eq!(ds.overview.ontology_name, "NEVDealershipRetailOntology");
        assert_eq!(
            ds.overview.ontology_key,
            "https://www.example.com/NEVDealershipRetailOntology#",
            "标识行必须保留原值",
        );
    }

    #[test]
    fn overview_name_prefers_caller_name_over_key() {
        // 创建向导填的「名称」由调用方传入（W3C 取自 owl:Ontology 的 rdfs:label），
        // 概览页必须优先用它，不能被标识覆盖。
        let ds = build_named(
            STANDARD_W3C,
            "https://www.example.com/NEVDealershipRetailOntology#",
            "新能源车零售本体",
            MappingExportScope::All,
            &[],
            &[],
            &empty_pk(),
        );
        assert_eq!(ds.overview.ontology_name, "新能源车零售本体");
    }

    #[test]
    fn primary_key_is_scoped_by_owner() {
        // 两个对象都有 name 属性，只有 Customer 的是主键
        let mut pk = HashMap::new();
        pk.insert(("Customer".to_string(), "name".to_string()), true);
        pk.insert(("Product".to_string(), "name".to_string()), false);

        let mut cust = el("property", "name", "客户名称", "STRING");
        cust.owner_id = "Customer".into();
        let mut prod = el("property", "name", "商品名称", "STRING");
        prod.owner_id = "Product".into();

        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[cust, prod],
            &[],
            &pk,
        );
        let by_owner = |o: &str| {
            ds.properties
                .iter()
                .find(|r| r.owner_name == o)
                .map(|r| r.is_primary_key)
        };
        assert_eq!(by_owner("Customer"), Some(true));
        assert_eq!(by_owner("Product"), Some(false), "同名属性不得串台");
    }

    #[test]
    fn orphan_mapping_is_exported_with_note() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[],
            &[mapping("property", "ghost", "t_x", "c", true)],
            &empty_pk(),
        );
        assert_eq!(ds.properties.len(), 1);
        assert!(ds.properties[0].note.contains("元素已不在本体中"));
    }

    #[test]
    fn export_file_name_uses_xlsx() {
        assert_eq!(export_file_name("Sales"), "Sales-元素映射.xlsx");
        assert_eq!(export_file_name("我的 本体"), "我的_本体-元素映射.xlsx");
        assert_eq!(
            export_file_name("http://ex/onto#Supply"),
            "Supply-元素映射.xlsx"
        );
        assert_eq!(export_file_name(""), "ontology-元素映射.xlsx");
    }

    #[test]
    fn export_time_is_formatted() {
        assert_eq!(format_export_time(0), "1970-01-01 00:00 UTC");
        // 2026-11-13 08:40 UTC = 1794559200s
        assert_eq!(
            format_export_time(1_794_559_200_000),
            "2026-11-13 08:40 UTC"
        );
    }

    #[test]
    fn display_width_counts_cjk_as_two() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("门店"), 4);
        assert_eq!(display_width("门店id"), 6);
    }

    // ── 回读校验：用 calamine 解析自己生成的字节，确认 Excel 真能读出内容 ──
    // 只断言数据集内容是不够的——xlsx 是 XML in zip，写错了照样能编译通过。

    fn read_back(bytes: &[u8]) -> calamine::Xlsx<std::io::Cursor<&[u8]>> {
        use calamine::Reader;
        let cursor = std::io::Cursor::new(bytes);
        let mut wb = calamine::Xlsx::new(cursor).expect("生成的字节必须是合法 xlsx");
        let _ = wb.worksheet_range_at(0).expect("至少有第一个 sheet");
        wb
    }

    fn sheet_names(bytes: &[u8]) -> Vec<String> {
        use calamine::Reader;
        let cursor = std::io::Cursor::new(bytes);
        calamine::Xlsx::new(cursor)
            .expect("合法 xlsx")
            .sheet_names()
            .to_vec()
    }

    fn cell_text(bytes: &[u8], sheet: &str, row: u32, col: u32) -> String {
        use calamine::Reader;
        let cursor = std::io::Cursor::new(bytes);
        let mut wb = calamine::Xlsx::new(cursor).expect("合法 xlsx");
        let range = wb.worksheet_range(sheet).expect("sheet 应存在");
        range
            .get_value((row, col))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn workbook_has_four_sheets_in_order() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[],
            &[],
            &empty_pk(),
        );
        let bytes = render_workbook(&ds).unwrap();
        let names = sheet_names(&bytes);
        assert_eq!(
            names,
            vec![
                OVERVIEW_SHEET.to_string(),
                CLASS_SHEET.to_string(),
                PROPERTY_SHEET.to_string(),
                RELATION_SHEET.to_string()
            ],
            "sheet 名与顺序是对外契约"
        );
        // 冒烟：首个 sheet 可被解析
        let _ = read_back(&bytes);
    }

    #[test]
    fn sheet_headers_are_written() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[],
            &[],
            &empty_pk(),
        );
        let bytes = render_workbook(&ds).unwrap();
        // 表头必须在第 0 行，列序与常量一致
        for (c, h) in CLASS_HEADERS.iter().enumerate() {
            assert_eq!(
                cell_text(&bytes, CLASS_SHEET, 0, c as u32),
                *h,
                "类页第 {c} 列表头"
            );
        }
        for (c, h) in PROPERTY_HEADERS.iter().enumerate() {
            assert_eq!(
                cell_text(&bytes, PROPERTY_SHEET, 0, c as u32),
                *h,
                "属性页第 {c} 列表头"
            );
        }
        for (c, h) in RELATION_HEADERS.iter().enumerate() {
            assert_eq!(
                cell_text(&bytes, RELATION_SHEET, 0, c as u32),
                *h,
                "关系页第 {c} 列表头"
            );
        }
    }

    #[test]
    fn chinese_content_survives_round_trip() {
        let mut prop = el("property", "dealership_id", "门店ID", "STRING");
        prop.owner_id = "Dealership".into();
        prop.owner_name = "门店".into();
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[prop],
            &[mapping(
                "property",
                "dealership_id",
                "mhero_ods.store",
                "dealer_code",
                true,
            )],
            &empty_pk(),
        );
        let bytes = render_workbook(&ds).unwrap();
        // 属性页：0=对象名称 1=对象描述 3=属性描述 6=来源表 7=来源字段 10=映射状态
        assert_eq!(cell_text(&bytes, PROPERTY_SHEET, 1, 0), "Dealership");
        assert_eq!(cell_text(&bytes, PROPERTY_SHEET, 1, 1), "门店");
        assert_eq!(cell_text(&bytes, PROPERTY_SHEET, 1, 3), "门店ID");
        assert_eq!(cell_text(&bytes, PROPERTY_SHEET, 1, 6), "mhero_ods.store");
        assert_eq!(cell_text(&bytes, PROPERTY_SHEET, 1, 7), "dealer_code");
        assert_eq!(cell_text(&bytes, PROPERTY_SHEET, 1, 10), STATUS_MAPPED);
    }

    #[test]
    fn overview_sheet_lists_context_fields() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[el("object_type", "Dealership", "门店", "")],
            &[],
            &empty_pk(),
        );
        let bytes = render_workbook(&ds).unwrap();
        assert_eq!(cell_text(&bytes, OVERVIEW_SHEET, 0, 0), "本体名称");
        assert_eq!(cell_text(&bytes, OVERVIEW_SHEET, 0, 1), "销售本体");
        assert_eq!(cell_text(&bytes, OVERVIEW_SHEET, 2, 0), "本体标准");
        assert_eq!(cell_text(&bytes, OVERVIEW_SHEET, 2, 1), "平台 payload");
        // 上下文只在这里出现，数据表不含这两列（由表头断言保证）
    }

    #[test]
    fn relation_sheet_has_no_source_table_columns() {
        // 规格：关系页不得出现独立的来源表/来源字段/目标表/目标字段列
        for h in RELATION_HEADERS {
            assert_ne!(*h, "来源表");
            assert_ne!(*h, "来源字段");
            assert_ne!(*h, "目标表");
            assert_ne!(*h, "目标字段");
        }
        assert!(RELATION_HEADERS.contains(&"连接条件"));
    }

    #[test]
    fn physical_facts_filled_when_table_given_as_dotted_string() {
        // 回归：写入侧 `manual("schema.table", column)` 会把整串塞进 table 字段，
        // 若快照查找不做拆分，物理事实列会静默全空（用户看到的是一张空白表）。
        let mut prop = el("property", "orderNo", "订单号", "STRING");
        prop.owner_id = "Order".into();
        // table 字段里是整串 `mhero_ods.sales_order`，schema 留空（真实写入侧形态）
        let m = mapping("property", "orderNo", "mhero_ods.sales_order", "order_no", true);
        let ds = build_dataset(DatasetInput {
            standard: STANDARD_PALANTIR,
            ontology_key: "Sales",
            ontology_name: "销售本体",
            scope: MappingExportScope::All,
            elements: &[prop],
            mappings: &[m],
            snapshot: Some(&dotted_snapshot()),
            primary_keys: &empty_pk(),
            exported_at: "2026-11-13 08:40 UTC",
        });
        let r = &ds.properties[0];
        assert_eq!(r.column_type, "varchar(64)", "字段类型应回填");
        assert_eq!(r.nullable, "否", "可空应回填");
    }

    #[test]
    fn relation_marked_mapped_even_if_not_verified() {
        // 规格：已保存但未与数据源核对 → 已映射。核对质量不进交付文档。
        let mut e = el("link", "hasCustomer", "所属客户", "");
        e.owner_id = "SalesOrder".into();
        e.target_id = "Customer".into();
        let mut m = mapping("link", "hasCustomer", "t_order", "customer_id", false);
        m.target = MappingEndpoint {
            table: "t_customer".into(),
            column: "id".into(),
            ..Default::default()
        };
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[e],
            &[m],
            &empty_pk(),
        );
        assert!(
            ds.relations[0].mapped,
            "两端都有映射即已映射，未核对不改变交付口径"
        );
    }

    fn dotted_snapshot() -> crate::mapping_verify::PhysicalSchemaSnapshot {
        use crate::mapping_verify::{PhysicalColumn, PhysicalSource, PhysicalTable};
        crate::mapping_verify::PhysicalSchemaSnapshot {
            sources: vec![PhysicalSource {
                name: "src".into(),
                kind: "federation".into(),
                tables: vec![PhysicalTable {
                    catalog: String::new(),
                    schema: "mhero_ods".into(),
                    name: "sales_order".into(),
                    columns: vec![PhysicalColumn {
                        name: "order_no".into(),
                        data_type: "varchar(64)".into(),
                        nullable: false,
                    }],
                }],
            }],
        }
    }

    #[test]
    fn preview_text_uses_single_header() {
        let ds = build(
            STANDARD_PALANTIR,
            MappingExportScope::All,
            &[el("property", "amount", "金额", "DECIMAL")],
            &[],
            &empty_pk(),
        );
        let text = render_preview_text(&ds);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], PREVIEW_HEADERS.join("\t"));
        assert_eq!(lines.len(), 2, "1 行表头 + 1 行数据");
        assert!(lines[1].contains("金额"));
    }
}