use ontology_store::ttl::TtlStore;
#[test]
fn one_query_for_all_labels() {
    let s = TtlStore::open_in_memory().unwrap();
    // 样例文件是作者本机个人路径，不在其他环境存在——直接跳过（非回归失败）
    let Ok(ttl) = std::fs::read_to_string("/Users/thinkpiggy/Downloads/DealerSalesOntology.ttl")
    else {
        eprintln!("skip: DealerSalesOntology.ttl 不在本机（环境依赖样例）");
        return;
    };
    s.import_ttl(&ttl, true).unwrap();
    let iri = s.list_ontologies().unwrap()[0].ontology_iri.clone();

    // 一个查询：所有 IRI 的中文 label，返回 iri→label 映射
    let q = "SELECT ?s ?label WHERE { ?s rdfs:label ?label . FILTER(LANGMATCHES(LANG(?label), \"zh\")) }";
    let r = s.query_sparql(&iri, q).unwrap();
    let n = r.matches("\"label\"").count();
    println!("全量 label 查询: {} 行, {} bytes", n, r.len());
    assert!(r.len() < 100_000, "label 映射太大: {} bytes", r.len());
}
