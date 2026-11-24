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

    /// 删本体 → validation_issues/validation_reviews 级联删：孤儿行会让其他
    /// 本体 persist 同名规则 id 撞全局主键（UNIQUE constraint failed）。
    #[test]
    fn delete_ontology_cascades_validation_issues() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        store.import_ttl(sample_ttl(), true).unwrap();
        store
            .persist_issues(
                iri,
                "v1",
                &[crate::validation::ValidationIssue::error("L3-3", "L3", "c", "m")],
            )
            .unwrap();
        assert!(store.delete_ontology(iri).unwrap());
        assert!(
            store.list_issues(iri).unwrap().is_empty(),
            "删本体后 validation_issues 应级联删"
        );
        // 孤儿行清空后，其他本体落同名规则 id 不再撞全局主键
        store
            .persist_issues(
                "https://example.org/ontology/other#",
                "v2",
                &[crate::validation::ValidationIssue::error("L3-3", "L3", "c", "m")],
            )
            .unwrap();
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
    fn delete_property_also_drops_its_restrictions() {
        // 删除被 owl:onProperty 引用的属性：约束已失去意义，随属性一并摘除。
        // 旧语义是「校验失败 → 拒绝落库」，会让含约束的本体上任何属性/类删除
        // 永远失败（用户明确要删却被技术细节挡住）。
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
        store
            .delete_ttl_property(iri, "https://example.org/ontology/test#supplies")
            .expect("删除应成功并清理其约束");
        let props = store.list_ttl_properties(iri).unwrap();
        assert!(!props.iter().any(|p| p.iri.ends_with("#supplies")));
        let after = store.export_ttl(iri).unwrap();
        assert!(
            !after.contains("onProperty"),
            "不得留下悬空 onProperty: {after}"
        );
        assert!(after.contains(":Supplier a owl:Class"));
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

    // ════════════════════════════════════════════════════════════════
    // 版本管理（ontology_ttl_versions + active_revision）
    // ════════════════════════════════════════════════════════════════

    const VIRI: &str = "https://example.org/ontology/test#TestOntology";

    #[test]
    fn version_publish_import_revert_lifecycle() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        // 未发布：active=NULL
        assert_eq!(store.get_active_revision(VIRI).unwrap(), None);

        // 发布 v1
        let rev1 = store.publish_version(VIRI, "v1.0", "初始版本", "user", None).unwrap();
        assert_eq!(rev1, 1);
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        let versions = store.list_versions(VIRI).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].label, "v1.0");
        assert_eq!(versions[0].author, "user");
        assert!(versions[0].triple_count > 0, "triple_count 应来自 ontology_ttl");
        // 首版无上一版本 → 修改点为空对象
        assert_eq!(versions[0].diff_summary, "{}");
        // 发布写 changelog，且 change_summary 带 published_version 映射
        let changelog = store.list_changelog(VIRI).unwrap();
        assert_eq!(changelog.len(), 1);
        assert!(changelog[0].change_summary.contains("\"published_version\": 1"));

        // 修改 → 置脏，保留生效指针
        let modified = sample_ttl().replace("供应商", "供应商v2");
        store.import_ttl(&modified, true).unwrap();
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        assert!(store.get_workspace_dirty(VIRI).unwrap());
        // import 不留新版本
        assert_eq!(store.list_versions(VIRI).unwrap().len(), 1);

        // 发布 v2
        let rev2 = store.publish_version(VIRI, "v2.0", "改标签", "agent", None).unwrap();
        assert_eq!(rev2, 2);
        assert_eq!(store.list_versions(VIRI).unwrap().len(), 2);
        // v2 的修改点已落库：Supplier 类 label 变（modified_classes）
        let versions = store.list_versions(VIRI).unwrap();
        let diff2: serde_json::Value =
            serde_json::from_str(&versions[0].diff_summary).expect("diff_summary 应为合法 JSON");
        assert_eq!(diff2["modified_classes"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(diff2["rev_a"], 1);
        assert_eq!(diff2["rev_b"], 2);

        // 回退到 v1：切指针不新建版本
        store.revert_to_version(VIRI, 1, "user", None).unwrap();
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        let ttl = store.export_ttl(VIRI).unwrap();
        assert!(ttl.contains("供应商"));
        assert!(!ttl.contains("供应商v2"), "回退后内容应为 v1");
        assert_eq!(store.list_versions(VIRI).unwrap().len(), 2, "回退不新建版本");

        // 回退后可逆：再启用 v2
        store.revert_to_version(VIRI, 2, "user", None).unwrap();
        let ttl = store.export_ttl(VIRI).unwrap();
        assert!(ttl.contains("供应商v2"));

        // 回退后原样再发布 → 内容相同的新版本（预期，发布=新里程碑）
        let rev3 = store.publish_version(VIRI, "v3", "", "user", None).unwrap();
        assert_eq!(rev3, 3);
        // label 为空 → 默认三位数 v0.0.N（N=发布序号）；用户显式填则以其为准
        let rev4 = store.publish_version(VIRI, "", "空标签自动生成", "user", None).unwrap();
        assert_eq!(rev4, 4);
        let versions = store.list_versions(VIRI).unwrap();
        assert_eq!(versions[0].label, "v0.0.4");
        assert_eq!(versions[0].description, "空标签自动生成");
    }

    #[test]
    fn revoke_workspace_diff_item_add_modify_remove_property() {
        use crate::ttl::WorkspaceDiffRevokeItem;
        use crate::ttl_edit::TtlPropertyEdit;

        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        store
            .publish_version(VIRI, "v1", "初始", "user", None)
            .unwrap();

        // 新增属性 → 撤销 add → 属性消失且 dirty 清零
        let new_prop = TtlPropertyEdit {
            iri: "https://example.org/ontology/test#newProp".into(),
            kind: "datatype".into(),
            label: "新属性".into(),
            comment: String::new(),
            domain: "https://example.org/ontology/test#Supplier".into(),
            range: "http://www.w3.org/2001/XMLSchema#string".into(),
        };
        store.add_ttl_property(VIRI, &new_prop).unwrap();
        assert!(store.get_workspace_dirty(VIRI).unwrap());
        store
            .revoke_workspace_diff_item(
                VIRI,
                &WorkspaceDiffRevokeItem::Property {
                    op: "add".into(),
                    iri: new_prop.iri.clone(),
                },
            )
            .unwrap();
        assert!(
            store
                .list_ttl_properties(VIRI)
                .unwrap()
                .iter()
                .all(|p| p.iri != new_prop.iri)
        );
        assert!(
            !store.get_workspace_dirty(VIRI).unwrap(),
            "撤销后与生效版一致应清 dirty"
        );

        // 修改 label → 撤销 modify → 恢复中文名
        let mut edit = store
            .list_ttl_properties(VIRI)
            .unwrap()
            .into_iter()
            .find(|p| p.iri.ends_with("supplierId"))
            .expect("supplierId");
        edit.label = "编号改名".into();
        store.edit_ttl_property(VIRI, &edit).unwrap();
        store
            .revoke_workspace_diff_item(
                VIRI,
                &WorkspaceDiffRevokeItem::Property {
                    op: "modify".into(),
                    iri: edit.iri.clone(),
                },
            )
            .unwrap();
        let restored = store
            .list_ttl_properties(VIRI)
            .unwrap()
            .into_iter()
            .find(|p| p.iri.ends_with("supplierId"))
            .unwrap();
        assert_eq!(restored.label, "供应商编号");

        // 删除属性 → 撤销 remove → 恢复
        store
            .delete_ttl_property(VIRI, &edit.iri)
            .unwrap();
        store
            .revoke_workspace_diff_item(
                VIRI,
                &WorkspaceDiffRevokeItem::Property {
                    op: "remove".into(),
                    iri: edit.iri.clone(),
                },
            )
            .unwrap();
        assert!(
            store
                .list_ttl_properties(VIRI)
                .unwrap()
                .iter()
                .any(|p| p.iri.ends_with("supplierId"))
        );
    }

    #[test]
    fn version_diff_historical_and_current() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        store.publish_version(VIRI, "v1", "初始", "user", None).unwrap();
        let modified = sample_ttl()
            .replace("供应商", "供应商v2")
            .replace("supplierId", "supplierCode");
        store.import_ttl(&modified, true).unwrap();
        store.publish_version(VIRI, "v2", "改类标签+属性改名", "agent", None).unwrap();

        // 历史 vs 历史
        let diff = store.diff_versions(VIRI, 1, 2).unwrap();
        assert_eq!(diff.modified_classes.len(), 1, "Supplier 类 label 变了");
        assert_eq!(diff.modified_classes[0].before.label, "供应商");
        assert_eq!(diff.modified_classes[0].after.label, "供应商v2");
        assert_eq!(diff.removed_properties.len(), 1, "supplierId 改名=旧 IRI 删除");
        assert_eq!(diff.added_properties.len(), 1, "supplierCode 新增");
        assert!(!diff.ontology_meta_changed, "本体头未变");

        // 当前工作区 vs 历史（rev=0 哨兵）：当前=刚发布的 v2 内容
        let diff2 = store.diff_versions(VIRI, 0, 1).unwrap();
        assert_eq!(diff2.modified_classes.len(), 1);
        assert_eq!(diff2.added_properties.len(), 1);
    }

    #[test]
    fn version_diff_includes_subclass_of_edges() {
        let store = TtlStore::open_in_memory().unwrap();
        let with_hierarchy = format!(
            "{}\n:Partner a owl:Class ;\n    rdfs:label \"合作方\"@zh ;\n    rdfs:subClassOf :Supplier .\n",
            sample_ttl()
        );
        store.import_ttl(&with_hierarchy, true).unwrap();
        store.publish_version(VIRI, "v1", "", "user", None).unwrap();

        // 去掉 Partner⊑Supplier，改为 Partner⊑ 新父类（先加 Party 类）
        let mutated = with_hierarchy
            .replace(
                "rdfs:subClassOf :Supplier .",
                "rdfs:subClassOf :Party .",
            )
            .replace(
                ":Partner a owl:Class ;",
                ":Party a owl:Class ;\n    rdfs:label \"主体\"@zh .\n\n:Partner a owl:Class ;",
            );
        store.import_ttl(&mutated, true).unwrap();

        let diff = store.diff_versions(VIRI, 1, 0).unwrap();
        assert!(
            diff.added_classes
                .iter()
                .any(|c| c.ends_with("#Party") || c.ends_with("Party")),
            "新增 Party: {:?}",
            diff.added_classes
        );
        assert_eq!(diff.removed_subclass_of.len(), 1, "{:?}", diff.removed_subclass_of);
        assert!(
            diff.removed_subclass_of[0].parent.ends_with("Supplier")
                || diff.removed_subclass_of[0].parent.contains("Supplier")
        );
        assert_eq!(diff.added_subclass_of.len(), 1, "{:?}", diff.added_subclass_of);
        assert!(
            diff.added_subclass_of[0].parent.ends_with("Party")
                || diff.added_subclass_of[0].parent.contains("Party")
        );

        // 旧 diff_summary JSON 缺 hierarchy 字段 → 反序列化默认空
        let legacy: TtlVersionDiff = serde_json::from_str(
            r#"{"rev_a":1,"rev_b":2,"added_classes":[],"removed_classes":[],"modified_classes":[],"added_properties":[],"removed_properties":[],"modified_properties":[],"ontology_meta_changed":false}"#,
        )
        .unwrap();
        assert!(legacy.added_subclass_of.is_empty());
        assert!(legacy.removed_subclass_of.is_empty());
    }

    #[test]
    fn version_append_and_delete_marks_dirty() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        store.publish_version(VIRI, "v1", "", "user", None).unwrap();
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        assert!(!store.get_workspace_dirty(VIRI).unwrap());

        // append 追加 → 置脏，保留生效指针（delta 元素须带证据注解过闸门）
        let delta = "\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n@prefix : <https://example.org/ontology/test#> .\n\n:extraProp a owl:DatatypeProperty ;\n    rdfs:label \"额外\"@zh ;\n    skos:editorialNote \"evidence: none | direct | 「测试豁免」\" .\n";
        let r = store.append_ttl(VIRI, delta).unwrap();
        assert!(r.errors.is_empty(), "append 应成功: {:?}", r.errors);
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        assert!(store.get_workspace_dirty(VIRI).unwrap());
        assert_eq!(store.list_versions(VIRI).unwrap().len(), 1, "append 不留版本");

        // 删除本体 → versions 级联清（显式兜底）
        store.delete_ontology(VIRI).unwrap();
        assert_eq!(store.list_versions(VIRI).unwrap().len(), 0);
        assert!(!store.exists(VIRI).unwrap());
    }

    #[test]
    fn export_effective_ttl_dirty_unpublished_and_revert() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        // 从未发布 → 拒绝
        let err = store.export_effective_ttl(VIRI).unwrap_err();
        assert!(
            matches!(err, StoreError::Unpublished(_)),
            "从未发布应 Unpublished: {err}"
        );

        store.publish_version(VIRI, "v1", "", "user", None).unwrap();
        let v1 = store.export_effective_ttl(VIRI).unwrap();
        assert!(v1.contains("供应商"));
        assert!(!store.get_workspace_dirty(VIRI).unwrap());

        // 脏导出仍为旧快照
        let modified = sample_ttl().replace("供应商", "供应商v2");
        store.import_ttl(&modified, true).unwrap();
        assert!(store.get_workspace_dirty(VIRI).unwrap());
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        let effective = store.export_effective_ttl(VIRI).unwrap();
        assert!(effective.contains("供应商"));
        assert!(!effective.contains("供应商v2"));
        let draft = store.export_ttl(VIRI).unwrap();
        assert!(draft.contains("供应商v2"));

        store.publish_version(VIRI, "v2", "", "user", None).unwrap();
        assert!(!store.get_workspace_dirty(VIRI).unwrap());
        let v2 = store.export_effective_ttl(VIRI).unwrap();
        assert!(v2.contains("供应商v2"));

        // 切回非最新后再编辑：生效仍为切回目标
        store.revert_to_version(VIRI, 1, "user", None).unwrap();
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        assert!(!store.get_workspace_dirty(VIRI).unwrap());
        let after_revert = sample_ttl().replace("供应商", "供应商draft");
        store.import_ttl(&after_revert, true).unwrap();
        assert!(store.get_workspace_dirty(VIRI).unwrap());
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
        let still_v1 = store.export_effective_ttl(VIRI).unwrap();
        assert!(still_v1.contains("供应商"));
        assert!(!still_v1.contains("供应商v2"));
        assert!(!still_v1.contains("供应商draft"));
    }

    #[test]
    fn version_fifo_trims_oldest_over_limit() {
        let store = TtlStore::open_in_memory().unwrap();
        store.set_version_limit(3).unwrap();
        assert_eq!(store.get_version_limit().unwrap(), 3);
        store.import_ttl(sample_ttl(), false).unwrap();
        for i in 1..=5 {
            store.publish_version(VIRI, &format!("v{i}"), "", "user", None).unwrap();
        }
        let revs: Vec<i64> = store
            .list_versions(VIRI)
            .unwrap()
            .into_iter()
            .map(|v| v.revision)
            .collect();
        assert_eq!(revs, vec![5, 4, 3], "超限删最旧（rev1/rev2 被删）");
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(5));
    }

    #[test]
    fn version_fifo_skips_active_revision() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        for i in 1..=3 {
            store.publish_version(VIRI, &format!("v{i}"), "", "user", None).unwrap();
        }
        // 手动构造"启用最旧版本"状态（回退到 rev1 后），直接触发清理：
        // 若 FIFO 不跳过 active，rev1 被删 → 指针悬空
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE ontology_ttl SET active_revision=1 WHERE ontology_iri=?1",
                rusqlite::params![VIRI],
            )
            .unwrap();
            TtlStore::fifo_trim_conn(&conn, VIRI, 2).unwrap();
        }
        let revs: Vec<i64> = store
            .list_versions(VIRI)
            .unwrap()
            .into_iter()
            .map(|v| v.revision)
            .collect();
        assert_eq!(revs, vec![3, 1], "rev1（active）被跳过，删 rev2");
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1), "指针不悬空");
        // 被跳过的版本仍可回退
        store.revert_to_version(VIRI, 1, "user", None).unwrap();
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
    }

    #[test]
    fn version_delete_single_and_reject_active() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        store.publish_version(VIRI, "v1", "", "user", None).unwrap();
        let modified = sample_ttl().replace("供应商", "供应商v2");
        store.import_ttl(&modified, true).unwrap();
        store.publish_version(VIRI, "v2", "", "user", None).unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        store.publish_version(VIRI, "v3", "", "user", None).unwrap();

        // active=rev3：删除 rev3 被拒
        assert!(store.delete_version(VIRI, 3).is_err());
        // 删除中间版本 rev2（非 active）→ OK，revision 空洞保留
        store.delete_version(VIRI, 2).unwrap();
        let revs: Vec<i64> = store
            .list_versions(VIRI)
            .unwrap()
            .into_iter()
            .map(|v| v.revision)
            .collect();
        assert_eq!(revs, vec![3, 1], "rev2 删除后空洞保留");
        // 删除不存在的版本 → NotFound
        assert!(store.delete_version(VIRI, 99).is_err());
        // 删除不存在本体 → NotFound（行不存在）
        assert!(store
            .delete_version("https://nonexistent.example/ont", 1)
            .is_err());
        // 删除后仍可回退到剩余版本
        store.revert_to_version(VIRI, 1, "user", None).unwrap();
        assert_eq!(store.get_active_revision(VIRI).unwrap(), Some(1));
    }

    #[test]
    fn version_list_backfills_empty_diff_summary() {
        // 模拟旧包发布的版本（diff_summary='{}'）→ list_versions 应惰性补算回填
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        store.publish_version(VIRI, "v1", "", "user", None).unwrap();
        let modified = sample_ttl().replace("供应商", "供应商v2");
        store.import_ttl(&modified, true).unwrap();
        store.publish_version(VIRI, "v2", "", "user", None).unwrap();
        // 手动清空 diff_summary（模拟旧库迁移后的存量行）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE ontology_ttl_versions SET diff_summary='{}' WHERE ontology_iri=?1 AND revision=2",
                rusqlite::params![VIRI],
            )
            .unwrap();
        }
        let versions = store.list_versions(VIRI).unwrap();
        let v2 = versions.iter().find(|v| v.revision == 2).unwrap();
        assert!(v2.diff_summary != "{}", "list_versions 应补算 v2 修改点");
        let diff2: serde_json::Value =
            serde_json::from_str(&v2.diff_summary).expect("补算后应为合法 JSON");
        assert_eq!(diff2["modified_classes"].as_array().map(|a| a.len()), Some(1));
        // 回填已落库（再次查询不再重算）
        let versions = store.list_versions(VIRI).unwrap();
        let v2 = versions.iter().find(|v| v.revision == 2).unwrap();
        assert_ne!(v2.diff_summary, "{}");
    }

    #[test]
    fn version_legacy_db_migration_adds_column_and_table() {
        // 老库升级回归：手工建旧 schema（无 active_revision 列、无 versions 表）→
        // init_ttl_schema 不 panic → 列已补 + 表已建 + 存量数据保留
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ontology_ttl (
                ontology_iri  TEXT PRIMARY KEY NOT NULL,
                ttl_content   TEXT NOT NULL,
                version       TEXT NOT NULL DEFAULT '',
                triple_count  INTEGER NOT NULL DEFAULT 0,
                updated_at    INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE ontology_ttl_changelog (
                id              TEXT PRIMARY KEY,
                ontology_iri    TEXT NOT NULL,
                revision        INTEGER NOT NULL,
                title           TEXT NOT NULL,
                body            TEXT NOT NULL DEFAULT '',
                change_summary  TEXT NOT NULL DEFAULT '{}',
                conversation_id TEXT,
                author          TEXT NOT NULL DEFAULT 'agent',
                created_at      INTEGER NOT NULL,
                UNIQUE(ontology_iri, revision)
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ontology_ttl (ontology_iri, ttl_content, version, triple_count, updated_at) \
             VALUES ('https://legacy.example/ont', 'legacy ttl', '', 3, 1)",
            [],
        )
        .unwrap();
        // 升级
        init_ttl_schema(&conn).unwrap();
        // 列已补
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(ontology_ttl)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(cols.iter().any(|c| c == "active_revision"), "老库应补 active_revision 列");
        assert!(cols.iter().any(|c| c == "workspace_dirty"), "老库应补 workspace_dirty 列");
        // versions 表已建
        let tbls: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ontology_ttl_versions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tbls, 1, "老库应建 ontology_ttl_versions 表");
        // 存量数据保留且未发布
        let ttl: String = conn
            .query_row(
                "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri='https://legacy.example/ont'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ttl, "legacy ttl");
        let active: Option<i64> = conn
            .query_row(
                "SELECT active_revision FROM ontology_ttl WHERE ontology_iri='https://legacy.example/ont'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, None, "存量本体=未发布状态");
        let dirty: i64 = conn
            .query_row(
                "SELECT workspace_dirty FROM ontology_ttl WHERE ontology_iri='https://legacy.example/ont'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dirty, 1, "存量默认 dirty");
    }

    #[test]
    fn revoke_workspace_class_cascade_clears_added_class_package() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), false).unwrap();
        store
            .publish_version(VIRI, "v1", "初始", "user", None)
            .unwrap();

        let chat = "https://example.org/ontology/test#ChatRecord";
        let with_pkg = format!(
            "{}\n:ChatRecord a owl:Class ;\n    rdfs:label \"微信聊天记录\"@zh ;\n    rdfs:subClassOf :Supplier .\n\n:logTime a owl:DatatypeProperty ;\n    rdfs:domain :ChatRecord ;\n    rdfs:range xsd:string ;\n    rdfs:label \"记录时间\"@zh .\n\n:mentions a owl:ObjectProperty ;\n    rdfs:domain :Supplier ;\n    rdfs:range :ChatRecord ;\n    rdfs:label \"提及\"@zh .\n",
            sample_ttl()
        );
        store.import_ttl(&with_pkg, true).unwrap();

        // 单条撤类应因引用失败；级联应成功
        let single = store.revoke_workspace_diff_item(
            VIRI,
            &crate::ttl::WorkspaceDiffRevokeItem::Class {
                op: "add".into(),
                iri: chat.into(),
            },
        );
        assert!(single.is_err(), "仍被属性引用时单条撤类应失败");

        store
            .revoke_workspace_class_cascade(VIRI, chat, "add")
            .unwrap();

        let props = store.list_ttl_properties(VIRI).unwrap();
        assert!(props.iter().all(|p| !p.iri.ends_with("logTime")));
        assert!(props.iter().all(|p| !p.iri.ends_with("mentions")));
        let metas = store.list_ttl_class_metas(VIRI).unwrap();
        assert!(metas.iter().all(|c| !c.iri.ends_with("ChatRecord")));
        assert!(
            !store.get_workspace_dirty(VIRI).unwrap(),
            "级联撤净后应清 dirty"
        );

        // 仅类无构件仍成功
        let only_class = format!(
            "{}\n:OnlyClass a owl:Class ;\n    rdfs:label \"仅类\"@zh .\n",
            sample_ttl()
        );
        store.import_ttl(&only_class, true).unwrap();
        store
            .revoke_workspace_class_cascade(
                VIRI,
                "https://example.org/ontology/test#OnlyClass",
                "add",
            )
            .unwrap();
        assert!(
            store
                .list_ttl_class_metas(VIRI)
                .unwrap()
                .iter()
                .all(|c| !c.iri.ends_with("OnlyClass"))
        );

        // 失败路径：类不在 diff → 整笔不写
        let before = store.export_ttl(VIRI).unwrap();
        let err = store.revoke_workspace_class_cascade(VIRI, chat, "add");
        assert!(err.is_err());
        assert_eq!(store.export_ttl(VIRI).unwrap(), before);
    }

    #[test]
    fn revoke_workspace_class_cascade_restores_removed_class_package() {
        let store = TtlStore::open_in_memory().unwrap();
        let with_lead = format!(
            "{}\n:LeadSource a owl:Class ;\n    rdfs:label \"线索来源\"@zh .\n\n:leadSourceName a owl:DatatypeProperty ;\n    rdfs:domain :LeadSource ;\n    rdfs:range xsd:string ;\n    rdfs:label \"来源名\"@zh .\n\n:sharedTag a owl:DatatypeProperty ;\n    rdfs:domain :LeadSource ;\n    rdfs:range xsd:string ;\n    rdfs:label \"共享标签\"@zh .\n",
            sample_ttl()
        );
        store.import_ttl(&with_lead, true).unwrap();
        store
            .publish_version(VIRI, "v1", "含线索来源", "user", None)
            .unwrap();

        // 删除类包：属性先删、sharedTag 定义域改到 Supplier、再删类
        store
            .delete_ttl_property(
                VIRI,
                "https://example.org/ontology/test#leadSourceName",
            )
            .unwrap();
        let mut shared = store
            .list_ttl_properties(VIRI)
            .unwrap()
            .into_iter()
            .find(|p| p.iri.ends_with("sharedTag"))
            .expect("sharedTag");
        shared.domain = "https://example.org/ontology/test#Supplier".into();
        store.edit_ttl_property(VIRI, &shared).unwrap();
        store
            .delete_ttl_class(VIRI, "https://example.org/ontology/test#LeadSource")
            .unwrap();

        let lead = "https://example.org/ontology/test#LeadSource";
        store
            .revoke_workspace_class_cascade(VIRI, lead, "remove")
            .unwrap();

        let metas = store.list_ttl_class_metas(VIRI).unwrap();
        assert!(
            metas.iter().any(|c| c.iri.ends_with("LeadSource")),
            "类应恢复"
        );
        let props = store.list_ttl_properties(VIRI).unwrap();
        assert!(
            props.iter().any(|p| p.iri.ends_with("leadSourceName")),
            "已删属性应恢复"
        );
        let shared_after = props
            .into_iter()
            .find(|p| p.iri.ends_with("sharedTag"))
            .expect("sharedTag");
        assert!(
            shared_after.domain.ends_with("LeadSource"),
            "定义域连带应回滚到 LeadSource，实际={}",
            shared_after.domain
        );
        assert!(!store.get_workspace_dirty(VIRI).unwrap());
    }

    #[test]
    fn revoke_workspace_class_cascade_restores_range_side_relation() {
        let store = TtlStore::open_in_memory().unwrap();
        let ttl = format!(
            "{}\n:LeadSource a owl:Class ;\n    rdfs:label \"线索来源\"@zh .\n\n:Advisor a owl:Class ;\n    rdfs:label \"销售顾问\"@zh .\n\n:sourceChannel a owl:ObjectProperty ;\n    rdfs:domain :Advisor ;\n    rdfs:range :LeadSource ;\n    rdfs:label \"来源渠道\"@zh .\n",
            sample_ttl()
        );
        store.import_ttl(&ttl, true).unwrap();
        store
            .publish_version(VIRI, "v1", "含值域关系", "user", None)
            .unwrap();

        store
            .delete_ttl_property(
                VIRI,
                "https://example.org/ontology/test#sourceChannel",
            )
            .unwrap();
        store
            .delete_ttl_class(VIRI, "https://example.org/ontology/test#LeadSource")
            .unwrap();

        store
            .revoke_workspace_class_cascade(
                VIRI,
                "https://example.org/ontology/test#LeadSource",
                "remove",
            )
            .unwrap();

        assert!(
            store
                .list_ttl_properties(VIRI)
                .unwrap()
                .iter()
                .any(|p| p.iri.ends_with("sourceChannel")),
            "值域落在已删类的关系应随类级联恢复"
        );
        assert!(!store.get_workspace_dirty(VIRI).unwrap());
    }

    /// 卡片硬删类（级联）：真 DB 落库 + 相对生效差异 + 失败不写。
    #[test]
    fn delete_ttl_class_cascades_and_reflects_in_workspace_diff() {
        let store = TtlStore::open_in_memory().unwrap();
        let ttl = format!(
            "{}\n:Route a owl:Class ;\n    rdfs:label \"线路\"@zh .\n\n:Store a owl:Class ;\n    rdfs:label \"门店\"@zh .\n\n:name a owl:DatatypeProperty ;\n    rdfs:domain :Route ;\n    rdfs:range xsd:string ;\n    rdfs:label \"线路名\"@zh .\n\n:belongsTo a owl:ObjectProperty ;\n    rdfs:domain :Route ;\n    rdfs:range :Store ;\n    rdfs:label \"所属门店\"@zh .\n\n:note a owl:DatatypeProperty ;\n    rdfs:domain :Route ;\n    rdfs:domain :Store ;\n    rdfs:range xsd:string ;\n    rdfs:label \"备注\"@zh .\n\n:Express a owl:Class ;\n    rdfs:label \"快线\"@zh ;\n    rdfs:subClassOf :Route .\n",
            sample_ttl()
        );
        store.import_ttl(&ttl, false).unwrap();
        let active = store.publish_version(VIRI, "v1", "含线路", "user", None).unwrap();
        let route = "https://example.org/ontology/test#Route";

        store.delete_ttl_class(VIRI, route).unwrap();

        // 1. 独占属性 / 关系 / 类本身已落库删除
        let props = store.list_ttl_properties(VIRI).unwrap();
        assert!(props.iter().all(|p| !p.iri.ends_with("#name")));
        assert!(props.iter().all(|p| !p.iri.ends_with("#belongsTo")));
        let metas = store.list_ttl_class_metas(VIRI).unwrap();
        assert!(metas.iter().all(|c| !c.iri.ends_with("#Route")));
        // 2. 共享多定义域属性保留，定义域只剩 Store
        let note = props
            .iter()
            .find(|p| p.iri.ends_with("#note"))
            .expect("共享属性应保留");
        assert!(note.domain.ends_with("#Store"), "domain={}", note.domain);
        // 3. 子类块仍合法可读（label 保留、父类只剩非 Route）
        let express = metas
            .iter()
            .find(|c| c.iri.ends_with("#Express"))
            .expect("子类应保留");
        assert_eq!(express.label, "快线");

        // 4. 相对生效差异反映删除
        let diff = store.diff_versions(VIRI, active as i64, 0).unwrap();
        assert!(
            diff.removed_classes.iter().any(|c| c.ends_with("#Route")),
            "diff 应记录类删除: {:?}",
            diff.removed_classes
        );
        assert!(
            diff.removed_properties.iter().any(|p| p.iri.ends_with("#name")),
            "diff 应记录独占属性删除: {:?}",
            diff.removed_properties
        );
        assert!(
            diff.removed_properties
                .iter()
                .any(|p| p.iri.ends_with("#belongsTo")),
            "diff 应记录关系删除"
        );
        assert!(
            diff.modified_properties.iter().any(|p| p.after.iri.ends_with("#note")),
            "共享属性定义域变化应记入 modified: {:?}",
            diff.modified_properties
        );
        assert!(
            diff.removed_subclass_of
                .iter()
                .any(|e| e.child.ends_with("#Express") && e.parent.ends_with("#Route")),
            "diff 应记录继承边解除"
        );
    }

    /// 非法路径：还有实例引用时整笔拒绝、工作区不变。
    #[test]
    fn delete_ttl_class_cascade_rejects_and_keeps_workspace_on_instance_refs() {
        let store = TtlStore::open_in_memory().unwrap();
        let ttl = format!(
            "{}\n:Route a owl:Class ;\n    rdfs:label \"线路\"@zh .\n\n:route1 a :Route .\n",
            sample_ttl()
        );
        store.import_ttl(&ttl, true).unwrap();
        let before = store.export_ttl(VIRI).unwrap();

        let err = store.delete_ttl_class(VIRI, "https://example.org/ontology/test#Route");
        assert!(err.is_err(), "实例引用时应拒绝");
        assert_eq!(store.export_ttl(VIRI).unwrap(), before, "失败不得部分写入");
    }

    // ════════════════════════════════════════════════════════════════
    // 类级联恢复：对端存在性门禁（恢复后不得产生指向不存在类的公理）
    // ════════════════════════════════════════════════════════════════

    /// 线索(Lead) + 来源(LeadSource) + 独占数据属性 channel(Lead→xsd:string)
    /// + 关系 sourcedFrom(Lead→LeadSource) + 自环关系 parentSource(LeadSource→LeadSource)
    /// + 子类 LeadTag ⊑ Lead。
    fn lead_pair_ttl() -> String {
        format!(
            "{}\n:Lead a owl:Class ;\n    rdfs:label \"线索\"@zh .\n\n:LeadSource a owl:Class ;\n    rdfs:label \"来源\"@zh .\n\n:channel a owl:DatatypeProperty ;\n    rdfs:domain :Lead ;\n    rdfs:range xsd:string ;\n    rdfs:label \"线索渠道\"@zh .\n\n:sourcedFrom a owl:ObjectProperty ;\n    rdfs:domain :Lead ;\n    rdfs:range :LeadSource ;\n    rdfs:label \"来源渠道\"@zh .\n\n:parentSource a owl:ObjectProperty ;\n    rdfs:domain :LeadSource ;\n    rdfs:range :LeadSource ;\n    rdfs:label \"上级来源\"@zh .\n\n:LeadTag a owl:Class ;\n    rdfs:label \"线索标签\"@zh ;\n    rdfs:subClassOf :Lead .\n",
            sample_ttl()
        )
    }

    fn has_prop_named(store: &TtlStore, suffix: &str) -> bool {
        store
            .list_ttl_properties(VIRI)
            .unwrap()
            .iter()
            .any(|p| p.iri.ends_with(suffix))
    }

    fn has_class_named(store: &TtlStore, suffix: &str) -> bool {
        store
            .list_ttl_class_metas(VIRI)
            .unwrap()
            .iter()
            .any(|c| c.iri.ends_with(suffix))
    }

    fn subclass_edges(store: &TtlStore) -> Vec<(String, String)> {
        crate::ttl_edit::list_subclass_of_edges(&store.export_ttl(VIRI).unwrap())
    }

    /// 对端类仍为删除态的关系不随本类恢复；类自身与独占数据属性正常恢复。
    #[test]
    fn class_cascade_restore_skips_relation_while_other_end_deleted() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(&lead_pair_ttl(), true).unwrap();
        store
            .publish_version(VIRI, "v1", "含线索与来源", "user", None)
            .unwrap();

        let lead = "https://example.org/ontology/test#Lead";
        let lead_source = "https://example.org/ontology/test#LeadSource";
        store.delete_ttl_class(VIRI, lead).unwrap();
        store.delete_ttl_class(VIRI, lead_source).unwrap();
        assert!(!has_class_named(&store, "Lead"));
        assert!(!has_prop_named(&store, "sourcedFrom"));

        store
            .revoke_workspace_class_cascade(VIRI, lead, "remove")
            .unwrap();

        assert!(has_class_named(&store, "Lead"), "类应恢复");
        assert!(has_prop_named(&store, "channel"), "独占数据属性应恢复");
        assert!(
            !has_prop_named(&store, "sourcedFrom"),
            "对端 LeadSource 仍删除，关系不得恢复（否则落库悬空公理）"
        );
        assert!(
            !has_class_named(&store, "LeadSource"),
            "不得连带恢复对端类"
        );
        assert!(
            subclass_edges(&store)
                .iter()
                .any(|(c, p)| c.ends_with("LeadTag") && p.ends_with("Lead")),
            "对端（父类）正在恢复，继承边应随级联带回"
        );
    }

    /// 随后恢复对端类时，被跳过的关系随该类的级联自动带回（顺序无关性·路径 A）。
    #[test]
    fn class_cascade_restore_brings_back_relation_after_other_end_restored() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(&lead_pair_ttl(), true).unwrap();
        store
            .publish_version(VIRI, "v1", "含线索与来源", "user", None)
            .unwrap();

        let lead = "https://example.org/ontology/test#Lead";
        let lead_source = "https://example.org/ontology/test#LeadSource";
        store.delete_ttl_class(VIRI, lead).unwrap();
        store.delete_ttl_class(VIRI, lead_source).unwrap();

        store
            .revoke_workspace_class_cascade(VIRI, lead, "remove")
            .unwrap();
        store
            .revoke_workspace_class_cascade(VIRI, lead_source, "remove")
            .unwrap();

        assert!(has_prop_named(&store, "sourcedFrom"), "两端齐备后关系应带回");
        assert!(
            has_prop_named(&store, "parentSource"),
            "自环关系 domain=range=C 应随 C 恢复"
        );
    }

    /// 先恢复 LeadSource 再恢复 Lead，终态一致（顺序无关性·路径 B）。
    #[test]
    fn class_cascade_restore_is_order_independent() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(&lead_pair_ttl(), true).unwrap();
        store
            .publish_version(VIRI, "v1", "含线索与来源", "user", None)
            .unwrap();

        let lead = "https://example.org/ontology/test#Lead";
        let lead_source = "https://example.org/ontology/test#LeadSource";
        store.delete_ttl_class(VIRI, lead).unwrap();
        store.delete_ttl_class(VIRI, lead_source).unwrap();

        store
            .revoke_workspace_class_cascade(VIRI, lead_source, "remove")
            .unwrap();
        assert!(
            !has_prop_named(&store, "sourcedFrom"),
            "反向：Lead 仍删除时关系不得恢复"
        );
        store
            .revoke_workspace_class_cascade(VIRI, lead, "remove")
            .unwrap();

        assert!(has_prop_named(&store, "sourcedFrom"), "终态应与路径 A 一致");
        assert!(has_prop_named(&store, "parentSource"));
        assert!(has_prop_named(&store, "channel"));
        assert!(has_class_named(&store, "Lead"));
        assert!(has_class_named(&store, "LeadSource"));
    }

    /// 继承边的另一端类仍删除时，该边不随本类恢复。
    #[test]
    fn class_cascade_restore_skips_subclass_edge_while_parent_deleted() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(&lead_pair_ttl(), true).unwrap();
        store
            .publish_version(VIRI, "v1", "含子类", "user", None)
            .unwrap();

        let lead = "https://example.org/ontology/test#Lead";
        let lead_tag = "https://example.org/ontology/test#LeadTag";
        store.delete_ttl_class(VIRI, lead_tag).unwrap();
        store.delete_ttl_class(VIRI, lead).unwrap();

        store
            .revoke_workspace_class_cascade(VIRI, lead_tag, "remove")
            .unwrap();

        assert!(has_class_named(&store, "LeadTag"), "子类本身应恢复");
        assert!(
            !subclass_edges(&store)
                .iter()
                .any(|(c, p)| c.ends_with("LeadTag") && p.ends_with("Lead")),
            "父类 Lead 仍删除，继承边不得恢复"
        );
    }

    /// 级联计划（纯函数）：~ 连带回滚按「回滚后的目标定义」判端点存在性。
    #[test]
    fn build_class_cascade_steps_skips_modify_rollback_with_missing_end() {
        let ns = "https://example.org/ontology/test#";
        let mk = |iri: &str, domain: &str, range: &str| crate::ttl_edit::TtlPropertyEdit {
            iri: iri.into(),
            kind: "object".into(),
            domain: domain.into(),
            range: range.into(),
            ..Default::default()
        };
        // 生效版 Gamma → Beta；工作区因 Gamma 被删而把定义域改到 Alpha（值域 Beta 也已删除）
        let diff = TtlVersionDiff {
            rev_a: 1,
            rev_b: 0,
            added_classes: vec![],
            removed_classes: vec![format!("{ns}Gamma"), format!("{ns}Beta")],
            modified_classes: vec![],
            added_properties: vec![],
            removed_properties: vec![],
            modified_properties: vec![PropertyChange {
                iri: format!("{ns}rel"),
                before: mk(&format!("{ns}rel"), &format!("{ns}Gamma"), &format!("{ns}Beta")),
                after: mk(&format!("{ns}rel"), &format!("{ns}Alpha"), &format!("{ns}Beta")),
            }],
            added_subclass_of: vec![],
            removed_subclass_of: vec![],
            ontology_meta_changed: false,
        };
        let base = sample_ttl();
        let effective = format!(
            "{base}\n:Alpha a owl:Class ;\n    rdfs:label \"甲\"@zh .\n\n:Beta a owl:Class ;\n    rdfs:label \"乙\"@zh .\n\n:Gamma a owl:Class ;\n    rdfs:label \"丙\"@zh .\n"
        );
        let without_beta =
            format!("{base}\n:Alpha a owl:Class ;\n    rdfs:label \"甲\"@zh .\n");
        let with_beta = format!(
            "{base}\n:Alpha a owl:Class ;\n    rdfs:label \"甲\"@zh .\n\n:Beta a owl:Class ;\n    rdfs:label \"乙\"@zh .\n"
        );

        // 恢复 Gamma：回滚后的定义指向仍不存在的 Beta → 该 ~ 不进级联
        let plan = crate::ttl::crud::build_class_cascade_steps(
            &diff,
            &format!("{ns}Gamma"),
            "remove",
            &without_beta,
            &effective,
        );
        assert_eq!(plan.skipped.len(), 1, "应跳过 1 项连带回滚");
        assert!(
            !plan
                .steps
                .iter()
                .any(|s| matches!(s, WorkspaceDiffRevokeItem::Property { op, .. } if op == "modify")),
            "跳过项不得进入步骤"
        );

        // 同一条 ~，值域 Beta 已在工作区 → 正常回滚
        let plan_ok = crate::ttl::crud::build_class_cascade_steps(
            &diff,
            &format!("{ns}Gamma"),
            "remove",
            &with_beta,
            &effective,
        );
        assert!(plan_ok.skipped.is_empty());
        assert!(plan_ok
            .steps
            .iter()
            .any(|s| matches!(s, WorkspaceDiffRevokeItem::Property { op, .. } if op == "modify")));
    }

    /// 级联计划（纯函数）：自环关系 domain=range=C 随 C 恢复。
    #[test]
    fn build_class_cascade_steps_restores_self_loop() {
        let ns = "https://example.org/ontology/test#";
        let diff = TtlVersionDiff {
            rev_a: 1,
            rev_b: 0,
            added_classes: vec![],
            removed_classes: vec![format!("{ns}Beta")],
            modified_classes: vec![],
            added_properties: vec![],
            removed_properties: vec![crate::ttl_edit::TtlPropertyEdit {
                iri: format!("{ns}selfRel"),
                kind: "object".into(),
                domain: format!("{ns}Beta"),
                range: format!("{ns}Beta"),
                ..Default::default()
            }],
            modified_properties: vec![],
            added_subclass_of: vec![],
            removed_subclass_of: vec![],
            ontology_meta_changed: false,
        };
        let workspace = sample_ttl();
        let effective = format!("{workspace}\n:Beta a owl:Class ;\n    rdfs:label \"乙\"@zh .\n");

        let plan = crate::ttl::crud::build_class_cascade_steps(
            &diff,
            &format!("{ns}Beta"),
            "remove",
            &workspace,
            &effective,
        );
        assert!(plan.skipped.is_empty(), "自环两端同为正在恢复的 C → 不跳过");
        assert!(plan
            .steps
            .iter()
            .any(|s| matches!(s, WorkspaceDiffRevokeItem::Property { op, iri } if op == "remove" && iri.ends_with("selfRel"))));
    }
