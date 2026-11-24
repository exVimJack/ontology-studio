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

use ontology_store::ttl::{extract_charter_predicates, extract_ontology_meta};
use ontology_store::TtlStore;
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
/// 5 个核心工具（validate/import/export/list/query）+ 2 个元数据工具
/// （commit_change/list_changelog）。
/// charter 写入已从 agent 工具移除（宪章用户主权：仅前端 CharterPanel 落库）。
pub fn ontology_ttl_tools(store: Arc<TtlStore>) -> Vec<DynamicTool> {
    vec![
        register_evidence_source_ttl_tool(store.clone()),
        validate_ontology_ttl_tool(store.clone()),
        import_ontology_ttl_tool(store.clone()),
        export_ontology_ttl_tool(store.clone()),
        list_ontology_ttl_tool(store.clone()),
        query_ontology_sparql_tool(store.clone()),
        commit_ontology_ttl_change_tool(store.clone()),
        list_ontology_ttl_changelog_tool(store.clone()),
        publish_ontology_version_tool(store.clone()),
        list_ontology_versions_tool(store.clone()),
        revert_ontology_version_tool(store.clone()),
        diff_ontology_versions_tool(store),
    ]
}

// ════════════════════════════════════════════════════════════════
// 5 个工具
// ════════════════════════════════════════════════════════════════

/// `register_evidence_source`（TTL 侧）：登记证据源（docs/EVIDENCE-CHAIN.md L1）。
/// 与 Palantir 侧同名工具同库同表（装配层同文件打开），
/// 两个工具组都暴露同名工具，确保 W3C-only 会话也能登记。
fn register_evidence_source_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "register_evidence_source",
        "登记建模证据源（本体建模证据链，docs/EVIDENCE-CHAIN.md）。\n\
         **TTL 建模前必须先调用**：把用户输入的建模依据（场景描述/文档片段/DB schema）登记为可引用的源，\n\
         返回 source_id。之后 TTL 中每个 owl:Class / owl:DatatypeProperty / owl:ObjectProperty \n\
         必须带证据注解：\n\
         skos:editorialNote \"evidence: {source_id} | direct | 「原文逐字摘录（≤200字）」\"\n\
         - role=\"direct\"：quote 必须是源 content 的逐字原文（机械校验，改写/臆想会被阻断）\n\
         - role=\"inferred\"：组合推断，多段原文在「」内用 \" ⟪…⟫ \" 拼接\n\
         同内容重复登记幂等（返回同一 source_id）。kind：scenario | document | db_table | db_column。",
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["scenario", "document", "db_table", "db_column"],
                    "description": "源类型",
                },
                "title": {
                    "type": "string",
                    "description": "人类可读出处（如：会话场景描述 / 需求文档.docx 第3节 / erp.orders 表）",
                },
                "content": {
                    "type": "string",
                    "description": "源文本全文（登记时刻快照；后续证据注解的 quote 从中逐字摘录）",
                },
                "ontology_api_name": {
                    "type": "string",
                    "description": "归属本体（可空=全局共享；TTL 侧可填本体 IRI 或留空）",
                    "default": "",
                },
                "origin_ref": {
                    "type": "string",
                    "description": "关联既有实体 id（可空）",
                    "default": "",
                },
            },
            "required": ["kind", "title", "content"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let kind = arg_str(&args, "kind");
                let title = arg_str(&args, "title");
                let content = arg_str(&args, "content");
                let ont = arg_str(&args, "ontology_api_name");
                let origin = arg_str(&args, "origin_ref");
                match store.register_evidence_source(&kind, &title, &content, &ont, &origin) {
                    Ok(id) => Ok(ToolOutput::text(format!(
                        "证据源已登记：source_id={id}\n\
                         TTL 中每个类/属性的证据注解（skos:editorialNote）引用此 id，\n\
                         quote 必须从 content 逐字摘录（≤200字），否则导入会被阻断。"
                    ))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：登记证据源失败 - {e}"))),
                }
            })
        },
    )
}

