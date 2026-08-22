//! documents 表 + FTS5 检索集成测试。
//!
//! 运行：cargo test -p memory --test documents_test -- --nocapture

use memory::{canonicalize_path, new_document_id, now_ms, DocumentRow, Memory};

fn doc(path: &str, name: &str, format: &str, text: &str) -> DocumentRow {
    DocumentRow {
        id: new_document_id(),
        path: canonicalize_path(path),
        name: name.into(),
        format: format.into(),
        text: text.into(),
        char_count: text.chars().count() as u32,
        created_at: now_ms(),
        folder_path: None,
        source_conv_id: None,
    }
}

#[test]
fn upsert_search_read_list_delete() {
    let mem = Memory::open_in_memory().unwrap();

    // 插入 3 篇文档（upsert 只写主行，需显式建 FTS5 索引才能搜到）。
    let id1 = mem.upsert_document(doc(
        "/tmp/doc1.pdf",
        "向量数据库选型指南.pdf",
        "pdf",
        "本文对比了 Qdrant、Milvus、sqlite-vec 等向量数据库的优劣，讨论索引算法与距离度量。",
    )).unwrap();
    mem.index_document(&id1).unwrap();
    let _id2 = mem.upsert_document(doc(
        "/tmp/doc2.docx",
        "知识图谱构建实践.docx",
        "docx",
        "从非结构化文本抽取实体关系，构建企业知识图谱，应用于智能问答与推荐系统。",
    )).unwrap();
    mem.index_document(&_id2).unwrap();
    let _id3 = mem.upsert_document(doc(
        "/tmp/doc3.txt",
        "Rust 异步编程笔记.txt",
        "txt",
        "tokio 运行时与 async/await 的最佳实践，包括任务调度与取消机制。",
    )).unwrap();
    mem.index_document(&_id3).unwrap();

    // 1. search：搜"向量数据库"应命中 doc1
    let hits = mem.search_documents("向量数据库", 10).unwrap();
    println!("[search] '向量数据库' → {} hits", hits.len());
    for h in &hits {
        println!("  - {} | rank={:.3} | {}", h.name, h.rank, h.snippet);
    }
    assert_eq!(hits.len(), 1);
    assert!(hits[0].name.contains("向量数据库"));
    assert!(hits[0].snippet.contains("【向量数据库】") || hits[0].snippet.contains("向量数据库"));

    // 2. search：搜"知识图谱"应命中 doc2
    let hits = mem.search_documents("知识图谱", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].name.contains("知识图谱"));

    // 3. search：多词组合"实体 关系"
    let hits = mem.search_documents("实体 关系", 10).unwrap();
    println!("[search] '实体 关系' → {} hits", hits.len());
    assert!(!hits.is_empty());

    // 4. read：按 id 读全文
    let (_path, name, format, text, cc) = mem.read_document(&id1, None, None).unwrap().unwrap();
    println!("[read] {} ({} chars)", name, cc);
    assert_eq!(name, "向量数据库选型指南.pdf");
    assert_eq!(format, "pdf");
    assert!(text.contains("Qdrant"));

    // 5. read：分页（offset 10, limit 5）
    let (_, _, _, sliced, _) = mem.read_document(&id1, Some(10), Some(5)).unwrap().unwrap();
    println!("[read paged] '{}'", sliced);
    assert_eq!(sliced.chars().count(), 5);

    // 6. list：3 篇
    let all = mem.list_documents().unwrap();
    assert_eq!(all.len(), 3);

    // 7. upsert 去重：同 path 再插，更新而非新增（id 保持稳定，内容更新）
    mem.upsert_document(doc(
        "/tmp/doc1.pdf",
        "向量数据库选型指南(第二版).pdf",
        "pdf",
        "更新后的内容：新增 Weaviate 对比。",
    )).unwrap();
    // upsert 重置 indexed_at=0，需重建索引（index_document 会先删旧索引再建新）。
    mem.index_document(&id1).unwrap();
    let all = mem.list_documents().unwrap();
    assert_eq!(all.len(), 3, "同 path upsert 应替换不新增");
    // 旧 id 应仍可读（ON CONFLICT 保留原行，更新内容）
    let (_, name, _, text, _) = mem.read_document(&id1, None, None).unwrap().unwrap();
    assert_eq!(name, "向量数据库选型指南(第二版).pdf");
    assert!(text.contains("Weaviate"));
    // FTS5 索引应已重建（旧内容搜不到，新内容搜得到）
    let hits = mem.search_documents("Qdrant", 10).unwrap();
    assert!(hits.is_empty(), "旧内容 Qdrant 已被更新移除");
    let hits = mem.search_documents("Weaviate", 10).unwrap();
    assert_eq!(hits.len(), 1);

    // 8. delete
    let n = mem.delete_document_by_path(&canonicalize_path("/tmp/doc2.docx")).unwrap();
    assert_eq!(n, 1);
    let all = mem.list_documents().unwrap();
    assert_eq!(all.len(), 2);
    // FTS5 索引应同步清除
    let hits = mem.search_documents("知识图谱", 10).unwrap();
    assert!(hits.is_empty(), "删除后 FTS5 应搜不到");
}

