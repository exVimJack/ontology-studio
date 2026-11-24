//! 真实链路冒烟：把含类/属性/关系且部分未映射的本体导出成真实 .xlsx 文件。
//!
//! 用环境变量 `ONTO_XLSX_OUT` 指定输出路径；未设置则跳过（不污染常规测试）。
//! 该测试的目的不是断言业务逻辑（那些在单测里），而是产出一个真实文件供
//! 外部工具（Excel / Python openpyxl）回读，验证「写出的文件真能被别的软件打开」。

use std::collections::HashMap;

use ontology_store::mapping::{self, ElementMappingInput, ElementMappingItem, MappingExportScope};
use ontology_store::mapping_verify::{
    PhysicalColumn, PhysicalSchemaSnapshot, PhysicalSource, PhysicalTable,
};
use ontology_store::payload::{Capabilities, LinkDef, ObjectTypeDef, OntologyPayload, PropertyDef};
use ontology_store::OntologyStore;

/// 带 schema 分段的便捷构造（真实映射里「来源表」是 schema.table 两段）。
fn manual_at(schema: &str, table: &str, column: &str) -> ElementMappingInput {
    ElementMappingInput {
        source: mapping::MappingEndpoint {
            schema: schema.to_string(),
            table: table.to_string(),
            column: column.to_string(),
            ..Default::default()
        },
        source_kind: mapping::SOURCE_KIND_MANUAL.to_string(),
        ..Default::default()
    }
}

fn prop(api: &str, display: &str, dt: &str, pk: bool) -> PropertyDef {
    PropertyDef {
        api_name: api.to_string(),
        display_name: display.to_string(),
        description: format!("{display}的业务定义"),
        data_type: dt.to_string(),
        searchable: true,
        is_primary_key: Some(pk),
        is_title_property: Some(pk),
        backing_mapping: None,
        vector_config: None,
        confidence: None,
        evidence: Vec::new(),
    }
}

