//! ontology_tools 单元测试。
//!
//! 验证两组工具的构造正确性：
//!   - `ontology_readonly_tools`：5 个只读 drill-in（会话模式挂这组）
//!   - `ontology_modeling_tools`：3 个建模工具（会话模式不挂）
//!
//! DynamicTool 的 `execute` 是 pub(crate)，无法在集成测试里直接调闭包，
//! 这里验证工具名 + 数量 + description 含关键告诫文案（如「不要调 export_ontology」）。
//! store 层的查询正确性由 ontology-store 的 25 个单测覆盖。

use std::sync::Arc;

use agent_core::ontology_tools::{ontology_modeling_tools, ontology_readonly_tools};
use ontology_store::OntologyStore;

/// 构造一个内存 store + 最小本体（1 OT，无 link/action）供工具构造。
fn store_with_min_ontology() -> Arc<OntologyStore> {
    let store = Arc::new(OntologyStore::open_in_memory().unwrap());
    let payload = ontology_store::OntologyPayload {
        api_name: "TestOnt".to_string(),
        display_name: "测试本体".to_string(),
        description: "测试用".to_string(),
        object_types: vec![ontology_store::ObjectTypeDef {
            api_name: "Entity".to_string(),
            display_name: "实体".to_string(),
            description: String::new(),
            primary_key: "entityId".to_string(),
            title_property: String::new(),
            storage_type: "MANAGED".to_string(),
            visibility: "NORMAL".to_string(),
            capabilities: ontology_store::Capabilities::default(),
            properties: vec![ontology_store::PropertyDef {
                api_name: "entityId".to_string(),
                display_name: "ID".to_string(),
                description: String::new(),
                data_type: "STRING".to_string(),
                searchable: true,
                is_primary_key: Some(true),
                is_title_property: None,
                backing_mapping: None,
                vector_config: None,
                confidence: None,
            }],
            links: vec![],
            confidence: None,
        }],
        action_types: vec![],
        datasets: vec![],
        data_sources: vec![],
        object_type_groups: vec![],
    };
    store
        .import(&ontology_store::ImportRequest {
            payload,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        })
        .unwrap();
    store
}

#[test]
fn readonly_tools_constructs_exactly_five() {
    let store = store_with_min_ontology();
    let tools = ontology_readonly_tools(store);
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names.len(), 5, "readonly group must have exactly 5 tools");
    assert_eq!(names, vec![
        "describe_ontology",
        "list_object_types",
        "describe_object_type",
        "list_link_types",
        "describe_link_type",
    ]);
}

#[test]
fn modeling_tools_constructs_four() {
    let store = store_with_min_ontology();
    let tools = ontology_modeling_tools(store);
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert_eq!(names.len(), 4, "modeling group must have exactly 4 tools (export/preview/import + set_ontology_charter)");
    assert_eq!(names, vec![
        "export_ontology",
        "preview_ontology_import",
        "import_ontology",
        "set_ontology_charter",
    ]);
}

#[test]
fn readonly_and_modeling_groups_disjoint() {
    // 两组工具名不能重叠（会话模式挂只读组，建模模式挂建模组，互不污染）
    let store = store_with_min_ontology();
    let readonly_tools = ontology_readonly_tools(store.clone());
    let modeling_tools = ontology_modeling_tools(store);
    let readonly: std::collections::HashSet<&str> =
        readonly_tools.iter().map(|t| t.name()).collect();
    let modeling: std::collections::HashSet<&str> =
        modeling_tools.iter().map(|t| t.name()).collect();
    let overlap: Vec<_> = readonly.intersection(&modeling).collect();
    assert!(overlap.is_empty(), "readonly/modeling groups overlap: {overlap:?}");
}

#[test]
fn describe_ontology_description_warns_against_export() {
    // describe_ontology 的 description 必须明确告诫「不要调 export_ontology」，
    // 防止模型在会话中误用体积大的建模工具（决策：会话引用场景体积管控）。
    let store = store_with_min_ontology();
    let tools = ontology_readonly_tools(store);
    let desc_tool = tools
        .iter()
        .find(|t| t.name() == "describe_ontology")
        .expect("describe_ontology tool should exist");
    let desc = desc_tool.definition().description;
    assert!(
        desc.contains("export_ontology"),
        "describe_ontology description must warn against export_ontology, got: {desc}"
    );
    assert!(
        desc.contains("不要调") || desc.contains("不要") || desc.contains("不要在会话"),
        "describe_ontology description must contain a warning against calling export, got: {desc}"
    );
}

#[test]
fn describe_object_type_takes_two_required_args() {
    // describe_object_type 需要 ontology_api_name + object_type_api_name 两个参数。
    // 验证 schema 的 required 字段，防 regression。
    let store = store_with_min_ontology();
    let tools = ontology_readonly_tools(store);
    let desc_tool = tools
        .iter()
        .find(|t| t.name() == "describe_object_type")
        .expect("describe_object_type tool should exist");
    let params = desc_tool.definition().parameters;
    let required = params
        .get("required")
        .and_then(|v| v.as_array())
        .expect("parameters must have required array");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(required_names, vec!["ontology_api_name", "object_type_api_name"]);
}
