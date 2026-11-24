//! 物理结构快照 + 映射核对判定。
//!
//! 设计要点（`design.md` D5 / D13）：
//! - **不依赖 `federation` crate**——装配层（`src-tauri`）从 `FederationService` 内存快照与
//!   `evidence_sources` 抽取后传进来。这样 `ontology-store` 保持单一依赖，
//!   单测可直接构造快照，不必起数据库。
//! - **核对状态由系统判定**——写入时在快照里找 `(table, column)`，命中才 `verified`。
//!   调用方无法声明"已核对"，否则闸门会被 LLM 一律声明为通过。
//! - **不拒绝写入**——用户可能先建模型、后连数据源。快照缺失时降级为"未核对"，
//!   而非报错失败。
//! - **`stale` 只按结构差异判定**——连接失败/数据源未注册不算失效（高频临时事件，
//!   算作失效会让报告长期充满噪声，用户会学会忽略它）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::evidence::EvidenceSource;
use crate::mapping::{MappingEndpoint, SOURCE_KIND_EVIDENCE, SOURCE_KIND_FEDERATION};

// ════════════════════════════════════════════════════════════════
// 快照 DTO
// ════════════════════════════════════════════════════════════════

/// 校验用的物理结构快照（由装配层构造后传入）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct PhysicalSchemaSnapshot {
    #[serde(default)]
    pub sources: Vec<PhysicalSource>,
}

/// 一个数据源（联邦已注册连接或证据源登记）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct PhysicalSource {
    /// 数据源名（联邦数据源 api_name / 证据源标题）；仅用于在告警中定位来源。
    pub name: String,
    /// `federation` | `evidence`（决定映射行的 `source_kind`）。
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub tables: Vec<PhysicalTable>,
}

/// 一张物理表。
#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct PhysicalTable {
    #[serde(default)]
    pub catalog: String,
    #[serde(default)]
    pub schema: String,
    pub name: String,
    #[serde(default)]
    pub columns: Vec<PhysicalColumn>,
}

/// 一个物理列。
#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct PhysicalColumn {
    pub name: String,
    /// 物理类型原文（如 `varchar(64)` / `bigint`）；类型兼容性打分用。
    #[serde(default)]
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
}

impl PhysicalSchemaSnapshot {
    pub fn is_empty(&self) -> bool {
        self.sources.iter().all(|s| s.tables.is_empty())
    }

    /// 全部表（按数据源顺序展开）。
    pub fn tables(&self) -> impl Iterator<Item = (&PhysicalSource, &PhysicalTable)> {
        self.sources
            .iter()
            .flat_map(|s| s.tables.iter().map(move |t| (s, t)))
    }

    /// 查找匹配的表（catalog/schema 允许为空，表示"不限定"）。
    ///
    /// 空段按通配处理：顶层证据源只有 `table.column` 记号，没有 catalog/schema 语义，
    /// 若要求段数严格相等会让这类快照永远匹配不上。
    ///
    /// 兜底：`table` 传成 `schema.table` 整串时自动拆段比较。写入侧
    /// `ElementMappingInput::manual(table, column)` 与 agent 工具都会把用户填的
    /// 「来源表」原样放进 `table`，用户完全可能写 `mhero_ods.sales_order`
    /// （前端表单有独立 schema 输入框，但不是唯一入口）。不兜底会让这类映射的
    /// 物理事实列（字段类型/可空）与核对状态静默失效，用户在导出表里看到空白。
    pub fn find_table(
        &self,
        catalog: &str,
        schema: &str,
        table: &str,
    ) -> Option<(&PhysicalSource, &PhysicalTable)> {
        let hit = |name: &str, sc: &str| {
            self.tables().find(|(_, t)| {
                t.name.eq_ignore_ascii_case(name)
                    && (sc.is_empty() || t.schema.is_empty() || t.schema.eq_ignore_ascii_case(sc))
                    && (catalog.is_empty()
                        || t.catalog.is_empty()
                        || t.catalog.eq_ignore_ascii_case(catalog))
            })
        };
        if let Some(found) = hit(table, schema) {
            return Some(found);
        }
        // `schema.table` 整串：拆开后重试（schema 参数为空时才拆，避免覆盖显式分段）
        if schema.is_empty() {
            if let Some((sc, name)) = table.rsplit_once('.') {
                return hit(name, sc);
            }
        }
        None
    }

