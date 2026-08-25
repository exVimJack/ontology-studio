//! W3C Turtle 本体工具（把 ontology-store 的 `TtlStore` 能力暴露给 agent）。
//!
//! ## 设计
//!
//! 对齐 `ontology_tools.rs` 的 DynamicTool 闭包风格——捕获 `Arc<TtlStore>`，
//! 走同一条 `ToolServerHandle.add_dynamic_tool()` 注入路径。
//!
//! 与 Palantir 工具组的对照：
//!
//! | W3C 工具（本文件） | Palantir 对应 | 语义 |
//! |---|---|---|
//! | `validate_ontology_ttl` | `preview_ontology_import` | dry-run 校验，不写库 |
//! | `import_ontology_ttl` | `import_ontology` | 落库（UPSERT） |
//! | `export_ontology_ttl` | `export_ontology` | 取出 Turtle 文本 |
//! | `list_ontology_ttl` | `describe_ontology` | 本体列表摘要 |
//! | `query_ontology_sparql` | `describe_object_type` | 钻取查询（第 7 类 CRUD） |
//!
//! ## 闭环
//!
//! 对齐 skill `ontology-modeling-w3c` 的五环联动 ③④⑤：
//!   - 冷启动：`validate_ontology_ttl(ttl)` → `import_ontology_ttl(ttl)`
//!   - 增量：`export_ontology_ttl(iri)` → 改 Turtle → `validate` → `import`（overwrite=true）
//!   - 查询：`query_ontology_sparql(iri, sparql)` 跑第 7 类 CRUD
//!
//! 体积可控：TtlStore 内部 SQLite 存 .ttl 文本，校验/查询临时 load 内存 oxigraph，
//! 大本体（< 10 万三元组）秒级 parse，不撑爆上下文。

use std::sync::Arc;

use ontology_store::{TtlCharter, TtlStore};
use rig::tool::{DynamicTool, ToolOutput};
use serde_json::json;

// ════════════════════════════════════════════════════════════════
// 工具组入口
// ════════════════════════════════════════════════════════════════

/// 构造 W3C Turtle 本体工具组（8 个工具）。
///
/// 对齐 `ontology_tools::ontology_modeling_tools` 的导出风格——
/// 装配层（src-tauri）拿到 `Arc<TtlStore>` 后调用本函数，把返回的 Vec 注入 agent。
///
/// 5 个核心工具（validate/import/export/list/query）+ 3 个元数据工具
/// （set_charter/commit_change/list_changelog）。
pub fn ontology_ttl_tools(store: Arc<TtlStore>) -> Vec<DynamicTool> {
    vec![
        validate_ontology_ttl_tool(store.clone()),
        import_ontology_ttl_tool(store.clone()),
        export_ontology_ttl_tool(store.clone()),
        list_ontology_ttl_tool(store.clone()),
        query_ontology_sparql_tool(store.clone()),
        set_ontology_ttl_charter_tool(store.clone()),
        commit_ontology_ttl_change_tool(store.clone()),
        list_ontology_ttl_changelog_tool(store),
    ]
}

// ════════════════════════════════════════════════════════════════
// 5 个工具
// ════════════════════════════════════════════════════════════════