#[test]
fn empty_query_returns_empty() {
    let mem = Memory::open_in_memory().unwrap();
    mem.upsert_document(doc("/tmp/x.txt", "x.txt", "txt", "测试内容")).unwrap();
    assert!(mem.search_documents("", 10).unwrap().is_empty());
    assert!(mem.search_documents("   ", 10).unwrap().is_empty());
}

#[test]
fn nonexistent_id_returns_none() {
    let mem = Memory::open_in_memory().unwrap();
    let r = mem.read_document("nonexistent-uuid", None, None).unwrap();
    assert!(r.is_none());
}

#[test]
fn mount_list_unmount() {
    let mem = Memory::open_in_memory().unwrap();
    let path = canonicalize_path("/tmp/a.pdf");
    mem.upsert_document(doc("/tmp/a.pdf", "a.pdf", "pdf", "文档 A 全文")).unwrap();
    mem.upsert_document(doc("/tmp/b.txt", "b.txt", "txt", "文档 B 全文")).unwrap();
    let path_b = canonicalize_path("/tmp/b.txt");

    // 挂载 a 到会话 1
    mem.mount_document("conv1", &path).unwrap();
    // 幂等：重复挂载不报错不重复
    mem.mount_document("conv1", &path).unwrap();
    // 挂载 b 到会话 1
    mem.mount_document("conv1", &path_b).unwrap();

    let mounted = mem.list_mounted_documents("conv1").unwrap();
    assert_eq!(mounted.len(), 2);
    assert_eq!(mounted[0].name, "a.pdf");
    assert_eq!(mounted[1].name, "b.txt");

    // 会话 2 独立（挂载隔离）
    let mounted2 = mem.list_mounted_documents("conv2").unwrap();
    assert!(mounted2.is_empty());

    // 卸载 a
    let n = mem.unmount_document("conv1", &path).unwrap();
    assert_eq!(n, 1);
    let mounted = mem.list_mounted_documents("conv1").unwrap();
    assert_eq!(mounted.len(), 1);
    assert_eq!(mounted[0].name, "b.txt");
}

#[test]
fn mounted_doc_skipped_after_delete() {
    let mem = Memory::open_in_memory().unwrap();
    let path = canonicalize_path("/tmp/c.md");
    mem.upsert_document(doc("/tmp/c.md", "c.md", "md", "内容")).unwrap();
    mem.mount_document("conv1", &path).unwrap();
    assert_eq!(mem.list_mounted_documents("conv1").unwrap().len(), 1);

    // 删文档全文（挂载关联仍在，但 list 时 LEFT JOIN 取不到应跳过）
    mem.delete_document_by_path(&path).unwrap();
    let mounted = mem.list_mounted_documents("conv1").unwrap();
    assert_eq!(mounted.len(), 0, "已删文档不应出现在挂载列表");
}

