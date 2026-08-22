//! PostgreSQL 执行器（federation SQLExecutor 实现，见 §2.4 方案 D）。
//!
//! 与 mysql.rs 同构：PgPool → PostgresExecutor(impl SQLExecutor) →
//! SQLFederationProvider → 注册为 catalog。

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::array::ArrayBuilder;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::Result as DfResult;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::prelude::SessionContext;
use datafusion_federation::sql::{SQLExecutor, SQLFederationProvider, SQLSchemaProvider};
use futures::stream;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::Row as _;

use crate::error::{FederationError, FederationResult};
use crate::source::DbConnection;

/// PostgreSQL 执行器：sqlx PgPool（rustls，禁 native-tls）。
pub struct PostgresExecutor {
    pool: PgPool,
    name: String,
    context: String,
}

impl PostgresExecutor {
    pub async fn new(name: &str, conn: &DbConnection) -> FederationResult<Self> {
        let url = build_pg_url(name, conn)?;
        tracing::info!(target: "federation::pg::executor", source = %name, url = %url, "PgPool connect begin");
        let t = std::time::Instant::now();
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(std::time::Duration::from_secs(8))
            .connect(&url)
            .await
            .map_err(|e| FederationError::Connect(format!("PostgreSQL 连接失败: {e}")))?;
        tracing::info!(target: "federation::pg::executor", source = %name, elapsed = ?t.elapsed(), "PgPool connect done");
        let context = format!("postgres:{}:{}:{}", conn.host, conn.port, conn.database);
        Ok(Self {
            pool,
            name: name.to_string(),
            context,
        })
    }
}

#[async_trait]
impl SQLExecutor for PostgresExecutor {
    fn name(&self) -> &str {
        &self.name
    }

    fn compute_context(&self) -> Option<String> {
        Some(self.context.clone())
    }

    fn dialect(&self) -> Arc<dyn datafusion::sql::unparser::dialect::Dialect> {
        Arc::new(datafusion::sql::unparser::dialect::PostgreSqlDialect {})
    }

    fn execute(
        &self,
        query: &str,
        schema: SchemaRef,
        _filters: &[Arc<dyn PhysicalExpr>],
    ) -> DfResult<SendableRecordBatchStream> {
        let pool = self.pool.clone();
        let query = query.to_string();
        let stream_schema = schema.clone();
        let adapter_schema = schema.clone();
        let s = stream::once(async move {
            match rows_to_batch(&pool, query, stream_schema).await {
                Ok(batch) => Ok(batch),
                Err(e) => Err(datafusion::common::DataFusionError::Execution(format!(
                    "postgres execute: {e}"
                ))),
            }
        });
        Ok(Box::pin(
            datafusion::physical_plan::stream::RecordBatchStreamAdapter::new(
                adapter_schema,
                s,
            ),
        ))
    }

    async fn table_names(&self) -> DfResult<Vec<String>> {
        let rows: Vec<PgRow> = sqlx::query(
            "SELECT table_name FROM information_schema.tables
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
             ORDER BY table_name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| datafusion::common::DataFusionError::Execution(format!("table_names: {e}")))?;
        Ok(rows
            .into_iter()
            .map(|r| r.try_get::<String, _>(0).unwrap_or_default())
            .collect())
    }

    async fn get_table_schema(&self, table_name: &str) -> DfResult<SchemaRef> {
        let rows: Vec<PgRow> = sqlx::query(
            "SELECT column_name, data_type, is_nullable
             FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1
             ORDER BY ordinal_position",
        )
        .bind(table_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| datafusion::common::DataFusionError::Execution(format!("get_table_schema: {e}")))?;
        let fields: Vec<Field> = rows
            .into_iter()
            .map(|r| {
                let name: String = r.try_get(0).unwrap_or_default();
                let ty: String = r.try_get(1).unwrap_or_default();
                let nullable: String = r.try_get(2).unwrap_or_else(|_| "YES".into());
                Field::new(
                    name,
                    pg_type_to_arrow(&ty),
                    nullable.eq_ignore_ascii_case("YES"),
                )
            })
            .collect();
        Ok(Arc::new(Schema::new(fields)))
    }
}