/// `validate_ontology_ttl(ttl_content)`：校验 Turtle（dry-run，不写库）。
fn validate_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
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
        move |args| {
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
    crate::dynamic_tool(
        "import_ontology_ttl",
        "导入 Turtle (.ttl) 本体到存储（落库 SQLite）。best-effort：校验失败仍尝试落库，\n\
         但 **强烈建议先调 validate_ontology_ttl 确认无 errors 再调本工具**。\n\
         对齐 skill `ontology-modeling-w3c` 五环联动 ④ 落库环节。\n\
         \n\
         **证据闸门（strict，docs/EVIDENCE-CHAIN.md）**：TTL 中每个 owl:Class / owl:DatatypeProperty / \
         owl:ObjectProperty 必须带证据注解：\n\
         skos:editorialNote \"evidence: {source_id} | direct | 「原文逐字摘录≤200字」\"\n\
         - source_id 来自 register_evidence_source 登记结果\n\
         - direct 的 quote 必须是登记源 content 的逐字原文（改写/臆想会被阻断）\n\
         - inferred 多段用 ⟪…⟫ 拼接；确属抽象设计的元素请向用户说明并请求确认\n\
         **grandfathering（存量豁免，闸门口径确定、无需实测）**：库内已存在的元素，\n\
         定义（IRI 的全部谓词-取值）与库内一致 → 无注解也自动放行（validate 输出一条\n\
         \"grandfathered: N 个…\" 汇总 warning）；定义变了或 IRI 是新增 → 必须有证据。\n\
         纯新增元素（新属性/新类，不改已有 IRI）直接用 **mode=\"append\"** 增量追加——\n\
         无需 export 全文搬运，存量三元组不受影响；修改/删除已有元素才走全文回导。\n\
         \n\
         工作流：\n\
         - 冷启动：register_evidence_source(登记用户输入) → 生成 TTL（含证据注解）→ validate → import\n\
         - 增量：export_ontology_ttl(iri, mode=\"full\") → 改 Turtle → validate → import_ontology_ttl(ttl, overwrite=true)\n\
         overwrite 语义（对齐 Palantir import_ontology 的 overwrite_object_types）：\n\
         - false（默认）：同 IRI 本体已存在则 skip（保护已有成果）\n\
         - true：同 IRI 本体 UPSERT 覆盖（整块 .ttl 文本替换——增量必须 export 全文修改后全量回导，
           提交切片会拒绝或导致其余内容丢失；缩水超 20% 需 confirm_full_replace=true）\n\
         \n\
         **宪章先行闸门**（决策 24）：目标 IRI 在库内不存在时拒绝导入——
         本体创建必须由用户在「新建本体」向导完成设计宪章；
         冷启动时请先输出建模思路 + 宪章要点，指引用户完成向导，本体建立后再导入。
         
         **宪章守卫**：覆盖同 IRI 时，owl:Ontology 头部宪章两字段\n\
         （dcterms:description/skos:definition）\n\
         必须与库内原文完全一致，否则拒绝导入——宪章是用户主权，\n\
         如需修改请让用户在「设计宪章」面板操作。\n\
         \n\
         返回 TtlImportResult：ontology_iri + triple_count + errors[] + warnings[]。\n\
         本体 IRI 从 ttl 里 `?o a owl:Ontology` 提取，作为主键。",
        json!({
            "type": "object",
            "properties": {
                "ttl_content": {
                    "type": "string",
                    "description": "Turtle (.ttl) 文本。mode=replace 时为完整全文；mode=append 时为纯新增元素的增量片段（可不含 owl:Ontology 头）",
                },
                "mode": {
                    "type": "string",
                    "enum": ["replace", "append"],
                    "description": "replace（默认，配合 overwrite）= 整本体替换，必须提交 export 全文修改后的完整文本；append = 纯增量追加：delta 只含新增元素（不允许重定义已有 IRI），直接拼接进库内原文（无损保留排版注释），不触发缩水守卫。给已有本体加新属性/新类时优先用 append",
                    "default": "replace",
                },
                "ontology_iri": {
                    "type": "string",
                    "description": "目标本体 IRI（mode=append 时必填——delta 可不含 owl:Ontology 头，用此参数指定追加到哪个本体）",
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "同 IRI 已存在时是否覆写（true=整本体替换，false=skip，默认 false）。注意：true 会用提交的 TTL 整体替换库内内容——增量更新请先 export 全文修改后全量回导",
                    "default": false,
                },
                "confirm_full_replace": {
                    "type": "boolean",
                    "description": "确认整体替换：新三元组数比存量缩水超过 20% 时，须显式传 true 才允许导入（防切片误替换导致数据丢失）",
                    "default": false,
                },
            },
            "required": ["ttl_content"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ttl = arg_str(&args, "ttl_content");
                if ttl.is_empty() {
                    return Ok(ToolOutput::text("错误：ttl_content 不能为空"));
                }
                let mode = args
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("replace")
                    .to_string();
                let overwrite = args
                    .get("overwrite")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // append 模式：纯增量追加（新增元素 → 拼接入库，存量不动）。
                // 闸门在 append_ttl 内部按合并后全文校验（存量豁免 + 新增需证据）。
                if mode == "append" {
                    let iri = args
                        .get("ontology_iri")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            let mut w = Vec::new();
                            extract_ontology_meta(&ttl, &mut w).0
                        });
                    let Some(iri) = iri else {
                        return Ok(ToolOutput::text(
                            "错误：mode=append 需要 ontology_iri 参数（delta 未含 owl:Ontology 头时无法推断目标本体）",
                        ));
                    };
                    return match store.append_ttl(&iri, &ttl) {
                        Ok(r) if r.errors.is_empty() => Ok(ToolOutput::json(json!(r))),
                        Ok(r) => Ok(ToolOutput::text(format!(
                            "追加被拒绝：\n{}\n{}",
                            r.errors.join("\n"),
                            if r.warnings.is_empty() {
                                String::new()
                            } else {
                                format!("\nwarnings: {}", r.warnings.join("\n"))
                            }
                        ))),
                        Err(e) => Ok(ToolOutput::text(format!("错误：追加失败 - {e}"))),
                    };
                }
                let overwrite = overwrite;
                // 宪章先行闸门（决策 24）：库内不存在的 IRI 拒绝导入——
                // 本体创建 = 用户在宪章向导确认宪章，AI 不得从 0 到 1 创建本体。
                if let Some(reject) = new_ontology_gate_ttl(&store, &ttl) {
                    return Ok(ToolOutput::text(reject));
                }
                // 宪章守卫（用户主权）：堵侧门——set_ontology_ttl_charter 已移除，
                // AI 不得借 import(overwrite) 改写 owl:Ontology 头部内嵌宪章。
                if let Some(reject) = charter_guard(&store, &ttl, overwrite) {
                    return Ok(ToolOutput::text(reject));
                }
                // 宪章空检测：手工导入的本体可能无宪章，agent 迭代前须让用户补全
                if let Some(reject) = (|| {
                    let (iri, _) = extract_ontology_meta(&ttl, &mut Vec::new());
                    let iri = iri?;
                    charter_empty_gate(&store, &iri)
                })() {
                    return Ok(ToolOutput::text(reject));
                }
                // 校验闸门（VALIDATION-DESIGN.md v0.3.1）：未闭环 error issue /
                // 未 resolved review / 改完没重跑 validate → 拒绝导入。
                // 内联重跑 validate_ttl（旁路堵漏），无签名比对存储。
                // 校验闸门：error 拦截；warnings 合并进导入结果（下方）
                let gate_warnings: Vec<String>;
                match store.validation_gate(&ttl) {
                    Ok((gate_errors, _gw)) if !gate_errors.is_empty() => {
                        return Ok(ToolOutput::text(format!(
                            "导入被拒绝：校验闸门拦截 {} 条问题，逐条修复后重新 validate 再导入：\n\n{}",
                            gate_errors.len(),
                            gate_errors.join("\n"),
                        )));
                    }
                    Ok((_, gw)) => gate_warnings = gw,
                    Err(e) => {
                        return Ok(ToolOutput::text(format!("错误：校验闸门执行失败 - {e}")));
                    }
                }
                // 证据闸门（strict，docs/EVIDENCE-CHAIN.md 5.4）：每个类/属性必须有
                // skos:editorialNote "evidence: {source_id} | {role} | 「quote」" 注解，
                // quote 与登记源逐字匹配；先 register_evidence_source 登记源。
                match store.validate_ttl_evidence(&ttl, true) {
                    Ok((errors, _)) if !errors.is_empty() => {
                        return Ok(ToolOutput::text(format!(
                            "导入被拒绝：证据闸门拦截 {} 条问题，逐条修复后重新导入\
                             （quote 必须从登记源逐字摘录，不要改写）：\n\n{}",
                            errors.len(),
                            errors
                                .iter()
                                .enumerate()
                                .map(|(i, e)| format!("{}. {e}", i + 1))
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        )));
                    }
                    Ok(_) => {}
                    Err(e) => return Ok(ToolOutput::text(format!("错误：证据校验失败 - {e}"))),
                }
                // 缩水防护（事故 2026-09-17）：overwrite=true 是【整本体替换】语义，
                // 不是按元素 UPSERT——提交切片会把库里其余内容全部抹掉。
                // 新三元组数比存量缩水超过 20% 时拒绝，除非显式传 confirm_full_replace=true。
                if overwrite
                    && !args
                        .get("confirm_full_replace")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                {
                    if let Ok(list) = store.list_ontologies() {
                        let (iri, _) = extract_ontology_meta(&ttl, &mut Vec::new());
                        if let Some(iri) = iri {
                            if let Some(existing) =
                                list.iter().find(|o| o.ontology_iri == iri)
                            {
                                let new_count =
                                    store.count_ttl_triples(&ttl).unwrap_or(0);
                                if new_count * 5 < existing.triple_count * 4
                                    && existing.triple_count > 20
                                {
                                    return Ok(ToolOutput::text(format!(
                                        "导入被拒绝：提交的 TTL 只有 {} 条三元组，而库内该本体有 {} 条\n\
                                         （缩水超过 20%）。import_ontology_ttl 的 overwrite 是【整本体替换】，\n\
                                         不是按元素合并——增量更新某个对象的正确做法：\n\
                                         1. export_ontology_ttl(iri, mode=\"full\") 取出完整原文\n\
                                         2. 在完整原文上修改目标对象\n\
                                         3. 全量回导\n\
                                         如确认要整体替换（例如删除大量内容），请带 confirm_full_replace=true 重试。",
                                        new_count, existing.triple_count
                                    )));
                                }
                            }
                        }
                    }
                }
                match store.import_ttl(&ttl, overwrite) {
                    Ok(mut r) => {
                        // 合并校验闸门警告（L2/L3 warning 等）到导入结果
                        for w in gate_warnings {
                            r.warnings.push(w);
                        }
                        Ok(ToolOutput::json(json!(r)))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：导入失败 - {e}"))),
                }
            })
        },
    )
}

