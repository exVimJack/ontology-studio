//! 本体工具（把 ontology-store 的能力暴露给 agent，分两组语义）。
//!
//! ## 只读组（会话引用场景）—— `ontology_readonly_tools`
//!
//! 5 个轻量只读工具，对齐 Gaia MCP 的 read-view 钻取链路：
//!   - `describe_ontology`：本体 summary（OT 目录 + link/action 计数，不展开定义）
//!   - `list_object_types`：OT 名单（极轻量，4 字段）
//!   - `describe_object_type`：单个 OT 完整 schema（properties + outbound/inbound link api_names + action api_names）
//!   - `list_link_types`：link 名单（api_name/source/target/cardinality/fk，5 字段）
//!   - `describe_link_type`：单个 link 完整定义（含 directional/has_properties 派生字段）
//!
//! **会话模式只挂这 5 个**（决策：会话页面引用本体）。体积可控：summary ~2-5KB，
//! 单 OT drill-in ~1-3KB，大本体不撑爆上下文（vs export 完整 payload 100KB+）。
//! 工具 description 明确告诫模型「不要在会话中调 export_ontology 取完整 payload」。
//!
//! ## 建模组（冷启动/增量更新场景）—— `ontology_modeling_tools`
//!
//! 3 个写工具，会话模式不挂（体积大 + 会话场景不需要写库）：
//!   - `export_ontology`：导出完整 write-view payload（建模/增量更新用）
//!   - `preview_ontology_import`：dry-run 预演导入
//!   - `import_ontology`：执行导入（DAG 顺序落库）
//!
//! 工作流（对齐 Gaia service）：
//!   - 冷启动：preview(payload) → import(payload)
//!   - 增量：export(ontology) → 改 payload → preview → import(带 overwrite_object_types)
//!
//! 与 `federation_tools` 同构（闭包捕获 `Arc<OntologyStore>`），走同一条
//! `ToolServerHandle.add_dynamic_tool()` 注入路径。

use std::sync::Arc;

use ontology_store::{ImportRequest, OntologyPayload, OntologyStore};
use rig::tool::{DynamicTool, ToolOutput};
use serde_json::json;

// ════════════════════════════════════════════════════════════════
// 只读组（会话引用场景）
// ════════════════════════════════════════════════════════════════

/// 构造 5 个只读本体工具，挂到 agent（会话模式专用）。
///
/// 对齐 Gaia MCP read-view 钻取链路：summary → list → describe。
/// 体积可控，大本体不撑爆上下文。会话模式不挂建模组（export/preview/import）。
pub fn ontology_readonly_tools(store: Arc<OntologyStore>) -> Vec<DynamicTool> {
    vec![
        describe_ontology_tool(store.clone()),
        list_object_types_tool(store.clone()),
        describe_object_type_tool(store.clone()),
        list_link_types_tool(store.clone()),
        describe_link_type_tool(store),
    ]
}

/// `describe_ontology(ontology_api_name)`：本体 summary（轻量目录视图）。
fn describe_ontology_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "describe_ontology",
        "获取本体的轻量目录视图（summary）。返回每个 ObjectType 的 api_name/display_name/primary_key/storage_type/property_count，\u{a}
         以及 link_type/action_type 的计数（不展开定义）。体积小（~2-5KB），适合会话中快速了解本体有哪些实体。\u{a}
         **返回同时携带 `charter`（本体设计宪章，不变点）：business_scenario（业务场景）/ business_essence（业务本质）/\u{a}
         这是「向 AI 说明业务本质」的结构化业务认知——\u{a}
         增量更新本体前必读 charter，确保变更不违背建模初衷和业务约束；除非用户明确要求调整 charter，否则不要修改它。**\u{a}
         需要某 OT 的完整 schema 时调 describe_object_type，需要某 link 详情时调 describe_link_type。\u{a}
         不要调 export_ontology 获取完整 payload——那是建模专用，体积可达 100KB+。\u{a}
         参数 ontology_api_name 是本体的 PascalCase 标识名（如 SupplyChain）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": {
                    "type": "string",
                    "description": "本体 api_name（PascalCase，如 SupplyChain）",
                },
            },
            "required": ["ontology_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                if ont.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 不能为空"));
                }
                match store.describe_ontology_summary(&ont) {
                    Ok(s) => Ok(ToolOutput::json(json!(s))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：获取本体 summary 失败 - {e}"))),
                }
            })
        },
    )
}