    /// 查找列（大小写不敏感）。
    pub fn find_column<'a>(table: &'a PhysicalTable, column: &str) -> Option<&'a PhysicalColumn> {
        table
            .columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(column))
    }
}

// ════════════════════════════════════════════════════════════════
// 核对判定
// ════════════════════════════════════════════════════════════════

/// 单条映射的核对结果。
#[derive(Debug, Clone, PartialEq)]
pub enum MappingVerifyOutcome {
    /// 物理位置在快照中确实存在。
    Verified,
    /// 未命中（附面向用户的聚合告警文本）。
    Unverified(String),
    /// 无法判定（快照为空 / 该端无表级定位）——保留原状态，不标记为失败。
    Skipped,
}

/// 校验一个物理位置：表必须存在；有列名时列也必须存在。
fn check_endpoint(
    snapshot: &PhysicalSchemaSnapshot,
    ep: &MappingEndpoint,
    role: &str,
) -> Result<(), MappingVerifyOutcome> {
    let Some((_, table)) = snapshot.find_table(&ep.catalog, &ep.schema, &ep.table) else {
        return Err(MappingVerifyOutcome::Unverified(format!(
            "{role}表 '{}' 未在已登记的数据源中找到",
            ep.table_locator()
        )));
    };
    if ep.column.is_empty() {
        return Ok(());
    }
    if PhysicalSchemaSnapshot::find_column(table, &ep.column).is_none() {
        return Err(MappingVerifyOutcome::Unverified(format!(
            "{role}字段 '{}' 未在已登记的数据源中找到",
            ep.locator()
        )));
    }
    Ok(())
}

/// 判定一条映射的核对状态（纯函数，无 IO）。
///
/// - 快照为空 → `Skipped`（"不校验"而非"校验失败"）
/// - 来源端无表级定位 → `Skipped`（写入路径已过滤，属防御）
/// - 来源/目标有一端不命中 → `Unverified`（附告警文本）
/// - 全命中 → `Verified`
pub fn verify_mapping(
    source: &MappingEndpoint,
    target: &MappingEndpoint,
    snapshot: &PhysicalSchemaSnapshot,
) -> MappingVerifyOutcome {
    if snapshot.is_empty() {
        return MappingVerifyOutcome::Skipped;
    }
    if !source.has_table() {
        return MappingVerifyOutcome::Skipped;
    }
    if let Err(outcome) = check_endpoint(snapshot, source, "来源") {
        return outcome;
    }
    if target.has_table() {
        if let Err(outcome) = check_endpoint(snapshot, target, "目标") {
            return outcome;
        }
    }
    MappingVerifyOutcome::Verified
}

/// 该物理位置来自哪一类 schema 源（federation 优先于 evidence）。
pub fn source_kind_for(
    snapshot: &PhysicalSchemaSnapshot,
    ep: &MappingEndpoint,
) -> &'static str {
    match snapshot.find_table(&ep.catalog, &ep.schema, &ep.table) {
        Some((src, _)) if src.kind == SOURCE_KIND_EVIDENCE => SOURCE_KIND_EVIDENCE,
        Some(_) => SOURCE_KIND_FEDERATION,
        None => crate::mapping::SOURCE_KIND_MANUAL,
    }
}

// ════════════════════════════════════════════════════════════════
// 证据源文本快照解析（kind ∈ {db_table, db_column}）
// ════════════════════════════════════════════════════════════════

