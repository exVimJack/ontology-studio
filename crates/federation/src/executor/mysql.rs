//! MySQL 执行器（federation SQLExecutor 实现，见 §2.4 方案 D）。
//!
//! 路径：MySqlPool → MysqlExecutor(impl SQLExecutor) → SQLFederationProvider
//!       → 注册为 catalog（跨源 JOIN 自动下推方言 SQL）。
//!
//! execute：federation 下推的方言 SQL（unparser 生成）→ sqlx 执行 →
//!          行流转 Arrow RecordBatch → SendableRecordBatchStream。
//! table_names / get_table_schema：查 MySQL information_schema。
//! dialect：MySqlDialect（sqlparser 自带，federation unparser 用它生成方言 SQL）。
//! compute_context：唯一字符串（如 mysql:host:port:db），避免同名源错误联邦。

use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::common::Result as DfResult;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::prelude::SessionContext;
use datafusion_federation::sql::{SQLExecutor, SQLFederationProvider, SQLSchemaProvider};
use futures::stream;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::Row as _;

use crate::error::{FederationError, FederationResult};
use crate::source::DbConnection;

/// MySQL 执行器：sqlx MySqlPool（rustls，禁 native-tls）。
pub struct MysqlExecutor {
    pool: MySqlPool,
    name: String,
    /// compute_context（唯一标识，避免同名源错误联邦）。
    context: String,
}

impl MysqlExecutor {
    pub async fn new(name: &str, conn: &DbConnection) -> FederationResult<Self> {
        let url = build_mysql_url(name, conn)?;
        tracing::info!(target: "federation::mysql::executor", source = %name, url = %url, "MySqlPool connect begin");
        let t = std::time::Instant::now();
        let pool = MySqlPoolOptions::new()
            .max_connections(4) // 桌面单用户，4 连接够用
            .acquire_timeout(std::time::Duration::from_secs(8))
            .connect(&url)
            .await
            .map_err(|e| FederationError::Connect(format!("MySQL 连接失败: {e}")))?;
        tracing::info!(target: "federation::mysql::executor", source = %name, elapsed = ?t.elapsed(), "MySqlPool connect done");
        let context = format!("mysql:{}:{}:{}", conn.host, conn.port, conn.database);
        Ok(Self {
            pool,
            name: name.to_string(),
            context,
        })
    }
}

#[async_trait]
impl SQLExecutor for MysqlExecutor {
    fn name(&self) -> &str {
        &self.name
    }

    fn compute_context(&self) -> Option<String> {
        // 必须返回唯一字符串，None 会导致同名源错误联邦（docs.rs 警告）。
        Some(self.context.clone())
    }

    fn dialect(&self) -> Arc<dyn datafusion::sql::unparser::dialect::Dialect> {
        Arc::new(datafusion::sql::unparser::dialect::MySqlDialect {})
    }

    fn execute(
        &self,
        query: &str,
        schema: SchemaRef,
        _filters: &[Arc<dyn PhysicalExpr>],
    ) -> DfResult<SendableRecordBatchStream> {
        // federation 把下推的方言 SQL（unparser 生成）传入。
        // sqlx 执行 → 行流 → Arrow RecordBatch stream。
        // filters 含运行时物理表达式（如 DynamicFilter），可安全忽略（docs.rs 说明）。
        let pool = self.pool.clone();
        let query = query.to_string();
        // adapter 与 stream 各需一份 schema（Arc clone，便宜）
        let stream_schema = schema.clone();
        let adapter_schema = schema.clone();
        // 用 stream::once 包裹 async，产生单个 batch 后结束。
        // 大结果集会一次性入内存——三期单用户只读 + LIMIT 200 兜底，可接受。
        let s = stream::once(async move {
            match rows_to_batch(&pool, query, stream_schema).await {
                Ok(batch) => Ok(batch),
                Err(e) => Err(datafusion::common::DataFusionError::Execution(format!(
                    "mysql execute: {e}"
                ))),
            }
        });
        Ok(Box::pin(datafusion::physical_plan::stream::RecordBatchStreamAdapter::new(
            adapter_schema,
            s,
        )))
    }

