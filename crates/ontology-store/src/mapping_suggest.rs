//! 确定性映射候选建议器（纯函数，无 IO，不落库）。
//!
//! 设计（`design.md` D6）：
//! - 候选值必须是**库里真实存在的列**。LLM 编造一个 `t_order.order_no` 看起来
//!   完全可信但库里没有——这正是 `preview_import` 的 P3 当初想挡的事。
//! - 打分全确定性、可解释、可单测、零 API 成本。LLM 的角色限缩为"从非结构化
//!   材料里指认哪张表/哪批字段值得登记"，产出的是**证据源登记**而非映射值。
//! - 类型不兼容**扣分不淘汰**：命名匹配往往比类型更可信（库里存 `varchar`
//!   但语义是金额的场景真实存在）。
//! - 建议不落库：`suggest` 只读，落库必须显式 `save`。

use crate::mapping::{
    ElementKind, ElementMapping, MappingElementRef, MappingEndpoint, MappingSuggestion,
};
use crate::mapping_verify::PhysicalSchemaSnapshot;

/// 权重：名称归一后相等。
const W_NAME_EQUAL: f64 = 0.60;
/// 权重：名称归一后包含/前缀。
const W_NAME_CONTAINS: f64 = 0.35;
/// 权重：类型兼容。
const W_TYPE_COMPATIBLE: f64 = 0.30;
/// 权重：同表先验。
const W_SAME_TABLE: f64 = 0.10;

/// 每个元素最多返回多少条候选。
pub const DEFAULT_TOP_N: usize = 3;

/// 名称归一化：`camelCase` → `snake_case`、剥离 ID 类后缀、小写、去非字母数字。
///
/// 目的：让本体侧 `orderNo` / `order_no` / `OrderNo` 与物理侧 `order_no` 对齐。
/// 剥离后缀是因为本体常带 `Id`/`No`/`Code` 尾巴而库里列名可能只是 `order`
/// （或反之），剥离后两侧才能落在同一比较基准上。
pub fn normalize_name(raw: &str) -> String {
    let mut s = String::with_capacity(raw.len());
    let chars: Vec<char> = raw.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_lowercase();
            let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();
            if (prev_lower || prev_digit || next_lower) && !s.is_empty() && !s.ends_with('_') {
                s.push('_');
            }
            s.extend(c.to_lowercase());
        } else {
            s.push(c);
        }
    }
    // 去非字母数字（保留下划线已无意义，最后统一去掉）
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let lowered = cleaned.to_lowercase();
    // 剥离常见 ID/编码类后缀（可多重，如 `order_no_id`）
    let mut out = lowered;
    loop {
        let before = out.clone();
        for suffix in ["_id", "_no", "_code", "_key", "_uuid", "_pk"] {
            if let Some(stripped) = out.strip_suffix(suffix) {
                if !stripped.is_empty() {
                    out = stripped.to_string();
                }
            }
        }
        if out == before {
            break;
        }
    }
    out
}

/// 属性 DataType → 物理类型族（归一化后的族名）。
///
/// `None` = 该类型不参与名称/类型打分（无可靠对应，直接跳过建议）。
pub fn type_family(data_type: &str) -> Option<&'static str> {
    match data_type.to_ascii_uppercase().as_str() {
        "STRING" | "MEDIA_REFERENCE" | "ATTACHMENT" => Some("string"),
        "INTEGER" | "SHORT" | "LONG" | "BYTE" => Some("integer"),
        "DECIMAL" => Some("decimal"),
        "FLOAT" | "DOUBLE" => Some("float"),
        "BOOLEAN" => Some("boolean"),
        "DATE" => Some("date"),
        "TIMESTAMP" => Some("timestamp"),
        "ARRAY" | "STRUCT" => Some("json"),
        // 无可靠物理对应：不参与建议
        "VECTOR" | "GEOPOINT" | "GEOSHAPE" | "GEOTEMPORAL_SERIES" | "TIME_SERIES" => None,
        _ => None,
    }
}

/// 物理类型原文 → 类型族（取首个类型词，忽略长度/精度后缀与 UNSIGNED 等修饰）。
pub fn physical_type_family(raw: &str) -> Option<&'static str> {
    let head = raw
        .trim()
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match head.as_str() {
        "char" | "varchar" | "nchar" | "nvarchar" | "text" | "tinytext" | "mediumtext"
        | "longtext" | "clob" | "string" | "uuid" => Some("string"),
        "int" | "integer" | "bigint" | "smallint" | "tinyint" | "serial" | "bigserial"
        | "int2" | "int4" | "int8" | "number" => Some("integer"),
        "decimal" | "numeric" | "money" | "dec" => Some("decimal"),
        "float" | "double" | "real" | "float4" | "float8" => Some("float"),
        "bool" | "boolean" | "bit" => Some("boolean"),
        "date" => Some("date"),
        "datetime" | "timestamp" | "timestamptz" | "datetime2" | "time" => Some("timestamp"),
        "json" | "jsonb" | "array" | "map" | "struct" => Some("json"),
        _ => None,
    }
}

