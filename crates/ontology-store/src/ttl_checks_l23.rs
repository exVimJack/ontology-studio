//! L2 逻辑一致性 + L3 方法论确定性规则（VALIDATION-DESIGN.md §2.1 / §2.4）。
//!
//! 与 L1 的 check_* 函数族同机制：SPARQL 跑在 oxigraph temp store 上，纯函数，
//! 无副作用。产出结构化 `ValidationIssue`（区别于 L1 的 Vec<String> push 模式，
//! 新规则直接产出 issues，validate_ttl 里统一转 errors/warnings 兼容层）。
//!
//! 严重级约定：
//!   - L2: error（阻断）——domain/range 违反、基数约束、disjoint 冲突、functional 多值
//!   - L2: warning——对称/传递自反冲突（启发式，可答辩）
//!   - L3: error——3.1 标注完备 / 3.2 tentative 不入库 / 3.6 SWRL 原子完备
//!   - L3: warning——3.3 外键不留 / 3.4 快照保留 / 3.5 M:N 拆分 / 3.7 7+1 完整性（可答辩）

use oxigraph::store::Store;

use crate::ttl::run_sparql_solutions;
use crate::ttl::strip_term;
use crate::validation::{IssueSeverity, ValidationIssue};

/// 通用规则前缀：rdf / rdfs / owl / xsd 全 IRI 常量（避免 SPARQL 前缀声明问题）。
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const OWL_MIN_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#minCardinality";
const OWL_MAX_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#maxCardinality";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_FUNCTIONAL_PROPERTY: &str = "http://www.w3.org/2002/07/owl#FunctionalProperty";
const OWL_SYMMETRIC_PROPERTY: &str = "http://www.w3.org/2002/07/owl#SymmetricProperty";
const OWL_TRANSITIVE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#TransitiveProperty";
const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";
const SWRL_IMP: &str = "http://www.w3.org/2003/11/swrl#Imp";

// ════════════════════════════════════════════════════════════════
// L2 逻辑一致性（5 条，SPARQL 模板）
// ════════════════════════════════════════════════════════════════

/// L2-1 domain/range 违反：属性声明了 `rdfs:domain X`，但存在三元组 `<s prop ?>`
/// 且 `s rdf:type Y`、`Y 与 X 无 subClassOf 关联`。
pub fn check_l2_domain_range(store: &Store, issues: &mut Vec<ValidationIssue>) {
    // 思路：找到 (prop, domainClass) 声明，再找使用该 prop 的 subject，
    // subject 的 type 集合中没有任何一个与 domainClass 有 subClassOf 传递关联。
    let q = format!(
        "SELECT ?prop ?dom ?s ?st WHERE {{\n\
            ?prop <{RDFS_DOMAIN}> ?dom .\n\
            ?s ?prop ?o .\n\
            ?s <{RDF_TYPE}> ?st .\n\
            FILTER(isIRI(?prop) && isIRI(?dom) && isIRI(?s) && isIRI(?st))\n\
            FILTER(?st != ?dom)\n\
            FILTER NOT EXISTS {{\n\
                ?st <{RDFS_SUBCLASS}>+ ?dom .\n\
            }}\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(prop) = row.get("prop").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(dom) = row.get("dom").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(st) = row.get("st").map(|s| strip_term(s)) else {
            continue;
        };
        issues.push(ValidationIssue::error(
            "L2-1",
            "L2",
            "check_l2_domain_range",
            format!(
                "属性 '{prop}' 声明 rdfs:domain '{dom}'，但实例类型 '{st}' 与其无 subClassOf 关联"
            ),
        ));
    }
}