#[test]
fn clear_mounted_by_path_removes_all_convs() {
    let mem = Memory::open_in_memory().unwrap();
    let path = canonicalize_path("/tmp/d.txt");
    mem.upsert_document(doc("/tmp/d.txt", "d.txt", "txt", "x")).unwrap();
    mem.mount_document("conv1", &path).unwrap();
    mem.mount_document("conv2", &path).unwrap();

    let n = mem.clear_mounted_documents_by_path(&path).unwrap();
    assert_eq!(n, 2, "应清除两个会话的挂载");
    assert!(mem.list_mounted_documents("conv1").unwrap().is_empty());
    assert!(mem.list_mounted_documents("conv2").unwrap().is_empty());
}

// ───────── 会话激活集 + 文件夹操作（CONVERSATION-SCOPE.md）─────────

fn doc_in_folder(path: &str, name: &str, format: &str, text: &str, folder: &str) -> DocumentRow {
    let mut d = doc(path, name, format, text);
    d.folder_path = Some(folder.to_string());
    d
}

#[test]
fn folder_path_persisted_and_listed() {
    let mem = Memory::open_in_memory().unwrap();
    mem.upsert_document(doc_in_folder("/tmp/a.pdf", "A", "pdf", "内容A", "/曾国藩专题"));
    mem.upsert_document(doc_in_folder("/tmp/b.pdf", "B", "pdf", "内容B", "/曾国藩专题/书信集"));
    mem.upsert_document(doc_in_folder("/tmp/c.pdf", "C", "pdf", "内容C", "/方法论"));
    mem.upsert_document(doc_in_folder("/tmp/d.pdf", "D", "pdf", "内容D", "/Inbox"));

    // list_folders 返回所有有文件的文件夹路径。
    let folders = mem.list_folders().unwrap();
    assert!(folders.contains(&"/曾国藩专题".to_string()));
    assert!(folders.contains(&"/曾国藩专题/书信集".to_string()));
    assert!(folders.contains(&"/方法论".to_string()));
    assert!(folders.contains(&"/Inbox".to_string()));

    // list_documents_by_folder 只返回直接子文件。
    let zeng = mem.list_documents_by_folder(Some("/曾国藩专题")).unwrap();
    assert_eq!(zeng.len(), 1, "/曾国藩专题 直接子文件只有 A（书信集是子文件夹）");
    assert_eq!(zeng[0].2, "A");

    // list_folder_tree 后端构建嵌套树 + 排序（Inbox 置顶）。
    let tree = mem.list_folder_tree().unwrap();
    // Inbox 应在最前。
    assert_eq!(tree[0].name, "Inbox");
    // 曾国藩专题 应有子节点 书信集。
    let zgf = tree.iter().find(|n| n.name == "曾国藩专题").expect("曾国藩专题 在树中");
    assert_eq!(zgf.children.len(), 1);
    assert_eq!(zgf.children[0].name, "书信集");
    assert_eq!(zgf.children[0].path, "/曾国藩专题/书信集");
}

#[test]
fn move_document_changes_folder() {
    let mem = Memory::open_in_memory().unwrap();
    mem.upsert_document(doc_in_folder("/tmp/x.pdf", "X", "pdf", "内容", "/Inbox"));
    // 移动到 /专题
    let n = mem.move_document("/tmp/x.pdf", Some("/专题")).unwrap();
    assert_eq!(n, 1);
    // 验证新文件夹
    let moved = mem.list_documents_by_folder(Some("/专题")).unwrap();
    assert_eq!(moved.len(), 1);
    // Inbox 应空了
    let inbox = mem.list_documents_by_folder(Some("/Inbox")).unwrap();
    assert_eq!(inbox.len(), 0);
}

#[test]
fn rename_folder_recursive() {
    let mem = Memory::open_in_memory().unwrap();
    mem.upsert_document(doc_in_folder("/tmp/a.pdf", "A", "pdf", "a", "/旧名"));
    mem.upsert_document(doc_in_folder("/tmp/b.pdf", "B", "pdf", "b", "/旧名/子目录"));
    // 重命名 /旧名 → /新名
    let n = mem.rename_folder("/旧名", "/新名").unwrap();
    assert_eq!(n, 2, "应重命名 2 个文件的 folder_path");
    // 验证
    let new_main = mem.list_documents_by_folder(Some("/新名")).unwrap();
    assert_eq!(new_main.len(), 1, "/新名 直接子文件 = A");
    let new_sub = mem.list_documents_by_folder(Some("/新名/子目录")).unwrap();
    assert_eq!(new_sub.len(), 1, "/新名/子目录 = B");
    // 旧名应不存在
    let old = mem.list_documents_by_folder(Some("/旧名")).unwrap();
    assert_eq!(old.len(), 0);
}

