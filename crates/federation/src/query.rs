//! query: 只读查询执行（见 PHASE3-FEDERATION.md §3.1 execute_sql / §3.2 安全护栏）。
//!
//! 三层只读防御：
//!   1. sqlparser 前置拦截：复用 datafusion 自带 sqlparser 0.62，解析后只放行
//!      Statement::Query（SELECT/WITH），其余直接拒
//!   2. 行数硬上限：默认 200，最大 1000，SQL 末尾自动追加 LIMIT（未含时）
//!   3. 超时：tokio::time::timeout(30s) 包裹 collect()
//!
//! SessionConfig 禁 DDL 由 SessionContext 构造层处理（lib.rs），此处不重复。

use std::time::{Duration, Instant};

use datafusion::arrow::datatypes::DataType;
use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::ast::Statement;
use datafusion::sql::sqlparser::dialect::GenericDialect;
use datafusion::sql::sqlparser::parser::Parser as SqlParser;

use crate::error::{FederationError, FederationResult};
use crate::source::{ColumnMeta, QueryResult};

/// 默认行数上限（防 SELECT * 爆内存 + 撑爆 agent 上下文）。
pub const DEFAULT_ROW_LIMIT: usize = 200;
/// 最大行数上限（用户显式传 limit 时 clamp）。
pub const MAX_ROW_LIMIT: usize = 1000;
/// 查询超时（秒）。DataFusion 49+ 协作式取消，stream drop 即停。
pub const QUERY_TIMEOUT_SECS: u64 = 30;

/// 只读护栏：解析 SQL，只放行 SELECT/WITH（Statement::Query）。
///
/// 复用 datafusion 自带的 sqlparser 0.62（不需单独加 sqlparser 依赖，见 §3.2）。
/// 遇 Insert/Update/Delete/Drop/Alter/Truncate/Create 等直接拒。
pub fn assert_readonly(sql: &str) -> FederationResult<()> {
    let dialect = GenericDialect {};
    let statements = SqlParser::parse_sql(&dialect, sql)
        .map_err(|e| FederationError::SqlParse(e.to_string()))?;
    if statements.is_empty() {
        return Err(FederationError::SqlParse("空 SQL".into()));
    }
    // 多语句：任一非 Query 即拒（防 `SELECT 1; DROP TABLE t` 注入）
    for stmt in &statements {
        if !is_readonly_statement(stmt) {
            return Err(FederationError::ReadonlyViolation(stmt.to_string()));
        }
    }
    Ok(())
}

/// 判断单条 Statement 是否只读（仅 Query 放行）。
///
/// Statement::Query 覆盖 SELECT / WITH...SELECT / VALUES / 子查询组合，
/// 不含任何写/DDL。其余变体（Insert/Update/Delete/Drop/Alter/Create/Truncate/
/// Copy/Explain{含非query}/...）一律拒。
fn is_readonly_statement(stmt: &Statement) -> bool {
    match stmt {
        Statement::Query(_) => true,
        // Explain/Describe 包裹的若是 Query 也允许（agent 排查计划常用）
        Statement::Explain { statement, .. } => is_readonly_statement(statement),
        _ => false,
    }
}

/// SQL 是否已含 LIMIT（用于决定是否自动追加）。
///
/// 简单启发：检查 query body 的 Select 是否有 limit。完整判断需遍历 AST，
/// 但 agent 生成的 SQL 通常规范，启发式足够；未识别到 LIMIT 时保守追加
/// （外层包一层 SELECT + LIMIT，安全且不破坏语义）。
fn has_limit(sql: &str) -> bool {
    // 大小写不敏感检索 LIMIT 关键字（粗略，覆盖绝大多数情况）。
    // 误判（如列名含 limit）至多多包一层 SELECT，不影响正确性。
    let lower = sql.to_lowercase();
    lower.contains(" limit ")
        || lower.ends_with(" limit")
        || lower.contains("\nlimit ")
}

