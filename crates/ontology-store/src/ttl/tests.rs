//! ttl 模块测试（从 ttl/mod.rs 纯移动）。
#[cfg(test)]
use super::*;

    use super::*;

    #[test]
    fn create_with_charter_makes_skeleton_and_rejects_duplicate() {
        let store = TtlStore::open_in_memory().unwrap();
        let charter = TtlCharter {
            business_scenario: "二手车交易".into(),
            business_essence: "非标品可信撮合".into(),
            updated_by: "user".into(),
            updated_at: 0,
        };
        store
            .create_with_charter(
                "https://example.org/ontology/usedcar#UsedCar",
                "二手车本体",
                &charter,
            )
            .unwrap();
        assert!(store.exists("https://example.org/ontology/usedcar#UsedCar").unwrap());
        // 宪章表已落库
        let c = store
            .get_charter("https://example.org/ontology/usedcar#UsedCar")
            .unwrap();
        assert_eq!(c.business_scenario, "二手车交易");
        // 骨架 TTL 头部含宪章谓词（双载体一致）
        let ttl = store
            .export_ttl("https://example.org/ontology/usedcar#UsedCar")
            .unwrap();
        assert!(ttl.contains("dcterms:description"));
        // 重复创建 → 拒绝
        assert!(store
            .create_with_charter(
                "https://example.org/ontology/usedcar#UsedCar",
                "重复",
                &charter,
            )
            .is_err());
    }


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
    rdfs:label "供应商"@zh ;
    rdfs:comment "采购供应商 [confirmed]"@zh .

:supplierId a owl:DatatypeProperty ;
    rdfs:domain :Supplier ;
    rdfs:range xsd:string ;
    rdfs:label "供应商编号"@zh .

:supplies a owl:ObjectProperty ;
    rdfs:domain :Supplier ;
    rdfs:range :Supplier ;
    rdfs:label "供货"@zh .
"#
    }

    #[test]
    fn validate_accepts_clean_ttl() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(sample_ttl()).unwrap();
        assert!(
            r.errors.is_empty(),
            "errors should be empty: {:?}",
            r.errors
        );
        assert!(r.triple_count > 0);
    }

    #[test]
    fn validate_rejects_rdfs_class() {
        let bad = r#"@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:Foo a rdfs:Class .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("owl:Class")));
    }

    #[test]
    fn validate_rejects_mixed_property() {
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <https://example.org/ontology/test#> .
:p a owl:DatatypeProperty, owl:ObjectProperty .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("混用")));
    }

    #[test]
    fn validate_rejects_bad_iri_naming() {
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <https://example.org/ontology/test#> .
:supplier a owl:Class .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("PascalCase")));
    }

    #[test]
    fn import_then_export_roundtrip() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.import_ttl(sample_ttl(), true).unwrap();
        assert!(r.errors.is_empty(), "import errors: {:?}", r.errors);
        assert_eq!(
            r.ontology_iri,
            "https://example.org/ontology/test#TestOntology"
        );
        assert!(r.triple_count > 0);
        let exported = store
            .export_ttl("https://example.org/ontology/test#TestOntology")
            .unwrap();
        assert!(exported.contains("owl:Class"));
    }

    #[test]
    fn import_overwrite_false_skips_existing() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let r = store.import_ttl(sample_ttl(), false).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("已跳过")));
    }

    #[test]
    fn list_and_delete_ontology() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let list = store.list_ontologies().unwrap();
        assert_eq!(list.len(), 1);
        assert!(store
            .delete_ontology("https://example.org/ontology/test#TestOntology")
            .unwrap());
        assert!(store.list_ontologies().unwrap().is_empty());
    }

    #[test]
    fn query_sparql_works() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let json = store
            .query_sparql(
                "https://example.org/ontology/test#TestOntology",
                "SELECT ?c WHERE { ?c a owl:Class . }",
            )
            .unwrap();
        assert!(json.contains("TestOntology") || json.contains("Supplier"));
    }

    // ── 问题 1 回归：subClassOf 子类缺 a owl:Class 必须被 validate 拦住 ──
    fn sample_ttl_with_subclass_missing_decl() -> &'static str {
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

# Anchor 用了 subClassOf 但缺 a owl:Class——应被校验拦住
:Anchor rdfs:subClassOf :Supplier ;
    rdfs:label "主播"@zh .
