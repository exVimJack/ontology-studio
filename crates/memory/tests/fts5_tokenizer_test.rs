//! FTS5 jieba 中文分词验证：注册、建表、检索、BM25 排序、snippet 高亮、持久化。
//!
//! 运行：cargo test -p memory --test fts5_tokenizer_test -- --nocapture

use rusqlite::Connection;
use memory::jieba_tokenizer::JiebaSentenceTokenizer;
use rusqlite_ext::register_tokenizer;

/// 裸连接注册分句版 jieba tokenizer（验证 per-connection 注册机制）。
fn register_jieba(conn: &Connection) {
    register_tokenizer::<JiebaSentenceTokenizer>(conn, ()).expect("register jieba tokenizer");
}

#[test]
fn jieba_chinese_search() {
    let conn = Connection::open_in_memory().unwrap();
    register_jieba(&conn);
    conn.execute_batch(
        "CREATE VIRTUAL TABLE docs USING fts5(name, text, tokenize='jieba');",
    )
    .unwrap();

    let docs = [
        ("向量数据库选型指南", "本文对比了 Qdrant、Milvus、sqlite-vec 等向量库的优劣"),
        ("知识图谱构建实践", "从非结构化文本抽取实体关系，构建企业知识图谱"),
        ("Rust 异步编程", "tokio 运行时与 async/await 的最佳实践"),
    ];
    for (name, text) in &docs {
        conn.execute(
            "INSERT INTO docs(name, text) VALUES(?, ?)",
            rusqlite::params![name, text],
        )
        .unwrap();
    }

    // 词语分词：搜索"向量库"应精准命中"向量数据库选型指南"
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT name, text FROM docs WHERE docs MATCH '向量库' ORDER BY rank")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    println!("[jieba] 搜索'向量库' → {:?}", rows);
    assert!(!rows.is_empty());
    assert!(rows[0].0.contains("向量数据库") || rows[0].1.contains("向量库"));

    // 多词组合搜索
    let combo: Vec<String> = conn
        .prepare("SELECT name FROM docs WHERE docs MATCH '知识 图谱' ORDER BY rank")
        .unwrap()
        .query_map([], |r: &rusqlite::Row| r.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    println!("[jieba] 搜索'知识 图谱' → {:?}", combo);
    assert!(combo.iter().any(|n| n.contains("知识图谱")));
}

#[test]
fn bm25_snippet_highlight() {
    let conn = Connection::open_in_memory().unwrap();
    register_jieba(&conn);
    conn.execute_batch(
        "CREATE VIRTUAL TABLE docs USING fts5(name, text, tokenize='jieba');",
    )
    .unwrap();

    let docs = [
        ("doc1", "向量数据库是现代 RAG 系统的核心组件"),
        ("doc2", "向量数据库选型需要考虑维度、距离度量、索引算法"),
        ("doc3", "SQLite 是轻量级嵌入式数据库"),
    ];
    for (name, text) in &docs {
        conn.execute(
            "INSERT INTO docs(name, text) VALUES(?, ?)",
            rusqlite::params![name, text],
        )
        .unwrap();
    }

    // BM25 排序：更切题的排前
    let ranked: Vec<(String, f64)> = conn
        .prepare("SELECT name, bm25(docs) FROM docs WHERE docs MATCH '向量数据库 选型' ORDER BY rank")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    println!("[jieba] BM25 排序: {:?}", ranked);
    assert!(!ranked.is_empty());

    // snippet 高亮
    let snippets: Vec<(String, String)> = conn
        .prepare("SELECT name, snippet(docs, 1, '<b>', '</b>', '...', 10) FROM docs WHERE docs MATCH '向量数据库'")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    println!("[jieba] snippet: {:?}", snippets);
    assert!(snippets.iter().any(|(_, s)| s.contains("<b>")));
}

#[test]
fn fts5_persists_across_connections() {
    /// FTS5 表持久化在 .db 文件，tokenizer 需 per-connection 注册。
    let tmp = std::env::temp_dir().join("onto_fts5_persist_test.db");
    let _ = std::fs::remove_file(&tmp);

    {
        let conn = Connection::open(&tmp).unwrap();
        register_jieba(&conn);
        conn.execute_batch("CREATE VIRTUAL TABLE docs USING fts5(name, text, tokenize='jieba');")
            .unwrap();
        conn.execute(
            "INSERT INTO docs(name, text) VALUES(?, ?)",
            rusqlite::params!["测试文档", "这是持久化测试的内容，关于向量数据库"],
        )
        .unwrap();
    }

    // 重开连接（模拟重启）：必须重新注册 tokenizer
    {
        let conn = Connection::open(&tmp).unwrap();
        register_jieba(&conn);
        let rows: Vec<String> = conn
            .prepare("SELECT name FROM docs WHERE docs MATCH '向量数据库'")
            .unwrap()
            .query_map([], |r: &rusqlite::Row| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        println!("[persist] 重开后查询 → {:?}", rows);
        assert_eq!(rows, vec!["测试文档".to_string()]);
    }

    let _ = std::fs::remove_file(&tmp);
}