/// `validate_ontology_ttl(ttl_content)`：校验 Turtle（dry-run，不写库）。
fn validate_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "validate_ontology_ttl",
        "校验 Turtle (.ttl) 本体内容——语法合法性 + 7+1 语义规范一致性。dry-run，不写库。\n\
         对齐 skill `ontology-modeling-w3c` 五环联动 ③（建模→校验→预览→落库→查询）的校验环节。\n\
         \n\
         **errors 非空则禁止 import**：\n\
         - Turtle 语法错（解析失败）\n\
         - 类未声明 `a owl:Class`（用了 rdfs:Class，违反第 2 类）\n\
         - 属性混用（同一 IRI 既是 owl:DatatypeProperty 又是 owl:ObjectProperty，违反第 1 类）\n\
         - IRI 局部名命名违规（类应 PascalCase / 属性应 camelCase）\n\
         - SWRL 规则未声明 `a swrl:Imp`（违反第 4 类）\n\
         - rdfs:subClassOf 循环继承（违反第 2 类）\n\
         \n\
         **warnings 非空可继续 import**：\n\
         - 中文标签未带 `@zh` 语言标签（违反第 3 类）\n\
         \n\
         返回 TtlValidation：triple_count + errors[] + warnings[]。\n\
         参数 ttl_content 是完整 Turtle 文本（含 @prefix 声明和 owl:Ontology 主语）。",
        json!({
            "type": "object",
            "properties": {
                "ttl_content": {
                    "type": "string",
                    "description": "完整 Turtle (.ttl) 文本，含 @prefix 声明和 owl:Ontology 主语",
                },
            },
            "required": ["ttl_content"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let ttl = arg_str(&args, "ttl_content");
                if ttl.is_empty() {
                    return Ok(ToolOutput::text("错误：ttl_content 不能为空"));
                }
                match store.validate_ttl(&ttl) {
                    Ok(v) => Ok(ToolOutput::json(json!(v))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：校验失败 - {e}"))),
                }
            })
        },
    )
}

/// `import_ontology_ttl(ttl_content, overwrite?)`：导入 Turtle 落库。
fn import_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "import_ontology_ttl",
        "导入 Turtle (.ttl) 本体到存储（落库 SQLite）。best-effort：校验失败仍尝试落库，\n\
         但 **强烈建议先调 validate_ontology_ttl 确认无 errors 再调本工具**。\n\
         对齐 skill `ontology-modeling-w3c` 五环联动 ④ 落库环节。\n\
         \n\
         工作流：\n\
         - 冷启动：validate_ontology_ttl(ttl) → import_ontology_ttl(ttl)\n\
         - 增量：export_ontology_ttl(iri) → 改 Turtle → validate → import_ontology_ttl(ttl, overwrite=true)\n\
         \n\
         overwrite 语义（对齐 Palantir import_ontology 的 overwrite_object_types）：\n\
         - false（默认）：同 IRI 本体已存在则 skip（保护已有成果）\n\
         - true：同 IRI 本体 UPSERT 覆盖（整块 .ttl 文本替换）\n\
         \n\
         返回 TtlImportResult：ontology_iri + triple_count + errors[] + warnings[]。\n\
         本体 IRI 从 ttl 里 `?o a owl:Ontology` 提取，作为主键。",
        json!({
            "type": "object",
            "properties": {
                "ttl_content": {
                    "type": "string",
                    "description": "完整 Turtle (.ttl) 文本",
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "同 IRI 已存在时是否覆写（true=UPSERT，false=skip，默认 false）",
                    "default": false,
                },
            },
            "required": ["ttl_content"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let ttl = arg_str(&args, "ttl_content");
                if ttl.is_empty() {
                    return Ok(ToolOutput::text("错误：ttl_content 不能为空"));
                }
                let overwrite = args
                    .get("overwrite")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match store.import_ttl(&ttl, overwrite) {
                    Ok(r) => Ok(ToolOutput::json(json!(r))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：导入失败 - {e}"))),
                }
            })
        },
    )
}

/// `export_ontology_ttl(ontology_iri)`：导出本体的 Turtle 文本。
fn export_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "export_ontology_ttl",
        "导出指定本体的完整 Turtle (.ttl) 文本。对齐 Palantir export_ontology 的语义。\n\
         直接从 SQLite 读 .ttl 文本，不碰 oxigraph——体积等于原始 ttl 文件大小。\n\
         \n\
         用途：\n\
         - 增量更新：export → 改 Turtle → validate → import（overwrite=true）\n\
         - 备份/迁移：export 出 .ttl 文件，可被任何 RDF 工具消费（产物中立）\n\
         - 审阅：人读 Turtle 看本体全貌\n\
         \n\
         参数 ontology_iri 是本体的完整 IRI（如 https://example.org/ontology/test#TestOntology），\n\
         可从 list_ontology_ttl 获取。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": {
                    "type": "string",
                    "description": "本体 IRI（完整 URI，从 list_ontology_ttl 获取）",
                },
            },
            "required": ["ontology_iri"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                match store.export_ttl(&iri) {
                    Ok(ttl) => Ok(ToolOutput::text(ttl)),
                    Err(e) => Ok(ToolOutput::text(format!("错误：导出失败 - {e}"))),
                }
            })
        },
    )
}

