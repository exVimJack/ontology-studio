//! 本体版本管理（设计方案：显式发布才留快照，回退切指针不新建版本）。
//!
//! 语义（w3c-effective-version-reads）：
//!   - `ontology_ttl_versions`：只在显式发布时写完整 TTL 快照（版本 = 里程碑）
//!   - `ontology_ttl.active_revision`：生效发布版本指针；NULL = 从未发布；
//!     **编辑不清空**（脏状态用 `workspace_dirty`）
//!   - `ontology_ttl.workspace_dirty`：工作区相对生效快照是否有未发布修改；
//!     内容写入置 1，publish/revert 置 0
//!   - 回退 = 切指针 + 覆盖工作区 + 清 dirty，**不新建版本、revision 不递增**
//!   - FIFO 清理（默认上限 20）**跳过 active_revision 指向的"已启用"版本**，
//!     避免指针悬空（发布超限时最旧版本被删但用户正启用着它）
//!   - changelog 与 versions 是两套独立递增序列：发布写 changelog 时把 versions
//!     的 revision 塞进 change_summary JSON（{"published_version": N}）建立映射
//!   - diff 接口约定：rev=0 = "当前工作区内容"（可能未发布），正数 = versions 表 revision

use super::*;
use rusqlite::OptionalExtension;

/// 版本上限默认值（可配置，存 ttl_settings 表）。
pub const VERSION_LIMIT_DEFAULT: u32 = 20;

/// 版本摘要（列表项，不含全文）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq)]
pub struct TtlVersionSummary {
    /// 发布版本号（每本体从 1 递增）
    #[specta(type = specta_typescript::Number)]
    pub revision: i64,
    /// 版本标签（如 "v1.0"；空则发布时自动生成 v0.0.N）
    pub label: String,
    /// 发布说明
    pub description: String,
    /// 发布时刻快照的 triple 数
    #[specta(type = specta_typescript::Number)]
    pub triple_count: u64,
    /// 发布时刻 vs 上一版本的修改点（TtlVersionDiff 序列化 JSON；首版为空对象 {}）
    #[serde(default)]
    pub diff_summary: String,
    /// 发布时刻（unix ms）
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
    /// 发布者（"agent" | "user"）
    pub author: String,
}

/// 元素修改（IRI 同但字段变了）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq)]
pub struct PropertyChange {
    /// 元素 IRI（原文形态，可能含双 #）
    pub iri: String,
    /// 修改前视图
    pub before: crate::ttl_edit::TtlPropertyEdit,
    /// 修改后视图
    pub after: crate::ttl_edit::TtlPropertyEdit,
}

/// 一条 rdfs:subClassOf 边（child ⊑ parent）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Eq, Hash)]
pub struct SubclassOfEdge {
    /// 子类 IRI（原文形态，可能含双 #）
    pub child: String,
    /// 父类 IRI
    pub parent: String,
}

/// 变更清单「逐条撤销」入参（相对生效快照对工作区做逆向补丁）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceDiffRevokeItem {
    /// 类增删改
    Class {
        /// add | remove | modify
        op: String,
        iri: String,
    },
    /// 属性/关系增删改
    Property {
        /// add | remove | modify
        op: String,
        iri: String,
    },
    /// 继承边增删
    Subclass {
        /// add | remove
        op: String,
        child: String,
        parent: String,
    },
    /// 本体头元信息修改（暂支持撤销为生效版 label/comment——若无生效版则拒绝）
    Meta {
        /// 恒为 modify
        op: String,
    },
}

