//! schema: 表结构探查（见 PHASE3-FEDERATION.md §3.1 describe_table）。
//!
//! 统一走 DataFusion information_schema（§2.2），agent 工具透明，不需记各源原生方言。

use datafusion::arrow::datatypes::DataType;
use datafusion::prelude::SessionContext;

use crate::error::{FederationError, FederationResult};
use crate::source::{ColumnMeta, SchemaSnapshot, TableMeta};

/// 浏览 catalog 下所有表结构（不含样本行）。
pub async fn browse_schema(ctx: &SessionContext, catalog: &str) -> FederationResult<SchemaSnapshot> {
    let t0 = std::time::Instant::now();
    let tables = crate::catalog::list_tables_in_catalog(ctx, catalog).await?;
    tracing::info!(target: "federation::browse_schema", catalog = %catalog, n = tables.len(), elapsed = ?t0.elapsed(), "list_tables done");
    let mut metas = Vec::with_capacity(tables.len());
    let mut slow_tables = Vec::new();
    for t in &tables {
        let tt = std::time::Instant::now();
        // 单表列信息
        let columns = describe_columns(ctx, catalog, t).await?;
        let d = tt.elapsed();
        if d > std::time::Duration::from_millis(200) {
            slow_tables.push((t.clone(), d));
        }
        metas.push(TableMeta {
            name: t.clone(),
            columns,
            row_count_estimate: None,
            sample_rows: Vec::new(),
        });
    }
    if !slow_tables.is_empty() {
        tracing::warn!(target: "federation::browse_schema", catalog = %catalog, slow = ?slow_tables, total = ?t0.elapsed(), "slow describe_columns (>200ms each)");
    } else {
        tracing::info!(target: "federation::browse_schema", catalog = %catalog, total = ?t0.elapsed(), "all describe_columns done");
    }
    Ok(SchemaSnapshot { tables: metas })
}

/// 描述单表：列名/类型/可空 + 前 5 行样本（describe_table 工具）。
pub async fn describe_table(
    ctx: &SessionContext,
    catalog: &str,
    table: &str,
) -> FederationResult<TableMeta> {
    let columns = describe_columns(ctx, catalog, table).await?;

    // 前 5 行样本（三段式寻址）
    let ref_str = format!("{catalog}.public.{table}");
    let sample_sql = format!("SELECT * FROM {ref_str} LIMIT 5");
    let sample_rows = match ctx.sql(&sample_sql).await {
        Ok(df) => match df.collect().await {
            Ok(batches) => batches_to_json_rows(&batches)?,
            Err(e) => {
                tracing::warn!(error = %e, table = %ref_str, "sample rows failed");
                Vec::new()
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, table = %ref_str, "sample query failed");
            Vec::new()
        }
    };

    // 行数估计（小表 COUNT，大表可能慢，超时则跳过）
    let row_count = estimate_row_count(ctx, catalog, table).await;

    Ok(TableMeta {
        name: table.to_string(),
        columns,
        row_count_estimate: row_count,
        sample_rows,
    })
}

/// 从 information_schema.columns 取列信息。
async fn describe_columns(
    ctx: &SessionContext,
    catalog: &str,
    table: &str,
) -> FederationResult<Vec<ColumnMeta>> {
    let sql = format!(
        "SELECT column_name, data_type, is_nullable
         FROM {catalog}.information_schema.columns
         WHERE table_schema = 'public' AND table_name = '{table}'
         ORDER BY ordinal_position"
    );
    let df = ctx.sql(&sql).await.map_err(|e| FederationError::Query(e.to_string()))?;
    let batches = df.collect().await.map_err(|e| FederationError::Query(e.to_string()))?;

    let mut cols = Vec::new();
    for batch in batches {
        let n = batch.num_rows();
        for i in 0..n {
            let name = cell_string(batch.column(0), i)?;
            let data_type = cell_string(batch.column(1), i)?;
            let nullable_str = cell_string(batch.column(2), i).unwrap_or_else(|_| "YES".into());
            cols.push(ColumnMeta {
                name,
                data_type: normalize_type(&data_type),
                nullable: nullable_str.eq_ignore_ascii_case("YES"),
            });
        }
    }

    // information_schema 可能没数据（某些 provider 未实现）→ 回退到 TableProvider.schema()
    if cols.is_empty() {
        let ref_str = format!("{catalog}.public.{table}");
        if let Ok(provider) = ctx.table_provider(ref_str).await {
            let schema = provider.schema();
            for f in schema.fields() {
                cols.push(ColumnMeta {
                    name: f.name().clone(),
                    data_type: arrow_type_name(f.data_type()),
                    nullable: f.is_nullable(),
                });
            }
        }
    }

    Ok(cols)
}