#[test]
fn delete_folder_removes_all_files_and_fts() {
    let mem = Memory::open_in_memory().unwrap();
    let d1 = doc_in_folder("/tmp/a.pdf", "A", "pdf", "向量数据库", "/专题");
    let d2 = doc_in_folder("/tmp/b.pdf", "B", "pdf", "知识图谱", "/专题/子目录");
    let id1 = mem.upsert_document(d1).unwrap();
    let id2 = mem.upsert_document(d2).unwrap();
    mem.index_document(&id1).unwrap();
    mem.index_document(&id2).unwrap();
    // 删文件夹
    let n = mem.delete_folder("/专题").unwrap();
    assert_eq!(n, 2, "应删除 2 个文件");
    // 文件夹应不存在
    let folders = mem.list_folders().unwrap();
    assert!(!folders.iter().any(|f| f.starts_with("/专题")));
    // FTS5 应搜不到了
    let hits = mem.search_documents("向量", 10).unwrap();
    assert!(hits.is_empty(), "删文件夹后 FTS5 应无命中");
}

#[test]
fn active_scope_resolve_and_filter() {
    let mem = Memory::open_in_memory().unwrap();
    // 先创建会话（list_mounted_documents 等需要 conversation 存在）
    let conv = mem.create_conversation(Some("测试会话")).unwrap();
    let conv_id = &conv.id;
    // 插入文件到两个文件夹
    mem.upsert_document(doc_in_folder("/tmp/a.pdf", "A", "pdf", "内容", "/曾国藩专题"));
    mem.upsert_document(doc_in_folder("/tmp/b.pdf", "B", "pdf", "内容", "/曾国藩专题/书信集"));
    mem.upsert_document(doc_in_folder("/tmp/c.pdf", "C", "pdf", "内容", "/方法论"));

    // 默认激活集为空
    let folders = mem.get_active_folders(conv_id).unwrap();
    assert!(folders.is_empty(), "新会话默认空激活集");
    let paths = mem.resolve_active_doc_paths(conv_id).unwrap();
    assert!(paths.is_empty(), "空激活集 → 空文档 path 列表");

    // 设置激活文件夹 = /曾国藩专题
    mem.set_active_folders(conv_id, &["/曾国藩专题".to_string()]).unwrap();
    let paths = mem.resolve_active_doc_paths(conv_id).unwrap();
    assert_eq!(paths.len(), 2, "/曾国藩专题 含子目录书信集 → 2 个文件");
    // /方法论 的文件不在激活集
    assert!(!paths.iter().any(|p| p.contains("c.pdf")), "方法论的 C 不在激活集");

    // @触发单文件激活：mount C 到会话
    mem.mount_document(conv_id, "/tmp/c.pdf").unwrap();
    let paths = mem.resolve_active_doc_paths(conv_id).unwrap();
    assert_eq!(paths.len(), 3, "@触发 C 后，激活集 = 2（曾国藩专题）+ 1（C）= 3");

    // 设置激活数据源
    mem.set_active_sources(conv_id, &["ontology".to_string(), "mydb".to_string()]).unwrap();
    let sources = mem.get_active_sources(conv_id).unwrap();
    assert_eq!(sources.len(), 2);
    assert!(sources.contains(&"ontology".to_string()));
}