    async fn table_names(&self) -> DfResult<Vec<String>> {
        let rows: Vec<MySqlRow> = sqlx::query(
            "SELECT table_name FROM information_schema.tables
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'
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
        let rows: Vec<MySqlRow> = sqlx::query(
            "SELECT column_name, data_type, is_nullable
             FROM information_schema.columns
             WHERE table_schema = DATABASE() AND table_name = ?
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
                    mysql_type_to_arrow(&ty),
                    nullable.eq_ignore_ascii_case("YES"),
                )
            })
            .collect();
        Ok(Arc::new(Schema::new(fields)))
    }
}

/// 注册 MySQL 数据源为 catalog。
pub async fn register(
    ctx: &SessionContext,
    name: &str,
    conn: &DbConnection,
) -> FederationResult<crate::source::SchemaSnapshot> {
    let t0 = std::time::Instant::now();
    tracing::info!(target: "federation::mysql::register", source = %name, "MysqlExecutor::new begin");
    let executor: Arc<dyn SQLExecutor> = Arc::new(MysqlExecutor::new(name, conn).await?);
    tracing::info!(target: "federation::mysql::register", source = %name, elapsed = ?t0.elapsed(), "MysqlExecutor::new done");
    // 1. 包成 federation provider（executor 是公共字段）
    let fed_provider = Arc::new(SQLFederationProvider::new(executor));
    // 2. 包成 schema provider（new 是 async，返回 Result；实现 SchemaProvider）
    let t1 = std::time::Instant::now();
    let table_names = fed_provider.executor.table_names().await.map_err(|e| FederationError::Query(format!("list tables: {e}")))?;
    tracing::info!(target: "federation::mysql::register", source = %name, n = table_names.len(), elapsed = ?t1.elapsed(), "table_names done");
    let t2 = std::time::Instant::now();
    let schema_provider = Arc::new(
        SQLSchemaProvider::new(Arc::clone(&fed_provider))
            .await
            .map_err(|e| FederationError::Query(format!("schema provider: {e}")))?,
    );
    tracing::info!(target: "federation::mysql::register", source = %name, elapsed = ?t2.elapsed(), total = ?t0.elapsed(), "SQLSchemaProvider::new done");
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
    // 3. 注册为 catalog
    let catalog = Arc::new(datafusion::catalog::MemoryCatalogProvider::new());
    catalog
        .register_schema("public", schema_provider)
        .map_err(|e| FederationError::Query(format!("register schema: {e}")))?;
    ctx.register_catalog(name, catalog);
    Ok(crate::source::SchemaSnapshot { tables })
}

/// MySQL type name → Arrow DataType（常用映射）。
fn mysql_type_to_arrow(ty: &str) -> DataType {
    let t = ty.to_lowercase();
    match t.as_str() {
        "tinyint" | "smallint" | "mediumint" | "int" | "integer" => DataType::Int64,
        "bigint" => DataType::Int64,
        "float" => DataType::Float64,
        "double" | "decimal" | "numeric" => DataType::Float64,
        "bit" | "bool" | "boolean" => DataType::Boolean,
        "date" => DataType::Date32,
        "datetime" | "timestamp" => DataType::Utf8, // 时间统一字符串，避免时区复杂度
        "time" => DataType::Utf8,
        "char" | "varchar" | "text" | "tinytext" | "mediumtext" | "longtext"
        | "enum" | "set" | "json" | "blob" | "binary" | "varbinary" => DataType::Utf8,
        _ => DataType::Utf8,
    }
}

/// 构造 MySQL 连接 URL（rustls，禁 native-tls）。
fn build_mysql_url(_name: &str, conn: &DbConnection) -> FederationResult<String> {
    // sqlx MySQL ssl-mode 接受枚举值：disabled / preferred / required / verify_ca / verify_identity
    // 我们的 ssl_mode 字段用三档范式：disable / require / verify（Beekeeper 风格）
    let ssl = match conn.ssl_mode.to_lowercase().as_str() {
        "disable" => "disabled",
        "require" => "required",
        "verify" => "verify_identity", // 桌面单用户，校验证书身份
        _ => "preferred",              // 未知值兜底为 sqlx 默认（尝试 TLS，失败回退明文）
    };
    let pass = conn
        .password
        .as_deref()
        .map(url_escape)
        .unwrap_or_default();
    let user_enc = url_escape(&conn.username);
    let host = crate::source::normalize_host(&conn.host);
    Ok(format!(
        "mysql://{user_enc}:{pass}@{host}:{}/{}?ssl-mode={ssl}",
        conn.port, conn.database
    ))
}