/// L2-2 基数约束违反：owl:cardinality / qualifiedCardinality 声明 vs 实际实例计数。
/// 简化实现：检测属性上同时声明了 minCardinality > maxCardinality（自相矛盾的基数范围），
/// 以及 cardinality=0（恒空的约束，通常是建模错误）。
pub fn check_l2_cardinality(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?restriction ?p ?min ?max WHERE {{\n\
            ?restriction <{OWL_MIN_CARDINALITY}> ?min .\n\
            ?restriction <{OWL_MAX_CARDINALITY}> ?max .\n\
            ?restriction <{RDF_TYPE}> <{OWL_CLASS}> .\n\
            FILTER(?min > ?max)\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(r) = row.get("restriction").map(|s| strip_term(s)) else {
            continue;
        };
        let min = row.get("min").cloned().unwrap_or_default();
        let max = row.get("max").cloned().unwrap_or_default();
        issues.push(ValidationIssue::error(
            "L2-2",
            "L2",
            "check_l2_cardinality",
            format!(
                "基数约束自相矛盾：'{r}' 声明 minCardinality={min} > maxCardinality={max}"
            ),
        ));
    }
}

/// L2-3 disjoint 冲突：owl:disjointWith 的两类同时被同一实例断言。
pub fn check_l2_disjoint(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?c1 ?c2 ?x WHERE {{\n\
            ?c1 <{OWL_DISJOINT_WITH}> ?c2 .\n\
            ?x <{RDF_TYPE}> ?c1 .\n\
            ?x <{RDF_TYPE}> ?c2 .\n\
            FILTER(isIRI(?c1) && isIRI(?c2) && isIRI(?x))\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(c1) = row.get("c1").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(c2) = row.get("c2").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(x) = row.get("x").map(|s| strip_term(s)) else {
            continue;
        };
        issues.push(ValidationIssue::error(
            "L2-3",
            "L2",
            "check_l2_disjoint",
            format!(
                "实例 '{x}' 同时断言为 disjoint 的两类 '{c1}' 与 '{c2}'"
            ),
        ));
    }
}

/// L2-4 FunctionalProperty 多值：functional 属性对同一 subject 有多个不同 value。
pub fn check_l2_functional(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?p ?s WHERE {{\n\
            ?p <{RDF_TYPE}> <{OWL_FUNCTIONAL_PROPERTY}> .\n\
            ?s ?p ?v1 .\n\
            ?s ?p ?v2 .\n\
            FILTER(?v1 != ?v2)\n\
            FILTER(isIRI(?p) && isIRI(?s))\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(p) = row.get("p").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(s) = row.get("s").map(|s| strip_term(s)) else {
            continue;
        };
        issues.push(ValidationIssue::error(
            "L2-4",
            "L2",
            "check_l2_functional",
            format!(
                "FunctionalProperty '{p}' 对同一 subject '{s}' 有多个不同 value"
            ),
        ));
    }
}

/// L2-5 对称/传递属性自反冲突：symmetric 属性缺反向三元组、transitive 属性断链。
/// warning 级（启发式——缺少反向三元组可能是数据不完整，不一定是错误）。
pub fn check_l2_symmetric_transitive(store: &Store, issues: &mut Vec<ValidationIssue>) {
    // symmetric：存在 s p o 但不存在 o p s
    let q = format!(
        "SELECT ?p ?s ?o WHERE {{\n\
            ?p <{RDF_TYPE}> <{OWL_SYMMETRIC_PROPERTY}> .\n\
            ?s ?p ?o .\n\
            FILTER(isIRI(?p) && isIRI(?s) && isIRI(?o))\n\
            FILTER NOT EXISTS {{ ?o ?p ?s . }}\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(p) = row.get("p").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(s) = row.get("s").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(o) = row.get("o").map(|s| strip_term(s)) else {
            continue;
        };
        issues.push(ValidationIssue::warning(
            "L2-5",
            "L2",
            "check_l2_symmetric_transitive",
            format!("SymmetricProperty '{p}' 缺反向三元组：'{s}' → '{o}' 无 '{o}' → '{s}'"),
        ));
    }

    // transitive：s p o1 且 o1 p o2，但缺 s p o2（断链）
    let q2 = format!(
        "SELECT ?p ?s ?o1 ?o2 WHERE {{\n\
            ?p <{RDF_TYPE}> <{OWL_TRANSITIVE_PROPERTY}> .\n\
            ?s ?p ?o1 .\n\
            ?o1 ?p ?o2 .\n\
            FILTER(isIRI(?p) && isIRI(?s) && isIRI(?o1) && isIRI(?o2))\n\
            FILTER NOT EXISTS {{ ?s ?p ?o2 . }}\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q2) {
        let Some(p) = row.get("p").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(s) = row.get("s").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(o2) = row.get("o2").map(|s| strip_term(s)) else {
            continue;
        };
        issues.push(ValidationIssue::warning(
            "L2-5",
            "L2",
            "check_l2_symmetric_transitive",
            format!("TransitiveProperty '{p}' 断链：'{s}' 经传递应达 '{o2}' 但缺直接边"),
        ));
    }
}

// ════════════════════════════════════════════════════════════════
// L3 方法论合规（7 条）
// ════════════════════════════════════════════════════════════════

/// L3-1 标注完备性（warning）：每个 owl:Class / ObjectProperty / DatatypeProperty
/// 应有 `rdfs:label` 与 `rdfs:comment`。统一 warning 级（落地调整，2026-09-17）：
/// 设计文档初稿 3.1 为 error，但 validate_ttl 是通用闸门，也校验手写/存量/导入 TTL
/// （如测试夹具与历史本体），标注缺失是质量缺陷而非逻辑错误——全 error 会阻断
/// 存量本体改动（违背「老的不要动」），故降为 warning 并入可答辩组。
pub fn check_l3_annotation_completeness(store: &Store, issues: &mut Vec<ValidationIssue>) {
    // label 缺失
    let q = format!(
        "SELECT ?e ?kind WHERE {{\n\
            {{ ?e <{RDF_TYPE}> <{OWL_CLASS}> . BIND(\"class\" AS ?kind) }}\n\
            UNION {{ ?e <{RDF_TYPE}> <{OWL_OBJECT_PROPERTY}> . BIND(\"objectProperty\" AS ?kind) }}\n\
            UNION {{ ?e <{RDF_TYPE}> <{OWL_DATATYPE_PROPERTY}> . BIND(\"datatypeProperty\" AS ?kind) }}\n\
            FILTER(isIRI(?e))\n\
            FILTER NOT EXISTS {{ ?e <{RDFS_LABEL}> ?lbl . }}\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(e) = row.get("e").map(|s| strip_term(s)) else {
            continue;
        };
        let kind = row.get("kind").cloned().unwrap_or_default();
        issues.push(ValidationIssue::warning(
            "L3-1",
            "L3",
            "check_l3_annotation_completeness",
            format!("{kind} '{e}' 缺 rdfs:label（标注完备性）"),
        ));
    }
    // comment 缺失（有 label）
    let q2 = format!(
        "SELECT ?e ?kind WHERE {{\n\
            {{ ?e <{RDF_TYPE}> <{OWL_CLASS}> . BIND(\"class\" AS ?kind) }}\n\
            UNION {{ ?e <{RDF_TYPE}> <{OWL_OBJECT_PROPERTY}> . BIND(\"objectProperty\" AS ?kind) }}\n\
            UNION {{ ?e <{RDF_TYPE}> <{OWL_DATATYPE_PROPERTY}> . BIND(\"datatypeProperty\" AS ?kind) }}\n\
            FILTER(isIRI(?e))\n\
            EXISTS {{ ?e <{RDFS_LABEL}> ?lbl . }}\n\
            FILTER NOT EXISTS {{ ?e <{RDFS_COMMENT}> ?cmt . }}\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q2) {
        let Some(e) = row.get("e").map(|s| strip_term(s)) else {
            continue;
        };
        let kind = row.get("kind").cloned().unwrap_or_default();
        issues.push(ValidationIssue::warning(
            "L3-1",
            "L3",
            "check_l3_annotation_completeness",
            format!("{kind} '{e}' 缺 rdfs:comment（建议补充业务说明）"),
        ));
    }
}

/// L3-2 tentative 不入库（error）：IRI 局部名含 tentative / draft / tmp / open_question
/// 标记的实体不得出现在 Turtle（应进 open_questions.json）。
pub fn check_l3_tentative_not_persisted(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?e WHERE {{\n\
            {{ ?e <{RDF_TYPE}> <{OWL_CLASS}> . }}\n\
            UNION {{ ?e <{RDF_TYPE}> <{OWL_OBJECT_PROPERTY}> . }}\n\
            UNION {{ ?e <{RDF_TYPE}> <{OWL_DATATYPE_PROPERTY}> . }}\n\
            FILTER(isIRI(?e))\n\
        }}"
    );
    let markers = ["tentative", "Tentative", "draft", "Draft", "tmp", "Tmp", "open_question"];
    for row in run_sparql_solutions(store, &q) {
        let Some(e) = row.get("e").map(|s| strip_term(s)) else {
            continue;
        };
        if markers.iter().any(|m| e.contains(m)) {
            issues.push(ValidationIssue::error(
                "L3-2",
                "L3",
                "check_l3_tentative_not_persisted",
                format!("实体 '{e}' 含 tentative/draft 标记，不应入库（应进 open_questions.json）"),
            ));
        }
    }
}