/// `list_object_types(ontology_api_name)`：OT 名单（极轻量）。
fn list_object_types_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "list_object_types",
        "列出本体中所有 ObjectType 的名单（极轻量，仅 api_name/display_name/description/storage_type）。\u{a}
         比 describe_ontology 的 summary 更轻——不含 primary_key/property_count。\u{a}
         用于「只想知道有哪些 OT」的场景。需要某 OT 详情调 describe_object_type。\u{a}
         参数 ontology_api_name 是本体的 PascalCase 标识名。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": {
                    "type": "string",
                    "description": "本体 api_name（PascalCase）",
                },
            },
            "required": ["ontology_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                if ont.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 不能为空"));
                }
                match store.list_object_types(&ont) {
                    Ok(ots) => Ok(ToolOutput::json(json!(ots))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：列出 OT 失败 - {e}"))),
                }
            })
        },
    )
}

/// `describe_object_type(ontology_api_name, object_type_api_name)`：单个 OT 完整 schema。
fn describe_object_type_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "describe_object_type",
        "获取单个 ObjectType 的完整 schema（对齐 Gaia describe_object_type）。返回：\u{a}
         properties[]（7 字段 read-view：api_name/display_name/data_type/is_primary_key/nullable/filterable/sortable） + outbound_links[]（以本 OT 为 source 的 link api_name 列表）+\u{a}
         inbound_links[]（以本 OT 为 target 的 link api_name 列表） + actions[]（作用于本 OT 的 action api_name 列表）。\u{a}
         links/actions 是 api_name 字符串列表（不是完整对象）——需要某 link 详情调 describe_link_type。\u{a}
         体积可控（单 OT 通常 1-3KB），适合会话中按需钻取某 OT 的详情。\u{a}
         这是会话引用场景的主工具——先调 describe_ontology 拿 OT 目录，再调本工具看具体 OT。\u{a}
         参数：ontology_api_name（本体名，PascalCase）、object_type_api_name（OT 名，PascalCase）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": {
                    "type": "string",
                    "description": "本体 api_name（PascalCase）",
                },
                "object_type_api_name": {
                    "type": "string",
                    "description": "ObjectType api_name（PascalCase，先调 list_object_types 或 describe_ontology 获取）",
                },
            },
            "required": ["ontology_api_name", "object_type_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                let ot = arg_str(&args, "object_type_api_name");
                if ont.is_empty() || ot.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 和 object_type_api_name 都不能为空"));
                }
                match store.describe_object_type(&ont, &ot) {
                    Ok(full) => Ok(ToolOutput::json(json!(full))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：获取 OT 详情失败 - {e}"))),
                }
            })
        },
    )
}

/// `list_link_types(ontology_api_name)`：link 名单。
fn list_link_types_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "list_link_types",
        "列出本体中所有 LinkType（关系类型）的名单。每条含 api_name/source_object_type/\u{a}
         target_object_type/cardinality/foreign_key_property_api_name（5 字段，对齐 Gaia）。\u{a}
         用于「有哪些关系」的概览。需要某 link 完整定义调 describe_link_type。\u{a}
         参数 ontology_api_name 是本体的 PascalCase 标识名。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": {
                    "type": "string",
                    "description": "本体 api_name（PascalCase）",
                },
            },
            "required": ["ontology_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                if ont.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 不能为空"));
                }
                match store.list_link_types(&ont) {
                    Ok(links) => Ok(ToolOutput::json(json!(links))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：列出 link 失败 - {e}"))),
                }
            })
        },
    )
}