/// `list_ontology_ttl()`：列出所有已存本体摘要。
fn list_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "list_ontology_ttl",
        "列出所有已存储的 W3C Turtle 本体（轻量摘要）。对齐 Palantir describe_ontology 的列表语义。\n\
         每条含：ontology_iri / version（owl:versionInfo）/ triple_count / updated_at（unix ms）。\n\
         按 updated_at 倒序（最近更新在前）。体积小，适合先调本工具看有哪些本体，\n\
         再调 export_ontology_ttl 或 query_ontology_sparql 钻取单个本体。\n\
         无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        move |_ctx, _args| {
            let store = store.clone();
            Box::pin(async move {
                match store.list_ontologies() {
                    Ok(list) => Ok(ToolOutput::json(json!(list))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：列出本体失败 - {e}"))),
                }
            })
        },
    )
}

/// `query_ontology_sparql(ontology_iri, sparql)`：跑 SPARQL 查询（第 7 类 CRUD）。
fn query_ontology_sparql_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "query_ontology_sparql",
        "对指定本体执行 SPARQL 查询，返回 SPARQL Results JSON。对齐 skill `ontology-modeling-w3c`\n\
         7+1 语义规范的第 7 类（SPARQL CRUD 操作模板）。\n\
         \n\
         内部：从 SQLite 读 .ttl → load 内存 oxigraph → 跑 SPARQL → JSON 序列化。\n\
         适合中小本体（< 10 万三元组）；大本体会有秒级 parse 延迟。\n\
         \n\
         **自动注入常用 W3C 前缀**（rdf/rdfs/owl/xsd/skos/swrl），查询可用缩写：\n\
         ```sparql\n\
         SELECT ?c ?label WHERE {\n\
           ?c a owl:Class .\n\
           ?c rdfs:label ?label .\n\
           FILTER(lang(?label) = \"zh\")\n\
         }\n\
         ```\n\
         \n\
         常用查询模式：\n\
         - 列所有类：`SELECT ?c WHERE { ?c a owl:Class . }`\n\
         - 看类继承：`SELECT ?sub ?sup WHERE { ?sub rdfs:subClassOf ?sup . }`\n\
         - 找中文标签：`SELECT ?s ?l WHERE { ?s rdfs:label ?l . FILTER(lang(?l)=\"zh\") }`\n\
         \n\
         参数：ontology_iri（本体 IRI，从 list_ontology_ttl 获取）、sparql（SPARQL 查询文本）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": {
                    "type": "string",
                    "description": "本体 IRI（完整 URI，从 list_ontology_ttl 获取）",
                },
                "sparql": {
                    "type": "string",
                    "description": "SPARQL 查询文本（可用 rdf/rdfs/owl/xsd/skos/swrl 缩写前缀）",
                },
            },
            "required": ["ontology_iri", "sparql"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                let sparql = arg_str(&args, "sparql");
                if iri.is_empty() || sparql.is_empty() {
                    return Ok(ToolOutput::text(
                        "错误：ontology_iri 和 sparql 都不能为空",
                    ));
                }
                match store.query_sparql(&iri, &sparql) {
                    Ok(json) => Ok(ToolOutput::text(json)),
                    Err(e) => Ok(ToolOutput::text(format!("错误：SPARQL 查询失败 - {e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 辅助
// ════════════════════════════════════════════════════════════════

/// `set_ontology_ttl_charter(ontology_iri, business_scenario, business_essence, design_intent, invariants)`。
/// 写入/更新本体设计宪章（不变点）。对齐 Palantir `set_ontology_charter`。
fn set_ontology_ttl_charter_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "set_ontology_ttl_charter",
        "写入或更新 W3C Turtle 本体的设计宪章（不变点：业务场景 / 本质 / 设计意图 / 补充说明）。\n\
         对齐 Palantir `set_ontology_charter` 的语义——charter 记业务本质说明，不随历史变化。\n\
         **只有用户明确要求调整 charter 时才调用**；常规增量更新（增删改类/属性）不应触碰 charter。\n\
         冷启动建模时，如历史对话已提及业务场景/本质/意图，应主动提取后调用本工具落库。\n\
         \n\
         **charter 同步写进 .ttl 的 W3C 原生词汇**（用户在 Turtle 里用 `dcterms:` + `skos:note`\n\
         记录本体元数据，本表是可查询的结构化索引，两者并存）：\n\
         - business_scenario → `dcterms:description`\n\
         - business_essence → `skos:definition`\n\
         - design_intent → `rdfs:comment`（设计意图）\n\
         - invariants → `skos:scopeNote`\n\
         \n\
         字段语义：\n\
         - business_scenario：业务场景（服务于什么业务目标、谁用、解决什么问题）\n\
         - business_essence：业务本质（核心业务对象/状态/关系/动态行为的一句话本质概括）\n\
         - design_intent：设计意图（为什么这样建模、够用且可扩展的取舍、可扩展方向）\n\
         - invariants：补充说明（自由文本，记录不可违反的业务约束、边界条件等）\n\
         updated_by 默认 'agent'，可选 'user'。返回成功提示。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": {
                    "type": "string",
                    "description": "本体 IRI（完整 URI，从 list_ontology_ttl 获取）",
                },
                "business_scenario": {
                    "type": "string",
                    "description": "业务场景",
                    "default": "",
                },
                "business_essence": {
                    "type": "string",
                    "description": "业务本质",
                    "default": "",
                },
                "design_intent": {
                    "type": "string",
                    "description": "设计意图",
                    "default": "",
                },
                "invariants": {
                    "type": "string",
                    "description": "补充说明（自由文本）",
                    "default": "",
                },
                "updated_by": {
                    "type": "string",
                    "description": "更新方（'agent' | 'user'，默认 'agent'）",
                    "default": "agent",
                },
            },
            "required": ["ontology_iri"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                let charter = TtlCharter {
                    business_scenario: arg_str(&args, "business_scenario"),
                    business_essence: arg_str(&args, "business_essence"),
                    design_intent: arg_str(&args, "design_intent"),
                    invariants: arg_str(&args, "invariants"),
                    updated_by: arg_str(&args, "updated_by"),
                    updated_at: 0,
                };
                match store.set_charter(&iri, &charter) {
                    Ok(()) => Ok(ToolOutput::text(format!(
                        "已更新本体 '{iri}' 的设计宪章"
                    ))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：写入 charter 失败 - {e}"))),
                }
            })
        },
    )
}

/// `commit_ontology_ttl_change(ontology_iri, title, body, change_summary?, conversation_id?)`。
/// 提交一条本体变更日志（git commit message 式）。对齐 Palantir `commit_ontology_change`。
fn commit_ontology_ttl_change_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "commit_ontology_ttl_change",
        "提交一条 W3C Turtle 本体变更日志（git commit message 式），记录本次变更的设计意图。\n\
         对齐 Palantir `commit_ontology_change` 的语义。\n\
         **每次 import_ontology_ttl 或 delete_ontology 之后必须调用**——把本次改动的「为什么」和\n\
         「整体设计」落成可回溯记录，方便以后复现思路。\n\
         \n\
         体积限制：title+body ≤500 字符，整条（含 change_summary）≤1K 字符。超出会被拒。\n\
         - title：一句话标题（如「给 18 个子类补 a owl:Class」）。\n\
         - body：设计说明正文——为什么改、整体思路、影响范围、以后怎么复现。\n\
         - change_summary：可选，机器可读 JSON 摘要（如 {\"created\":[...],\"deleted\":[...]}}），\n\
           可从 import_ontology_ttl 的返回结果归纳。省略时存空对象。\n\
         - conversation_id：可选，来源会话 id。\n\
         返回分配的 revision 序号（本体内从 1 递增）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": { "type": "string", "description": "本体 IRI（完整 URI）" },
                "title": { "type": "string", "description": "一句话标题（≤500 chars 合计约束）" },
                "body": { "type": "string", "description": "设计说明正文", "default": "" },
                "change_summary": { "type": "string", "description": "机器可读 JSON 摘要", "default": "{}" },
                "conversation_id": { "type": "string", "description": "来源会话 id（可选）" },
            },
            "required": ["ontology_iri", "title"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                let title = arg_str(&args, "title");
                if iri.is_empty() || title.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 和 title 不能为空"));
                }
                let body = arg_str(&args, "body");
                let summary = {
                    let s = arg_str(&args, "change_summary");
                    if s.is_empty() { "{}".to_string() } else { s }
                };
                let conv_id = args.get("conversation_id").and_then(|v| v.as_str());
                match store.commit_change(&iri, &title, &body, &summary, conv_id, "agent") {
                    Ok(rev) => Ok(ToolOutput::text(format!("已记录变更日志 revision={rev}"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：记录变更日志失败 - {e}"))),
                }
            })
        },
    )
}