/// 注册 PostgreSQL 数据源为 catalog。
pub async fn register(
    ctx: &SessionContext,
    name: &str,
    conn: &DbConnection,
) -> FederationResult<crate::source::SchemaSnapshot> {
    let t0 = std::time::Instant::now();
    tracing::info!(target: "federation::pg::register", source = %name, "PostgresExecutor::new begin");
    let executor: Arc<dyn SQLExecutor> = Arc::new(PostgresExecutor::new(name, conn).await?);
    tracing::info!(target: "federation::pg::register", source = %name, elapsed = ?t0.elapsed(), "PostgresExecutor::new done (pool connected)");
    let fed_provider = Arc::new(SQLFederationProvider::new(executor));
    let t1 = std::time::Instant::now();
    let table_names = fed_provider.executor.table_names().await.map_err(|e| FederationError::Query(format!("list tables: {e}")))?;
    tracing::info!(target: "federation::pg::register", source = %name, n = table_names.len(), elapsed = ?t1.elapsed(), "table_names done");
    let t2 = std::time::Instant::now();
    let schema_provider = Arc::new(
        SQLSchemaProvider::new(Arc::clone(&fed_provider))
            .await
            .map_err(|e| FederationError::Query(format!("schema provider: {e}")))?,
    );
    tracing::info!(target: "federation::pg::register", source = %name, elapsed = ?t2.elapsed(), total = ?t0.elapsed(), "SQLSchemaProvider::new done (fetched all table schemas)");
    use datafusion::catalog::{CatalogProvider, SchemaProvider};
    // 从 schema_provider 缓存生成单源精确快照（不走联邦 information_schema，避免跨 catalog 合并）
    let mut tables = Vec::with_capacity(table_names.len());
    for tn in &table_names {
        if let Ok(Some(tp)) = schema_provider.table(tn).await {
            let schema = tp.schema();
            let columns = schema.fields().iter().map(|f| crate::source::ColumnMeta {
                name: f.name().clone(),
                data_type: crate::schema::arrow_type_name(f.data_type()),
                nullable: f.is_nullable(),
            }).collect();
            tables.push(crate::source::TableMeta {
                name: tn.clone(), columns,
                row_count_estimate: None, sample_rows: Vec::new(),
            });
        }
    }
    let catalog = Arc::new(datafusion::catalog::MemoryCatalogProvider::new());
    catalog
        .register_schema("public", schema_provider)
        .map_err(|e| FederationError::Query(format!("register schema: {e}")))?;
    ctx.register_catalog(name, catalog);
    Ok(crate::source::SchemaSnapshot { tables })
}

/// PostgreSQL `information_schema.columns.data_type` 字符串 → Arrow DataType。
///
/// 对齐 spice `map_column_type_to_data_type`：
/// - timestamp/timestamptz → `Timestamp(Nanosecond)`（保留精度，不用 Utf8）
/// - date → `Date32`
/// - json/jsonb/uuid/text/varchar → `Utf8`
/// - int2/int4 → `Int32`，int8 → `Int64`
/// - float4 → `Float32`，float8/numeric → `Float64`
/// - bool → `Boolean`
fn pg_type_to_arrow(ty: &str) -> DataType {
    let t = ty.to_lowercase();
    // timestamp 类型在 information_schema 里是 "timestamp without time zone" / "timestamp with time zone"
    if t.starts_with("timestamp") {
        if t.contains("with time zone") {
            DataType::Timestamp(datafusion::arrow::datatypes::TimeUnit::Nanosecond, Some("UTC".into()))
        } else {
            DataType::Timestamp(datafusion::arrow::datatypes::TimeUnit::Nanosecond, None)
        }
    } else {
        match t.as_str() {
            "smallint" | "int2" => DataType::Int16,
            "integer" | "int" | "int4" | "serial" => DataType::Int32,
            "bigint" | "int8" | "bigserial" => DataType::Int64,
            "real" | "float4" => DataType::Float32,
            "double precision" | "float8" | "numeric" | "decimal" => DataType::Float64,
            "boolean" | "bool" => DataType::Boolean,
            "date" => DataType::Date32,
            "time" | "time without time zone" | "timetz" | "time with time zone" => {
                DataType::Time64(datafusion::arrow::datatypes::TimeUnit::Nanosecond)
            }
            // text/varchar/char/uuid/json/jsonb/bytea/未知 → Utf8（前端友好，避免列式类型爆炸）
            _ => DataType::Utf8,
        }
    }
}