/// `describe_link_type(ontology_api_name, link_api_name)`：单个 link 完整定义。
fn describe_link_type_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "describe_link_type",
        "获取单个 LinkType 的完整定义（对齐 Gaia describe_link_type）。返回 9 字段：\u{a}
         api_name/display_name/description/source_object_type/target_object_type/\u{a}
         foreign_key_property_api_name/cardinality/directional/has_properties。\u{a}
         - directional：link 是否有方向性（反向遍历是否有意义），固定 true\u{a}
         - has_properties：link 是否自带属性，固定 false\u{a}
         适合「只关心一条关系」的细粒度场景。\u{a}
         参数：ontology_api_name（本体名）、link_api_name（link 名，camelCase，先调 list_link_types 获取）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": {
                    "type": "string",
                    "description": "本体 api_name（PascalCase）",
                },
                "link_api_name": {
                    "type": "string",
                    "description": "LinkType api_name（camelCase，先调 list_link_types 获取）",
                },
            },
            "required": ["ontology_api_name", "link_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                let link = arg_str(&args, "link_api_name");
                if ont.is_empty() || link.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 和 link_api_name 都不能为空"));
                }
                match store.describe_link_type(&ont, &link) {
                    Ok(l) => Ok(ToolOutput::json(json!(l))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：获取 link 详情失败 - {e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 建模组（冷启动/增量更新，会话模式不挂）
// ════════════════════════════════════════════════════════════════

/// 构造 3 个建模工具。
/// charter 写入已从 agent 工具移除（宪章用户主权：仅前端面板落库）。会话模式不挂——体积大 + 会话场景不需要写库。
///
/// 保留旧名 `ontology_tools` 供 OntologyView 建模页面 / 历史调用方使用。
pub fn ontology_tools(store: Arc<OntologyStore>) -> Vec<DynamicTool> {
    ontology_modeling_tools(store)
}

/// 构造 3 个建模工具 + 1 个证据源登记工具。
pub fn ontology_modeling_tools(store: Arc<OntologyStore>) -> Vec<DynamicTool> {
    vec![
        export_ontology_tool(store.clone()),
        register_evidence_source_tool(store.clone()),
        preview_ontology_import_tool(store.clone()),
        import_ontology_tool(store),
    ]
}

/// `register_evidence_source(kind, title, content, ontology_api_name?, origin_ref?)`：
/// 登记证据源（docs/EVIDENCE-CHAIN.md L1）。返回 source_id。
fn register_evidence_source_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "register_evidence_source",
        "登记建模证据源（本体建模证据链，docs/EVIDENCE-CHAIN.md）。\n\
         **建模前必须先调用**：把用户输入的建模依据（场景描述/文档片段/DB schema）登记为可引用的源，\n\
         返回 source_id。之后 payload 里每个 ObjectType/Property/LinkType 都必须带 evidence 字段：\n\
         [{\"source_id\":\"src-xxx\",\"quote\":\"原文逐字摘录（≤200字）\",\"role\":\"direct\"}]\n\
         - role=\"direct\"：quote 必须是源 content 的逐字原文（机械校验，改写/臆想会被阻断）\n\
         - role=\"inferred\"：组合推断，多段原文用 \" ⟪…⟫ \" 拼接，需用户确认\n\
         - 确属抽象设计（如纯结构中间层）的元素可省略 evidence，但 strict 模式会被阻断——\n\
           此时请向用户说明并请求确认\n\
         同内容重复登记幂等（返回同一 source_id）。kind：scenario（会话场景描述）| document（文档片段）\n\
         | db_table | db_column。",
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
                    "description": "人类可读出处（如：会话场景描述 2026-09-01 / 需求文档.docx 第3节 / erp.orders 表）",
                },
                "content": {
                    "type": "string",
                    "description": "源文本全文（登记时刻快照，之后不可变；后续 evidence.quote 从中逐字摘录）",
                },
                "ontology_api_name": {
                    "type": "string",
                    "description": "归属本体 api_name（可空=全局共享）",
                    "default": "",
                },
                "origin_ref": {
                    "type": "string",
                    "description": "关联既有实体 id（document_id/dataset_id/消息 id，可空）",
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
                         后续 payload 中每个 ObjectType/Property/LinkType 的 evidence 字段引用此 id，\n\
                         quote 必须从 content 逐字摘录（≤200字），否则落库会被阻断。"
                    ))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：登记证据源失败 - {e}"))),
                }
            })
        },
    )
}

