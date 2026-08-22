//! executor: SQL 执行器（federation SQLExecutor 实现，见 §2.4 方案 D）。
//!
//! 每个远程 SQL 源实现 `SQLExecutor` trait 的 6 个必需方法：
//!   - name / compute_context / dialect
//!   - execute（下推方言 SQL → sqlx 执行 → SendableRecordBatchStream）
//!   - table_names / get_table_schema
//!
//! 方言 SQL 由 federation 的 unparser 自动生成（datafusion 54 默认启用 unparser），
//! 不手写方言翻译。
//!
//! CSV/Excel 不走 SQLExecutor（catalog.rs 用 datafusion 内置 register_csv / MemTable）。

pub mod mysql;
pub mod postgres;