/// 估算行数（COUNT(*)，超时或失败返回 None）。
async fn estimate_row_count(
    ctx: &SessionContext,
    catalog: &str,
    table: &str,
) -> Option<i64> {
    let sql = format!("SELECT COUNT(*) FROM {catalog}.public.{table}");
    let fut = async {
        let df = ctx.sql(&sql).await?;
        df.collect().await
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), fut).await;
    match result {
        Ok(Ok(batches)) => {
            if let Some(b) = batches.first() {
                if b.num_rows() > 0 {
                    return datafusion::arrow::util::display::array_value_to_string(b.column(0), 0)
                        .ok()
                        .and_then(|s| s.parse().ok());
                }
            }
            None
        }
        _ => None,
    }
}

fn cell_string(
    array: &dyn datafusion::arrow::array::Array,
    idx: usize,
) -> Result<String, FederationError> {
    if array.is_null(idx) {
        return Ok(String::new());
    }
    datafusion::arrow::util::display::array_value_to_string(array, idx)
        .map_err(|e| FederationError::Query(e.to_string()))
}

/// information_schema.data_type（如 "INTEGER"/"VARCHAR"）→ 友好名称。
fn normalize_type(t: &str) -> String {
    let upper = t.to_uppercase();
    match upper.as_str() {
        "INTEGER" | "INT" | "INT4" | "BIGINT" | "INT8" | "SMALLINT" | "INT2" => "Int64",
        "REAL" | "FLOAT4" | "FLOAT" | "DOUBLE" | "FLOAT8" => "Float64",
        "TEXT" | "VARCHAR" | "CHAR" | "CHARACTER" | "STRING" => "Utf8",
        "BOOLEAN" | "BOOL" => "Boolean",
        "DATE" => "Date",
        "TIMESTAMP" | "DATETIME" => "Timestamp",
        _ => &upper,
    }
    .to_string()
}

pub fn arrow_type_name(dt: &DataType) -> String {
    match dt {
        DataType::Null => "Null",
        DataType::Boolean => "Boolean",
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 => "Int64",
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => "UInt64",
        DataType::Float16 | DataType::Float32 | DataType::Float64 => "Float64",
        DataType::Utf8 | DataType::LargeUtf8 => "Utf8",
        DataType::Date32 | DataType::Date64 => "Date",
        DataType::Timestamp(_, _) => "Timestamp",
        _ => "Other",
    }
    .into()
}

/// RecordBatch → JSON 行数组（前 5 行样本用）。
fn batches_to_json_rows(
    batches: &[datafusion::arrow::record_batch::RecordBatch],
) -> FederationResult<Vec<String>> {
    use datafusion::arrow::array::Array;
    let mut rows = Vec::new();
    for batch in batches {
        let schema = batch.schema();
        let n = batch.num_rows();
        for i in 0..n {
            let mut obj = serde_json::Map::new();
            for (j, field) in schema.fields().iter().enumerate() {
                let col = batch.column(j);
                let val = if col.is_null(i) {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(
                        datafusion::arrow::util::display::array_value_to_string(col, i)
                            .unwrap_or_default(),
                    )
                };
                obj.insert(field.name().clone(), val);
            }
            rows.push(serde_json::to_string(&serde_json::Value::Object(obj))?);
        }
    }
    Ok(rows)
}