/// 从 `evidence_sources` 的文本快照解析出物理结构，并入快照。
///
/// 支持三种常见书写形态（用户贴 DDL / 贴字段清单 / 贴 Markdown 表）：
/// - `t_order.order_no`
/// - `t_order(order_no, amount, created_at)`
/// - `CREATE TABLE t_order ( order_no varchar(64), ... )`
///
/// 识别失败的文本直接跳过（宁缺勿错：错误解析会产出假的"已核对"）。
pub fn parse_evidence_physical(
    sources: impl IntoIterator<Item = EvidenceSource>,
) -> PhysicalSchemaSnapshot {
    // 表名（小写）→ (显示名, 列名→类型)
    let mut acc: HashMap<String, (String, Vec<PhysicalColumn>)> = HashMap::new();

    for src in sources {
        let target = match src.kind.as_str() {
            "db_table" | "db_column" => &src.content,
            _ => continue,
        };
        // db_column / db_table 源的 title 里常直接写着 `库.表.列`
        parse_schema_text(target, &mut acc);
        if !src.title.is_empty() {
            parse_schema_text(&src.title, &mut acc);
        }
    }

    let tables: Vec<PhysicalTable> = acc
        .into_iter()
        .map(|(name, (display, columns))| PhysicalTable {
            catalog: String::new(),
            schema: String::new(),
            name: if display.is_empty() { name.clone() } else { display },
            columns,
        })
        .collect();

    PhysicalSchemaSnapshot {
        sources: vec![PhysicalSource {
            name: "evidence".to_string(),
            kind: SOURCE_KIND_EVIDENCE.to_string(),
            tables,
        }],
    }
}

/// 从一段文本里抽取 `table.column` / `table(col, ...)` / DDL 记号，累积进 `acc`。
fn parse_schema_text(text: &str, acc: &mut HashMap<String, (String, Vec<PhysicalColumn>)>) {
    // ① 先处理 DDL 块（`CREATE TABLE t (...)` 常跨多行，逐行解析会只剩空括号）
    for (table, body) in extract_ddl_blocks(text) {
        merge_columns(acc, table, parse_column_list(&body));
    }

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        // 形态 1：单行表定义 `t_order(col1 type, col2 type)`
        if let Some((table, rest)) = split_table_paren(line) {
            let columns = parse_column_list(rest);
            merge_columns(acc, table, columns);
            continue;
        }
        // 形态 2：`t_order.order_no`（可能带类型后缀 `t_order.order_no: varchar`）
        for ident in collect_qualified_idents(line) {
            let Some((table, column)) = ident.rsplit_once('.') else {
                continue;
            };
            if table.is_empty() || column.is_empty() {
                continue;
            }
            merge_columns(
                acc,
                table.to_string(),
                vec![PhysicalColumn {
                    name: column.to_string(),
                    data_type: String::new(),
                    nullable: true,
                }],
            );
        }
    }
}

/// 从文本中提取全部 `CREATE TABLE <name> ( ... )` 块（括号配平，跨行）。
fn extract_ddl_blocks(text: &str) -> Vec<(String, String)> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(pos) = lower[from..].find("create table") {
        let start = from + pos + "create table".len();
        let Some(open_rel) = text[start..].find('(') else {
            break;
        };
        let open = start + open_rel;
        let head = text[start..open].trim();
        // 表名可能是 `db.schema.t_order` / `"t_order"` / `IF NOT EXISTS t_order`
        let head = head
            .trim_end_matches("if not exists")
            .trim();
        let head = head
            .split_whitespace()
            .next()
            .unwrap_or(head)
            .rsplit('.')
            .next()
            .unwrap_or(head)
            .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']');
        // 括号配平（忽略字符串内的括号）
        let mut depth = 0i32;
        let mut close = None;
        for (i, c) in text[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        if !head.is_empty() {
            out.push((head.to_string(), text[open + 1..close].to_string()));
        }
        from = close + 1;
        if from >= text.len() {
            break;
        }
    }
    out
}