/// `export_ontology(ontology_api_name)`：导出本体完整定义。
fn export_ontology_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "export_ontology",
        "导出指定本体的完整定义（ObjectType + Property + LinkType + ActionType + Dataset + DataSource + ObjectTypeGroup）。\
         返回 OntologyPayload JSON——这是本体的 write-view 快照，包含所有实体定义。\
         增量更新场景：先调此工具拿到当前本体 JSON，修改需要变更的实体（增/改/删），\
         再用 preview_ontology_import 校验、import_ontology 落库。\
         参数 ontology_api_name 是本体的 PascalCase 标识名（如 SupplyChain）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": {
                    "type": "string",
                    "description": "本体 api_name（PascalCase，如 SupplyChain）",
                },
            },
            "required": ["ontology_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = args
                    .get("ontology_api_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if ont.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 不能为空"));
                }
                match store.export(&ont) {
                    Ok(payload) => Ok(ToolOutput::json(json!(payload))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：导出本体失败 - {e}"))),
                }
            })
        },
    )
}

/// `preview_ontology_import(payload, overwrite_object_types?, overwrite_data_sources?)`：dry-run 预演导入。
fn preview_ontology_import_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "preview_ontology_import",
        "预演导入本体（dry-run，不写库）。返回 ImportPreview：\
         per-entity 预测（create/skip/overwrite/fail）+ 引用完整性 errors（阻断性）+ warnings（非阻断）。\
         **落库前必须先调此工具**——errors 非空时不要调 import_ontology，先修正 payload。\
         warnings 非空时可继续导入（如占位符凭据、缺 backing_mapping），但建议先处理。\
         **证据闸门（strict，docs/EVIDENCE-CHAIN.md）**：payload 中每个 ObjectType/Property/LinkType \
         必须带 evidence 数组（先 register_evidence_source 登记源，再逐元素附 evidence）；\
         无 evidence 的元素会被阻断，quote 与源原文不符（改写/臆想）也会被阻断——\
         错误信息会附带源原文片段，请据此重新逐字摘录，不要自行改写。\
         参数 payload 是 OntologyPayload JSON（完整本体定义），\
         overwrite_object_types 是选择覆写的 ObjectType api_name 列表\
         （未列入的同名 OT 默认 skip，保护已有成果）。\
         overwrite_data_sources 是选择覆写的 DataSource api_name 列表\
         （未列入的同名 DS 默认 skip；列入会用 payload 的 connector_config 等 UPDATE 已有记录，\
         用于从脱敏 *** 升级到真实凭据等场景）。",
        json!({
            "type": "object",
            "properties": {
                "payload": {
                    "type": "object",
                    "description": "OntologyPayload JSON（本体完整定义）",
                },
                "overwrite_object_types": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "选择覆写的 ObjectType api_name 列表（未列入的同名 OT 默认 skip）",
                    "default": [],
                },
                "overwrite_data_sources": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "选择覆写的 DataSource api_name 列表（未列入的同名 DS 默认 skip；列入会 UPDATE 已有记录）",
                    "default": [],
                },
            },
            "required": ["payload"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let payload_val = args.get("payload").cloned().unwrap_or(json!({}));
                let payload: OntologyPayload = match serde_json::from_value(payload_val) {
                    Ok(p) => p,
                    Err(e) => return Ok(ToolOutput::text(format!("错误：payload 解析失败 - {e}"))),
                };
                let overwrite: Vec<String> = args
                    .get("overwrite_object_types")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let overwrite_ds: Vec<String> = args
                    .get("overwrite_data_sources")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                // 证据闸门：agent 路径硬编码 strict（docs/EVIDENCE-CHAIN.md）——LLM 无法绕过
                let req = ImportRequest { payload, overwrite_object_types: overwrite, overwrite_data_sources: overwrite_ds, evidence_mode: "strict".to_string() };
                match store.preview_import(&req) {
                    Ok(preview) => Ok(ToolOutput::json(json!(preview))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：预演导入失败 - {e}"))),
                }
            })
        },
    )
}

