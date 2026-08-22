//! Store：连接句柄 + export / preview_import / import 三个核心函数。
//!
//! 严格对齐 Gaia `ontology_service.py` 的三个函数语义：
//!   - `export_ontology`：从 DB 查询拼出 OntologyPayload（write-view JSON）
//!   - `preview_ontology_import`：dry-run，预测 create/skip/overwrite/fail +
//!     引用完整性 errors + 非阻塞 warnings（不写库）
//!   - `import_ontology`：DAG 顺序落库（Ontology → DataSource → Dataset →
//!     ObjectType+Property → Link → Action → Group），best-effort 部分失败

use std::sync::Mutex;

use rusqlite::Connection;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::data_type::is_valid_data_type;
use crate::error::{StoreError, StoreResult};
use crate::naming::{
    is_valid_dataset_api_name, is_valid_object_type_api_name, is_valid_property_api_name,
};
use crate::payload::*;
use crate::schema::init_schema;

/// 本体存储句柄。持有一个 SQLite 连接（多线程下用 Mutex 串行化）。
///
/// 与 `memory::Memory` 同构——桌面单进程，Tauri AppState 会包一层 Arc。
pub struct OntologyStore {
    conn: Mutex<Connection>,
    /// 落库变更回调（平台无关）：import/delete 成功后触发。
    /// 由装配层（src-tauri）注入，用于通知前端刷新查询。
    on_change: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl OntologyStore {
    /// 打开（或创建）位于 `path` 的库文件，并完成 schema 初始化。
    pub fn open(path: impl AsRef<std::path::Path>) -> StoreResult<Self> {
        let conn = Connection::open(path)?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            on_change: Mutex::new(None),
        })
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            on_change: Mutex::new(None),
        })
    }

    /// 注册落库变更回调（覆盖式）。
    pub fn set_on_change(&self, cb: Box<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().unwrap() = Some(cb);
    }

    /// 触发变更回调（若已注册）。在 import/delete 成功提交后调用。
    fn notify_change(&self) {
        if let Some(cb) = self.on_change.lock().unwrap().as_ref() {
            cb();
        }
    }

    // ════════════════════════════════════════════════════════════════
    // export
    // ════════════════════════════════════════════════════════════════

    /// 列出所有已存储的本体（轻量摘要，用于前端列表页）。
    pub fn list_ontologies(&self) -> StoreResult<Vec<OntologySummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, COALESCE(description, '') FROM ontologies ORDER BY api_name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(OntologySummary {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 列出某本体下的全部数据集（决策 10 修订：按本体隔离，不再全局返回）。
    ///
    /// 与 `export` 内嵌的 `datasets` 字段同源：导出时也只返回该本体声明的资产。
    /// 建模工作台的数据集独立视图调此方法（传具体本体的 api_name）。
    pub fn list_datasets(&self, ontology_api_name: &str) -> StoreResult<Vec<DatasetDef>> {
        let conn = self.conn.lock().unwrap();
        self.export_datasets(&conn, ontology_api_name)
    }

    /// 列出某本体下的全部数据源（决策 10 修订：按本体隔离，同数据集）。
    pub fn list_data_sources(&self, ontology_api_name: &str) -> StoreResult<Vec<DataSourceDef>> {
        let conn = self.conn.lock().unwrap();
        self.export_data_sources(&conn, ontology_api_name)
    }

    /// 硬删除本体及其全部子表（对齐 Gaia `hard_delete_ontology`，ADR-023）。
    ///
    /// 语义：物理 `DELETE FROM ontologies WHERE api_name=?`，依赖 DB `ON DELETE
    /// CASCADE` 级联清掉 object_types / properties / link_types / action_types /
    /// object_type_groups / object_type_group_members / datasets / data_sources
    /// （这些表都有 ontology_api_name 外键——决策 10 修订：dataset/data_source
    /// 按本体隔离，随本体级联删除）。`api_name` 释放，可重新导入。
    ///
    /// 返回 true=已删除；false=未找到该本体（幂等，不报错）。
    pub fn delete(&self, ontology_api_name: &str) -> StoreResult<bool> {
        let mut conn = self.conn.lock().unwrap();
        // foreign_keys 连接级开关，事务前显式开（CASCADE 生性依赖它；WAL 下重启
        // 连接可能重置，防御性 ensure）。
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let tx = conn.transaction()?;
        let affected = tx.execute(
            "DELETE FROM ontologies WHERE api_name=?1",
            rusqlite::params![ontology_api_name],
        )?;
        tx.commit()?;
        if affected > 0 {
            self.notify_change();
        }
        Ok(affected > 0)
    }

    // ════════════════════════════════════════════════════════════
    // 实体级删除（会话内 agent 工具用）：
    // import 只能 upsert，删除必须走这组方法。
    // 语义：物理删除、幂等（不存在返回 false）；删 OT 时连删
    // 字符串引用它的 link/action（无 FK 级联）；dataset/data_source
    // 被引用时拒绝删（先解绑再删）。
    // ════════════════════════════════════════════════════════════

    /// 删除单个 ObjectType（连带 properties、group 成员、引用它的 Link 和 Action）。
    /// 返回 (是否删到, 连删的链接数, 连删的动作数)。
    pub fn delete_object_type(
        &self,
        ontology_api_name: &str,
        ot_api_name: &str,
    ) -> StoreResult<(bool, usize, usize)> {
        let mut conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let tx = conn.transaction()?;
        let ont = self.get_ontology_row(&tx, ontology_api_name)?;
        let ot_id: Option<String> = tx
            .query_row(
                "SELECT id FROM object_types WHERE ontology_id=?1 AND api_name=?2",
                rusqlite::params![ont.id, ot_api_name],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })?;
        let Some(_) = ot_id else {
            return Ok((false, 0, 0));
        };
        // link/action 用 api_name 字符串引用 OT，无 FK 级联，显式连删
        let links_removed = tx.execute(
            "DELETE FROM link_types WHERE ontology_id=?1 \
             AND (source_object_type_api_name=?2 OR target_object_type_api_name=?2)",
            rusqlite::params![ont.id, ot_api_name],
        )?;
        let actions_removed = tx.execute(
            "DELETE FROM action_types WHERE ontology_id=?1 AND affected_object_type_api_name=?2",
            rusqlite::params![ont.id, ot_api_name],
        )?;
        // properties / group 成员由 FK ON DELETE CASCADE 自动清
        let removed = tx.execute(
            "DELETE FROM object_types WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont.id, ot_api_name],
        )? > 0;
        tx.commit()?;
        if removed {
            self.notify_change();
        }
        Ok((removed, links_removed, actions_removed))
    }

    /// 删除单个 LinkType（幂等）。返回是否删到。
    pub fn delete_link_type(
        &self,
        ontology_api_name: &str,
        link_api_name: &str,
    ) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let removed = conn.execute(
            "DELETE FROM link_types WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont.id, link_api_name],
        )? > 0;
        drop(conn);
        if removed {
            self.notify_change();
        }
        Ok(removed)
    }

    /// 删除单个 ActionType（幂等）。返回是否删到。
    pub fn delete_action_type(
        &self,
        ontology_api_name: &str,
        action_api_name: &str,
    ) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let removed = conn.execute(
            "DELETE FROM action_types WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont.id, action_api_name],
        )? > 0;
        drop(conn);
        if removed {
            self.notify_change();
        }
        Ok(removed)
    }

    /// 删除全局 Dataset。被 ObjectType（backing）或 Dataset（view 派生）
    /// 引用时拒绝，返回引用方名单。
    pub fn delete_dataset(&self, ontology_api_name: &str, dataset_api_name: &str) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let mut refs = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT o.api_name FROM object_types o \
                 WHERE o.backing_dataset_api_name=?1",
            )?;
            let names: Vec<String> = stmt
                .query_map(rusqlite::params![dataset_api_name], |r| r.get(0))?
                .filter_map(Result::ok)
                .collect();
            refs.extend(names.iter().map(|n| format!("ObjectType {n}")));
        }
        {
            let mut stmt = conn.prepare(
                "SELECT api_name FROM datasets WHERE ontology_api_name=?1 AND source_dataset_api_name=?2",
            )?;
            let names: Vec<String> = stmt
                .query_map(rusqlite::params![ontology_api_name, dataset_api_name], |r| r.get(0))?
                .filter_map(Result::ok)
                .collect();
            refs.extend(names.iter().map(|n| format!("Dataset {n}")));
        }
        if !refs.is_empty() {
            return Err(StoreError::ReferentialIntegrity(format!(
                "dataset '{dataset_api_name}' 被引用，先解除引用再删除：{}",
                refs.join(", ")
            )));
        }
        let removed = conn.execute(
            "DELETE FROM datasets WHERE ontology_api_name=?1 AND api_name=?2",
            rusqlite::params![ontology_api_name, dataset_api_name],
        )? > 0;
        drop(conn);
        if removed {
            self.notify_change();
        }
        Ok(removed)
    }

    /// 删除指定本体下的 DataSource。被同本体内 Dataset 引用时拒绝，返回引用方名单。
    pub fn delete_data_source(&self, ontology_api_name: &str, data_source_api_name: &str) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let refs: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT api_name FROM datasets WHERE ontology_api_name=?1 AND data_source_api_name=?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![ontology_api_name, data_source_api_name], |r| {
                r.get::<_, String>(0)
            })?;
            rows.filter_map(Result::ok).collect()
        };
        if !refs.is_empty() {
            return Err(StoreError::ReferentialIntegrity(format!(
                "data_source '{data_source_api_name}' 被引用，先解除引用再删除：{}",
                refs.join(", ")
            )));
        }
        let removed = conn.execute(
            "DELETE FROM data_sources WHERE ontology_api_name=?1 AND api_name=?2",
            rusqlite::params![ontology_api_name, data_source_api_name],
        )? > 0;
        drop(conn);
        if removed {
            self.notify_change();
        }
        Ok(removed)
    }

    // ════════════════════════════════════════════════════════════
    // 变更日志（git commit log 式）
    // ════════════════════════════════════════════════════════════

    /// 体积上限（chars）：title+body 合计 ≤500，整条 changelog ≤1K。
    /// 超出返回 InvalidApiName 复用为“参数非法”错误（避免新增 variant）。
    const MAX_TITLE_BODY_CHARS: usize = 500;
    const MAX_CHANGELOG_CHARS: usize = 1000;

    /// 提交一条本体变更日志（git commit message 式）。
    ///
    /// - `title`：一句话标题；`body`：设计说明正文。**title+body ≤500 chars**。
    /// - `change_summary`：机器可读 JSON（实体级 +/−/~ 摘要），不参与 500 限制但
    ///   整条记录（title+body+change_summary）≤1K chars。
    /// - `conversation_id`：来源会话（可空）。
    /// - revision 本体内自动递增（从 1 开始）。
    pub fn commit_change(
        &self,
        ontology_api_name: &str,
        title: &str,
        body: &str,
        change_summary: &str,
        conversation_id: Option<&str>,
        author: &str,
    ) -> StoreResult<u32> {
        let title_body = title.chars().count() + body.chars().count();
        if title_body > Self::MAX_TITLE_BODY_CHARS {
            return Err(StoreError::InvalidApiName {
                entity_kind: "changelog",
                api_name: format!("title+body={title_body}"),
                pattern: "title+body <= 500 chars",
            });
        }
        let total = title_body + change_summary.chars().count();
        if total > Self::MAX_CHANGELOG_CHARS {
            return Err(StoreError::InvalidApiName {
                entity_kind: "changelog",
                api_name: format!("total={total}"),
                pattern: "total <= 1000 chars",
            });
        }
        let conn = self.conn.lock().unwrap();
        // 本体内 revision 递增（不存在本体则拒）
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let next_rev: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM ontology_changelog WHERE ontology_api_name=?1",
                rusqlite::params![ont.api_name],
                |r| r.get(0),
            )?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO ontology_changelog (id, ontology_api_name, revision, title, body, change_summary, conversation_id, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                new_id(),
                ont.api_name,
                next_rev,
                title,
                body,
                change_summary,
                conversation_id,
                author,
                now
            ],
        )?;
        // changelog 写入不视为本体本身变更，不触发 notify_change（避免循环刷新）
        Ok(next_rev as u32)
    }

    /// 列出本体的变更日志（按 revision 倒序，最新在前）。
    pub fn list_changelog(
        &self,
        ontology_api_name: &str,
    ) -> StoreResult<Vec<OntologyChangelog>> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let mut stmt = conn.prepare(
            "SELECT revision, title, body, change_summary, conversation_id, author, created_at \
             FROM ontology_changelog WHERE ontology_api_name=?1 ORDER BY revision DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont.api_name], |r| {
            Ok(OntologyChangelog {
                revision: r.get::<_, i64>(0)? as u32,
                title: r.get(1)?,
                body: r.get(2)?,
                change_summary: r.get(3)?,
                conversation_id: r.get(4)?,
                author: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ══════════════════════════════════════════════════════════════════
    // 本体设计宪章（不变点：业务场景 / 本质 / 设计意图 / 补充说明）
    // ══════════════════════════════════════════════════════════════════

    /// 读取本体设计宪章（不变点）。
    ///
    /// 无 charter 行时返回空结构体（各字段空串）——业务上视为「尚未定义宪章」，
    /// 不报错。调用方（describe_ontology_summary / 工具层）据此决定是否提示建模者补充。
    /// `conn` 已持有锁的版本（内部调用，避免重复加锁）。
    fn get_charter_inner(&self, conn: &Connection, ontology_api_name: &str) -> StoreResult<OntologyCharter> {
        let row = conn.query_row(
            "SELECT business_scenario, business_essence, design_intent, invariants, updated_by, updated_at \
             FROM ontology_charter WHERE ontology_api_name=?1",
            rusqlite::params![ontology_api_name],
            |r| {
                Ok(OntologyCharter {
                    business_scenario: r.get(0)?,
                    business_essence: r.get(1)?,
                    design_intent: r.get(2)?,
                    invariants: r.get(3)?,
                    updated_by: r.get(4)?,
                    updated_at: r.get(5)?,
                })
            },
        );
        match row {
            Ok(c) => Ok(c),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(OntologyCharter::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// 读取本体设计宪章（公共接口，供 IPC 命令调用）。
    pub fn get_charter(&self, ontology_api_name: &str) -> StoreResult<OntologyCharter> {
        let conn = self.conn.lock().unwrap();
        // 先确认本体存在（不存在报 NotFound，与其它只读接口一致）
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        self.get_charter_inner(&conn, &ont.api_name)
    }

    /// 写入/更新本体设计宪章（不变点）。
    ///
    /// 由独立命令/工具调用，**不进 import 流程**——避免增量导入覆盖业务说明。
    /// `updated_by` 为 "agent" | "user"。
    ///
    /// 写后触发 `notify_change()`：charter 虽不影响实体定义，但会话内 agent 工具
    /// （set_ontology_charter）不经 IPC 命令层，前端 charter 面板的 query 缓存无法
    /// 感知变更；故统一走 on_change 回调发事件，让前端失效 charter 缓存（否则
    /// 会出现「已落库但 OntologyView 不刷新」的假象）。
    pub fn set_charter(
        &self,
        ontology_api_name: &str,
        charter: &OntologyCharter,
    ) -> StoreResult<()> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO ontology_charter (ontology_api_name, business_scenario, business_essence, design_intent, invariants, updated_by, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(ontology_api_name) DO UPDATE SET
                business_scenario = excluded.business_scenario,
                business_essence   = excluded.business_essence,
                design_intent      = excluded.design_intent,
                invariants         = excluded.invariants,
                updated_by         = excluded.updated_by,
                updated_at         = excluded.updated_at",
            rusqlite::params![
                ont.api_name,
                charter.business_scenario,
                charter.business_essence,
                charter.design_intent,
                charter.invariants,
                if charter.updated_by.is_empty() { "agent" } else { &charter.updated_by },
                now
            ],
        )?;
        self.notify_change();
        Ok(())
    }

    /// 导出本体为 OntologyPayload（对齐 Gaia `export_ontology`）。
    ///
    /// write-view：properties 嵌在 object_types 下，links 也嵌在 object_types 下
    /// （source 侧持有），action_types 独立列出。
    pub fn export(&self, ontology_api_name: &str) -> StoreResult<OntologyPayload> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;

        let object_types = self.export_object_types(&conn, &ont.id)?;
        let action_types = self.export_action_types(&conn, &ont.id)?;
        let datasets = self.export_datasets(&conn, &ont.api_name)?;
        let data_sources = self.export_data_sources(&conn, &ont.api_name)?;
        let object_type_groups = self.export_object_type_groups(&conn, &ont.id)?;

        Ok(OntologyPayload {
            api_name: ont.api_name,
            display_name: ont.display_name,
            description: ont.description,
            object_types,
            action_types,
            datasets,
            data_sources,
            object_type_groups,
        })
    }

    // ════════════════════════════════════════════════════════════════
    // preview_import
    // ════════════════════════════════════════════════════════════════

    /// 预演导入（对齐 Gaia `preview_ontology_import`）。
    ///
    /// 静态检查：不写库、不调外部系统。返回 per-entity 预测 + 引用完整性
    /// errors（阻断性）+ warnings（非阻断：占位符凭据、缺 backing_column 等）。
    pub fn preview_import(&self, req: &ImportRequest) -> StoreResult<ImportPreview> {
        let conn = self.conn.lock().unwrap();
        let payload = &req.payload;
        let overwrite = &req.overwrite_object_types;
        let overwrite_ds = &req.overwrite_data_sources;

        // ── 1. 本体自身 ──
        let ont_exists = self.ontology_exists(&conn, &payload.api_name)?;
        let ontology_status = if ont_exists { "skip" } else { "create" };

        // ── 2. ObjectType 预测 ──
        let existing_ots = self.list_existing_ot_api_names(&conn, &payload.api_name)?;
        let mut object_types = Vec::new();
        for ot in &payload.object_types {
            validate_ot(ot)?;
            let status = if !existing_ots.contains(&ot.api_name) {
                "create"
            } else if overwrite.contains(&ot.api_name) {
                "overwrite"
            } else {
                "skip"
            };
            object_types.push(ImportPreviewItem {
                api_name: ot.api_name.clone(),
                status: status.to_string(),
                reason: if status == "skip" {
                    "同名已存在且未列入 overwrite_object_types".to_string()
                } else {
                    String::new()
                },
            });
        }

        // ── 3. 引用完整性检查（阻断性 errors）──
        let mut errors = Vec::new();
        let mut ot_api_names: Vec<&str> = payload.object_types.iter().map(|o| o.api_name.as_str()).collect();
        // 已存在但未覆写的 OT 也参与引用解析（它们的 link target 可能指向已存在的 OT）
        for e in &existing_ots {
            if !ot_api_names.iter().any(|a| *a == e) {
                ot_api_names.push(e);
            }
        }

        // Link target 必须在本体已知 OT 中
        for ot in &payload.object_types {
            for link in &ot.links {
                if !ot_api_names.contains(&link.target_object_type_api_name.as_str()) {
                    errors.push(format!(
                        "LinkType '{}.{}' 的 target_object_type_api_name '{}' 在本体内不存在",
                        ot.api_name, link.api_name, link.target_object_type_api_name
                    ));
                }
            }
        }
        // Action affected_object_type 必须存在
        for act in &payload.action_types {
            if !ot_api_names.contains(&act.affected_object_type_api_name.as_str()) {
                errors.push(format!(
                    "ActionType '{}' 的 affected_object_type_api_name '{}' 在本体内不存在",
                    act.api_name, act.affected_object_type_api_name
                ));
            }
        }
        // 每个 OT 有且仅一个 is_primary_key
        for ot in &payload.object_types {
            let pk_count = ot.properties.iter().filter(|p| p.is_primary_key.unwrap_or(false)).count();
            if pk_count != 1 {
                errors.push(format!(
                    "ObjectType '{}' 必须有且仅一个 is_primary_key=true 的属性（当前 {} 个）",
                    ot.api_name, pk_count
                ));
            }
        }

        // ── 4. 非阻断 warnings ──
        let mut warnings = Vec::new();
        for ds in &payload.data_sources {
            if ds.connector_config.get("password").and_then(|v| v.as_str()).is_some() {
                warnings.push(format!("DataSource '{}' 的 connector_config 含明文 password，建议改用 credential", ds.api_name));
            }
        }
        for ot in &payload.object_types {
            for p in &ot.properties {
                if p.backing_mapping.is_none() && ot.storage_type == "VIRTUAL" {
                    warnings.push(format!(
                        "VIRTUAL ObjectType '{}' 的属性 '{}' 缺 backing_mapping（虚拟类型通常需声明物理列映射）",
                        ot.api_name, p.api_name
                    ));
                }
            }
        }

        // ── 5. links / actions / datasets / data_sources / groups 预测 ──
        let links: Vec<ImportPreviewItem> = payload
            .object_types
            .iter()
            .flat_map(|ot| ot.links.iter().map(move |l| (ot, l)))
            .map(|(ot, l)| {
                validate_link(l)?;
                Ok(ImportPreviewItem {
                    api_name: l.api_name.clone(),
                    status: "create".to_string(),
                    reason: format!("(via {})", ot.api_name),
                })
            })
            .collect::<StoreResult<Vec<_>>>()?;

        let actions = payload.action_types.iter().map(|a| {
            validate_action(a)?;
            Ok(ImportPreviewItem {
                api_name: a.api_name.clone(),
                status: "create".to_string(),
                reason: String::new(),
            })
        }).collect::<StoreResult<Vec<_>>>()?;

        let datasets = payload.datasets.iter().map(|d| {
            validate_dataset(d)?;
            let exists = self.dataset_exists(&conn, &payload.api_name, &d.api_name)?;
            Ok(ImportPreviewItem {
                api_name: d.api_name.clone(),
                status: if exists { "skip".to_string() } else { "create".to_string() },
                reason: if exists { "同名已存在".to_string() } else { String::new() },
            })
        }).collect::<StoreResult<Vec<_>>>()?;

        let data_sources = payload.data_sources.iter().map(|d| {
            validate_data_source(d)?;
            let exists = self.data_source_exists(&conn, &payload.api_name, &d.api_name)?;
            let status = if !exists {
                "create".to_string()
            } else if overwrite_ds.contains(&d.api_name) {
                "overwrite".to_string()
            } else {
                "skip".to_string()
            };
            Ok(ImportPreviewItem {
                api_name: d.api_name.clone(),
                status,
                reason: if exists && !overwrite_ds.contains(&d.api_name) {
                    "同名已存在".to_string()
                } else {
                    String::new()
                },
            })
        }).collect::<StoreResult<Vec<_>>>()?;

        let existing_groups = self.list_existing_group_api_names(&conn, &payload.api_name)?;
        let object_type_groups = payload.object_type_groups.iter().map(|g| {
            validate_group(g)?;
            let exists = existing_groups.contains(&g.api_name);
            Ok(ImportPreviewItem {
                api_name: g.api_name.clone(),
                status: if exists { "skip".to_string() } else { "create".to_string() },
                reason: if exists { "同名已存在".to_string() } else { String::new() },
            })
        }).collect::<StoreResult<Vec<_>>>()?;

        Ok(ImportPreview {
            ontology_api_name: payload.api_name.clone(),
            ontology_status: ontology_status.to_string(),
            object_types,
            links,
            actions,
            datasets,
            data_sources,
            object_type_groups,
            warnings,
            errors,
        })
    }

    // ════════════════════════════════════════════════════════════════
    // import
    // ════════════════════════════════════════════════════════════════

    /// 执行导入（对齐 Gaia `import_ontology`）。
    ///
    /// DAG 顺序：Ontology → DataSource → Dataset → ObjectType(+Property) →
    /// Link → Action → Group。best-effort：单个实体失败记入 errors 继续往下，
    /// 不整体回滚（对齐 Gaia 的 partial failure 语义）。
    pub fn import(&self, req: &ImportRequest) -> StoreResult<ImportResult> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let payload = &req.payload;
        let overwrite = &req.overwrite_object_types;
        let overwrite_ds = &req.overwrite_data_sources;
        let now = now_ms();
        let mut errors = Vec::new();
        let mut ot_results = Vec::new();
        let mut links_created: i32 = 0;
        let mut links_skipped: i32 = 0;
        let mut action_results = Vec::new();
        let mut dataset_results = Vec::new();
        let mut data_source_results = Vec::new();
        let mut group_results = Vec::new();

        // ── 1. Ontology ──
        let ontology_status = match self.upsert_ontology(&tx, payload, now)? {
            OntUpsert::Created => "created",
            OntUpsert::Existed => "existed",
        };

        // ── 2. DataSource ──
        for ds in &payload.data_sources {
            match self.upsert_data_source(&tx, &payload.api_name, ds, overwrite_ds.contains(&ds.api_name), now) {
                Ok(Upsert::Created) => dataset_results_push(&mut data_source_results, &ds.api_name, "created", None),
                Ok(Upsert::Overwritten) => dataset_results_push(&mut data_source_results, &ds.api_name, "overwritten", None),
                Ok(Upsert::Skipped) => dataset_results_push(&mut data_source_results, &ds.api_name, "skipped", None),
                Err(e) => {
                    let msg = format!("DataSource '{}': {}", ds.api_name, e);
                    errors.push(msg.clone());
                    dataset_results_push(&mut data_source_results, &ds.api_name, "failed", Some(msg));
                }
            }
        }

        // ── 3. Dataset ──
        for d in &payload.datasets {
            match self.upsert_dataset(&tx, &payload.api_name, d, now) {
                Ok(true) => dataset_results_push(&mut dataset_results, &d.api_name, "created", None),
                Ok(false) => dataset_results_push(&mut dataset_results, &d.api_name, "skipped", None),
                Err(e) => {
                    let msg = format!("Dataset '{}': {}", d.api_name, e);
                    errors.push(msg.clone());
                    dataset_results_push(&mut dataset_results, &d.api_name, "failed", Some(msg));
                }
            }
        }

        // （决策 10 修订：dataset/data_source 现在直接带 ontology_api_name 列，
        // 归属在主表上——不再需要独立的 refs 声明表登记。）

        // ── 4. ObjectType + Property ──
        for ot in &payload.object_types {
            match self.upsert_object_type(&tx, &payload.api_name, ot, overwrite.contains(&ot.api_name), now) {
                Ok(Upsert::Created) => ot_results.push(ImportItemResult {
                    api_name: ot.api_name.clone(),
                    status: "created".to_string(),
                    error: None,
                }),
                Ok(Upsert::Overwritten) => ot_results.push(ImportItemResult {
                    api_name: ot.api_name.clone(),
                    status: "overwritten".to_string(),
                    error: None,
                }),
                Ok(Upsert::Skipped) => ot_results.push(ImportItemResult {
                    api_name: ot.api_name.clone(),
                    status: "skipped".to_string(),
                    error: None,
                }),
                Err(e) => {
                    let msg = format!("ObjectType '{}': {}", ot.api_name, e);
                    errors.push(msg.clone());
                    ot_results.push(ImportItemResult {
                        api_name: ot.api_name.clone(),
                        status: "failed".to_string(),
                        error: Some(msg),
                    });
                    // OT 失败则跳过它的 links（引用解析不了）
                    continue;
                }
            }
            // links 嵌在 OT 下，OT 成功后才写 link
            for link in &ot.links {
                match self.upsert_link(&tx, &payload.api_name, &ot.api_name, link, now) {
                    Ok(true) => links_created += 1,
                    Ok(false) => links_skipped += 1,
                    Err(e) => {
                        errors.push(format!("Link '{}.{}': {}", ot.api_name, link.api_name, e));
                    }
                }
            }
        }

        // ── 5. ActionType ──
        for act in &payload.action_types {
            match self.upsert_action(&tx, &payload.api_name, act, now) {
                Ok(true) => action_results.push(ImportItemResult {
                    api_name: act.api_name.clone(),
                    status: "created".to_string(),
                    error: None,
                }),
                Ok(false) => action_results.push(ImportItemResult {
                    api_name: act.api_name.clone(),
                    status: "skipped".to_string(),
                    error: None,
                }),
                Err(e) => {
                    let msg = format!("ActionType '{}': {}", act.api_name, e);
                    errors.push(msg.clone());
                    action_results.push(ImportItemResult {
                        api_name: act.api_name.clone(),
                        status: "failed".to_string(),
                        error: Some(msg),
                    });
                }
            }
        }

        // ── 6. ObjectTypeGroup ──
        for g in &payload.object_type_groups {
            match self.upsert_group(&tx, &payload.api_name, g, now) {
                Ok(true) => group_results.push(ImportItemResult {
                    api_name: g.api_name.clone(),
                    status: "created".to_string(),
                    error: None,
                }),
                Ok(false) => group_results.push(ImportItemResult {
                    api_name: g.api_name.clone(),
                    status: "skipped".to_string(),
                    error: None,
                }),
                Err(e) => {
                    let msg = format!("ObjectTypeGroup '{}': {}", g.api_name, e);
                    errors.push(msg.clone());
                    group_results.push(ImportItemResult {
                        api_name: g.api_name.clone(),
                        status: "failed".to_string(),
                        error: Some(msg),
                    });
                }
            }
        }

        tx.commit()?;

        // 通知前端刷新（无论是否部分失败，库已变更）。
        self.notify_change();

        Ok(ImportResult {
            ontology_api_name: payload.api_name.clone(),
            ontology_status: ontology_status.to_string(),
            object_types: ot_results,
            links_created,
            links_skipped,
            action_types: action_results,
            datasets: dataset_results,
            data_sources: data_source_results,
            object_type_groups: group_results,
            errors,
        })
    }

    // ════════════════════════════════════════════════════════════════
    // 只读 summary / drill-in（对齐 Gaia describe_ontology / list_object_types /
    // describe_object_type 的 read-view）
    // ════════════════════════════════════════════════════════════════
    //
    // 会话引用场景专用——不返回完整 OntologyPayload（write-view，100KB+），
    // 而是分层轻量视图：summary 拿 OT 目录 → 按需 describe 单个 OT。
    // 总上下文增量 5-6KB（vs 整包 100KB+），大本体不撑爆预算。

    /// 本体 summary（对齐 Gaia `describe_ontology(summary=True)`）。
    ///
    /// 轻量目录视图：OT 只给 api_name/display_name/primary_key/storage_type/
    /// property_count，不含 properties[]/links[]/actions[]。link/action 仅计数。
    /// 供 agent 第一跳拿到 OT 概览，需要详情时调 `describe_object_type`。
    pub fn describe_ontology_summary(&self, ontology_api_name: &str) -> StoreResult<OntologySummaryFull> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;

        // OT 摘要 + property_count（一条 join 聚合）
        let mut stmt = conn.prepare(
            "SELECT ot.api_name, ot.display_name, COALESCE(ot.description,''),
                    ot.primary_key, ot.storage_type, COUNT(p.id)
             FROM object_types ot
             LEFT JOIN properties p ON p.object_type_id = ot.id
             WHERE ot.ontology_id = ?1
             GROUP BY ot.id
             ORDER BY ot.api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont.id], |r| {
            Ok(ObjectTypeSummary {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                primary_key: r.get(3)?,
                storage_type: r.get(4)?,
                property_count: r.get::<_, i64>(5)? as usize,
            })
        })?;
        let mut object_types = Vec::new();
        for row in rows {
            object_types.push(row?);
        }

        let link_type_count = self.count_by_ontology(&conn, "link_types", &ont.id)?;
        let action_type_count = self.count_by_ontology(&conn, "action_types", &ont.id)?;
        let charter = self.get_charter_inner(&conn, &ont.api_name)?;

        Ok(OntologySummaryFull {
            api_name: ont.api_name,
            display_name: ont.display_name,
            description: ont.description,
            charter,
            object_types,
            link_type_count,
            action_type_count,
        })
    }

    /// ObjectType 列表（对齐 Gaia `list_object_types`，比 summary 更轻）。
    ///
    /// 仅 api_name/display_name/description/storage_type，无 primary_key/property_count。
    /// 用于「只想知道有哪些 OT」的极轻量场景。
    pub fn list_object_types(&self, ontology_api_name: &str) -> StoreResult<Vec<ObjectTypeBrief>> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, COALESCE(description,''), storage_type
             FROM object_types WHERE ontology_id=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont.id], |r| {
            Ok(ObjectTypeBrief {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                storage_type: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// ObjectType 完整元数据（对齐 Gaia `describe_object_type` 的 read-view）。
    ///
    /// 单个 OT 的完整 schema：properties[] + outbound/inbound links[] + applicable
    /// actions[]。体积可控（单 OT 通常 1-3KB），适合会话中按需 drill-in。
    /// OT 不存在返回 NotFound。
    pub fn describe_object_type(
        &self,
        ontology_api_name: &str,
        object_type_api_name: &str,
    ) -> StoreResult<ObjectTypeFullMetadata> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;

        // OT 基本字段
        let ot_id: String = conn.query_row(
            "SELECT id FROM object_types WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont.id, object_type_api_name],
            |r| r.get(0),
        ).map_err(|_| StoreError::NotFound(format!(
            "object_type '{object_type_api_name}' in ontology '{ontology_api_name}'"
        )))?;

        let (display_name, description, primary_key, title_property, storage_type, visibility, backing_dataset_api_name):
            (String, String, String, String, String, String, Option<String>) = conn.query_row(
            "SELECT display_name, COALESCE(description,''), primary_key,
                    COALESCE(title_property,''), storage_type, visibility,
                    backing_dataset_api_name
             FROM object_types WHERE id=?1",
            rusqlite::params![ot_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6).unwrap_or(None))),
        )?;

        // properties（复用 export 的查询逻辑）
        let properties: Vec<PropertyReadView> = self.export_properties(&conn, &ot_id)?
            .into_iter().map(|p| p.to_read_view()).collect();
        // outbound/inbound links：对齐 Gaia，只返 api_name 列表（详情调 describe_link_type）
        let outbound_links: Vec<String> = self.export_links(&conn, &ont.id, object_type_api_name)?
            .into_iter().map(|l| l.api_name).collect();
        let inbound_links: Vec<String> = self.export_inbound_links(&conn, &ont.id, object_type_api_name)?
            .into_iter().map(|l| l.api_name).collect();
        // applicable actions：对齐 Gaia，只返 api_name 列表
        let actions: Vec<String> = self.export_actions_for_ot(&conn, &ont.id, object_type_api_name)?
            .into_iter().map(|a| a.api_name).collect();

        Ok(ObjectTypeFullMetadata {
            api_name: object_type_api_name.to_string(),
            display_name,
            description,
            primary_key,
            title_property,
            storage_type,
            visibility,
            status: "ACTIVE".to_string(),
            backing_dataset_api_name,
            properties,
            outbound_links,
            inbound_links,
            actions,
        })
    }

    /// LinkType 列表（对齐 Gaia `list_link_types`）。
    ///
    /// 名单视图（5 字段）：api_name/source_object_type/target_object_type/cardinality/
    /// foreign_key_property_api_name。无 display_name（对齐 Gaia）。
    pub fn list_link_types(&self, ontology_api_name: &str) -> StoreResult<Vec<LinkTypeBrief>> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        let mut stmt = conn.prepare(
            "SELECT api_name, source_object_type_api_name,
                    target_object_type_api_name, cardinality, foreign_key_property_api_name
             FROM link_types WHERE ontology_id=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont.id], |r| {
            Ok(LinkTypeBrief {
                api_name: r.get(0)?,
                source_object_type: r.get(1)?,
                target_object_type: r.get(2)?,
                cardinality: r.get(3)?,
                foreign_key_property_api_name: r.get(4).unwrap_or(None),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 单条 LinkType 完整定义（对齐 Gaia `describe_link_type`）。
    ///
    /// 按 link api_name 查询，返回 `LinkTypeFull`（对齐 Gaia `describe_link_type` 的 9 字段 read-view）。
    /// 适合「只关心一条关系」的细粒度场景，不必先 describe 承载它的 OT。
    /// link 不存在返回 NotFound。
    pub fn describe_link_type(&self, ontology_api_name: &str, link_api_name: &str) -> StoreResult<LinkTypeFull> {
        let conn = self.conn.lock().unwrap();
        let ont = self.get_ontology_row(&conn, ontology_api_name)?;
        conn.query_row(
            "SELECT api_name, display_name, description, source_object_type_api_name,
                    target_object_type_api_name, foreign_key_property_api_name, cardinality
             FROM link_types WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont.id, link_api_name],
            |r| {
                Ok(LinkTypeFull {
                    api_name: r.get(0)?,
                    display_name: r.get(1)?,
                    description: r.get(2)?,
                    source_object_type: r.get(3)?,
                    target_object_type: r.get(4)?,
                    foreign_key_property_api_name: r.get(5).unwrap_or(None),
                    cardinality: r.get(6)?,
                    // directional：固定 true（对齐 Gaia Sprint 1——所有 link 默认有方向性）
                    directional: true,
                    // has_properties：固定 false（本地未实现 link 属性表）
                    has_properties: false,
                })
            },
        )
        .map_err(|_| StoreError::NotFound(format!(
            "link_type '{link_api_name}' in ontology '{ontology_api_name}'"
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════
// 内部辅助
// ═══════════════════════════════════════════════════════════════════

#[derive(PartialEq)]
enum OntUpsert {
    Created,
    Existed,
}

#[derive(PartialEq)]
enum Upsert {
    Created,
    Overwritten,
    Skipped,
}

struct OntRow {
    id: String,
    api_name: String,
    display_name: String,
    description: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn dataset_results_push(v: &mut Vec<ImportItemResult>, api_name: &str, status: &str, error: Option<String>) {
    v.push(ImportItemResult {
        api_name: api_name.to_string(),
        status: status.to_string(),
        error,
    });
}

fn json_to_string(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string())
}

// ── 校验（preview 和 import 共用）──

fn validate_ot(ot: &ObjectTypeDef) -> StoreResult<()> {
    if !is_valid_object_type_api_name(&ot.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "ObjectType",
            api_name: ot.api_name.clone(),
            pattern: crate::naming::OBJECT_TYPE_API_NAME_PATTERN,
        });
    }
    if ot.storage_type != "MANAGED" && ot.storage_type != "VIRTUAL" {
        return Err(StoreError::Other(anyhow::anyhow!(
            "storage_type 必须是 MANAGED 或 VIRTUAL，got '{}'",
            ot.storage_type
        )));
    }
    for p in &ot.properties {
        validate_property(p)?;
    }
    Ok(())
}

fn validate_property(p: &PropertyDef) -> StoreResult<()> {
    if !is_valid_property_api_name(&p.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "Property",
            api_name: p.api_name.clone(),
            pattern: crate::naming::PROPERTY_API_NAME_PATTERN,
        });
    }
    if !is_valid_data_type(&p.data_type) {
        return Err(StoreError::InvalidDataType(p.data_type.clone()));
    }
    Ok(())
}

fn validate_link(l: &LinkDef) -> StoreResult<()> {
    if !is_valid_property_api_name(&l.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "LinkType",
            api_name: l.api_name.clone(),
            pattern: crate::naming::PROPERTY_API_NAME_PATTERN,
        });
    }
    if !is_valid_object_type_api_name(&l.target_object_type_api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "LinkType target",
            api_name: l.target_object_type_api_name.clone(),
            pattern: crate::naming::OBJECT_TYPE_API_NAME_PATTERN,
        });
    }
    if l.cardinality != "ONE" && l.cardinality != "MANY" {
        return Err(StoreError::Other(anyhow::anyhow!(
            "cardinality 必须是 ONE 或 MANY，got '{}'",
            l.cardinality
        )));
    }
    Ok(())
}

