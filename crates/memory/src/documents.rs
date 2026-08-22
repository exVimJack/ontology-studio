//! documents: 文档全文存储 + FTS5 检索（jieba 中文分词）。
//!
//! 架构（一期收尾：文件工具 + FTS5，替代向量 RAG）：
//!   - `documents` 表：存全文 + 元数据（path/name/format/char_count/created_at/indexed_at）
//!   - `documents_fts` FTS5 虚拟表：jieba 分词索引，external content table 模式
//!     （全文只存一份在 documents 表，FTS5 只存倒排索引，避免翻倍）
//!   - **异步索引**（无触发器）：upsert 只写主行（毫秒级），FTS5 索引由
//!     [`index_document`] 在独立连接 + spawn_blocking 后台构建（jieba 对大全文
//!     分词耗时数分钟，同步会阻塞 ingest 返回）。indexed_at 标记索引状态。
//!
//! 检索由 agent 工具（search_documents）驱动：模型给中文关键词 → FTS5 MATCH →
//! BM25 排序 + snippet 高亮。详见 ARCHITECTURE.md 决策（一期收尾）。

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::{MemoryError, MemoryResult};


/// 一篇已入库的文档（全文 + 元数据）。
#[derive(Debug, Clone)]
pub struct DocumentRow {
    pub id: String,           // UUID v4
    pub path: String,         // 源文件绝对路径（去重键）
    pub name: String,         // 文件名（展示用）
    pub format: String,       // pdf/docx/xlsx/epub/txt/...
    pub text: String,         // 全文
    pub char_count: u32,
    pub created_at: i64,      // unix ms
    pub folder_path: Option<String>,  // 文件夹路径（如 /曾国藩专题/书信集）；None=/Inbox（会话上传默认）
    pub source_conv_id: Option<String>, // 会话上传来源（Inbox 文件可选记来源会话，用于展示）
}

/// 搜索命中（不含全文，避免一次拉太多；全文走 read_document 分页）。
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub path: String,
    pub name: String,
    pub format: String,
    pub snippet: String,      // FTS5 snippet 高亮上下文
    pub rank: f64,            // BM25 分数（越小越相关）
}

/// 会话已挂载的文档（`@` 挂载，不含全文）。
#[derive(Debug, Clone)]
pub struct MountedDoc {
    pub path: String,
    pub name: String,
    pub format: String,
    pub char_count: u32,
    pub mounted_at: i64,
}