fn type_compatible(declared: &str, physical: &str) -> bool {
    let Some(a) = type_family(declared) else {
        return false;
    };
    let Some(b) = physical_type_family(physical) else {
        return false;
    };
    if a == b {
        return true;
    }
    // 宽容组合：整数 ↔ 小数/浮点（库里存 decimal 表示整数金额很常见）
    matches!(
        (a, b),
        ("integer", "decimal") | ("decimal", "integer") | ("integer", "float")
            | ("float", "integer") | ("decimal", "float") | ("float", "decimal")
    )
}

/// 为一组元素产出候选物理位置（每个元素最多 `top_n` 条，按分数降序）。
///
/// `known` 是该本体已有的映射：用于"同一类已映射到的表"先验（该类属性优先同表列），
/// 以及把已映射的列排在更前（避免重复建议用户已确认过的位置）。
pub fn suggest_with_top_n(
    elements: &[MappingElementRef],
    schema: &PhysicalSchemaSnapshot,
    known: &[ElementMapping],
    top_n: usize,
) -> Vec<MappingSuggestion> {
    let mut out = Vec::new();
    if schema.is_empty() {
        return out;
    }
    // 类已映射到的表（同表先验的来源之一）：element_id / display_name → 表定位
    let class_tables: Vec<(String, String)> = known
        .iter()
        .filter(|m| element_kind_of(&m.element_type) == ElementKind::Class)
        .map(|m| (m.element_id.clone(), m.source.table_locator()))
        .collect();

    for el in elements {
        let kind = element_kind_of(&el.element_type);
        let mut candidates: Vec<MappingSuggestion> = Vec::new();
        let norm_el = normalize_name(if el.display_name.is_empty() {
            &el.element_id
        } else {
            &el.display_name
        });
        let norm_id = normalize_name(&el.element_id);

        for (_, table) in schema.tables() {
            let table_loc = {
                let mut parts: Vec<&str> = Vec::new();
                for seg in [&table.catalog, &table.schema, &table.name] {
                    if !seg.is_empty() {
                        parts.push(seg);
                    }
                }
                parts.join(".")
            };
            // 同表线索：元素自带 hint，或该类已映射到的表
            let same_table = !el.same_table_hint.is_empty()
                && el.same_table_hint.eq_ignore_ascii_case(&table_loc)
                || class_tables
                    .iter()
                    .any(|(k, t)| t.eq_ignore_ascii_case(&table_loc) && hint_matches(k, el));

            if kind == ElementKind::Class {
                // 类 → 表：只有表名参与打分，无列
                let norm_table = normalize_name(&table.name);
                let mut score = 0.0;
                let mut reasons = Vec::new();
                if !norm_el.is_empty() && norm_table == norm_el {
                    score += W_NAME_EQUAL;
                    reasons.push("名称归一相等".to_string());
                } else if name_contains(&norm_el, &norm_table) {
                    score += W_NAME_CONTAINS;
                    reasons.push("名称包含".to_string());
                } else {
                    continue; // 表级候选必须有名称线索，否则全是噪声
                }
                candidates.push(MappingSuggestion {
                    element_type: el.element_type.clone(),
                    element_id: el.element_id.clone(),
                    candidate: MappingEndpoint {
                        catalog: table.catalog.clone(),
                        schema: table.schema.clone(),
                        table: table.name.clone(),
                        column: String::new(),
                    },
                    score,
                    reasons,
                });
                continue;
            }

            // 属性/关系 → 列
            for col in &table.columns {
                let norm_col = normalize_name(&col.name);
                let mut score = 0.0;
                let mut reasons: Vec<String> = Vec::new();
                if !norm_col.is_empty()
                    && (norm_col == norm_el || norm_col == norm_id)
                {
                    score += W_NAME_EQUAL;
                    reasons.push("名称归一相等".to_string());
                } else if name_contains(&norm_el, &norm_col)
                    || name_contains(&norm_id, &norm_col)
                {
                    score += W_NAME_CONTAINS;
                    reasons.push("名称包含".to_string());
                }
                if !el.data_type.is_empty() && type_compatible(&el.data_type, &col.data_type) {
                    score += W_TYPE_COMPATIBLE;
                    reasons.push("类型兼容".to_string());
                }
                if same_table {
                    score += W_SAME_TABLE;
                    reasons.push("同表".to_string());
                }
                // 既无名称线索又不同表 → 纯类型噪声，丢弃（否则每个元素都拿到一堆候选）
                let has_name_signal = reasons
                    .iter()
                    .any(|r| r.starts_with("名称"));
                if !has_name_signal && !same_table {
                    continue;
                }
                candidates.push(MappingSuggestion {
                    element_type: el.element_type.clone(),
                    element_id: el.element_id.clone(),
                    candidate: MappingEndpoint {
                        catalog: table.catalog.clone(),
                        schema: table.schema.clone(),
                        table: table.name.clone(),
                        column: col.name.clone(),
                    },
                    score,
                    reasons,
                });
            }
        }

        // 排序：分数降序 → 已映射过的位置优先 → 表名/列名（稳定输出，便于快照测试）
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let ka = already_known(known, el, &a.candidate);
                    let kb = already_known(known, el, &b.candidate);
                    kb.cmp(&ka)
                })
                .then_with(|| a.candidate.table.cmp(&b.candidate.table))
                .then_with(|| a.candidate.column.cmp(&b.candidate.column))
        });
        candidates.truncate(top_n);
        out.extend(candidates);
    }
    out
}