#[test]
fn create_folder_persists_empty_and_lists() {
    let mem = Memory::open_in_memory().unwrap();
    // 创建空文件夹
    assert!(mem.create_folder("/曾国藩专题").unwrap(), "首次创建应返回 true");
    // 幂等：重复创建返回 false
    assert!(!mem.create_folder("/曾国藩专题").unwrap(), "重复创建应返回 false");
    // 创建子文件夹（空）
    assert!(mem.create_folder("/曾国藩专题/书信集").unwrap());

    // list_folders 应包含空文件夹（即使无文件）
    let folders = mem.list_folders().unwrap();
    assert!(folders.contains(&"/曾国藩专题".to_string()), "空文件夹应被持久化并列出");
    assert!(folders.contains(&"/曾国藩专题/书信集".to_string()), "空子文件夹应被持久化");

    // list_folder_tree 应展示空文件夹的层级
    let tree = mem.list_folder_tree().unwrap();
    let zgf = tree.iter().find(|n| n.name == "曾国藩专题").expect("空文件夹在树中");
    assert_eq!(zgf.children.len(), 1, "空子文件夹应出现在 children");
    assert_eq!(zgf.children[0].name, "书信集");
}

#[test]
fn create_folder_merges_with_document_folders() {
    // 双轨合并：folders 表 + documents.folder_path 去重
    let mem = Memory::open_in_memory().unwrap();
    // 文件落到 /有文件目录
    mem.upsert_document(doc_in_folder("/tmp/a.pdf", "A", "pdf", "内容", "/有文件目录"));
    // 手动创建空文件夹 /空目录
    mem.create_folder("/空目录").unwrap();

    let folders = mem.list_folders().unwrap();
    assert!(folders.contains(&"/有文件目录".to_string()), "文件推导的文件夹应列出");
    assert!(folders.contains(&"/空目录".to_string()), "空文件夹应列出");
    // 不应有重复
    let count_dup = folders.iter().filter(|f| *f == &"/有文件目录".to_string()).count();
    assert_eq!(count_dup, 1, "双轨合并去重");
}

#[test]
fn delete_folder_removes_empty_folder_record() {
    let mem = Memory::open_in_memory().unwrap();
    mem.create_folder("/空目录").unwrap();
    mem.create_folder("/空目录/子空").unwrap();
    // 删除空文件夹（无文件）
    let n = mem.delete_folder("/空目录").unwrap();
    assert_eq!(n, 0, "无文件删除");
    // folders 表记录应被清除
    let folders = mem.list_folders().unwrap();
    assert!(!folders.iter().any(|f| f.starts_with("/空目录")), "删空文件夹后表记录清除");
}

#[test]
fn rename_folder_updates_empty_folder_record() {
    let mem = Memory::open_in_memory().unwrap();
    mem.create_folder("/旧空").unwrap();
    mem.create_folder("/旧空/子空").unwrap();
    // 重命名 /旧空 → /新空
    let n = mem.rename_folder("/旧空", "/新空").unwrap();
    assert_eq!(n, 0, "无文件受影响");
    // folders 表应更新
    let folders = mem.list_folders().unwrap();
    assert!(folders.contains(&"/新空".to_string()), "重命名后新路径存在");
    assert!(folders.contains(&"/新空/子空".to_string()), "子文件夹路径前缀替换");
    assert!(!folders.iter().any(|f| f.starts_with("/旧空")), "旧路径清除");
}