/// `list_ontology_ttl_changelog(ontology_iri)`：列出本体变更历史。对齐 Palantir `list_ontology_changelog`。
fn list_ontology_ttl_changelog_tool(store: Arc<TtlStore>) -> DynamicTool {
    DynamicTool::new(
        "list_ontology_ttl_changelog",
        "列出 W3C Turtle 本体的变更历史（git commit log 式，按 revision 倒序，最新在前）。\n\
         对齐 Palantir `list_ontology_changelog` 的语义。\n\
         每条含 revision/title/body/change_summary/conversation_id/author/created_at。\n\
         供回溯本体的演进过程与设计决策。\n\
         \n\
         参数 ontology_iri 是本体的完整 IRI，从 list_ontology_ttl 获取。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": { "type": "string", "description": "本体 IRI（完整 URI）" },
            },
            "required": ["ontology_iri"],
            "additionalProperties": false,
        }),
        move |_ctx, args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                match store.list_changelog(&iri) {
                    Ok(logs) => Ok(ToolOutput::json(json!(logs))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：查询变更历史失败 - {e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 辅助
// ════════════════════════════════════════════════════════════════

/// 从工具参数里取字符串字段，缺失或非字符串返回空串。
///
/// 对齐 `ontology_tools.rs::arg_str`——统一参数取值 + 空串校验，
/// 避免每个工具重复写 `args.get(...).and_then(...).unwrap_or("")`。
fn arg_str(args: &serde_json::Value, key: &str) -> String {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ttl() -> &'static str {
        r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/ontology/test#> .

:TestOntology a owl:Ontology ;
    rdfs:label "测试本体"@zh ;
    owl:versionInfo "1.0.0" .

:Supplier a owl:Class ;
    rdfs:label "供应商"@zh .

:supplierId a owl:DatatypeProperty ;
    rdfs:domain :Supplier ;
    rdfs:range xsd:string ;
    rdfs:label "供应商编号"@zh .
"#
    }

    #[test]
    fn tool_group_has_eight_tools() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        let tools = ontology_ttl_tools(store);
        // 5 核心 (validate/import/export/list/query) + 3 元数据 (set_charter/commit_change/list_changelog)
        assert_eq!(tools.len(), 8);
    }

    #[test]
    fn validate_workflow_end_to_end() {
        // 闭环：validate → import → list → export → query
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        // 1. validate
        let v = store.validate_ttl(sample_ttl()).unwrap();
        assert!(v.errors.is_empty(), "validate errors: {:?}", v.errors);
        assert!(v.triple_count > 0);
        // 2. import
        let r = store.import_ttl(sample_ttl(), true).unwrap();
        assert!(r.errors.is_empty());
        assert_eq!(
            r.ontology_iri,
            "https://example.org/ontology/test#TestOntology"
        );
        // 3. list
        let list = store.list_ontologies().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "1.0.0");
        // 4. export
        let ttl = store
            .export_ttl("https://example.org/ontology/test#TestOntology")
            .unwrap();
        assert!(ttl.contains("owl:Ontology"));
        // 5. query
        let json = store
            .query_sparql(
                "https://example.org/ontology/test#TestOntology",
                "SELECT ?c WHERE { ?c a owl:Class . }",
            )
            .unwrap();
        assert!(json.contains("Supplier"));
    }
}