/// `import_ontology(payload, overwrite_object_types?)`：执行导入。
fn import_ontology_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "import_ontology",
        "执行本体导入（落库）。DAG 顺序：Ontology → DataSource → Dataset → ObjectType(+Property) → Link → Action → Group。\
         best-effort：单个实体失败记入 errors 继续往下，不整体回滚。\
         **调用前必须先 preview_ontology_import 确认无 errors**。\
         返回 ImportResult：per-entity 状态（created/skipped/overwritten/failed）+ errors 列表。\
         证据闸门与 preview 同规则（strict）：无 evidence 或 quote 不符会整体拒绝落库。\
         参数同 preview_ontology_import。\
         **宪章先行闸门**（决策 24）：目标 api_name 在库内不存在时拒绝导入——\
         本体创建必须由用户在「新建本体」向导完成设计宪章；冷启动时请先\
         输出建模思路 + 宪章要点，指引用户完成向导，本体建立后再导入。\
         增量更新：改 payload 里几个实体 + 把要覆写的 OT 列入 overwrite_object_types 重新导入即可，\
         未列入的同名 OT 保持不变（保护已有成果）。\
         DataSource 覆写：把要覆写的 DS 列入 overwrite_data_sources，会用 payload 的 connector_config 等\
         UPDATE 已有记录（用于从脱敏 *** 升级到真实凭据等场景）。",
        json!({
            "type": "object",
            "properties": {
                "payload": {
                    "type": "object",
                    "description": "OntologyPayload JSON（本体完整定义）",
                },
                "overwrite_object_types": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "选择覆写的 ObjectType api_name 列表（未列入的同名 OT 默认 skip）",
                    "default": [],
                },
                "overwrite_data_sources": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "选择覆写的 DataSource api_name 列表（未列入的同名 DS 默认 skip；列入会 UPDATE 已有记录）",
                    "default": [],
                },
            },
            "required": ["payload"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let payload_val = args.get("payload").cloned().unwrap_or(json!({}));
                let payload: OntologyPayload = match serde_json::from_value(payload_val) {
                    Ok(p) => p,
                    Err(e) => return Ok(ToolOutput::text(format!("错误：payload 解析失败 - {e}"))),
                };
                // 宪章先行闸门（决策 24）：库内不存在的 api_name 拒绝导入——
                // 本体创建 = 用户在宪章向导确认宪章，AI 不得从 0 到 1 创建本体。
                if let Some(reject) = new_ontology_gate_palantir(&store, &payload.api_name) {
                    return Ok(ToolOutput::text(reject));
                }
                // 宪章空检测：手工导入的本体可能无宪章，agent 迭代前须让用户补全
                if let Some(reject) = charter_empty_gate_palantir(&store, &payload.api_name) {
                    return Ok(ToolOutput::text(reject));
                }
                let overwrite: Vec<String> = args
                    .get("overwrite_object_types")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let overwrite_ds: Vec<String> = args
                    .get("overwrite_data_sources")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let req = ImportRequest { payload, overwrite_object_types: overwrite, overwrite_data_sources: overwrite_ds, evidence_mode: "strict".to_string() };
                match store.import(&req) {
                    Ok(result) => Ok(ToolOutput::json(json!(result))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：导入失败 - {e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 辅助
// ════════════════════════════════════════════════════════════════

/// 宪章先行闸门（决策 24）：目标 api_name 在库内不存在时拒绝导入。
///
/// 本体创建的唯一合法路径 = 用户在「新建本体」宪章向导中确认宪章
/// （前端 IPC create_ontology_with_charter）。AI 只能在已建立的本体上增量建模。
/// 宪章空检测（决策 24 延伸）：手工导入的本体可能没有宪章（绕过了向导）。
/// agent 迭代 schema 前须让用户补全——没有业务意图基线无法校验变更方向。
fn charter_empty_gate_palantir(store: &OntologyStore, api_name: &str) -> Option<String> {
    match store.get_charter(api_name) {
        Ok(c)
            if c.business_scenario.trim().is_empty()
                && c.business_essence.trim().is_empty()
                =>
        {
            Some(format!(
                "导入被拒绝：本体 {api_name} 的设计宪章为空（该本体可能由手工导入建立，\n\
                 未经过宪章向导）。\n\
                 宪章是建模的业务意图基线（决策 24）。请先知会用户补全宪章：在「设计宪章」\n\
                 面板填写业务场景/业务本质/两字段（你可先起草建议供用户\n\
                 修改确认，但写入必须由用户操作），补全后再执行导入。"
            ))
        }
        Ok(_) => None,
        Err(e) => Some(format!("错误：检查宪章失败 - {e}")),
    }
}

fn new_ontology_gate_palantir(store: &OntologyStore, api_name: &str) -> Option<String> {
    match store.exists(api_name) {
        Ok(true) => None,
        Ok(false) => Some(format!(
            "导入被拒绝：本体 {api_name} 尚未建立。宪章先行（ARCHITECTURE.md 决策 24）：\
             设计宪章是本体建模的前提，本体创建必须由用户完成。\n\
             请指引用户：在「Palantir标准」页点「新建本体」，填写业务场景/业务本质/\
             两字段（可先用 AI 起草建议预填）并保存；\n\
             本体建立后，把你的建模思路和宪章要点作为参考给用户，再在新本体上\
             执行增量导入。在此之前不要尝试任何导入。"
        )),
        Err(e) => Some(format!("错误：检查本体是否存在失败 - {e}")),
    }
}

/// 从工具参数里取字符串字段，缺失或非字符串返回空串。
///
/// 只读工具的 ontology_api_name/object_type_api_name 等参数都是必填字符串，
/// 统一用此辅助取值 + 空串校验，避免每个工具重复写 `args.get(...).and_then(...).unwrap_or("")`。
/// `args` 是 rig DynamicTool 闭包传入的 `serde_json::Value`（object 形态）。
fn arg_str(args: &serde_json::Value, key: &str) -> String {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

// ════════════════════════════════════════════════════════════════
// 实体级删除工具（import 只能 upsert，删除走这组工具）
// ════════════════════════════════════════════════════════════════

/// 构造 5 个实体删除工具。
pub fn ontology_delete_tools(store: Arc<OntologyStore>) -> Vec<DynamicTool> {
    vec![
        delete_object_type_tool(store.clone()),
        delete_link_type_tool(store.clone()),
        delete_action_type_tool(store.clone()),
        delete_dataset_tool(store.clone()),
        delete_data_source_tool(store),
    ]
}

/// `delete_object_type(ontology_api_name, object_type_api_name)`。
fn delete_object_type_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "delete_object_type",
        "删除本体中的一个 ObjectType（物理删除）。\
         连带删除：它的所有 properties、所在分组成员、\
         引用它（source 或 target）的 LinkType、影响它的 ActionType。\
         幂等：不存在时返回 deleted=false。\
         **删除前先向用户确认**（连删范围较大时尤其要说明）。\
         参数均为 PascalCase api_name。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
                "object_type_api_name": { "type": "string", "description": "要删除的 ObjectType api_name（PascalCase）" },
            },
            "required": ["ontology_api_name", "object_type_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let (ont, ot) = match (arg_str(&args, "ontology_api_name"), arg_str(&args, "object_type_api_name")) {
                    (o, t) if !o.is_empty() && !t.is_empty() => (o, t),
                    _ => return Ok(ToolOutput::text("错误：ontology_api_name 和 object_type_api_name 不能为空")),
                };
                match store.delete_object_type(&ont, &ot) {
                    Ok((true, links, actions)) => Ok(ToolOutput::text(format!(
                        "已删除 ObjectType '{ot}'（连带 {links} 个链接、{actions} 个动作）"
                    ))),
                    Ok((false, _, _)) => Ok(ToolOutput::text(format!("未删除：ObjectType '{ot}' 不存在"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：删除 ObjectType 失败 - {e}"))),
                }
            })
        },
    )
}

/// `delete_link_type(ontology_api_name, link_api_name)`。
fn delete_link_type_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "delete_link_type",
        "删除本体中的一个 LinkType（关系类型）。幂等：不存在返回 deleted=false。\
         参数均为 api_name（本体 PascalCase，链接 camelCase）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
                "link_api_name": { "type": "string", "description": "要删除的 LinkType api_name（camelCase）" },
            },
            "required": ["ontology_api_name", "link_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let (ont, link) = match (arg_str(&args, "ontology_api_name"), arg_str(&args, "link_api_name")) {
                    (o, l) if !o.is_empty() && !l.is_empty() => (o, l),
                    _ => return Ok(ToolOutput::text("错误：ontology_api_name 和 link_api_name 不能为空")),
                };
                match store.delete_link_type(&ont, &link) {
                    Ok(true) => Ok(ToolOutput::text(format!("已删除 LinkType '{link}'"))),
                    Ok(false) => Ok(ToolOutput::text(format!("未删除：LinkType '{link}' 不存在"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：删除 LinkType 失败 - {e}"))),
                }
            })
        },
    )
}