/// 文档存储 + FTS5 检索 Repo。
impl super::Memory {
    /// 初始化 documents 表 + FTS5 虚拟表 + 同步触发器。
    /// 在 init_schema 中调用，幂等。
    pub(crate) fn init_documents_schema(conn: &Connection) -> MemoryResult<()> {
        conn.execute_batch(
            r#"
            -- 文档全文表
            CREATE TABLE IF NOT EXISTS documents (
                id            TEXT PRIMARY KEY,           -- UUID v4
                path          TEXT NOT NULL UNIQUE,       -- 源文件路径（去重键）
                name          TEXT NOT NULL,              -- 文件名（展示用）
                format        TEXT NOT NULL,              -- pdf/docx/...
                text          TEXT NOT NULL,              -- 全文
                char_count    INTEGER NOT NULL,
                created_at    INTEGER NOT NULL            -- unix ms
            );
            CREATE INDEX IF NOT EXISTS idx_documents_name ON documents(name);
            CREATE INDEX IF NOT EXISTS idx_documents_format ON documents(format);

            -- FTS5 虚拟表（jieba 分词，external content table 模式）。
            -- content='documents' + content_rowid='rowid'：FTS5 不存全文副本，
            -- 只建倒排索引，通过 documents 表的 rowid 关联回查。支持 'delete'
            -- 命令（需原文）和 snippet()/bm25()。
            --
            -- **无触发器**：FTS5 索引由 [`index_document`] 显式异步构建，不走同步
            -- 触发器。原因：jieba 对大全文（1M+ 字符 PDF）分词建索引耗时数分钟，
            -- 同步触发器会阻塞 upsert 返回，前端“解析完成后卡住”。改为：upsert 只
            -- 写主行（毫秒级，但会先用旧值删旧 FTS5 索引保证一致性）→ 异步
            -- spawn_blocking 建 FTS5 索引 → indexed_at 标记。
            -- 搜索时 WHERE indexed_at>0 过滤未索引文档。
            DROP TRIGGER IF EXISTS documents_ai;
            DROP TRIGGER IF EXISTS documents_ad;
            DROP TRIGGER IF EXISTS documents_au;
            CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
                name,
                text,
                content='documents',
                content_rowid='rowid',
                tokenize='jieba'
            );
            -- 关闭 FTS5 自动增量合并（automerge=0）：避免索引构建时同步合并 b-tree
            -- 段拖慢写入。查询时合并多段稍慢，但检索低频可接受。
            INSERT OR IGNORE INTO documents_fts(documents_fts, rank) VALUES('automerge', 0);

            -- 会话 ↔ 文档 挂载关联表（`@` 挂载持久化）。
            -- 一篇文档可被多个会话挂载，一个会话可挂载多篇文档。
            -- 全文始终只存 documents 表一份，此表只记关联（path + 挂载顺序）。
            -- path 而非 document_id 作外键：文件被 delete 时不级联清理关联,
            -- 列表时 LEFT JOIN documents 取不到全文则跳过（已删除文档）。 
            CREATE TABLE IF NOT EXISTS conversation_documents (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT    NOT NULL,
                path            TEXT    NOT NULL,           -- 对应 documents.path
                mounted_at      INTEGER NOT NULL,            -- unix ms（排序用）
                UNIQUE(conversation_id, path)
            );
            CREATE INDEX IF NOT EXISTS idx_conv_docs_conv ON conversation_documents(conversation_id);
            "#,
        )?;
        Ok(())
    }

    /// 插入或替换一篇文档（按 path 去重，已存在则更新全文）。
    /// 返回文档 id。
    ///
    /// **只写主行，不建 FTS5 索引**（毫秒级）。但会先用旧值删除旧 FTS5 索引
    /// （external content 模式 'delete' 需原文，保证一致性）。索引由调用方异步调
    /// [`index_document`] 构建。INSERT/UPDATE 均置 indexed_at=0（未索引），
    /// 索引完成后由 index_document 置为时间戳。
    pub fn upsert_document(&self, row: DocumentRow) -> MemoryResult<String> {
        let conn = self.lock();
        // 若已存在同 path 旧记录，先用旧值删 FTS5 索引（external content 'delete'
        // 需原文，否则索引与新文本不一致）。旧记录可能未索引（indexed_at=0），
        // 此时 FTS5 无对应行，'delete' 会报错——用 optional + ok() 容错。
        let old: Option<(i64, String, String)> = conn.query_row(
            "SELECT rowid, name, text FROM documents WHERE path = ?", params![row.path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional()?;
        if let Some((rowid, name, text)) = old {
            conn.execute(
                "INSERT INTO documents_fts(documents_fts, rowid, name, text)
                 VALUES('delete', ?, ?, ?)",
                params![rowid, name, text],
            ).ok(); // 旧索引不存在不算错
        }
        conn.execute(
            "INSERT INTO documents(id, path, name, format, text, char_count, created_at, indexed_at, folder_path, source_conv_id)
             VALUES(?, ?, ?, ?, ?, ?, ?, 0, ?, ?)
             ON CONFLICT(path) DO UPDATE SET
                name=excluded.name, format=excluded.format, text=excluded.text,
                char_count=excluded.char_count, created_at=excluded.created_at,
                indexed_at=0,
                folder_path=COALESCE(excluded.folder_path, documents.folder_path),
                source_conv_id=COALESCE(excluded.source_conv_id, documents.source_conv_id)",
            params![row.id, row.path, row.name, row.format, row.text, row.char_count, row.created_at, row.folder_path, row.source_conv_id],
        )?;
        Ok(row.id)
    }

    /// 异步构建文档的 FTS5 索引（jieba 分词全文）。应在 upsert_document 后由
    /// spawn_blocking 调用，不阻塞 ingest 返回。
    ///
    /// 用独立连接（不抢主连接 Mutex 锁）：WAL 模式下与主连接并发安全。
    /// 流程：删旧索引（如有）→ 读全文 → INSERT 到 documents_fts → 置 indexed_at。
    ///
    /// 内存库（db_path=None）退化为在主连接上同步建索引（测试场景可接受）。
    /// 大全文（1M+ 字符）jieba 分词可能耗时数分钟，期间该文档搜不到（indexed_at=0），
    /// 其他功能不受影响。
    pub fn index_document(&self, id: &str) -> MemoryResult<()> {
        // 独立连接：文件库走后台索引路径；内存库回退到主连接（测试用）。
        let own_conn = match self.open_indexer_connection() {
            Some(Ok(c)) => Some(c),
            Some(Err(e)) => return Err(e),
            None => None, // 内存库
        };
        let result = if let Some(conn) = own_conn {
            Self::index_document_on(&conn, id)
        } else {
            let conn = self.lock();
            Self::index_document_on(&conn, id)
        };
        result
    }

    /// 在指定连接上构建文档 FTS5 索引（内部辅助）。
    fn index_document_on(conn: &Connection, id: &str) -> MemoryResult<()> {
        let tx = conn.unchecked_transaction()?;
        // 取 rowid + 全文（未找到则静默返回——文档可能已被删）。
        let row: Option<(i64, String, String)> = tx.query_row(
            "SELECT rowid, name, text FROM documents WHERE id = ?", params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional()?;
        let Some((rowid, name, text)) = row else {
            return Ok(());
        };
        // 旧索引已在 upsert_document 时用旧值删除，这里只建新索引。
        // （首次 INSERT 无旧记录，upsert 不删——正确；upsert 更新时 upsert 用旧值
        //   删旧索引——正确。index_document 不再重复删，避免用新文本 'delete' 破坏。）
        // 建新索引（jieba 分词全文，耗时操作）。
        tx.execute(
            "INSERT INTO documents_fts(rowid, name, text) VALUES(?, ?, ?)",
            params![rowid, name, text],
        )?;
        // 标记已索引。
        tx.execute(
            "UPDATE documents SET indexed_at = ? WHERE id = ?",
            params![crate::now_ms(), id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 按 path 查文档 id（用于判断是否已入库）。
    pub fn document_id_by_path(&self, path: &str) -> MemoryResult<Option<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT id FROM documents WHERE path = ?")?;
        let mut rows = stmt.query(params![path])?;
        match rows.next()? {
            Some(r) => Ok(Some(r.get(0)?)),
            None => Ok(None),
        }
    }

    /// 列出全部文档（不含全文，仅供 list_documents 工具展示）。
    /// 返回 (id, path, name, format, char_count, created_at, folder_path)。
    pub fn list_documents(&self) -> MemoryResult<Vec<(String, String, String, String, u32, i64, Option<String>)>> {
        let conn = self.lock();
        // 包含 skill body（skill-md）与三类资源（skill-resource：references/assets/scripts）：
        // list_documents_tool 会再按 allowed_paths（doc_paths_set）过滤，
        // 只有本会话激活的 skill 文档才会返回给模型，不会泄露未激活 skill。
        // 过去排除 skill-md 是因为模型拿不到 body id、preamble 却叫它“先 list 找”，
        // 形成断链——现已修（references 入库 + body 进 list 统一发现路径）。
        let mut stmt = conn.prepare(
            "SELECT id, path, name, format, char_count, created_at, folder_path FROM documents ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)? as u32,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 按 id 读全文（可分页，offset/limit 按字符数）。
    /// offset/limit 为 None 时返回全文。
    pub fn read_document(
        &self,
        id: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> MemoryResult<Option<(String, String, String, String, u32)>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT path, name, format, text, char_count FROM documents WHERE id = ?")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(r) => {
                let path: String = r.get(0)?;
                let name: String = r.get(1)?;
                let format: String = r.get(2)?;
                let text: String = r.get(3)?;
                let char_count: u32 = r.get(4)?;
                let sliced = match (offset, limit) {
                    (Some(off), Some(lim)) => text.chars().skip(off).take(lim).collect(),
                    (Some(off), None) => text.chars().skip(off).collect(),
                    (None, Some(lim)) => text.chars().take(lim).collect(),
                    (None, None) => text,
                };
                Ok(Some((path, name, format, sliced, char_count)))
            }
            None => Ok(None),
        }
    }

    /// 按 path 读全文元数据（不分页，send_message 拼接 context_texts 用）。
    /// 返回 (name, format, text, char_count)。
    pub fn read_document_by_path(
        &self,
        path: &str,
    ) -> MemoryResult<Option<(String, String, String, String, u32)>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, format, text, char_count FROM documents WHERE path = ?")?;
        let mut rows = stmt.query(params![path])?;
        match rows.next()? {
            Some(r) => Ok(Some((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)? as u32,
            ))),
            None => Ok(None),
        }
    }

    /// FTS5 关键词搜索（jieba 分词 + BM25 排序 + snippet 高亮）。
    /// query 由 agent 工具提供（模型把用户口语化意图转成关键词，非用户原话）。
    /// FTS5 关键词搜索（jieba 分词 + BM25 排序 + snippet 高亮）。
    /// 只返回已索引文档（indexed_at>0，见 [`index_document`]）。
    /// 未索引完的文档（异步索引进行中）不在结果内。
    /// FTS5 关键词搜索（jieba 分词 + BM25 排序 + snippet 高亮）。
    /// 只返回已索引文档（indexed_at>0，见 [`index_document`]）。
    /// 未索引完的文档（异步索引进行中）不在结果内。
    pub fn search_documents(&self, query: &str, limit: usize) -> MemoryResult<Vec<SearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        // JOIN documents 拿元数据，FTS5 表提供排序 + snippet。
        // bm25() 越小越相关（负数），ORDER BY rank 升序。
        // snippet(documents_fts, 1, ...) 列序号 1 = text 列。
        // d.indexed_at>0 过滤未索引文档（异步索引未完成时搜不到）。
        let mut stmt = conn.prepare(
            "SELECT d.id, d.path, d.name, d.format,
                    snippet(documents_fts, 1, '【', '】', '…', 16) AS snip,
                    bm25(documents_fts) AS rank
             FROM documents_fts
             JOIN documents d ON d.rowid = documents_fts.rowid
             WHERE documents_fts MATCH ? AND d.indexed_at > 0
             ORDER BY rank
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |r| {
            Ok(SearchHit {
                id: r.get(0)?,
                path: r.get(1)?,
                name: r.get(2)?,
                format: r.get(3)?,
                snippet: r.get(4)?,
                rank: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 按 path 删除文档。手动清 FTS5 索引（无触发器，external content 'delete' 需原文）。
    pub fn delete_document_by_path(&self, path: &str) -> MemoryResult<usize> {
        let conn = self.lock();
        // 先取 rowid+name+text 用于 FTS5 'delete' 命令。
        let row: Option<(i64, String, String)> = conn.query_row(
            "SELECT rowid, name, text FROM documents WHERE path = ?", params![path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional()?;
        let n = conn.execute("DELETE FROM documents WHERE path = ?", params![path])?;
        if let Some((rowid, name, text)) = row {
            conn.execute(
                "INSERT INTO documents_fts(documents_fts, rowid, name, text)
                 VALUES('delete', ?, ?, ?)",
                params![rowid, name, text],
            ).ok(); // 索引不存在不算错
        }
        Ok(n)
    }

    /// 挂载文档到会话（`@` 挂载持久化）。幂等：已挂载则不动。
    /// path 必须先存在于 documents 表（先 upsert_document 再挂载）。
    pub fn mount_document(&self, conversation_id: &str, path: &str) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT OR IGNORE INTO conversation_documents(conversation_id, path, mounted_at)
             VALUES (?, ?, ?)",
            params![conversation_id, path, now_ms()],
        )?;
        Ok(())
    }

    /// 卸载会话下的某篇文档（`@` 移除挂载，不删 documents 全文）。
    pub fn unmount_document(&self, conversation_id: &str, path: &str) -> MemoryResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM conversation_documents WHERE conversation_id = ? AND path = ?",
            params![conversation_id, path],
        )?)
    }

    /// 列出会话已挂载的文档（不含全文，按挂载时间排序）。
    /// LEFT JOIN documents：文件已删除（被 delete_document_by_path）则跳过。
    /// 返回 (path, name, format, char_count)。
    ///
    /// **排除 skill**：防御性过滤 format='skill-md'。skill 不走 mount_document
    /// （MentionMenu 的 skill 分支调 setSkillConversationEnabled，不写 conversation_documents），
    /// 正常不会出现在此表；但若历史数据或并发写入残留，此过滤兑底不出现在文件挂载列表。
    pub fn list_mounted_documents(
        &self,
        conversation_id: &str,
    ) -> MemoryResult<Vec<MountedDoc>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT d.path, d.name, d.format, d.char_count, cd.mounted_at
             FROM conversation_documents cd
             LEFT JOIN documents d ON d.path = cd.path
             WHERE cd.conversation_id = ? AND (d.format IS NULL OR d.format != 'skill-md')
             ORDER BY cd.mounted_at ASC",
        )?;
        let rows = stmt
            .query_map(params![conversation_id], |row| {
                Ok(MountedDoc {
                    path: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    format: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    char_count: row.get::<_, Option<u32>>(3)?.unwrap_or(0),
                    mounted_at: row.get::<_, i64>(4)?,
                })
            })?
            .filter(|r| {
                // 跳过 documents 表查不到的（文件已删除）—— path 为空表示 JOIN 失败。
                r.as_ref().map(|d| !d.path.is_empty()).unwrap_or(false)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 清空会话的全部挂载（删会话时调用）。
    pub fn clear_mounted_documents(&self, conversation_id: &str) -> MemoryResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM conversation_documents WHERE conversation_id = ?",
            params![conversation_id],
        )?)
    }

    /// 清除所有会话对某篇文档的挂载（删文档时调用）。
    pub fn clear_mounted_documents_by_path(&self, path: &str) -> MemoryResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "DELETE FROM conversation_documents WHERE path = ?",
            params![path],
        )?)
    }

    // ───────── 文件夹操作（CONVERSATION-SCOPE.md §3.4）─────────
    // 独立 folders 表 + documents.folder_path 双轨（决策 19 修订）：
    // folders 表持久化空文件夹；documents.folder_path 隐式推导有文件的文件夹。
    // 文件夹树 = 两者 UNION 去重。

    /// 新建空文件夹（持久化到 folders 表）。
    /// path 如 "/曾国藩专题" 或 "/曾国藩专题/书信集"。自动推导 parent_path/name。
    /// 已存在则忽略（幂等）。返回是否实际创建。
    pub fn create_folder(&self, path: &str) -> MemoryResult<bool> {
        let conn = self.lock();
        let path = normalize_folder_path(path);
        if path.is_empty() {
            return Err(MemoryError::Invalid("文件夹路径不能为空".into()));
        }
        // 拆分出 name 和 parent_path：/A/B → name=B, parent=/A；/A → name=A, parent=NULL
        let (name, parent_path) = split_folder_path(&path);
        let created = now_ms();
        let n = conn.execute(
            "INSERT OR IGNORE INTO folders(path, parent_path, name, created_at) VALUES (?, ?, ?, ?)",
            params![path, parent_path, name, created],
        )?;
        Ok(n > 0)
    }

    /// 列出所有文件夹路径（folders 表 ∪ DISTINCT documents.folder_path，去重）。
    /// 返回如 ["/曾国藩专题", "/方法论", "/Inbox"]。
    ///
    /// 注意：此方法返回扁平路径列表，仅用于内部（如激活集 resolve）。
    /// 前端展示文件夹树请用 [`list_folder_tree`]（后端已构建好层级）。
    pub fn list_folders(&self) -> MemoryResult<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT path FROM folders\n\
             \tWHERE path IS NOT NULL AND path != ''\n\
             \tUNION\n\
             \tSELECT DISTINCT folder_path FROM documents WHERE folder_path IS NOT NULL AND folder_path != '' AND folder_path NOT LIKE '/Skills/%'\n\
             \tORDER BY path",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 列出文件夹树（后端构建嵌套层级 + 排序）。
    ///
    /// 规则：
    /// - 从 DISTINCT folder_path 按 `/` 拆分构建树
    /// - `Inbox` 置顶（会话上传默认落点，最常访问）
    /// - 其余按名 `numeric` 排序（"曾国藩2" 排在 "曾国藩10" 前）
    /// - 子文件夹递归同样排序
    pub fn list_folder_tree(&self) -> MemoryResult<Vec<FolderNode>> {
        let paths = self.list_folders()?;
        Ok(build_folder_tree(&paths))
    }

    /// 列出指定文件夹下的文件（仅直接子文件，不递归子文件夹）。
    /// folder = "/" 或 None 表示根目录散文件（folder_path IS NULL）。
    /// 用于 Library 右栏选中文件夹时展示。
    pub fn list_documents_by_folder(&self, folder: Option<&str>) -> MemoryResult<Vec<(String, String, String, String, u32, i64)>> {
        let conn = self.lock();
        // 统一用一个闭包避免 if/else 产生两个闭包类型（E0308）。
        let is_root = folder.is_none() || folder == Some("/");
        let sql = if is_root {
            "SELECT id, path, name, format, char_count, created_at FROM documents WHERE folder_path IS NULL AND format != 'skill-md' ORDER BY name"
        } else {
            "SELECT id, path, name, format, char_count, created_at FROM documents WHERE folder_path = ? AND format != 'skill-md' ORDER BY name"
        };
        let mut stmt = conn.prepare(sql)?;
        let map_fn = |r: &rusqlite::Row| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)? as u32,
                r.get::<_, i64>(5)?,
            ))
        };
        let rows = if is_root {
            stmt.query_map([], map_fn)?
        } else {
            stmt.query_map(params![folder.unwrap()], map_fn)?
        };
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 移动单个文件到目标文件夹。target_folder=None 表示根目录散文件。
    pub fn move_document(&self, path: &str, target_folder: Option<&str>) -> MemoryResult<usize> {
        let conn = self.lock();
        Ok(conn.execute(
            "UPDATE documents SET folder_path = ? WHERE path = ?",
            params![target_folder, path],
        )?)
    }

    /// 重命名文件夹：同时更新 folders 表 + documents.folder_path。
    /// old_path 如 "/曾国藩专题"，new_path 如 "/曾公研究"。
    /// 会递归处理子文件夹（如 /曾国藩专题/书信集 → /曾公研究/书信集）。
    /// 返回受影响的文件数（documents 行数）。
    pub fn rename_folder(&self, old_path: &str, new_path: &str) -> MemoryResult<usize> {
        let old_path = normalize_folder_path(old_path);
        let new_path = normalize_folder_path(new_path);
        if old_path.is_empty() || new_path.is_empty() || old_path == new_path {
            return Err(MemoryError::Invalid("无效的重命名参数".into()));
        }
        let conn = self.lock();
        // 1) 更新 documents.folder_path（原逻辑）
        let n_docs = conn.execute(
            "UPDATE documents SET folder_path = 
                CASE 
                    WHEN folder_path = ?1 THEN ?2
                    WHEN folder_path LIKE ?3 THEN ?2 || substr(folder_path, length(?1) + 1)
                    ELSE folder_path
                END
             WHERE folder_path = ?1 OR folder_path LIKE ?3",
            params![old_path, new_path, format!("{old_path}/%")],
        )?;
        // 2) 更新 folders 表：重命名文件夹本身 + 所有子文件夹的 path/parent_path/name
        //    SQLite 的 UPDATE 不允许在 SET 里引用其他行的值做前缀替换，故逐行更新。
        let prefix = format!("{old_path}/%");
        let to_rename: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT path FROM folders WHERE path = ? OR path LIKE ?",
            )?;
            let rows = stmt.query_map(params![old_path, prefix], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            out
        };
        for p in to_rename {
            let new_p = if p == old_path {
                new_path.clone()
            } else {
                // p = old_path + "/sub..." → new_path + "/sub..."
                format!("{new_path}{}", &p[old_path.len()..])
            };
            let (name, parent_path) = split_folder_path(&new_p);
            conn.execute(
                "UPDATE folders SET path = ?, parent_path = ?, name = ? WHERE path = ?",
                params![new_p, parent_path, name, p],
            )?;
        }
        Ok(n_docs)
    }

    /// 删除文件夹：删除 folders 表中该文件夹及其子文件夹记录 + 删除 documents 中该文件夹下所有文件。
    /// 每个文件走 delete_document_by_path（清 documents 行 + FTS5 索引 + conversation_documents 关联）。
    /// 返回删除的文件数。folder 如 "/曾国藩专题"。
    pub fn delete_folder(&self, folder: &str) -> MemoryResult<usize> {
        let folder = normalize_folder_path(folder);
        // 先查出该文件夹下所有文件的 path（含子文件夹），再逐个删。
        // 不能直接 DELETE FROM documents WHERE folder_path LIKE '/folder%' ——
        // 那样不会清 FTS5 索引（无触发器）和 conversation_documents。
        let paths: Vec<String> = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT path FROM documents WHERE folder_path = ? OR folder_path LIKE ?",
            )?;
            let prefix = format!("{folder}/%");
            let rows = stmt.query_map(params![folder, prefix], |r| r.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        // 删除 folders 表中该文件夹及其子文件夹记录（空文件夹持久化，必须显式删）
        {
            let conn = self.lock();
            let prefix = format!("{folder}/%");
            conn.execute(
                "DELETE FROM folders WHERE path = ? OR path LIKE ?",
                params![folder, prefix],
            )?;
        }
        let mut count = 0;
        for p in paths {
            count += self.delete_document_by_path(&p)?;
        }
        Ok(count)
    }

    // ───────── 会话激活集（CONVERSATION-SCOPE.md §2.2）─────────
    // 激活集 = folders（文件夹路径，含子目录递归）+ documents（单文件 path，@触发）
    //          + sources（数据源名）。
    // folders/sources 存 conversations.active_folders/active_sources（JSON）；
    // documents 复用 conversation_documents 表（向后兼容 @挂载）。

    /// 读取会话激活集的文件夹部分（JSON 数组，如 ["/曾国藩专题", "/方法论"]）。
    /// None 表示空激活集（默认）。
    pub fn get_active_folders(&self, conversation_id: &str) -> MemoryResult<Vec<String>> {
        let conn = self.lock();
        let json: Option<String> = conn
            .query_row(
                "SELECT active_folders FROM conversations WHERE id = ?",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(parse_json_str_array(json.as_deref()))
    }

    /// 读取会话激活集的数据源部分（JSON 数组，如 ["ontology", "mydb"]）。
    pub fn get_active_sources(&self, conversation_id: &str) -> MemoryResult<Vec<String>> {
        let conn = self.lock();
        let json: Option<String> = conn
            .query_row(
                "SELECT active_sources FROM conversations WHERE id = ?",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(parse_json_str_array(json.as_deref()))
    }

    /// 设置会话激活集的文件夹部分。传入空 Vec 等价于清空（存 "[]"）。
    pub fn set_active_folders(&self, conversation_id: &str, folders: &[String]) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET active_folders = ? WHERE id = ?",
            params![serde_json::to_string(folders).map_err(|e| MemoryError::Invalid(format!("active_scope serde: {e}")))?, conversation_id],
        )?;
        Ok(())
    }

    /// 设置会话激活集的数据源部分。
    pub fn set_active_sources(&self, conversation_id: &str, sources: &[String]) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET active_sources = ? WHERE id = ?",
            params![serde_json::to_string(sources).map_err(|e| MemoryError::Invalid(format!("active_scope serde: {e}")))?, conversation_id],
        )?;
        Ok(())
    }

    // ── 本体引用（决策：会话页面 @OntologyName 引用本体）──
    //
    // 与 folders/sources 同表存（conversations.active_ontologies，JSON 数组），
    // 但语义不同：存的是 ontology api_name（如 ["SupplyChain"]），不是文件路径。
    //
    // 不需要 resolve_* 方法——agent 的只读工具（describe_ontology 等）直接用
    // api_name 查 store，不需展开成 doc_paths。会话激活集仅用于：
    //   1. chat.rs 注入 <mounted-ontologies> 注脚（user message 尾部）
    //   2. 前端显示当前会话引用了哪些本体

    /// 读取会话激活集的本体部分（JSON 数组，如 ["SupplyChain", "Sales"]）。
    /// 存的是 ontology api_name。空 = 未引用任何本体。
    pub fn get_active_ontologies(&self, conversation_id: &str) -> MemoryResult<Vec<String>> {
        let conn = self.lock();
        let json: Option<String> = conn
            .query_row(
                "SELECT active_ontologies FROM conversations WHERE id = ?",
                params![conversation_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(parse_json_str_array(json.as_deref()))
    }

    /// 设置会话激活集的本体部分。传入空 Vec 等价于清空（存 "[]"）。
    pub fn set_active_ontologies(&self, conversation_id: &str, ontologies: &[String]) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET active_ontologies = ? WHERE id = ?",
            params![serde_json::to_string(ontologies).map_err(|e| MemoryError::Invalid(format!("active_scope serde: {e}")))?, conversation_id],
        )?;
        Ok(())
    }

    /// 解析会话激活集为完整的文档 path 列表（供 agent 工具过滤用）。
    /// = folders 下所有文件（含子目录递归）的 path ∪ documents 部分（conversation_documents）。
    /// 激活集为空 → 返回空 Vec（调用方据此不挂文档工具）。
    pub fn resolve_active_doc_paths(&self, conversation_id: &str) -> MemoryResult<Vec<String>> {
        let folders = self.get_active_folders(conversation_id)?;
        let conn = self.lock();
        let mut out = std::collections::HashSet::new();
        // folders 部分：每个文件夹下所有文件（含子目录递归）。
        for folder in &folders {
            let prefix = format!("{folder}/%");
            let mut stmt = conn.prepare(
                "SELECT path FROM documents WHERE folder_path = ? OR folder_path LIKE ?",
            )?;
            let rows = stmt.query_map(params![folder, prefix], |r| r.get::<_, String>(0))?;
            for p in rows.flatten() {
                out.insert(p);
            }
        }
        // documents 部分：conversation_documents 表里该会话的 path。
        {
            let mut stmt = conn.prepare(
                "SELECT path FROM conversation_documents WHERE conversation_id = ?",
            )?;
            let rows = stmt.query_map(params![conversation_id], |r| r.get::<_, String>(0))?;
            for p in rows.flatten() {
                out.insert(p);
            }
        }
        Ok(out.into_iter().collect())
    }
}