/// 宪章先行闸门（决策 24）：目标 IRI 在库内不存在时拒绝导入。
///
/// 本体创建的唯一合法路径 = 用户在「新建本体」宪章向导中确认宪章
/// （前端 IPC create_ontology_ttl_with_charter）。AI 只能在已建立的本体上
/// 增量建模；冷启动时应输出建模思路 + 宪章要点，指引用户完成向导。
/// 返回 Some(拒绝说明) 表示应拒绝；None 放行。
fn new_ontology_gate_ttl(store: &TtlStore, ttl: &str) -> Option<String> {
    let mut warnings = Vec::new();
    let (iri, _) = extract_ontology_meta(ttl, &mut warnings);
    let iri = iri?;
    match store.exists(&iri) {
        Ok(true) => None,
        Ok(false) => Some(format!(
            "导入被拒绝：本体 {iri} 尚未建立。宪章先行（ARCHITECTURE.md 决策 24）：\
             设计宪章是本体建模的前提，本体创建必须由用户完成。\n\
             请指引用户：在「设计宪章」页点「新建本体」，填写业务场景与业务目标\
             两字段（可先用 AI 起草建议预填）并保存；\n\
             本体建立后，把你的建模思路和宪章要点作为参考给用户，再在新本体上\
             执行增量导入。在此之前不要尝试任何导入。"
        )),
        Err(e) => Some(format!("错误：检查本体是否存在失败 - {e}")),
    }
}