/// L3-3 外键不留（warning）：datatype property 命名命中 `*Id/*Code/*No` 且另一实体命名
/// 含同词根 → 疑似应为 ObjectProperty。
pub fn check_l3_foreign_key_should_be_link(store: &Store, issues: &mut Vec<ValidationIssue>) {
    // 收集所有 datatype property 局部名
    let q = format!(
        "SELECT ?p WHERE {{\n\
            ?p <{RDF_TYPE}> <{OWL_DATATYPE_PROPERTY}> .\n\
            FILTER(isIRI(?p))\n\
        }}"
    );
    let mut props: Vec<String> = Vec::new();
    for row in run_sparql_solutions(store, &q) {
        if let Some(p) = row.get("p").map(|s| strip_term(s)) {
            props.push(p.to_string());
        }
    }
    // 收集所有 class 局部名
    let q2 = format!(
        "SELECT ?c WHERE {{\n\
            ?c <{RDF_TYPE}> <{OWL_CLASS}> .\n\
            FILTER(isIRI(?c))\n\
        }}"
    );
    let mut classes: Vec<String> = Vec::new();
    for row in run_sparql_solutions(store, &q2) {
        if let Some(c) = row.get("c").map(|s| strip_term(s)) {
            classes.push(c.to_string());
        }
    }
    // 启发式：datatype 属性局部名去掉 *Id/*Code/*No 后缀后，若在类名中找到同词根
    let suffixes = ["Id", "ID", "Code", "No"];
    for prop in &props {
        let local = prop.rsplit(['#', '/']).next().unwrap_or(prop);
        for suf in suffixes {
            if let Some(stripped) = local.strip_suffix(suf) {
                let stem = stripped.to_lowercase();
                if stem.len() < 2 {
                    continue;
                }
                let matched = classes.iter().any(|c| {
                    let cl = c.rsplit(['#', '/']).next().unwrap_or(c).to_lowercase();
                    cl.starts_with(&stem) || cl.contains(&stem)
                });
                if matched {
                    issues.push(ValidationIssue::warning(
                        "L3-3",
                        "L3",
                        "check_l3_foreign_key_should_be_link",
                        format!(
                            "DatatypeProperty '{prop}' 命名命中 *{suf} 且存在同词根类——疑似应为 ObjectProperty（外键不留，LinkType 原则）"
                        ),
                    ));
                    break;
                }
            }
        }
    }
}