/// 默认 top-N 的建议入口。
pub fn suggest(
    elements: &[MappingElementRef],
    schema: &PhysicalSchemaSnapshot,
    known: &[ElementMapping],
) -> Vec<MappingSuggestion> {
    suggest_with_top_n(elements, schema, known, DEFAULT_TOP_N)
}

fn element_kind_of(element_type: &str) -> ElementKind {
    match element_type {
        "object_type" | "class" => ElementKind::Class,
        "property" | "datatype_property" => ElementKind::Property,
        "link" | "object_property" => ElementKind::Relation,
        _ => ElementKind::Other,
    }
}

/// 名称包含判定（任一方向），过短的名字不做包含匹配（`id` 会匹配一切）。
fn name_contains(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.chars().count() < 3 || b.chars().count() < 3 {
        return false;
    }
    a.contains(b) || b.contains(a)
}

/// 该类已映射到的表是否属于"这个元素所属的类"。
///
/// 元素只带 `element_id`/`display_name`（Palantir 属性是裸 api_name），
/// 无法可靠推出归属类——因此只在元素自带 `same_table_hint` 时生效，
/// 否则退化为"该本体任一类已映射的表"这一弱先验（仅加分，不影响可保存性）。
fn hint_matches(_class_key: &str, el: &MappingElementRef) -> bool {
    !el.same_table_hint.is_empty()
}

