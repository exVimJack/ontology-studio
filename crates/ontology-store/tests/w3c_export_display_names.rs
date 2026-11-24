//! W3C 侧映射导出的真实链路测试：概览页「本体名称」必须取创建向导填写的名称。
//!
//! 背景：概览页原本对 W3C 固定回退到 IRI 末段，导致用户明明填过「名称」，
//! 导出里却看到 `https://www.example.com/NEVDealershipRetailOntology#`。
//! 名称实际落在 `owl:Ontology` 头部的 `rdfs:label`（见 `create_with_charter`
//! → `skeleton_ttl`），因此这里走真实 store：create → 装配 → 读概览字段。

use ontology_store::mapping::MappingExportScope;
use ontology_store::ttl::TtlCharter;
use ontology_store::TtlStore;

const IRI: &str = "https://www.example.com/NEVDealershipRetailOntology#";

fn charter() -> TtlCharter {
    TtlCharter {
        business_scenario: "新能源汽车零售".into(),
        business_essence: "到店到交付的全链路".into(),
        updated_by: "user".into(),
        updated_at: 0,
    }
}

#[test]
fn overview_name_uses_name_given_at_creation() {
    let store = TtlStore::open_in_memory().unwrap();
    // 模拟前端「新建本体——先完成设计宪章」：填了本体编码 + 名称
    store
        .create_with_charter(IRI, "新能源车零售本体", &charter())
        .unwrap();

    let ds = store
        .mapping_dataset(IRI, MappingExportScope::All, None, "2026-11-16 09:06 UTC")
        .unwrap();

    // 1. 概览页「本体名称」= 创建时填的名称，而不是 IRI
    assert_eq!(ds.overview.ontology_name, "新能源车零售本体");
    // 2. 「本体标识」仍保留完整 IRI（对账需要）
    assert_eq!(ds.overview.ontology_key, IRI);
}

#[test]
fn overview_name_survives_agent_full_import_without_label() {
    // 真实链路回归：创建向导填过名称后，agent 会走 export → 改 → import(overwrite)
    // 全量回导，而它提交的 TTL 常常不带 rdfs:label。若名称只存在 TTL 头部，
    // 这一步就把名字抹掉了（导出概览页随即显示成 IRI）。
    let store = TtlStore::open_in_memory().unwrap();
    store
        .create_with_charter(IRI, "新能源车零售本体", &charter())
        .unwrap();

    // agent 全量回导：头部只有 owl:Ontology，没有 rdfs:label
    store
        .import_ttl(
            &format!(
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                 @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                 <{IRI}> a owl:Ontology .\n\
                 <{IRI}Dealership> a owl:Class ; rdfs:label \"门店\" .\n"
            ),
            true,
        )
        .unwrap();

    let ds = store
        .mapping_dataset(IRI, MappingExportScope::All, None, "2026-11-16 09:06 UTC")
        .unwrap();
    assert_eq!(
        ds.overview.ontology_name, "新能源车零售本体",
        "用户填的名称不得被 agent 全量导入抹掉",
    );
}

#[test]
fn overview_name_falls_back_to_local_name_when_label_missing() {
    let store = TtlStore::open_in_memory().unwrap();
    // 直接导入一份头部没有 rdfs:label 的 TTL（模拟手工导入场景）
    store
        .import_ttl(
            &format!(
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                 @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                 <{IRI}> a owl:Ontology .\n\
                 <{IRI}Dealership> a owl:Class ; rdfs:label \"门店\" .\n"
            ),
            false,
        )
        .unwrap();

    let ds = store
        .mapping_dataset(IRI, MappingExportScope::All, None, "2026-11-16 09:06 UTC")
        .unwrap();

    // 无 label 时回退成末段；IRI 以 `#` 收尾，末段为空 → 用整串而不是空串
    assert!(
        !ds.overview.ontology_name.trim().is_empty(),
        "名称不得为空串",
    );
    assert_eq!(ds.overview.ontology_name, IRI);
}

#[test]
fn property_and_relation_object_names_have_no_iri_prefix() {
    let store = TtlStore::open_in_memory().unwrap();
    store
        .create_with_charter(IRI, "新能源车零售本体", &charter())
        .unwrap();
    let r = store
        .import_ttl(
            &format!(
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                 @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                 @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                 <{IRI}> a owl:Ontology .\n\
                 <{IRI}Dealership> a owl:Class ; rdfs:label \"门店\" .\n\
                 <{IRI}ChatRecord> a owl:Class ; rdfs:label \"微信聊天记录\" .\n\
                 <{IRI}Dealership#dealerId> a owl:DatatypeProperty ;\n\
                     rdfs:domain <{IRI}Dealership> ; rdfs:range xsd:string ;\n\
                     rdfs:label \"门店编号\" .\n\
                 <{IRI}ChatRecord#mentions> a owl:ObjectProperty ;\n\
                     rdfs:domain <{IRI}ChatRecord> ; rdfs:range <{IRI}Dealership> ;\n\
                     rdfs:label \"提及门店\" .\n"
            ),
            true,
        )
        .unwrap();
    assert!(r.errors.is_empty(), "import errors: {:?}", r.errors);

    let ds = store
        .mapping_dataset(IRI, MappingExportScope::All, None, "2026-11-16 09:06 UTC")
        .unwrap();

    // 属性页「对象名称」去前缀，中文描述另列保留
    let prop = ds
        .properties
        .iter()
        .find(|r| r.name == "dealerId")
        .expect("dealerId 属性行应存在");
    assert_eq!(prop.owner_name, "Dealership");
    assert_eq!(prop.owner_desc, "门店");

    // 关系页「源对象 / 目标对象」同样去前缀
    let rel = ds
        .relations
        .iter()
        .find(|r| r.name == "mentions")
        .expect("mentions 关系行应存在");
    assert_eq!(rel.source_object, "ChatRecord");
    assert_eq!(rel.target_object, "Dealership");
    assert!(!rel.source_object.contains("://"));
    assert!(!rel.target_object.contains("://"));

    // 可选：落一份真实 xlsx 供外部工具回读（OpenSpec 变更的人工验收路径）
    if let Ok(out) = std::env::var("ONTO_W3C_XLSX_OUT") {
        let bytes = store
            .mapping_workbook(IRI, MappingExportScope::All, None, "2026-11-16 09:06 UTC")
            .unwrap();
        std::fs::write(&out, &bytes).unwrap();
        eprintln!("已写出：{out}（{} 字节）", bytes.len());
    }
}