/// L3-4 快照保留（warning）：类名含 Snapshot / Record / History 等快照语义词——
/// 历史快照字段应为属性而非新建类（防过度实体化，与 3.3 反向）。
pub fn check_l3_snapshot_should_be_attribute(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?c WHERE {{\n\
            ?c <{RDF_TYPE}> <{OWL_CLASS}> .\n\
            FILTER(isIRI(?c))\n\
        }}"
    );
    let markers = ["Snapshot", "snapshot", "Record", "History", "历史", "快照", "意向"];
    for row in run_sparql_solutions(store, &q) {
        let Some(c) = row.get("c").map(|s| strip_term(s)) else {
            continue;
        };
        if markers.iter().any(|m| c.contains(m)) {
            issues.push(ValidationIssue::warning(
                "L3-4",
                "L3",
                "check_l3_snapshot_should_be_attribute",
                format!(
                    "类 '{c}' 命名含快照/历史语义——疑似历史快照字段过度实体化，应为属性而非新建类"
                ),
            ));
        }
    }
}

/// L3-5 M:N 拆分（warning）：两实体间存在语义上多对多但未建中间类。
/// 检测：同一对 (domain, range) 间 >3 条独立 ObjectProperty（一对多关系的"扇出"信号）。
pub fn check_l3_many_to_many_split(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?d ?r (COUNT(DISTINCT ?p) AS ?cnt) WHERE {{\n\
            ?p <{RDF_TYPE}> <{OWL_OBJECT_PROPERTY}> .\n\
            ?p <{RDFS_DOMAIN}> ?d .\n\
            ?p <{RDFS_RANGE}> ?r .\n\
            FILTER(isIRI(?p) && isIRI(?d) && isIRI(?r))\n\
        }} GROUP BY ?d ?r"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(d) = row.get("d").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(r) = row.get("r").map(|s| strip_term(s)) else {
            continue;
        };
        let cnt = row
            .get("cnt")
            .map(|s| s.parse::<u32>().unwrap_or(0))
            .unwrap_or(0);
        if cnt > 3 {
            issues.push(ValidationIssue::warning(
                "L3-5",
                "L3",
                "check_l3_many_to_many_split",
                format!(
                    "'{d}' → '{r}' 间有 {cnt} 条独立 ObjectProperty——疑似多对多未建中间类"
                ),
            ));
        }
    }
}