/// 宪章守卫：同 IRI 覆盖导入时，禁止改动 owl:Ontology 头部内嵌宪章两谓词。
/// 返回 Some(拒绝说明) 表示应拒绝导入；None 放行。
/// 放行场景：新本体首次导入（宪章随本体建立，属正常创建）；overwrite=false（同 IRI skip 无写入）；
/// 宪章两字段与库内原文完全一致（改的只是类/属性等非宪章内容）。
/// 宪章空检测（决策 24 延伸）：手工导入的本体可能没有宪章（绕过了向导）。
/// agent 在这种本体上迭代 schema = 没有业务意图基线，拒绝并知会用户先补全。
/// 仅拦 agent 路径——用户自己的 UI 导入/编辑不受限。
fn charter_empty_gate(store: &TtlStore, ontology_iri: &str) -> Option<String> {
    match store.get_charter(ontology_iri) {
        Ok(c)
            if c.business_scenario.trim().is_empty()
                && c.business_essence.trim().is_empty()
                =>
        {
            Some(format!(
                "导入被拒绝：本体 {ontology_iri} 的设计宪章为空（该本体可能由手工导入建立，                 未经过宪章向导）。\n\
                 宪章是建模的业务意图基线（决策 24）：没有它，agent 迭代 schema 时无法校验\n\
                 变更是否偏离建模初衷。请先知会用户补全宪章：在「设计宪章」面板填写\n\
                 业务场景/业务目标两字段（你可先根据既有模型内容起草\n\
                 建议供用户修改确认，但写入必须由用户操作），补全后再执行导入。"
            ))
        }
        Ok(_) => None,
        Err(e) => Some(format!("错误：检查宪章失败 - {e}")),
    }
}

fn charter_guard(store: &TtlStore, ttl: &str, overwrite: bool) -> Option<String> {
    let mut warnings = Vec::new();
    let (iri, _) = extract_ontology_meta(ttl, &mut warnings);
    let iri = iri?;
    let old = store.export_ttl(&iri).ok()?;
    if !overwrite {
        return None;
    }
    let new_c = extract_charter_predicates(ttl);
    let old_c = extract_charter_predicates(&old);
    if new_c == old_c {
        return None;
    }
    const FIELDS: [&str; 2] = [
        "业务场景(dcterms:description)",
        "业务目标(skos:definition)",
    ];
    let changed: Vec<&str> = FIELDS
        .iter()
        .zip(new_c.iter().zip(old_c.iter()))
        .filter(|(_, (n, o))| n != o)
        .map(|(f, _)| *f)
        .collect();
    Some(format!(
        "导入被拒绝：检测到设计宪章变更（{}）。宪章是用户主权的不变点，\n\
         AI 不得通过 import 改写 owl:Ontology 头部宪章两字段\n\
         （dcterms:description/skos:definition）。\n\
         请保持这两个字段与库内原文完全一致后再导入；\n\
         确需修改请让用户在「设计宪章」面板操作。",
        changed.join("、")
    ))
}

