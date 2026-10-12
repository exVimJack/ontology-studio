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
#[cfg(test)]
use crate::ttl_edit::TtlPropertyEdit;

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
    /// 结构化 issue 列表（向后兼容：errors/warnings 保留，issues 是结构化形态）
    #[serde(default)]
    pub issues: Vec<crate::validation::ValidationIssue>,
    /// L4.3 多轮评审状态（默认无；review_ontology 启动后填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<crate::validation::ValidationReview>,
}

impl TtlValidation {
    /// 构造空的校验结果（全字段默认）。
    pub fn empty() -> Self {
        Self {
            triple_count: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            issues: Vec::new(),
            review: None,
        }
    }
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
    /// 设计宪章是否已填写（手工导入的本体可能为 false——agent 迭代前应知会用户补全）
    pub has_charter: bool,
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
    pub(crate) conn: Mutex<Connection>,
    /// 落库变更回调（平台无关）：import 成功后触发。
    /// 由装配层（src-tauri）注入，用于通知前端刷新。
    on_change: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}


// ── 职责切分子模块（纯移动重构：逻辑零改动）──
mod charter;
mod crud;
#[cfg(test)]
mod tests;
mod validate;

pub use charter::extract_charter_predicates;
// agent-core 的 TTL 工具组跨 crate 引用 extract_ontology_meta（ontology_ttl_tools.rs）；
// src-tauri 的 coverage 命令跨 crate 引用 parse_ttl_to_memory（src-tauri/src/commands/validation.rs）。
pub use validate::extract_ontology_meta;
pub use validate::parse_ttl_to_memory;
pub(crate) use validate::*;

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
            ON ontology_ttl_changelog(ontology_iri, revision DESC);

        -- ═══ 证据链源登记（docs/EVIDENCE-CHAIN.md L1）═══
        -- 与 Palantir `init_schema` 中同表同构：生产环境两个 store 装配层共用同一 db 文件，
        -- 任一侧先建表即可；这里再建一次幂等（保障 TtlStore 单独使用时表存在）。
        CREATE TABLE IF NOT EXISTS evidence_sources (
            source_id         TEXT PRIMARY KEY,
            ontology_api_name TEXT NOT NULL DEFAULT '',
            kind              TEXT NOT NULL,
            title             TEXT NOT NULL,
            content           TEXT NOT NULL,
            content_hash      TEXT NOT NULL,
            origin_ref        TEXT NOT NULL DEFAULT '',
            created_at        INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_evidence_sources_ont ON evidence_sources(ontology_api_name);

        -- 元素级证据索引（与 schema.rs 同构；TtlStore 单独使用时保障建表）
        CREATE TABLE IF NOT EXISTS element_evidence (
            id           TEXT PRIMARY KEY,
            standard     TEXT NOT NULL,
            ontology_key TEXT NOT NULL,
            element_type TEXT NOT NULL,
            element_id   TEXT NOT NULL,
            source_id    TEXT NOT NULL,
            quote        TEXT NOT NULL,
            span         TEXT NOT NULL DEFAULT '',
            role         TEXT NOT NULL,
            matched_by   TEXT NOT NULL DEFAULT '',
            created_at   INTEGER NOT NULL,
            UNIQUE(standard, ontology_key, element_type, element_id, source_id, quote)
        );
        CREATE INDEX IF NOT EXISTS idx_ee_lookup ON element_evidence(standard, ontology_key);

        -- ═══ 校验对抗（VALIDATION-DESIGN.md v0.3.1，与 schema.rs 同构）═══
        -- TtlStore 单独使用时保障建表；生产环境与 Palantir store 共用 db 文件幂等。
        CREATE TABLE IF NOT EXISTS validation_issues (
            id                     TEXT PRIMARY KEY,
            ontology_iri           TEXT NOT NULL,
            validation_id          TEXT NOT NULL,
            layer                  TEXT NOT NULL,
            source                 TEXT NOT NULL DEFAULT 'rules',
            severity               TEXT NOT NULL,
            rule                   TEXT NOT NULL,
            message                TEXT NOT NULL,
            target                 TEXT,
            evidence_source_id     TEXT,
            evidence_quote         TEXT,
            principle_source       TEXT,
            principle_ref          TEXT,
            suggestion             TEXT,
            status                 TEXT NOT NULL DEFAULT 'open',
            a_action               TEXT,
            dispute_argument       TEXT,
            dispute_counter_source TEXT,
            dispute_counter_quote  TEXT,
            dispute_resolved_by    TEXT,
            dispute_verdict        TEXT,
            created_at             INTEGER NOT NULL,
            updated_at             INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_vi_ont ON validation_issues(ontology_iri);
        CREATE INDEX IF NOT EXISTS idx_vi_status ON validation_issues(ontology_iri, status);
        CREATE INDEX IF NOT EXISTS idx_vi_source ON validation_issues(ontology_iri, source);

        CREATE TABLE IF NOT EXISTS validation_reviews (
            id            TEXT PRIMARY KEY,
            ontology_iri  TEXT NOT NULL,
            round         INTEGER NOT NULL DEFAULT 1,
            max_rounds    INTEGER NOT NULL DEFAULT 2,
            b_report      TEXT,
            a_revision    TEXT,
            a_reasoning   TEXT,
            b_verdict     TEXT,
            step_ms_b     INTEGER NOT NULL DEFAULT 0,
            step_ms_a     INTEGER NOT NULL DEFAULT 0,
            step_ms_re    INTEGER NOT NULL DEFAULT 0,
            status        TEXT NOT NULL DEFAULT 'in_progress',
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_vr_ont ON validation_reviews(ontology_iri);",
    )?;
    // 幂等迁移（统一发现池 v0.7，与 schema.rs migrate_validation_unified_pool 同构）：
    // 旧库（含 schema.rs 建的表）补 source/a_action/b_verdict/step_ms 列。
    // TtlStore 与 Palantir store 共用 db 文件时，任一侧先跑迁移均幂等。
    crate::schema::ensure_columns(
        conn,
        "validation_issues",
        &[
            ("source", "TEXT NOT NULL DEFAULT 'rules'"),
            ("a_action", "TEXT"),
        ],
    )?;
    conn.execute_batch(
        "UPDATE validation_issues SET source = 'rules' WHERE source IS NULL OR source = '';
         CREATE INDEX IF NOT EXISTS idx_vi_source ON validation_issues(ontology_iri, source);",
    )?;
    crate::schema::ensure_columns(
        conn,
        "validation_reviews",
        &[
            ("b_verdict", "TEXT"),
            ("step_ms_b", "INTEGER NOT NULL DEFAULT 0"),
            ("step_ms_a", "INTEGER NOT NULL DEFAULT 0"),
            ("step_ms_re", "INTEGER NOT NULL DEFAULT 0"),
        ],
    )?;
    Ok(())
}

/// 跑 SPARQL SELECT，返回 Vec<HashMap<变量名, 值字符串>>。
pub(crate) fn run_sparql_solutions(
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

/// 去掉 oxigraph Term::to_string() 输出的尖括号（`<iri>` → `iri`）。
fn strip_iri_brackets(s: &str) -> &str {
    let s = s.strip_prefix('<').unwrap_or(s);
    s.strip_suffix('>').unwrap_or(s)
}

/// 去掉 oxigraph Term::to_string() 输出的封装：IRI 去 `<>`，字面量去引号。
pub(crate) fn strip_term(s: &str) -> &str {
    let s = strip_iri_brackets(s);
    // 字面量 "xxx" → xxx（仅去首尾一对引号，内部转义不动）
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// 当前 unix 毫秒时间戳。
pub(crate) fn now_ms() -> i64 {
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