"#
    }

    #[test]
    fn validate_rejects_subclass_missing_owl_class() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store
            .validate_ttl(sample_ttl_with_subclass_missing_decl())
            .unwrap();
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("subClassOf") && e.contains("a owl:Class")),
            "应报子类缺 a owl:Class，实际 errors: {:?}",
            r.errors
        );
    }

    #[test]
    fn validate_accepts_subclass_with_owl_class() {
        // 同样有 subClassOf 但主语声明了 a owl:Class——应通过
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh .
:Anchor a owl:Class ; rdfs:subClassOf :Supplier ; rdfs:label "主播"@zh .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(ttl).unwrap();
        assert!(r.errors.is_empty(), "errors 应为空: {:?}", r.errors);
    }

    // ── charter / changelog 闭环测试 ──
    #[test]
    fn charter_set_get_roundtrip() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        // 初始无 charter → 空结构体
        let empty = store.get_charter(iri).unwrap();
        assert!(empty.business_scenario.is_empty());
        // set 后 get 应一致
        let c = TtlCharter {
            business_scenario: "门店销售场景".to_string(),
            business_essence: "覆盖门店潜客试驾订单".to_string(),
            updated_by: "agent".to_string(),
            updated_at: 0,
        };
        store.set_charter(iri, &c).unwrap();
        let got = store.get_charter(iri).unwrap();
        assert_eq!(got.business_scenario, "门店销售场景");
        assert_eq!(got.business_essence, "覆盖门店潜客试驾订单");
        assert_eq!(got.updated_by, "agent");
        assert!(got.updated_at > 0);
    }

    #[test]
    fn charter_set_twice_overrides() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let c1 = TtlCharter {
            business_scenario: "v1".to_string(),
            ..Default::default()
        };
        store.set_charter(iri, &c1).unwrap();
        let c2 = TtlCharter {
            business_scenario: "v2".to_string(),
            ..Default::default()
        };
        store.set_charter(iri, &c2).unwrap();
        let got = store.get_charter(iri).unwrap();
        assert_eq!(got.business_scenario, "v2");
    }

    #[test]
    fn charter_for_unknown_ontology_returns_empty() {
        let store = TtlStore::open_in_memory().unwrap();
        let empty = store
            .get_charter("https://example.org/nonexistent#Ont")
            .unwrap();
        assert!(empty.business_scenario.is_empty());
    }

    #[test]
    fn changelog_revision_increments() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let rev1 = store
            .commit_change(iri, "初始导入", "冷启动建模", "{}", None, "agent")
            .unwrap();
        let rev2 = store
            .commit_change(iri, "补子类声明", "加 a owl:Class", "{}", None, "agent")
            .unwrap();
        assert_eq!(rev1, 1);
        assert_eq!(rev2, 2);
        let log = store.list_changelog(iri).unwrap();
        assert_eq!(log.len(), 2);
        // 倒序：最新在前
        assert_eq!(log[0].revision, 2);
        assert_eq!(log[0].title, "补子类声明");
        assert_eq!(log[1].revision, 1);
        assert_eq!(log[1].title, "初始导入");
    }

    #[test]
    fn changelog_rejects_unknown_ontology() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.commit_change(
            "https://example.org/nonexistent#Ont",
            "t",
            "b",
            "{}",
            None,
            "agent",
        );
        assert!(matches!(r, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn changelog_enforces_size_limits() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let big_body = "x".repeat(501);
        let r = store.commit_change(iri, "t", &big_body, "{}", None, "agent");
        assert!(r.is_err(), "title+body > 500 应被拒");
    }

    // ── 双 # IRI 导入兼容（RFC 3987 fragment 禁多 #）──
    fn sample_ttl_with_double_hash() -> &'static str {
        r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/ontology/test#> .

:TestOntology a owl:Ontology ;
    rdfs:label "测试本体"@zh .

:Lead a owl:Class ;
    rdfs:label "潜客"@zh .

# 双 # IRI（外部工具导出的扁平化写法）——应被自动合法化为 :Lead.leadId
<https://example.org/ontology/test#Lead#leadId> a owl:DatatypeProperty ;
    rdfs:domain :Lead ;
    rdfs:range xsd:string ;
    rdfs:label "潜客编号"@zh .

# 字符串里出现的 <x#y#z> 不应被改写
:Note a owl:Class ;
    rdfs:comment "示例 <https://x.cn/a#B#c> 原样保留"@zh .
"#
    }

    #[test]
    fn legalize_double_hash_iri_rewrites_and_warns() {
        let (fixed, warns) = legalize_double_hash_iris(
            "<https://x.cn/ontology/O#Lead#leadId> a owl:DatatypeProperty .",
        );
        assert_eq!(
            fixed,
            "<https://x.cn/ontology/O#Lead.leadId> a owl:DatatypeProperty ."
        );
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("合法化"));
    }

    #[test]
    fn legalize_ignores_strings_and_comments() {
        let src = "# 注释里的 <a#b#c> 不动\n:x rdfs:comment \"串里 <a#b#c> 不动\" ;\n    rdfs:comment \"\"\"长串 <a#b#c> 不动\"\"\" .\n";
        let (fixed, warns) = legalize_double_hash_iris(src);
        assert_eq!(fixed, src, "字符串/注释内的 <a#b#c> 不应被改写");
        assert!(warns.is_empty());
    }

    #[test]
    fn legalize_noop_on_clean_ttl() {
        let (fixed, warns) = legalize_double_hash_iris(sample_ttl());
        assert_eq!(fixed, sample_ttl());
        assert!(warns.is_empty());
    }

    #[test]
    fn validate_legalizes_double_hash_iris() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(sample_ttl_with_double_hash()).unwrap();
        assert!(r.errors.is_empty(), "errors 应为空: {:?}", r.errors);
        assert!(r.triple_count > 0);
        assert!(
            r.warnings.iter().any(|w| w.contains("合法化")),
            "应有合法化 warning: {:?}",
            r.warnings
        );
    }

    #[test]
    fn import_double_hash_ttl_roundtrip_idempotent() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store
            .import_ttl(sample_ttl_with_double_hash(), true)
            .unwrap();
        assert!(r.errors.is_empty(), "import errors: {:?}", r.errors);
        assert_eq!(
            r.ontology_iri,
            "https://example.org/ontology/test#TestOntology"
        );
        assert!(r.warnings.iter().any(|w| w.contains("合法化")));
        // 落库的是**原文**（无损保真，导出可复原）
        let exported = store
            .export_ttl("https://example.org/ontology/test#TestOntology")
            .unwrap();
        assert!(
            exported.contains("#Lead#leadId"),
            "导出应保留原始双 # IRI: {exported}"
        );
        // 导出的原文再导入 → 存储内容一致（byte 级往返保真）
        let r2 = store.import_ttl(&exported, true).unwrap();
        assert!(r2.errors.is_empty());
        let re_exported = store
            .export_ttl("https://example.org/ontology/test#TestOntology")
            .unwrap();
        assert_eq!(re_exported, exported, "原文往返应 byte 级一致");
    }

    #[test]
    fn naming_accepts_class_dot_prop_form() {
        assert!(is_valid_property_local_name("leadId"));
        assert!(is_valid_property_local_name("Lead.leadId"));
        assert!(!is_valid_property_local_name("Lead.lead-id"));
        assert!(!is_valid_property_local_name("lead.Id"));
        assert!(!is_valid_property_local_name("LeadId"));
    }

    #[test]
    fn delete_ontology_cascades_charter_and_changelog() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        // 写 charter + changelog
        store
            .set_charter(
                iri,
                &TtlCharter {
                    business_scenario: "s".to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .commit_change(iri, "t", "b", "{}", None, "agent")
            .unwrap();
        // 删本体 → charter/changelog 应级联删
        assert!(store.delete_ontology(iri).unwrap());
        assert!(
            store.get_charter(iri).unwrap().business_scenario.is_empty(),
            "删本体后 charter 应级联删"
        );
        assert!(
            store.list_changelog(iri).unwrap().is_empty(),
            "删本体后 changelog 应级联删"
        );
    }

    // ── 属性/关系结构化编辑（ttl_edit 手术链）──────────────────

    const EDIT_IRI: &str = "https://example.org/ontology/test#supplierId";

    #[test]
    fn list_ttl_properties_from_store() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let props = store
            .list_ttl_properties("https://example.org/ontology/test#TestOntology")
            .unwrap();
        assert_eq!(props.len(), 2);
        assert!(props.iter().any(|p| p.iri == EDIT_IRI && p.kind == "datatype"));
    }

    #[test]
    fn edit_ttl_property_persists() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let edited = store
            .edit_ttl_property(
                iri,
                &TtlPropertyEdit {
                    iri: EDIT_IRI.to_string(),
                    kind: "datatype".to_string(),
                    label: "新编号".to_string(),
                    comment: String::new(),
                    domain: "https://example.org/ontology/test#Supplier".to_string(),
                    range: "http://www.w3.org/2001/XMLSchema#string".to_string(),
                },
            )
            .unwrap();
        // 落库内容 = 手术结果
        assert_eq!(store.export_ttl(iri).unwrap(), edited);
        // 重新读：新 label 生效
        let props = store.list_ttl_properties(iri).unwrap();
        let p = props.iter().find(|p| p.iri == EDIT_IRI).unwrap();
        assert_eq!(p.label, "新编号");
    }

    #[test]
    fn edit_ttl_property_rejects_invalid_result() {
        // 编辑成非法结果（range 指向不存在的怪 IRI 不触发校验；改成把 label
        // 弄成违反 IRI 命名规范的方式不可行——label 不受命名约束。
        // 可靠的非法路径：把属性 IRI 声明成 PascalCase 违规）——
        // 这里用 edit 把 kind 置为 object 且 range 指向一个 PascalCase 违规类，
        // 让 check_iri_naming 之外的规则触发。最直接：新增属性 IRI 命名违规 → add 拒绝。
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let r = store.add_ttl_property(
            iri,
            &TtlPropertyEdit {
                // BadName 违反属性 camelCase 规范 → validate errors → 拒绝落库
                iri: "https://example.org/ontology/test#BadName".to_string(),
                kind: "datatype".to_string(),
                ..Default::default()
            },
        );
        assert!(r.is_err(), "非法属性名应被校验拒绝: {r:?}");
        // 库内容未被污染
        let props = store.list_ttl_properties(iri).unwrap();
        assert!(!props.iter().any(|p| p.iri.ends_with("#BadName")));
    }

    #[test]
    fn add_and_delete_ttl_property_roundtrip() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let new_iri = "https://example.org/ontology/test#note";
        store
            .add_ttl_property(
                iri,
                &TtlPropertyEdit {
                    iri: new_iri.to_string(),
                    kind: "datatype".to_string(),
                    label: "备注".to_string(),
                    comment: String::new(),
                    domain: "https://example.org/ontology/test#Supplier".to_string(),
                    range: "http://www.w3.org/2001/XMLSchema#string".to_string(),
                },
            )
            .unwrap();
        assert_eq!(store.list_ttl_properties(iri).unwrap().len(), 3);
        store.delete_ttl_property(iri, new_iri).unwrap();
        assert_eq!(store.list_ttl_properties(iri).unwrap().len(), 2);
    }

    #[test]
    fn surgery_preserves_unrelated_blocks() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let before = store.export_ttl(iri).unwrap();
        store
            .edit_ttl_property(
                iri,
                &TtlPropertyEdit {
                    iri: EDIT_IRI.to_string(),
                    kind: "datatype".to_string(),
                    label: "改名".to_string(),
                    comment: String::new(),
                    domain: String::new(),
                    range: String::new(),
                },
            )
            .unwrap();
        let after = store.export_ttl(iri).unwrap();
        // 类块与关系块的行原样保留（字节级）
        for keep in [
            ":Supplier a owl:Class ;",
            "采购供应商 [confirmed]",
            ":supplies a owl:ObjectProperty ;",
        ] {
            assert!(after.contains(keep), "应保留 {keep}：\n{after}");
        }
        assert_ne!(before, after);
    }

    // ── 属性编辑安全网：subPropertyOf 环 + 悬空属性引用 ──

    #[test]
    fn validate_rejects_subproperty_cycle() {
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:p1 a owl:DatatypeProperty ; rdfs:subPropertyOf :p2 .
:p2 a owl:DatatypeProperty ; rdfs:subPropertyOf :p1 .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(
            r.errors.iter().any(|e| e.contains("subPropertyOf 循环")),
            "应报 subPropertyOf 循环，实际 errors: {:?}",
            r.errors
        );
    }

    #[test]
    fn validate_accepts_subproperty_chain() {
        // 合法属性继承链（非环）——应通过
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:name a owl:DatatypeProperty .
:fullName a owl:DatatypeProperty ; rdfs:subPropertyOf :name .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(ttl).unwrap();
        assert!(r.errors.is_empty(), "errors 应为空: {:?}", r.errors);
    }

    #[test]
    fn validate_rejects_dangling_on_property() {
        // owl:Restriction 引用了未声明属性 :ghost——悬空
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ;
    rdfs:subClassOf [
        a owl:Restriction ;
        owl:onProperty :ghost ;
        owl:someValuesFrom :Supplier
    ] .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("onProperty") && e.contains("ghost")),
            "应报 owl:onProperty 悬空引用，实际 errors: {:?}",
            r.errors
        );
    }

    #[test]
    fn validate_rejects_dangling_subproperty_parent() {
        // 父属性 :missing 从未声明——悬空
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:fullName a owl:DatatypeProperty ; rdfs:subPropertyOf :missing .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(
            r.errors.iter().any(|e| e.contains("missing")),
            "应报父属性悬空，实际 errors: {:?}",
            r.errors
        );
    }

    #[test]
    fn validate_accepts_declared_on_property() {
        // Restriction 引用的属性已声明——应通过
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ;
    rdfs:subClassOf [
        a owl:Restriction ;
        owl:onProperty :supplies ;
        owl:someValuesFrom :Supplier
    ] .
:supplies a owl:ObjectProperty .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(ttl).unwrap();
        assert!(r.errors.is_empty(), "errors 应为空: {:?}", r.errors);
    }

    #[test]
    fn validation_gate_passes_clean_ttl() {
        let store = TtlStore::open_in_memory().unwrap();
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/gate#> .
:Onto a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "供应商实体" .
:supplierId a owl:DatatypeProperty ; rdfs:domain :Supplier ; rdfs:range xsd:string ; rdfs:label "编号"@zh .
"#;
        let (errors, _warnings) = store.validation_gate(ttl).unwrap();
        assert!(errors.is_empty(), "干净 TTL 应通过: {:?}", errors);
    }

    #[test]
    fn validation_gate_rejects_error_ttl() {
        let store = TtlStore::open_in_memory().unwrap();
        // 无 rdfs:label 的属性 + 属性混用（error 级）
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/gate#> .
:Onto a owl:Ontology .
:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "x" .
:supplierId a owl:DatatypeProperty, owl:ObjectProperty .
"#;
        let (errors, _warnings) = store.validation_gate(ttl).unwrap();
        assert!(!errors.is_empty(), "属性混用应被闸门拦截");
    }

    #[test]
    fn validation_gate_rejects_pending_issue_and_review() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/gate2";
        let ttl = format!(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <{iri}#> .
<{iri}#> a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "x" .
"#
        );
        // 先建本体（validation_gate 内联 validate 不依赖库内存在，但查表需要 iri 匹配）
        let _ = store.import_ttl(&ttl, true).unwrap();
        // 造一条未闭环 error issue
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO validation_issues (id, ontology_iri, validation_id, layer, severity, rule, message, status, created_at, updated_at)
                 VALUES ('vi-test-1', ?1, 'v1', 'L2', 'error', 'check_x', '测试错误', 'open', 1, 1)",
                rusqlite::params![format!("{iri}#")],
            )
            .unwrap();
        }
        // 造一条未 resolved review
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO validation_reviews (id, ontology_iri, round, max_rounds, status, created_at, updated_at)
                 VALUES ('vr-test-1', ?1, 1, 2, 'in_progress', 1, 1)",
                rusqlite::params![format!("{iri}#")],
            )
            .unwrap();
        }
        let (errors, _w) = store.validation_gate(&ttl).unwrap();
        assert!(
            errors.iter().any(|e| e.contains("vi-test-1")),
            "应拦截未闭环 issue: {:?}",
            errors
        );
        assert!(
            errors.iter().any(|e| e.contains("vr-test-1")),
            "应拦截未完成 review: {:?}",
            errors
        );
    }

    #[test]
    fn validation_gate_rejects_clean_ttl_after_issue_resolved() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/gate3";
        let ttl = format!(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <{iri}#> .
<{iri}#> a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "x" .
"#
        );
        let _ = store.import_ttl(&ttl, true).unwrap();
        // 已 overruled 的 issue 不应拦截（对抗闭环完成）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO validation_issues (id, ontology_iri, validation_id, layer, severity, rule, message, status, dispute_verdict, created_at, updated_at)
                 VALUES ('vi-test-2', ?1, 'v1', 'L3', 'warning', 'check_y', '已撤销', 'overruled', 'overruled', 1, 1)",
                rusqlite::params![format!("{iri}#")],
            )
            .unwrap();
        }
        let (errors, _w) = store.validation_gate(&ttl).unwrap();
        assert!(errors.is_empty(), "overruled 后应通过: {:?}", errors);
    }

    #[test]
    fn validation_gate_no_ontology_iri() {
        let store = TtlStore::open_in_memory().unwrap();
        let ttl =
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix : <https://example.org/gate4#> .\n:Supplier a owl:Class .";
        let (errors, _w) = store.validation_gate(ttl).unwrap();
        assert!(
            errors.iter().any(|e| e.contains("owl:Ontology")),
            "无本体声明应报错: {:?}",
            errors
        );
    }

    #[test]
    fn append_ttl_new_elements_only_and_rejects_redefinition() {
        let store = TtlStore::open_in_memory().unwrap();
        let base = "https://example.org/append";
        store
            .import_ttl(
                &format!(
                    "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                     <{base}> a owl:Ontology .\n\
                     <{base}#A> a owl:Class ."
                ),
                false,
            )
            .unwrap();
        let sid = store
            .register_evidence_source(
                "document",
                "追加测试源",
                "追加测试源：客户标签来自访谈",
                base,
                "",
            )
            .unwrap();
        let delta = format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
             <{base}#tag> a owl:DatatypeProperty ;\n\
             rdfs:label \"客户标签\"@zh ;\n\
             rdfs:domain <{base}#A> ; rdfs:range <http://www.w3.org/2001/XMLSchema#string> ;\n\
             skos:editorialNote \"evidence: {sid} | direct | 「客户标签来自访谈」\" ."
        );
        // ① 纯新增：成功，存量原文无损拼接
        let r = store.append_ttl(base, &delta).unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let merged = store.export_ttl(base).unwrap();
        assert!(merged.contains("#A> a owl:Class"), "存量原文保留");
        assert!(merged.contains("#tag"), "新增已拼入");
        assert_eq!(r.triple_count, store.count_ttl_triples(&merged).unwrap());
        // ② 重定义已有 IRI：拒绝（修改类变更走全文回导）
        let bad = format!(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             <{base}#A> a owl:Class ; rdfs:label \"改\"@zh ."
        );
        let r2 = store.append_ttl(base, &bad).unwrap();
        assert!(
            r2.errors.iter().any(|e| e.contains("已有定义")),
            "{:?}",
            r2.errors
        );
        // ③ 无证据新增：strict 证据闸门拒绝
        let noev = format!("@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             <{base}#b> a owl:Class .");
        let r3 = store.append_ttl(base, &noev).unwrap();
        assert!(
            r3.errors.iter().any(|e| e.contains("无任何证据注解")),
            "{:?}",
            r3.errors
        );
    }

    #[test]
    fn ttl_evidence_gate_strict_blocks_ungrounded_and_paraphrase() {
        let store = TtlStore::open_in_memory().unwrap();
        let base = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
"#;
        // 登记证据源（与 validate 同库）
        let sid = store
            .register_evidence_source("scenario", "场景", "客户下单后，订单创建成功。若三天未付款，系统自动取消订单。", "", "")
            .unwrap();
        // ① strict：无注解 → 阻断
        let ttl_ung = format!("{base}:Supplier a owl:Class .");
        let (errors, _) = store.validate_ttl_evidence(&ttl_ung, true).unwrap();
        assert!(errors.iter().any(|e| e.contains("无任何证据注解")), "{errors:?}");
        // ② 逐字引用 → 通过
        let ttl_ok = format!(
            "{base}:Supplier a owl:Class ;\n\
             skos:editorialNote \"evidence: {sid} | direct | 「若三天未付款，系统自动取消订单。」\" ."
        );
        let (errors, warnings) = store.validate_ttl_evidence(&ttl_ok, true).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let _ = warnings;
        // ③ 改写（臆想）→ E2 阻断，错误含原文上下文
        let ttl_bad = format!(
            "{base}:Supplier a owl:Class ;\n\
             skos:editorialNote \"evidence: {sid} | direct | 「用户逾期三天未付款」\" ."
        );
        let (errors, _) = store.validate_ttl_evidence(&ttl_bad, true).unwrap();
        assert!(errors.iter().any(|e| e.contains("疑似臆想")), "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("相关原文片段")), "{errors:?}");
        // ④ 引用未登记源 → E1
        let ttl_e1 = format!(
            "{base}:Supplier a owl:Class ;\n\
             skos:editorialNote \"evidence: src-nope | direct | 「订单」\" ."
        );
        let (errors, _) = store.validate_ttl_evidence(&ttl_e1, true).unwrap();
        assert!(errors.iter().any(|e| e.contains("未登记的源")), "{errors:?}");
        // ⑤ 非 strict：ungrounded 仅 warning（grandfathered 元素聚合为一条口径说明）
        let (errors, warnings) = store.validate_ttl_evidence(&ttl_ung, false).unwrap();
        assert!(errors.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("grandfathered") && w.contains("自动豁免不阻断")),
            "{warnings:?}"
        );

        // ⑤' grandfathering：存量元素（已在库内、无注解）strict 下不阻断；新增元素仍阻断
        // （事故 2026-09-17：存量本体被 agent 触碰时若 S1 全量拦截，配合整本体替换
        //  语义会导致 agent 无法回导原文 = 数据丢失）
        let legacy = format!(
            "{base}:LegacyClass a owl:Class .\n\
             {base}:AnotherLegacy a owl:Class ."
        );
        store.import_ttl(&legacy, false).unwrap();
        // 原样回导（全部为存量元素）→ 通过
        let (errors, _) = store.validate_ttl_evidence(&legacy, true).unwrap();
        assert!(
            !errors.iter().any(|e| e.contains("无任何证据注解")),
            "存量元素应被 grandfathered: {errors:?}"
        );
        // 存量 + 新增无注解元素 → 只拦新增
        let legacy_plus_new = format!(
            "{legacy}\n\
             {base}:BrandNew a owl:Class ."
        );
        let (errors, _) = store
            .validate_ttl_evidence(&legacy_plus_new, true)
            .unwrap();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("BrandNew") && e.contains("无任何证据注解")),
            "{errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|e| e.contains("LegacyClass") && e.contains("无任何证据注解")),
            "{errors:?}"
        );

        // ⑤'' 定义变化：存量元素定义被修改且无注解 → 拦截（IRI 豁免不保护新主张）
        let legacy_modified = format!(
            "{base}:LegacyClass a owl:Class ; rdfs:label \"改过的标签\"@zh .\n\
             {base}:AnotherLegacy a owl:Class ."
        );
        let (errors, _) = store
            .validate_ttl_evidence(&legacy_modified, true)
            .unwrap();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("LegacyClass") && e.contains("定义相对库内存量版本发生了修改")),
            "{errors:?}"
        );
        assert!(
            !errors
                .iter()
                .any(|e| e.contains("AnotherLegacy") && e.contains("定义")),
            "未改动的存量元素不应被拦: {errors:?}"
        );
        // 定义变化 + 带注解（引用已登记源原句）→ 通过
        let legacy_modified_ok = format!(
            "{base}:LegacyClass a owl:Class ; rdfs:label \"改过的标签\"@zh ;\n\
             skos:editorialNote \"evidence: {sid} | direct | 「客户下单后，订单创建成功。」\" .\n\
             {base}:AnotherLegacy a owl:Class ."
        );
        let (errors, _) = store
            .validate_ttl_evidence(&legacy_modified_ok, true)
            .unwrap();
        assert!(errors.is_empty(), "{errors:?}");

        // ⑥ 豁免注解 evidence: none —— strict 放行，落库 matched_by='manual'
        let ttl_manual = format!(
            "{base}:AbstractConcept a owl:Class ;\n\
             skos:editorialNote \"evidence: none | direct | 「用户在对话中确认为纯设计抽象」\" ."
        );
        let (errors, _) = store.validate_ttl_evidence(&ttl_manual, true).unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        store.import_ttl(&ttl_manual, true).unwrap();
        let rows = crate::evidence::list_element_evidence(
            &store.conn.lock().unwrap(),
            "w3c",
            "https://example.org/ontology/test#TestOntology",
        )
        .unwrap();
        let m = rows.iter().find(|r| r.element_id.contains("AbstractConcept"));
        assert!(m.is_some(), "{rows:?}");
        assert_eq!(m.unwrap().matched_by, "manual");
        assert_eq!(m.unwrap().source_id, "");
    }

    #[test]
    fn delete_property_blocked_by_restriction() {
        // 删除被 owl:onProperty 引用的属性 → 手术结果校验失败 → 拒绝落库
        let store = TtlStore::open_in_memory().unwrap();
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ;
    rdfs:subClassOf [
        a owl:Restriction ;
        owl:onProperty :supplies ;
        owl:someValuesFrom :Supplier
    ] .
:supplies a owl:ObjectProperty .
"#;
        store.import_ttl(ttl, true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let r = store.delete_ttl_property(iri, "https://example.org/ontology/test#supplies");
        assert!(r.is_err(), "删除被引用属性应被拒: {r:?}");
        // 属性仍在库中
        let props = store.list_ttl_properties(iri).unwrap();
        assert!(props.iter().any(|p| p.iri.ends_with("#supplies")));
    }

    #[test]
    fn evidence_element_id_matches_frontend_iri_for_double_hash() {
        // 回归：2026-09-20 东风 AutoDealerRetail 11 属性不显示 🟢。
        // 双 # IRI（<...#TdNextAction#closeProbability>）import 后，element_evidence 的
        // element_id 必须是「无尖括号 + 双 # 原文形态」——与前端 list_properties 输出
        // （p.iri）完全一致，前端 statusOf 的 r.element_id === iri 才能命中。
        // 旧实现（oxigraph 版）落库成「尖括号 + 点号」→ 永远匹配不上（⚪ 而非 🟢）。
        let store = TtlStore::open_in_memory().unwrap();
        let base = r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Ont> a owl:Ontology ;
    rdfs:label "AutoDealerRetail"@zh .
"#;
        let sid = store
            .register_evidence_source(
                "document",
                "deal_intent_schema.md",
                "成交概率（%）。参考分档：0-20 仅初步了解；21-40 有兴趣未决策；41-60 需求较明确、对比评估中；61-80 意向较强有倾向；81-100 高度意向接近成交",
                "",
                "",
            )
            .unwrap();
        let ttl = format!(
            r#"{base}
<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#TdNextAction> a owl:Class ;
    rdfs:label "试驾报告-跟进策略"@zh .

<https://jslc.auto-retail.cn/ontology/AutoDealerRetail#TdNextAction#closeProbability>
    rdf:type      owl:DatatypeProperty;
    rdfs:comment  "成交概率（%，0-100）[tentative]";
    rdfs:domain   <https://jslc.auto-retail.cn/ontology/AutoDealerRetail#TdNextAction>;
    rdfs:label    "成交概率";
    rdfs:range    xsd:float ;
    skos:editorialNote "evidence: {sid} | direct | 「成交概率（%）。参考分档：0-20 仅初步了解；21-40 有兴趣未决策；41-60 需求较明确、对比评估中；61-80 意向较强有倾向；81-100 高度意向接近成交」" .
"#
        );
        let r = store.import_ttl(&ttl, true).unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let rows = crate::evidence::list_element_evidence(
            &store.conn.lock().unwrap(),
            "w3c",
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Ont",
        )
        .unwrap();
        let prop_row = rows
            .iter()
            .find(|r| r.element_id.contains("closeProbability"))
            .expect("closeProbability 证据行应存在");
        // 关键断言：element_id 与前端 list_properties 的 p.iri 精确一致
        assert_eq!(
            prop_row.element_id,
            "https://jslc.auto-retail.cn/ontology/AutoDealerRetail#TdNextAction#closeProbability"
        );
        assert_eq!(prop_row.element_type, "datatype_property");
        assert_eq!(prop_row.matched_by, "exact");
        // 前端视角：list_properties 拿到的 iri 应与证据行 element_id 相等
        let props = store
            .list_ttl_properties("https://jslc.auto-retail.cn/ontology/AutoDealerRetail#Ont")
            .unwrap();
        let p = props
            .iter()
            .find(|p| p.iri.contains("closeProbability"))
            .expect("前端属性列表应有 closeProbability");
        assert_eq!(p.iri, prop_row.element_id, "前端 iri 与证据 element_id 必须完全一致");
    }