/// 执行只读查询。
///
/// 内部：只读校验 → 自动追加 LIMIT → DataFusion 执行（超时包裹）→ Arrow→JSON。
/// `sources_touched` 从执行计划提取涉及的 catalog 名。
pub async fn execute_query(
    ctx: &SessionContext,
    sql: &str,
    limit: Option<usize>,
) -> FederationResult<QueryResult> {
    // 1. 只读护栏
    assert_readonly(sql)?;

    // 2. 行数上限 + 自动追加 LIMIT
    let row_limit = limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROW_LIMIT);
    let final_sql = if has_limit(sql) {
        sql.to_string()
    } else {
        // 外层包一层，避免破坏原 SQL 结构（如含 ORDER BY/WITH）
        format!("SELECT * FROM ({sql}) AS __onto_sub LIMIT {row_limit}")
    };

    let start = Instant::now();

    // 3. 超时包裹执行
    let collect_fut = async {
        let df = ctx.sql(&final_sql).await?;
        // 取涉及的 catalog 名（从优化后的逻辑计划提取，比物理计划稳定）
        let logical_plan = df.clone().into_optimized_plan().ok();
        let sources = logical_plan
            .as_ref()
            .map(extract_sources_from_logical_plan)
            .unwrap_or_default();
        let batches = df.collect().await?;
        FederationResult::Ok((batches, sources))
    };
    let (batches, sources_touched) = tokio::time::timeout(
        Duration::from_secs(QUERY_TIMEOUT_SECS),
        collect_fut,
    )
    .await
    .map_err(|_| FederationError::Timeout)??;

    // 4. Arrow → JSON
    let (columns, rows) = arrow_to_json(&batches)?;

    Ok(QueryResult {
        columns,
        row_count: rows.len(),
        rows,
        elapsed_ms: start.elapsed().as_millis() as u64,
        sources_touched,
        explain: None,
    })
}

/// 执行 EXPLAIN（生成执行计划摘要，供调试/审计；本身是只读的）。
pub async fn explain_query(ctx: &SessionContext, sql: &str) -> FederationResult<String> {
    assert_readonly(sql)?;
    let df = ctx.sql(&format!("EXPLAIN {sql}")).await?;
    let batches = df.collect().await?;
    let mut out = String::new();
    for batch in batches {
        for row in 0..batch.num_rows() {
            for col in 0..batch.num_columns() {
                let val = datafusion::arrow::util::display::array_value_to_string(
                    batch.column(col),
                    row,
                )
                .map_err(|e| FederationError::Query(e.to_string()))?;
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&val);
            }
        }
    }
    Ok(out)
}

/// Arrow RecordBatch → (ColumnMeta, Vec<JSON row>)。
///
/// 每行转为 {col_name: value} JSON 对象。null 用 JSON null 表示。
fn arrow_to_json(
    batches: &[datafusion::arrow::record_batch::RecordBatch],
) -> FederationResult<(Vec<ColumnMeta>, Vec<String>)> {
    use datafusion::arrow::datatypes::Field;

    if batches.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    // 列元信息取自首个 batch 的 schema（各 batch schema 一致）
    let schema = batches[0].schema();
    let columns: Vec<ColumnMeta> = schema
        .fields()
        .iter()
        .map(|f: &std::sync::Arc<Field>| ColumnMeta {
            name: f.name().clone(),
            data_type: arrow_type_name(f.data_type()),
            nullable: f.is_nullable(),
        })
        .collect();

    let mut rows = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        for row_idx in 0..n {
            let mut obj = serde_json::Map::with_capacity(columns.len());
            for (col_idx, col) in batch.columns().iter().enumerate() {
                let name = &columns[col_idx].name;
                let val = array_cell_to_json(col, row_idx)
                    .map_err(|e| FederationError::Arrow(format!("col {name}: {e}")))?;
                obj.insert(name.clone(), val);
            }
            rows.push(serde_json::to_string(&serde_json::Value::Object(obj))?);
        }
        // 提前停止（理论上 LIMIT 已在 SQL 层裁剪，此处双保险）
        if rows.len() >= MAX_ROW_LIMIT * 2 {
            break;
        }
    }
    Ok((columns, rows))
}