/// 两版本结构化 diff（不逐行文本 diff）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq)]
pub struct TtlVersionDiff {
    /// 对比基准版本（0 = 当前工作区内容）
    #[specta(type = specta_typescript::Number)]
    pub rev_a: i64,
    /// 对比目标版本（0 = 当前工作区内容）
    #[specta(type = specta_typescript::Number)]
    pub rev_b: i64,
    /// B 有 A 无的类 IRI
    pub added_classes: Vec<String>,
    /// A 有 B 无的类 IRI
    pub removed_classes: Vec<String>,
    /// IRI 同但 label/comment 变了
    pub modified_classes: Vec<PropertyChange>,
    /// 新增属性
    pub added_properties: Vec<crate::ttl_edit::TtlPropertyEdit>,
    /// 删除属性
    pub removed_properties: Vec<crate::ttl_edit::TtlPropertyEdit>,
    /// IRI 同但 label/comment/domain/range（或 kind）变了
    pub modified_properties: Vec<PropertyChange>,
    /// B 有 A 无的 subClassOf 边
    #[serde(default)]
    pub added_subclass_of: Vec<SubclassOfEdge>,
    /// A 有 B 无的 subClassOf 边
    #[serde(default)]
    pub removed_subclass_of: Vec<SubclassOfEdge>,
    /// 本体头部元信息（本体自身 rdfs:label / rdfs:comment）有变化
    pub ontology_meta_changed: bool,
}