/// `export_ontology_ttl(ontology_iri)`：读生效摘要 / 或工作区全文（编辑环）。
fn export_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "export_ontology_ttl",
        "读取指定本体。\n\
         \n\
         **mode 参数（重要——影响内容来源与体积）**：\n\
         - mode=\"summary\"（默认）：返回**已发布生效版**的结构化摘要（类/属性清单）。\n\
           若本体尚未发布，会拒绝并提示用户先发布。未发布修改不会出现在摘要中。\n\
         - mode=\"full\"：返回**工作区草稿**完整 Turtle（供增量更新：改 → validate → import）。\n\
           可含未发布修改。体积大，用完即止，勿在同一任务内重复 full。\n\
         \n\
         增量更新示例：export_ontology_ttl(iri, mode=\"full\") → 改 Turtle → validate → import(overwrite=true)\n\
         查已发布结构：export_ontology_ttl(iri) 或 mode=\"summary\"\n\
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
                "mode": {
                    "type": "string",
                    "enum": ["summary", "full"],
                    "description": "summary=生效版结构摘要（默认）；full=工作区草稿全文（编辑用）",
                    "default": "summary",
                },
            },
            "required": ["ontology_iri"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                let mode = arg_str(&args, "mode");
                let mode = if mode.is_empty() { "summary" } else { mode.as_str() };
                if mode == "full" {
                    // 编辑环：读工作区草稿
                    return match store.export_ttl(&iri) {
                        Ok(ttl) => Ok(ToolOutput::text(format!(
                            "{ttl}\n\n\
                             ——（工作区草稿全文。此结果体积较大，已记入对话历史。\n\
                             查已发布结构请用 mode=\"summary\"。）"
                        ))),
                        Err(e) => Ok(ToolOutput::text(format!("错误：导出失败 - {e}"))),
                    };
                }
                // summary：消费生效快照；从未发布则拒绝
                let effective = match store.export_effective_ttl(&iri) {
                    Ok(ttl) => ttl,
                    Err(ontology_store::StoreError::Unpublished(_)) => {
                        return Ok(ToolOutput::text(
                            "错误：本体尚未发布。请先让用户发布版本后，再查看生效结构摘要。\n\
                             若需编辑当前草稿，请使用 mode=\"full\"。",
                        ));
                    }
                    Err(e) => {
                        return Ok(ToolOutput::text(format!("错误：导出失败 - {e}")));
                    }
                };
                let classes = ontology_store::ttl_edit::list_class_iris(&effective);
                let properties = ontology_store::ttl_edit::list_properties(&effective);
                let ontologies = store.list_ontologies().unwrap_or_default();
                let meta = ontologies.iter().find(|o| o.ontology_iri == iri);
                let active = meta.and_then(|o| o.active_revision).unwrap_or(0);
                let dirty = meta.map(|o| o.workspace_dirty).unwrap_or(false);
                let has_charter = meta.map(|o| o.has_charter).unwrap_or(false);
                let mut out = String::new();
                out.push_str(&format!(
                    "# 本体摘要（生效版）：{iri}\n\
                     生效版本：第 {active} 次发布 | 未发布修改：{dirty_zh} | 宪章：{ch}\n\n",
                    dirty_zh = if dirty { "有（摘要不含）" } else { "无" },
                    ch = if has_charter { "已填写" } else { "未填写" }
                ));
                out.push_str(&format!("## 类（{} 个）\n", classes.len()));
                for c in &classes {
                    let short = c.rsplit(['#', '/']).next().unwrap_or(c);
                    out.push_str(&format!("- {short}  <{c}>\n"));
                }
                out.push_str(&format!("\n## 属性（{} 个）\n", properties.len()));
                for p in &properties {
                    let short = p.iri.rsplit(['#', '/']).next().unwrap_or(&p.iri);
                    out.push_str(&format!(
                        "- {short} [{kind}]  <{iri}>\n",
                        kind = p.kind,
                        iri = p.iri,
                    ));
                    if !p.label.is_empty() {
                        out.push_str(&format!("    label: {label}\n", label = p.label));
                    }
                    if !p.comment.is_empty() {
                        out.push_str(&format!("    comment: {comment}\n", comment = p.comment));
                    }
                }
                out.push_str(
                    "\n（生效结构摘要完毕。编辑草稿用 mode=\"full\"；\
                     定向查询可用 query_ontology_sparql。）",
                );
                Ok(ToolOutput::text(out))
            })
        },
    )
}