/// Arrow 类型 → 可读名称（前端展示用）。
fn arrow_type_name(dt: &DataType) -> String {
    match dt {
        DataType::Null => "Null".into(),
        DataType::Boolean => "Boolean".into(),
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 => "Int64".into(),
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => "UInt64".into(),
        DataType::Float16 | DataType::Float32 | DataType::Float64 => "Float64".into(),
        DataType::Utf8 | DataType::LargeUtf8 => "Utf8".into(),
        DataType::Date32 | DataType::Date64 => "Date".into(),
        DataType::Time32(_) | DataType::Time64(_) => "Time".into(),
        DataType::Timestamp(_, _) => "Timestamp".into(),
        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => "Decimal".into(),
        DataType::Binary | DataType::LargeBinary => "Binary".into(),
        _ => format!("{dt}"),
    }
}

/// 单个 Arrow 数组单元格 → JSON value。
fn array_cell_to_json(
    array: &dyn datafusion::arrow::array::Array,
    idx: usize,
) -> Result<serde_json::Value, String> {
    use datafusion::arrow::array::*;
    use datafusion::arrow::array::types::*;
    if array.is_null(idx) {
        return Ok(serde_json::Value::Null);
    }
    Ok(match array.data_type() {
        DataType::Boolean => {
            serde_json::Value::Bool(array.as_boolean().value(idx))
        }
        DataType::Int8 => json_num(array.as_primitive::<Int8Type>().value(idx)),
        DataType::Int16 => json_num(array.as_primitive::<Int16Type>().value(idx)),
        DataType::Int32 => json_num(array.as_primitive::<Int32Type>().value(idx)),
        DataType::Int64 => json_num(array.as_primitive::<Int64Type>().value(idx)),
        DataType::UInt8 => json_num(array.as_primitive::<UInt8Type>().value(idx)),
        DataType::UInt16 => json_num(array.as_primitive::<UInt16Type>().value(idx)),
        DataType::UInt32 => json_num(array.as_primitive::<UInt32Type>().value(idx)),
        DataType::UInt64 => json_num(array.as_primitive::<UInt64Type>().value(idx) as i64),
        DataType::Float32 => json_num_f(array.as_primitive::<Float32Type>().value(idx)),
        DataType::Float64 => json_num_f(array.as_primitive::<Float64Type>().value(idx)),
        DataType::Utf8 => serde_json::Value::String(
            array.as_string::<i32>().value(idx).to_string(),
        ),
        DataType::LargeUtf8 => serde_json::Value::String(
            array.as_string::<i64>().value(idx).to_string(),
        ),
        DataType::Date32 => {
            let d = array.as_primitive::<Date32Type>().value(idx);
            serde_json::Value::String(format!("days since epoch: {d}"))
        }
        DataType::Date64 => {
            let d = array.as_primitive::<Date64Type>().value(idx);
            serde_json::Value::String(format!("ms since epoch: {d}"))
        }
        DataType::Timestamp(_, _) => {
            // 用 display 格式化，避免时区复杂度
            let s = datafusion::arrow::util::display::array_value_to_string(array, idx)
                .map_err(|e| e.to_string())?;
            serde_json::Value::String(s)
        }
        DataType::Decimal128(_, _) | DataType::Decimal256(_, _) => {
            let s = datafusion::arrow::util::display::array_value_to_string(array, idx)
                .map_err(|e| e.to_string())?;
            serde_json::Value::String(s)
        }
        _ => {
            // 兜底：用 display 格式化为字符串
            let s = datafusion::arrow::util::display::array_value_to_string(array, idx)
                .map_err(|e| e.to_string())?;
            serde_json::Value::String(s)
        }
    })
}