/// 构造 PostgreSQL 连接 URL（rustls，禁 native-tls）。
fn build_pg_url(_name: &str, conn: &DbConnection) -> FederationResult<String> {
    // sqlx postgres 用 sslmode 参数：disable/require/verify-full
    let sslmode = match conn.ssl_mode.to_lowercase().as_str() {
        "disable" => "disable",
        "verify" => "verify-full",
        _ => "require",
    };
    let pass = conn.password.as_deref().map(url_escape).unwrap_or_default();
    let user_enc = url_escape(&conn.username);
    let host = crate::source::normalize_host(&conn.host);
    Ok(format!(
        "postgres://{user_enc}:{pass}@{host}:{}/{}?sslmode={sslmode}",
        conn.port, conn.database
    ))
}

fn url_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '@' => "%40".into(),
            ':' => "%3A".into(),
            '/' => "%2F".into(),
            '#' => "%23".into(),
            '?' => "%3F".into(),
            '&' => "%26".into(),
            ' ' => "%20".into(),
            _ => c.to_string(),
        })
        .collect()
}

/// sqlx 行 → 单个 RecordBatch。
async fn rows_to_batch(
    pool: &PgPool,
    query: String,
    schema: SchemaRef,
) -> FederationResult<RecordBatch> {
    use datafusion::arrow::array::*;
    let rows: Vec<PgRow> = sqlx::raw_sql(sqlx::AssertSqlSafe(query)).fetch_all(pool).await?;
    if rows.is_empty() {
        return Ok(RecordBatch::new_empty(schema));
    }
    let n_cols = schema.fields().len();

    // 按 Arrow DataType 创建对应 builder（一对应关系，避免类型猜测）
    let mut builders: Vec<Box<dyn ArrayBuilder>> = Vec::with_capacity(n_cols);
    for f in schema.fields() {
        builders.push(make_array_builder(f.data_type()));
    }

    let col_count = rows[0].columns().len();
    for row in &rows {
        for i in 0..n_cols.min(col_count) {
            let dt = schema.field(i).data_type();
            append_cell(&mut builders[i], dt, row, i);
        }
    }

    let arrays: Vec<Arc<dyn Array>> = builders
        .into_iter()
        .map(|mut b| b.finish())
        .map(|a| Arc::new(a) as Arc<dyn Array>)
        .collect();
    RecordBatch::try_new(schema, arrays).map_err(|e| FederationError::Arrow(e.to_string()))
}

/// 按 Arrow DataType 创建对应 builder（一一对应，避免类型猜测）。
///
/// 架构对齐 spice `map_data_type_to_array_builder`：
/// 每种 Arrow 类型有专用 builder + 专用读取路径（见 `append_cell`）。
fn make_array_builder(dt: &DataType) -> Box<dyn ArrayBuilder> {
    use datafusion::arrow::array::*;
    match dt {
        DataType::Boolean => Box::new(BooleanBuilder::new()),
        DataType::Int16 => Box::new(Int16Builder::new()),
        DataType::Int32 => Box::new(Int32Builder::new()),
        DataType::Int64 => Box::new(Int64Builder::new()),
        DataType::Float32 => Box::new(Float32Builder::new()),
        DataType::Float64 => Box::new(Float64Builder::new()),
        DataType::Date32 => Box::new(Date32Builder::new()),
        DataType::Timestamp(_, tz) => {
            if tz.is_some() {
                Box::new(TimestampNanosecondBuilder::new().with_timezone("UTC"))
            } else {
                Box::new(TimestampNanosecondBuilder::new())
            }
        }
        // text/varchar/char/uuid/json/jsonb/未知 → Utf8（前端友好）
        _ => Box::new(StringBuilder::new()),
    }
}

