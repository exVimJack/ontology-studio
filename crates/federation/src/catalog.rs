//! catalog: 数据源注册/注销/探测到 SessionContext（热增删，见 §2.5 配套设计 2）。
//!
//! 各 kind 注册路径：
//!   - CSV：datafusion 内置 `register_csv`（零 SQLExecutor，§4.1 executor/csv.rs）
//!   - Excel：calamine 读 → MemTable 注册（复用 ingest，§4.1 executor/excel.rs）
//!   - MySQL/PG：federation SQLExecutor → SQLFederationProvider → 注册为 catalog
//!     （§2.4 方案 D；实现见 executor 模块）
//!
//! 每个数据源注册为一个 catalog，跨源 JOIN 用三段式 `catalog.schema.table`。

use std::sync::Arc;

use datafusion::prelude::SessionContext;

use crate::error::{FederationError, FederationResult};
use crate::executor;
use crate::source::{ConnectionConfig, DataSourceConfig, SchemaSnapshot};

/// 注册数据源到 catalog。返回 schema 快照（单源精确，不走联邦 information_schema，
/// 避免相同 compute_context 的多 catalog 表名合并）。
pub async fn register_source(
    ctx: &SessionContext,
    config: &DataSourceConfig,
) -> FederationResult<SchemaSnapshot> {
    match &config.connection {
        ConnectionConfig::Csv(file) => register_csv(ctx, &config.name, file).await,
        ConnectionConfig::Excel(file) => register_excel(ctx, &config.name, file).await,
        ConnectionConfig::Mysql(db) => executor::mysql::register(ctx, &config.name, db).await,
        ConnectionConfig::Postgres(db) => executor::postgres::register(ctx, &config.name, db).await,
    }
}

/// 注销数据源 catalog。
///
/// DataFusion 54 的 SessionContext 无 `deregister_catalog` 方法，
/// 这里用**空 catalog 覆盖**同名注册：旧 catalog 被替换，表/schema 全部下线。
/// 这是联邦场景下唯一的“热注销”手段（空 MemoryCatalogProvider 无 schema 无表）。
/// 注：进程内残留的 SQLFederationProvider 连接池会在下次同名注册时被新池替换；
/// sqlx Pool drop 时关闭连接。三期单用户场景下足够。
pub fn deregister_source(ctx: &SessionContext, name: &str) {
    use datafusion::catalog::{CatalogProvider, MemoryCatalogProvider};
    let empty: Arc<dyn CatalogProvider> = Arc::new(MemoryCatalogProvider::new());
    // register_catalog 同名覆盖（SessionContext 内部 HashMap::insert 替换旧值）
    ctx.register_catalog(name, empty);
    tracing::info!(source = name, "deregister_source: 已用空 catalog 覆盖（54 无直接注销 API）");
}

/// 探测数据源连接状态（list_sources 用，轻量：仅检查 catalog 是否可列出表）。
pub async fn probe_source(
    ctx: &SessionContext,
    config: &DataSourceConfig,
) -> (bool, Option<usize>, Option<String>) {
    // 已注册的源：尝试列 catalog 下表
    match list_tables_in_catalog(ctx, &config.name).await {
        Ok(tables) => (true, Some(tables.len()), None),
        Err(_e) => {
            // 未注册或注册失败：尝试临时注册探测
            match register_source(ctx, config).await {
                Ok(snap) => {
                    // 注册成功但 list 失败的情况罕见，以注册成功为准
                    (true, Some(snap.tables.len()), None)
                }
                Err(e2) => (false, None, Some(e2.to_string())),
            }
        }
    }
}

/// 列出 catalog 下 public schema 的所有表名（通过目标 catalog 的 information_schema）。
///
/// 用三段式 `{catalog}.information_schema.tables` 查询，避免误列默认 catalog 的系统表。
/// 过滤 `table_schema = 'public'`（排除 information_schema 自身的表）。
pub async fn list_tables_in_catalog(
    ctx: &SessionContext,
    catalog: &str,
) -> FederationResult<Vec<String>> {
    let sql = format!(
        "SELECT table_name FROM {catalog}.information_schema.tables \
         WHERE table_schema = 'public' ORDER BY table_name"
    );
    let df = ctx.sql(&sql).await.map_err(|e| FederationError::Query(e.to_string()))?;
    let batches = df.collect().await.map_err(|e| FederationError::Query(e.to_string()))?;
    let mut tables = Vec::new();
    for batch in batches {
        let col = batch.column(0);
        for i in 0..batch.num_rows() {
            if !col.is_null(i) {
                let s = datafusion::arrow::util::display::array_value_to_string(col, i)
                    .map_err(|e| FederationError::Query(e.to_string()))?;
                tables.push(s);
            }
        }
    }
    Ok(tables)
}

