//! 元素映射绑定层（决策 25）集成测试：存储 / 校验 / 覆盖率 / 完整性 / 导出。
//!
//! 这些测试走**真实 SQLite**（内存库），不 mock：验证 schema 建表、`ON CONFLICT`
//! 幂等、整组替换语义、两 store 共用同一库文件等真实行为。

use std::collections::HashMap;

use ontology_store::mapping::{
    self, ElementMappingInput, ElementMappingItem, MappingEndpoint, MappingExportScope,
    STANDARD_PALANTIR, STANDARD_W3C,
};
use ontology_store::mapping_verify::{
    PhysicalColumn, PhysicalSchemaSnapshot, PhysicalSource, PhysicalTable,
};
use ontology_store::payload::{
    Capabilities, OntologyPayload, ObjectTypeDef, PropertyDef,
};
use ontology_store::OntologyStore;

// ── 夹具 ────────────────────────────────────────────────────────

fn payload() -> OntologyPayload {
    OntologyPayload {
        api_name: "Sales".to_string(),
        display_name: "销售本体".to_string(),
        description: String::new(),
        object_types: vec![ObjectTypeDef {
            api_name: "Order".to_string(),
            display_name: "订单".to_string(),
            description: String::new(),
            primary_key: "orderNo".to_string(),
            title_property: "orderNo".to_string(),
            storage_type: "MANAGED".to_string(),
            visibility: "NORMAL".to_string(),
            capabilities: Capabilities::default(),
            properties: vec![
                PropertyDef {
                    api_name: "orderNo".to_string(),
                    display_name: "订单号".to_string(),
                    description: String::new(),
                    data_type: "STRING".to_string(),
                    searchable: true,
                    is_primary_key: Some(true),
                    is_title_property: Some(true),
                    backing_mapping: None,
                    vector_config: None,
                    confidence: None,
                    evidence: Vec::new(),
                },
                PropertyDef {
                    api_name: "amount".to_string(),
                    display_name: "金额".to_string(),
                    description: String::new(),
                    data_type: "DECIMAL".to_string(),
                    searchable: true,
                    is_primary_key: Some(false),
                    is_title_property: None,
                    backing_mapping: None,
                    vector_config: None,
                    confidence: None,
                    evidence: Vec::new(),
                },
            ],
            links: vec![],
            confidence: None,
            evidence: Vec::new(),
        }],
        action_types: vec![],
        datasets: vec![],
        data_sources: vec![],
        object_type_groups: vec![],
    }
}