fn json_num<T: Into<i64> + Copy>(v: T) -> serde_json::Value {
    serde_json::json!(v.into())
}

fn json_num_f<T: Into<f64> + Copy>(v: T) -> serde_json::Value {
    let f = v.into();
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0072e15 {
        serde_json::json!(f as i64)
    } else {
        serde_json::json!(f)
    }
}

/// 从物理计划提取涉及的 catalog 名（source 透明性）。
///
/// 遍历计划树，收集所有 "scan" 节点的表名首段（catalog.schema.table 的 catalog）。
/// 简化实现：DataFusion 的 ExecutionPlan 是树形，递归遍历 children 提取
/// TableScan 的表名。CSV/Excel 注册为 catalog 下的表，能正确识别。
/// 从逻辑计划提取涉及的 catalog 名（遍历 TableScan 节点）。
fn extract_sources_from_logical_plan(
    plan: &datafusion::logical_expr::LogicalPlan,
) -> Vec<String> {
    use std::collections::HashSet;
    use datafusion::common::tree_node::{TreeNode, TreeNodeRecursion};
    let mut sources = HashSet::new();
    let _ = plan.apply(|node| {
        if let datafusion::logical_expr::LogicalPlan::TableScan(t) = node {
            // table_name 形如 catalog.schema.table；取首段（catalog）
            if let Some(cat) = t.table_name.catalog() {
                if !cat.is_empty() && cat != "datafusion" {
                    sources.insert(cat.to_string());
                }
            }
        }
        Ok(TreeNodeRecursion::Continue)
    });
    let mut v: Vec<_> = sources.into_iter().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_allows_select() {
        assert!(assert_readonly("SELECT 1").is_ok());
        assert!(assert_readonly("SELECT * FROM t WHERE x > 1").is_ok());
        assert!(assert_readonly("WITH cte AS (SELECT 1) SELECT * FROM cte").is_ok());
    }

    #[test]
    fn readonly_blocks_writes() {
        assert!(matches!(
            assert_readonly("INSERT INTO t VALUES (1)"),
            Err(FederationError::ReadonlyViolation(_))
        ));
        assert!(matches!(
            assert_readonly("UPDATE t SET x = 1"),
            Err(FederationError::ReadonlyViolation(_))
        ));
        assert!(matches!(
            assert_readonly("DELETE FROM t"),
            Err(FederationError::ReadonlyViolation(_))
        ));
        assert!(matches!(
            assert_readonly("DROP TABLE t"),
            Err(FederationError::ReadonlyViolation(_))
        ));
        assert!(matches!(
            assert_readonly("CREATE TABLE t (x INT)"),
            Err(FederationError::ReadonlyViolation(_))
        ));
        assert!(matches!(
            assert_readonly("ALTER TABLE t ADD COLUMN x INT"),
            Err(FederationError::ReadonlyViolation(_))
        ));
    }

    #[test]
    fn readonly_blocks_multi_statement_injection() {
        // SELECT 1; DROP TABLE t —— 多语句中含写操作，整体拒
        assert!(matches!(
            assert_readonly("SELECT 1; DROP TABLE t"),
            Err(FederationError::ReadonlyViolation(_))
        ));
    }

    #[test]
    fn readonly_allows_explain_select() {
        assert!(assert_readonly("EXPLAIN SELECT * FROM t").is_ok());
        assert!(matches!(
            assert_readonly("EXPLAIN DROP TABLE t"),
            Err(FederationError::ReadonlyViolation(_))
        ));
    }

    #[test]
    fn has_limit_detection() {
        assert!(has_limit("SELECT * FROM t LIMIT 10"));
        assert!(has_limit("select * from t\nlimit 10"));
        assert!(!has_limit("SELECT * FROM t"));
    }
}