/// `delete_action_type(ontology_api_name, action_api_name)`。
fn delete_action_type_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "delete_action_type",
        "删除本体中的一个 ActionType。幂等：不存在返回 deleted=false。\
         参数均为 api_name（本体 PascalCase，动作 camelCase）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
                "action_api_name": { "type": "string", "description": "要删除的 ActionType api_name（camelCase）" },
            },
            "required": ["ontology_api_name", "action_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let (ont, act) = match (arg_str(&args, "ontology_api_name"), arg_str(&args, "action_api_name")) {
                    (o, a) if !o.is_empty() && !a.is_empty() => (o, a),
                    _ => return Ok(ToolOutput::text("错误：ontology_api_name 和 action_api_name 不能为空")),
                };
                match store.delete_action_type(&ont, &act) {
                    Ok(true) => Ok(ToolOutput::text(format!("已删除 ActionType '{act}'"))),
                    Ok(false) => Ok(ToolOutput::text(format!("未删除：ActionType '{act}' 不存在"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：删除 ActionType 失败 - {e}"))),
                }
            })
        },
    )
}

/// `delete_dataset(ontology_api_name, dataset_api_name)`。
fn delete_dataset_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "delete_dataset",
        "删除指定本体下的一个 Dataset（按本体隔离，决策 10 修订）。\
         被 ObjectType（backing 绑定）或同本体内 Dataset（view 派生）引用时拒绝，\
         错误信息会列出引用方——需先解除引用（改 OT 绑定或删引用实体）再删。\
         幂等：不存在返回 deleted=false。参数为本体 PascalCase + snake_case api_name。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
                "dataset_api_name": { "type": "string", "description": "要删除的 Dataset api_name（snake_case）" },
            },
            "required": ["ontology_api_name", "dataset_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                let ds = arg_str(&args, "dataset_api_name");
                if ont.is_empty() || ds.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 和 dataset_api_name 不能为空"));
                }
                match store.delete_dataset(&ont, &ds) {
                    Ok(true) => Ok(ToolOutput::text(format!("已删除 Dataset '{ds}'"))),
                    Ok(false) => Ok(ToolOutput::text(format!("未删除：Dataset '{ds}' 不存在"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：删除 Dataset 失败 - {e}"))),
                }
            })
        },
    )
}