/// `t_order(a int, b text)` → Some(("t_order", "a int, b text"))
fn split_table_paren(line: &str) -> Option<(String, &str)> {
    // 去掉 `CREATE TABLE` 之类前缀
    let lower = line.to_ascii_lowercase();
    let start = lower
        .find("create table")
        .map(|i| i + "create table".len())
        .unwrap_or(0);
    let rest = line[start..].trim_start();
    let open = rest.find('(')?;
    let head = rest[..open].trim();
    // 表名必须是**单个标识符**：`order_no varchar` / `PRIMARY KEY` 这类
    // "名字 + 类型/约束关键字" 的行不是表定义（否则会把 DDL 的列行误判成表）
    if head.is_empty() || head.chars().any(char::is_whitespace) {
        return None;
    }
    let table = head
        .rsplit('.')
        .next()
        .unwrap_or(head)
        .trim()
        .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']')
        .to_string();
    if table.is_empty() || !table.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let close = rest.rfind(')')?;
    if close <= open {
        return None;
    }
    Some((table, &rest[open + 1..close]))
}

/// 解析列清单 `a int NOT NULL, b varchar(64)`（跳过约束行）。
fn parse_column_list(rest: &str) -> Vec<PhysicalColumn> {
    let mut out = Vec::new();
    for part in split_top_level_commas(rest) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // 列名可能带引号且含空格（`` `Customer Name` varchar(32) ``）：先按引号整体取，
        // 无引号才按首个空白切。结束偏移按原文实际位置算——剥掉引号后 `name.len()`
        // 不等于原文结束位置，中文列名若按剥后长度切片会切进字符中间 panic。
        let consumed = column_name_len(part);
        if consumed == 0 {
            continue;
        }
        let name = part[..consumed]
            .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']')
            .to_string();
        if name.is_empty() {
            continue;
        }
        // 约束/索引声明不是列
        let upper = name.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "PRIMARY" | "KEY" | "UNIQUE" | "CONSTRAINT" | "INDEX" | "FOREIGN" | "CHECK"
        ) {
            continue;
        }
        let data_type = part
            .get(consumed..)
            .unwrap_or("")
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        out.push(PhysicalColumn {
            name,
            data_type,
            nullable: !part.to_ascii_uppercase().contains("NOT NULL"),
        });
    }
    out
}

/// 列名的字节长度（含引号）：带引号则取到配对引号，否则取到首个空白。
///
/// 必须返回**原文里的字节长度**而不是剥引号后的名称长度——后续按该偏移切
/// 类型声明，用剥后的长度会在中文/带引号列名上切进字符中间 panic。
fn column_name_len(part: &str) -> usize {
    let Some(open) = part.chars().next() else {
        return 0;
    };
    let close = match open {
        '`' => '`',
        '"' => '"',
        '[' => ']',
        _ => {
            return part
                .split_whitespace()
                .next()
                .map(|w| w.len())
                .unwrap_or(part.len())
        }
    };
    // 引号内的列名可以含空格：找配对引号，找不到就退回空白切分
    if let Some(rel) = part[open.len_utf8()..].find(close) {
        return open.len_utf8() + rel + close.len_utf8();
    }
    part.split_whitespace()
        .next()
        .map(|w| w.len())
        .unwrap_or(part.len())
}

/// 顶层逗号切分（括号内的逗号不切，如 `decimal(10,2)`）。
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// 形态 2：收集 `a.b` 形状的标识符（排除小数、URL、时间、DDL 列行）。
fn collect_qualified_idents(line: &str) -> Vec<String> {
    line.split(|c: char| {
        c.is_whitespace() || c == ',' || c == ';' || c == '|' || c == '(' || c == ')'
    })
    .filter(|tok| {
        tok.matches('.').count() == 1
            && !tok.contains("://")
            && tok.chars().all(|c| {
                c.is_alphanumeric() || c == '_' || c == '.' || c == '`' || c == '"' || c == '-' || c == '$'
            })
            && !tok.chars().next().is_some_and(|c| c.is_ascii_digit())
            // 左右两侧都必须像标识符（`decimal(10,2)` 的残片会被切成 `10.2` 之类，
            // 已由 digit 检查挡掉；这里再挡 `t.` / `.x`）
            && !tok.starts_with('.')
            && !tok.ends_with('.')
    })
    .map(|t| t.trim_matches(|c| c == '`' || c == '"').to_string())
    .collect()
}