/// 按 Arrow DataType 从 sqlx PgRow 读取该列值并 append 到 builder。
///
/// 架构对齐 spice `rows_to_arrow` 的 `match *postgres_type`：
/// 每种类型用 sqlx 文档明确的 decode 类型（见 sqlx/postgres/types），
/// 避免用错误类型读取导致 NULL 错误。
#[allow(clippy::too_many_lines)]
fn append_cell(builder: &mut Box<dyn ArrayBuilder>, dt: &DataType, row: &PgRow, i: usize) {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
    use datafusion::arrow::array::*;
    match dt {
        DataType::Boolean => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<BooleanBuilder>() {
                match row.try_get::<Option<bool>, _>(i) {
                    Ok(Some(v)) => b.append_value(v),
                    _ => b.append_null(),
                }
            }
        }
        DataType::Int16 => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<Int16Builder>() {
                match row.try_get::<Option<i16>, _>(i) {
                    Ok(Some(v)) => b.append_value(v),
                    _ => b.append_null(),
                }
            }
        }
        DataType::Int32 => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<Int32Builder>() {
                match row.try_get::<Option<i32>, _>(i) {
                    Ok(Some(v)) => b.append_value(v),
                    _ => b.append_null(),
                }
            }
        }
        DataType::Int64 => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<Int64Builder>() {
                match row.try_get::<Option<i64>, _>(i) {
                    Ok(Some(v)) => b.append_value(v),
                    _ => b.append_null(),
                }
            }
        }
        DataType::Float32 => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<Float32Builder>() {
                match row.try_get::<Option<f32>, _>(i) {
                    Ok(Some(v)) => b.append_value(v),
                    _ => b.append_null(),
                }
            }
        }
        DataType::Float64 => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<Float64Builder>() {
                match row.try_get::<Option<f64>, _>(i) {
                    Ok(Some(v)) => b.append_value(v),
                    _ => b.append_null(),
                }
            }
        }
        DataType::Date32 => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<Date32Builder>() {
                // sqlx: NaiveDate ↔ DATE
                match row.try_get::<Option<NaiveDate>, _>(i) {
                    Ok(Some(d)) => {
                        // epoch 1970-01-01 起的天数
                        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                        let days = d.signed_duration_since(epoch).num_days() as i32;
                        b.append_value(days);
                    }
                    _ => b.append_null(),
                }
            }
        }
        DataType::Timestamp(_, tz) => {
            if tz.is_some() {
                // TIMESTAMPTZ → chrono::DateTime<Utc>
                if let Some(b) = builder
                    .as_any_mut()
                    .downcast_mut::<TimestampNanosecondBuilder>()
                {
                    match row.try_get::<Option<DateTime<Utc>>, _>(i) {
                        Ok(Some(dt)) => b.append_value(dt.timestamp_nanos_opt().unwrap_or(0)),
                        _ => b.append_null(),
                    }
                }
            } else if let Some(b) = builder
                .as_any_mut()
                .downcast_mut::<TimestampNanosecondBuilder>()
            {
                // TIMESTAMP（无时区）→ chrono::NaiveDateTime
                match row.try_get::<Option<NaiveDateTime>, _>(i) {
                    Ok(Some(ndt)) => b.append_value(ndt.and_utc().timestamp_nanos_opt().unwrap_or(0)),
                    _ => b.append_null(),
                }
            }
        }
        // Utf8 兜底：text/varchar/char/uuid/json/jsonb/未知
        // 先试 String（覆盖 text/varchar/char），失败试 serde_json::Value（json/jsonb）、uuid::Uuid
        _ => {
            if let Some(b) = builder.as_any_mut().downcast_mut::<StringBuilder>() {
                if let Ok(Some(v)) = row.try_get::<Option<String>, _>(i) {
                    b.append_value(&v);
                } else if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Uuid>, _>(i) {
                    b.append_value(v.to_string());
                } else if let Ok(Some(v)) = row.try_get::<Option<serde_json::Value>, _>(i) {
                    b.append_value(v.to_string());
                } else {
                    b.append_null();
                }
            }
        }
    }
}
