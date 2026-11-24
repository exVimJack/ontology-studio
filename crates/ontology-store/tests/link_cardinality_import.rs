//! 回归测试：关系的基数必须是 Gaia 契约 ONE|MANY，且两个值都能真正落库。
//!
//! 历史 bug：前端自造 Palantir 四元组（ONE_TO_ONE/ONE_TO_MANY/MANY_TO_ONE/
//! MANY_TO_MANY）填进 `LinkDef.cardinality`，被 `validate_link` 全数拒绝，
//! 表现为「新增关系时选哪个基数都失败」。本测试锁住后端的契约与落库行为。
use ontology_store::payload::*;
use ontology_store::store::OntologyStore;

fn payload_json(card: &str) -> String {
    format!(
        r#"{{
      "api_name":"ContractOntology","display_name":"契约","description":"",
      "object_types":[
        {{"api_name":"Order","display_name":"订单","storage_type":"MANAGED",
          "properties":[{{"api_name":"orderId","display_name":"订单号","data_type":"STRING","is_primary_key":true}}],
          "links":[{{"api_name":"belongsTo","display_name":"归属于",
                    "target_object_type_api_name":"Customer","cardinality":"{card}"}}]}},
        {{"api_name":"Customer","display_name":"客户","storage_type":"MANAGED",
          "properties":[{{"api_name":"customerId","display_name":"客户号","data_type":"STRING","is_primary_key":true}}],
          "links":[]}}
      ],
      "action_types":[],"datasets":[],"data_sources":[],"object_type_groups":[]
    }}"#
    )
}

fn open_store(tag: &str) -> OntologyStore {
    let dir = std::env::temp_dir().join(format!("ontostudio_link_card_{tag}.db"));
    let _ = std::fs::remove_file(&dir);
    OntologyStore::open(&dir).unwrap()
}

fn req(card: &str) -> ImportRequest {
    ImportRequest {
        payload: serde_json::from_str(&payload_json(card)).unwrap(),
        overwrite_object_types: vec!["Order".into()],
        overwrite_data_sources: vec![],
        evidence_mode: String::new(),
    }
}

#[test]
fn one_and_many_both_import_and_roundtrip() {
    for card in ["ONE", "MANY"] {
        let store = open_store(card);
        let r = req(card);
        let preview = store.preview_import(&r).unwrap();
        assert!(preview.errors.is_empty(), "{card} preview errors={:?}", preview.errors);
        let imported = store.import(&r).unwrap();
        assert!(imported.errors.is_empty(), "{card} import errors={:?}", imported.errors);
        assert_eq!(imported.links_created, 1, "{card} 关系未落库");

        let exported = store.export("ContractOntology").unwrap();
        let order = exported
            .object_types
            .iter()
            .find(|o| o.api_name == "Order")
            .expect("Order 未导出");
        assert_eq!(order.links.len(), 1, "{card} 关系被重复落库");
        assert_eq!(order.links[0].cardinality, card, "回读基数不一致");
    }
}

#[test]
fn palantir_four_tuple_values_are_rejected() {
    for bad in ["ONE_TO_ONE", "ONE_TO_MANY", "MANY_TO_ONE", "MANY_TO_MANY"] {
        let store = open_store(&format!("bad_{bad}"));
        let err = store.preview_import(&req(bad));
        assert!(err.is_err(), "{bad} 应被后端拒绝（Gaia 契约只有 ONE|MANY）");
    }
}