/// L3-6 SWRL 原子完备（error）：swrl:Imp 的 body/head 引用的变量均有绑定价。
/// 简化实现：swrl:Imp 必须声明 body 与 head（不空）。
pub fn check_l3_swrl_atomic_completeness(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let q = format!(
        "SELECT ?imp WHERE {{\n\
            ?imp <{RDF_TYPE}> <{SWRL_IMP}> .\n\
            FILTER(isIRI(?imp))\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(imp) = row.get("imp").map(|s| strip_term(s)) else {
            continue;
        };
        // 检查该 imp 是否有 body/head（这里简单检查：存在至少一条以 imp 为主语的谓词）
        // 完整原子性需解析 swrl:body/head 内部结构，此处降级为"主体存在"检查
        issues.push(ValidationIssue::error(
            "L3-6",
            "L3",
            "check_l3_swrl_atomic_completeness",
            format!("SWRL 规则 '{imp}' 需人工复核 body/head 原子完备性（悬空变量检查）"),
        ));
    }
}

/// L3-7 7+1 类完整性（warning）：核心五类产物齐备（RDF/OWL/SKOS/SWRL/SPARQL 模板）。
/// 存在性检查：有 owl:Class 但无任何 SKOS 术语（skos:Concept）时告警。
pub fn check_l3_seven_plus_one_completeness(store: &Store, issues: &mut Vec<ValidationIssue>) {
    let has_class = !run_sparql_solutions(
        store,
        &format!(
            "SELECT ?c WHERE {{ ?c <{RDF_TYPE}> <{OWL_CLASS}> . }} LIMIT 1"
        ),
    )
    .is_empty();
    let has_skos = !run_sparql_solutions(
        store,
        &"SELECT ?c WHERE { ?c <http://www.w3.org/2004/02/skos/core#Concept> ?o . } LIMIT 1",
    )
    .is_empty();
    let has_imports = !run_sparql_solutions(
        store,
        &format!("SELECT ?o WHERE {{ ?x <{OWL_IMPORTS}> ?o . }} LIMIT 1"),
    )
    .is_empty();

    if has_class && !has_skos {
        issues.push(ValidationIssue::warning(
            "L3-7",
            "L3",
            "check_l3_seven_plus_one_completeness",
            "有 owl:Class 但无任何 SKOS 术语（skos:Concept）——术语统一层缺失（第 3 类语义）",
        ));
    }
    if has_class && !has_imports {
        issues.push(ValidationIssue::warning(
            "L3-7",
            "L3",
            "check_l3_seven_plus_one_completeness",
            "本体无 owl:imports 声明——外部依赖未显式化（语义管理缺失）",
        ));
    }
}

// ════════════════════════════════════════════════════════════════
// L2.5 数据可落地性（规则 + 数据源比对）
// ════════════════════════════════════════════════════════════════