#[test]
fn smoke_write_real_workbook() {
    let Ok(out) = std::env::var("ONTO_XLSX_OUT") else {
        eprintln!("跳过：未设置 ONTO_XLSX_OUT");
        return;
    };

    let payload = OntologyPayload {
        api_name: "SmokeSales".to_string(),
        display_name: "冒烟销售本体".to_string(),
        description: String::new(),
        object_types: vec![
            ObjectTypeDef {
                api_name: "SalesOrder".to_string(),
                display_name: "销售订单".to_string(),
                description: "客户下单形成的单据".to_string(),
                primary_key: "orderNo".to_string(),
                title_property: "orderNo".to_string(),
                storage_type: "MANAGED".to_string(),
                visibility: "NORMAL".to_string(),
                capabilities: Capabilities::default(),
                properties: vec![
                    prop("orderNo", "订单号", "STRING", true),
                    prop("amount", "订单金额", "DECIMAL", false),
                    prop("customerId", "客户编号", "STRING", false),
                    prop("createdAt", "下单时间", "TIMESTAMP", false),
                ],
                links: vec![LinkDef {
                    api_name: "orderOfCustomer".to_string(),
                    display_name: "所属客户".to_string(),
                    description: "订单归属的客户".to_string(),
                    target_object_type_api_name: "Customer".to_string(),
                    foreign_key_property_api_name: Some("customerId".to_string()),
                    cardinality: "MANY".to_string(),
                    weight_property: None,
                    temporal: false,
                    confidence: None,
                    evidence: Vec::new(),
                }],
                confidence: None,
                evidence: Vec::new(),
            },
            ObjectTypeDef {
                api_name: "Customer".to_string(),
                display_name: "客户".to_string(),
                description: "购买方主体".to_string(),
                primary_key: "customerNo".to_string(),
                title_property: "customerName".to_string(),
                storage_type: "MANAGED".to_string(),
                visibility: "NORMAL".to_string(),
                capabilities: Capabilities::default(),
                properties: vec![
                    prop("customerNo", "客户编号", "STRING", true),
                    prop("customerName", "客户名称", "STRING", false),
                ],
                links: vec![],
                confidence: None,
                evidence: Vec::new(),
            },
        ],
        action_types: vec![],
        datasets: vec![],
        data_sources: vec![],
        object_type_groups: vec![],
    };

    let store = OntologyStore::open_in_memory().unwrap();
    store
        .import(&ontology_store::payload::ImportRequest {
            payload,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap();

    // 只映射一部分：类映射 1 个（Customer 不映射）、属性映射 3 个、关系两端都映射
    let link_map = ElementMappingInput {
        source: mapping::MappingEndpoint {
            schema: "mhero_ods".into(),
            table: "sales_order".into(),
            column: "customer_no".into(),
            ..Default::default()
        },
        target: mapping::MappingEndpoint {
            schema: "mhero_ods".into(),
            table: "customer".into(),
            column: "customer_no".into(),
            ..Default::default()
        },
        source_kind: mapping::SOURCE_KIND_MANUAL.to_string(),
        ..Default::default()
    };
    store
        .save_element_mappings(
            "SmokeSales",
            vec![
                ElementMappingItem {
                    element_type: "object_type".to_string(),
                    element_id: "SalesOrder".to_string(),
                    mappings: vec![manual_at("mhero_ods", "sales_order", "")],
                },
                ElementMappingItem {
                    element_type: "property".to_string(),
                    element_id: "orderNo".to_string(),
                    mappings: vec![manual_at("mhero_ods", "sales_order", "order_no")],
                },
                ElementMappingItem {
                    element_type: "property".to_string(),
                    element_id: "amount".to_string(),
                    mappings: vec![manual_at("mhero_ods", "sales_order", "order_amount")],
                },
                ElementMappingItem {
                    element_type: "property".to_string(),
                    element_id: "customerId".to_string(),
                    mappings: vec![manual_at("mhero_ods", "sales_order", "customer_no")],
                },
                ElementMappingItem {
                    element_type: "link".to_string(),
                    element_id: "orderOfCustomer".to_string(),
                    mappings: vec![link_map],
                },
            ],
            None,
        )
        .unwrap();

    let snapshot = PhysicalSchemaSnapshot {
        sources: vec![PhysicalSource {
            name: "mhero".to_string(),
            kind: "federation".to_string(),
            tables: vec![PhysicalTable {
                catalog: String::new(),
                schema: "mhero_ods".to_string(),
                name: "sales_order".to_string(),
                columns: vec![
                    PhysicalColumn {
                        name: "order_no".to_string(),
                        data_type: "varchar(64)".to_string(),
                        nullable: false,
                    },
                    PhysicalColumn {
                        name: "order_amount".to_string(),
                        data_type: "decimal(10,2)".to_string(),
                        nullable: true,
                    },
                    PhysicalColumn {
                        name: "customer_no".to_string(),
                        data_type: "varchar(32)".to_string(),
                        nullable: false,
                    },
                ],
            }],
        }],
    };

    let bytes = store
        .mapping_workbook(
            "SmokeSales",
            MappingExportScope::All,
            Some(&snapshot),
            "2026-11-16 03:40 UTC",
        )
        .unwrap();
    std::fs::write(&out, &bytes).unwrap();
    eprintln!("已写出：{out}（{} 字节）", bytes.len());

    // 简单自检：数据集口径
    let ds = store
        .mapping_dataset(
            "SmokeSales",
            MappingExportScope::All,
            Some(&snapshot),
            "2026-11-16 03:40 UTC",
        )
        .unwrap();
    assert_eq!(ds.overview.class_total, 2);
    assert_eq!(ds.overview.class_mapped, 1);
    assert_eq!(ds.overview.property_total, 6);
    assert_eq!(ds.overview.property_mapped, 3);
    assert_eq!(ds.overview.relation_total, 1);
    let _: HashMap<String, bool> = HashMap::new();
}