/// `list_ontology_ttl()`：列出所有已存本体摘要。
fn list_ontology_ttl_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "list_ontology_ttl",
        "列出所有已存储的 W3C Turtle 本体（轻量摘要）。对齐 Palantir describe_ontology 的列表语义。\n\
         每条含：ontology_iri / has_charter（宪章是否已填）/ version（owl:versionInfo） / triple_count / updated_at（unix ms）。\n\
         按 updated_at 倒序（最近更新在前）。体积小，适合先调本工具看有哪些本体，\n\
         再调 export_ontology_ttl 或 query_ontology_sparql 钻取单个本体。\n\
         **has_charter=false 的本体缺设计宪章（多为手工导入）：开始迭代前先知会用户\n\
         补全宪章（在「设计宪章」面板），否则导入时会被宪章空检测闸门拒绝。**\n\
         无参数。",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        move |_args| {
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
    crate::dynamic_tool(
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
        move |args| {
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
// 版本管理（显式发布才留快照；回退切指针不新建版本）
// ════════════════════════════════════════════════════════════════

/// `publish_ontology_version(ontology_iri, label, description)`：
/// 发布当前内容为新版本（版本 = 里程碑）。写完整 TTL 快照 + 分配新 revision
/// + 设 active_revision + 写 changelog。返回新 revision 号。
fn publish_ontology_version_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "publish_ontology_version",
        "发布当前内容为新版本（版本 = 里程碑）。\\n\\\
         **import/append/编辑后内容脱离已发布状态（active_revision=NULL），需要显式发布才留版本快照**。\\n\\\
         发布时保存当前 TTL 全文快照 + 分配新 revision + 标记为当前启用版本 + 写 changelog。\\n\\\
         - label：版本标签（如 \\\"v1.0\\\"）；**留空自动生成 v0.0.N（N=发布序号），用户显式填则以用户为准**。\\n\\\
         - description：发布说明（如 \\\"采购本体-增加审批流\\\"）。\\n\
         返回新分配的 revision 号。超版本上限（默认 20）时最旧版本被清理（正在启用的版本不删）。\\n\\\
         建议在每次 import/append/编辑后、向用户交付前调用，让每个里程碑可回退。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": { "type": "string", "description": "本体 IRI（完整 URI）" },
                "label": { "type": "string", "description": "版本标签（如 v1.0）", "default": "" },
                "description": { "type": "string", "description": "发布说明", "default": "" },
            },
            "required": ["ontology_iri"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                let label = arg_str(&args, "label");
                let description = arg_str(&args, "description");
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                match store.publish_version(&iri, &label, &description, "agent", None) {
                    Ok(rev) => Ok(ToolOutput::text(format!(
                        "已发布版本 revision={rev}（label={}）\n当前内容已标记为已发布，后续修改需重新发布。",
                        if label.is_empty() { rev.to_string() } else { label }
                    ))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：发布版本失败 - {e}"))),
                }
            })
        },
    )
}

/// `list_ontology_versions(ontology_iri)`：版本历史（revision 降序）。
fn list_ontology_versions_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "list_ontology_versions",
        "列出本体版本历史（revision 降序，最新在前）。\\n\\\
         返回 revision/label/description/triple_count/创建时间/发布者，不含全文。\\n\\\
         配合 get_ontology_active_revision 判断当前启用哪个版本；配合 diff_ontology_versions 对比任意两版本。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": { "type": "string", "description": "本体 IRI（完整 URI）" },
            },
            "required": ["ontology_iri"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                match store.list_versions(&iri) {
                    Ok(versions) => {
                        if versions.is_empty() {
                            return Ok(ToolOutput::text(
                                "该本体还没有发布过版本（版本 = 显式发布的里程碑，中间过程只记 changelog）。",
                            ));
                        }
                        let lines: Vec<String> = versions
                            .iter()
                            .map(|v| {
                                format!(
                                    "rev{}  label={}  triples={}  at={}  by={}  desc={}",
                                    v.revision,
                                    v.label,
                                    v.triple_count,
                                    v.created_at,
                                    v.author,
                                    v.description,
                                )
                            })
                            .collect();
                        Ok(ToolOutput::text(format!(
                            "版本历史（共 {} 条）：\n{}",
                            versions.len(),
                            lines.join("\n")
                        )))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：列版本失败 - {e}"))),
                }
            })
        },
    )
}

/// `revert_ontology_version(ontology_iri, revision)`：回退到指定版本（切指针）。
fn revert_ontology_version_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "revert_ontology_version",
        "回退到指定历史版本（切指针，不新建版本）。\\n\\\
         取该版本 TTL 全文覆盖当前内容 + 标记为当前启用版本 + 写 changelog。\\n\\\
         回退可逆：所有已发布版本仍在，随时可再切回去。\\n\
         **影响当前内容**：调用前向用户说明将覆盖当前未发布修改。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": { "type": "string", "description": "本体 IRI（完整 URI）" },
                "revision": { "type": "integer", "description": "目标版本号（正数，list_ontology_versions 可见）" },
            },
            "required": ["ontology_iri", "revision"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                let rev = args.get("revision").and_then(|v| v.as_i64());
                let Some(rev) = rev else {
                    return Ok(ToolOutput::text("错误：revision 必须是正整数"));
                };
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                match store.revert_to_version(&iri, rev, "agent", None) {
                    Ok(()) => Ok(ToolOutput::text(format!(
                        "已回退到版本 rev{rev}（当前内容 = 该版本快照，未发布修改被覆盖）"
                    ))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：回退失败 - {e}"))),
                }
            })
        },
    )
}