/// `delete_data_source(ontology_api_name, data_source_api_name)`。
fn delete_data_source_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "delete_data_source",
        "删除指定本体下的一个 DataSource（按本体隔离，决策 10 修订）。\
         被同本体内 Dataset 引用时拒绝，错误信息会列出引用方——需先删或解绑相关 Dataset 再删。\
         幂等：不存在返回 deleted=false。参数为本体 PascalCase + snake_case api_name。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
                "data_source_api_name": { "type": "string", "description": "要删除的 DataSource api_name（snake_case）" },
            },
            "required": ["ontology_api_name", "data_source_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                let src = arg_str(&args, "data_source_api_name");
                if ont.is_empty() || src.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 和 data_source_api_name 不能为空"));
                }
                match store.delete_data_source(&ont, &src) {
                    Ok(true) => Ok(ToolOutput::text(format!("已删除 DataSource '{src}'"))),
                    Ok(false) => Ok(ToolOutput::text(format!("未删除：DataSource '{src}' 不存在"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：删除 DataSource 失败 - {e}"))),
                }
            })
        },
    )
}

// ════════════════════════════════════════════════════════════════
// 变更日志工具（git commit log 式）
// ════════════════════════════════════════════════════════════════

/// 构造 2 个变更日志工具。
pub fn ontology_changelog_tools(store: Arc<OntologyStore>) -> Vec<DynamicTool> {
    vec![
        commit_ontology_change_tool(store.clone()),
        list_ontology_changelog_tool(store),
    ]
}