fn merge_columns(
    acc: &mut HashMap<String, (String, Vec<PhysicalColumn>)>,
    table: String,
    columns: Vec<PhysicalColumn>,
) {
    let key = table.to_ascii_lowercase();
    let entry = acc.entry(key).or_insert_with(|| (table, Vec::new()));
    for col in columns {
        let exists = entry
            .1
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&col.name));
        if !exists {
            entry.1.push(col);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> PhysicalSchemaSnapshot {
        PhysicalSchemaSnapshot {
            sources: vec![PhysicalSource {
                name: "sales-db".into(),
                kind: SOURCE_KIND_FEDERATION.into(),
                tables: vec![PhysicalTable {
                    catalog: "sales".into(),
                    schema: "public".into(),
                    name: "t_order".into(),
                    columns: vec![
                        PhysicalColumn {
                            name: "order_no".into(),
                            data_type: "varchar(64)".into(),
                            nullable: false,
                        },
                        PhysicalColumn {
                            name: "amount".into(),
                            data_type: "decimal(10,2)".into(),
                            nullable: true,
                        },
                    ],
                }],
            }],
        }
    }

    fn ep(table: &str, column: &str) -> MappingEndpoint {
        MappingEndpoint {
            table: table.into(),
            column: column.into(),
            ..Default::default()
        }
    }

    #[test]
    fn verified_when_column_exists() {
        let out = verify_mapping(&ep("t_order", "order_no"), &MappingEndpoint::default(), &snapshot());
        assert_eq!(out, MappingVerifyOutcome::Verified);
    }

    #[test]
    fn table_level_mapping_verified() {
        // 类映射到表（无列名）
        let out = verify_mapping(&ep("t_order", ""), &MappingEndpoint::default(), &snapshot());
        assert_eq!(out, MappingVerifyOutcome::Verified);
    }

    #[test]
    fn unverified_when_column_missing() {
        let out = verify_mapping(&ep("t_order", "nope"), &MappingEndpoint::default(), &snapshot());
        match out {
            MappingVerifyOutcome::Unverified(msg) => assert!(msg.contains("t_order.nope"), "{msg}"),
            other => panic!("expected Unverified, got {other:?}"),
        }
    }

    #[test]
    fn unverified_when_table_missing() {
        let out = verify_mapping(&ep("t_missing", "x"), &MappingEndpoint::default(), &snapshot());
        assert!(matches!(out, MappingVerifyOutcome::Unverified(_)));
    }

    #[test]
    fn skipped_when_snapshot_empty() {
        let out = verify_mapping(
            &ep("t_order", "order_no"),
            &MappingEndpoint::default(),
            &PhysicalSchemaSnapshot::default(),
        );
        assert_eq!(out, MappingVerifyOutcome::Skipped);
    }

    #[test]
    fn relation_target_also_checked() {
        let mut snap = snapshot();
        snap.sources[0].tables.push(PhysicalTable {
            name: "t_customer".into(),
            columns: vec![PhysicalColumn {
                name: "id".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        let out = verify_mapping(
            &ep("t_order", "order_no"),
            &ep("t_customer", "id"),
            &snap,
        );
        assert_eq!(out, MappingVerifyOutcome::Verified);
        // 目标列不存在 → 未核对
        let out = verify_mapping(
            &ep("t_order", "order_no"),
            &ep("t_customer", "nope"),
            &snap,
        );
        assert!(matches!(out, MappingVerifyOutcome::Unverified(_)));
    }

    #[test]
    fn parses_ddl_and_column_notations() {
        let sources = vec![
            EvidenceSource {
                source_id: "src-1".into(),
                ontology_api_name: String::new(),
                kind: "db_table".into(),
                title: "sales.t_order".into(),
                content: "CREATE TABLE t_order (\n  order_no varchar(64) NOT NULL,\n  amount decimal(10,2),\n  PRIMARY KEY (order_no)\n);".into(),
                content_hash: String::new(),
                origin_ref: String::new(),
                created_at: 0,
            },
            EvidenceSource {
                source_id: "src-2".into(),
                ontology_api_name: String::new(),
                kind: "db_column".into(),
                title: "t_customer.id".into(),
                content: "t_customer.id\nt_customer.name".into(),
                content_hash: String::new(),
                origin_ref: String::new(),
                created_at: 0,
            },
        ];
        let snap = parse_evidence_physical(sources);
        let out = verify_mapping(&ep("t_order", "order_no"), &MappingEndpoint::default(), &snap);
        assert_eq!(out, MappingVerifyOutcome::Verified, "DDL 解析: {snap:?}");
        let out = verify_mapping(&ep("t_order", "amount"), &MappingEndpoint::default(), &snap);
        assert_eq!(out, MappingVerifyOutcome::Verified);
        let out = verify_mapping(&ep("t_customer", "name"), &MappingEndpoint::default(), &snap);
        assert_eq!(out, MappingVerifyOutcome::Verified, "db_column 解析: {snap:?}");
        // 约束声明不应被当成列
        let out = verify_mapping(&ep("t_order", "PRIMARY"), &MappingEndpoint::default(), &snap);
        assert!(matches!(out, MappingVerifyOutcome::Unverified(_)));
    }

    #[test]
    fn parses_chinese_and_quoted_column_names_without_panic() {
        // 回归：`part[name.len()..]` 在剥掉引号/遇到多字节字符时会切进字符中间 panic
        // （真实崩溃：`start byte index 45 is not a char boundary`）。
        let snap = parse_evidence_physical(vec![
            EvidenceSource {
                source_id: "src-1".into(),
                ontology_api_name: String::new(),
                kind: "db_table".into(),
                title: "订单表".into(),
                content: "CREATE TABLE t_order (\n  订单号 varchar(64) NOT NULL,\n  金额 decimal(10,2),\n  `Customer Name` varchar(32),\n  PRIMARY KEY (订单号)\n);".into(),
                content_hash: String::new(),
                origin_ref: String::new(),
                created_at: 0,
            },
            EvidenceSource {
                source_id: "src-2".into(),
                ontology_api_name: String::new(),
                kind: "db_column".into(),
                title: "t_order.备注".into(),
                content: "t_order.备注".into(),
                content_hash: String::new(),
                origin_ref: String::new(),
                created_at: 0,
            },
        ]);
        let out = verify_mapping(&ep("t_order", "订单号"), &MappingEndpoint::default(), &snap);
        assert_eq!(out, MappingVerifyOutcome::Verified, "中文列名: {snap:?}");
        let out = verify_mapping(&ep("t_order", "金额"), &MappingEndpoint::default(), &snap);
        assert_eq!(out, MappingVerifyOutcome::Verified);
        let out = verify_mapping(&ep("t_order", "备注"), &MappingEndpoint::default(), &snap);
        assert_eq!(out, MappingVerifyOutcome::Verified, "中文列记号: {snap:?}");
        // 类型解析不被名称长度影响
        let cols = parse_column_list("订单号 varchar(64) NOT NULL, `Customer Name` varchar(32)");
        assert_eq!(cols[0].name, "订单号");
        assert_eq!(cols[0].data_type, "varchar(64)");
        assert_eq!(cols[1].name, "Customer Name");
        assert_eq!(cols[1].data_type, "varchar(32)");
    }

    #[test]
    fn source_kind_reported_from_snapshot() {
        let snap = parse_evidence_physical(vec![EvidenceSource {
            source_id: "s".into(),
            ontology_api_name: String::new(),
            kind: "db_column".into(),
            title: "t_order.order_no".into(),
            content: "t_order.order_no".into(),
            content_hash: String::new(),
            origin_ref: String::new(),
            created_at: 0,
        }]);
        assert_eq!(
            source_kind_for(&snap, &ep("t_order", "order_no")),
            SOURCE_KIND_EVIDENCE
        );
        assert_eq!(
            source_kind_for(&snap, &ep("t_unknown", "")),
            crate::mapping::SOURCE_KIND_MANUAL
        );
    }
}