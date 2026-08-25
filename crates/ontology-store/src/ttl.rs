//! W3C Turtle 本体存储 + 校验 + 导入导出（对齐 skill `ontology-modeling-w3c`）。
//!
//! 架构（方案 2.5：.ttl 文本存 SQLite + 内存 oxigraph 校验/查询）：
//!   - 持久化：SQLite `ontology_ttl` 表存整块 Turtle 文本（一行一个本体，按 IRI 隔离）
//!   - 校验/查询：临时内存 oxigraph `Store` parse Turtle + 跑 SPARQL（不持久化内存图）
//!   - 导出：直接 SELECT ttl_content 返回（不碰 oxigraph）
//!
//! 选型理由（守 AGENTS.md 原则）：
//!   - 零 C++ 依赖：oxigraph `default-features=false` 关掉 rocksdb 后端，纯 Rust
//!   - 持久化复用 bundled SQLite（对齐 Palantir `OntologyStore` 的文件存储体验）
//!   - SPARQL 不打折扣：校验（7+1 规范）和查询（第 7 类 CRUD）走 oxigraph 原生引擎
//!
//! 工作流（对齐 Palantir `ontology_tools` 的闭环）：
//!   - 冷启动：validate_ttl(ttl) → import_ttl(ttl)
//!   - 增量：export_ttl(iri) → 改 Turtle → validate → import_ttl(ttl, overwrite=true)
//!
//! 推理暂不内置（对齐 oxigraph 官方设计决策 + 原则 2 轻量化）。

use std::sync::Mutex;

use rusqlite::Connection;
use uuid::Uuid;

use crate::error::{StoreError, StoreResult};

// ════════════════════════════════════════════════════════════════
// 校验/导入结果 DTO（对齐 Palantir 的 ImportPreview/ImportResult）
// ════════════════════════════════════════════════════════════════

/// Turtle 校验结果（对应 Palantir `ImportPreview` 的 dry-run 角色）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct TtlValidation {
    /// 三元组总数（解析成功后）
    #[specta(type = specta_typescript::Number)]
    pub triple_count: u64,
    /// 阻断性错误（非空则禁止 import）
    pub errors: Vec<String>,
    /// 非阻断性警告（可继续 import）
    pub warnings: Vec<String>,
}

/// Turtle 导入结果（对应 Palantir `ImportResult`）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct TtlImportResult {
    /// 本体 IRI（从 ttl 里 owl:Ontology 提取）
    pub ontology_iri: String,
    /// 已导入三元组数
    #[specta(type = specta_typescript::Number)]
    pub triple_count: u64,
    /// 导入期间发生的错误（best-effort，单三元组失败不整体回滚）
    pub errors: Vec<String>,
    /// 导入期间发生的警告
    pub warnings: Vec<String>,
}

/// 本体摘要（对齐 Palantir `OntologySummary`，用于列表页）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct TtlOntologySummary {
    /// 本体 IRI（主键）
    pub ontology_iri: String,
    /// 版本（owl:versionInfo，可空）
    pub version: String,
    /// 三元组数
    #[specta(type = specta_typescript::Number)]
    pub triple_count: u64,
    /// 最后更新时间（unix ms）
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

