//! Tauri 命令层（见 ARCHITECTURE.md §六 / §13 IPC 契约）。
//!
//! 仅做 `#[tauri::command]` 薄封装，业务逻辑全在 crates/（AGENTS.md 工程结构硬约束）。
//! 类型经 tauri-specta 生成 TS 绑定（决策 F5），禁止前端手写类型。

pub mod chat;
pub mod conversation;
pub mod error;
pub mod federation;
pub mod ingest;
pub mod mcp;
pub mod ontology;
pub mod provider;
pub mod skill;

// 命令函数 + DTO 类型统一 re-export，供 lib.rs 的 collect_commands! 与上层引用
pub use chat::{cancel_stream, send_message, ChatStreamChunk, SendMessageInput};
pub use conversation::{
    create_conversation, delete_conversation, delete_message, delete_message_and_after,
    generate_conversation_title, list_conversations, list_messages, rename_conversation,
    set_conversation_pinned, set_message_status, CreateConversationInput,
    DeleteMessageAndAfterInput, DeleteMessageInput, SetMessageStatusInput, SetPinnedInput,
};
pub use error::AppError;
pub use federation::{
    browse_federation_schema, deregister_data_source, describe_federation_table,
    execute_federation_query, explain_federation_query, get_data_source,
    list_data_sources, register_data_source, test_data_source,
};
pub use ingest::{
    cancel_ingest, ingest_files, mount_document, unmount_document, list_mounted_documents,
    list_all_documents, delete_document, read_document, CancelRegistry, IngestProgress,
    IngestResultItem, IngestStage, MountedDocDto, DocumentSummaryDto, DocumentContentDto,
    // 文件夹操作 + 会话激活集（CONVERSATION-SCOPE.md）
    create_folder, list_folders, list_documents_by_folder, move_document, rename_folder, delete_folder,
    get_active_scope, set_active_folders, set_active_sources, set_active_ontologies, ActiveScopeDto,
};
pub use mcp::{get_mcp_servers, list_mcp_tools, set_mcp_servers, McpServerStatus, McpToolDef};
pub use ontology::{
    list_ontologies, export_ontology, preview_ontology_import, import_ontology,
    delete_ontology,
    list_ontology_changelog,
    list_ontology_datasets, list_ontology_data_sources,
    get_ontology_charter, set_ontology_charter,
    list_ontology_ttl, export_ontology_ttl, validate_ontology_ttl, import_ontology_ttl,
    delete_ontology_ttl, query_ontology_sparql,
    get_ontology_ttl_charter, set_ontology_ttl_charter,
    list_ontology_ttl_changelog, commit_ontology_ttl_change,
};
pub use provider::{get_provider, set_provider, restore_provider, SetProviderInput};
pub use skill::{
    list_skills, import_skill_from_dir, import_skill_from_zip, uninstall_skill,
    set_skill_conversation_enabled, set_skill_globally_disabled, SkillDto,
};