/// 解析 JSON 字符串数组（如 ["/曾国藩专题", "/方法论"]）为 Vec<String>。
/// None/空串/解析失败 → 空 Vec。用于 active_folders/active_sources 列。
fn parse_json_str_array(json: Option<&str>) -> Vec<String> {
    let Some(s) = json else { return Vec::new(); };
    if s.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
}

/// 生成文档 id（UUID v4）。
pub fn new_document_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 当前时间戳（unix ms）。
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 校验路径：文档存储按绝对路径去重，相对路径转绝对。
pub fn canonicalize_path(p: &str) -> String {
    Path::new(p)
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| p.to_string())
}

/// 文件夹树节点（后端构建层级，前端直接渲染）。
///
/// 文件夹由文件的 `folder_path` 隐式定义（无独立 folders 表）；本方法从
/// DISTINCT folder_path 构建嵌套树，Inbox 置顶，其余按名排序。
/// 把树构建放在后端是因为层级解析 + 排序是领域逻辑，不应散在前端。
#[derive(Debug, Clone)]
pub struct FolderNode {
    /// 当前层名字（不含路径前缀，如 "书信集"）。
    pub name: String,
    /// 完整路径（如 "/曾国藩专题/书信集"），根为 ""。
    pub path: String,
    /// 子文件夹（已排序）。
    pub children: Vec<FolderNode>,
}