/// `diff_ontology_versions(ontology_iri, rev_a, rev_b)`：两版本结构化对比。
fn diff_ontology_versions_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
        "diff_ontology_versions",
        "对比两个版本的结构化差异（类/属性增删改 + 本体头变化），不逐行文本 diff。\\n\\\
         rev 约定：0 = 当前工作区内容（可能未发布），正数 = 已发布版本 revision。\\n\
         输出：新增/删除/修改的类与属性（含 label/comment/domain/range 变化），本体头元信息变化提示。",
        json!({
            "type": "object",
            "properties": {
                "ontology_iri": { "type": "string", "description": "本体 IRI（完整 URI）" },
                "rev_a": { "type": "integer", "description": "基准版本（0=当前工作区）" },
                "rev_b": { "type": "integer", "description": "对比版本（0=当前工作区）" },
            },
            "required": ["ontology_iri", "rev_a", "rev_b"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let iri = arg_str(&args, "ontology_iri");
                let ra = args.get("rev_a").and_then(|v| v.as_i64());
                let rb = args.get("rev_b").and_then(|v| v.as_i64());
                let (Some(ra), Some(rb)) = (ra, rb) else {
                    return Ok(ToolOutput::text("错误：rev_a/rev_b 必须是整数（0=当前工作区）"));
                };
                if iri.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_iri 不能为空"));
                }
                match store.diff_versions(&iri, ra, rb) {
                    Ok(d) => {
                        let mut lines = Vec::new();
                        lines.push(format!("rev{} vs rev{}：", d.rev_a, d.rev_b));
                        if d.added_classes.is_empty()
                            && d.removed_classes.is_empty()
                            && d.modified_classes.is_empty()
                            && d.added_properties.is_empty()
                            && d.removed_properties.is_empty()
                            && d.modified_properties.is_empty()
                            && !d.ontology_meta_changed
                        {
                            lines.push("无差异".to_string());
                        } else {
                            for c in &d.added_classes {
                                lines.push(format!("+ 类 {c}"));
                            }
                            for c in &d.removed_classes {
                                lines.push(format!("- 类 {c}"));
                            }
                            for c in &d.modified_classes {
                                lines.push(format!(
                                    "~ 类 {} label: {} -> {} ; comment: {} -> {}",
                                    c.iri, c.before.label, c.after.label, c.before.comment, c.after.comment
                                ));
                            }
                            for p in &d.added_properties {
                                lines.push(format!("+ 属性 {} (kind={})", p.iri, p.kind));
                            }
                            for p in &d.removed_properties {
                                lines.push(format!("- 属性 {}", p.iri));
                            }
                            for p in &d.modified_properties {
                                lines.push(format!(
                                    "~ 属性 {} label: {} -> {} ; comment: {} -> {} ; domain: {} -> {} ; range: {} -> {}",
                                    p.iri, p.before.label, p.after.label, p.before.comment, p.after.comment,
                                    p.before.domain, p.after.domain, p.before.range, p.after.range
                                ));
                            }
                            if d.ontology_meta_changed {
                                lines.push("~ 本体头部元信息（本体自身 label/comment）有变化".to_string());
                            }
                        }
                        Ok(ToolOutput::text(lines.join("\n")))
                    }
                    Err(e) => Ok(ToolOutput::text(format!("错误：对比失败 - {e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 辅助
// ════════════════════════════════════════════════════════════════

/// `commit_ontology_ttl_change(ontology_iri, title, body, change_summary?, conversation_id?)`。
/// 提交一条本体变更日志（git commit message 式）。对齐 Palantir `commit_ontology_change`。
fn commit_ontology_ttl_change_tool(store: Arc<TtlStore>) -> DynamicTool {
    crate::dynamic_tool(
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
        move |args| {
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
    crate::dynamic_tool(
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
        move |args| {
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
    fn tool_group_exposes_twelve_tools() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        let tools = ontology_ttl_tools(store);
        // 5 核心 (validate/import/export/list/query) + 2 元数据 (commit_change/list_changelog)
        // + 1 证据源登记 (register_evidence_source，P1 加入)
        // + 4 版本管理 (publish/list/revert/diff，d0b08d15 加入)
        // set_charter 已按宪章用户主权移出 agent 工具（仅前端 CharterPanel 落库）
        assert_eq!(tools.len(), 12);
        assert!(!tools.iter().any(|t| t.name() == "set_ontology_ttl_charter"));
    }

    #[test]
    fn new_ontology_gate_rejects_uncreated_and_allows_existing() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        // 库内不存在 → 拒绝
        let reject = new_ontology_gate_ttl(&store, sample_ttl());
        assert!(reject.is_some(), "未建立的本体必须被拒绝");
        assert!(reject.unwrap().contains("尚未建立"));
        // 用户已通过向导建立（模拟：直接 import 骨架）→ 放行
        store.import_ttl(sample_ttl(), true).unwrap();
        assert!(new_ontology_gate_ttl(&store, sample_ttl()).is_none());
    }

    #[test]
    fn charter_guard_allows_first_import_and_noncharter_edits() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        // 新本体首次导入（库内不存在）→ 放行
        assert!(charter_guard(&store, sample_ttl(), true).is_none());
        store.import_ttl(sample_ttl(), true).unwrap();
        // 同 IRI 覆盖但未动宪章（sample 无宪章谓词）→ 放行
        assert!(charter_guard(&store, sample_ttl(), true).is_none());
        // overwrite=false（skip 无写入）→ 放行
        assert!(charter_guard(&store, sample_ttl(), false).is_none());
    }

    #[test]
    fn charter_empty_gate_blocks_manual_imported_ontology() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        // 模拟手工导入：直接 import_ttl 落库（不走宪章向导）→ 无 charter 行
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/manual#> .
:Manual a owl:Ontology ; rdfs:label "手工导入"@zh .
"#;
        let _r = store.import_ttl(ttl, false).unwrap();
        let (iri, _) = ontology_store::ttl::extract_ontology_meta(ttl, &mut Vec::new());
        assert!(iri.is_some(), "iri 提取失败");
        let iri = iri.unwrap();
        assert!(store.exists(&iri).unwrap(), "库内应有该本体: {iri}");
        // 宪章为空 → gate 拦截
        let reject = charter_empty_gate(&store, &iri);
        assert!(reject.is_some());
        assert!(reject.unwrap().contains("设计宪章为空"));
        // 用户补全宪章后 → 放行
        store
            .set_charter(
                &iri,
                &ontology_store::ttl::TtlCharter {
                    business_scenario: "采购业务".into(),
                    business_essence: "采购流程数字化".into(),
                    updated_by: "user".into(),
                    updated_at: 0,
                },
            )
            .unwrap();
        assert!(charter_empty_gate(&store, &iri).is_none());
    }

    #[test]
    fn charter_guard_rejects_charter_edit_via_import() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        // 注意：sample_ttl 前缀声明没有 dcterms，须先补声明否则 oxigraph 解析失败
        let with_charter = sample_ttl().replace(
            "@prefix : <https://example.org/ontology/test#> .",
            "@prefix : <https://example.org/ontology/test#> .\n@prefix dcterms: <http://purl.org/dc/terms/> .",
        )
        .replace(
            "    rdfs:label \"测试本体\"@zh ;",
            "    rdfs:label \"测试本体\"@zh ;\n    dcterms:description \"缺口根因分析场景\"@zh ;",
        );
        store.import_ttl(&with_charter, true).unwrap();
        // 改宪章字段 → 拒绝
        let modified = with_charter.replace("缺口根因分析场景", "改为营销分析场景");
        let reject = charter_guard(&store, &modified, true);
        assert!(reject.is_some(), "改宪章必须被拒绝");
        assert!(reject.unwrap().contains("设计宪章变更"));
        // 只改类 label（不动宪章）→ 放行
        let edited_class = with_charter.replace("供应商", "供应商主体");
        assert!(charter_guard(&store, &edited_class, true).is_none());
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
        // 4. export workspace
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

    #[test]
    fn export_effective_summary_refuses_unpublished_and_ignores_dirty() {
        let store = Arc::new(TtlStore::open_in_memory().unwrap());
        let iri = "https://example.org/ontology/test#TestOntology";
        store.import_ttl(sample_ttl(), true).unwrap();
        // 从未发布 → summary 路径拒绝
        let err = store.export_effective_ttl(iri).unwrap_err();
        assert!(matches!(
            err,
            ontology_store::StoreError::Unpublished(_)
        ));
        // full（工作区）仍可读
        assert!(store.export_ttl(iri).unwrap().contains("供应商"));

        store.publish_version(iri, "v1", "", "user", None).unwrap();
        let v1 = store.export_effective_ttl(iri).unwrap();
        assert!(v1.contains("供应商"));

        let dirty = sample_ttl().replace("供应商", "供应商v2");
        store.import_ttl(&dirty, true).unwrap();
        assert!(store.get_workspace_dirty(iri).unwrap());
        assert_eq!(store.get_active_revision(iri).unwrap(), Some(1));
        // 生效摘要仍为 v1；工作区已脏
        let effective = store.export_effective_ttl(iri).unwrap();
        assert!(effective.contains("供应商"));
        assert!(!effective.contains("供应商v2"));
        assert!(store.export_ttl(iri).unwrap().contains("供应商v2"));
    }
}