fn validate_action(a: &ActionTypeDef) -> StoreResult<()> {
    if !is_valid_property_api_name(&a.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "ActionType",
            api_name: a.api_name.clone(),
            pattern: crate::naming::PROPERTY_API_NAME_PATTERN,
        });
    }
    if !is_valid_object_type_api_name(&a.affected_object_type_api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "ActionType affected",
            api_name: a.affected_object_type_api_name.clone(),
            pattern: crate::naming::OBJECT_TYPE_API_NAME_PATTERN,
        });
    }
    Ok(())
}

fn validate_dataset(d: &DatasetDef) -> StoreResult<()> {
    if !is_valid_dataset_api_name(&d.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "Dataset",
            api_name: d.api_name.clone(),
            pattern: crate::naming::DATASET_API_NAME_PATTERN,
        });
    }
    Ok(())
}

fn validate_data_source(d: &DataSourceDef) -> StoreResult<()> {
    if !is_valid_dataset_api_name(&d.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "DataSource",
            api_name: d.api_name.clone(),
            pattern: crate::naming::DATASET_API_NAME_PATTERN,
        });
    }
    Ok(())
}

fn validate_group(g: &ObjectTypeGroupDef) -> StoreResult<()> {
    if !is_valid_object_type_api_name(&g.api_name) {
        return Err(StoreError::InvalidApiName {
            entity_kind: "ObjectTypeGroup",
            api_name: g.api_name.clone(),
            pattern: crate::naming::OBJECT_TYPE_API_NAME_PATTERN,
        });
    }
    Ok(())
}

