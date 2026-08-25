use ontology_store::ttl::TtlStore;
fn store() -> (TtlStore, String) {
    let s = TtlStore::open_in_memory().unwrap();
    let ttl = std::fs::read_to_string("/Users/thinkpiggy/Downloads/DealerSalesOntology.ttl").unwrap();
    s.import_ttl(&ttl, true).unwrap();
    let iri = s.list_ontologies().unwrap()[0].ontology_iri.clone();
    (s, iri)
}

#[test]
fn all_panels_label_queries() {
    let (s, iri) = store();

    // ── HierarchyPanel：cls + sup + 各自 label（嵌套 OPTIONAL，方案 D）──
    let q = "SELECT ?cls ?sup ?clslabel ?suplabel WHERE { \
        ?cls a owl:Class . FILTER(isIRI(?cls)) \
        OPTIONAL { ?cls rdfs:label ?clslabel . FILTER(LANGMATCHES(LANG(?clslabel), \"zh\")) } \
        OPTIONAL { ?cls rdfs:subClassOf ?sup . FILTER(isIRI(?sup)) \
            OPTIONAL { ?sup rdfs:label ?suplabel . FILTER(LANGMATCHES(LANG(?suplabel), \"zh\")) } } }";
    let r = s.query_sparql(&iri, q).unwrap();
    let n = r.matches("\"cls\"").count();
    println!("Hierarchy 嵌套: {} 行, {} bytes", n, r.len());
    assert!(n <= 60, "Hierarchy 行数异常: {}", n);
    assert!(r.len() < 20_000);

    // ── GraphPanel：class + subClassOf + ObjectProperty domain→range + 3 个 label（嵌套）──
    let q = "SELECT ?s ?p ?o ?type ?slabel ?olabel ?plabel WHERE { \
        { ?s a owl:Class . BIND(\"class\" AS ?type) \
          OPTIONAL { ?s rdfs:label ?slabel . FILTER(LANGMATCHES(LANG(?slabel), \"zh\")) } } \
        UNION \
        { ?s rdfs:subClassOf ?o . BIND(\"subClassOf\" AS ?p) BIND(\"hierarchy\" AS ?type) \
          OPTIONAL { ?s rdfs:label ?slabel . FILTER(LANGMATCHES(LANG(?slabel), \"zh\")) } \
          OPTIONAL { ?o rdfs:label ?olabel . FILTER(LANGMATCHES(LANG(?olabel), \"zh\")) } } \
        UNION \
        { ?p a owl:ObjectProperty . ?p rdfs:domain ?s . ?p rdfs:range ?o . BIND(\"objectProperty\" AS ?type) \
          OPTIONAL { ?s rdfs:label ?slabel . FILTER(LANGMATCHES(LANG(?slabel), \"zh\")) } \
          OPTIONAL { ?o rdfs:label ?olabel . FILTER(LANGMATCHES(LANG(?olabel), \"zh\")) } \
          OPTIONAL { ?p rdfs:label ?plabel . FILTER(LANGMATCHES(LANG(?plabel), \"zh\")) } } \
        FILTER(isIRI(?s)) FILTER(!BOUND(?o) || isIRI(?o)) }";
    let r = s.query_sparql(&iri, q).unwrap();
    let n = r.matches("\"s\"").count();
    println!("Graph 嵌套: {} 行, {} bytes", n, r.len());
    // GraphPanel 有 subClassOf + ObjectProperty，行数会比 class 多
    assert!(n < 500, "Graph 行数异常: {}", n);

    // ── ClassDetailPanel superQ：指定类的父类 + label（嵌套）──
    // 注意 classLabel 是 IRI 短名，用 CONTAINS 匹配
    let q = "SELECT ?sup ?suplabel WHERE { \
        ?cls rdfs:subClassOf ?sup . FILTER(isIRI(?sup)) FILTER(CONTAINS(STR(?cls), \"CustomerProfile\")) \
        OPTIONAL { ?sup rdfs:label ?suplabel . FILTER(LANGMATCHES(LANG(?suplabel), \"zh\")) } }";
    let r = s.query_sparql(&iri, q).unwrap();
    println!("ClassDetail superQ: {} 行, {} bytes", r.matches("\"sup\"").count(), r.len());
}
