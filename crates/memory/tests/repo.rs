//! memory crate 单元测试：会话/消息 CRUD 与流式增量。
//!
//! 用内存库跑，不落盘。

use memory::{MessageRole, MessageStatus};

fn open() -> memory::Memory {
    memory::Memory::open_in_memory().expect("open in-memory db")
}

#[test]
fn create_and_list_conversations() {
    let db = open();
    let c1 = db.create_conversation(Some("第一会话")).unwrap();
    let c2 = db.create_conversation(None).unwrap();
    assert_eq!(c1.title, "第一会话");
    assert_eq!(c2.title, "新会话");
    assert!(!c1.pinned);

    let list = db.list_conversations().unwrap();
    assert_eq!(list.len(), 2);
    // 两个会话均无消息
    assert!(list.iter().all(|s| s.message_count == 0));
    assert!(list.iter().all(|s| s.last_message_preview.is_none()));
    // 标题能取回
    let titles: Vec<_> = list.iter().map(|s| s.conv.title.as_str()).collect();
    assert!(titles.contains(&"第一会话"));
    assert!(titles.contains(&"新会话"));
}

#[test]
fn pin_and_rename() {
    let db = open();
    let c = db.create_conversation(None).unwrap();
    db.set_pinned(&c.id, true).unwrap();
    db.rename_conversation(&c.id, "重命名").unwrap();
    let got = db.get_conversation(&c.id).unwrap();
    assert_eq!(got.title, "重命名");
    assert!(got.pinned);
}

#[test]
fn message_lifecycle_and_streaming_append() {
    let db = open();
    let c = db.create_conversation(None).unwrap();

    // user 发送
    let u = db
        .create_message(&c.id, MessageRole::User, MessageStatus::Complete, "你好", None)
        .unwrap();
    assert_eq!(u.role, MessageRole::User);

    // assistant 占位 + 流式增量
    let a = db
        .create_message(
            &c.id,
            MessageRole::Assistant,
            MessageStatus::Streaming,
            "",
            Some("gpt-4o"),
        )
        .unwrap();
    db.append_message_text(&a.id, "你好").unwrap();
    db.append_message_text(&a.id, "，世界").unwrap();
    db.set_message_status(&a.id, MessageStatus::Complete, None).unwrap();

    let msgs = db.list_messages(&c.id).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1].content, "你好，世界");
    assert_eq!(msgs[1].status, MessageStatus::Complete);
    assert_eq!(msgs[1].model.as_deref(), Some("gpt-4o"));

    // summary 预览
    let list = db.list_conversations().unwrap();
    assert_eq!(list[0].message_count, 2);
    assert_eq!(list[0].last_message_preview.as_deref(), Some("你好，世界"));
}

#[test]
fn delete_conversation_cascades_messages() {
    let db = open();
    let c = db.create_conversation(None).unwrap();
    db.create_message(&c.id, MessageRole::User, MessageStatus::Complete, "x", None).unwrap();
    db.delete_conversation(&c.id).unwrap();
    let msgs = db.list_messages(&c.id).unwrap();
    assert_eq!(msgs.len(), 0); // 级联删除
}

#[test]
fn not_found_errors() {
    let db = open();
    let err = db.get_conversation("nope").unwrap_err();
    assert!(matches!(err, memory::MemoryError::NotFound(_)));
    let err = db.append_message_text("nope", "x").unwrap_err();
    assert!(matches!(err, memory::MemoryError::NotFound(_)));
}

#[test]
fn error_status_persists_message() {
    let db = open();
    let c = db.create_conversation(None).unwrap();
    let a = db
        .create_message(&c.id, MessageRole::Assistant, MessageStatus::Streaming, "", None)
        .unwrap();
    db.append_message_text(&a.id, "部分内容").unwrap();
    db.set_message_status(&a.id, MessageStatus::Error, Some("API 401")).unwrap();
    let msgs = db.list_messages(&c.id).unwrap();
    assert_eq!(msgs[0].status, MessageStatus::Error);
    assert_eq!(msgs[0].error.as_deref(), Some("API 401"));
    assert_eq!(msgs[0].content, "部分内容"); // 已产出内容保留（§十五 乐观回滚）
}

#[test]
fn cancel_keeps_partial_content() {
    let db = open();
    let c = db.create_conversation(None).unwrap();
    let a = db
        .create_message(&c.id, MessageRole::Assistant, MessageStatus::Streaming, "", None)
        .unwrap();
    db.append_message_text(&a.id, "前半").unwrap();
    db.set_message_status(&a.id, MessageStatus::Cancelled, None).unwrap();
    let msgs = db.list_messages(&c.id).unwrap();
    assert_eq!(msgs[0].status, MessageStatus::Cancelled);
    assert_eq!(msgs[0].content, "前半"); // 中止保留 partial（§十五）
}

#[test]
fn delete_message_and_after_truncates() {
    let db = open();
    let c = db.create_conversation(None).unwrap();
    // 构造 4 条消息：u1, a1, u2, a2
    let u1 = db
        .create_message(&c.id, MessageRole::User, MessageStatus::Complete, "u1", None)
        .unwrap();
    let a1 = db
        .create_message(&c.id, MessageRole::Assistant, MessageStatus::Complete, "a1", None)
        .unwrap();
    let _u2 = db
        .create_message(&c.id, MessageRole::User, MessageStatus::Complete, "u2", None)
        .unwrap();
    let a2 = db
        .create_message(&c.id, MessageRole::Assistant, MessageStatus::Complete, "a2", None)
        .unwrap();
    assert_eq!(db.list_messages(&c.id).unwrap().len(), 4);

    // 从 a1 截断：应删 a1, u2, a2（3 条），保留 u1
    let affected = db.delete_message_and_after(&a1.id).unwrap();
    assert_eq!(affected, 3);
    let msgs = db.list_messages(&c.id).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].id, u1.id);
    assert_eq!(msgs[0].content, "u1");
}

#[test]
fn delete_message_and_after_not_found() {
    let db = open();
    let err = db.delete_message_and_after("nope").unwrap_err();
    assert!(matches!(err, memory::MemoryError::NotFound(_)));
}

#[test]
fn list_messages_limited_returns_last_n() {
    let db = open();
    let c = db.create_conversation(None).unwrap();
    // 创建 10 条 user 消息。created_at 可能同毫秒，但 rowid 递增，
    // SQLite DESC 偏向高 rowid（后插入的）。
    for i in 1..=10 {
        db.create_message(
            &c.id,
            MessageRole::User,
            MessageStatus::Complete,
            &format!("msg {i}"),
            None,
        )
        .unwrap();
    }
    // 全量 = 10
    assert_eq!(db.list_messages(&c.id).unwrap().len(), 10);
    // limit 3：应返回 3 条，ASC 升序。
    let limited = db.list_messages_limited(&c.id, Some(3)).unwrap();
    assert_eq!(limited.len(), 3, "limit 3 should return exactly 3");
    // 验证：结果按 ASC 升序
    for w in limited.windows(2) {
        let a: u32 = w[0].content.strip_prefix("msg ").unwrap().parse().unwrap();
        let b: u32 = w[1].content.strip_prefix("msg ").unwrap().parse().unwrap();
        assert!(a <= b, "should be ASC: {w:?}");
    }
    // limit > 实际条数 = 全量
    let all = db.list_messages_limited(&c.id, Some(999)).unwrap();
    assert_eq!(all.len(), 10);
    // limit 0 = 空
    let empty = db.list_messages_limited(&c.id, Some(0)).unwrap();
    assert!(empty.is_empty());
}