/// 本体设计宪章（不变点，对齐 Palantir `OntologyCharter`）。
///
/// 字段语义与 Palantir `OntologyCharter` 完全一致，仅主键改为 `ontology_iri`。
/// charter 记录业务本质说明（不随历史变化），与 changelog（变化点）分离。
/// 额外设计：charter 的四字段可同步以 W3C 原生词汇（`dcterms:` + `skos:note`）
/// 写入 .ttl 文本，本表是可查询的结构化索引。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq, Default)]
pub struct TtlCharter {
    /// 业务场景（服务于什么业务目标、谁用、解决什么问题）
    #[serde(default)]
    pub business_scenario: String,
    /// 业务本质（核心业务对象/状态/关系/动态行为的一句话本质概括）
    #[serde(default)]
    pub business_essence: String,
    /// 设计意图（为什么这样建模、够用且可扩展的取舍、可扩展方向）
    #[serde(default)]
    pub design_intent: String,
    /// 补充说明（自由文本，记录不可违反的业务约束、边界条件等）
    #[serde(default)]
    pub invariants: String,
    /// "agent" | "user"
    #[serde(default)]
    pub updated_by: String,
    /// unix ms
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

/// 本体变更日志条目（git commit log 式，对齐 Palantir `OntologyChangelog`）。
///
/// 每条记录一次 import/delete 后的设计说明：title+body 为人可读的 commit message，
/// change_summary 为机器可读的实体级 +/−/~ 摘要。revision 每本体从 1 递增。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, PartialEq)]
pub struct TtlChangelog {
    /// 本体内递增序号
    #[specta(type = specta_typescript::Number)]
    pub revision: u32,
    /// 一句话标题
    pub title: String,
    /// 设计说明正文
    pub body: String,
    /// JSON 字符串：{"created":[...],"deleted":[...],"modified":[...]}
    pub change_summary: String,
    /// 来源会话 id（可空，手工导入无）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// "agent" | "user"
    pub author: String,
    /// unix ms
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
}

// ════════════════════════════════════════════════════════════════
// TtlStore：SQLite 持久化 + 内存 oxigraph 校验/查询
// ════════════════════════════════════════════════════════════════

/// W3C Turtle 本体存储。
///
/// 对齐 Palantir `OntologyStore` 的结构：`Mutex<Connection>` + open/open_in_memory +
/// on_change 回调。Tauri AppState 会包一层 Arc。
///
/// 存储：SQLite `ontology_ttl` 表（持久化 .ttl 文本）；校验/查询时临时构造内存
/// oxigraph `Store`，不持久化内存图。
pub struct TtlStore {
    conn: Mutex<Connection>,
    /// 落库变更回调（平台无关）：import 成功后触发。
    /// 由装配层（src-tauri）注入，用于通知前端刷新。
    on_change: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl TtlStore {
    /// 打开（或创建）位于 `path` 的 SQLite 库文件，并完成 schema 初始化。
    pub fn open(path: impl AsRef<std::path::Path>) -> StoreResult<Self> {
        let conn = Connection::open(path)?;
        init_ttl_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            on_change: Mutex::new(None),
        })
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> StoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        init_ttl_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            on_change: Mutex::new(None),
        })
    }

    /// 注册落库变更回调（覆盖式）。
    pub fn set_on_change(&self, cb: Box<dyn Fn() + Send + Sync>) {
        *self.on_change.lock().unwrap() = Some(cb);
    }

    /// 触发变更回调（若已注册）。import 成功后调用。
    fn notify_change(&self) {
        if let Some(cb) = self.on_change.lock().unwrap().as_ref() {
            cb();
        }
    }

    // ── 校验（dry-run，不写库）──────────────────────────────────

    /// 校验 Turtle 内容：语法合法性 + 7+1 语义规范一致性。
    ///
    /// **errors 非空则禁止 import**：
    ///   - Turtle 语法错（解析失败）
    ///   - 类非 `a owl:Class`（用了 rdfs:Class）
    ///   - 属性混用（同一 IRI 既是 DatatypeProperty 又是 ObjectProperty）
    ///   - SWRL 规则未用 swrl:Imp 标准词汇
    ///   - IRI 命名规范违反（类 PascalCase / 属性 camelCase）
    ///   - rdfs:subClassOf 循环继承
    ///
    /// **warnings 非空可继续 import**：
    ///   - 中文标签未带 `@zh`
    pub fn validate_ttl(&self, ttl_content: &str) -> StoreResult<TtlValidation> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // ── 1. 语法解析（oxigraph 兜底）──
        let temp = match parse_ttl_to_memory(ttl_content) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("Turtle 语法错误: {e}"));
                return Ok(TtlValidation {
                    triple_count: 0,
                    errors,
                    warnings,
                });
            }
        };
        let triple_count = temp.len().map(|n| n as u64).unwrap_or(0);

        // ── 2. 7+1 语义规范检查（Rust 侧真相源，SPARQL 在 temp 上跑）──
        check_class_uses_owl_class(&temp, &mut errors);
        check_property_not_mixed(&temp, &mut errors);
        check_iri_naming(&temp, &mut errors);
        check_chinese_labels_have_lang(&temp, &mut warnings);
        check_swrl_uses_standard_vocab(&temp, &mut errors);
        check_subclass_no_cycle(&temp, &mut errors);
        check_subclass_subject_is_class(&temp, &mut errors);
        check_subproperty_subject_is_property(&temp, &mut errors);

        Ok(TtlValidation {
            triple_count,
            errors,
            warnings,
        })
    }

    // ── 导入（落库）──────────────────────────────────────────────

    /// 导入 Turtle 到存储。best-effort：校验失败仍尝试落库（但建议先 validate）。
    ///
    /// **调用前建议先 validate_ttl 确认无 errors**。
    /// `overwrite=true` 时 UPSERT（同 IRI 覆盖）；`overwrite=false` 时已存在则 skip。
    pub fn import_ttl(&self, ttl_content: &str, overwrite: bool) -> StoreResult<TtlImportResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // 1. 提取本体 IRI（从 owl:Ontology 声明）
        let (ontology_iri, version) = extract_ontology_meta(ttl_content, &mut warnings);
        let ontology_iri = match ontology_iri {
            Some(iri) => iri,
            None => {
                return Ok(TtlImportResult {
                    ontology_iri: String::new(),
                    triple_count: 0,
                    errors: vec!["未找到 owl:Ontology 声明，无法确定本体 IRI".to_string()],
                    warnings,
                });
            }
        };

        // 2. 统计三元组数（解析一次）
        let triple_count = match parse_ttl_to_memory(ttl_content) {
            Ok(t) => t.len().map(|n| n as u64).unwrap_or(0),
            Err(e) => {
                return Ok(TtlImportResult {
                    ontology_iri,
                    triple_count: 0,
                    errors: vec![format!("Turtle 解析失败: {e}")],
                    warnings,
                });
            }
        };

        // 3. 落库（SQLite UPSERT）
        let conn = self.conn.lock().unwrap();
        if !overwrite {
            // 检查是否已存在
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM ontology_ttl WHERE ontology_iri = ?",
                    rusqlite::params![&ontology_iri],
                    |_| Ok(()),
                )
                .is_ok();
            if exists {
                return Ok(TtlImportResult {
                    ontology_iri,
                    triple_count,
                    errors,
                    warnings: {
                        let mut w = warnings;
                        w.push("本体已存在且 overwrite=false，已跳过".to_string());
                        w
                    },
                });
            }
        }
        match conn.execute(
            "INSERT INTO ontology_ttl (ontology_iri, ttl_content, version, triple_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(ontology_iri) DO UPDATE SET
                ttl_content = excluded.ttl_content,
                version = excluded.version,
                triple_count = excluded.triple_count,
                updated_at = excluded.updated_at",
            rusqlite::params![
                &ontology_iri,
                ttl_content,
                version.as_deref().unwrap_or(""),
                triple_count as i64,
                now_ms(),
            ],
        ) {
            Ok(_) => {}
            Err(e) => {
                errors.push(format!("SQLite 写入失败: {e}"));
            }
        }
        drop(conn);

        self.notify_change();

        Ok(TtlImportResult {
            ontology_iri,
            triple_count,
            errors,
            warnings,
        })
    }

    // ── 导出（序列化）────────────────────────────────────────────

    /// 导出指定本体 IRI 的 Turtle 文本（对齐 Palantir `export(ontology_api_name)`）。
    /// 直接从 SQLite 读 .ttl 文本，不碰 oxigraph。
    pub fn export_ttl(&self, ontology_iri: &str) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        let ttl: String = conn.query_row(
            "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        )?;
        Ok(ttl)
    }

    /// 列出所有已存本体（轻量摘要，对齐 Palantir `list_ontologies`）。
    pub fn list_ontologies(&self) -> StoreResult<Vec<TtlOntologySummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT ontology_iri, COALESCE(version, ''), triple_count, updated_at
             FROM ontology_ttl ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TtlOntologySummary {
                ontology_iri: r.get(0)?,
                version: r.get(1)?,
                triple_count: r.get::<_, i64>(2)? as u64,
                updated_at: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 删除指定本体 IRI（幂等）。
    pub fn delete_ontology(&self, ontology_iri: &str) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
        )?;
        drop(conn);
        if affected > 0 {
            self.notify_change();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 执行 SPARQL 查询（第 7 类 CRUD，对齐 skill 方法论）。
    ///
    /// 把指定本体 IRI 的 .ttl 文本 load 进内存 oxigraph，跑 SPARQL，返回 JSON 结果。
    /// 适合中小本体（< 10 万三元组）；大本体会有秒级 parse 延迟。
    ///
    /// 自动注入常用 W3C 前缀声明（rdf/rdfs/owl/xsd/skos/swrl），用户查询可用缩写。
    pub fn query_sparql(&self, ontology_iri: &str, sparql: &str) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let store = parse_ttl_to_memory(&ttl)?;
        use oxigraph::sparql::{SparqlEvaluator, results::{QueryResultsFormat, QueryResultsSerializer}};
        // 注入常用前缀，让用户查询能用 owl:Class 等缩写
        let prefixed = inject_common_prefixes(sparql);
        let parsed = SparqlEvaluator::new()
            .parse_query(&prefixed)
            .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
        let results = parsed
            .on_store(&store)
            .execute()
            .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
        // 序列化为 SPARQL Results JSON
        let serializer = QueryResultsSerializer::from_format(QueryResultsFormat::Json);
        let mut buf = Vec::new();
        match results {
            oxigraph::sparql::QueryResults::Solutions(sols) => {
                let vars = sols.variables().to_vec();
                let mut writer = serializer
                    .serialize_solutions_to_writer(&mut buf, vars)
                    .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
                for sol in sols {
                    let sol = sol.map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
                    writer.serialize(&sol).map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
                }
                writer.finish().map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
            }
            oxigraph::sparql::QueryResults::Boolean(b) => {
                serializer.serialize_boolean_to_writer(&mut buf, b).map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
            }
            oxigraph::sparql::QueryResults::Graph(_) => {
                return Ok("{}".to_string());
            }
        }
        String::from_utf8(buf).map_err(|e| StoreError::Other(anyhow::anyhow!(e)))
    }

    // ════════════════════════════════════════════════════════════════
    // 本体设计宪章（不变点）+ 变更日志（变化点）
    // ════════════════════════════════════════════════════════════════

    /// 读取本体设计宪章（不变点）。
    ///
    /// 无 charter 行时返回空结构体（各字段空串）——业务上视为「尚未定义宪章」。
    /// 不报错，调用方决定是否提示建模者补充。
    pub fn get_charter(&self, ontology_iri: &str) -> StoreResult<TtlCharter> {
        let conn = self.conn.lock().unwrap();
        let row = conn.query_row(
            "SELECT business_scenario, business_essence, design_intent, invariants, updated_by, updated_at \
             FROM ontology_ttl_charter WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| {
                Ok(TtlCharter {
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
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(TtlCharter::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// 写入/更新本体设计宪章（不变点）。
    ///
    /// **charter 不随历史变化**——它记录本体的业务本质说明，不随实体增删改而变更。
    /// 只有用户明确要求调整 charter 时才调用；常规增量更新不应触碰 charter。
    /// 冷启动建模时，如历史对话已提及业务场景/本质/意图，应主动提取后调用本工具落库。
    /// import_ttl 不覆盖 charter（写入路径分离）。
    pub fn set_charter(&self, ontology_iri: &str, charter: &TtlCharter) -> StoreResult<()> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        conn.execute(
            "INSERT INTO ontology_ttl_charter (ontology_iri, business_scenario, business_essence, design_intent, invariants, updated_by, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(ontology_iri) DO UPDATE SET
                business_scenario = excluded.business_scenario,
                business_essence   = excluded.business_essence,
                design_intent      = excluded.design_intent,
                invariants         = excluded.invariants,
                updated_by         = excluded.updated_by,
                updated_at         = excluded.updated_at",
            rusqlite::params![
                ontology_iri,
                charter.business_scenario,
                charter.business_essence,
                charter.design_intent,
                charter.invariants,
                if charter.updated_by.is_empty() { "agent" } else { &charter.updated_by },
                now,
            ],
        )?;
        drop(conn);
        self.notify_change();
        Ok(())
    }

    /// 提交一条本体变更日志（git commit message 式）。
    ///
    /// **每次 import_ttl 或 delete_ontology 之后应调用**——把本次改动的「为什么」和
    /// 「整体设计」落成可回溯记录。体积限制：title+body ≤500 字符，整条（含 summary）≤1K 字符。
    /// revision 每本体从 1 递增。返回分配的 revision 序号。
    /// changelog 写入不视为本体本身变更，不触发 notify_change（避免循环刷新）。
    pub fn commit_change(
        &self,
        ontology_iri: &str,
        title: &str,
        body: &str,
        change_summary: &str,
        conversation_id: Option<&str>,
        author: &str,
    ) -> StoreResult<u32> {
        const MAX_TITLE_BODY_CHARS: usize = 500;
        const MAX_TOTAL_CHARS: usize = 1000;
        let title_body = title.chars().count() + body.chars().count();
        if title_body > MAX_TITLE_BODY_CHARS {
            return Err(StoreError::InvalidApiName {
                entity_kind: "ttl_changelog",
                api_name: format!("title+body={title_body}"),
                pattern: "title+body <= 500 chars",
            });
        }
        let total = title_body + change_summary.chars().count();
        if total > MAX_TOTAL_CHARS {
            return Err(StoreError::InvalidApiName {
                entity_kind: "ttl_changelog",
                api_name: format!("total={total}"),
                pattern: "total <= 1000 chars",
            });
        }
        let conn = self.conn.lock().unwrap();
        // 校验本体存在
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM ontology_ttl WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
                |_| Ok(()),
            )
            .is_ok();
        if !exists {
            return Err(StoreError::NotFound(format!(
                "本体 '{ontology_iri}' 未找到，无法提交 changelog"
            )));
        }
        let next_rev: i64 = conn.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM ontology_ttl_changelog WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        )?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO ontology_ttl_changelog (id, ontology_iri, revision, title, body, change_summary, conversation_id, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                new_id(),
                ontology_iri,
                next_rev,
                title,
                body,
                change_summary,
                conversation_id,
                author,
                now,
            ],
        )?;
        Ok(next_rev as u32)
    }

    /// 列出本体的变更日志（按 revision 倒序，最新在前）。
    pub fn list_changelog(&self, ontology_iri: &str) -> StoreResult<Vec<TtlChangelog>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT revision, title, body, change_summary, conversation_id, author, created_at \
             FROM ontology_ttl_changelog WHERE ontology_iri=?1 ORDER BY revision DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![ontology_iri], |r| {
            Ok(TtlChangelog {
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
}

// ════════════════════════════════════════════════════════════════
// 内部：schema 初始化 + Turtle 解析 + 元数据提取
// ════════════════════════════════════════════════════════════════

/// 初始化 ttl 存储表（对齐 Palantir `init_schema`）。
fn init_ttl_schema(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ontology_ttl (
            ontology_iri  TEXT PRIMARY KEY NOT NULL,
            ttl_content   TEXT NOT NULL,
            version       TEXT NOT NULL DEFAULT '',
            triple_count  INTEGER NOT NULL DEFAULT 0,
            updated_at    INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_ontology_ttl_updated
            ON ontology_ttl(updated_at);

        -- ═══ 本体设计宪章（不变点）═══
        -- 对齐 Palantir `ontology_charter` 表：记业务场景/本质/设计意图/补充说明。
        -- 主键用 ontology_iri（非 Palantir 的 api_name）——W3C 本体以 IRI 为身份。
        -- charter 由 set_ontology_ttl_charter 工具独立写入，不进 import_ttl 流程
        -- （import 不覆盖 charter，charter 也不从 .ttl 读取）。
        -- 设计动机与 Palantir 一致：charter 记「业务本质说明」不随历史变化，
        -- 与 changelog（变化点）物理/语义/写入路径三重分离。
        CREATE TABLE IF NOT EXISTS ontology_ttl_charter (
            ontology_iri      TEXT PRIMARY KEY,
            business_scenario TEXT NOT NULL DEFAULT '',
            business_essence   TEXT NOT NULL DEFAULT '',
            design_intent      TEXT NOT NULL DEFAULT '',
            invariants         TEXT NOT NULL DEFAULT '',
            updated_at         INTEGER NOT NULL,
            updated_by         TEXT NOT NULL DEFAULT 'agent',
            FOREIGN KEY (ontology_iri) REFERENCES ontology_ttl(ontology_iri) ON DELETE CASCADE
        );

        -- ═══ 本体变更日志（git commit log 式）═══
        -- 对齐 Palantir `ontology_changelog` 表：每次 import/delete 后留一条设计说明。
        -- revision 每本体从 1 递增（UNIQUE(ontology_iri, revision) 保证）。
        CREATE TABLE IF NOT EXISTS ontology_ttl_changelog (
            id              TEXT PRIMARY KEY,
            ontology_iri    TEXT NOT NULL,
            revision        INTEGER NOT NULL,
            title           TEXT NOT NULL,
            body            TEXT NOT NULL DEFAULT '',
            change_summary  TEXT NOT NULL DEFAULT '{}',
            conversation_id TEXT,
            author          TEXT NOT NULL DEFAULT 'agent',
            created_at      INTEGER NOT NULL,
            UNIQUE(ontology_iri, revision),
            FOREIGN KEY (ontology_iri) REFERENCES ontology_ttl(ontology_iri) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_ttl_changelog_ont
            ON ontology_ttl_changelog(ontology_iri, revision DESC);",
    )?;
    Ok(())
}

/// 解析 Turtle 文本到内存 oxigraph Store（复用）。
fn parse_ttl_to_memory(ttl: &str) -> StoreResult<oxigraph::store::Store> {
    use oxigraph::io::{RdfFormat, RdfParser};
    let temp = oxigraph::store::Store::new().map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
    let parser = RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri("urn:onto-studio:validate:")
        .unwrap_or_else(|_| RdfParser::from_format(RdfFormat::Turtle));
    temp.load_from_reader(parser, ttl.as_bytes())
        .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
    Ok(temp)
}

/// 从 Turtle 提取本体 IRI 和版本（找 `?o a owl:Ontology` 的 `?o` + `owl:versionInfo`）。
fn extract_ontology_meta(ttl: &str, _warnings: &mut Vec<String>) -> (Option<String>, Option<String>) {
    let Ok(temp) = parse_ttl_to_memory(ttl) else {
        return (None, None);
    };
    let iri = run_sparql_solutions(
        &temp,
        "SELECT ?o WHERE { ?o <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Ontology> . } LIMIT 1",
    )
    .into_iter()
    .next()
    .and_then(|row| row.get("o").map(|v| strip_iri_brackets(&v.to_string()).to_string()));
    let version = iri.as_ref().and_then(|iri| {
        run_sparql_solutions(
            &temp,
            &format!("SELECT ?v WHERE {{ <{iri}> <http://www.w3.org/2002/07/owl#versionInfo> ?v . }} LIMIT 1"),
        )
        .into_iter()
        .next()
        .and_then(|row| row.get("v").map(|v| strip_term(&v.to_string()).to_string()))
    });
    (iri, version)
}

// ════════════════════════════════════════════════════════════════
// 内部：7+1 语义规范校验子项（纯函数，操作内存 oxigraph Store）
// ════════════════════════════════════════════════════════════════

/// 第 2 类：类必须 `a owl:Class`（非 `rdfs:Class`）。
fn check_class_uses_owl_class(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    // 用全 IRI 避免 SPARQL 前缀声明问题
    let q = "SELECT ?c WHERE {\n\
        ?c <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2000/01/rdf-schema#Class> .\n\
        FILTER NOT EXISTS { ?c <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> }\n\
        FILTER(isIRI(?c))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(v) = row.get("c") {
            errors.push(format!(
                "类 '{v}' 声明为 rdfs:Class，应为 owl:Class（第 2 类约束）"
            ));
        }
    }
}

/// 第 1 类：DatatypeProperty 与 ObjectProperty 不可混用。
fn check_property_not_mixed(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?p WHERE {\n\
        ?p <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> .\n\
        ?p <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> .\n\
        FILTER(isIRI(?p))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(v) = row.get("p") {
            errors.push(format!(
                "属性 '{v}' 同时声明为 DatatypeProperty 和 ObjectProperty（第 1 类约束，禁混用）"
            ));
        }
    }
}

/// IRI 局部名规范：类 PascalCase，属性 camelCase。
fn check_iri_naming(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?s ?t WHERE {\n\
        { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> . BIND(\"Class\" AS ?t) }\n\
        UNION { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> . BIND(\"Prop\" AS ?t) }\n\
        UNION { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> . BIND(\"Prop\" AS ?t) }\n\
        FILTER(isIRI(?s))\n    }";
    for row in run_sparql_solutions(store, q) {
        let Some(s) = row.get("s") else { continue };
        let Some(t) = row.get("t") else { continue };
        let iri = strip_iri_brackets(s).to_string();
        let local = extract_local_name(&iri);
        if local.is_empty() {
            continue;
        }
        let kind = strip_term(t).to_string();
        if kind == "Class" && !is_pascal_case(&local) {
            errors.push(format!(
                "类 IRI 局部名 '{local}' 不符合 PascalCase 规范（应为如 Supplier）"
            ));
        } else if kind == "Prop" && !is_camel_case(&local) {
            errors.push(format!(
                "属性 IRI 局部名 '{local}' 不符合 camelCase 规范（应为如 supplierId）"
            ));
        }
    }
}

/// 第 3 类：中文标签必须带 `@zh` 语言标签。
fn check_chinese_labels_have_lang(
    store: &oxigraph::store::Store,
    warnings: &mut Vec<String>,
) {
    // 找 rdfs:label / rdfs:comment 的中文字面量但无语言标签
    let q = "SELECT ?s WHERE {\n\
        { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l . }\n\
        UNION { ?s <http://www.w3.org/2000/01/rdf-schema#comment> ?l . }\n\
        FILTER(isLiteral(?l) && lang(?l) = \"\")\n\
        FILTER(regex(str(?l), \"[\\\\u4e00-\\\\u9fff]\"))\n\
    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(s) = row.get("s") {
            warnings.push(format!(
                "实体 '{s}' 的中文标签/说明未带 @zh 语言标签（第 3 类约束）"
            ));
        }
    }
}

/// 第 4 类：SWRL 规则必须用 swrl:Imp 标准词汇。
fn check_swrl_uses_standard_vocab(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?r WHERE {\n\
        { ?r <http://www.w3.org/2003/11/swrl#body> ?b . }\n\
        UNION { ?r <http://www.w3.org/2003/11/swrl#head> ?h . }\n\
        FILTER(isIRI(?r))\n\
        FILTER NOT EXISTS { ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2003/11/swrl#Imp> }\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(r) = row.get("r") {
            errors.push(format!(
                "规则 '{r}' 用了 swrl:body/head 但未声明 a swrl:Imp（第 4 类约束）"
            ));
        }
    }
}

/// 第 2 类：rdfs:subClassOf 不可循环继承。
fn check_subclass_no_cycle(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?c WHERE {\n\
        ?c <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?c .\n\
        FILTER(isIRI(?c))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(c) = row.get("c") {
            errors.push(format!(
                "类 '{c}' 存在 rdfs:subClassOf 循环继承（第 2 类约束）"
            ));
        }
    }
}

/// 第 2 类（补充）：rdfs:subClassOf 的主语必须是 owl:Class。
///
/// oxigraph 无推理，子类若未显式声明 `a owl:Class`，`?c a owl:Class` 查询会漏掉它，
/// 破坏分层语义。该校验防止 29 句话模板里的 `:X rdfs:subClassOf :Y` 缺声明。
fn check_subclass_subject_is_class(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?sub WHERE {\n\
        ?sub <http://www.w3.org/2000/01/rdf-schema#subClassOf> ?sup .\n\
        FILTER(isIRI(?sub))\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2000/01/rdf-schema#Class> }\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(sub) = row.get("sub") {
            let iri = strip_iri_brackets(sub);
            errors.push(format!(
                "子类 '{iri}' 用了 rdfs:subClassOf 但未声明 a owl:Class（第 2 类约束，子类必须显式声明为类——oxigraph 无推理，未声明会从类查询中遗漏）"
            ));
        }
    }
}

/// 第 1 类（补充）：rdfs:subPropertyOf 的主语必须是 rdf:Property 或其子类。
///
/// 同理 subClassOf——subPropertyOf 的主语必须是属性，否则分层语义断裂。
fn check_subproperty_subject_is_property(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?sub WHERE {\n\
        ?sub <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> ?sup .\n\
        FILTER(isIRI(?sub))\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#AnnotationProperty> }\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(sub) = row.get("sub") {
            let iri = strip_iri_brackets(sub);
            errors.push(format!(
                "子属性 '{iri}' 用了 rdfs:subPropertyOf 但未声明为 rdf:Property/owl:DatatypeProperty/owl:ObjectProperty（第 1 类约束）"
            ));
        }
    }
}

// ════════════════════════════════════════════════════════════════
// 内部辅助：SPARQL 执行 + IRI 处理
// ════════════════════════════════════════════════════════════════

/// 跑 SPARQL SELECT，返回 Vec<HashMap<变量名, 值字符串>>。
fn run_sparql_solutions(
    store: &oxigraph::store::Store,
    query: &str,
) -> Vec<std::collections::HashMap<String, String>> {
    use oxigraph::sparql::{QueryResults, SparqlEvaluator};
    let parsed = match SparqlEvaluator::new().parse_query(query) {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };
    let results = match parsed.on_store(store).execute() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let QueryResults::Solutions(sols) = results else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for sol in sols {
        let Ok(sol) = sol else { continue };
        let mut map = std::collections::HashMap::new();
        for (k, v) in sol.iter() {
            // Variable.to_string() 输出 "?c"，去掉问号前缀以 key="c"
            let key = k.to_string().trim_start_matches('?').to_string();
            map.insert(key, v.to_string());
        }
        out.push(map);
    }
    out
}

/// 从 IRI 字符串提取局部名（# 后或 / 后最后一段）。
fn extract_local_name(iri: &str) -> String {
    let s = strip_iri_brackets(iri);
    s.rsplit(['#', '/']).next().unwrap_or("").to_string()
}

/// 去掉 oxigraph Term::to_string() 输出的尖括号（`<iri>` → `iri`）。
fn strip_iri_brackets(s: &str) -> &str {
    let s = s.strip_prefix('<').unwrap_or(s);
    s.strip_suffix('>').unwrap_or(s)
}

/// 去掉 oxigraph Term::to_string() 输出的封装：IRI 去 `<>`，字面量去引号。
fn strip_term(s: &str) -> &str {
    let s = strip_iri_brackets(s);
    // 字面量 "xxx" → xxx（仅去首尾一对引号，内部转义不动）
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// 当前 unix 毫秒时间戳。
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 生成 UUID v4（simple 格式，无连字符）——对齐 Palantir `new_id`。
fn new_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// PascalCase：首字母大写，仅字母数字。
fn is_pascal_case(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// camelCase：首字母小写，仅字母数字。
fn is_camel_case(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 给 SPARQL 查询注入常用 W3C 前缀声明（若用户查询用了缩写但没声明 PREFIX）。
/// 仅当查询不含 `PREFIX` 关键字时注入（避免重复声明报错）。
fn inject_common_prefixes(sparql: &str) -> String {
    if sparql.contains("PREFIX ") || sparql.contains("PREFIX	") {
        return sparql.to_string();
    }
    format!(
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n         PREFIX owl: <http://www.w3.org/2002/07/owl#>\n         PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n         PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n         PREFIX swrl: <http://www.w3.org/2003/11/swrl#>\n         PREFIX rdfg: <http://www.w3.org/2009/pointg#>\n         {sparql}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ttl() -> &'static str {
        r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/ontology/test#> .

:TestOntology a owl:Ontology ;
    rdfs:label "测试本体"@zh ;
    owl:versionInfo "1.0.0" .

:Supplier a owl:Class ;
    rdfs:label "供应商"@zh ;
    rdfs:comment "采购供应商 [confirmed]"@zh .

:supplierId a owl:DatatypeProperty ;
    rdfs:domain :Supplier ;
    rdfs:range xsd:string ;
    rdfs:label "供应商编号"@zh .

:supplies a owl:ObjectProperty ;
    rdfs:domain :Supplier ;
    rdfs:range :Supplier ;
    rdfs:label "供货"@zh .
"#
    }

    #[test]
    fn validate_accepts_clean_ttl() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(sample_ttl()).unwrap();
        assert!(r.errors.is_empty(), "errors should be empty: {:?}", r.errors);
        assert!(r.triple_count > 0);
    }

    #[test]
    fn validate_rejects_rdfs_class() {
        let bad = r#"@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:Foo a rdfs:Class .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("owl:Class")));
    }

    #[test]
    fn validate_rejects_mixed_property() {
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <https://example.org/ontology/test#> .
:p a owl:DatatypeProperty, owl:ObjectProperty .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("混用")));
    }

    #[test]
    fn validate_rejects_bad_iri_naming() {
        let bad = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix : <https://example.org/ontology/test#> .
:supplier a owl:Class .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(bad).unwrap();
        assert!(r.errors.iter().any(|e| e.contains("PascalCase")));
    }

    #[test]
    fn import_then_export_roundtrip() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.import_ttl(sample_ttl(), true).unwrap();
        assert!(r.errors.is_empty(), "import errors: {:?}", r.errors);
        assert_eq!(
            r.ontology_iri,
            "https://example.org/ontology/test#TestOntology"
        );
        assert!(r.triple_count > 0);
        let exported = store
            .export_ttl("https://example.org/ontology/test#TestOntology")
            .unwrap();
        assert!(exported.contains("owl:Class"));
    }

    #[test]
    fn import_overwrite_false_skips_existing() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let r = store.import_ttl(sample_ttl(), false).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("已跳过")));
    }

    #[test]
    fn list_and_delete_ontology() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let list = store.list_ontologies().unwrap();
        assert_eq!(list.len(), 1);
        assert!(store
            .delete_ontology("https://example.org/ontology/test#TestOntology")
            .unwrap());
        assert!(store.list_ontologies().unwrap().is_empty());
    }

    #[test]
    fn query_sparql_works() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let json = store
            .query_sparql(
                "https://example.org/ontology/test#TestOntology",
                "SELECT ?c WHERE { ?c a owl:Class . }",
            )
            .unwrap();
        assert!(json.contains("TestOntology") || json.contains("Supplier"));
    }

    // ── 问题 1 回归：subClassOf 子类缺 a owl:Class 必须被 validate 拦住 ──
    fn sample_ttl_with_subclass_missing_decl() -> &'static str {
        r#"@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix : <https://example.org/ontology/test#> .

