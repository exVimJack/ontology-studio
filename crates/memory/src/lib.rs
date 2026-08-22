//! memory: 统一存储（SQLite + jieba FTS5）
//!
//! 架构（见 ARCHITECTURE.md §四 / 决策 3）：
//!   - rusqlite(bundled)：会话 / 消息 / 元数据同一 .db 文件
//!   - sqlite-jieba-tokenizer：FTS5 中文分词（一期收尾：文件检索）
//!   - 单一真相源，跨窗口一致

pub mod documents;
pub mod error;
pub mod jieba_tokenizer;
pub mod message;
pub mod repo;
pub mod skill_repo;
pub mod timestamp;

pub use documents::{DocumentRow, FolderNode, MountedDoc, SearchHit, new_document_id, now_ms, canonicalize_path, normalize_folder_path};
pub use error::{MemoryError, MemoryResult};
pub use message::{MessageRole, MessageRow, MessageStatus};
pub use repo::{ConversationRow, ConversationRepo, ConversationSummary};
pub use skill_repo::{DisabledSkillRow, ConversationSkillRow};
pub use timestamp::Timestamp;

use std::path::Path;

use rusqlite::Connection;

/// 统一存储句柄。持有一个 SQLite 连接（多线程下用 Mutex 串行化，见 Repo 内部）。
///
/// 另持有 DB 文件路径，用于开独立连接做后台 FTS5 索引构建（异步，不抢主连接锁；
/// WAL 模式下多连接可并发：读不阻塞写、写串行）。见 [`index_document_async`]。
///
/// 线程安全：rusqlite::Connection 默认非 Sync，但加 Mutex 后可跨线程共享。
/// Tauri AppState 会包一层 Arc，见 src-tauri/state.rs。
pub struct Memory {
    conn: std::sync::Mutex<Connection>,
    /// DB 文件路径（None = 内存库，无异步索引能力）。
    db_path: Option<std::path::PathBuf>,
}