// ── CSV（datafusion 内置 register_csv） ──────────────────────────

async fn register_csv(
    ctx: &SessionContext,
    catalog: &str,
    file: &crate::source::FileConnection,
) -> FederationResult<SchemaSnapshot> {
    let path = std::path::Path::new(&file.path);
    if !path.exists() {
        return Err(FederationError::File(format!("文件不存在: {}", file.path)));
    }

    // CSV delimiter：支持 "\t" 转实际制表符
    let delim_byte = match file.delimiter.as_str() {
        "\\t" | "\t" => b'\t',
        s if s.len() == 1 => s.as_bytes()[0],
        _ => b',',
    };

    // 解析出表名（文件名去扩展名）
    let table_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| FederationError::File(format!("无效文件名: {}", file.path)))?
        .to_string();

    // 用 register_csv 注册到临时表名，再取出 TableProvider 注册到独立 catalog。
    // datafusion 54 的 CsvReadOptions（旧版，方法无 with_ 前缀）。
    use datafusion::datasource::file_format::options::CsvReadOptions;
    let csv_opts = CsvReadOptions::new()
        .has_header(file.has_header)
        .delimiter(delim_byte);
    let temp_name = format!("__onto_temp_{}", uuid::Uuid::new_v4().simple());
    ctx.register_csv(&temp_name, &file.path, csv_opts)
        .await
        .map_err(|e| FederationError::Query(format!("register_csv: {e}")))?;

    // 取出 TableProvider
    let table_ref = datafusion::common::TableReference::bare(temp_name.clone());
    let provider = ctx.table_provider(table_ref.clone()).await.map_err(|e| {
        FederationError::Query(format!("table_provider: {e}"))
    })?;
    // 从默认 schema 注销临时表
    let _ = ctx.deregister_table(table_ref);

    register_catalog_with_table(ctx, catalog, &table_name, provider.clone()).await?;
    let columns = csv_table_metas(provider.schema());
    Ok(SchemaSnapshot { tables: vec![crate::source::TableMeta {
        name: table_name, columns,
        row_count_estimate: None, sample_rows: Vec::new(),
    }] })
}

// ── Excel（calamine → MemTable） ─────────────────────────────────

async fn register_excel(
    ctx: &SessionContext,
    catalog: &str,
    file: &crate::source::FileConnection,
) -> FederationResult<SchemaSnapshot> {
    use datafusion::datasource::MemTable;

    let path = std::path::Path::new(&file.path);
    if !path.exists() {
        return Err(FederationError::File(format!("文件不存在: {}", file.path)));
    }

    // 复用 ingest 的 calamine 读取逻辑（sheet → rows → MemTable）
    let sheets = ingest_excel_sheets(path)?;
    if sheets.is_empty() {
        return Err(FederationError::File("Excel 无有效 sheet".into()));
    }

    let mut tables = Vec::new();
    for (sheet_name, (schema, batch)) in sheets {
        let table = MemTable::try_new(schema.clone(), vec![vec![batch]])
            .map_err(|e| FederationError::Query(format!("MemTable: {e}")))?;
        let tp: Arc<dyn datafusion::catalog::TableProvider> = Arc::new(table);
        register_catalog_with_table(ctx, catalog, &sheet_name, tp.clone()).await?;
        let columns = csv_table_metas(schema.clone());
        tables.push(crate::source::TableMeta {
            name: sheet_name, columns,
            row_count_estimate: None, sample_rows: Vec::new(),
        });
    }
    Ok(SchemaSnapshot { tables })
}