:TestOntology a owl:Ontology ;
    rdfs:label "测试本体"@zh ;
    owl:versionInfo "1.0.0" .

:Supplier a owl:Class ;
    rdfs:label "供应商"@zh .

# Anchor 用了 subClassOf 但缺 a owl:Class——应被校验拦住
:Anchor rdfs:subClassOf :Supplier ;
    rdfs:label "主播"@zh .
"#
    }

    #[test]
    fn validate_rejects_subclass_missing_owl_class() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(sample_ttl_with_subclass_missing_decl()).unwrap();
        assert!(
            r.errors.iter().any(|e| e.contains("subClassOf") && e.contains("a owl:Class")),
            "应报子类缺 a owl:Class，实际 errors: {:?}",
            r.errors
        );
    }

    #[test]
    fn validate_accepts_subclass_with_owl_class() {
        // 同样有 subClassOf 但主语声明了 a owl:Class——应通过
        let ttl = r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/test#> .
:TestOntology a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh .
:Anchor a owl:Class ; rdfs:subClassOf :Supplier ; rdfs:label "主播"@zh .
"#;
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.validate_ttl(ttl).unwrap();
        assert!(r.errors.is_empty(), "errors 应为空: {:?}", r.errors);
    }

    // ── charter / changelog 闭环测试 ──
    #[test]
    fn charter_set_get_roundtrip() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        // 初始无 charter → 空结构体
        let empty = store.get_charter(iri).unwrap();
        assert!(empty.business_scenario.is_empty());
        // set 后 get 应一致
        let c = TtlCharter {
            business_scenario: "门店销售场景".to_string(),
            business_essence: "覆盖门店潜客试驾订单".to_string(),
            design_intent: "够用且可扩展".to_string(),
            invariants: "客户编号唯一".to_string(),
            updated_by: "agent".to_string(),
            updated_at: 0,
        };
        store.set_charter(iri, &c).unwrap();
        let got = store.get_charter(iri).unwrap();
        assert_eq!(got.business_scenario, "门店销售场景");
        assert_eq!(got.business_essence, "覆盖门店潜客试驾订单");
        assert_eq!(got.design_intent, "够用且可扩展");
        assert_eq!(got.invariants, "客户编号唯一");
        assert_eq!(got.updated_by, "agent");
        assert!(got.updated_at > 0);
    }

    #[test]
    fn charter_set_twice_overrides() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let c1 = TtlCharter { business_scenario: "v1".to_string(), ..Default::default() };
        store.set_charter(iri, &c1).unwrap();
        let c2 = TtlCharter { business_scenario: "v2".to_string(), ..Default::default() };
        store.set_charter(iri, &c2).unwrap();
        let got = store.get_charter(iri).unwrap();
        assert_eq!(got.business_scenario, "v2");
    }

    #[test]
    fn charter_for_unknown_ontology_returns_empty() {
        let store = TtlStore::open_in_memory().unwrap();
        let empty = store.get_charter("https://example.org/nonexistent#Ont").unwrap();
        assert!(empty.business_scenario.is_empty());
    }

    #[test]
    fn changelog_revision_increments() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let rev1 = store.commit_change(iri, "初始导入", "冷启动建模", "{}", None, "agent").unwrap();
        let rev2 = store.commit_change(iri, "补子类声明", "加 a owl:Class", "{}", None, "agent").unwrap();
        assert_eq!(rev1, 1);
        assert_eq!(rev2, 2);
        let log = store.list_changelog(iri).unwrap();
        assert_eq!(log.len(), 2);
        // 倒序：最新在前
        assert_eq!(log[0].revision, 2);
        assert_eq!(log[0].title, "补子类声明");
        assert_eq!(log[1].revision, 1);
        assert_eq!(log[1].title, "初始导入");
    }

    #[test]
    fn changelog_rejects_unknown_ontology() {
        let store = TtlStore::open_in_memory().unwrap();
        let r = store.commit_change(
            "https://example.org/nonexistent#Ont",
            "t", "b", "{}", None, "agent",
        );
        assert!(matches!(r, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn changelog_enforces_size_limits() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        let big_body = "x".repeat(501);
        let r = store.commit_change(iri, "t", &big_body, "{}", None, "agent");
        assert!(r.is_err(), "title+body > 500 应被拒");
    }

    #[test]
    fn delete_ontology_cascades_charter_and_changelog() {
        let store = TtlStore::open_in_memory().unwrap();
        store.import_ttl(sample_ttl(), true).unwrap();
        let iri = "https://example.org/ontology/test#TestOntology";
        // 写 charter + changelog
        store.set_charter(iri, &TtlCharter { business_scenario: "s".to_string(), ..Default::default() }).unwrap();
        store.commit_change(iri, "t", "b", "{}", None, "agent").unwrap();
        // 删本体 → charter/changelog 应级联删
        assert!(store.delete_ontology(iri).unwrap());
        assert!(store.get_charter(iri).unwrap().business_scenario.is_empty(),
            "删本体后 charter 应级联删");
        assert!(store.list_changelog(iri).unwrap().is_empty(),
            "删本体后 changelog 应级联删");
    }
}