/// 从扁平 folder_path 列表构建嵌套树（后端领域逻辑）。
///
/// Inbox 置顶，其余按名排序，子文件夹递归。
/// 抽出为模块级函数便于单元测试。
pub(crate) fn build_folder_tree(paths: &[String]) -> Vec<FolderNode> {
    let mut root: FolderNode = FolderNode {
        name: String::new(),
        path: String::new(),
        children: Vec::new(),
    };
    for p in paths {
        let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
        let mut cur = &mut root;
        let mut acc = String::new();
        for part in parts {
            acc.push('/');
            acc.push_str(part);
            let idx = cur.children.iter().position(|c| c.name == part);
            let i = match idx {
                Some(i) => i,
                None => {
                    cur.children.push(FolderNode {
                        name: part.to_string(),
                        path: acc.clone(),
                        children: Vec::new(),
                    });
                    cur.children.len() - 1
                }
            };
            cur = &mut cur.children[i];
        }
    }
    sort_folder_nodes(&mut root.children);
    root.children
}

/// 文件夹节点排序：Inbox 置顶，其余按名（CJK 下字符串比较近似 numeric）。
fn sort_folder_nodes(nodes: &mut [FolderNode]) {
    nodes.sort_by(|a, b| {
        if a.name == "Inbox" {
            return std::cmp::Ordering::Less;
        }
        if b.name == "Inbox" {
            return std::cmp::Ordering::Greater;
        }
        a.name.cmp(&b.name)
    });
    for n in nodes.iter_mut() {
        sort_folder_nodes(&mut n.children);
    }
}

/// 规范化文件夹路径：去掉首尾空白，保证以 `/` 开头（根目录返回空串）。
/// 如 "曾国藩专题" → "/曾国藩专题"，"/A/B" → "/A/B"，"" → ""。
pub fn normalize_folder_path(path: &str) -> String {
    let p = path.trim();
    if p.is_empty() {
        return String::new();
    }
    if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    }
}

/// 拆分文件夹路径为 (name, parent_path)。
/// 如 "/A/B" → ("B", Some("/A"))；"/A" → ("A", None)；"" → ("", None)。
/// 用于 folders 表插入时推导 name 和 parent_path。
pub(crate) fn split_folder_path(path: &str) -> (String, Option<String>) {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "/" {
        return (String::new(), None);
    }
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    match trimmed.rfind('/') {
        Some(idx) => {
            // /A/B → name=B (idx 后), parent=/A (含前导 /)
            let name = trimmed[idx + 1..].to_string();
            let parent = format!("/{}", &trimmed[..idx]);
            (name, Some(parent))
        }
        None => (trimmed.to_string(), None),
    }
}

// 避免 unused 警告（MemoryError 在方法签名里用了，但这里再确认）
const _: fn() = || {
    let _: MemoryError;
};
