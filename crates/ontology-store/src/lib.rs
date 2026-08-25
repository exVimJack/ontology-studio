//! ontology-store: 本体定义存储 + 导入导出（对齐 Gaia `ontology_service.py`）
//!
//! 架构：rusqlite(bundled) 复刻 Gaia 的本体定义层表族 + export/preview/import
//! 三函数。agent 通过这三个函数完成冷启动建模和可持续增量更新：
//!   - 冷启动：preview(payload) → import(payload)
//!   - 增量：export(ontology) → 改 payload → preview → import(带 overwrite 列表)
//!
//! 设计原则：Rust 侧校验是真相源（naming.rs + data_type.rs），DB 约束做兜底。

pub mod data_type;
pub mod error;
pub mod naming;
pub mod payload;
pub mod schema;
pub mod store;
pub mod ttl;

pub use error::{StoreError, StoreResult};
pub use payload::*;
pub use store::OntologyStore;
pub use ttl::{TtlChangelog, TtlCharter, TtlImportResult, TtlOntologySummary, TtlStore, TtlValidation};