#[test]
fn skill_md_visible_to_list_documents_but_excluded_from_folder_views() {
    // 决策 20 + 资源支持：skill body（format='skill-md'）与 references/assets/scripts
    // 三类资源（format='skill-resource'）入库 documents。
    // - list_documents（模型工具用）：包含 skill-md，由 document_tools 层 allowed_paths
    //   过滤，只返回本会话激活的 skill 文档（修复过去“模型拿不到 body id”的断链）。
    // - list_documents_by_folder / list_mounted_documents（前端 Library / @ 菜单用）：
    //   仍排除 skill-md，skill 走独立的 Inspector SkillTogglePanel 暴露。
    let mem = Memory::open_in_memory().unwrap();

    // 一篇普通文件 + 一个 skill（folder_path 均 None，模拟根目录散文件场景）
    let _file_id = mem.upsert_document(doc(
        "/tmp/notes.txt",
        "笔记.txt",
        "txt",
        "普通文件内容",
    )).unwrap();
    let skill_row = memory::DocumentRow {
        id: memory::new_document_id(),
        path: "skill://test-skill".to_string(), // 虚拟 path
        name: "test-skill".to_string(),
        format: "skill-md".to_string(),
        text: "# Test Skill\nbody".to_string(),
        char_count: 16,
        created_at: memory::now_ms(),
        folder_path: None, // skill 不进文件夹，与 ensure_skill_documented 一致
        source_conv_id: None,
    };
    let skill_id = mem.upsert_document(skill_row).unwrap();

    // 1. list_documents（模型 list_documents 工具用）：现在包含 skill-md。
    //    设计变更（skill references 支持修复）：过去排除 skill-md 是因为模型拿不到
    //    body id、preamble 却叫它“先 list 找”，形成断链。现在 list_documents 包含
    //    skill-md 与 skill-resource，由 document_tools::list_documents_tool 层的
    //    allowed_paths（doc_paths_set）过滤——只有本会话激活的 skill 文档才返回给模型。
    let all = mem.list_documents().unwrap();
    assert!(all.iter().any(|(_, _, _, format, _, _, _)| format == "skill-md"),
        "list_documents 现在应包含 skill-md（供模型发现）");
    assert_eq!(all.len(), 2, "普通文件 + skill body 都应列出");

    // 2. list_documents_by_folder(None)（根目录散文件）：skill folder_path=None 会被命中，需过滤
    let root_docs = mem.list_documents_by_folder(None).unwrap();
    assert!(root_docs.iter().all(|(_, _, format, _, _, _)| format != "skill-md"),
        "list_documents_by_folder 不应返回 skill-md");
    assert_eq!(root_docs.len(), 1, "根目录仅普通文件");

    // 3. list_mounted_documents：防御性过滤（skill 不走 mount，但兑底不出现）
    let conv = mem.create_conversation(None).unwrap().id;
    // 模拟异常：直接往 conversation_documents 塞 skill path（正常流程不会发生）
    mem.mount_document(&conv, "skill://test-skill").unwrap();
    let mounted = mem.list_mounted_documents(&conv).unwrap();
    assert!(mounted.iter().all(|m| m.format != "skill-md"),
        "list_mounted_documents 防御性过滤 skill-md");

    // 4. read_document_by_path 仍能查到 skill（后端 send_message 注脚逻辑依赖此）
    let read = mem.read_document_by_path("skill://test-skill").unwrap();
    assert!(read.is_some(), "skill 按 path 仍可读（注脚逻辑用）");
    let (_, name, format, _, _) = read.unwrap();
    assert_eq!(name, "test-skill");
    assert_eq!(format, "skill-md");

    // 5. read_document(id) 仍能读 skill 全文（read_document 工具 Tier 2）
    let by_id = mem.read_document(&skill_id, None, None).unwrap();
    assert!(by_id.is_some(), "skill 按 id 仍可读全文");
}

#[test]
fn active_ontologies_get_set_roundtrip() {
    // 决策：会话页面 @OntologyName 引用本体。active_ontologies 存 ontology api_name 列表。
    // 与 folders/sources 同模式（conversations.active_ontologies JSON 列），
    // 但不展开成 doc_paths——agent 只读工具直接用 api_name 查 store。
    let mem = Memory::open_in_memory().unwrap();
    let conv = mem.create_conversation(Some("本体引用测试")).unwrap();
    let conv_id = &conv.id;

    // 默认空激活集
    let onts = mem.get_active_ontologies(conv_id).unwrap();
    assert!(onts.is_empty(), "新会话默认无本体引用");

    // 设置引用两个本体
    mem.set_active_ontologies(conv_id, &["SupplyChain".to_string(), "Sales".to_string()]).unwrap();
    let onts = mem.get_active_ontologies(conv_id).unwrap();
    assert_eq!(onts.len(), 2);
    assert!(onts.contains(&"SupplyChain".to_string()));
    assert!(onts.contains(&"Sales".to_string()));

    // 增量更新：只引用一个
    mem.set_active_ontologies(conv_id, &["SupplyChain".to_string()]).unwrap();
    let onts = mem.get_active_ontologies(conv_id).unwrap();
    assert_eq!(onts.len(), 1, "增量更新覆盖旧值");

    // 清空
    mem.set_active_ontologies(conv_id, &[]).unwrap();
    let onts = mem.get_active_ontologies(conv_id).unwrap();
    assert!(onts.is_empty(), "空 Vec 清空激活集");
}