// ── export 辅助 ──

impl OntologyStore {
    fn get_ontology_row(&self, conn: &Connection, api_name: &str) -> StoreResult<OntRow> {
        conn.query_row(
            "SELECT id, api_name, display_name, description FROM ontologies WHERE api_name=?1",
            rusqlite::params![api_name],
            |r| {
                Ok(OntRow {
                    id: r.get(0)?,
                    api_name: r.get(1)?,
                    display_name: r.get(2)?,
                    description: r.get(3)?,
                })
            },
        )
        .map_err(|_| StoreError::NotFound(format!("ontology '{api_name}'")))
    }

    fn export_object_types(&self, conn: &Connection, ont_id: &str) -> StoreResult<Vec<ObjectTypeDef>> {
        let mut stmt = conn.prepare(
            "SELECT id, api_name, display_name, description, primary_key, title_property, storage_type, visibility, capabilities
             FROM object_types WHERE ontology_id=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_id], |r| {
            let caps_str: String = r.get::<_, String>(8).unwrap_or_else(|_| "{}".to_string());
            let caps: Capabilities = serde_json::from_str(&caps_str).unwrap_or_default();
            Ok((
                r.get::<_, String>(0)?, // id
                ObjectTypeDef {
                    api_name: r.get(1)?,
                    display_name: r.get(2)?,
                    description: r.get(3)?,
                    primary_key: r.get(4)?,
                    title_property: r.get(5)?,
                    storage_type: r.get(6)?,
                    visibility: r.get(7)?,
                    capabilities: caps,
                    properties: Vec::new(),
                    links: Vec::new(),
                    confidence: None,
                },
            ))
        })?;
        let mut ots = Vec::new();
        for row in rows {
            let (ot_id, mut ot) = row?;
            ot.properties = self.export_properties(conn, &ot_id)?;
            ot.links = self.export_links(conn, ont_id, &ot.api_name)?;
            ots.push(ot);
        }
        Ok(ots)
    }

    fn export_properties(&self, conn: &Connection, ot_id: &str) -> StoreResult<Vec<PropertyDef>> {
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, description, data_type, searchable, is_primary_key, is_title_property,
                    backing_dataset_api_name, backing_catalog, backing_schema, backing_table, backing_column, vector_config
             FROM properties WHERE object_type_id=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ot_id], |r| {
            let bm_ds: Option<String> = r.get(7).unwrap_or(None);
            let bm_cat: Option<String> = r.get(8).unwrap_or(None);
            let bm_sch: Option<String> = r.get(9).unwrap_or(None);
            let bm_tbl: Option<String> = r.get(10).unwrap_or(None);
            let bm_col: Option<String> = r.get(11).unwrap_or(None);
            let vc_str: Option<String> = r.get(12).unwrap_or(None);
            let backing_mapping = if bm_ds.is_some() || bm_col.is_some() {
                Some(BackingMapping {
                    dataset_api_name: bm_ds.unwrap_or_default(),
                    backing_catalog: bm_cat.unwrap_or_default(),
                    backing_schema: bm_sch.unwrap_or_default(),
                    backing_table: bm_tbl.unwrap_or_default(),
                    backing_column: bm_col.unwrap_or_default(),
                })
            } else {
                None
            };
            let vector_config = vc_str.and_then(|s| serde_json::from_str(&s).ok());
            Ok(PropertyDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                data_type: r.get(3)?,
                searchable: r.get::<_, i64>(4)? != 0,
                is_primary_key: Some(r.get::<_, i64>(5)? != 0),
                is_title_property: Some(r.get::<_, i64>(6)? != 0),
                backing_mapping,
                vector_config,
                confidence: None,
            })
        })?;
        let mut props = Vec::new();
        for row in rows {
            props.push(row?);
        }
        Ok(props)
    }

    fn export_links(&self, conn: &Connection, ont_id: &str, ot_api_name: &str) -> StoreResult<Vec<LinkDef>> {
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, description, target_object_type_api_name, foreign_key_property_api_name,
                    cardinality, weight_property, temporal
             FROM link_types WHERE ontology_id=?1 AND source_object_type_api_name=?2 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_id, ot_api_name], |r| {
            Ok(LinkDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                target_object_type_api_name: r.get(3)?,
                foreign_key_property_api_name: r.get(4).unwrap_or(None),
                cardinality: r.get(5)?,
                weight_property: r.get(6).unwrap_or(None),
                temporal: r.get::<_, i64>(7)? != 0,
                confidence: None,
            })
        })?;
        let mut links = Vec::new();
        for row in rows {
            links.push(row?);
        }
        Ok(links)
    }

    /// inbound links：target = 本 OT 的 link（对齐 Gaia describe_object_type 的 inbound_links）。
    /// 与 `export_links`（outbound）对称，仅 WHERE 条件不同。
    fn export_inbound_links(&self, conn: &Connection, ont_id: &str, ot_api_name: &str) -> StoreResult<Vec<LinkDef>> {
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, description, target_object_type_api_name, foreign_key_property_api_name,
                    cardinality, weight_property, temporal
             FROM link_types WHERE ontology_id=?1 AND target_object_type_api_name=?2 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_id, ot_api_name], |r| {
            Ok(LinkDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                target_object_type_api_name: r.get(3)?,
                foreign_key_property_api_name: r.get(4).unwrap_or(None),
                cardinality: r.get(5)?,
                weight_property: r.get(6).unwrap_or(None),
                temporal: r.get::<_, i64>(7)? != 0,
                confidence: None,
            })
        })?;
        let mut links = Vec::new();
        for row in rows {
            links.push(row?);
        }
        Ok(links)
    }

    fn export_action_types(&self, conn: &Connection, ont_id: &str) -> StoreResult<Vec<ActionTypeDef>> {
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, description, affected_object_type_api_name,
                    parameters, rules, submission_criteria, effects, ontology_rules, risk_level, operation_kind, batch_enabled
             FROM action_types WHERE ontology_id=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_id], |r| {
            let parse = |s: String| -> Vec<Value> {
                serde_json::from_str(&s).unwrap_or_default()
            };
            Ok(ActionTypeDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                affected_object_type_api_name: r.get(3)?,
                parameters: parse(r.get::<_, String>(4).unwrap_or_else(|_| "[]".to_string())),
                rules: parse(r.get::<_, String>(5).unwrap_or_else(|_| "[]".to_string())),
                submission_criteria: parse(r.get::<_, String>(6).unwrap_or_else(|_| "[]".to_string())),
                effects: parse(r.get::<_, String>(7).unwrap_or_else(|_| "[]".to_string())),
                ontology_rules: parse(r.get::<_, String>(8).unwrap_or_else(|_| "[]".to_string())),
                risk_level: r.get(9)?,
                operation_kind: r.get(10)?,
                batch_enabled: r.get::<_, i64>(11)? != 0,
                confidence: None,
            })
        })?;
        let mut acts = Vec::new();
        for row in rows {
            acts.push(row?);
        }
        Ok(acts)
    }

    /// 作用于指定 OT 的 action（affected_object_type = 本 OT，对齐 Gaia
    /// describe_object_type 的 actions[]）。与 `export_action_types` 的区别仅 WHERE。
    fn export_actions_for_ot(&self, conn: &Connection, ont_id: &str, ot_api_name: &str) -> StoreResult<Vec<ActionTypeDef>> {
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, description, affected_object_type_api_name,
                    parameters, rules, submission_criteria, effects, ontology_rules, risk_level, operation_kind, batch_enabled
             FROM action_types WHERE ontology_id=?1 AND affected_object_type_api_name=?2 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_id, ot_api_name], |r| {
            let parse = |s: String| -> Vec<Value> {
                serde_json::from_str(&s).unwrap_or_default()
            };
            Ok(ActionTypeDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                affected_object_type_api_name: r.get(3)?,
                parameters: parse(r.get::<_, String>(4).unwrap_or_else(|_| "[]".to_string())),
                rules: parse(r.get::<_, String>(5).unwrap_or_else(|_| "[]".to_string())),
                submission_criteria: parse(r.get::<_, String>(6).unwrap_or_else(|_| "[]".to_string())),
                effects: parse(r.get::<_, String>(7).unwrap_or_else(|_| "[]".to_string())),
                ontology_rules: parse(r.get::<_, String>(8).unwrap_or_else(|_| "[]".to_string())),
                risk_level: r.get(9)?,
                operation_kind: r.get(10)?,
                batch_enabled: r.get::<_, i64>(11)? != 0,
                confidence: None,
            })
        })?;
        let mut acts = Vec::new();
        for row in rows {
            acts.push(row?);
        }
        Ok(acts)
    }

    /// 计数：某 ontology 下指定表（link_types / action_types）的行数。
    /// summary 用——只给计数不展开，避免体积膨胀。
    fn count_by_ontology(&self, conn: &Connection, table: &str, ont_id: &str) -> StoreResult<usize> {
        // table 名是内部常量，不做用户输入拼接（防注入）
        let sql = match table {
            "link_types" => "SELECT COUNT(*) FROM link_types WHERE ontology_id=?1",
            "action_types" => "SELECT COUNT(*) FROM action_types WHERE ontology_id=?1",
            _ => return Err(StoreError::Other(anyhow::anyhow!("invalid count table: {table}"))),
        };
        let n: i64 = conn.query_row(sql, rusqlite::params![ont_id], |r| r.get(0))?;
        Ok(n as usize)
    }

    fn export_datasets(
        &self,
        conn: &Connection,
        ontology_api_name: &str,
    ) -> StoreResult<Vec<DatasetDef>> {
        // 决策 10 修订：dataset 按本体隔离，export 只返回该本体声明的数据集。
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, storage_location, partition_config,
                    source_dataset_api_name, data_source_api_name, kind, is_view
             FROM datasets WHERE ontology_api_name=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ontology_api_name], |r| {
            let partition_config_str: Option<String> = r.get(3)?;
            let partition_config = partition_config_str
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(&s).ok());
            Ok(DatasetDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                storage_location: r.get(2)?,
                partition_config,
                source_dataset_api_name: r.get(4)?,
                data_source_api_name: r.get(5)?,
                kind: r.get(6)?,
                is_view: r.get::<_, i64>(7)? != 0,
                confidence: None,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn export_data_sources(
        &self,
        conn: &Connection,
        ontology_api_name: &str,
    ) -> StoreResult<Vec<DataSourceDef>> {
        // 决策 10 修订：data_source 按本体隔离，export 只返回该本体声明的数据源。
        let mut stmt = conn.prepare(
            "SELECT api_name, display_name, description, connector_type,
                    connector_config, credential_id
             FROM data_sources WHERE ontology_api_name=?1 ORDER BY api_name",
        )?;
        let rows = stmt.query_map(rusqlite::params![ontology_api_name], |r| {
            let config_str: Option<String> = r.get(4)?;
            let connector_config = config_str
                .filter(|s| !s.is_empty())
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Object(Default::default()));
            Ok(DataSourceDef {
                api_name: r.get(0)?,
                display_name: r.get(1)?,
                description: r.get(2)?,
                connector_type: r.get(3)?,
                connector_config,
                credential_id: r.get(5)?,
                confidence: None,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn export_object_type_groups(&self, _conn: &Connection, _ont_id: &str) -> StoreResult<Vec<ObjectTypeGroupDef>> {
        // ADR-023: groups 是 passthrough 字段——import 接受，export 不导出（对齐 Gaia）。
        Ok(Vec::new())
    }
}

// ── import upsert 辅助 ──

impl OntologyStore {
    fn ontology_exists(&self, conn: &Connection, api_name: &str) -> StoreResult<bool> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ontologies WHERE api_name=?1",
            rusqlite::params![api_name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn upsert_ontology(&self, tx: &rusqlite::Transaction, payload: &OntologyPayload, now: i64) -> StoreResult<OntUpsert> {
        if self.ontology_exists_tx(tx, &payload.api_name)? {
            // 只 UPDATE display_name + updated_at——description 不由 import 覆盖
            // （description 是本体级简介，归属 charter 管理域，避免增量导入覆盖业务说明）。
            tx.execute(
                "UPDATE ontologies SET display_name=?2, updated_at=?3 WHERE api_name=?1",
                rusqlite::params![payload.api_name, payload.display_name, now],
            )?;
            Ok(OntUpsert::Existed)
        } else {
            // 新建本体：description 用 payload 初始值（冷启动首导，后续不再被覆盖）。
            tx.execute(
                "INSERT INTO ontologies (id, api_name, display_name, description, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'ACTIVE', ?5, ?5)",
                rusqlite::params![new_id(), payload.api_name, payload.display_name, payload.description, now],
            )?;
            Ok(OntUpsert::Created)
        }
    }

    fn ontology_exists_tx(&self, tx: &rusqlite::Transaction, api_name: &str) -> StoreResult<bool> {
        let n: i64 = tx.query_row(
            "SELECT COUNT(*) FROM ontologies WHERE api_name=?1",
            rusqlite::params![api_name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn list_existing_ot_api_names(&self, conn: &Connection, ont_api_name: &str) -> StoreResult<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT ot.api_name FROM object_types ot
             JOIN ontologies o ON o.id = ot.ontology_id
             WHERE o.api_name=?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_api_name], |r| r.get::<_, String>(0))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row?);
        }
        Ok(v)
    }

    fn list_existing_group_api_names(&self, conn: &Connection, ont_api_name: &str) -> StoreResult<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT g.api_name FROM object_type_groups g
             JOIN ontologies o ON o.id = g.ontology_id
             WHERE o.api_name=?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![ont_api_name], |r| r.get::<_, String>(0))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row?);
        }
        Ok(v)
    }

    fn dataset_exists(&self, conn: &Connection, ontology_api_name: &str, api_name: &str) -> StoreResult<bool> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM datasets WHERE ontology_api_name=?1 AND api_name=?2",
            rusqlite::params![ontology_api_name, api_name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn data_source_exists(&self, conn: &Connection, ontology_api_name: &str, api_name: &str) -> StoreResult<bool> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM data_sources WHERE ontology_api_name=?1 AND api_name=?2",
            rusqlite::params![ontology_api_name, api_name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn upsert_data_source(&self, tx: &rusqlite::Transaction, ontology_api_name: &str, ds: &DataSourceDef, overwrite: bool, now: i64) -> StoreResult<Upsert> {
        if self.data_source_exists_tx(tx, ontology_api_name, &ds.api_name)? {
            if !overwrite {
                // skip（不覆写，对齐 Gaia：同名 DS 默认 skip）
                return Ok(Upsert::Skipped);
            }
            // overwrite=true：用 payload 字段 UPDATE 已有记录
            // （用于从脱敏 *** 升级到真实凭据等场景）
            tx.execute(
                "UPDATE data_sources
                 SET display_name=?2, description=?3, connector_type=?4,
                     connector_config=?5, credential_id=?6, updated_at=?7
                 WHERE ontology_api_name=?1 AND api_name=?8",
                rusqlite::params![
                    ontology_api_name, ds.display_name, ds.description,
                    ds.connector_type, json_to_string(&ds.connector_config),
                    ds.credential_id, now, ds.api_name
                ],
            )?;
            Ok(Upsert::Overwritten)
        } else {
            tx.execute(
                "INSERT INTO data_sources (id, ontology_api_name, api_name, display_name, description, connector_type, connector_config, credential_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                rusqlite::params![
                    new_id(), ontology_api_name, ds.api_name, ds.display_name, ds.description,
                    ds.connector_type, json_to_string(&ds.connector_config),
                    ds.credential_id, now
                ],
            )?;
            Ok(Upsert::Created)
        }
    }

    fn data_source_exists_tx(&self, tx: &rusqlite::Transaction, ontology_api_name: &str, api_name: &str) -> StoreResult<bool> {
        let n: i64 = tx.query_row(
            "SELECT COUNT(*) FROM data_sources WHERE ontology_api_name=?1 AND api_name=?2",
            rusqlite::params![ontology_api_name, api_name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn upsert_dataset(&self, tx: &rusqlite::Transaction, ontology_api_name: &str, d: &DatasetDef, now: i64) -> StoreResult<bool> {
        if self.dataset_exists_tx(tx, ontology_api_name, &d.api_name)? {
            Ok(false)
        } else {
            let pc = d.partition_config.as_ref().map(json_to_string);
            tx.execute(
                "INSERT INTO datasets (id, ontology_api_name, api_name, display_name, storage_location, partition_config, source_dataset_api_name, data_source_api_name, kind, is_view, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
                rusqlite::params![
                    new_id(), ontology_api_name, d.api_name, d.display_name, d.storage_location, pc,
                    d.source_dataset_api_name, d.data_source_api_name, d.kind,
                    d.is_view as i64, now
                ],
            )?;
            Ok(true)
        }
    }

    fn dataset_exists_tx(&self, tx: &rusqlite::Transaction, ontology_api_name: &str, api_name: &str) -> StoreResult<bool> {
        let n: i64 = tx.query_row(
            "SELECT COUNT(*) FROM datasets WHERE ontology_api_name=?1 AND api_name=?2",
            rusqlite::params![ontology_api_name, api_name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    fn upsert_object_type(
        &self,
        tx: &rusqlite::Transaction,
        ont_api_name: &str,
        ot: &ObjectTypeDef,
        overwrite: bool,
        now: i64,
    ) -> StoreResult<Upsert> {
        let ont_id: String = tx.query_row(
            "SELECT id FROM ontologies WHERE api_name=?1",
            rusqlite::params![ont_api_name],
            |r| r.get(0),
        )?;
        let existing_id: Option<String> = tx.query_row(
            "SELECT id FROM object_types WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont_id, ot.api_name],
            |r| r.get(0),
        ).optional()?;

        if let Some(id) = existing_id {
            if !overwrite {
                return Ok(Upsert::Skipped);
            }
            // 覆写：删旧 OT（级联删 properties），重建
            tx.execute("DELETE FROM object_types WHERE id=?1", rusqlite::params![id])?;
            let new_ot_id = new_id();
            self.insert_object_type(tx, &new_ot_id, &ont_id, ot, now)?;
            for p in &ot.properties {
                self.insert_property(tx, &new_ot_id, p, now)?;
            }
            Ok(Upsert::Overwritten)
        } else {
            let new_ot_id = new_id();
            self.insert_object_type(tx, &new_ot_id, &ont_id, ot, now)?;
            for p in &ot.properties {
                self.insert_property(tx, &new_ot_id, p, now)?;
            }
            Ok(Upsert::Created)
        }
    }

    fn insert_object_type(
        &self,
        tx: &rusqlite::Transaction,
        id: &str,
        ont_id: &str,
        ot: &ObjectTypeDef,
        now: i64,
    ) -> StoreResult<()> {
        let caps = json!({
            "graph_indexing_enabled": ot.capabilities.graph_indexing_enabled,
            "geotime_indexing_enabled": ot.capabilities.geotime_indexing_enabled,
        });
        tx.execute(
            "INSERT INTO object_types (id, ontology_id, api_name, display_name, description, primary_key, title_property, storage_type, visibility, capabilities, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            rusqlite::params![
                id, ont_id, ot.api_name, ot.display_name, ot.description,
                ot.primary_key, ot.title_property, ot.storage_type, ot.visibility,
                json_to_string(&caps), now
            ],
        )?;
        Ok(())
    }

    fn insert_property(
        &self,
        tx: &rusqlite::Transaction,
        ot_id: &str,
        p: &PropertyDef,
        now: i64,
    ) -> StoreResult<()> {
        let bm = p.backing_mapping.as_ref();
        let vc = p.vector_config.as_ref().map(json_to_string);
        tx.execute(
            "INSERT INTO properties (id, object_type_id, api_name, display_name, description, data_type, is_primary_key, is_title_property, searchable, backing_dataset_api_name, backing_catalog, backing_schema, backing_table, backing_column, vector_config, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
            rusqlite::params![
                new_id(), ot_id, p.api_name, p.display_name, p.description, p.data_type,
                p.is_primary_key.unwrap_or(false) as i64,
                p.is_title_property.unwrap_or(false) as i64,
                p.searchable as i64,
                bm.map(|b| b.dataset_api_name.as_str()),
                bm.map(|b| b.backing_catalog.as_str()),
                bm.map(|b| b.backing_schema.as_str()),
                bm.map(|b| b.backing_table.as_str()),
                bm.map(|b| b.backing_column.as_str()),
                vc, now
            ],
        )?;
        Ok(())
    }

    fn upsert_link(
        &self,
        tx: &rusqlite::Transaction,
        ont_api_name: &str,
        source_ot_api_name: &str,
        link: &LinkDef,
        now: i64,
    ) -> StoreResult<bool> {
        let ont_id: String = tx.query_row(
            "SELECT id FROM ontologies WHERE api_name=?1",
            rusqlite::params![ont_api_name],
            |r| r.get(0),
        )?;
        let exists: bool = {
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM link_types WHERE ontology_id=?1 AND api_name=?2",
                rusqlite::params![ont_id, link.api_name],
                |r| r.get(0),
            )?;
            n > 0
        };
        if exists {
            Ok(false)
        } else {
            tx.execute(
                "INSERT INTO link_types (id, ontology_id, api_name, display_name, description, source_object_type_api_name, target_object_type_api_name, foreign_key_property_api_name, cardinality, weight_property, temporal, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                rusqlite::params![
                    new_id(), ont_id, link.api_name, link.display_name, link.description,
                    source_ot_api_name, link.target_object_type_api_name,
                    link.foreign_key_property_api_name, link.cardinality,
                    link.weight_property, link.temporal as i64, now
                ],
            )?;
            Ok(true)
        }
    }

    fn upsert_action(
        &self,
        tx: &rusqlite::Transaction,
        ont_api_name: &str,
        act: &ActionTypeDef,
        now: i64,
    ) -> StoreResult<bool> {
        let ont_id: String = tx.query_row(
            "SELECT id FROM ontologies WHERE api_name=?1",
            rusqlite::params![ont_api_name],
            |r| r.get(0),
        )?;
        let exists: bool = {
            let n: i64 = tx.query_row(
                "SELECT COUNT(*) FROM action_types WHERE ontology_id=?1 AND api_name=?2",
                rusqlite::params![ont_id, act.api_name],
                |r| r.get(0),
            )?;
            n > 0
        };
        if exists {
            Ok(false)
        } else {
            tx.execute(
                "INSERT INTO action_types (id, ontology_id, api_name, display_name, description, affected_object_type_api_name, parameters, rules, submission_criteria, effects, ontology_rules, risk_level, operation_kind, batch_enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)",
                rusqlite::params![
                    new_id(), ont_id, act.api_name, act.display_name, act.description,
                    act.affected_object_type_api_name,
                    json_to_string(&Value::Array(act.parameters.clone())),
                    json_to_string(&Value::Array(act.rules.clone())),
                    json_to_string(&Value::Array(act.submission_criteria.clone())),
                    json_to_string(&Value::Array(act.effects.clone())),
                    json_to_string(&Value::Array(act.ontology_rules.clone())),
                    act.risk_level, act.operation_kind, act.batch_enabled as i64, now
                ],
            )?;
            Ok(true)
        }
    }

    fn upsert_group(
        &self,
        tx: &rusqlite::Transaction,
        ont_api_name: &str,
        g: &ObjectTypeGroupDef,
        now: i64,
    ) -> StoreResult<bool> {
        let ont_id: String = tx.query_row(
            "SELECT id FROM ontologies WHERE api_name=?1",
            rusqlite::params![ont_api_name],
            |r| r.get(0),
        )?;
        let existing_gid: Option<String> = tx.query_row(
            "SELECT id FROM object_type_groups WHERE ontology_id=?1 AND api_name=?2",
            rusqlite::params![ont_id, g.api_name],
            |r| r.get(0),
        ).optional()?;
        if existing_gid.is_some() {
            Ok(false)
        } else {
            let gid = new_id();
            tx.execute(
                "INSERT INTO object_type_groups (id, ontology_id, api_name, display_name, description, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                rusqlite::params![gid, ont_id, g.api_name, g.display_name, g.description, now],
            )?;
            // 成员关联
            for member_api_name in &g.members {
                let ot_id: Option<String> = tx.query_row(
                    "SELECT id FROM object_types WHERE ontology_id=?1 AND api_name=?2",
                    rusqlite::params![ont_id, member_api_name],
                    |r| r.get(0),
                ).optional()?;
                if let Some(ot_id) = ot_id {
                    tx.execute(
                        "INSERT OR IGNORE INTO object_type_group_members (group_id, object_type_id, ontology_id, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![gid, ot_id, ont_id, now],
                    )?;
                }
            }
            Ok(true)
        }
    }
}

// ── 辅助 trait（rusqlite OptionalExtension）──
use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn sample_payload() -> OntologyPayload {
        OntologyPayload {
            api_name: "SupplyChain".to_string(),
            display_name: "供应链本体".to_string(),
            description: "测试用".to_string(),
            object_types: vec![ObjectTypeDef {
                api_name: "Supplier".to_string(),
                display_name: "供应商".to_string(),
                description: String::new(),
                primary_key: "supplierId".to_string(),
                title_property: "name".to_string(),
                storage_type: "MANAGED".to_string(),
                visibility: "NORMAL".to_string(),
                capabilities: Capabilities::default(),
                properties: vec![
                    PropertyDef {
                        api_name: "supplierId".to_string(),
                        display_name: "供应商ID".to_string(),
                        description: String::new(),
                        data_type: "STRING".to_string(),
                        searchable: true,
                        is_primary_key: Some(true),
                        is_title_property: None,
                        backing_mapping: None,
                        vector_config: None,
                        confidence: None,
                    },
                    PropertyDef {
                        api_name: "name".to_string(),
                        display_name: "名称".to_string(),
                        description: String::new(),
                        data_type: "STRING".to_string(),
                        searchable: true,
                        is_primary_key: Some(false),
                        is_title_property: Some(true),
                        backing_mapping: None,
                        vector_config: None,
                        confidence: None,
                    },
                ],
                links: vec![],
                confidence: None,
            }],
            action_types: vec![],
            datasets: vec![],
            data_sources: vec![],
            object_type_groups: vec![],
        }
    }

    #[test]
    fn roundtrip_export_after_import() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        // 补 dataset + data_source（sample_payload 默认为空，需显式加入才能验证导出链路）
        payload.data_sources.push(DataSourceDef {
            api_name: "erp_postgres".to_string(),
            display_name: "ERP 数据库".to_string(),
            description: "供应链主库".to_string(),
            connector_type: "postgresql".to_string(),
            connector_config: serde_json::json!({"host": "localhost", "port": 5432}),
            credential_id: None,
            confidence: None,
        });
        payload.datasets.push(DatasetDef {
            api_name: "suppliers_dataset".to_string(),
            display_name: "供应商数据集".to_string(),
            storage_location: "erp_postgres.public.suppliers".to_string(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: Some("erp_postgres".to_string()),
            kind: "VIRTUAL".to_string(),
            is_view: false,
            confidence: None,
        });
        let req = ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        };
        // preview 应无 errors
        let preview = store.preview_import(&req).unwrap();
        assert!(preview.errors.is_empty(), "preview errors: {:?}", preview.errors);
        assert_eq!(preview.object_types[0].status, "create");
        // import
        let result = store.import(&req).unwrap();
        assert_eq!(result.ontology_status, "created");
        assert_eq!(result.object_types[0].status, "created");
        // export 应能拿回
        let exported = store.export("SupplyChain").unwrap();
        assert_eq!(exported.api_name, "SupplyChain");
        assert_eq!(exported.object_types.len(), 1);
        assert_eq!(exported.object_types[0].api_name, "Supplier");
        assert_eq!(exported.object_types[0].properties.len(), 2);
        // export 按 api_name 排序（稳定输出），用 find 而非下标断言
        let pk = exported.object_types[0].properties.iter()
            .find(|p| p.is_primary_key == Some(true))
            .expect("应有主键属性");
        assert_eq!(pk.api_name, "supplierId");
        // datasets / data_sources 是全局资产，import 落库后 export 必须能拿回
        // （回归 guard：曾因 export_* 返回空 vec 导致前端看不到数据源/数据集）
        assert!(!exported.datasets.is_empty(), "export 应返回已导入的 datasets");
        assert!(!exported.data_sources.is_empty(), "export 应返回已导入的 data_sources");
        assert_eq!(exported.datasets[0].api_name, "suppliers_dataset");
        assert_eq!(exported.data_sources[0].api_name, "erp_postgres");
    }

    #[test]
    fn incremental_skip_then_overwrite() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = sample_payload();
        // 第一次 import：create
        let r1 = store.import(&ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        assert_eq!(r1.object_types[0].status, "created");
        // 第二次 import（不带 overwrite）：应 skip
        let r2 = store.import(&ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        assert_eq!(r2.object_types[0].status, "skipped");
        // 第三次 import（带 overwrite）：应 overwritten
        let r3 = store.import(&ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec!["Supplier".to_string()],
            overwrite_data_sources: vec![],
        }).unwrap();
        assert_eq!(r3.object_types[0].status, "overwritten");
    }

    #[test]
    fn preview_catches_missing_primary_key() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.object_types[0].properties[0].is_primary_key = Some(false); // 0 个主键
        let req = ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] };
        let preview = store.preview_import(&req).unwrap();
        assert!(preview.errors.iter().any(|e| e.contains("必须有且仅一个 is_primary_key")));
    }

    #[test]
    fn preview_catches_dangling_link_target() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.object_types[0].links.push(LinkDef {
            api_name: "supplies".to_string(),
            display_name: "供应".to_string(),
            description: String::new(),
            target_object_type_api_name: "NonExistent".to_string(), // 不存在
            foreign_key_property_api_name: None,
            cardinality: "MANY".to_string(),
            weight_property: None,
            temporal: false,
            confidence: None,
        });
        let req = ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] };
        let preview = store.preview_import(&req).unwrap();
        assert!(preview.errors.iter().any(|e| e.contains("NonExistent") && e.contains("不存在")));
    }

    #[test]
    fn invalid_api_name_rejected() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.object_types[0].api_name = "supplier".to_string(); // 小写，非法 PascalCase
        let req = ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] };
        let preview = store.preview_import(&req);
        assert!(preview.is_err());
    }

    #[test]
    fn action_type_roundtrip() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.action_types.push(ActionTypeDef {
            api_name: "submitOrder".to_string(),
            display_name: "提交订单".to_string(),
            description: "供应商提交订单".to_string(),
            affected_object_type_api_name: "Supplier".to_string(),
            parameters: vec![serde_json::json!({ "name": "amount", "data_type": "DECIMAL" })],
            rules: vec![serde_json::json!({ "rule": "amount > 0" })],
            submission_criteria: vec![serde_json::json!({
                "expression": "amount > 0",
                "error_message": "金额必须大于 0"
            })],
            effects: vec![serde_json::json!({ "type": "NOTIFY" })],
            ontology_rules: vec![serde_json::json!({ "target_object_type": "Supplier" })],
            risk_level: "high".to_string(),
            operation_kind: "create".to_string(),
            batch_enabled: true,
            confidence: None,
        });
        let req = ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] };
        let preview = store.preview_import(&req).unwrap();
        assert!(preview.errors.is_empty(), "preview should have no errors");
        assert_eq!(preview.actions[0].status, "create");
        let result = store.import(&req).unwrap();
        assert_eq!(result.action_types[0].status, "created");
        // export 后拿回，字段值一致
        let exported = store.export("SupplyChain").unwrap();
        assert_eq!(exported.action_types.len(), 1);
        let at = &exported.action_types[0];
        assert_eq!(at.api_name, "submitOrder");
        assert_eq!(at.affected_object_type_api_name, "Supplier");
        assert_eq!(at.risk_level, "high");
        assert_eq!(at.operation_kind, "create");
        assert!(at.batch_enabled);
        assert_eq!(at.parameters.len(), 1);
        assert_eq!(at.submission_criteria.len(), 1);
        assert_eq!(at.ontology_rules.len(), 1);
    }

    #[test]
    fn object_type_group_roundtrip() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.object_type_groups.push(ObjectTypeGroupDef {
            api_name: "CoreEntities".to_string(),
            display_name: "核心实体".to_string(),
            description: "核心业务实体分组".to_string(),
            members: vec!["Supplier".to_string()],
            confidence: None,
        });
        let req = ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] };
        let preview = store.preview_import(&req).unwrap();
        assert!(preview.errors.is_empty(), "preview should have no errors");
        let result = store.import(&req).unwrap();
        assert_eq!(result.object_type_groups[0].status, "created");
        // 验证 group 成员绑定落库（members 表）
        let conn = store.conn.lock().unwrap();
        let member_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM object_type_group_members m
                 JOIN object_type_groups g ON g.id = m.group_id
                 WHERE g.api_name = 'CoreEntities'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(member_count, 1, "group member should be bound");
        drop(conn);
    }

    #[test]
    fn dataset_and_data_source_import() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.data_sources.push(DataSourceDef {
            api_name: "erp_supplier_master".to_string(),
            display_name: "ERP 供应商主数据".to_string(),
            description: String::new(),
            connector_type: "mysql".to_string(),
            connector_config: serde_json::json!({ "host": "10.0.0.1", "port": 3306 }),
            credential_id: None,
            confidence: None,
        });
        payload.datasets.push(DatasetDef {
            api_name: "supplier_master".to_string(),
            display_name: "供应商主数据集".to_string(),
            storage_location: "erp/supplier/supplier_master".to_string(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: Some("erp_supplier_master".to_string()),
            kind: "MANAGED".to_string(),
            is_view: false,
            confidence: None,
        });
        let req = ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] };
        let preview = store.preview_import(&req).unwrap();
        assert!(preview.errors.is_empty(), "preview should have no errors");
        let result = store.import(&req).unwrap();
        assert_eq!(result.data_sources[0].status, "created");
        assert_eq!(result.datasets[0].status, "created");
        // 再 import 一遍：应 skip（幂等）
        let r2 = store.import(&ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        assert_eq!(r2.data_sources[0].status, "skipped");
        assert_eq!(r2.datasets[0].status, "skipped");
    }

    #[test]
    fn overwrite_rebuilds_properties() {
        // overwrite OT 时，旧 properties 应被级联删除，新 properties 重建
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = sample_payload();
        store.import(&ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        // 改 OT：加一个属性、删一个属性
        let mut payload2 = payload.clone();
        payload2.object_types[0].properties.push(PropertyDef {
            api_name: "phone".to_string(),
            display_name: "电话".to_string(),
            description: String::new(),
            data_type: "STRING".to_string(),
            searchable: true,
            is_primary_key: Some(false),
            is_title_property: None,
            backing_mapping: None,
            vector_config: None,
            confidence: None,
        });
        // 删除 name 属性（index 1）
        payload2.object_types[0].properties.remove(1);
        let r = store.import(&ImportRequest {
            payload: payload2,
            overwrite_object_types: vec!["Supplier".to_string()],
            overwrite_data_sources: vec![],
        }).unwrap();
        assert_eq!(r.object_types[0].status, "overwritten");
        // export 验证：应有 supplierId + phone 两个属性（name 已删）
        let exported = store.export("SupplyChain").unwrap();
        let ot = &exported.object_types[0];
        assert_eq!(ot.properties.len(), 2);
        let names: Vec<&str> = ot.properties.iter().map(|p| p.api_name.as_str()).collect();
        assert!(names.contains(&"supplierId"));
        assert!(names.contains(&"phone"));
        assert!(!names.contains(&"name"), "deleted property should not reappear");
    }

    // ── 只读 summary / drill-in 测试 ──

    /// 构造带 2 OT + 1 link（Supplier→Part）+ 1 action（作用于 Supplier）的本体。
    fn linked_payload() -> OntologyPayload {
        let mut p = sample_payload();
        // Supplier 加一个 outbound link：supplies → Part
        p.object_types[0].links.push(LinkDef {
            api_name: "supplies".to_string(),
            display_name: "供应".to_string(),
            description: String::new(),
            target_object_type_api_name: "Part".to_string(),
            foreign_key_property_api_name: None,
            cardinality: "MANY".to_string(),
            weight_property: None,
            temporal: false,
            confidence: None,
        });
        // 第二个 OT：Part（无 link）
        p.object_types.push(ObjectTypeDef {
            api_name: "Part".to_string(),
            display_name: "零件".to_string(),
            description: "产品零件".to_string(),
            primary_key: "partId".to_string(),
            title_property: "partName".to_string(),
            storage_type: "MANAGED".to_string(),
            visibility: "NORMAL".to_string(),
            capabilities: Capabilities::default(),
            properties: vec![
                PropertyDef {
                    api_name: "partId".to_string(),
                    display_name: "零件ID".to_string(),
                    description: String::new(),
                    data_type: "STRING".to_string(),
                    searchable: true,
                    is_primary_key: Some(true),
                    is_title_property: None,
                    backing_mapping: None,
                    vector_config: None,
                    confidence: None,
                },
                PropertyDef {
                    api_name: "partName".to_string(),
                    display_name: "零件名".to_string(),
                    description: String::new(),
                    data_type: "STRING".to_string(),
                    searchable: true,
                    is_primary_key: Some(false),
                    is_title_property: Some(true),
                    backing_mapping: None,
                    vector_config: None,
                    confidence: None,
                },
            ],
            links: vec![],
            confidence: None,
        });
        // 作用 Supplier 的 action
        p.action_types.push(ActionTypeDef {
            api_name: "submitOrder".to_string(),
            display_name: "提交订单".to_string(),
            description: "供应商提交订单".to_string(),
            affected_object_type_api_name: "Supplier".to_string(),
            parameters: vec![],
            rules: vec![],
            submission_criteria: vec![],
            effects: vec![],
            ontology_rules: vec![],
            risk_level: "low".to_string(),
            operation_kind: "create".to_string(),
            batch_enabled: false,
            confidence: None,
        });
        p
    }

    #[test]
    fn describe_ontology_summary_returns_ot_catalog_with_counts() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let s = store.describe_ontology_summary("SupplyChain").unwrap();
        assert_eq!(s.api_name, "SupplyChain");
        assert_eq!(s.object_types.len(), 2);
        // OT 按 api_name 排序：Part 在 Supplier 前
        assert_eq!(s.object_types[0].api_name, "Part");
        assert_eq!(s.object_types[1].api_name, "Supplier");
        // summary 字段：property_count 准确，无 properties[] 详载
        assert_eq!(s.object_types[0].property_count, 2);
        assert_eq!(s.object_types[1].property_count, 2);
        assert_eq!(s.object_types[0].primary_key, "partId");
        assert_eq!(s.object_types[1].primary_key, "supplierId");
        // link/action 仅计数
        assert_eq!(s.link_type_count, 1);
        assert_eq!(s.action_type_count, 1);
        // 未设 charter 时返回空结构体（不报错，各字段空串）
        assert_eq!(s.charter.business_scenario, "");
        assert_eq!(s.charter.business_essence, "");
        assert_eq!(s.charter.design_intent, "");
        assert_eq!(s.charter.invariants, "");
    }

    #[test]
    fn describe_ontology_summary_unknown_ontology_returns_not_found() {
        let store = OntologyStore::open_in_memory().unwrap();
        let err = store.describe_ontology_summary("NoSuchOnt").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    // ── 本体设计宪章（不变点）测试 ──

    #[test]
    fn set_and_get_charter_roundtrip() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let charter = OntologyCharter {
            business_scenario: "供应链采购场景：采购员向供应商下达采购单".into(),
            business_essence: "核心对象：供应商、零件、采购单；状态流转：草稿→已下单→已到货".into(),
            design_intent: "够用且可扩展：先建供应商+零件+采购单三角，后续可扩展质检/物流".into(),
            invariants: "采购单金额必须 >0；供应商状态为活跃时才可下单".into(),
            updated_by: "agent".into(),
            updated_at: 0,
        };
        store.set_charter("SupplyChain", &charter).unwrap();

        let got = store.get_charter("SupplyChain").unwrap();
        assert_eq!(got.business_scenario, charter.business_scenario);
        assert_eq!(got.business_essence, charter.business_essence);
        assert_eq!(got.design_intent, charter.design_intent);
        assert_eq!(got.invariants, charter.invariants);
        assert_eq!(got.updated_by, "agent");
        assert!(got.updated_at > 0, "updated_at 应被后端填为当前时间");
    }

    #[test]
    fn set_charter_twice_overrides_not_appends() {
        // charter 是 1:1 upsert，重复写是 UPDATE 不是新增多条
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let mut c1 = OntologyCharter { business_essence: "第一版本质".into(), ..Default::default() };
        c1.updated_by = "agent".into();
        store.set_charter("SupplyChain", &c1).unwrap();

        let mut c2 = OntologyCharter { business_essence: "第二版本质修订".into(), ..Default::default() };
        c2.updated_by = "user".into();
        store.set_charter("SupplyChain", &c2).unwrap();

        let got = store.get_charter("SupplyChain").unwrap();
        assert_eq!(got.business_essence, "第二版本质修订");
        assert_eq!(got.updated_by, "user");
    }

    #[test]
    fn get_charter_unknown_ontology_returns_not_found() {
        let store = OntologyStore::open_in_memory().unwrap();
        let err = store.get_charter("NoSuchOnt").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[test]
    fn describe_ontology_summary_includes_charter_when_set() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let charter = OntologyCharter {
            business_scenario: "场景X".into(),
            business_essence: "本质Y".into(),
            design_intent: "意图Z".into(),
            invariants: "不变量W".into(),
            updated_by: "agent".into(),
            updated_at: 0,
        };
        store.set_charter("SupplyChain", &charter).unwrap();

        let s = store.describe_ontology_summary("SupplyChain").unwrap();
        assert_eq!(s.charter.business_scenario, "场景X");
        assert_eq!(s.charter.business_essence, "本质Y");
        assert_eq!(s.charter.design_intent, "意图Z");
        assert_eq!(s.charter.invariants, "不变量W");
    }

    #[test]
    fn import_does_not_overwrite_charter() {
        // 核心不变点语义：增量 import 不覆盖 charter。
        // 先建本体 + 设 charter，再用带 description 的 payload 二次 import，
        // charter 应保持不变，ontology.description 也不被覆盖（由 set_charter 管理）。
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let charter = OntologyCharter {
            business_essence: "不应被覆盖的本质".into(),
            updated_by: "agent".into(),
            ..Default::default()
        };
        store.set_charter("SupplyChain", &charter).unwrap();

        // 二次 import（模拟增量更新，payload 带不同 description）
        let mut p2 = linked_payload();
        p2.description = "被 import 覆盖的 description（不应生效）".to_string();
        store.import(&ImportRequest { payload: p2, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let got = store.get_charter("SupplyChain").unwrap();
        assert_eq!(got.business_essence, "不应被覆盖的本质", "charter 不应被 import 覆盖");
    }

    #[test]
    fn delete_ontology_cascades_charter() {
        // ON DELETE CASCADE 应级联删 charter
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        let charter = OntologyCharter { business_essence: "本质".into(), updated_by: "agent".into(), ..Default::default() };
        store.set_charter("SupplyChain", &charter).unwrap();

        store.delete("SupplyChain").unwrap();

        let err = store.get_charter("SupplyChain").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[test]
    fn list_object_types_is_lighter_than_summary() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let ots = store.list_object_types("SupplyChain").unwrap();
        assert_eq!(ots.len(), 2);
        assert_eq!(ots[0].api_name, "Part");
        assert_eq!(ots[1].api_name, "Supplier");
        // Brief 只有 4 字段，不含 primary_key/property_count
        assert_eq!(ots[0].storage_type, "MANAGED");
    }

    #[test]
    fn describe_object_type_returns_full_schema_with_inbound_links_and_actions() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        // Supplier：有 1 个 outbound link（supplies→Part）、0 inbound、1 action
        let sup = store.describe_object_type("SupplyChain", "Supplier").unwrap();
        assert_eq!(sup.api_name, "Supplier");
        assert_eq!(sup.primary_key, "supplierId");
        assert_eq!(sup.title_property, "name");
        assert_eq!(sup.properties.len(), 2);
        // 对齐 Gaia：inbound/outbound_links 是 api_name 字符串列表
        assert_eq!(sup.outbound_links.len(), 1);
        assert_eq!(sup.outbound_links[0], "supplies");
        assert!(sup.inbound_links.is_empty(), "Supplier has no inbound links");
        assert_eq!(sup.actions.len(), 1);
        assert_eq!(sup.actions[0], "submitOrder");

        // Part：0 outbound、1 inbound（被 supplies 指向）、0 action
        let part = store.describe_object_type("SupplyChain", "Part").unwrap();
        assert_eq!(part.api_name, "Part");
        assert!(part.outbound_links.is_empty());
        assert_eq!(part.inbound_links.len(), 1);
        assert_eq!(part.inbound_links[0], "supplies");
        assert!(part.actions.is_empty());
    }

    #[test]
    fn describe_object_type_unknown_ot_returns_not_found() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let err = store.describe_object_type("SupplyChain", "Ghost").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    // ── LinkType drill-in 测试 ──

    #[test]
    fn list_link_types_returns_brief_catalog() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let links = store.list_link_types("SupplyChain").unwrap();
        assert_eq!(links.len(), 1);
        let l = &links[0];
        assert_eq!(l.api_name, "supplies");
        assert_eq!(l.source_object_type, "Supplier");
        assert_eq!(l.target_object_type, "Part");
        assert_eq!(l.cardinality, "MANY");
        // Brief 对齐 Gaia：5 字段，无 display_name
        assert!(l.foreign_key_property_api_name.is_none());
    }

    #[test]
    fn describe_link_type_returns_full_def() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let l = store.describe_link_type("SupplyChain", "supplies").unwrap();
        assert_eq!(l.api_name, "supplies");
        assert_eq!(l.source_object_type, "Supplier");
        assert_eq!(l.target_object_type, "Part");
        assert_eq!(l.cardinality, "MANY");
        // 对齐 Gaia：directional/has_properties 派生字段
        assert!(l.directional);
        assert!(!l.has_properties);
    }

    #[test]
    fn describe_link_type_unknown_returns_not_found() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = linked_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        let err = store.describe_link_type("SupplyChain", "ghostLink").unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    // ── delete ──

    #[test]
    fn delete_removes_ontology_and_cascades_children() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = sample_payload();
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        // 导入后应能查到
        assert!(store.list_ontologies().unwrap().iter().any(|o| o.api_name == "SupplyChain"));

        let deleted = store.delete("SupplyChain").unwrap();
        assert!(deleted, "删除已存在的本体应返回 true");

        // 本体本身不在了
        assert!(!store.list_ontologies().unwrap().iter().any(|o| o.api_name == "SupplyChain"));
        // 级联清子表：object_types 为空（直接查 DB 验证级联生效，不只依赖 list 语义）
        let conn = store.conn.lock().unwrap();
        let ot_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM object_types", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ot_count, 0, "object_types 应被级联删除");
        let prop_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM properties", [], |r| r.get(0))
            .unwrap();
        assert_eq!(prop_count, 0, "properties 应被级联删除");
        drop(conn);
    }

    #[test]
    fn delete_unknown_returns_false_idempotent() {
        let store = OntologyStore::open_in_memory().unwrap();
        // 空库删不存在的本体：不报错，返回 false（幂等）
        let deleted = store.delete("GhostOnt").unwrap();
        assert!(!deleted);
    }

    /// 带 dataset + data_source 的 payload（决策 10 修订：删除本体时级联清理孤儿资产）
    fn payload_with_assets(api_name: &str) -> OntologyPayload {
        let mut p = sample_payload();
        p.api_name = api_name.to_string();
        p.display_name = format!("{}本体", api_name);
        p.data_sources.push(DataSourceDef {
            api_name: "erp_postgres".to_string(),
            display_name: "ERP 数据库".to_string(),
            description: "供应链主库".to_string(),
            connector_type: "postgresql".to_string(),
            connector_config: serde_json::json!({"host": "localhost", "port": 5432}),
            credential_id: None,
            confidence: None,
        });
        p.datasets.push(DatasetDef {
            api_name: "suppliers_dataset".to_string(),
            display_name: "供应商数据集".to_string(),
            storage_location: "erp_postgres.public.suppliers".to_string(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: Some("erp_postgres".to_string()),
            kind: "VIRTUAL".to_string(),
            is_view: false,
            confidence: None,
        });
        p
    }

    #[test]
    fn delete_ontology_cascades_owned_datasets_and_data_sources() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = payload_with_assets("SupplyChain");
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        // 导入后该本体的资产可见（按本体隔离）
        assert_eq!(store.list_datasets("SupplyChain").unwrap().len(), 1);
        assert_eq!(store.list_data_sources("SupplyChain").unwrap().len(), 1);

        store.delete("SupplyChain").unwrap();

        // 删除本体后：资产随本体级联删除（FK ON DELETE CASCADE）
        assert!(store.list_datasets("SupplyChain").unwrap().is_empty(),
            "删除本体应级联删除其 dataset");
        assert!(store.list_data_sources("SupplyChain").unwrap().is_empty(),
            "删除本体应级联删除其 data_source");
    }

    #[test]
    fn datasets_data_sources_are_isolated_between_ontologies() {
        // 决策 10 修订：不同本体的同名 dataset/data_source 独立存在，互不申到。
        let store = OntologyStore::open_in_memory().unwrap();
        store.import(&ImportRequest { payload: payload_with_assets("SupplyChain"), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        store.import(&ImportRequest { payload: payload_with_assets("Logistics"), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        // 两个本体各自拥有同名资产，不串扰
        assert_eq!(store.list_datasets("SupplyChain").unwrap().len(), 1);
        assert_eq!(store.list_datasets("Logistics").unwrap().len(), 1);
        assert_eq!(store.list_data_sources("SupplyChain").unwrap().len(), 1);
        assert_eq!(store.list_data_sources("Logistics").unwrap().len(), 1);

        // 删 SupplyChain：只影响 SupplyChain 的资产，Logistics 的保留
        store.delete("SupplyChain").unwrap();
        assert!(store.list_datasets("SupplyChain").unwrap().is_empty(),
            "SupplyChain 的 dataset 应随本体删除");
        assert_eq!(store.list_datasets("Logistics").unwrap().len(), 1,
            "Logistics 的 dataset 不应被删（按本体隔离）");
        assert_eq!(store.list_data_sources("Logistics").unwrap().len(), 1,
            "Logistics 的 data_source 不应被删（按本体隔离）");

        // export Logistics 仍能看到它的资产
        let exported = store.export("Logistics").unwrap();
        assert_eq!(exported.datasets.len(), 1);
        assert_eq!(exported.data_sources.len(), 1);
        // export SupplyChain（已删）报 NotFound
        assert!(store.export("SupplyChain").is_err());
    }

    #[test]
    fn delete_frees_api_name_for_reimport() {
        let store = OntologyStore::open_in_memory().unwrap();
        let payload = sample_payload();
        store.import(&ImportRequest { payload: payload.clone(), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        assert!(store.delete("SupplyChain").unwrap());

        // 删除后同 api_name 可重新导入（不报 UNIQUE 冲突）
        let res = store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] });
        assert!(res.is_ok(), "删除后应可重新导入同 api_name");
        // 重新导入后能查到
        assert!(store.list_ontologies().unwrap().iter().any(|o| o.api_name == "SupplyChain"));
    }
    #[test]
    fn on_change_callback_fires_on_import_and_delete() {
        let store = OntologyStore::open_in_memory().unwrap();
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = fired.clone();
        store.set_on_change(Box::new(move || {
            n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));

        // import 触发一次
        store.import(&ImportRequest {
            payload: sample_payload(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        })
        .unwrap();
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 1);

        // 未注册前删除不触发（此处已注册）：删除存在 → 触发；删除不存在 → 不触发
        assert!(store.delete("SupplyChain").unwrap());
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(!store.delete("NoSuchOntology").unwrap());
        assert_eq!(fired.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn delete_object_type_cascades_links_and_actions() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = linked_payload();
        // 加一个影响 Part 的 action，验证连删
        payload.action_types.push(ActionTypeDef {
            api_name: "renamePart".to_string(),
            display_name: "重命名零件".to_string(),
            description: String::new(),
            affected_object_type_api_name: "Part".to_string(),
            parameters: vec![],
            rules: vec![],
            submission_criteria: vec![],
            effects: vec![],
            ontology_rules: vec![],
            risk_level: "low".to_string(),
            operation_kind: "update".to_string(),
            batch_enabled: false,
            confidence: None,
        });
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        // 删 Part：links(supplies 双端引用) + actions(renamePart) 连删，properties FK 级联
        let (removed, links, actions) = store.delete_object_type("SupplyChain", "Part").unwrap();
        assert!(removed);
        assert_eq!(links, 1);
        assert_eq!(actions, 1);

        let payload = store.export("SupplyChain").unwrap();
        assert!(payload.object_types.iter().all(|ot| ot.api_name != "Part"));
        // Supplier 还在，但它的 links 被(双端规则)清了
        let supplier = payload.object_types.iter().find(|ot| ot.api_name == "Supplier").unwrap();
        assert!(supplier.links.is_empty());
        // renamePart(影响 Part)被连删;submitOrder(影响 Supplier)保留
        assert_eq!(payload.action_types.len(), 1);
        assert_eq!(payload.action_types[0].api_name, "submitOrder");

        // 幂等：再删返回 false
        let (removed, _, _) = store.delete_object_type("SupplyChain", "Part").unwrap();
        assert!(!removed);
    }

    #[test]
    fn delete_link_and_action_type_are_idempotent() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = linked_payload();
        payload.action_types.push(ActionTypeDef {
            api_name: "renamePart".to_string(),
            display_name: "重命名零件".to_string(),
            description: String::new(),
            affected_object_type_api_name: "Part".to_string(),
            parameters: vec![],
            rules: vec![],
            submission_criteria: vec![],
            effects: vec![],
            ontology_rules: vec![],
            risk_level: "low".to_string(),
            operation_kind: "update".to_string(),
            batch_enabled: false,
            confidence: None,
        });
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        assert!(store.delete_link_type("SupplyChain", "supplies").unwrap());
        assert!(!store.delete_link_type("SupplyChain", "supplies").unwrap());
        assert!(store.delete_action_type("SupplyChain", "renamePart").unwrap());
        assert!(!store.delete_action_type("SupplyChain", "renamePart").unwrap());

        let payload = store.export("SupplyChain").unwrap();
        assert!(payload.object_types[0].links.is_empty());
        // renamePart 删了;sample 自带的 submitOrder 保留
        assert_eq!(payload.action_types.len(), 1);
        assert_eq!(payload.action_types[0].api_name, "submitOrder");
    }

    #[test]
    fn delete_dataset_refuses_when_referenced() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.data_sources.push(DataSourceDef {
            api_name: "erp_postgres".to_string(),
            display_name: "ERP 数据库".to_string(),
            description: String::new(),
            connector_type: "postgresql".to_string(),
            connector_config: serde_json::json!({"host": "localhost"}),
            credential_id: None,
            confidence: None,
        });
        payload.datasets.push(DatasetDef {
            api_name: "suppliers_dataset".to_string(),
            display_name: "供应商数据集".to_string(),
            storage_location: "erp.public.suppliers".to_string(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: Some("erp_postgres".to_string()),
            kind: "VIRTUAL".to_string(),
            is_view: false,
            confidence: None,
        });
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();
        // OT 绑定该 dataset（backing 引用，直接 SQL 设置——import 的 write-view 不含 OT 级 backing）
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("UPDATE object_types SET backing_dataset_api_name='suppliers_dataset'", []).unwrap();
        }

        // 被 OT backing 引用 → 拒绝
        let err = store.delete_dataset("SupplyChain", "suppliers_dataset").unwrap_err();
        assert!(err.to_string().contains("被引用"));

        // 解绑后可删
        let conn_affected = {
            // 直接清掉 OT 的 backing 引用（模拟用户先解绑）
            let conn = store.conn.lock().unwrap();
            conn.execute("UPDATE object_types SET backing_dataset_api_name=NULL", []).unwrap()
        };
        assert_eq!(conn_affected, 1);
        assert!(store.delete_dataset("SupplyChain", "suppliers_dataset").unwrap());
        // 幂等
        assert!(!store.delete_dataset("SupplyChain", "suppliers_dataset").unwrap());

        // 不存在 → false
        assert!(!store.delete_dataset("SupplyChain", "no_such_dataset").unwrap());
    }

    #[test]
    fn delete_data_source_refuses_when_dataset_bound() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.data_sources.push(DataSourceDef {
            api_name: "erp_postgres".to_string(),
            display_name: "ERP 数据库".to_string(),
            description: String::new(),
            connector_type: "postgresql".to_string(),
            connector_config: serde_json::json!({"host": "localhost"}),
            credential_id: None,
            confidence: None,
        });
        payload.datasets.push(DatasetDef {
            api_name: "suppliers_dataset".to_string(),
            display_name: "供应商数据集".to_string(),
            storage_location: "erp.public.suppliers".to_string(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: Some("erp_postgres".to_string()),
            kind: "VIRTUAL".to_string(),
            is_view: false,
            confidence: None,
        });
        store.import(&ImportRequest { payload, overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        // 被 suppliers_dataset 引用 → 拒绝
        let err = store.delete_data_source("SupplyChain", "erp_postgres").unwrap_err();
        assert!(err.to_string().contains("被引用"));
        assert!(!store.delete_data_source("SupplyChain", "no_such_source").unwrap());
    }

    #[test]
    fn changelog_revision_increments_and_enforces_limits() {
        let store = OntologyStore::open_in_memory().unwrap();
        store.import(&ImportRequest { payload: sample_payload(), overwrite_object_types: vec![], overwrite_data_sources: vec![] }).unwrap();

        // 第一条 revision=1
        let r1 = store.commit_change(
            "SupplyChain", "冷启动建模", "从业务材料抽象出 Supplier/Part",
            r#"{"created":["Supplier","Part"]}"#, None, "agent",
        ).unwrap();
        assert_eq!(r1, 1);

        // 第二条 revision=2
        let r2 = store.commit_change(
            "SupplyChain", "加状态属性", "给 Supplier 加 status",
            r#"{"modified":["Supplier"]}"#, Some("conv-123"), "agent",
        ).unwrap();
        assert_eq!(r2, 2);

        // list 倒序：最新在前
        let logs = store.list_changelog("SupplyChain").unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].revision, 2);
        assert_eq!(logs[0].conversation_id, Some("conv-123".to_string()));
        assert_eq!(logs[1].revision, 1);

        // title+body 超 500 → 拒
        let long_title = "x".repeat(501);
        let err = store.commit_change("SupplyChain", &long_title, "", "{}", None, "agent").unwrap_err();
        assert!(err.to_string().contains("500"));

        // 整条超 1K（change_summary 撑大）→ 拒
        let big_summary = "y".repeat(999);
        let err = store.commit_change("SupplyChain", "t", "b", &big_summary, None, "agent").unwrap_err();
        assert!(err.to_string().contains("1000"));

        // 不存在的本体 → 拒（get_ontology_row 返回 NotFound）
        assert!(store.commit_change("NoSuch", "x", "", "{}", None, "agent").is_err());
    }

    /// DataSource 見写：脱敏 *** 先入库，再带 overwrite_data_sources 重导真实密码，
    /// 验证 DB 实际字段被 UPDATE（不只断言状态，直接查 connector_config 原文）。
    #[test]
    fn data_source_overwrite_updates_connector_config() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.data_sources.push(DataSourceDef {
            api_name: "erp_postgres".to_string(),
            display_name: "ERP 数据库".to_string(),
            description: String::new(),
            connector_type: "postgresql".to_string(),
            // 模拟从 Gaia 导出的脱敏 payload：password 为占位符 ***
            connector_config: serde_json::json!({
                "host": "gaia-postgres",
                "port": 5432,
                "database": "ascend",
                "username": "ontology",
                "password": "***",
            }),
            credential_id: None,
            confidence: None,
        });

        // 第一次导入：created，DB 里存的是脱敏值
        let r1 = store.import(&ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        }).unwrap();
        assert_eq!(r1.data_sources[0].status, "created");
        let exported1 = store.export("SupplyChain").unwrap();
        let cfg1 = exported1.data_sources[0].connector_config.as_object().unwrap();
        assert_eq!(cfg1.get("password").and_then(|v| v.as_str()), Some("***"));

        // 第二次导入（不带 overwrite）：skip，DB 不变
        let r2 = store.import(&ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        }).unwrap();
        assert_eq!(r2.data_sources[0].status, "skipped");
        let exported2 = store.export("SupplyChain").unwrap();
        assert_eq!(
            exported2.data_sources[0].connector_config.as_object().unwrap().get("password").and_then(|v| v.as_str()),
            Some("***"),
            "未見写时 password 应仍为脱敏值"
        );

        // 修改 payload：把脱敏 *** 换成真实密码
        let mut payload_real = payload.clone();
        let ds = payload_real.data_sources.iter_mut().find(|d| d.api_name == "erp_postgres").unwrap();
        ds.connector_config = serde_json::json!({
            "host": "gaia-postgres",
            "port": 5432,
            "database": "ascend",
            "username": "ontology",
            "password": "real-secret-123",
        });
        // 顺便验证 display_name / credential_id 也会被 UPDATE
        ds.display_name = "ERP 数据库（已配置）".to_string();
        ds.credential_id = Some("cred-abc".to_string());

        // 第三次导入（带 overwrite_data_sources）：overwritten，DB 真实密码落库
        let r3 = store.import(&ImportRequest {
            payload: payload_real,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec!["erp_postgres".to_string()],
        }).unwrap();
        assert_eq!(r3.data_sources[0].status, "overwritten");
        assert!(r3.errors.is_empty());

        // 验证实际落库的字段值（不只断言状态）
        let exported3 = store.export("SupplyChain").unwrap();
        let ds3 = &exported3.data_sources[0];
        assert_eq!(ds3.api_name, "erp_postgres");
        assert_eq!(ds3.display_name, "ERP 数据库（已配置）");
        assert_eq!(ds3.credential_id, Some("cred-abc".to_string()));
        let cfg3 = ds3.connector_config.as_object().unwrap();
        assert_eq!(cfg3.get("password").and_then(|v| v.as_str()), Some("real-secret-123"));
        assert_eq!(cfg3.get("host").and_then(|v| v.as_str()), Some("gaia-postgres"));
        assert_eq!(cfg3.get("port").and_then(|v| v.as_i64()), Some(5432));
    }

    /// preview 对同名 DataSource 的状态预测：未列入 overwrite → skip，列入 → overwrite。
    #[test]
    fn preview_data_source_overwrite_prediction() {
        let store = OntologyStore::open_in_memory().unwrap();
        let mut payload = sample_payload();
        payload.data_sources.push(DataSourceDef {
            api_name: "erp_postgres".to_string(),
            display_name: "ERP 数据库".to_string(),
            description: String::new(),
            connector_type: "postgresql".to_string(),
            connector_config: serde_json::json!({"host": "localhost"}),
            credential_id: None,
            confidence: None,
        });
        // 先入库
        store.import(&ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        }).unwrap();

        // 不带 overwrite 预演：skip
        let p1 = store.preview_import(&ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        }).unwrap();
        assert_eq!(p1.data_sources[0].status, "skip");
        assert!(p1.data_sources[0].reason.contains("同名已存在"));

        // 带 overwrite 预演：overwrite
        let p2 = store.preview_import(&ImportRequest {
            payload: payload.clone(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec!["erp_postgres".to_string()],
        }).unwrap();
        assert_eq!(p2.data_sources[0].status, "overwrite");
        assert!(p2.data_sources[0].reason.is_empty());
    }

    /// 回归测试（用户实际 bug）：旧 .db 文件的 link_types 表含残留
    /// `direction TEXT NOT NULL` 列，app 用 `OntologyStore::open` 打开旧库时
    /// `init_schema` 的 `CREATE TABLE IF NOT EXISTS` 是 no-op 不会改表结构，
    /// 导致 import 时 `upsert_link` 的 INSERT 不含 direction 列报
    /// `NOT NULL constraint failed: link_types.direction`，14 条 LinkType
    /// 全部未落库。修复：init_schema 末尾幂等迁移 DROP direction 列。
    ///
    /// 此测试用文件 db（非内存）复现：先手工建旧 schema（含 direction NOT NULL），
    /// 再用 `OntologyStore::open` 打开（触发迁移），再 import 含 link 的 payload，
    /// 验证 link 成功落库 + 迁移后表无 direction 列。
    #[test]
    fn open_migrates_legacy_link_types_direction_and_imports_links() {
        use std::path::PathBuf;
        // 用 /tmp 下的文件 db，避免污染内存库 + 模拟真实持久 db 行为。
        let tmp: PathBuf =
            std::env::temp_dir().join(format!("onto_store_e2e_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        // 1. 先用裸 rusqlite 建一张「旧 schema」link_types（含 direction NOT NULL），
        //    模拟早期开发版留下的 .db 文件（其他表不建，让 init_schema 补建）。
        {
            let conn = Connection::open(&tmp).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE link_types (
                    id TEXT PRIMARY KEY, ontology_id TEXT NOT NULL,
                    api_name TEXT NOT NULL, display_name TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '',
                    source_object_type_api_name TEXT NOT NULL,
                    target_object_type_api_name TEXT NOT NULL,
                    foreign_key_property_api_name TEXT,
                    cardinality TEXT NOT NULL CHECK (cardinality IN ('ONE','MANY')),
                    direction TEXT NOT NULL CHECK (direction IN ('OUTGOING','INCOMING')),
                    weight_property TEXT, temporal INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                    UNIQUE(ontology_id, api_name),
                    FOREIGN KEY (ontology_id) REFERENCES ontologies(id) ON DELETE CASCADE
                );
                "#,
            ).unwrap();
            // 确认旧表带 direction 列。
            let mut s = conn.prepare("PRAGMA table_info(link_types)").unwrap();
            let cols: Vec<String> = s.query_map([], |r| r.get::<_, String>(1)).unwrap()
                .filter_map(Result::ok).collect();
            assert!(cols.iter().any(|c| c == "direction"), "旧表应有 direction 列");
        }
        // 2. 用修复后的 OntologyStore::open 打开旧库（触发 init_schema 迁移）。
        let store = OntologyStore::open(&tmp).unwrap();
        // 3. import 含 link 的 payload（linked_payload = 2 OT + 1 link）。
        let payload = linked_payload();
        let result = store.import(&ImportRequest {
            payload,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        }).unwrap();
        // 4. link 应成功落库（修复前会因 direction NOT NULL 失败）。
        assert_eq!(result.links_created, 1, "link 应成功落库");
        assert!(result.errors.is_empty(), "不应有错误: {:?}", result.errors);
        // 5. 迁移后表无 direction 列。
        {
            let conn = store.conn.lock().unwrap();
            let mut s = conn.prepare("PRAGMA table_info(link_types)").unwrap();
            let cols: Vec<String> = s.query_map([], |r| r.get::<_, String>(1)).unwrap()
                .filter_map(Result::ok).collect();
            assert!(!cols.iter().any(|c| c == "direction"), "迁移后不应有 direction 列");
        }
        // 6. 二次 open 幂等（direction 已无，迁移为 no-op，import 仍正常）。
        drop(store);
        let store2 = OntologyStore::open(&tmp).unwrap();
        let result2 = store2.import(&ImportRequest {
            payload: linked_payload(),
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        }).unwrap();
        assert_eq!(result2.links_created, 0, "二次导入同名 link 应 skip（不报错）");
        assert!(result2.errors.is_empty(), "二次导入不应有错误: {:?}", result2.errors);
        let _ = std::fs::remove_file(&tmp);
    }

}