fn store_with_payload() -> OntologyStore {
    let store = OntologyStore::open_in_memory().unwrap();
    store
        .import(&ontology_store::payload::ImportRequest {
            payload: payload(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap();
    store
}

fn snapshot() -> PhysicalSchemaSnapshot {
    PhysicalSchemaSnapshot {
        sources: vec![PhysicalSource {
            name: "sales-db".to_string(),
            kind: "federation".to_string(),
            tables: vec![PhysicalTable {
                catalog: String::new(),
                schema: "public".to_string(),
                name: "t_order".to_string(),
                columns: vec![
                    PhysicalColumn {
                        name: "order_no".to_string(),
                        data_type: "varchar(64)".to_string(),
                        nullable: false,
                    },
                    PhysicalColumn {
                        name: "amount".to_string(),
                        data_type: "decimal(10,2)".to_string(),
                        nullable: true,
                    },
                ],
            }],
        }],
    }
}

fn prop_item(element_id: &str, table: &str, column: &str) -> ElementMappingItem {
    ElementMappingItem {
        element_type: "property".to_string(),
        element_id: element_id.to_string(),
        mappings: vec![ElementMappingInput::manual(table, column)],
    }
}

/// 两个对象（Order / Customer）之间有一个关系，用于验证关系页语义端点。
fn payload_with_link() -> OntologyPayload {
    use ontology_store::payload::LinkDef;

    let mut p = payload();
    let customer_props = vec![
        PropertyDef {
            api_name: "customerId".to_string(),
            display_name: "客户ID".to_string(),
            description: String::new(),
            data_type: "STRING".to_string(),
            searchable: true,
            is_primary_key: Some(true),
            is_title_property: Some(true),
            backing_mapping: None,
            vector_config: None,
            confidence: None,
            evidence: Vec::new(),
        },
        PropertyDef {
            api_name: "customerName".to_string(),
            display_name: "客户名称".to_string(),
            description: String::new(),
            data_type: "STRING".to_string(),
            searchable: true,
            is_primary_key: Some(false),
            is_title_property: None,
            backing_mapping: None,
            vector_config: None,
            confidence: None,
            evidence: Vec::new(),
        },
    ];
    let mut order = p.object_types.remove(0);
    order.links = vec![LinkDef {
        api_name: "hasCustomer".to_string(),
        display_name: "所属客户".to_string(),
        description: String::new(),
        target_object_type_api_name: "Customer".to_string(),
        foreign_key_property_api_name: Some("customerId".to_string()),
        cardinality: "MANY".to_string(),
        weight_property: None,
        temporal: false,
        confidence: None,
        evidence: Vec::new(),
    }];
    order.properties.push(PropertyDef {
        api_name: "customerId".to_string(),
        display_name: "客户ID".to_string(),
        description: String::new(),
        data_type: "STRING".to_string(),
        searchable: true,
        is_primary_key: Some(false),
        is_title_property: None,
        backing_mapping: None,
        vector_config: None,
        confidence: None,
        evidence: Vec::new(),
    });
    p.object_types = vec![
        order,
        ObjectTypeDef {
            api_name: "Customer".to_string(),
            display_name: "客户".to_string(),
            description: String::new(),
            primary_key: "customerId".to_string(),
            title_property: "customerName".to_string(),
            storage_type: "MANAGED".to_string(),
            visibility: "NORMAL".to_string(),
            capabilities: Capabilities::default(),
            properties: customer_props,
            links: vec![],
            confidence: None,
            evidence: Vec::new(),
        },
    ];
    p
}

fn store_with_links() -> OntologyStore {
    let store = OntologyStore::open_in_memory().unwrap();
    store
        .import(&ontology_store::payload::ImportRequest {
            payload: payload_with_link(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap();
    // 关系两端都给一个物理映射（连接条件才有内容）
    let mut m = ElementMappingInput::manual("t_order", "customer_id");
    m.target = MappingEndpoint {
        schema: "public".into(),
        table: "t_customer".into(),
        column: "customer_id".into(),
        ..Default::default()
    };
    store
        .save_element_mappings(
            "Sales",
            vec![ElementMappingItem {
                element_type: "link".to_string(),
                element_id: "hasCustomer".to_string(),
                mappings: vec![m],
            }],
            None,
        )
        .unwrap();
    store
}

/// 两个对象都有 `name` 属性，只有 Customer 的是主键——用于验证主键标记不串台。
fn payload_with_same_name_properties() -> OntologyPayload {
    let prop = |api: &str, display: &str, pk: bool| PropertyDef {
        api_name: api.to_string(),
        display_name: display.to_string(),
        description: String::new(),
        data_type: "STRING".to_string(),
        searchable: true,
        is_primary_key: Some(pk),
        is_title_property: Some(pk),
        backing_mapping: None,
        vector_config: None,
        confidence: None,
        evidence: Vec::new(),
    };
    OntologyPayload {
        api_name: "Sales".to_string(),
        display_name: "销售本体".to_string(),
        description: String::new(),
        object_types: vec![
            ObjectTypeDef {
                api_name: "Customer".to_string(),
                display_name: "客户".to_string(),
                description: String::new(),
                primary_key: "name".to_string(),
                title_property: "name".to_string(),
                storage_type: "MANAGED".to_string(),
                visibility: "NORMAL".to_string(),
                capabilities: Capabilities::default(),
                properties: vec![prop("name", "客户名称", true)],
                links: vec![],
                confidence: None,
                evidence: Vec::new(),
            },
            ObjectTypeDef {
                api_name: "Product".to_string(),
                display_name: "商品".to_string(),
                description: String::new(),
                primary_key: "sku".to_string(),
                title_property: "name".to_string(),
                storage_type: "MANAGED".to_string(),
                visibility: "NORMAL".to_string(),
                capabilities: Capabilities::default(),
                properties: vec![prop("name", "商品名称", false), prop("sku", "商品编码", true)],
                links: vec![],
                confidence: None,
                evidence: Vec::new(),
            },
        ],
        action_types: vec![],
        datasets: vec![],
        data_sources: vec![],
        object_type_groups: vec![],
    }
}

fn store_with_same_name_properties() -> OntologyStore {
    let store = OntologyStore::open_in_memory().unwrap();
    store
        .import(&ontology_store::payload::ImportRequest {
            payload: payload_with_same_name_properties(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap();
    store
}

// ── 1.3 双 store 建表 ──────────────────────────────────────────

#[test]
fn both_stores_create_element_mapping_table() {
    let palantir = OntologyStore::open_in_memory().unwrap();
    let w3c = ontology_store::TtlStore::open_in_memory().unwrap();
    // 两个 store 各自初始化都建出表；再验证同一库先后打开两 store 不报错
    // （生产环境共用同一 .db 文件，任一 store 先打开都要让另一侧可用）
    let dir = std::env::temp_dir().join(format!(
        "onto-mapping-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shared.db");
    {
        let p = OntologyStore::open(&path).unwrap();
        p.list_element_mappings("Sales").unwrap();
    }
    {
        let w = ontology_store::TtlStore::open(&path).unwrap();
        w.list_element_mappings("https://x#Ont").unwrap();
    }
    {
        // 反向顺序再开一次（验证幂等）
        let p = OntologyStore::open(&path).unwrap();
        p.list_element_mappings("Sales").unwrap();
    }
    // 内存库的两个 store 也能用
    palantir.list_element_mappings("Sales").unwrap();
    w3c.list_element_mappings("https://x#Ont").unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

// ── 1.5 写入语义 ──────────────────────────────────────────────

#[test]
fn duplicate_identical_mapping_is_idempotent() {
    let store = store_with_payload();
    let item = prop_item("orderNo", "t_order", "order_no");
    store
        .save_element_mappings("Sales", vec![item.clone()], None)
        .unwrap();
    let second = store
        .save_element_mappings("Sales", vec![item], None)
        .unwrap();
    assert_eq!(second.written, 1);
    assert_eq!(second.removed, 0);
    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows.len(), 1, "重复保存不应产生重复行: {rows:?}");
    assert_eq!(rows[0].source.column, "order_no");
}

#[test]
fn same_element_accepts_two_different_columns() {
    let store = store_with_payload();
    let item = ElementMappingItem {
        element_type: "property".to_string(),
        element_id: "orderNo".to_string(),
        mappings: vec![
            ElementMappingInput::manual("t_order", "order_no"),
            ElementMappingInput::manual("t_order_archive", "order_no"),
        ],
    };
    store.save_element_mappings("Sales", vec![item], None).unwrap();
    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows.len(), 2, "同一元素允许多条映射: {rows:?}");
    assert!(rows.iter().any(|r| r.source.table == "t_order"));
    assert!(rows.iter().any(|r| r.source.table == "t_order_archive"));
}

#[test]
fn group_replacement_removes_absent_rows() {
    let store = store_with_payload();
    let two = ElementMappingItem {
        element_type: "property".to_string(),
        element_id: "orderNo".to_string(),
        mappings: vec![
            ElementMappingInput::manual("t_order", "order_no"),
            ElementMappingInput::manual("t_order_old", "order_no"),
        ],
    };
    store.save_element_mappings("Sales", vec![two], None).unwrap();
    assert_eq!(store.list_element_mappings("Sales").unwrap().len(), 2);

    // 只保留一条 → 另一条被移除
    let one = prop_item("orderNo", "t_order", "order_no");
    let outcome = store.save_element_mappings("Sales", vec![one], None).unwrap();
    assert_eq!(outcome.removed, 1);
    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source.table, "t_order");
}

#[test]
fn empty_mapping_list_deletes_all_for_element() {
    let store = store_with_payload();
    store
        .save_element_mappings("Sales", vec![prop_item("orderNo", "t_order", "order_no")], None)
        .unwrap();
    let empty = ElementMappingItem {
        element_type: "property".to_string(),
        element_id: "orderNo".to_string(),
        mappings: vec![],
    };
    store.save_element_mappings("Sales", vec![empty], None).unwrap();
    assert!(store.list_element_mappings("Sales").unwrap().is_empty());
}

#[test]
fn cross_standard_isolation() {
    let store = store_with_payload();
    let w3c = ontology_store::TtlStore::open_in_memory().unwrap();
    store
        .save_element_mappings("Sales", vec![prop_item("orderNo", "t_order", "order_no")], None)
        .unwrap();
    // W3C 侧同一库 key 下无映射（互不串味）
    assert!(w3c.list_element_mappings("Sales").unwrap().is_empty());
    // 标准不匹配的元素类型被拒（告警不阻断）
    let bad = ElementMappingItem {
        element_type: "datatype_property".to_string(),
        element_id: "x".to_string(),
        mappings: vec![ElementMappingInput::manual("t", "c")],
    };
    let out = store.save_element_mappings("Sales", vec![bad], None).unwrap();
    assert_eq!(out.written, 0);
    assert!(!out.warnings.is_empty());
}

// ── 2.4 校验 ─────────────────────────────────────────────────

#[test]
fn verified_when_snapshot_hits() {
    let store = store_with_payload();
    let snap = snapshot();
    store
        .save_element_mappings(
            "Sales",
            vec![prop_item("orderNo", "t_order", "order_no")],
            Some(&snap),
        )
        .unwrap();
    let rows = store.list_element_mappings("Sales").unwrap();
    assert!(rows[0].verified, "快照命中应标记已核对");
    assert!(rows[0].verified_at > 0);
}

#[test]
fn claimed_verified_downgraded_when_snapshot_misses() {
    let store = store_with_payload();
    let snap = snapshot();
    // 声称已核对但快照里没有这列 → 写入成功（不拒绝），状态降级为未核对 + 告警
    let item = ElementMappingItem {
        element_type: "property".to_string(),
        element_id: "amount".to_string(),
        mappings: vec![ElementMappingInput {
            source: MappingEndpoint {
                table: "t_order".to_string(),
                column: "not_a_column".to_string(),
                ..Default::default()
            },
            source_kind: "federation".to_string(),
            ..Default::default()
        }],
    };
    let out = store
        .save_element_mappings("Sales", vec![item], Some(&snap))
        .unwrap();
    assert_eq!(out.written, 1, "未命中不拒绝写入");
    assert!(!out.warnings.is_empty(), "未命中应给出告警");
    let rows = store.list_element_mappings("Sales").unwrap();
    assert!(!rows[0].verified);
    assert_eq!(rows[0].verified_at, 0);
}

#[test]
fn empty_snapshot_writes_unverified_without_error() {
    let store = store_with_payload();
    let empty = PhysicalSchemaSnapshot::default();
    let out = store
        .save_element_mappings(
            "Sales",
            vec![prop_item("orderNo", "t_order", "order_no")],
            Some(&empty),
        )
        .unwrap();
    assert_eq!(out.written, 1, "空快照不应报错");
    assert!(out.warnings.is_empty(), "空快照不是失败，不应告警");
    let rows = store.list_element_mappings("Sales").unwrap();
    assert!(!rows[0].verified);
}

#[test]
fn existing_verification_preserved_without_snapshot() {
    let store = store_with_payload();
    let snap = snapshot();
    store
        .save_element_mappings(
            "Sales",
            vec![prop_item("orderNo", "t_order", "order_no")],
            Some(&snap),
        )
        .unwrap();
    assert!(store.list_element_mappings("Sales").unwrap()[0].verified);
    // 不带快照重复保存同一位置 → 保留原核对状态（不误降级）
    store
        .save_element_mappings("Sales", vec![prop_item("orderNo", "t_order", "order_no")], None)
        .unwrap();
    assert!(store.list_element_mappings("Sales").unwrap()[0].verified);
}

// ── 3.4 覆盖率 / 完整性 ──────────────────────────────────────

#[test]
fn coverage_does_not_derive_property_from_class() {
    let store = store_with_payload();
    // 只映射类（object_type）→ 属性覆盖率必须仍是 0（不得由类推导）
    let class_item = ElementMappingItem {
        element_type: "object_type".to_string(),
        element_id: "Order".to_string(),
        mappings: vec![ElementMappingInput {
            source: MappingEndpoint {
                table: "t_order".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }],
    };
    store.save_element_mappings("Sales", vec![class_item], None).unwrap();
    let cov = store.mapping_coverage("Sales").unwrap();
    assert_eq!(cov.classes.total, 1);
    assert_eq!(cov.classes.mapped, 1);
    assert_eq!(cov.properties.total, 2, "订单有两个属性");
    assert_eq!(cov.properties.mapped, 0, "类的映射不得算到属性上");
    assert_eq!(cov.relations.total, 0);
}

#[test]
fn orphan_reported_and_not_auto_deleted() {
    let store = store_with_payload();
    store
        .save_element_mappings("Sales", vec![prop_item("ghostProp", "t_order", "x")], None)
        .unwrap();
    let report = store.mapping_integrity("Sales", None).unwrap();
    assert_eq!(report.orphans.len(), 1, "元素不存在应进孤儿: {report:?}");
    // 不自动删除
    assert_eq!(store.list_element_mappings("Sales").unwrap().len(), 1);
    // 显式清理后才消失
    let n = store.purge_orphan_mappings("Sales", None).unwrap();
    assert_eq!(n, 1);
    assert!(store.list_element_mappings("Sales").unwrap().is_empty());
}

#[test]
fn stale_when_snapshot_table_disappears() {
    let store = store_with_payload();
    store
        .save_element_mappings(
            "Sales",
            vec![prop_item("orderNo", "t_order", "order_no")],
            Some(&snapshot()),
        )
        .unwrap();
    // 快照里表消失（结构差异）→ 进 stale
    let empty_but_present = PhysicalSchemaSnapshot {
        sources: vec![PhysicalSource {
            name: "sales-db".to_string(),
            kind: "federation".to_string(),
            tables: vec![PhysicalTable {
                name: "t_other".to_string(),
                ..Default::default()
            }],
        }],
    };
    let report = store
        .mapping_integrity("Sales", Some(&empty_but_present))
        .unwrap();
    assert_eq!(report.stale.len(), 1, "结构差异应进 stale: {report:?}");
    assert!(report.orphans.is_empty());
    // 快照为空（数据源不可达）不判失效，只算未核对 → 不产生噪声
    let report = store.mapping_integrity("Sales", None).unwrap();
    assert!(report.stale.is_empty());
    assert!(report.unverified.is_empty(), "已核对过的行不该算未核对");
}

// ── 4.4 建议不落库 ──────────────────────────────────────────

#[test]
fn suggest_does_not_persist() {
    let store = store_with_payload();
    let elements = vec![mapping::MappingElementRef {
        element_type: "property".to_string(),
        element_id: "orderNo".to_string(),
        display_name: "订单号".to_string(),
        data_type: "STRING".to_string(),
        same_table_hint: String::new(),
        ..Default::default()
    }];
    let out = store
        .suggest_element_mappings("Sales", &elements, &snapshot())
        .unwrap();
    assert!(!out.is_empty(), "应有候选: {out:?}");
    assert_eq!(out[0].candidate.column, "order_no");
    assert!(
        store.list_element_mappings("Sales").unwrap().is_empty(),
        "建议不得落库"
    );
}

// ── 5. 导出（多工作表工作簿） ────────────────────────────────

#[test]
fn workbook_export_all_and_gap() {
    use calamine::Reader;

    let store = store_with_payload();
    store
        .save_element_mappings(
            "Sales",
            vec![prop_item("orderNo", "t_order", "order_no")],
            Some(&snapshot()),
        )
        .unwrap();

    let all = store
        .mapping_workbook(
            "Sales",
            MappingExportScope::All,
            Some(&snapshot()),
            "2026-11-13 08:40 UTC",
        )
        .unwrap();
    assert!(all.starts_with(b"PK\x03\x04"), "xlsx 是 zip 容器");
    let mut wb = calamine::Xlsx::new(std::io::Cursor::new(all.as_slice())).unwrap();
    assert_eq!(
        wb.sheet_names(),
        &["概览", "类", "属性", "关系"],
        "四页工作簿是对外契约"
    );

    // 属性页：1 行表头 + 2 行数据（orderNo 已映射、amount 未映射）
    let props = wb.worksheet_range("属性").unwrap();
    assert_eq!(props.height(), 3, "表头 + 两个属性");
    let descriptions: Vec<String> = (1..props.height() as u32)
        .map(|r| {
            props
                .get_value((r, 3))
                .map(|v| v.to_string())
                .unwrap_or_default()
        })
        .collect();
    assert!(
        descriptions.contains(&"订单号".to_string()),
        "应含已映射属性: {descriptions:?}"
    );
    assert!(
        descriptions.contains(&"金额".to_string()),
        "应含未映射属性: {descriptions:?}"
    );

    // 缺口范围：只剩未映射的金额
    let gap = store
        .mapping_workbook(
            "Sales",
            MappingExportScope::Gap,
            Some(&snapshot()),
            "2026-11-13 08:40 UTC",
        )
        .unwrap();
    let mut gap_wb = calamine::Xlsx::new(std::io::Cursor::new(gap.as_slice())).unwrap();
    let gap_props = gap_wb.worksheet_range("属性").unwrap();
    assert_eq!(gap_props.height(), 2, "缺口只含未映射元素");
    assert_eq!(
        gap_props.get_value((1, 3)).map(|v| v.to_string()),
        Some("金额".to_string())
    );
}

#[test]
fn workbook_relation_sheet_carries_semantic_endpoints() {
    use calamine::Reader;

    let store = store_with_links();
    let bytes = store
        .mapping_workbook(
            "Sales",
            MappingExportScope::All,
            Some(&snapshot()),
            "2026-11-13 08:40 UTC",
        )
        .unwrap();
    let mut wb = calamine::Xlsx::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
    let rel = wb.worksheet_range("关系").unwrap();

    // 关系页列序：0=关系名称 2=源对象 4=目标对象 6=基数 7=连接条件
    assert_eq!(
        rel.get_value((0, 0)).map(|v| v.to_string()),
        Some("关系名称".to_string())
    );
    assert_eq!(
        rel.get_value((1, 2)).map(|v| v.to_string()),
        Some("Order".to_string()),
        "源对象取自本体定义（link_types），不是从映射反推"
    );
    assert_eq!(
        rel.get_value((1, 4)).map(|v| v.to_string()),
        Some("Customer".to_string()),
        "目标对象取自本体定义"
    );
}

#[test]
fn workbook_primary_keys_are_scoped_per_object() {
    use calamine::Reader;

    // 两个对象都有 name 属性，只有 Customer 的是主键——串台会让 Product.name 也显示「是」
    let store = store_with_same_name_properties();
    let bytes = store
        .mapping_workbook(
            "Sales",
            MappingExportScope::All,
            None,
            "2026-11-13 08:40 UTC",
        )
        .unwrap();
    let mut wb = calamine::Xlsx::new(std::io::Cursor::new(bytes.as_slice())).unwrap();
    let props = wb.worksheet_range("属性").unwrap();
    let mut found: HashMap<String, String> = HashMap::new();
    for r in 1..props.height() as u32 {
        let owner = props
            .get_value((r, 0))
            .map(|v| v.to_string())
            .unwrap_or_default();
        let is_pk = props
            .get_value((r, 5))
            .map(|v| v.to_string())
            .unwrap_or_default();
        if props.get_value((r, 2)).map(|v| v.to_string()) == Some("name".to_string()) {
            found.insert(owner, is_pk);
        }
    }
    assert_eq!(
        found.get("Customer").map(|s| s.as_str()),
        Some("是"),
        "Customer.name 是主键: {found:?}"
    );
    assert_eq!(
        found.get("Product").map(|s| s.as_str()),
        Some("否"),
        "Product.name 不是主键，不得被 Customer 覆盖: {found:?}"
    );
}

// ── 9. 回填与双写 ───────────────────────────────────────────

fn payload_with_backing() -> OntologyPayload {
    let mut p = payload();
    p.object_types[0].properties[0].backing_mapping = Some(ontology_store::BackingMapping {
        dataset_api_name: String::new(),
        backing_catalog: String::new(),
        backing_schema: "public".to_string(),
        backing_table: "t_order".to_string(),
        backing_column: "order_no".to_string(),
    });
    p
}

fn import_payload(store: &OntologyStore, p: OntologyPayload) -> ontology_store::payload::ImportResult {
    store
        .import(&ontology_store::payload::ImportRequest {
            payload: p,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap()
}

#[test]
fn import_with_backing_populates_element_mapping() {
    // 决策 25：import 路径回填——payload 带 backing_mapping 时绑定层同步有行，
    // 避免导入后在新视图里"映射消失"。
    let store = OntologyStore::open_in_memory().unwrap();
    let res = import_payload(&store, payload_with_backing());
    assert!(res.errors.is_empty(), "导入不该报错: {:?}", res.errors);

    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows.len(), 1, "payload 的 backing 应回填进绑定层");
    assert_eq!(rows[0].element_type, "property");
    assert_eq!(rows[0].element_id, "orderNo");
    assert_eq!(rows[0].source.table, "t_order");
    assert_eq!(rows[0].source.column, "order_no");
    assert!(!rows[0].verified, "payload 里的来源未经核对");
    assert_eq!(rows[0].source_kind, "manual");
}

#[test]
fn import_backfill_is_idempotent() {
    let store = OntologyStore::open_in_memory().unwrap();
    import_payload(&store, payload_with_backing());
    // 同一 payload 再导入一次（覆盖场景）：不应产生重复行
    import_payload(&store, payload_with_backing());
    assert_eq!(
        store.list_element_mappings("Sales").unwrap().len(),
        1,
        "重复导入不得产生重复映射行"
    );
}

#[test]
fn import_backfill_does_not_clobber_existing_multi_column() {
    // 已有两列绑定（新视图里做的）→ import 的单列 backing 不该把它们改掉
    let store = store_with_payload();
    store
        .save_element_mappings(
            "Sales",
            vec![ElementMappingItem {
                element_type: "property".to_string(),
                element_id: "orderNo".to_string(),
                mappings: vec![
                    ElementMappingInput {
                        source: MappingEndpoint {
                            table: "t_order".to_string(),
                            column: "order_no".to_string(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ElementMappingInput {
                        source: MappingEndpoint {
                            table: "t_order_alias".to_string(),
                            column: "order_no".to_string(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                ],
            }],
            None,
        )
        .unwrap();

    import_payload(&store, payload_with_backing());
    let rows = store.list_element_mappings("Sales").unwrap();
    let for_prop: Vec<_> = rows.iter().filter(|r| r.element_id == "orderNo").collect();
    assert_eq!(for_prop.len(), 2, "已有多列绑定应保持不变");
    assert!(for_prop.iter().any(|r| r.source.table == "t_order_alias"));
}

#[test]
fn explicit_backfill_is_noop_after_import_backfill() {
    // 导入已回填 → 显式调用 backfill 不再重复写入（集合非空即跳过）
    let store = OntologyStore::open_in_memory().unwrap();
    import_payload(&store, payload_with_backing());
    let n = store.backfill_mappings_from_properties("Sales").unwrap();
    assert_eq!(n, 0);
    assert_eq!(store.list_element_mappings("Sales").unwrap().len(), 1);
}

#[test]
fn explicit_backfill_migrates_legacy_rows() {
    // 旧库场景：properties.backing_* 有值但绑定层空（本次升级前的存量数据）
    let store = store_with_payload();
    // 直接写回 backing 列，模拟存量数据（绕过 import 回填路径）
    store
        .import(&ontology_store::payload::ImportRequest {
            payload: payload_with_backing(),
            overwrite_object_types: vec!["Order".to_string()],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap();
    // 清空绑定层，还原"旧库"状态
    store
        .delete_element_mappings_for_element("Sales", "property", "orderNo")
        .unwrap();
    assert!(store.list_element_mappings("Sales").unwrap().is_empty());

    let n = store.backfill_mappings_from_properties("Sales").unwrap();
    assert_eq!(n, 1, "存量 backing 数据应可迁移");
    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows[0].source.column, "order_no");
    assert!(!rows[0].verified, "迁移数据没有核对语义");
}

#[test]
fn backfill_from_properties_is_idempotent() {
    let store = OntologyStore::open_in_memory().unwrap();
    let mut p = payload();
    p.object_types[0].properties[0].backing_mapping = Some(ontology_store::BackingMapping {
        dataset_api_name: String::new(),
        backing_catalog: String::new(),
        backing_schema: "public".to_string(),
        backing_table: "t_order".to_string(),
        backing_column: "order_no".to_string(),
    });
    store
        .import(&ontology_store::payload::ImportRequest {
            payload: p,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
            evidence_mode: String::new(),
        })
        .unwrap();

    // import 已回填，显式回填是幂等的空操作
    let n = store.backfill_mappings_from_properties("Sales").unwrap();
    assert_eq!(n, 0);
    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source.column, "order_no");
    assert!(!rows[0].verified, "回填数据没有核对语义");
    assert_eq!(rows[0].source_kind, "manual");

    let again = store.backfill_mappings_from_properties("Sales").unwrap();
    assert_eq!(again, 0);
    assert_eq!(store.list_element_mappings("Sales").unwrap().len(), 1);
}

#[test]
fn double_write_keeps_payload_backing() {
    let store = store_with_payload();
    store
        .save_element_mappings("Sales", vec![prop_item("orderNo", "t_order", "order_no")], None)
        .unwrap();
    // 双写：payload export 仍带 backing（契约兼容）
    let exported = store.export("Sales").unwrap();
    let prop = exported.object_types[0]
        .properties
        .iter()
        .find(|p| p.api_name == "orderNo")
        .unwrap();
    let backing = prop.backing_mapping.as_ref().expect("payload 应仍带 backing");
    assert_eq!(backing.backing_table, "t_order");
    assert_eq!(backing.backing_column, "order_no");
}

// ── 关系单端 ────────────────────────────────────────────────

#[test]
fn relation_partial_mapping_allowed_and_marked() {
    let store = store_with_payload();
    let link = ElementMappingItem {
        element_type: "link".to_string(),
        element_id: "hasCustomer".to_string(),
        mappings: vec![ElementMappingInput {
            source: MappingEndpoint {
                table: "t_order".to_string(),
                column: "customer_id".to_string(),
                ..Default::default()
            },
            // 目标端留空：允许单端记录（D14）
            ..Default::default()
        }],
    };
    store.save_element_mappings("Sales", vec![link], None).unwrap();
    let rows = store.list_element_mappings("Sales").unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_partial(), "单端关系应标为不完整");
}

// ── W3C 侧 ──────────────────────────────────────────────────

#[test]
fn w3c_mapping_does_not_touch_ttl_and_uses_iri_key() {
    let store = ontology_store::TtlStore::open_in_memory().unwrap();
    let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://x.org/sales#> .

:Sales a owl:Ontology ; rdfs:label "销售"@zh .
:Order a owl:Class ; rdfs:label "订单"@zh .
:orderNo a owl:DatatypeProperty ;
    rdfs:domain :Order ;
    rdfs:range xsd:string ;
    rdfs:label "订单号"@zh .
"#;
    store.import_ttl(ttl, true).unwrap();
    let iri = store.list_ontologies().unwrap()[0].ontology_iri.clone();
    let before = store.export_ttl(&iri).unwrap();

    let item = ElementMappingItem {
        element_type: "datatype_property".to_string(),
        element_id: "https://x.org/sales#orderNo".to_string(),
        mappings: vec![ElementMappingInput::manual("t_order", "order_no")],
    };
    store.save_element_mappings(&iri, vec![item], None).unwrap();

    // 概念层不变（映射不写进 .ttl）
    let after = store.export_ttl(&iri).unwrap();
    assert_eq!(before, after, "映射不得改动 Turtle 文本");

    let rows = store.list_element_mappings(&iri).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].standard, STANDARD_W3C);
    assert_eq!(rows[0].element_id, "https://x.org/sales#orderNo");

    // 覆盖率：1 类 1 属性，属性已映射
    let cov = store.mapping_coverage(&iri).unwrap();
    assert_eq!(cov.classes.total, 1);
    assert_eq!(cov.properties.total, 1);
    assert_eq!(cov.properties.mapped, 1);
}

#[test]
fn w3c_suggest_uses_labels_for_normalization() {
    let store = ontology_store::TtlStore::open_in_memory().unwrap();
    let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://x.org/sales#> .
:Sales a owl:Ontology .
:Order a owl:Class ; rdfs:label "Order" .
:orderNo a owl:DatatypeProperty ;
    rdfs:domain :Order ; rdfs:range xsd:string ; rdfs:label "orderNo" .
"#;
    store.import_ttl(ttl, true).unwrap();
    let iri = store.list_ontologies().unwrap()[0].ontology_iri.clone();
    let elements = vec![mapping::MappingElementRef {
        element_type: "datatype_property".to_string(),
        element_id: format!("{iri}#orderNo"),
        display_name: "orderNo".to_string(),
        data_type: "STRING".to_string(),
        same_table_hint: String::new(),
        ..Default::default()
    }];
    let out = store
        .suggest_element_mappings(&iri, &elements, &snapshot())
        .unwrap();
    assert!(out.iter().any(|s| s.candidate.column == "order_no"), "{out:?}");
    assert!(store.list_element_mappings(&iri).unwrap().is_empty());
}

// ── 6.5 只读视图注入 ────────────────────────────────────────

#[test]
fn describe_object_type_exposes_backing_only_when_mapped() {
    let store = store_with_payload();
    store
        .save_element_mappings(
            "Sales",
            vec![prop_item("orderNo", "t_order", "order_no")],
            Some(&snapshot()),
        )
        .unwrap();
    let detail = store.describe_object_type("Sales", "Order").unwrap();
    let mapped = detail
        .properties
        .iter()
        .find(|p| p.api_name == "orderNo")
        .unwrap();
    let backing = mapped.backing.as_ref().expect("已映射属性应带 binding");
    assert_eq!(backing.table, "t_order");
    assert_eq!(backing.column, "order_no");
    assert!(backing.verified);

    let unmapped = detail
        .properties
        .iter()
        .find(|p| p.api_name == "amount")
        .unwrap();
    assert!(
        unmapped.backing.is_none(),
        "未映射属性不应出现 binding 字段"
    );
    // 序列化后该键整体缺省（不是 null 也不是空对象）
    let json = serde_json::to_string(unmapped).unwrap();
    assert!(!json.contains("backing"), "未映射属性序列化后不得含 backing: {json}");
}

#[test]
fn class_level_backing_injected() {
    let store = store_with_payload();
    let class_item = ElementMappingItem {
        element_type: "object_type".to_string(),
        element_id: "Order".to_string(),
        mappings: vec![ElementMappingInput {
            source: MappingEndpoint {
                table: "t_order".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }],
    };
    store.save_element_mappings("Sales", vec![class_item], None).unwrap();
    let detail = store.describe_object_type("Sales", "Order").unwrap();
    let backing = detail.backing.as_ref().expect("类级映射应注入");
    assert_eq!(backing.table, "t_order");
    assert!(backing.column.is_empty());

    // 未映射的类不出现该字段
    let store2 = store_with_payload();
    let detail2 = store2.describe_object_type("Sales", "Order").unwrap();
    assert!(detail2.backing.is_none());
}

// ── 导出文件名 ──────────────────────────────────────────────

#[test]
fn export_file_name_helper() {
    use ontology_store::mapping_export::export_file_name;
    // 导出只有全量一种范围，文件名不再带范围后缀
    assert_eq!(export_file_name("Sales"), "Sales-元素映射.xlsx");
    assert_eq!(export_file_name(""), "ontology-元素映射.xlsx");
    // W3C 本体标识是 IRI，取末段做业务可读名
    assert_eq!(
        export_file_name("http://ex.org/onto#Supply"),
        "Supply-元素映射.xlsx"
    );
    let _: HashMap<String, bool> = HashMap::new();
}

// ── Palantir 标准常量一致性 ─────────────────────────────────

#[test]
fn standard_constants() {
    assert_eq!(STANDARD_PALANTIR, "palantir");
    assert_eq!(STANDARD_W3C, "w3c");
}