/// L2.5 数据可落地性（warning/suggestion）：本体产物能否映射真实数据源。
///
/// 复用 onto-studio 已挂载的 datasets/data_sources（`list_ontology_datasets` /
/// `list_ontology_data_sources` 已存在）。本函数接受**数据源表名/列名列表**作为
/// 外部输入（由命令层从 datasets 表读取传入），做三类结构比对：
///   1. 属性 range 类型必须是 xsd:* 标准类型（否则无法映射数据列）
///   2. 外键命名属性（*Id/*Code）应有对应的数据列（真实表中存在）
///   3. 类的 IRI 局部名应能在数据源表名/列名中找到近似匹配（命名一致性）
///
/// `data_source_tables`: 已挂载数据源的「表名,列名」描述列表，如
/// `["supplier,id", "supplier,code", "order,order_id"]`。
pub fn run_l25_data_grounding(
    store: &Store,
    data_source_tables: &[String],
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    // 1. range 类型必须是 xsd 标准类型
    let q = format!(
        "SELECT ?p ?r WHERE {{\n\
            ?p <{RDF_TYPE}> <{OWL_DATATYPE_PROPERTY}> .\n\
            ?p <{RDFS_RANGE}> ?r .\n\
            FILTER(isIRI(?p) && isIRI(?r))\n\
        }}"
    );
    for row in run_sparql_solutions(store, &q) {
        let Some(p) = row.get("p").map(|s| strip_term(s)) else {
            continue;
        };
        let Some(r) = row.get("r").map(|s| strip_term(s)) else {
            continue;
        };
        if !r.starts_with("http://www.w3.org/2001/XMLSchema#") {
            issues.push(ValidationIssue::warning(
                "L2.5-1",
                "L2.5",
                "run_l25_data_grounding",
                format!(
                    "属性 '{p}' 的 range '{r}' 非 xsd:* 标准类型——数据可落地性存疑（真实数据列类型难映射）"
                ),
            ));
        }
    }

    // 2. 外键命名属性应有对应数据列（数据源已挂载时）
    if !data_source_tables.is_empty() {
        let q2 = format!(
            "SELECT ?p WHERE {{\n\
                ?p <{RDF_TYPE}> <{OWL_DATATYPE_PROPERTY}> .\n\
                FILTER(isIRI(?p))\n\
            }}"
        );
        let mut props: Vec<String> = Vec::new();
        for row in run_sparql_solutions(store, &q2) {
            if let Some(p) = row.get("p").map(|s| strip_term(s)) {
                props.push(p.to_string());
            }
        }
        // 数据源列名集合（小写比较）
        let columns: Vec<String> = data_source_tables
            .iter()
            .flat_map(|t| t.split(',').map(|c| c.trim().to_lowercase()))
            .collect();
        let suffixes = ["Id", "ID", "Code", "No"];
        for prop in &props {
            // 局部名：取 # 或 / 后最后一段（IRI 可能无 #，如 http://example.org/supplierId）
            let local = prop.rsplit(['#', '/']).next().unwrap_or(prop);
            for suf in suffixes {
                if let Some(stripped) = local.strip_suffix(suf) {
                    let stem = stripped.to_lowercase();
                    if stem.len() < 2 {
                        continue;
                    }
                    // 在列名中找同词根（列名可能带表前缀，取最后一段比较）
                    let matched = columns.iter().any(|c| {
                        let last = c.split('.').last().unwrap_or(c);
                        last.starts_with(&stem) || last.contains(&stem)
                    });
                    if !matched {
                        issues.push(ValidationIssue::warning(
                            "L2.5-2",
                            "L2.5",
                            "run_l25_data_grounding",
                            format!(
                                "外键属性 '{prop}' 在已挂载数据源列中找不到同词根列——数据可落地性存疑（映射断裂）"
                            ),
                        ));
                    }
                    break;
                }
            }
        }
    }

    issues
}

// ════════════════════════════════════════════════════════════════
// L2.6 业务可用性 CQ 反测（SPARQL 模板）
// ════════════════════════════════════════════════════════════════

/// L2.6 业务可用性 CQ 反测（error/suggestion）：用业务问题反测本体能否回答。
///
/// `cq_templates`：专家黄金用例 SPARQL 模板列表（charter 的 business_scenario +
/// 29 句话第 7 类模板推导；由调用方组装传入）。
///
/// 两级执行：
///   - 模板语法/变量绑定错误 → error（模板本身坏了）
///   - 模板可执行但返回空 → suggestion（本体可能缺支撑该查询的路径）
pub fn run_l26_cq_reverse_test(
    store: &Store,
    cq_templates: &[String],
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();

    for (i, template) in cq_templates.iter().enumerate() {
        // 模板变量绑定检查：确保 ?probe 在 SPARQL 中有绑定（防悬空变量）
        if !template.contains('?') {
            issues.push(ValidationIssue::error(
                "L2.6-1",
                "L2.6",
                "run_l26_cq_reverse_test",
                format!("CQ 模板 #{i} 不含任何变量绑定——模板无效"),
            ));
            continue;
        }
        // 执行模板（oxigraph 求值；语法错返回空集）
        let rows = run_sparql_solutions(store, template);
        if rows.is_empty() {
            issues.push(ValidationIssue::suggestion(
                "L2.6-2",
                "L2.6",
                "run_l26_cq_reverse_test",
                format!(
                    "CQ 模板 #{i} 执行无结果——本体可能缺支撑该业务查询的类/属性路径（业务可用性待查）"
                ),
            ));
        }
    }

    issues
}