impl TtlStore {
    /// 发布当前内容为新版本（版本 = 里程碑）。
    ///
    /// 流程（单事务）：读当前全文 + triple_count → 分配新 revision → 写快照 →
    /// 设 active_revision = 新 revision → 写 changelog（change_summary 塞
    /// {"published_version": N} 建立 changelog↔versions 映射）→ FIFO 超限清理
    /// （跳过 active_revision 指向的版本）。
    ///
    /// 返回新分配的 revision 号。
    pub fn publish_version(
        &self,
        ontology_iri: &str,
        label: &str,
        description: &str,
        author: &str,
        conversation_id: Option<&str>,
    ) -> StoreResult<u32> {
        let conn = self.conn.lock().unwrap();
        // 本体必须存在（与 commit_change 同闸门）
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM ontology_ttl WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
                |_| Ok(()),
            )
            .is_ok();
        if !exists {
            return Err(StoreError::NotFound(format!(
                "本体 '{ontology_iri}' 未找到，无法发布版本"
            )));
        }
        let (ttl, triple_count): (String, i64) = conn.query_row(
            "SELECT ttl_content, triple_count FROM ontology_ttl WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let now = now_ms();

        conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> StoreResult<u32> {
            let next_rev: i64 = conn.query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM ontology_ttl_versions WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
                |r| r.get(0),
            )?;
            // 版本号默认三位数 v0.0.N（N = 发布序号）；用户显式填 label 则以其为准
            let effective_label = if label.trim().is_empty() {
                format!("v0.0.{next_rev}")
            } else {
                label.trim().to_string()
            };
            // 本次发布修改点：当前内容 vs 上一版本（首版无上一版本 → 空对象）
            let diff_summary = if next_rev > 1 {
                let prev_ttl = match conn.query_row(
                    "SELECT ttl_content FROM ontology_ttl_versions \
                     WHERE ontology_iri=?1 AND revision=?2",
                    rusqlite::params![ontology_iri, next_rev - 1],
                    |r| r.get::<_, String>(0),
                ) {
                    Ok(t) => t,
                    Err(rusqlite::Error::QueryReturnedNoRows) => String::new(),
                    Err(e) => return Err(e.into()),
                };
                let diff = diff_ttl_contents(&prev_ttl, &ttl, next_rev - 1, next_rev);
                serde_json::to_string(&diff)
                    .map_err(|e| StoreError::Other(anyhow::anyhow!("修改点序列化失败: {e}")))?
            } else {
                "{}".to_string()
            };
            conn.execute(
                "INSERT INTO ontology_ttl_versions \
                 (id, ontology_iri, revision, ttl_content, triple_count, label, description, diff_summary, created_at, author) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    new_id(),
                    ontology_iri,
                    next_rev,
                    ttl,
                    triple_count,
                    effective_label,
                    description,
                    diff_summary,
                    now,
                    author,
                ],
            )?;
            conn.execute(
                "UPDATE ontology_ttl SET active_revision=?2, updated_at=?3, workspace_dirty=0 \
                 WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri, next_rev, now],
            )?;
            // changelog：内联 INSERT（commit_change 会重锁 conn，这里已在事务内持锁）
            let title = format!("发布版本 {}", effective_label);
            let body = description.to_string();
            let change_summary = format!(r#"{{"published_version": {next_rev}}}"#);
            let cl_rev: i64 = conn.query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM ontology_ttl_changelog WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO ontology_ttl_changelog \
                 (id, ontology_iri, revision, title, body, change_summary, conversation_id, author, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    new_id(),
                    ontology_iri,
                    cl_rev,
                    title,
                    body,
                    change_summary,
                    conversation_id,
                    author,
                    now,
                ],
            )?;
            // FIFO 超限清理（跳过 active_revision 指向的版本）
            Self::fifo_trim_conn(&conn, ontology_iri, Self::get_version_limit_conn(&conn)?)?;
            Ok(next_rev as u32)
        })();
        match result {
            Ok(v) => {
                conn.execute("COMMIT", [])?;
                Ok(v)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(e)
            }
        }
    }

    /// 内容变更时置 `workspace_dirty=1`（保留 `active_revision`）。
    ///
    /// 挂点：import_ttl / append_ttl / 元素级编辑写回（surgery 走 import_ttl）。
    /// publish/revert 自行清 dirty，不调用本方法。
    pub fn mark_workspace_dirty(&self, ontology_iri: &str) -> StoreResult<()> {
        let conn = self.conn.lock().unwrap();
        Self::mark_workspace_dirty_conn(&conn, ontology_iri)
    }

    /// 无锁版（调用方已持 conn 锁时复用，避免 Mutex 重入）。
    pub(crate) fn mark_workspace_dirty_conn(
        conn: &Connection,
        ontology_iri: &str,
    ) -> StoreResult<()> {
        conn.execute(
            "UPDATE ontology_ttl SET workspace_dirty = 1 WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
        )?;
        Ok(())
    }

    /// 列出某本体版本历史（revision 降序，最新在前；不含全文）。
    ///
    /// **惰性补算修改点**：旧库迁移产生的空 diff_summary（revision>1）实时从
    /// TTL 快照计算并回填——旧包发布的版本没有修改点记录，但快照还在，随时可算。
    pub fn list_versions(&self, ontology_iri: &str) -> StoreResult<Vec<TtlVersionSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT revision, label, description, triple_count, diff_summary, created_at, author \
             FROM ontology_ttl_versions WHERE ontology_iri=?1 ORDER BY revision DESC",
        )?;
        let mut summaries: Vec<TtlVersionSummary> = stmt
            .query_map(rusqlite::params![ontology_iri], |r| {
                Ok(TtlVersionSummary {
                    revision: r.get(0)?,
                    label: r.get(1)?,
                    description: r.get(2)?,
                    triple_count: r.get::<_, i64>(3)? as u64,
                    diff_summary: r.get(4)?,
                    created_at: r.get(5)?,
                    author: r.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, rusqlite::Error>>()
            .map_err(StoreError::Sqlite)?;
        for s in summaries.iter_mut() {
            if s.revision > 1 && (s.diff_summary.is_empty() || s.diff_summary == "{}") {
                if let Some(filled) = Self::backfill_diff_summary_conn(
                    &conn, ontology_iri, s.revision,
                )? {
                    s.diff_summary = filled;
                }
            }
        }
        Ok(summaries)
    }

    /// 补算某版本的修改点（取 prev 快照 diff，回填 DB）。返回补算后的 JSON；无法补算返回 None。
    fn backfill_diff_summary_conn(
        conn: &Connection,
        ontology_iri: &str,
        revision: i64,
    ) -> StoreResult<Option<String>> {
        let ttl: String = conn.query_row(
            "SELECT ttl_content FROM ontology_ttl_versions WHERE ontology_iri=?1 AND revision=?2",
            rusqlite::params![ontology_iri, revision],
            |r| r.get(0),
        )?;
        let prev_ttl: Option<String> = match conn.query_row(
            "SELECT ttl_content FROM ontology_ttl_versions WHERE ontology_iri=?1 AND revision=?2",
            rusqlite::params![ontology_iri, revision - 1],
            |r| r.get(0),
        ) {
            Ok(t) => Some(t),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        let Some(prev_ttl) = prev_ttl else {
            return Ok(None);
        };
        let diff = diff_ttl_contents(&prev_ttl, &ttl, revision - 1, revision);
        let json = serde_json::to_string(&diff)
            .map_err(|e| StoreError::Other(anyhow::anyhow!("修改点序列化失败: {e}")))?;
        conn.execute(
            "UPDATE ontology_ttl_versions SET diff_summary=?1 WHERE ontology_iri=?2 AND revision=?3",
            rusqlite::params![json, ontology_iri, revision],
        )?;
        Ok(Some(json))
    }

    /// 取某版本完整 TTL（回退/对比用；revision 必须为正数，0 表示当前工作区请用 export_ttl）。
    pub fn get_version_ttl(&self, ontology_iri: &str, revision: i64) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        let ttl: String = conn.query_row(
            "SELECT ttl_content FROM ontology_ttl_versions \
             WHERE ontology_iri=?1 AND revision=?2",
            rusqlite::params![ontology_iri, revision],
            |r| r.get(0),
        )?;
        Ok(ttl)
    }

    /// 回退到指定版本（切指针，不新建版本）。
    ///
    /// 取该版本 TTL → 覆盖当前内容（含 triple_count/updated_at）→
    /// 设 active_revision = 目标 revision → 写 changelog（标"回退到 revision N"）。
    /// 回退可逆：所有已发布版本仍在，随时可再切回去。
    pub fn revert_to_version(
        &self,
        ontology_iri: &str,
        revision: i64,
        author: &str,
        conversation_id: Option<&str>,
    ) -> StoreResult<()> {
        let conn = self.conn.lock().unwrap();
        let (ttl, triple_count): (String, i64) = conn.query_row(
            "SELECT ttl_content, triple_count FROM ontology_ttl_versions \
             WHERE ontology_iri=?1 AND revision=?2",
            rusqlite::params![ontology_iri, revision],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let now = now_ms();

        conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> StoreResult<()> {
            conn.execute(
                "UPDATE ontology_ttl SET ttl_content=?2, triple_count=?3, updated_at=?4, \
                 active_revision=?5, workspace_dirty=0 WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri, ttl, triple_count, now, revision],
            )?;
            // changelog：内联 INSERT（避免重锁）
            let title = format!("回退到版本 {revision}");
            let body = String::new();
            let change_summary = format!(r#"{{"reverted_to": {revision}}}"#);
            let cl_rev: i64 = conn.query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM ontology_ttl_changelog WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO ontology_ttl_changelog \
                 (id, ontology_iri, revision, title, body, change_summary, conversation_id, author, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    new_id(),
                    ontology_iri,
                    cl_rev,
                    title,
                    body,
                    change_summary,
                    conversation_id,
                    author,
                    now,
                ],
            )?;
            Ok(())
        })();
        match result {
            Ok(v) => {
                conn.execute("COMMIT", [])?;
                drop(conn);
                self.notify_change();
                Ok(v)
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(e)
            }
        }
    }

    /// 删除单个版本快照（revision 空洞保留；当前启用的版本拒绝删除防指针悬空）。
    /// 已发布快照删除后不可恢复；changelog 里的发布记录保留（映射指向已删版本）。
    pub fn delete_version(&self, ontology_iri: &str, revision: i64) -> StoreResult<()> {
        let conn = self.conn.lock().unwrap();
        let active = match conn.query_row(
            "SELECT active_revision FROM ontology_ttl WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        if active == Some(revision) {
            return Err(StoreError::Other(anyhow::anyhow!(
                "当前启用的版本（第 {revision} 次发布）不能删除——请先启用其他版本"
            )));
        }
        let affected = conn.execute(
            "DELETE FROM ontology_ttl_versions WHERE ontology_iri=?1 AND revision=?2",
            rusqlite::params![ontology_iri, revision],
        )?;
        if affected == 0 {
            return Err(StoreError::NotFound(format!(
                "版本（第 {revision} 次发布）不存在"
            )));
        }
        Ok(())
    }

    /// 取当前生效发布版本（NULL = 从未发布；编辑不清空）。
    pub fn get_active_revision(&self, ontology_iri: &str) -> StoreResult<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let active = match conn.query_row(
            "SELECT active_revision FROM ontology_ttl WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        Ok(active)
    }

    /// 两版本结构化 diff（rev=0 = 当前工作区内容，正数 = versions 表 revision）。
    ///
    /// 复用 ttl_edit 的原文 tokenize 解析（list_properties / list_class_metas）——
    /// 与前端 list_ttl_properties 同一坐标系，双 # IRI 形态一致。
    pub fn diff_versions(
        &self,
        ontology_iri: &str,
        rev_a: i64,
        rev_b: i64,
    ) -> StoreResult<TtlVersionDiff> {
        let ttl_a = self.resolve_ttl_for_diff(ontology_iri, rev_a)?;
        let ttl_b = self.resolve_ttl_for_diff(ontology_iri, rev_b)?;
        Ok(diff_ttl_contents(&ttl_a, &ttl_b, rev_a, rev_b))
    }

    fn resolve_ttl_for_diff(&self, ontology_iri: &str, rev: i64) -> StoreResult<String> {
        if rev <= 0 {
            self.export_ttl(ontology_iri)
        } else {
            self.get_version_ttl(ontology_iri, rev)
        }
    }

    /// 版本上限（默认 20；0/非法值回退默认）。
    pub fn get_version_limit(&self) -> StoreResult<u32> {
        let conn = self.conn.lock().unwrap();
        Self::get_version_limit_conn(&conn)
    }

    fn get_version_limit_conn(conn: &Connection) -> StoreResult<u32> {
        let v: Option<String> = conn
            .query_row(
                "SELECT value FROM ttl_settings WHERE key='version_limit'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.and_then(|s| s.parse().ok()).unwrap_or(VERSION_LIMIT_DEFAULT))
    }

    /// 设置版本上限（存 ttl_settings）。
    pub fn set_version_limit(&self, limit: u32) -> StoreResult<u32> {
        let limit = if limit == 0 { VERSION_LIMIT_DEFAULT } else { limit };
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ttl_settings (key, value) VALUES ('version_limit', ?1) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![limit.to_string()],
        )?;
        Ok(limit)
    }

    /// FIFO 超限清理（跳过 active_revision 指向的"已启用"版本，避免指针悬空）。
    pub(crate) fn fifo_trim_conn(
        conn: &Connection,
        ontology_iri: &str,
        limit: u32,
    ) -> StoreResult<()> {
        let active = match conn.query_row(
            "SELECT active_revision FROM ontology_ttl WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| r.get::<_, Option<i64>>(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(e.into()),
        };
        // 全部版本（升序 = 最旧在前）
        let mut stmt = conn.prepare(
            "SELECT revision FROM ontology_ttl_versions WHERE ontology_iri=?1 ORDER BY revision ASC",
        )?;
        let revs: Vec<i64> = stmt
            .query_map(rusqlite::params![ontology_iri], |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if revs.len() as u32 <= limit {
            return Ok(());
        }
        let mut to_delete = revs.len() as u32 - limit;
        for rev in &revs {
            if to_delete == 0 {
                break;
            }
            if Some(*rev) == active {
                // 正在启用的版本不删（它是最旧也不删，直到别的版本被启用）
                continue;
            }
            conn.execute(
                "DELETE FROM ontology_ttl_versions WHERE ontology_iri=?1 AND revision=?2",
                rusqlite::params![ontology_iri, rev],
            )?;
            to_delete -= 1;
        }
        Ok(())
    }
}

/// 类列表 → (iri → meta) 索引（同一输入生命周期的借用）。
fn class_index(
    list: &[crate::ttl_edit::TtlClassMeta],
) -> std::collections::HashMap<String, &crate::ttl_edit::TtlClassMeta> {
    list.iter().map(|c| (c.iri.clone(), c)).collect()
}

/// 属性列表 → (iri → edit) 索引。
fn prop_index(
    list: &[crate::ttl_edit::TtlPropertyEdit],
) -> std::collections::HashMap<String, &crate::ttl_edit::TtlPropertyEdit> {
    list.iter().map(|p| (p.iri.clone(), p)).collect()
}

/// 纯函数：两段 TTL 全文的结构化 diff（类/属性/subClassOf/本体头）。
fn diff_ttl_contents(ttl_a: &str, ttl_b: &str, rev_a: i64, rev_b: i64) -> TtlVersionDiff {
    let props_a = crate::ttl_edit::list_properties(ttl_a);
    let props_b = crate::ttl_edit::list_properties(ttl_b);
    let classes_a = crate::ttl_edit::list_class_metas(ttl_a);
    let classes_b = crate::ttl_edit::list_class_metas(ttl_b);

    let ca = class_index(&classes_a);
    let cb = class_index(&classes_b);
    let mut added_classes = Vec::new();
    let mut removed_classes = Vec::new();
    let mut modified_classes = Vec::new();
    for (iri, meta_b) in &cb {
        match ca.get(iri) {
            None => added_classes.push(iri.clone()),
            Some(meta_a) => {
                if meta_a.label != meta_b.label || meta_a.comment != meta_b.comment {
                    modified_classes.push(PropertyChange {
                        iri: iri.clone(),
                        before: class_to_prop_edit(meta_a),
                        after: class_to_prop_edit(meta_b),
                    });
                }
            }
        }
    }
    for (iri, _) in &ca {
        if !cb.contains_key(iri) {
            removed_classes.push(iri.clone());
        }
    }

    let pa = prop_index(&props_a);
    let pb = prop_index(&props_b);
    let mut added_properties = Vec::new();
    let mut removed_properties = Vec::new();
    let mut modified_properties = Vec::new();
    for (iri, prop_b) in &pb {
        match pa.get(iri) {
            None => added_properties.push((*prop_b).clone()),
            Some(prop_a) => {
                if prop_a != prop_b {
                    modified_properties.push(PropertyChange {
                        iri: iri.clone(),
                        before: (*prop_a).clone(),
                        after: (*prop_b).clone(),
                    });
                }
            }
        }
    }
    for (iri, prop_a) in &pa {
        if !pb.contains_key(iri) {
            removed_properties.push((*prop_a).clone());
        }
    }

    let edges_a: std::collections::HashSet<SubclassOfEdge> =
        crate::ttl_edit::list_subclass_of_edges(ttl_a)
            .into_iter()
            .map(|(child, parent)| SubclassOfEdge { child, parent })
            .collect();
    let edges_b: std::collections::HashSet<SubclassOfEdge> =
        crate::ttl_edit::list_subclass_of_edges(ttl_b)
            .into_iter()
            .map(|(child, parent)| SubclassOfEdge { child, parent })
            .collect();
    let mut added_subclass_of: Vec<SubclassOfEdge> = edges_b
        .difference(&edges_a)
        .cloned()
        .collect();
    let mut removed_subclass_of: Vec<SubclassOfEdge> = edges_a
        .difference(&edges_b)
        .cloned()
        .collect();
    added_subclass_of.sort_by(|a, b| (&a.child, &a.parent).cmp(&(&b.child, &b.parent)));
    removed_subclass_of.sort_by(|a, b| (&a.child, &a.parent).cmp(&(&b.child, &b.parent)));

    let ontology_meta_changed = ontology_label(ttl_a) != ontology_label(ttl_b)
        || ontology_comment(ttl_a) != ontology_comment(ttl_b);

    TtlVersionDiff {
        rev_a,
        rev_b,
        added_classes,
        removed_classes,
        modified_classes,
        added_properties,
        removed_properties,
        modified_properties,
        added_subclass_of,
        removed_subclass_of,
        ontology_meta_changed,
    }
}

/// TtlClassMeta → TtlPropertyEdit（PropertyChange.before/after 复用同一视图结构）。
fn class_to_prop_edit(c: &crate::ttl_edit::TtlClassMeta) -> crate::ttl_edit::TtlPropertyEdit {
    crate::ttl_edit::TtlPropertyEdit {
        iri: c.iri.clone(),
        kind: "class".to_string(),
        label: c.label.clone(),
        comment: c.comment.clone(),
        domain: String::new(),
        range: String::new(),
    }
}

/// 本体头部 rdfs:label（内存 oxigraph 查询；双 # 在本体 IRI 上罕见，失败返回空）。
pub(crate) fn ontology_label(ttl: &str) -> String {
    ontology_meta(ttl, "http://www.w3.org/2000/01/rdf-schema#label")
}

/// 本体头部 rdfs:comment。
fn ontology_comment(ttl: &str) -> String {
    ontology_meta(ttl, "http://www.w3.org/2000/01/rdf-schema#comment")
}

fn ontology_meta(ttl: &str, predicate: &str) -> String {
    let Ok(store) = parse_ttl_to_memory(ttl) else {
        return String::new();
    };
    let Ok(iri) = extract_ontology_iri(&store) else {
        return String::new();
    };
    let iri = iri.unwrap_or_default();
    if iri.is_empty() {
        return String::new();
    }
    run_sparql_solutions(
        &store,
        &format!("SELECT ?v WHERE {{ <{iri}> <{predicate}> ?v }} LIMIT 1"),
    )
    .into_iter()
    .next()
    .and_then(|row| row.get("v").map(|v| strip_term(&v).to_string()))
    .unwrap_or_default()
}

/// 从内存 oxigraph 提取 owl:Ontology IRI（失败返回 Ok(None)）。
fn extract_ontology_iri(store: &oxigraph::store::Store) -> StoreResult<Option<String>> {
    Ok(run_sparql_solutions(
        store,
        "SELECT ?o WHERE { ?o <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Ontology> . } LIMIT 1",
    )
    .into_iter()
    .next()
    .and_then(|row| row.get("o").map(|v| strip_iri_brackets(&v).to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ttl(label: &str, comment: &str, extra: &str) -> String {
        format!(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.com/ont#> .

<https://example.com/ont> a owl:Ontology ;
    rdfs:label "{label}" ;
    rdfs:comment "{comment}" .

:Policy a owl:Class ;
    rdfs:label "政策" ;
    rdfs:comment "采购政策" .

:hasPolicy a owl:ObjectProperty ;
    rdfs:domain :Policy ;
    rdfs:range :Policy .
{extra}"#
        )
    }

    #[test]
    fn diff_detects_class_property_and_meta_changes() {
        let a = sample_ttl("采购本体", "v1", "");
        let b = sample_ttl("采购本体v2", "v2", ":newProp a owl:DatatypeProperty ;\n    rdfs:label \"新属性\" .\n");
        let diff = diff_ttl_contents(&a, &b, 1, 2);
        assert!(diff.ontology_meta_changed, "本体头 label/comment 变化应检出");
        assert!(diff.added_classes.is_empty());
        assert_eq!(diff.modified_classes.len(), 0, "类未变");
        assert_eq!(diff.added_properties.len(), 1, "新增属性应检出");
        assert_eq!(diff.added_properties[0].iri, "https://example.com/ont#newProp");
        assert!(diff.removed_properties.is_empty());
    }

    #[test]
    fn diff_detects_class_label_change_and_removals() {
        let a = sample_ttl("采购本体", "v1", ":toRemove a owl:Class ;\n    rdfs:label \"将删除\" .\n");
        let b = sample_ttl(
            "采购本体",
            "v1",
            "PLACEHOLDER",
        )
        .replace(
            "PLACEHOLDER",
            "",
        )
        .replace(
            ":Policy a owl:Class ;\n    rdfs:label \"政策\" ;\n    rdfs:comment \"采购政策\" .",
            ":Policy a owl:Class ;\n    rdfs:label \"政策v2\" ;\n    rdfs:comment \"采购政策\" .",
        );
        let diff = diff_ttl_contents(&a, &b, 1, 2);
        assert_eq!(diff.removed_classes, vec!["https://example.com/ont#toRemove"]);
        assert_eq!(diff.modified_classes.len(), 1);
        assert_eq!(diff.modified_classes[0].iri, "https://example.com/ont#Policy");
        assert_eq!(diff.modified_classes[0].before.label, "政策");
        assert_eq!(diff.modified_classes[0].after.label, "政策v2");
    }

    #[test]
    fn diff_identical_ttl_is_empty() {
        let ttl = sample_ttl("采购本体", "v1", "");
        let diff = diff_ttl_contents(&ttl, &ttl, 5, 5);
        assert!(diff.added_classes.is_empty());
        assert!(diff.removed_classes.is_empty());
        assert!(diff.modified_classes.is_empty());
        assert!(diff.added_properties.is_empty());
        assert!(diff.modified_properties.is_empty());
        assert!(!diff.ontology_meta_changed);
    }
}
