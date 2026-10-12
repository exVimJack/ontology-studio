//! Schema 初始化（对齐 Gaia `core/models/ontology.py` + `datasource.py` 表设计）。
//!
//! 只复刻本体定义层表族，砍掉 Gaia 运行时/权限/管线层（object_state/outbox/
//! pipelines/sync_tasks/permissions 等）——离线建模不需要。
//!
//! 约束策略：Rust 侧（naming.rs + data_type.rs + import 校验）是真相源，
//! DB 层 UNIQUE/FK/CHECK 做兜底。api_name pattern 不在 DB 做（GLOB 不等价正则），
//! 由 Rust 预校验。

use rusqlite::Connection;

use crate::error::StoreResult;

/// 初始化全部表 schema（幂等，可重复调用）。
pub fn init_schema(conn: &Connection) -> StoreResult<()> {
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(
        r#"
        -- ═══ 本体容器 ═══
        CREATE TABLE IF NOT EXISTS ontologies (
            id           TEXT PRIMARY KEY,            -- UUID v4
            api_name     TEXT NOT NULL UNIQUE,        -- PascalCase, 本体级唯一
            display_name TEXT NOT NULL,
            description  TEXT NOT NULL DEFAULT '',
            status       TEXT NOT NULL DEFAULT 'ACTIVE',
            created_at   INTEGER NOT NULL,            -- unix ms
            updated_at   INTEGER NOT NULL
        );

        -- ═══ ObjectType ═══
        CREATE TABLE IF NOT EXISTS object_types (
            id                       TEXT PRIMARY KEY,             -- UUID v4
            ontology_id              TEXT NOT NULL,
            api_name                 TEXT NOT NULL,                -- PascalCase
            display_name             TEXT NOT NULL,
            description              TEXT NOT NULL DEFAULT '',
            primary_key              TEXT NOT NULL,                -- 属性 api_name（推导自 is_primary_key）
            title_property           TEXT NOT NULL DEFAULT '',     -- 属性 api_name（可空串）
            storage_type             TEXT NOT NULL CHECK (storage_type IN ('MANAGED','VIRTUAL')),
            visibility               TEXT NOT NULL DEFAULT 'NORMAL' CHECK (visibility IN ('NORMAL','PROMINENT','HIDDEN')),
            status                   TEXT NOT NULL DEFAULT 'ACTIVE' CHECK (status IN ('ACTIVE','ENDORSED','EXPERIMENTAL','DEPRECATED')),
            backing_dataset_api_name TEXT,                          -- 可空（未绑定时 NULL）
            capabilities             TEXT NOT NULL DEFAULT '{}',    -- JSON: {graph_indexing_enabled, geotime_indexing_enabled}
            created_at               INTEGER NOT NULL,
            updated_at               INTEGER NOT NULL,
            UNIQUE(ontology_id, api_name),
            FOREIGN KEY (ontology_id) REFERENCES ontologies(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_object_types_ontology ON object_types(ontology_id);

        -- ═══ PropertyDef（嵌在 ObjectType 下）═══
        CREATE TABLE IF NOT EXISTS properties (
            id                        TEXT PRIMARY KEY,             -- UUID v4
            object_type_id            TEXT NOT NULL,
            api_name                  TEXT NOT NULL,                -- camelCase
            display_name              TEXT NOT NULL,
            description               TEXT NOT NULL DEFAULT '',
            data_type                 TEXT NOT NULL,                -- DataType 枚举（Rust 校验 + 应用层兜底）
            is_primary_key            INTEGER NOT NULL DEFAULT 0,   -- 0/1
            is_title_property         INTEGER NOT NULL DEFAULT 0,   -- 0/1
            searchable                INTEGER NOT NULL DEFAULT 1,   -- 0/1
            backing_dataset_api_name  TEXT,
            backing_catalog           TEXT,
            backing_schema            TEXT,
            backing_table             TEXT,
            backing_column            TEXT,
            vector_config             TEXT,                          -- JSON，非 VECTOR 属性为 NULL
            created_at                INTEGER NOT NULL,
            updated_at                INTEGER NOT NULL,
            UNIQUE(object_type_id, api_name),
            FOREIGN KEY (object_type_id) REFERENCES object_types(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_properties_ot ON properties(object_type_id);

        -- ═══ LinkTypeDef（顶层，用 api_name 引用 ObjectType）═══
        CREATE TABLE IF NOT EXISTS link_types (
            id                              TEXT PRIMARY KEY,         -- UUID v4
            ontology_id                     TEXT NOT NULL,
            api_name                        TEXT NOT NULL,            -- camelCase
            display_name                    TEXT NOT NULL,
            description                     TEXT NOT NULL DEFAULT '',
            source_object_type_api_name     TEXT NOT NULL,            -- PascalCase（引用 OT.api_name）
            target_object_type_api_name     TEXT NOT NULL,            -- PascalCase
            foreign_key_property_api_name   TEXT,                     -- camelCase，可空
            cardinality                     TEXT NOT NULL CHECK (cardinality IN ('ONE','MANY')),
            weight_property                 TEXT,
            temporal                        INTEGER NOT NULL DEFAULT 0,
            created_at                      INTEGER NOT NULL,
            updated_at                      INTEGER NOT NULL,
            UNIQUE(ontology_id, api_name),
            FOREIGN KEY (ontology_id) REFERENCES ontologies(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_link_types_ontology ON link_types(ontology_id);

        -- ═══ ActionType ═══
        CREATE TABLE IF NOT EXISTS action_types (
            id                              TEXT PRIMARY KEY,         -- UUID v4
            ontology_id                     TEXT NOT NULL,
            api_name                        TEXT NOT NULL,            -- camelCase
            display_name                    TEXT NOT NULL,
            description                     TEXT NOT NULL DEFAULT '',
            affected_object_type_api_name   TEXT NOT NULL,            -- PascalCase（引用 OT.api_name）
            parameters                      TEXT NOT NULL DEFAULT '[]',    -- JSON 数组
            rules                           TEXT NOT NULL DEFAULT '[]',    -- JSON 数组
            submission_criteria             TEXT NOT NULL DEFAULT '[]',    -- JSON 数组
            effects                         TEXT NOT NULL DEFAULT '[]',    -- JSON 数组
            ontology_rules                  TEXT NOT NULL DEFAULT '[]',    -- JSON 数组
            risk_level                      TEXT NOT NULL DEFAULT 'low' CHECK (risk_level IN ('low','medium','high')),
            operation_kind                  TEXT NOT NULL DEFAULT 'mixed' CHECK (operation_kind IN ('create','update','delete','mixed')),
            batch_enabled                   INTEGER NOT NULL DEFAULT 0,
            created_at                      INTEGER NOT NULL,
            updated_at                      INTEGER NOT NULL,
            UNIQUE(ontology_id, api_name),
            FOREIGN KEY (ontology_id) REFERENCES ontologies(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_action_types_ontology ON action_types(ontology_id);

        -- ═══ ObjectTypeGroup（ADR-022 纯分类原语）+ 成员关联 ═══
        CREATE TABLE IF NOT EXISTS object_type_groups (
            id           TEXT PRIMARY KEY,             -- UUID v4
            ontology_id  TEXT NOT NULL,
            api_name     TEXT NOT NULL,                -- PascalCase
            display_name TEXT NOT NULL,
            description  TEXT NOT NULL DEFAULT '',
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL,
            UNIQUE(ontology_id, api_name),
            FOREIGN KEY (ontology_id) REFERENCES ontologies(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_ot_groups_ontology ON object_type_groups(ontology_id);

        CREATE TABLE IF NOT EXISTS object_type_group_members (
            group_id        TEXT NOT NULL,
            object_type_id  TEXT NOT NULL,
            ontology_id     TEXT NOT NULL,
            created_at      INTEGER NOT NULL,
            PRIMARY KEY (group_id, object_type_id),
            FOREIGN KEY (group_id) REFERENCES object_type_groups(id) ON DELETE CASCADE,
            FOREIGN KEY (object_type_id) REFERENCES object_types(id) ON DELETE CASCADE,
            FOREIGN KEY (ontology_id) REFERENCES ontologies(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_otgm_object_type ON object_type_group_members(object_type_id);

        -- ═══ DatasetGovernance（决策 10 修订：数据集/数据源按本体隔离，
        -- 加 ontology_api_name 列 + (ontology_api_name, api_name) 联合唯一）
        -- 导出时按本体过滤，导入时由 payload 所属本体回填——payload 不带该字段（与 Gaia 兼容）。
        -- ═══
        CREATE TABLE IF NOT EXISTS datasets (
            id                        TEXT PRIMARY KEY,             -- UUID v4
            ontology_api_name         TEXT NOT NULL,                -- 所属本体 PascalCase（隔离键）
            api_name                  TEXT NOT NULL,                -- snake_case，本体内唯一
            display_name              TEXT NOT NULL DEFAULT '',
            storage_location          TEXT NOT NULL DEFAULT '',     -- VIRTUAL 三段式 catalog.schema.table
            partition_config          TEXT,                          -- JSON，可空
            source_dataset_api_name   TEXT,                          -- 同本体内引用
            data_source_api_name      TEXT,                          -- 同本体内引用
            kind                      TEXT NOT NULL DEFAULT 'MANAGED' CHECK (kind IN ('MANAGED','VIRTUAL')),
            is_view                   INTEGER NOT NULL DEFAULT 0,
            created_at                INTEGER NOT NULL,
            updated_at                INTEGER NOT NULL,
            UNIQUE(ontology_api_name, api_name),
            FOREIGN KEY (ontology_api_name) REFERENCES ontologies(api_name) ON DELETE CASCADE
        );
        -- idx_datasets_ontology 由迁移函数建（引用 ontology_api_name 列）

        -- ═══ DataSource（同数据集，按本体隔离）
        CREATE TABLE IF NOT EXISTS data_sources (
            id                    TEXT PRIMARY KEY,             -- UUID v4
            ontology_api_name     TEXT NOT NULL,                -- 所属本体 PascalCase（隔离键）
            api_name              TEXT NOT NULL,                -- snake_case，本体内唯一
            display_name          TEXT NOT NULL,
            description           TEXT NOT NULL DEFAULT '',
            connector_type        TEXT NOT NULL,
            connector_config      TEXT NOT NULL DEFAULT '{}',   -- JSON，敏感字段用 *** 占位
            credential_id         TEXT,
            created_at            INTEGER NOT NULL,
            updated_at            INTEGER NOT NULL,
            UNIQUE(ontology_api_name, api_name),
            FOREIGN KEY (ontology_api_name) REFERENCES ontologies(api_name) ON DELETE CASCADE
        );
        -- idx_data_sources_ontology 由迁移函数建（引用 ontology_api_name 列）

        -- ═══ Credential（通常不产出，占位用）═══
        CREATE TABLE IF NOT EXISTS credentials (
            id              TEXT PRIMARY KEY,             -- UUID v4
            api_name        TEXT NOT NULL UNIQUE,         -- snake_case
            credential_type TEXT NOT NULL,
            secret_data     TEXT NOT NULL DEFAULT '{}',   -- JSON，必须占位
            created_at      INTEGER NOT NULL
        );

        -- ═══ 本体变更日志（git commit log 式：每次 import/delete 后留一条设计说明）═══
        -- 不存完整 payload 快照（太重），只存人可读的设计说明 + 机器摘要。
        -- revision 每本体从 1 递增（UNIQUE(ontology_api_name, revision) 保证）。
        CREATE TABLE IF NOT EXISTS ontology_changelog (
            id                TEXT PRIMARY KEY,           -- UUID v4
            ontology_api_name TEXT NOT NULL,             -- 所属本体 PascalCase
            revision          INTEGER NOT NULL,          -- 本体内递增序号
            title             TEXT NOT NULL,             -- 一句话标题（title+body ≤500 chars 在 Rust 侧校验）
            body              TEXT NOT NULL DEFAULT '',  -- 设计说明正文
            change_summary    TEXT NOT NULL DEFAULT '{}',-- JSON: {created/deleted/modified: [api_name...]}
            conversation_id   TEXT,                      -- 来源会话 id（可空，手工导入无）
            author            TEXT NOT NULL DEFAULT 'agent', -- 'agent' | 'user'
            created_at        INTEGER NOT NULL,          -- unix ms
            UNIQUE(ontology_api_name, revision)
        );
        CREATE INDEX IF NOT EXISTS idx_changelog_ont ON ontology_changelog(ontology_api_name, revision DESC);

        -- ═══ 本体设计宪章（不变点：业务场景 / 本质 / 设计意图 / 补充说明）═══
        -- 与 changelog（变化点）物理分离、语义分离、写入路径分离：
        --   - changelog 记「每次变更」——随历史增长，revision 递增
        --   - charter 记「业务本质说明」——不随历史变化，1:1 关联本体
        -- charter 由独立命令 set_ontology_charter 写入，不进 import 流程
        -- （import 的 upsert_ontology 不覆盖 charter，也不从 payload 读 charter）。
        -- 用 api_name（而非 ontology_id）作主键：ontology 行被 delete+recreate
        -- 时 id 会变（new_id()），但 api_name 稳定，charter 跟着 api_name 走不丢。
        -- 设计动机（决策：本体不变点）：
        --   1. 业务意图目标决定，遵循「够用且可扩展」——charter 记录建模的取舍边界
        --   2. 本体始终扮演「向 AI 说明业务本质」的角色——charter 是 AI 的结构化业务认知
        -- 四字段语义：
        --   business_scenario：业务场景（服务于什么业务目标、谁用、解决什么问题）
        --   business_essence：业务本质（核心业务对象/状态/关系/动态行为的一句话本质概括）
        CREATE TABLE IF NOT EXISTS ontology_charter (
            ontology_api_name  TEXT PRIMARY KEY,        -- 1:1 关联 ontologies.api_name
            business_scenario  TEXT NOT NULL DEFAULT '',
            business_essence   TEXT NOT NULL DEFAULT '',
            updated_at         INTEGER NOT NULL,
            updated_by         TEXT NOT NULL DEFAULT 'agent', -- 'agent' | 'user'
            FOREIGN KEY (ontology_api_name) REFERENCES ontologies(api_name) ON DELETE CASCADE
        );

        -- ═══ 证据链源登记（docs/EVIDENCE-CHAIN.md L1，决策24后续）═══
        -- 每个建模输入（场景描述/文档 chunk/DB schema 表列）先登记为可引用的源。
        -- content 是登记时刻快照（不可变）：用户事后改文档/删消息不影响已有引用的有效性。
        -- ontology_api_name 空串 = 全局共享（会话场景源先于本体建立时登记）。
        -- content_hash（FNV-1a 64）用于幂等登记判重：同 hash 跳过。
        CREATE TABLE IF NOT EXISTS evidence_sources (
            source_id         TEXT PRIMARY KEY,          -- 稳定 id：src-{uuid simple}
            ontology_api_name TEXT NOT NULL DEFAULT '',
            kind              TEXT NOT NULL,             -- 'scenario' | 'document' | 'db_table' | 'db_column'
            title             TEXT NOT NULL,             -- 人类可读出处：会话时间/文件名+chunk/库.表.列
            content           TEXT NOT NULL,             -- 源文本快照
            content_hash      TEXT NOT NULL,
            origin_ref        TEXT NOT NULL DEFAULT '',  -- document_id / dataset_id / conversation_message_id
            created_at        INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_evidence_sources_ont ON evidence_sources(ontology_api_name);

        -- ═══ 元素级证据索引（docs/EVIDENCE-CHAIN.md L4，决策24后续）═══
        -- Palantir 与 W3C TTL 两条路径的证据落库统一到本表：
        --   standard='palantir'：import 时从 payload evidence 字段写入；export 时回填（round-trip）
        --   standard='w3c'：import_ttl 时从 skos:editorialNote 注解解析写入（TTL 原文本身也存注解，双保险）
        -- ontology_key：palantir=本体 api_name；w3c=本体 IRI
        -- element_id：palantir=api_name；w3c=元素 IRI
        -- matched_by：闸门校验时的匹配等级（'exact' | 'normalized'；'w3c' 侧解析注解时重算）
        CREATE TABLE IF NOT EXISTS element_evidence (
            id           TEXT PRIMARY KEY,
            standard     TEXT NOT NULL,              -- 'palantir' | 'w3c'
            ontology_key TEXT NOT NULL,
            element_type TEXT NOT NULL,              -- palantir: object_type|property|link；w3c: class|datatype_property|object_property
            element_id   TEXT NOT NULL,
            source_id    TEXT NOT NULL,
            quote        TEXT NOT NULL,
            span         TEXT NOT NULL DEFAULT '',   -- JSON [start,end] 或空串
            role         TEXT NOT NULL,              -- 'direct' | 'inferred'
            matched_by   TEXT NOT NULL DEFAULT '',
            created_at   INTEGER NOT NULL,
            UNIQUE(standard, ontology_key, element_type, element_id, source_id, quote)
        );
        CREATE INDEX IF NOT EXISTS idx_ee_lookup ON element_evidence(standard, ontology_key);
        "#
    )?;

    // ═══ 校验对抗：issue 生命周期 + L4.3 多轮评审（VALIDATION-DESIGN.md v0.3.1）═══
    // validation_issues：一条 issue 一次行，status 为对抗生命周期
    //（open→disputed→accepted/overruled）；dispute 字段存储 A 的答辩（argument +
    // counterEvidence，counterEvidence 必须引用已登记 evidence_source 并经 match_quote
    // 机械匹配——裁决权在 B/人工，不在 A 自己）。
    // validation_reviews：L4.3 多轮状态机（round/max_rounds=2、b_report、a_revision、
    // a_reasoning 对 B 复核不可见、status=in_progress/resolved/escalated）。
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS validation_issues (
            id                     TEXT PRIMARY KEY,            -- 稳定 id：vi-{uuid simple}；review 来源为 "{review_id}:{B条目id}"
            ontology_iri           TEXT NOT NULL,               -- 归属本体 IRI
            validation_id          TEXT NOT NULL,               -- 同一次 validate 调用分组（review 来源 = review_id）
            layer                  TEXT NOT NULL,               -- 'L0'|'L1'|'L2'|'L2.5'|'L2.6'|'L3'|'L4'
            source                 TEXT NOT NULL DEFAULT 'rules', -- 'rules'|'review'（统一发现池 v0.7）
            severity               TEXT NOT NULL,               -- 'error'|'warning'|'suggestion'
            rule                   TEXT NOT NULL,               -- check_* 函数名；review 来源 'review:{dimension}'
            message                TEXT NOT NULL,
            target                 TEXT,                        -- 实体 IRI / Turtle 行号（可空）
            evidence_source_id     TEXT,                        -- FK→evidence_sources（证据引用）
            evidence_quote         TEXT,                        -- 证据原文（denormalize 免 JOIN）
            principle_source       TEXT,                        -- 'skill'|'charter'|'input'|'w3c'
            principle_ref          TEXT,
            suggestion             TEXT,
            status                 TEXT NOT NULL DEFAULT 'open', -- open|disputed|accepted|overruled
            a_action               TEXT,                        -- review 来源：A 的 accepted/rejected JSON（可空）
            dispute_argument       TEXT,
            dispute_counter_source TEXT,
            dispute_counter_quote  TEXT,
            dispute_resolved_by    TEXT,                        -- 'verifier'|'human'
            dispute_verdict        TEXT,                        -- 'overruled'|'accepted'
            created_at             INTEGER NOT NULL,
            updated_at             INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_vi_ont ON validation_issues(ontology_iri);
        CREATE INDEX IF NOT EXISTS idx_vi_status ON validation_issues(ontology_iri, status);
        -- idx_vi_source 依赖 source 列，存量库该列由 migrate_validation_unified_pool
        -- 的 ensure_columns 补齐，索引也移到那里建（在这里建会因旧库缺列直接失败）。

        CREATE TABLE IF NOT EXISTS validation_reviews (
            id            TEXT PRIMARY KEY,            -- 稳定 id：vr-{uuid simple}
            ontology_iri  TEXT NOT NULL,
            round         INTEGER NOT NULL DEFAULT 1,
            max_rounds    INTEGER NOT NULL DEFAULT 2,
            b_report      TEXT,                        -- B 的评审输出（结构化建议 JSON）
            a_revision    TEXT,                        -- A 修订后的 Turtle V2 全文
            a_reasoning   TEXT,                        -- A 的修订说明（B 复核不可见）
            b_verdict     TEXT,                        -- B 复核原始输出 JSON（verdict+remaining_issues）
            step_ms_b     INTEGER NOT NULL DEFAULT 0,  -- B 评审耗时 ms（0=未完成）
            step_ms_a     INTEGER NOT NULL DEFAULT 0,  -- A 修订耗时 ms（0=未完成）
            step_ms_re    INTEGER NOT NULL DEFAULT 0,  -- B 复核耗时 ms（0=未完成）
            status        TEXT NOT NULL DEFAULT 'in_progress', -- in_progress|resolved|escalated
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_vr_ont ON validation_reviews(ontology_iri);
        "#,
    )?;

    // 迁移：对齐 Gaia alembic `021f553a89b8_drop_direction_column_from_link_types`。
    // 历史 bug：早期本地开发版 schema.rs 曾在 link_types 定义
    // `direction TEXT NOT NULL CHECK (direction IN ('OUTGOING','INCOMING'))`，
    // 后续移除方向语义（FK 归属改由 source 侧声明 + foreign_key_property_api_name 表达），
    // 但 `CREATE TABLE IF NOT EXISTS` 对已存在的旧表是 no-op，不会改表结构——
    // 导致旧 .db 文件的 link_types 仍带 NOT NULL direction 列，而新代码 INSERT
    // 不写该列，SQLite 报 `NOT NULL constraint failed: link_types.direction`。
    // 此迁移幂等检测并 DROP 该残留列，使旧库与新 schema 对齐。
    drop_link_types_direction_column(conn)?;
    // 迁移：统一发现池 v0.7——validation_issues 加 source/a_action 列（review 来源
    // B 发现与规则发现并列同一生命周期），validation_reviews 加 b_verdict/环节耗时列。
    migrate_validation_unified_pool(conn)?;
    // 迁移：datasets / data_sources 加 ontology_api_name 列 + 联合唯一索引（决策 10 修订）。
    // 必须在主 execute_batch 之后、但引用 ontology_api_name 的索引语句已在迁移函数内建好。
    // 主 execute_batch 里不再建 idx_datasets_ontology / idx_data_sources_ontology
    // （这两个索引引用 ontology_api_name 列，旧库迁移前没有该列，会 panic）。
    migrate_datasets_data_sources_ontology_scoping(conn)?;
    Ok(())
}

/// 幂等迁移：datasets / data_sources 加 `ontology_api_name` 列与联合唯一索引，
/// **并删除旧库遗留的单列 `api_name` 全局唯一约束**（决策 10 修订前 schema 为
/// `api_name TEXT NOT NULL UNIQUE`，是表级单列约束，SQLite 表达为
/// `sqlite_autoindex_<table>_<n>` 且 `origin='u'`）。
///
/// ## 根因（用户实测 bug）
/// 旧库的 datasets/data_sources 表定义里 `api_name TEXT NOT NULL UNIQUE`，
/// SQLite 为此自动建了一个**单列全局唯一索引**（`sqlite_autoindex_datasets_2`），
/// 这个约束与决策 10 修订后的「按本体隔离」语义**直接冲突**：
/// 即使 `(ontology_api_name, api_name)` 联合唯一索引允许同 api_name 跨本体共存，
/// 旧的单列约束仍会先触发 `UNIQUE constraint failed: datasets.api_name`，
/// 导致导入新本体（其 dataset api_name 恰好与旧库遗留全局行同名，如 dealership/
/// lead/user）时全部失败——而 preview 只查联合唯一索引，故 preview 说 create、
/// 实际 INSERT 却失败。
///
/// SQLite 不支持 `ALTER TABLE DROP CONSTRAINT`，必须用标准 12 步表重建法去掉
/// 列级 UNIQUE：建新表（无单列 UNIQUE，只有联合 UNIQUE）→ 拷数据 → DROP 旧表 →
/// 重命名。本迁移幂等：只有当检测到旧的单列 `api_name` 唯一约束存在时才重建。
///
/// 旧库遗留的全局行（`ontology_api_name=''`）在重建后仍保留，语义上"无所属本体"，
/// 导入新本体时不会复用（upsert 按 `(ontology_api_name, api_name)` 命中）。
/// 幂等迁移（统一发现池 v0.7）：
/// - validation_issues 加 `source`（'rules'|'review'）与 `a_action`（A 逐条裁决 JSON）
///   列，并对存量行回填 source='rules'——旧库全是规则检查产物。
/// - validation_reviews 加 `b_verdict`（B 复核原始输出）与三个环节耗时列。
/// - review 来源的 issue id 形如 "{review_id}:{B条目id}"，validation_id = review_id：
///   重跑规则检查（按 source='rules' 分域清理）不会冲掉 B 发现行。
fn migrate_validation_unified_pool(conn: &Connection) -> StoreResult<()> {
    ensure_columns(
        conn,
        "validation_issues",
        &[
            ("source", "TEXT NOT NULL DEFAULT 'rules'"),
            ("a_action", "TEXT"),
        ],
    )?;
    // 存量行回填（新库默认值已覆盖；旧库 ensure_columns 前的行全按 'rules'）
    conn.execute_batch(
        "UPDATE validation_issues SET source = 'rules' WHERE source IS NULL OR source = '';
         CREATE INDEX IF NOT EXISTS idx_vi_source ON validation_issues(ontology_iri, source);",
    )?;
    ensure_columns(
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

fn migrate_datasets_data_sources_ontology_scoping(conn: &Connection) -> StoreResult<()> {
    ensure_columns(
        conn,
        "datasets",
        &[("ontology_api_name", "TEXT NOT NULL DEFAULT ''")],
    )?;
    ensure_columns(
        conn,
        "data_sources",
        &[("ontology_api_name", "TEXT NOT NULL DEFAULT ''")],
    )?;
    // 检测并删除旧的单列 api_name 全局唯一约束（表重建），必须在建联合唯一索引之前
    // 执行——否则两套唯一约束并存，旧的单列约束仍会先触发冲突。
    drop_legacy_single_column_unique_on_api_name(conn, "datasets")?;
    drop_legacy_single_column_unique_on_api_name(conn, "data_sources")?;
    // 联合唯一索引（IF NOT EXISTS 幂等；全新库已由 CREATE TABLE 的 UNIQUE 约束覆盖，
    // 旧库迁移后靠此索引生效）。SQLite 不支持 ALTER TABLE 加表级约束，故用唯一索引等价表达。
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_datasets_ont_api ON datasets(ontology_api_name, api_name);
         CREATE INDEX IF NOT EXISTS idx_datasets_ontology ON datasets(ontology_api_name);
         CREATE UNIQUE INDEX IF NOT EXISTS idx_data_sources_ont_api ON data_sources(ontology_api_name, api_name);
         CREATE INDEX IF NOT EXISTS idx_data_sources_ontology ON data_sources(ontology_api_name);",
    )?;
    Ok(())
}

/// 检测 `<table>` 是否存在**仅覆盖单列 `api_name` 的唯一约束**（列级 `UNIQUE` 或
/// 显式单列唯一索引），若存在则用 SQLite 标准 12 步表重建法去掉它，仅保留联合唯一。
///
/// 检测策略：不依赖 `PRAGMA index_list`——某些 SQLite 编译/版本下 `sqlite_autoindex_*`
/// 不出现在 `index_list` 结果里（实测 rusqlite bundled SQLite 查返回空，但约束仍生效）。
/// 改查 `sqlite_master.sql` 的建表 DDL：若 DDL 里 `api_name` 列声明含列级 `UNIQUE`，
/// 则重建表去掉它（重建后新表 DDL 无列级 UNIQUE，幂等不再触发）。
///
/// 幂等：重建后新表 DDL 不含列级 api_name UNIQUE，再次调用检测不命中，不重建。
fn drop_legacy_single_column_unique_on_api_name(conn: &Connection, table: &str) -> StoreResult<()> {
    // 表可能尚未创建（防御性，正常 init_schema 已先建）。
    let ddl: Option<String> = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
        rusqlite::params![table],
        |r| r.get::<_, Option<String>>(0),
    )?;
    let ddl = match ddl {
        Some(s) => s,
        None => return Ok(()), // 表不存在
    };
    // 检测 DDL 里 api_name 是否带列级 UNIQUE（形如 `api_name TEXT NOT NULL UNIQUE`）。
    // 不匹配联合 UNIQUE(ontology_api_name, api_name)——那是表级约束，不在列行。
    // 用大小写不敏感 + 忽略空白的方式匹配「api_name ... UNIQUE」且不在括号表级约束内。
    // 简单启发：DDL 里找 `api_name` 行，看该行内是否有独立的 UNIQUE 词。
    let has_col_level_unique = ddl.lines().map(|l| l.trim()).any(|line| {
        // 跳过表级 UNIQUE(...) 约束行（以 UNIQUE( 开头）
        if line.to_uppercase().starts_with("UNIQUE(") {
            return false;
        }
        // 列行：含 api_name 且含 UNIQUE
        let up = line.to_uppercase();
        up.contains("API_NAME") && up.contains("UNIQUE")
    });
    if !has_col_level_unique {
        return Ok(());
    }
    // 存在旧的单列 api_name 唯一约束 → 用 12 步表重建法去掉。
    // SQLite 官方推荐流程（https://www.sqlite.org/lang_altertable.html#otheralter）：
    //   1. 开事务
    //   2. 用新 schema（无单列 UNIQUE）建临时表 _new
    //   3. INSERT INTO _new SELECT * FROM <table>（列对齐）
    //   4. DROP TABLE <table>
    //   5. ALTER TABLE _new RENAME TO <table>
    //   6. 重建索引（联合唯一索引由上层 migrate 函数 IF NOT EXISTS 重建；FK 由
    //      foreign_keys=ON 自动恢复，但 RENAME 后需 PRAGMA legacy_alter_table 或
    //      重建外键引用——这里 datasets/data_sources 只被 ontologies 的 ON DELETE
    //      CASCADE 引用，重建不破坏 FK 方向）
    //
    // 新表 schema 与 init_schema 的 CREATE TABLE 完全一致（无单列 UNIQUE，
    // 有联合 UNIQUE(ontology_api_name, api_name) + FK + CHECK）。
    let new_ddl = match table {
        "datasets" => {
            r#"
            CREATE TABLE datasets__migrate (
                id                        TEXT PRIMARY KEY,
                ontology_api_name         TEXT NOT NULL,
                api_name                  TEXT NOT NULL,
                display_name              TEXT NOT NULL DEFAULT '',
                storage_location          TEXT NOT NULL DEFAULT '',
                partition_config          TEXT,
                source_dataset_api_name   TEXT,
                data_source_api_name      TEXT,
                kind                      TEXT NOT NULL DEFAULT 'MANAGED' CHECK (kind IN ('MANAGED','VIRTUAL')),
                is_view                   INTEGER NOT NULL DEFAULT 0,
                created_at                INTEGER NOT NULL,
                updated_at                INTEGER NOT NULL,
                UNIQUE(ontology_api_name, api_name),
                FOREIGN KEY (ontology_api_name) REFERENCES ontologies(api_name) ON DELETE CASCADE
            );
        "#
        }
        "data_sources" => {
            r#"
            CREATE TABLE data_sources__migrate (
                id                    TEXT PRIMARY KEY,
                ontology_api_name     TEXT NOT NULL,
                api_name              TEXT NOT NULL,
                display_name          TEXT NOT NULL,
                description           TEXT NOT NULL DEFAULT '',
                connector_type        TEXT NOT NULL,
                connector_config      TEXT NOT NULL DEFAULT '{}',
                credential_id         TEXT,
                created_at            INTEGER NOT NULL,
                updated_at            INTEGER NOT NULL,
                UNIQUE(ontology_api_name, api_name),
                FOREIGN KEY (ontology_api_name) REFERENCES ontologies(api_name) ON DELETE CASCADE
            );
        "#
        }
        _ => return Ok(()), // 防御性，只迁移这两张表
    };
    // legacy_alter_table=OFF（默认）时 RENAME 会自动修复外键引用；显式置 ON 更稳。
    // 先关 FK 检查避免迁移中途因引用完整性报错（旧库可能有孤儿行，但重建后 FK 仍生效）。
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
    let tx = conn.unchecked_transaction()?;
    let result = (|| -> StoreResult<()> {
        tx.execute_batch(new_ddl)?;
        // 旧表可能没有 ontology_api_name 列吗？不会——上层 ensure_columns 已先加列。
        // 因此直接 SELECT 全列（按新表列序）拷贝即可。
        let copy_sql = match table {
            "datasets" =>
                "INSERT INTO datasets__migrate (id, ontology_api_name, api_name, display_name, storage_location, partition_config, source_dataset_api_name, data_source_api_name, kind, is_view, created_at, updated_at)
                 SELECT id, ontology_api_name, api_name, display_name, storage_location, partition_config, source_dataset_api_name, data_source_api_name, kind, is_view, created_at, updated_at FROM datasets",
            "data_sources" =>
                "INSERT INTO data_sources__migrate (id, ontology_api_name, api_name, display_name, description, connector_type, connector_config, credential_id, created_at, updated_at)
                 SELECT id, ontology_api_name, api_name, display_name, description, connector_type, connector_config, credential_id, created_at, updated_at FROM data_sources",
            _ => return Ok(()),
        };
        tx.execute_batch(copy_sql)?;
        tx.execute_batch(&format!("DROP TABLE {table};"))?;
        tx.execute_batch(&format!("ALTER TABLE {table}__migrate RENAME TO {table};"))?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            tx.commit()?;
            conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        }
        Err(e) => {
            // 回滚时 tx drop 自动 rollback（unchecked_transaction 的 commit 未调用即回滚）
            conn.execute_batch("PRAGMA foreign_keys=ON;").ok();
            return Err(e);
        }
    }
    Ok(())
}

/// 幂等迁移：若 `link_types` 表含残留的 `direction` 列则 DROP。
///
/// 对齐 Gaia 的 alembic 迁移（见 engines/gaia/alembic/versions/
/// 20260813_0754_*_drop_direction_column_from_link_types.py）。SQLite 3.35+
/// 支持 `ALTER TABLE ... DROP COLUMN`，rusqlite 0.39 bundled 自带 SQLite ≥3.40，满足。
fn drop_link_types_direction_column(conn: &Connection) -> StoreResult<()> {
    // 先确认 link_types 表已存在（init_schema 的 CREATE 在前，正常必存在；
    // 此 guard 仅为防御性，避免对空库 PRAGMA 报错）。
    let table_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='link_types'",
        [],
        |r| r.get(0),
    )?;
    if table_exists == 0 {
        return Ok(());
    }
    // 检测 direction 列是否存在。
    let has_direction: bool = {
        let mut stmt = conn.prepare("PRAGMA table_info(link_types)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .collect();
        cols.iter().any(|c| c == "direction")
    };
    if has_direction {
        conn.execute_batch("ALTER TABLE link_types DROP COLUMN direction;")?;
    }
    Ok(())
}

/// 幂等加列：检测 col 是否已存在，不存在则 ADD COLUMN。返回 true 表示新增了列。
pub fn ensure_columns(conn: &Connection, table: &str, cols: &[(&str, &str)]) -> StoreResult<bool> {
    let mut added = false;
    for (col, decl) in cols {
        let sql = format!("PRAGMA table_info({table})");
        let mut stmt = conn.prepare(&sql)?;
        let exists = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|c| c == *col);
        if !exists {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {col} {decl};"))?;
            added = true;
        }
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_schema_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        init_schema(&conn).unwrap(); // 二次调用不报错
                                     // 验证表存在
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='object_types'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn legacy_validation_issues_without_source_upgrades() {
        // 模拟 v0.7 之前的存量库：validation_issues 无 source/a_action 列。
        // init_schema 必须先补列再建 idx_vi_source 索引——旧实现把索引建在
        // 主 execute_batch 里，老库缺列直接 panic（闪退根因，2026-09-23）。
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE validation_issues (
                id            TEXT PRIMARY KEY,
                ontology_iri  TEXT NOT NULL,
                rule_id       TEXT NOT NULL,
                severity      TEXT NOT NULL,
                message       TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'open',
                created_at    INTEGER NOT NULL,
                updated_at    INTEGER NOT NULL
            );",
        )
        .unwrap();
        // 旧库升级不应 panic
        init_schema(&conn).unwrap();
        // source 列已补 + 默认值回填
        let src: String = conn
            .query_row("SELECT source FROM validation_issues LIMIT 1", [], |r| r.get(0))
            .unwrap_or_default();
        assert!(src.is_empty() || src == "rules");
        // 索引已建
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_vi_source'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1);
    }

    #[test]
    fn foreign_key_cascade_works() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute("INSERT INTO ontologies (id, api_name, display_name, created_at, updated_at) VALUES ('o1','Ont','O',1,1)", []).unwrap();
        conn.execute("INSERT INTO object_types (id, ontology_id, api_name, display_name, primary_key, storage_type, capabilities, created_at, updated_at) VALUES ('ot1','o1','Supplier','供应商','supplierId','MANAGED','{}',1,1)", []).unwrap();
        // 删 ontology 应级联删 object_type
        conn.execute("DELETE FROM ontologies WHERE id='o1'", [])
            .unwrap();
        let ot_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM object_types", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ot_count, 0, "ObjectType 应被级联删除");
    }

    #[test]
    fn check_constraint_rejects_bad_enum() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn.execute("INSERT INTO ontologies (id, api_name, display_name, created_at, updated_at) VALUES ('o1','Ont','O',1,1)", []).unwrap();
        // storage_type 非法应被 CHECK 拒绝
        let res = conn.execute(
            "INSERT INTO object_types (id, ontology_id, api_name, display_name, primary_key, storage_type, capabilities, created_at, updated_at) VALUES ('ot1','o1','Bad','坏','id','INVALID','{}',1,1)",
            [],
        );
        assert!(res.is_err(), "非法 storage_type 应被 CHECK 拦截");
    }

    /// 辅助：检测某表是否存在指定列。
    fn column_exists(conn: &Connection, table: &str, col: &str) -> bool {
        let mut s = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        let names: Vec<String> = s
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        names.iter().any(|c| c == col)
    }

    /// 回归测试：旧 .db 文件的 link_types 表含残留 `direction NOT NULL` 列，
    /// init_schema 必须幂等迁移 DROP 该列，否则新代码 INSERT 会报
    /// `NOT NULL constraint failed: link_types.direction`（用户实际遭遇的 14 条
    /// LinkType 未落库根因）。
    #[test]
    fn init_schema_migrates_legacy_link_types_direction_column() {
        let conn = Connection::open_in_memory().unwrap();
        // 1. 先建一张「旧 schema」link_types（含 direction NOT NULL + CHECK），
        //    模拟早期开发版留下的 .db 文件。
        conn.execute_batch(
            r#"
            CREATE TABLE ontologies (
                id TEXT PRIMARY KEY, api_name TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'ACTIVE',
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
            );
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
        )
        .unwrap();
        // 旧表确实带 direction 列。
        let has_dir: bool = column_exists(&conn, "link_types", "direction");
        assert!(has_dir, "旧表应有 direction 列");

        // 2. 跑 init_schema（CREATE TABLE IF NOT EXISTS 对已存在表是 no-op，
        //    迁移函数负责 DROP direction）。
        init_schema(&conn).unwrap();

        // 3. direction 列应被迁移移除。
        let still_has_dir: bool = column_exists(&conn, "link_types", "direction");
        assert!(!still_has_dir, "迁移后不应再有 direction 列");

        // 4. 用当前代码的 INSERT 形态（不写 direction）应能成功落库。
        conn.execute(
            "INSERT INTO ontologies (id, api_name, display_name, created_at, updated_at) \
             VALUES ('o1','SupplyChain','供应链本体',1,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO link_types (id, ontology_id, api_name, display_name, \
             source_object_type_api_name, target_object_type_api_name, cardinality, \
             created_at, updated_at) \
             VALUES ('l1','o1','supplies','供应','Supplier','Order','MANY',1,1)",
            [],
        )
        .unwrap();

        // 5. 幂等：再跑一次 init_schema 不报错（direction 已无，迁移为 no-op）。
        init_schema(&conn).unwrap();
    }
}