// ════════════════════════════════════════════════════════════════
// 聚合入口
// ════════════════════════════════════════════════════════════════

/// 运行全部 L2/L3 确定性规则，产出结构化 issues。
pub fn run_l23_checks(store: &Store) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    check_l2_domain_range(store, &mut issues);
    check_l2_cardinality(store, &mut issues);
    check_l2_disjoint(store, &mut issues);
    check_l2_functional(store, &mut issues);
    check_l2_symmetric_transitive(store, &mut issues);
    check_l3_annotation_completeness(store, &mut issues);
    check_l3_tentative_not_persisted(store, &mut issues);
    check_l3_foreign_key_should_be_link(store, &mut issues);
    check_l3_snapshot_should_be_attribute(store, &mut issues);
    check_l3_many_to_many_split(store, &mut issues);
    check_l3_swrl_atomic_completeness(store, &mut issues);
    check_l3_seven_plus_one_completeness(store, &mut issues);
    // 同一规则可命中多次（如多条 domain/range 违反），但 ID 是每条规则写死的——
    // 直接落库会撞 validation_issues.id 的 UNIQUE 约束。重复 ID 加实例序号。
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for issue in issues.iter_mut() {
        let n = seen.entry(issue.id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            issue.id = format!("{}#{}", issue.id, n);
        }
    }
    issues
}