#[cfg(test)]
mod repro_real_db {
    use super::*;
    /// 回归测试：旧库（决策 10 修订前的 schema）能被 init_schema 平滑迁移打开。
    /// 旧 datasets/data_sources 表无 ontology_api_name 列，主 batch 的 CREATE INDEX
    /// 引用该列会 panic——此测试确保迁移函数正确先加列再建索引。
    #[test]
    fn open_legacy_db_migrates_cleanly() {
        let src = "/Users/thinkpiggy/Library/Application Support/com.onto-studio.app/ontology.db";
        if !std::path::Path::new(src).exists() {
            eprintln!("skip: legacy db not found at {src}");
            return;
        }
        let tmp = std::env::temp_dir().join(format!(
            "onto_migrate_test_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        std::fs::copy(src, &tmp).unwrap();
        let store = OntologyStore::open(&tmp).expect("legacy db should migrate cleanly");
        let onts = store.list_ontologies().unwrap();
        for o in &onts {
            let _ = store.list_datasets(&o.api_name).unwrap();
            let _ = store.list_data_sources(&o.api_name).unwrap();
        }
        let _ = std::fs::remove_file(&tmp);
    }

    /// 回归测试：构造旧 schema（无 ontology_api_name 列 + api_name 全局唯一）的库，
    /// 验证 init_schema 迁移后能正常 import + export（决策 10 修订）。
    /// 回归测试（用户实测 bug）：旧库 datasets 表有列级 `api_name TEXT NOT NULL UNIQUE`
    /// （单列全局唯一），且已有全局行 `(ontology_api_name='', api_name='dealership')`。
    /// 迁移后导入新本体 NewOnt，其 payload 含同名 dataset `dealership`——
    /// 迁移前：INSERT 触发旧单列 UNIQUE 约束 → `UNIQUE constraint failed: datasets.api_name`；
    /// 迁移后：旧单列约束被表重建去掉，仅联合唯一索引生效 → 成功创建。
    ///
    /// 同时验证 preview 与实际 import 一致：preview 说 create，import 也成功 create
    /// （旧 bug 是 preview 查联合索引说 create，但 INSERT 撞旧单列约束失败）。
    #[test]
    fn migrate_legacy_global_unique_does_not_block_new_ontology_same_name() {
        let tmp = std::env::temp_dir().join(format!(
            "onto_migrate_global_unique_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        {
            let conn = rusqlite::Connection::open(&tmp).unwrap();
            // 旧 schema：datasets 无 ontology_api_name 列，api_name 列级 UNIQUE（全局唯一）
            conn.execute_batch(
                r#"CREATE TABLE ontologies (
                       id TEXT PRIMARY KEY, api_name TEXT NOT NULL UNIQUE,
                       display_name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
                       status TEXT NOT NULL DEFAULT 'ACTIVE',
                       created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                   CREATE TABLE data_sources (
                       id TEXT PRIMARY KEY, api_name TEXT NOT NULL UNIQUE,
                       display_name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
                       connector_type TEXT NOT NULL,
                       connector_config TEXT NOT NULL DEFAULT '{}',
                       credential_id TEXT,
                       created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                   CREATE TABLE datasets (
                       id TEXT PRIMARY KEY, api_name TEXT NOT NULL UNIQUE,
                       display_name TEXT NOT NULL DEFAULT '',
                       storage_location TEXT NOT NULL DEFAULT '',
                       partition_config TEXT, source_dataset_api_name TEXT,
                       data_source_api_name TEXT,
                       kind TEXT NOT NULL DEFAULT 'MANAGED',
                       is_view INTEGER NOT NULL DEFAULT 0,
                       created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                   -- 旧库已有一个"无本体归属"的 dealership 行（来自之前导入的其他本体）
                   INSERT INTO ontologies (id, api_name, display_name, created_at, updated_at)
                       VALUES ('oid0', 'OldOnt', '旧本体', 0, 0);
                   INSERT INTO datasets (id, api_name, display_name, storage_location, kind, is_view, created_at, updated_at)
                       VALUES ('d0', 'dealership', '经销商', 'loc', 'MANAGED', 0, 0, 0);
                "#,
            ).unwrap();
        }
        let store = OntologyStore::open(&tmp).expect("legacy db should migrate cleanly");

        // 迁移后：datasets 表 DDL 不应含列级 api_name UNIQUE（旧约束已由表重建去掉）
        let conn = store.conn.lock().unwrap();
        let ddl: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='datasets'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // DDL 里 api_name 所在行不应含 UNIQUE（列级）；表级 UNIQUE(ontology_api_name, api_name) 不算
        let has_col_level_unique = ddl
            .lines()
            .map(|l| l.trim())
            .any(|line| {
                let up = line.to_uppercase();
                !up.starts_with("UNIQUE(")
                    && up.contains("API_NAME")
                    && up.contains("UNIQUE")
            });
        assert!(!has_col_level_unique,
            "迁移后 datasets DDL 不应含列级 api_name UNIQUE（正是用户 bug 根因）。实际 DDL:\n{}", ddl);
        drop(conn);

        // 构造新本体 payload，含同名 dataset dealership（与旧库遗留全局行同名）
        let mut p = super::tests::sample_payload();
        p.api_name = "NewOnt".to_string();
        p.display_name = "新本体".to_string();
        p.datasets.push(DatasetDef {
            api_name: "dealership".to_string(),
            display_name: "经销商".to_string(),
            storage_location: String::new(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: None,
            kind: "MANAGED".to_string(),
            is_view: false,
            confidence: None,
        });
        let req = ImportRequest {
            payload: p,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        };

        // preview 应说该 dataset 是 create（联合唯一索引不冲突）
        let prev = store.preview_import(&req).unwrap();
        let ds_item = prev.datasets.iter().find(|i| i.api_name == "dealership")
            .expect("preview 应包含 dealership");
        assert_eq!(ds_item.status, "create",
            "preview 应判定 dealership 为新建（联合唯一索引下 (NewOnt, dealership) 不冲突）");

        // 实际 import 必须成功——这正是用户 bug：旧代码这里会因残留单列 UNIQUE 失败
        let result = store.import(&req).expect(
            "迁移后导入新本体（含与旧库遗留全局行同名的 dataset）必须成功——
            若失败说明旧的单列 api_name UNIQUE 约束未被表重建去掉");
        let ds_created = result.datasets.iter().find(|i| i.api_name == "dealership")
            .expect("import 结果应包含 dealership");
        assert_eq!(ds_created.status, "created",
            "dealership 应被成功创建，旧库遗留的全局行不阻硕新本体同名 dataset");

        // 验证两行共存：旧的全局行 (空串, dealership) + 新本体 (NewOnt, dealership)
        let conn = store.conn.lock().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM datasets WHERE api_name='dealership'",
                [],
                |r| r.get(0),
            ).unwrap();
        assert_eq!(n, 2,
            "应有两个同名 dealership 行：旧全局行 + 新本体行，按本体隔离共存");
        drop(conn);
        let _ = std::fs::remove_file(&tmp);
    }

    /// 端到端验证（真实用户 DB 副本）：用户的 ontology.db 里 datasets 表已迁移过加列，
    /// 但仍残留旧的单列 api_name UNIQUE 约束（`sqlite_autoindex_datasets_2`），
    /// 且已有全局行 (空串, dealership/lead/user 等来自 AutoMarketing 本体)。
    /// 修复后重新 open 应去掉旧约束，且能导入同名 dataset 的新本体。
    /// 此测试仅在开发者机器上跑（真实 DB 存在时），CI 上跳过。
    #[test]
    fn real_legacy_db_can_import_same_name_dataset_after_migration() {
        let src = "/Users/thinkpiggy/Library/Application Support/com.onto-studio.app/ontology.db";
        if !std::path::Path::new(src).exists() {
            eprintln!("skip: 真实 DB 不存在于 {src}");
            return;
        }
        let tmp = std::env::temp_dir().join(format!(
            "onto_real_verify_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp);
        std::fs::copy(src, &tmp).unwrap();
        let store = OntologyStore::open(&tmp).expect("真实库应迁移成功");

        // 迁移后 datasets DDL 不应含列级 api_name UNIQUE
        let conn = store.conn.lock().unwrap();
        let ddl: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='datasets'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let has_col_unique = ddl.lines().map(|l| l.trim()).any(|line| {
            let up = line.to_uppercase();
            !up.starts_with("UNIQUE(") && up.contains("API_NAME") && up.contains("UNIQUE")
        });
        assert!(!has_col_unique,
            "真实库迁移后不应残留列级 api_name UNIQUE。实际 DDL:\n{}", ddl);
        drop(conn);

        // 导入新本体 NewOnt，含同名 dataset dealership（与真实库遗留全局行同名）
        let mut p = super::tests::sample_payload();
        p.api_name = "NewOnt".to_string();
        p.display_name = "新本体".to_string();
        p.datasets.push(DatasetDef {
            api_name: "dealership".to_string(),
            display_name: "经销商".to_string(),
            storage_location: String::new(),
            partition_config: None,
            source_dataset_api_name: None,
            data_source_api_name: None,
            kind: "MANAGED".to_string(),
            is_view: false,
            confidence: None,
        });
        let req = ImportRequest {
            payload: p,
            overwrite_object_types: vec![],
            overwrite_data_sources: vec![],
        };
        let result = store.import(&req).expect(
            "真实库迁移后导入同名 dataset 的新本体必须成功——这正是用户报的 bug");
        let ds = result.datasets.iter().find(|i| i.api_name == "dealership")
            .expect("import 结果应含 dealership");
        assert_eq!(ds.status, "created");
        // 清理：删除测试创建的本体（级联清其 dataset）
        let _ = store.delete("NewOnt");
        let _ = std::fs::remove_file(&tmp);
    }
}