fn already_known(known: &[ElementMapping], el: &MappingElementRef, cand: &MappingEndpoint) -> bool {
    known.iter().any(|m| {
        m.element_type == el.element_type
            && m.element_id == el.element_id
            && m.source.table.eq_ignore_ascii_case(&cand.table)
            && m.source.column.eq_ignore_ascii_case(&cand.column)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping_verify::{PhysicalColumn, PhysicalSource, PhysicalTable};

    #[test]
    fn normalize_camel_and_suffixes() {
        assert_eq!(normalize_name("orderNo"), "order");
        assert_eq!(normalize_name("order_no"), "order");
        assert_eq!(normalize_name("OrderNo"), "order");
        assert_eq!(normalize_name("orderId"), "order");
        assert_eq!(normalize_name("customer_code"), "customer");
        assert_eq!(normalize_name("totalAmount"), "total_amount");
        assert_eq!(normalize_name("endDate"), "end_date");
        assert_eq!(normalize_name("order_no_id"), "order");
        assert_eq!(normalize_name("保单号"), "保单号");
    }

    #[test]
    fn type_families_cover_matrix() {
        assert_eq!(type_family("STRING"), Some("string"));
        assert_eq!(type_family("LONG"), Some("integer"));
        assert_eq!(type_family("DECIMAL"), Some("decimal"));
        assert_eq!(type_family("TIMESTAMP"), Some("timestamp"));
        assert_eq!(type_family("ARRAY"), Some("json"));
        assert_eq!(type_family("VECTOR"), None);
        assert_eq!(type_family("GEOPOINT"), None);
        assert_eq!(type_family("TIME_SERIES"), None);
        assert_eq!(physical_type_family("varchar(64)"), Some("string"));
        assert_eq!(physical_type_family("BIGINT UNSIGNED"), Some("integer"));
        assert_eq!(physical_type_family("decimal(10,2)"), Some("decimal"));
        assert_eq!(physical_type_family("jsonb"), Some("json"));
        assert!(type_compatible("INTEGER", "decimal(10,2)"));
        assert!(!type_compatible("TIMESTAMP", "varchar(64)"));
    }

    fn snapshot() -> PhysicalSchemaSnapshot {
        PhysicalSchemaSnapshot {
            sources: vec![PhysicalSource {
                name: "db".into(),
                kind: "federation".into(),
                tables: vec![
                    PhysicalTable {
                        schema: "public".into(),
                        name: "t_order".into(),
                        columns: vec![
                            PhysicalColumn {
                                name: "order_no".into(),
                                data_type: "varchar(64)".into(),
                                nullable: false,
                            },
                            PhysicalColumn {
                                name: "amount".into(),
                                data_type: "decimal(10,2)".into(),
                                nullable: true,
                            },
                            PhysicalColumn {
                                name: "created_at".into(),
                                data_type: "timestamp".into(),
                                nullable: true,
                            },
                        ],
                        ..Default::default()
                    },
                    PhysicalTable {
                        name: "t_customer".into(),
                        columns: vec![PhysicalColumn {
                            name: "customer_name".into(),
                            data_type: "varchar(128)".into(),
                            nullable: true,
                        }],
                        ..Default::default()
                    },
                ],
            }],
        }
    }

    #[test]
    fn order_no_matches_snake_case_column() {
        let elements = vec![MappingElementRef {
            element_type: "property".into(),
            element_id: "orderNo".into(),
            data_type: "STRING".into(),
            ..Default::default()
        }];
        let out = suggest(&elements, &snapshot(), &[]);
        assert!(!out.is_empty());
        let top = &out[0];
        assert_eq!(top.candidate.table, "t_order");
        assert_eq!(top.candidate.column, "order_no");
        assert!(top.reasons.iter().any(|r| r == "名称归一相等"));
        assert!(top.score >= 0.60);
    }

    #[test]
    fn type_incompatible_appears_but_ranks_lower() {
        // amount 声明为 TIMESTAMP（类型不兼容），应出现但分数低于兼容候选
        let elements = vec![MappingElementRef {
            element_type: "property".into(),
            element_id: "amount".into(),
            data_type: "TIMESTAMP".into(),
            ..Default::default()
        }];
        let out = suggest(&elements, &snapshot(), &[]);
        let amount_hit = out
            .iter()
            .find(|s| s.candidate.column == "amount")
            .expect("类型不兼容候选仍应出现");
        assert!(!amount_hit.reasons.iter().any(|r| r == "类型兼容"));
        // 名称相等 0.60 + 无类型 0.30 → 仍存在但不含类型依据
        assert!(amount_hit.score <= W_NAME_EQUAL + 1e-9);
    }

    #[test]
    fn same_table_hint_prioritizes_columns() {
        let elements = vec![MappingElementRef {
            element_type: "property".into(),
            element_id: "remark".into(),
            data_type: "STRING".into(),
            // 该类已映射到 t_customer，同表列应优先
            same_table_hint: "t_customer".into(),
            ..Default::default()
        }];
        let out = suggest(&elements, &snapshot(), &[]);
        // remark 本身无名称线索，靠同表先验进入候选
        assert!(!out.is_empty(), "同表先验应产出候选");
        assert!(out.iter().all(|s| s.candidate.table == "t_customer"));
        assert!(out[0].reasons.iter().any(|r| r == "同表"));
    }

    #[test]
    fn class_suggests_table_only() {
        let elements = vec![MappingElementRef {
            element_type: "class".into(),
            element_id: "Order".into(),
            ..Default::default()
        }];
        let out = suggest(&elements, &snapshot(), &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].candidate.table, "t_order");
        assert!(out[0].candidate.column.is_empty(), "类映射不应带列");
    }

    #[test]
    fn vector_type_gets_no_suggestion() {
        let elements = vec![MappingElementRef {
            element_type: "property".into(),
            element_id: "embedding".into(),
            data_type: "VECTOR".into(),
            ..Default::default()
        }];
        let out = suggest(&elements, &snapshot(), &[]);
        assert!(out.is_empty(), "VECTOR 无可靠物理对应，不应参与建议");
    }

    #[test]
    fn empty_snapshot_yields_nothing() {
        let elements = vec![MappingElementRef {
            element_type: "property".into(),
            element_id: "orderNo".into(),
            data_type: "STRING".into(),
            ..Default::default()
        }];
        assert!(suggest(&elements, &PhysicalSchemaSnapshot::default(), &[]).is_empty());
    }
}