/// URL 转义（密码含特殊字符）。
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

/// sqlx 行流 → 单个 RecordBatch（全量入内存）。
async fn rows_to_batch(
    pool: &MySqlPool,
    query: String,
    schema: SchemaRef,
) -> FederationResult<RecordBatch> {
    use datafusion::arrow::array::*;
    let rows: Vec<MySqlRow> = sqlx::raw_sql(sqlx::AssertSqlSafe(query)).fetch_all(pool).await?;
    if rows.is_empty() {
        return Ok(RecordBatch::new_empty(schema));
    }
    // 按 schema 列构造 builders
    let n_cols = schema.fields().len();
    let mut builders: Vec<Box<dyn ArrayBuilder>> = Vec::with_capacity(n_cols);
    for f in schema.fields() {
        let b: Box<dyn ArrayBuilder> = match f.data_type() {
            DataType::Boolean => Box::new(BooleanBuilder::new()),
            DataType::Int64 => Box::new(Int64Builder::new()),
            DataType::Float64 => Box::new(Float64Builder::new()),
            DataType::Date32 => Box::new(Date32Builder::new()),
            _ => Box::new(StringBuilder::new()),
        };
        builders.push(b);
    }

    let col_info: Vec<sqlx::mysql::MySqlColumn> = {
        if rows.is_empty() {
            Vec::new()
        } else {
            // 所有行共享同一列信息（同一 query）
            rows[0].columns().to_vec()
        }
    };

    for row in &rows {
        for (i, builder) in builders.iter_mut().enumerate() {
            if i >= col_info.len() {
                // 列数不匹配，补 null
                continue;
            }
            let col_idx = i;
            // 用字符串统一取值，按 builder 类型转换
            let dt = schema.field(i).data_type();
            match dt {
                DataType::Int64 => {
                    let b = builder.as_any_mut().downcast_mut::<Int64Builder>().unwrap();
                    match row.try_get::<Option<i64>, _>(col_idx) {
                        Ok(Some(v)) => b.append_value(v),
                        _ => b.append_null(),
                    }
                }
                DataType::Float64 => {
                    let b = builder.as_any_mut().downcast_mut::<Float64Builder>().unwrap();
                    match row.try_get::<Option<f64>, _>(col_idx) {
                        Ok(Some(v)) => b.append_value(v),
                        _ => b.append_null(),
                    }
                }
                DataType::Boolean => {
                    let b = builder
                        .as_any_mut()
                        .downcast_mut::<BooleanBuilder>()
                        .unwrap();
                    match row.try_get::<Option<bool>, _>(col_idx) {
                        Ok(Some(v)) => b.append_value(v),
                        _ => b.append_null(),
                    }
                }
                _ => {
                    let b = builder
                        .as_any_mut()
                        .downcast_mut::<StringBuilder>()
                        .unwrap();
                    // 兜底：尝试用 String 取，失败则用 display
                    match row.try_get::<Option<String>, _>(col_idx) {
                        Ok(Some(v)) => b.append_value(&v),
                        _ => {
                            // 尝试 i64/f64/bool 再转字符串
                            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(col_idx) {
                                b.append_value(v.to_string());
                            } else if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(col_idx) {
                                b.append_value(v.to_string());
                            } else {
                                b.append_null();
                            }
                        }
                    }
                }
            }
        }
    }

    let arrays: Vec<Arc<dyn Array>> = builders
        .into_iter()
        .map(|mut b| b.finish())
        .map(|a| Arc::new(a) as Arc<dyn Array>)
        .collect();
    RecordBatch::try_new(schema, arrays).map_err(|e| FederationError::Arrow(e.to_string()))
}
