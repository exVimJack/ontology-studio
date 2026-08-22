//! CSV 端到端测试（零外部依赖，验证全链路，见 §7.2 验证标准）。
//!
//! 注册 CSV → list_data_sources → describe_table → execute_sql 返回行集。
//! 测试样本在 tests/data/ 下。

use std::sync::Arc;

use federation::{ConnectionConfig, DataSourceConfig, DataSourceKind, FileConnection};
use memory::{Memory, Timestamp};
use uuid::Uuid;
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn setup() -> federation::FederationService {
    let mem = Arc::new(Memory::open_in_memory().unwrap());
    federation::FederationService::new_for_test(mem).await.unwrap()
}

fn csv_config(name: &str, path: &str) -> DataSourceConfig {
    DataSourceConfig {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        connection: ConnectionConfig::Csv(FileConnection {
            path: path.to_string(),
            has_header: true,
            delimiter: ",".to_string(),
        }),
        color: None,
        created_at: Timestamp(now_ms()),
    }
}

#[tokio::test]
async fn csv_register_and_query() {
    let svc = setup().await;
    let csv_path = env!("CARGO_MANIFEST_DIR").to_string() + "/tests/data/users.csv";

    // 1. 注册
    let summary = svc.register(csv_config("testdb", &csv_path)).await.unwrap();
    assert!(summary.connected, "应连接成功: {:?}", summary.last_error);
    assert_eq!(summary.table_count, Some(1));

    // 2. list_data_sources
    let sources = svc.list_sources().await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].name, "testdb");
    assert!(sources[0].connected);

    // 3. browse_schema
    let snapshot = federation::schema::browse_schema(svc.ctx(), "testdb").await.unwrap();
    assert_eq!(snapshot.tables.len(), 1);
    let table = &snapshot.tables[0];
    assert_eq!(table.name, "users");
    assert!(table.columns.len() >= 3, "users.csv 至少 3 列");

    // 4. describe_table（含前 5 行样本）
    let desc = federation::schema::describe_table(svc.ctx(), "testdb", "users")
        .await
        .unwrap();
    assert_eq!(desc.name, "users");
    assert!(!desc.sample_rows.is_empty(), "应有样本行");

    // 5. execute_sql（只读护栏 + 查询）
    let result = federation::query::execute_query(
        svc.ctx(),
        "SELECT * FROM testdb.public.users",
        None,
    )
    .await
    .unwrap();
    assert!(result.row_count > 0, "应返回行");
    assert!(!result.columns.is_empty());
    assert_eq!(result.sources_touched, vec!["testdb".to_string()]);
}

#[tokio::test]
async fn readonly_guard_blocks_writes() {
    let svc = setup().await;
    let csv_path = env!("CARGO_MANIFEST_DIR").to_string() + "/tests/data/users.csv";
    svc.register(csv_config("db2", &csv_path)).await.unwrap();

    // INSERT 被拦截
    let err = federation::query::execute_query(
        svc.ctx(),
        "INSERT INTO db2.public.users VALUES (1, 'a', 20)",
        None,
    )
    .await;
    assert!(err.is_err());

    // DROP 被拦截
    let err = federation::query::execute_query(svc.ctx(), "DROP TABLE db2.public.users", None).await;
    assert!(err.is_err());
}

#[tokio::test]
async fn row_limit_auto_applied() {
    let svc = setup().await;
    let csv_path = env!("CARGO_MANIFEST_DIR").to_string() + "/tests/data/users.csv";
    svc.register(csv_config("db3", &csv_path)).await.unwrap();

    // 不含 LIMIT 的查询自动追加
    let result = federation::query::execute_query(
        svc.ctx(),
        "SELECT * FROM db3.public.users",
        Some(2), // 显式 limit=2
    )
    .await
    .unwrap();
    assert!(result.row_count <= 2, "行数应 <= 2，实际 {}", result.row_count);
}

#[tokio::test]
async fn deregister_removes_source() {
    let svc = setup().await;
    let csv_path = env!("CARGO_MANIFEST_DIR").to_string() + "/tests/data/users.csv";
    let summary = svc.register(csv_config("tempdb", &csv_path)).await.unwrap();
    svc.deregister(&summary.id).await.unwrap();

    let sources = svc.list_sources().await.unwrap();
    assert_eq!(sources.len(), 0, "注销后应无源");
}

#[tokio::test]
async fn persistence_across_instances() {
    // 注册后重建服务，源应从 SQLite 恢复（restore_sources）
    let mem = Arc::new(Memory::open_in_memory().unwrap());
    let svc = federation::FederationService::new_for_test(mem.clone()).await.unwrap();
    let csv_path = env!("CARGO_MANIFEST_DIR").to_string() + "/tests/data/users.csv";
    svc.register(csv_config("persistdb", &csv_path)).await.unwrap();

    // 重新构造（同 memory），restore_sources 恢复
    let svc2 = federation::FederationService::new(mem).await.unwrap();
    let sources = svc2.list_sources().await.unwrap();
    assert_eq!(sources.len(), 1, "重建后应恢复 1 个源");
    assert_eq!(sources[0].name, "persistdb");
}