/// `commit_ontology_change(ontology_api_name, title, body, change_summary?, conversation_id?)`。
fn commit_ontology_change_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "commit_ontology_change",
        "提交一条本体变更日志（git commit message 式），记录本次本体变更的设计意图。\
         **每次 import_ontology 或 delete_* 之后必须调用**——把本次改动的「为什么」和「整体设计」\
         落成可回溯记录，方便以后复现思路。\
         体积限制：title+body ≤500 字符，整条（含 change_summary）≤1K 字符。超出会被拒。\
         - title：一句话标题（如「新增 model_instance 状态机」）。\
         - body：设计说明正文——为什么改、整体思路、影响范围、以后怎么复现。\
         - change_summary：可选，机器可读 JSON 摘要（如 {\"created\":[...],\"deleted\":[...],\"modified\":[...]}），\
           可从 import_ontology 的返回结果归纳。省略时存空对象。\
         - conversation_id：可选，来源会话 id（用于前端关联跳转）。\
         返回分配的 revision 序号（本体内从 1 递增）。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
                "title": { "type": "string", "description": "一句话标题（≤500 chars 合计约束）" },
                "body": { "type": "string", "description": "设计说明正文", "default": "" },
                "change_summary": { "type": "string", "description": "机器可读 JSON 摘要", "default": "{}" },
                "conversation_id": { "type": "string", "description": "来源会话 id（可选）" },
            },
            "required": ["ontology_api_name", "title"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                let title = arg_str(&args, "title");
                if ont.is_empty() || title.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 和 title 不能为空"));
                }
                let body = arg_str(&args, "body");
                let summary = {
                    let s = arg_str(&args, "change_summary");
                    if s.is_empty() { "{}".to_string() } else { s }
                };
                let conv_id = args.get("conversation_id").and_then(|v| v.as_str());
                match store.commit_change(&ont, &title, &body, &summary, conv_id, "agent") {
                    Ok(rev) => Ok(ToolOutput::text(format!("已记录变更日志 revision={rev}"))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：记录变更日志失败 - {e}"))),
                }
            })
        },
    )
}

/// `list_ontology_changelog(ontology_api_name)`。
fn list_ontology_changelog_tool(store: Arc<OntologyStore>) -> DynamicTool {
    crate::dynamic_tool(
        "list_ontology_changelog",
        "列出本体的变更历史（git commit log 式，按 revision 倒序，最新在前）。\
         每条含 revision/title/body/change_summary/conversation_id/author/created_at。\
         供回溯本体的演进过程与设计决策。",
        json!({
            "type": "object",
            "properties": {
                "ontology_api_name": { "type": "string", "description": "本体 api_name（PascalCase）" },
            },
            "required": ["ontology_api_name"],
            "additionalProperties": false,
        }),
        move |args| {
            let store = store.clone();
            Box::pin(async move {
                let ont = arg_str(&args, "ontology_api_name");
                if ont.is_empty() {
                    return Ok(ToolOutput::text("错误：ontology_api_name 不能为空"));
                }
                match store.list_changelog(&ont) {
                    Ok(logs) => Ok(ToolOutput::json(json!(logs))),
                    Err(e) => Ok(ToolOutput::text(format!("错误：查询变更历史失败 - {e}"))),
                }
            })
        },
    )
}

#[cfg(test)]
mod charter_first_gate_tests {
    use super::*;
    use ontology_store::OntologyCharter;

    #[test]
    fn new_ontology_gate_rejects_uncreated_and_allows_existing() {
        let store = OntologyStore::open_in_memory().unwrap();
        // 库内不存在 → 拒绝
        let reject = new_ontology_gate_palantir(&store, "TestOntology");
        assert!(reject.is_some(), "未建立的本体必须被拒绝");
        assert!(reject.unwrap().contains("尚未建立"));
        // 用户已通过向导建立（create_with_charter 仅前端可达，此处模拟其效果）
        store
            .create_with_charter(
                "TestOntology",
                "测试本体",
                "",
                &OntologyCharter::default(),
            )
            .unwrap();
        assert!(new_ontology_gate_palantir(&store, "TestOntology").is_none());
    }
}