impl Memory {
    /// 打开（或创建）位于 `path` 的库文件，并完成 schema 初始化。
    pub fn open(path: impl AsRef<Path>) -> MemoryResult<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path)?;
        register_fts5_tokenizers(&conn);
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
            db_path: Some(path),
        })
    }

    /// 内存库（测试用）。无异步索引能力（db_path=None）。
    pub fn open_in_memory() -> MemoryResult<Self> {
        let conn = Connection::open_in_memory()?;
        register_fts5_tokenizers(&conn);
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
            db_path: None,
        })
    }

    /// 取一个连接的锁，供 Repo 操作。锁在 Repo 方法内短时持有。
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner()) // poison 不影响读出数据
    }

    /// DB 文件路径（内存库返回 None）。用于在独立连接上做后台 FTS5 索引构建。
    /// WAL 模式下此连接与主连接并发安全（读不阻塞写、写串行）。
    pub fn db_path(&self) -> Option<&std::path::Path> {
        self.db_path.as_deref()
    }

    /// 开一个独立连接（已注册 jieba tokenizer + WAL），用于后台索引等长时操作。
    /// 仅文件库可用（内存库返回 None）。
    pub fn open_indexer_connection(&self) -> Option<MemoryResult<Connection>> {
        let path = self.db_path.as_ref()?;
        match Connection::open(path) {
            Ok(conn) => {
                if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL;") {
                    tracing::warn!(error = %e, "indexer conn: set WAL failed");
                }
                register_fts5_tokenizers(&conn);
                Some(Ok(conn))
            }
            Err(e) => Some(Err(e.into())),
        }
    }

    fn init_schema(conn: &Connection) -> MemoryResult<()> {
        // 启用 WAL：并发读不阻塞写，桌面单进程多窗口更顺。
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(
            r#"
            -- 会话
            CREATE TABLE IF NOT EXISTS conversations (
                id           TEXT PRIMARY KEY,            -- UUID v4 字符串
                title        TEXT NOT NULL DEFAULT '新会话',
                created_at   INTEGER NOT NULL,            -- unix ms
                updated_at   INTEGER NOT NULL,            -- unix ms（消息变动时刷新）
                pinned       INTEGER NOT NULL DEFAULT 0   -- 0/1，置顶
            );

            -- 消息
            CREATE TABLE IF NOT EXISTS messages (
                id              TEXT PRIMARY KEY,         -- UUID v4
                conversation_id TEXT NOT NULL,
                role            TEXT NOT NULL,            -- 'user' | 'assistant' | 'system'
                status          TEXT NOT NULL,            -- 'streaming'|'complete'|'error'|'cancelled'
                content         TEXT NOT NULL DEFAULT '', -- 累积的纯文本/Markdown（流式增量写入）
                error           TEXT,                     -- status='error' 时的可空信息
                model           TEXT,                     -- assistant 消息记录所用模型
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL,
                -- 二期 B1：真实 token usage（provider 报告时落库，用于下次压缩判定）
                prompt_tokens     INTEGER,                -- 输入 token（含历史+本轮 prompt）
                completion_tokens INTEGER,                -- 输出 token
                total_tokens      INTEGER,                -- 总 token
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversations(updated_at DESC);

            -- 软删除：删除会话级联删消息（上方 ON DELETE CASCADE）。一期不做回收站。
            "#,
        )?;

        // 二期 B1 迁移：旧库 messages 表缺 token usage 列时补上。
        // 幂等：用 PRAGMA table_info 检测列是否存在。
        Self::ensure_columns(conn, "messages", &[
            ("prompt_tokens", "INTEGER"),
            ("completion_tokens", "INTEGER"),
            ("total_tokens", "INTEGER"),
        ])?;

        // reasoning 列（reasoning 模型的思考链，独立于 content）。
        Self::ensure_columns(conn, "messages", &[("reasoning", "TEXT")])?;

        // documents 表 + FTS5 虚拟表（jieba 分词，异步索引，无触发器）。
        Self::init_documents_schema(conn)?;

        // Skill 系统两张表（决策 20）。
        Self::init_skill_schema(conn)?;

        // documents.indexed_at 迁移：异步索引标记（0=未索引，>0=索引完成 unix ms）。
        // 首次迁移（列不存在）时：加列后把现有文档 indexed_at 置为 created_at，
        // 信任旧 FTS5 索引有效（旧库的文档是旧触发器建的索引，保持可搜）。
        // 新 ingest 的文档 upsert 时 indexed_at=0，走异步 index_document。
        if Self::ensure_columns(conn, "documents", &[("indexed_at", "INTEGER NOT NULL DEFAULT 0")])? {
            conn.execute(
                "UPDATE documents SET indexed_at = created_at WHERE indexed_at = 0",
                [],
            )?;
        }

        // 会话级知识范围（CONVERSATION-SCOPE.md）：documents 加 folder_path，
        // conversations 加 active_folders/active_sources（JSON 字符串）。
        // 首次迁移（folder_path 列不存在）时：旧文档全部归入 /Inbox（未分类），
        // 用户后续可在 Library 文件树里手动移动归类。
        if Self::ensure_columns(conn, "documents", &[("folder_path", "TEXT")])? {
            conn.execute(
                "UPDATE documents SET folder_path = '/Inbox' WHERE folder_path IS NULL",
                [],
            )?;
        }
        Self::ensure_columns(conn, "documents", &[("source_conv_id", "TEXT")])?; // 会话上传来源（可选，用于 Inbox 展示来源会话）
        Self::ensure_columns(conn, "conversations", &[("active_folders", "TEXT"), ("active_sources", "TEXT")])?;
        // 本体引用（决策：会话页面 @OntologyName）：conversations 加 active_ontologies（JSON）。
        // 存 ontology api_name 列表，如 ["SupplyChain"]。与 folders/sources 同模式。
        Self::ensure_columns(conn, "conversations", &[("active_ontologies", "TEXT")])?;

        // folder_path 索引：Library 按文件夹列出文件、删文件夹批量查找。
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_documents_folder ON documents(folder_path)",
            [],
        )?;

        // 独立 folders 表（决策 19 修订）：持久化空文件夹。
        // 文件夹树 = folders 表 ∪ DISTINCT documents.folder_path（双轨合并去重）。
        // path 作为 PK（如 /曾国藩专题/书信集），parent_path 为父文件夹路径（根为 NULL）。
        conn.execute(
            "CREATE TABLE IF NOT EXISTS folders (\n\
             \tpath TEXT PRIMARY KEY,\n\
             \tparent_path TEXT,\n\
             \tname TEXT NOT NULL,\n\
             \tcreated_at INTEGER NOT NULL\n\
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent_path)",
            [],
        )?;

        Ok(())
    }

    /// 幂等加列：检测 col 是否已存在，不存在则 ADD COLUMN。
    fn ensure_columns(
        conn: &Connection,
        table: &str,
        cols: &[(&str, &str)],
    ) -> MemoryResult<bool> {
        let existing: std::collections::HashSet<String> = conn
            .prepare(&format!("PRAGMA table_info({table})"))?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        let mut added = false;
        for (name, ty) in cols {
            if !existing.contains(*name) {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {name} {ty}"),
                    [],
                )?;
                added = true;
            }
        }
        Ok(added)
    }
}

/// 注册 FTS5 jieba 中文分词 tokenizer。
///
/// tokenizer 是 per-connection 的（不同于进程级 auto_extension），
/// 每个新 Connection::open 后都要调用一次。注册后建 FTS5 表时可指定
/// `tokenize='jieba'`，MATCH 检索由 jieba 词语分词驱动（见 PoC 测试）。
fn register_fts5_tokenizers(conn: &Connection) {
    // 注册分句版 jieba tokenizer（替代 sqlite-jieba-tokenizer 0.6）。
    // 分句避免 jieba DAG 对大全文 O(n²) 退化（1.4M 字符 600s→1s）。
    if let Err(e) = rusqlite_ext::register_tokenizer::<jieba_tokenizer::JiebaSentenceTokenizer>(conn, ()) {
        tracing::warn!(error = %e, "register jieba fts5 tokenizer failed");
    }
}
