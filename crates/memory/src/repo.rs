//! 会话与消息的仓储层。
//!
//! 所有方法在内部 `Memory::lock()` 取连接锁，短时持有，保证多线程安全。
//! 时间戳统一用 unix 毫秒（i64），与前端 `Date.now()` 对齐。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::message::{MessageRole, MessageRow, MessageStatus};
use crate::timestamp::Timestamp;
use crate::{Memory, MemoryError, MemoryResult};

/// 会话行（DB 行 ↔ 前端 DTO）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ConversationRow {
    pub id: String,
    pub title: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub pinned: bool,
}

/// 会话 + 首条预览（侧栏列表用，避免一次拉全部消息）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ConversationSummary {
    #[serde(flatten)]
    pub conv: ConversationRow,
    /// 最近一条消息的纯文本预览（截断），无消息则为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,
    /// 消息条数
    pub message_count: i32,
}

// ── 会话仓储 ──────────────────────────────────────────────────

impl Memory {
    /// 新建会话，返回完整行。
    pub fn create_conversation(&self, title: Option<&str>) -> MemoryResult<ConversationRow> {
        let conn = self.lock();
        let now = now_ms();
        let row = ConversationRow {
            id: Uuid::new_v4().to_string(),
            title: title.map(|s| s.to_string()).unwrap_or_else(|| "新会话".to_string()),
            created_at: now,
            updated_at: now,
            pinned: false,
        };
        conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, pinned) VALUES (?1, ?2, ?3, ?4, 0)",
            rusqlite::params![&row.id, &row.title, row.created_at, row.updated_at],
        )?;
        Ok(row)
    }

    /// 列出会话（按 updated_at 倒序），带首条预览与消息数。
    pub fn list_conversations(&self) -> MemoryResult<Vec<ConversationSummary>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            r#"
            SELECT c.id, c.title, c.created_at, c.updated_at, c.pinned,
                   (SELECT content FROM messages m
                      WHERE m.conversation_id = c.id
                      ORDER BY m.created_at DESC LIMIT 1) AS preview,
                   (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) AS cnt
            FROM conversations c
            ORDER BY c.pinned DESC, c.updated_at DESC
            "#,
        )?;
        // SELECT 列序: id(0) title(1) created_at(2) updated_at(3) pinned(4) preview(5) cnt(6)
        let rows = stmt.query_map([], |r| {
            let preview: Option<String> = r.get(5)?;
            let preview = preview.map(|s| truncate(&s, 80));
            Ok(ConversationSummary {
                conv: ConversationRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    pinned: r.get::<_, i64>(4)? != 0,
                },
                last_message_preview: preview,
                message_count: r.get::<_, i64>(6)? as i32,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_conversation(&self, id: &str) -> MemoryResult<ConversationRow> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT id, title, created_at, updated_at, pinned FROM conversations WHERE id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok(ConversationRow {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                        pinned: r.get::<_, i64>(4)? != 0,
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MemoryError::NotFound(id.to_string()),
                other => other.into(),
            })?;
        Ok(row)
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> MemoryResult<ConversationRow> {
        let conn = self.lock();
        let now = now_ms();
        let affected = conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![title, now, id],
        )?;
        if affected == 0 {
            return Err(MemoryError::NotFound(id.to_string()));
        }
        drop(conn);
        self.get_conversation(id)
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) -> MemoryResult<()> {
        let conn = self.lock();
        let affected = conn.execute(
            "UPDATE conversations SET pinned = ?1 WHERE id = ?2",
            rusqlite::params![pinned as i64, id],
        )?;
        if affected == 0 {
            return Err(MemoryError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn delete_conversation(&self, id: &str) -> MemoryResult<()> {
        let conn = self.lock();
        let affected = conn.execute("DELETE FROM conversations WHERE id = ?1", rusqlite::params![id])?;
        if affected == 0 {
            return Err(MemoryError::NotFound(id.to_string()));
        }
        // 手动清理挂载关联（conversation_documents 无 FK CASCADE）。
        let _ = conn.execute(
            "DELETE FROM conversation_documents WHERE conversation_id = ?1",
            rusqlite::params![id],
        );
        Ok(())
    }

    /// 触摸会话的 updated_at（消息变动时调用，保持列表排序正确）。
    pub fn touch_conversation(&self, id: &str) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now_ms(), id],
        )?;
        Ok(())
    }
}

// ── 消息仓储 ──────────────────────────────────────────────────

impl Memory {
    /// 创建一条消息（user 发送时 / assistant 占位时调用）。
    pub fn create_message(
        &self,
        conversation_id: &str,
        role: MessageRole,
        status: MessageStatus,
        content: &str,
        model: Option<&str>,
    ) -> MemoryResult<MessageRow> {
        let id = Uuid::new_v4().to_string();
        self.create_message_with_id(conversation_id, role, status, content, model, &id)
    }

    /// 创建一条消息并指定预生成的 id（stream_with_memory 流程用：
    /// 前端需预生成 message_id 做 patch，turn 结束后用该 id 落库）。
    pub fn create_message_with_id(
        &self,
        conversation_id: &str,
        role: MessageRole,
        status: MessageStatus,
        content: &str,
        model: Option<&str>,
        id: &str,
    ) -> MemoryResult<MessageRow> {
        self.create_message_with_id_reasoning(conversation_id, role, status, content, None, model, id)
    }

    /// 同上，但可带 reasoning（reasoning 模型的思考链）。turn 结束落库 assistant 消息时用。
    pub fn create_message_with_id_reasoning(
        &self,
        conversation_id: &str,
        role: MessageRole,
        status: MessageStatus,
        content: &str,
        reasoning: Option<&str>,
        model: Option<&str>,
        id: &str,
    ) -> MemoryResult<MessageRow> {
        let conn = self.lock();
        let now = now_ms();
        let row = MessageRow {
            id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            role,
            status,
            content: content.to_string(),
            reasoning: reasoning.map(|s| s.to_string()),
            error: None,
            model: model.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
        };
        conn.execute(
            r#"INSERT INTO messages
               (id, conversation_id, role, status, content, reasoning, error, model, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, ?9)"#,
            rusqlite::params![
                &row.id,
                &row.conversation_id,
                role.as_str(),
                status.as_str(),
                &row.content,
                row.reasoning.as_deref(),
                row.model.as_deref(),
                row.created_at,
                row.updated_at,
            ],
        )?;
        drop(conn);
        self.touch_conversation(conversation_id)?;
        Ok(row)
    }

    /// 创建一条消息并指定 created_at（二期 B2 历史压缩用，使摘要消息时间戳
    /// 早于被保留的近期消息，排序在历史开头）。
    pub fn create_message_at(
        &self,
        conversation_id: &str,
        role: MessageRole,
        status: MessageStatus,
        content: &str,
        model: Option<&str>,
        created_at: Timestamp,
    ) -> MemoryResult<MessageRow> {
        let conn = self.lock();
        let row = MessageRow {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role,
            status,
            content: content.to_string(),
            reasoning: None,
            error: None,
            model: model.map(|s| s.to_string()),
            created_at,
            updated_at: created_at,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
        };
        conn.execute(
            r#"INSERT INTO messages
               (id, conversation_id, role, status, content, reasoning, error, model, created_at, updated_at)
               VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8)"#,
            rusqlite::params![
                &row.id,
                &row.conversation_id,
                role.as_str(),
                status.as_str(),
                &row.content,
                row.model.as_deref(),
                row.created_at,
                row.updated_at,
            ],
        )?;
        drop(conn);
        self.touch_conversation(conversation_id)?;
        Ok(row)
    }

    /// 列出某会话的全部消息（按 created_at 升序，对话顺序）。
    ///
    /// `limit` 限制返回条数（取最后 N 条，None = 全部）。
    /// 用于侧栏加载历史——避免超长会话一次性拉取全量（含大段 assistant 回复）
    /// 阻塞 IPC 几秒。前端可翻页：offset=0 时取倒数 limit 条。
    pub fn list_messages(&self, conversation_id: &str) -> MemoryResult<Vec<MessageRow>> {
        self.list_messages_limited(conversation_id, None)
    }

    /// 同 list_messages，带 limit（None = 全部）。
    ///
    /// 实现：先 DESC 取最后 N 条，再 ASC 排序（子查询），保证对话顺序的同时
    /// 限制扫描行数。索引 `idx_messages_conv(conversation_id, created_at)` 覆盖。
    pub fn list_messages_limited(
        &self,
        conversation_id: &str,
        limit: Option<usize>,
    ) -> MemoryResult<Vec<MessageRow>> {
        let conn = self.lock();
        let sql;
        let params: Vec<Box<dyn rusqlite::types::ToSql>>;
        if let Some(n) = limit {
            sql = format!(
                "SELECT id, conversation_id, role, status, content, reasoning, error, model,\
                       created_at, updated_at, prompt_tokens, completion_tokens, total_tokens \
                FROM messages WHERE conversation_id = ?1 \
                ORDER BY created_at DESC, rowid DESC LIMIT {n}"
            );
            params = vec![Box::new(conversation_id.to_string())];
        } else {
            sql = r#"SELECT id, conversation_id, role, status, content, reasoning, error, model,
                      created_at, updated_at, prompt_tokens, completion_tokens, total_tokens
               FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC"#.to_string();
            params = vec![Box::new(conversation_id.to_string())];
        }
        let mut stmt = conn.prepare(&sql)?;
        // SELECT 列序: id(0) conv(1) role(2) status(3) content(4) reasoning(5) error(6) model(7)
        //              created_at(8) updated_at(9) prompt(10) completion(11) total(12)
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(param_refs.as_slice(), |r| {
            let role = MessageRole::parse(&r.get::<_, String>(2)?)
                .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad role")),
                ))?;
            let status = MessageStatus::parse(&r.get::<_, String>(3)?)
                .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad status")),
                ))?;
            Ok(MessageRow {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                role,
                status,
                content: r.get(4)?,
                reasoning: r.get(5)?,
                error: r.get(6)?,
                model: r.get(7)?,
                created_at: r.get(8)?,
                updated_at: r.get(9)?,
                prompt_tokens: r.get::<_, Option<i64>>(10)?.map(|v| v as u64),
                completion_tokens: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                total_tokens: r.get::<_, Option<i64>>(12)?.map(|v| v as u64),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        // 有限制时 SQL 走 DESC LIMIT，reverse 后恢复 ASC 顺序
        if limit.is_some() {
            out.reverse();
        }
        Ok(out)
    }

    /// 追加文本到某消息的 content（流式增量，§14.1 主链路）。
    /// 同时刷新 updated_at 与会话 updated_at。
    pub fn append_message_text(&self, message_id: &str, delta: &str) -> MemoryResult<()> {
        let conn = self.lock();
        let now = now_ms();
        let affected = conn.execute(
            r#"UPDATE messages
               SET content = content || ?1, updated_at = ?2
               WHERE id = ?3"#,
            rusqlite::params![delta, now, message_id],
        )?;
        if affected == 0 {
            return Err(MemoryError::NotFound(message_id.to_string()));
        }
        // 取 conversation_id 以 touch（避免再查一次，用子查询）
        conn.execute(
            r#"UPDATE conversations SET updated_at = ?1
               WHERE id = (SELECT conversation_id FROM messages WHERE id = ?2)"#,
            rusqlite::params![now, message_id],
        )?;
        Ok(())
    }

    /// 设置消息状态（complete/error/cancelled），可附带 error 文本。
    pub fn set_message_status(
        &self,
        message_id: &str,
        status: MessageStatus,
        error: Option<&str>,
    ) -> MemoryResult<()> {
        let conn = self.lock();
        let now = now_ms();
        let affected = conn.execute(
            r#"UPDATE messages SET status = ?1, error = ?2, updated_at = ?3 WHERE id = ?4"#,
            rusqlite::params![status.as_str(), error, now, message_id],
        )?;
        if affected == 0 {
            return Err(MemoryError::NotFound(message_id.to_string()));
        }
        Ok(())
    }

    /// 删除单条消息。
    pub fn delete_message(&self, message_id: &str) -> MemoryResult<()> {
        let conn = self.lock();
        let affected = conn.execute("DELETE FROM messages WHERE id = ?1", rusqlite::params![message_id])?;
        if affected == 0 {
            return Err(MemoryError::NotFound(message_id.to_string()));
        }
        Ok(())
    }

    /// 删除指定消息**及其之后**的所有消息（含自身），单事务原子完成。
    ///
    /// 用于前端「重新生成 assistant 回复」/「编辑 user 消息后重发」：
    /// 截断掉目标消息及后续，再以新内容 `send_message` 重发。
    /// 以 SQLite 隐含 `rowid`（插入自增）为时序基准，避免 created_at 同毫秒撞值。
    pub fn delete_message_and_after(&self, message_id: &str) -> MemoryResult<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        // 取目标消息的 conversation_id + rowid
        let row: (String, i64) = tx
            .query_row(
                "SELECT conversation_id, rowid FROM messages WHERE id = ?1",
                rusqlite::params![message_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => MemoryError::NotFound(message_id.to_string()),
                other => other.into(),
            })?;
        let (conv_id, rowid) = row;
        let affected = tx.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND rowid >= ?2",
            rusqlite::params![conv_id, rowid],
        )?;
        // touch 会话 updated_at（消息被截断，列表预览/排序需刷新）
        tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now_ms(), conv_id],
        )?;
        tx.commit()?;
        Ok(affected)
    }

    /// 删除某会话的全部消息，但保留会话本身（rig `ConversationMemory::clear` 用）。
    pub fn delete_conversation_messages(&self, conversation_id: &str) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            rusqlite::params![conversation_id],
        )?;
        Ok(())
    }

    /// 重置某消息的 content 为空（二期 B1 overflow 重试用）。
    ///
    /// overflow 重试前需清空已追加到 assistant 消息的半截文本，
    /// 否则重试生成的新内容会拼在旧文本后面。
    pub fn reset_message_content(&self, message_id: &str) -> MemoryResult<()> {
        let conn = self.lock();
        let now = now_ms();
        let affected = conn.execute(
            r#"UPDATE messages SET content = '', updated_at = ?1 WHERE id = ?2"#,
            rusqlite::params![now, message_id],
        )?;
        if affected == 0 {
            return Err(MemoryError::NotFound(message_id.to_string()));
        }
        Ok(())
    }

    /// 写入 provider 报告的真实 token usage 到某 assistant 消息（二期 B1）。
    ///
    /// 在流式完成（Done/收尾）时调用，把 `CompletionCall.usage` 落库。
    /// 下次发消息前用 `get_last_assistant_usage` 读取，替代 chars/4 估算做压缩判定。
    pub fn set_message_usage(
        &self,
        message_id: &str,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        total_tokens: Option<u64>,
    ) -> MemoryResult<()> {
        let conn = self.lock();
        let now = now_ms();
        let affected = conn.execute(
            r#"UPDATE messages
               SET prompt_tokens = ?1, completion_tokens = ?2, total_tokens = ?3, updated_at = ?4
               WHERE id = ?5"#,
            rusqlite::params![
                prompt_tokens.map(|v| v as i64),
                completion_tokens.map(|v| v as i64),
                total_tokens.map(|v| v as i64),
                now,
                message_id,
            ],
        )?;
        if affected == 0 {
            return Err(MemoryError::NotFound(message_id.to_string()));
        }
        Ok(())
    }

    /// 取某会话最近一条 assistant 消息的 prompt_tokens（二期 B1 压缩判定用）。
    ///
    /// 返回最近一条有 usage 记录的 assistant 消息的输入 token 数。
    /// None 表示无可用 usage（首次对话/provider 未报告/旧消息），上层降级为 chars/4 估算。
    pub fn get_last_assistant_usage(&self, conversation_id: &str) -> MemoryResult<Option<u64>> {
        let conn = self.lock();
        // query_row 无行时返回 Err(QueryReturnedNoRows)，转 Option。
        let v: Option<i64> = conn
            .query_row(
                r#"SELECT prompt_tokens FROM messages
                   WHERE conversation_id = ?1 AND role = 'assistant' AND prompt_tokens IS NOT NULL
                   ORDER BY created_at DESC LIMIT 1"#,
                rusqlite::params![conversation_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();
        Ok(v.map(|v| v as u64))
    }
}

// ── 便利别名 ───────────────────────────────────────────────────

/// 一期只用到会话+消息，给个聚合别名方便上层引用。
pub struct ConversationRepo;

fn now_ms() -> Timestamp {
    Timestamp::now()
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().take(max + 1).collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut t: String = chars[..max].iter().collect();
        t.push('…');
        t
    }
}