/// 用 calamine 读 Excel，每个 sheet 转 (Schema, RecordBatch)。
/// 复用 ingest::XlsxCalamineParser 的 cell 解析逻辑。
fn ingest_excel_sheets(
    path: &std::path::Path,
) -> FederationResult<Vec<(String, (datafusion::arrow::datatypes::SchemaRef, datafusion::arrow::record_batch::RecordBatch))>> {
    use calamine::{open_workbook, Reader, Xlsx};
    use datafusion::arrow::array::*;
    use datafusion::arrow::datatypes::{DataType, Field, Schema};
    use datafusion::arrow::record_batch::RecordBatch;

    let mut workbook: Xlsx<_> = open_workbook(path)
        .map_err(|e| FederationError::File(format!("打开 Excel 失败: {e:?}")))?;
    let sheets = workbook.worksheets();
    let mut out = Vec::new();

    for (name, range) in sheets {
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|r| r.iter().map(cell_to_string).collect())
            .collect();
        if rows.is_empty() {
            continue;
        }
        let header = &rows[0];
        let n_cols = header.len();
        // 全部按 Utf8 处理（Excel 单元格类型混合，统一字符串最稳）
        let fields: Vec<_> = header
            .iter()
            .map(|h| Field::new(h, DataType::Utf8, true))
            .collect();
        let schema = Arc::new(Schema::new(fields));

        let mut col_builders: Vec<StringBuilder> = (0..n_cols).map(|_| StringBuilder::new()).collect();
        for row in &rows[1..] {
            for (i, cell) in row.iter().enumerate().take(n_cols) {
                col_builders[i].append_value(cell);
            }
            // 不足列补 null
            for i in row.len().min(n_cols)..n_cols {
                col_builders[i].append_null();
            }
        }
        let arrays: Vec<ArrayRef> = col_builders.into_iter().map(|mut b| Arc::new(b.finish()) as ArrayRef).collect();
        let batch = RecordBatch::try_new(schema.clone(), arrays)
            .map_err(|e| FederationError::Arrow(format!("build batch: {e}")))?;
        out.push((name, (schema, batch)));
    }
    Ok(out)
}

fn cell_to_string(d: &calamine::Data) -> String {
    match d {
        calamine::Data::Empty => String::new(),
        calamine::Data::Int(i) => i.to_string(),
        calamine::Data::Float(f) => {
            if f.fract() == 0.0 && f.is_finite() {
                format!("{}", *f as i64)
            } else {
                format!("{f}")
            }
        }
        calamine::Data::String(s) => s.clone(),
        calamine::Data::Bool(b) => b.to_string(),
        calamine::Data::DateTime(dt) => format!("{dt}"),
        calamine::Data::DateTimeIso(s) | calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("#ERR:{e:?}"),
    }
}

/// SchemaRef → ColumnMeta（CSV/Excel 用，不走联邦 information_schema）。
fn csv_table_metas(schema: datafusion::arrow::datatypes::SchemaRef) -> Vec<crate::source::ColumnMeta> {
    schema.fields().iter().map(|f| crate::source::ColumnMeta {
        name: f.name().clone(),
        data_type: crate::schema::arrow_type_name(f.data_type()),
        nullable: f.is_nullable(),
    }).collect()
}

/// 创建独立 catalog（含 default schema），注册一张表。
/// 这样 agent 可用 `catalog.table_name` 或 `catalog.default.table_name` 寻址。
async fn register_catalog_with_table(
    ctx: &SessionContext,
    catalog: &str,
    table_name: &str,
    table: Arc<dyn datafusion::catalog::TableProvider>,
) -> FederationResult<()> {
    use datafusion::catalog::{MemoryCatalogProvider, MemorySchemaProvider};

    // 若 catalog 已存在，复用；否则新建
    if ctx.catalog(catalog).is_none() {
        let cat = Arc::new(MemoryCatalogProvider::new());
        ctx.register_catalog(catalog, cat);
    }
    let cat = ctx.catalog(catalog).ok_or_else(|| {
        FederationError::Other(format!("注册 catalog 失败: {catalog}"))
    })?;

    // 用 "public" schema（统一约定，避免 default 命名歧义）
    let schema_name = "public";
    if cat.schema(schema_name).is_none() {
        let schema = Arc::new(MemorySchemaProvider::new());
        cat.register_schema(schema_name, schema)
            .map_err(|e| FederationError::Query(format!("register schema: {e}")))?;
    }
    let schema = cat.schema(schema_name).ok_or_else(|| {
        FederationError::Other(format!("注册 schema 失败: {catalog}.{schema_name}"))
    })?;
    schema
        .register_table(table_name.to_string(), table)
        .map_err(|e| FederationError::Query(format!("register table: {e}")))?;
    Ok(())
}

/// 把 TableReference 转字符串（调试用）。
#[allow(dead_code)]
fn table_ref_str(r: &datafusion::common::TableReference) -> String {
    r.to_string()
}