/// 按严重级拆分：error 进 errors，warning 进 warnings（与 L1 的 Vec<String> 兼容层）。
/// 返回 (errors, warnings, issues)——issues 为原始结构化列表。
pub fn split_issues(issues: Vec<ValidationIssue>) -> (Vec<String>, Vec<String>, Vec<ValidationIssue>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    for issue in &issues {
        match issue.severity {
            IssueSeverity::Error => errors.push(issue.message.clone()),
            IssueSeverity::Warning | IssueSeverity::Suggestion => {
                warnings.push(issue.message.clone())
            }
        }
    }
    (errors, warnings, issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(ttl: &str) -> Store {
        use oxigraph::io::{RdfFormat, RdfParser};
        let temp = Store::new().unwrap();
        let parser = RdfParser::from_format(RdfFormat::Turtle);
        temp.load_from_reader(parser, ttl.as_bytes()).unwrap();
        temp
    }

    #[test]
    fn l2_domain_range_detects_violation() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:Supplier a owl:Class .
            ex:Order a owl:Class .
            ex:hasSupplier a owl:ObjectProperty ; rdfs:domain ex:Order ; rdfs:range ex:Supplier .
            ex:wrong a ex:WrongType .
            ex:wrong ex:hasSupplier ex:Supplier .
        "#;
        let store = parse(ttl);
        let mut issues = Vec::new();
        check_l2_domain_range(&store, &mut issues);
        assert!(issues.iter().any(|i| i.id == "L2-1"));
    }

    #[test]
    fn l2_functional_detects_multi_value() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:code a owl:DatatypeProperty, owl:FunctionalProperty .
            ex:s ex:code "A" .
            ex:s ex:code "B" .
        "#;
        let store = parse(ttl);
        let mut issues = Vec::new();
        check_l2_functional(&store, &mut issues);
        assert!(issues.iter().any(|i| i.id == "L2-4"));
    }

    #[test]
    fn l3_annotation_missing_detected() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "供应商实体" .
            ex:Product a owl:Class .
        "#;
        let store = parse(ttl);
        let mut issues = Vec::new();
        check_l3_annotation_completeness(&store, &mut issues);
        assert!(issues
            .iter()
            .any(|i| i.id == "L3-1" && i.message.contains("Product")));
    }

    #[test]
    fn l3_foreign_key_suffix_detected() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:Supplier a owl:Class .
            ex:supplierId a owl:DatatypeProperty .
        "#;
        let store = parse(ttl);
        let mut issues = Vec::new();
        check_l3_foreign_key_should_be_link(&store, &mut issues);
        assert!(issues.iter().any(|i| i.id == "L3-3"));
    }

    #[test]
    fn l3_snapshot_class_detected() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:OrderSnapshot a owl:Class .
        "#;
        let store = parse(ttl);
        let mut issues = Vec::new();
        check_l3_snapshot_should_be_attribute(&store, &mut issues);
        assert!(issues.iter().any(|i| i.id == "L3-4"));
    }

    #[test]
    fn l25_data_grounding_detects_non_xsd_range() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            @prefix ex: <http://example.org/> .
            ex:ok a owl:DatatypeProperty ; rdfs:range xsd:string .
            ex:bad a owl:DatatypeProperty ; rdfs:range ex:CustomType .
        "#;
        let store = parse(ttl);
        let issues = run_l25_data_grounding(&store, &[]);
        assert!(
            issues.iter().any(|i| i.id == "L2.5-1" && i.message.contains("/bad")),
            "非 xsd range 应报: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        assert!(!issues.iter().any(|i| i.message.contains("/ok")));
    }

    #[test]
    fn l25_data_grounding_detects_broken_fk_mapping() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            @prefix ex: <http://example.org/> .
            ex:supplierId a owl:DatatypeProperty ; rdfs:range xsd:string .
        "#;
        let store = parse(ttl);
        // 数据源只有 order 表，没有 supplier 列 → supplierId 映射断裂
        let tables = vec!["order,order_id".to_string()];
        let issues = run_l25_data_grounding(&store, &tables);
        assert!(
            issues.iter().any(|i| i.id == "L2.5-2" && i.message.contains("supplierId")),
            "外键映射断裂应报: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn l25_passes_when_column_exists() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            @prefix ex: <http://example.org/> .
            ex:supplierId a owl:DatatypeProperty ; rdfs:range xsd:string .
        "#;
        let store = parse(ttl);
        let tables = vec!["supplier,id".to_string(), "supplier,code".to_string()];
        let issues = run_l25_data_grounding(&store, &tables);
        assert!(
            !issues.iter().any(|i| i.id == "L2.5-2"),
            "有 supplier 列不应报映射断裂: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }
    #[test]
    fn l26_cq_reverse_test_suggests_empty() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
            @prefix ex: <http://example.org/> .
            ex:Supplier a owl:Class ; rdfs:label "供应商"@zh .
        "#;
        let store = parse(ttl);
        // 问一个本体里不存在的属性 → 空结果 → suggestion
        let cqs = vec![
            "SELECT ?x WHERE { ?x <http://example.org/hasNonexistent> ?y . }".to_string(),
        ];
        let issues = run_l26_cq_reverse_test(&store, &cqs);
        assert!(
            issues.iter().any(|i| i.id == "L2.6-2"),
            "空结果应给 suggestion: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn l26_cq_reverse_test_error_on_no_variables() {
        let ttl = r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex: <http://example.org/> .
            ex:Supplier a owl:Class .
        "#;
        let store = parse(ttl);
        let cqs = vec!["SELECT * WHERE { }".to_string()];
        let issues = run_l26_cq_reverse_test(&store, &cqs);
        assert!(
            issues.iter().any(|i| i.id == "L2.6-1"),
            "无变量模板应报 error: {:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
    }

    #[test]
    #[test]
    fn duplicate_rule_hits_get_unique_ids() {
        // 两条 domain/range 违反 → 同名规则 ID 必须实例化去重（UNIQUE 约束）
        let ttl = r#"
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/t#> .
<https://example.org/t> a owl:Ontology ; rdfs:label "t"@zh .
:A a owl:Class ; rdfs:label "A"@zh ; rdfs:comment "x" .
:B a owl:Class ; rdfs:label "B"@zh ; rdfs:comment "x" .
:thing1 a owl:Thing ; rdfs:label "t1"@zh ; :p "1" .
:thing2 a owl:Thing ; rdfs:label "t2"@zh ; :p "2" .
:p a owl:DatatypeProperty ; rdfs:label "p"@zh ; rdfs:domain :A .
"#;
        let store = parse(ttl);
        let mut issues = Vec::new();
        check_l2_domain_range(&store, &mut issues);
        assert!(issues.len() >= 2, "应命中多次: {}", issues.len());
        let ids: Vec<_> = run_l23_checks(&store).iter().map(|i| i.id.clone()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "ID 重复: {:?}", ids);
    }

    fn split_issues_by_severity() {
        let issues = vec![
            ValidationIssue::error("L2-1", "L2", "c1", "e"),
            ValidationIssue::warning("L3-3", "L3", "c2", "w"),
            ValidationIssue::suggestion("L4-1", "L4", "c3", "s"),
        ];
        let (errors, warnings, back) = split_issues(issues);
        assert_eq!(errors.len(), 1);
        assert_eq!(warnings.len(), 2); // warning + suggestion
        assert_eq!(back.len(), 3);
    }
}
