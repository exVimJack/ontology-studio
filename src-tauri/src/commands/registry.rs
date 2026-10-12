//! 命令注册单点（结构重构 #1）：消除 lib.rs 三份手工同步的命令清单。
//!
//! 之前 `gen_bindings` 的 use 块、`run` 的 use 块、`collect_commands!` 三处
//! 各自维护一份命令列表——加一个命令要改三处（save_b_report 曾踩坑）。
//!
//! 现在唯一真相源是 `command_names!` 宏（消费宏模式）：
//!   - `command_names!(use_commands)`      → 展开为 `use crate::commands::{...}`
//!   - `command_names!(collect_commands)`  → 展开为 `tauri_specta::collect_commands![...]`
//!
//! 新增命令时只需在 `command_names!` 列表加一行，三处自动生效。

/// 命令清单（唯一真相源）。`$mac` 是消费者宏，决定如何展开列表。
#[macro_export]
macro_rules! command_names {
    ($mac:ident) => {
        $mac!(
            // ── 会话 ──
            cancel_ingest,
            create_conversation,
            delete_conversation,
            delete_message,
            delete_message_and_after,
            generate_conversation_title,
            list_conversations,
            list_messages,
            rename_conversation,
            send_message,
            set_conversation_pinned,
            set_message_status,
            cancel_stream,
            // ── 文件库 ──
            ingest_files,
            list_all_documents,
            mount_document,
            unmount_document,
            list_mounted_documents,
            delete_document,
            read_document,
            create_folder,
            list_folders,
            list_documents_by_folder,
            move_document,
            rename_folder,
            delete_folder,
            get_active_scope,
            set_active_folders,
            set_active_sources,
            set_active_ontologies,
            // ── 数据源（federation）──
            register_data_source,
            test_data_source,
            deregister_data_source,
            list_data_sources,
            get_data_source,
            browse_federation_schema,
            describe_federation_table,
            execute_federation_query,
            explain_federation_query,
            // ── HTTP / MCP / provider ──
            http_post_json,
            get_mcp_servers,
            list_mcp_tools,
            set_mcp_servers,
            get_context_window,
            get_provider,
            set_provider,
            get_verifier_provider,
            set_verifier_provider,
            // ── Skill 系统 ──
            list_skills,
            import_skill_from_dir,
            import_skill_from_zip,
            uninstall_skill,
            set_skill_conversation_enabled,
            set_skill_globally_disabled,
            // ── 本体建模（Palantir）──
            list_ontologies,
            export_ontology,
            preview_ontology_import,
            import_ontology,
            delete_ontology,
            list_element_evidence,
            list_ontology_changelog,
            list_ontology_datasets,
            list_ontology_data_sources,
            get_ontology_charter,
            set_ontology_charter,
            create_ontology_with_charter,
            // ── 本体建模 W3C（Turtle）──
            list_ontology_ttl,
            export_ontology_ttl,
            validate_ontology_ttl,
            import_ontology_ttl,
            delete_ontology_ttl,
            query_ontology_sparql,
            get_ontology_ttl_charter,
            set_ontology_ttl_charter,
            create_ontology_ttl_with_charter,
            list_ontology_ttl_changelog,
            commit_ontology_ttl_change,
            // ── W3C Turtle 属性结构化编辑 ──
            list_ontology_ttl_properties,
            list_ontology_ttl_class_iris,
            get_ontology_ttl_default_property_ns,
            edit_ontology_ttl_property,
            add_ontology_ttl_property,
            delete_ontology_ttl_property,
            // ── 校验对抗（VALIDATION-DESIGN.md v0.3.1）──
            dispute_issue,
            resolve_dispute,
            dispute_and_judge,
            resolve_dispute_llm,
            list_validation_issues,
            persist_validation_issues,
            save_b_report,
            review_ontology,
            a_revise,
            b_recheck,
            escalate_review,
            latest_validation_review,
            coverage_matrix,
            independence_tier,
            run_review_loop,
            run_auto_dispute,
            get_auto_dispute_result,
            run_post_arbitration_revise,
            get_post_arbitration_state,
            run_b_review_llm,
            run_a_revise_llm,
            run_b_recheck_llm,
        )
    };
}

/// 消费者宏 ①：展开为 `use crate::commands::{...}`（gen_bindings / run 的导入）。
#[macro_export]
macro_rules! use_commands {
    ($($cmd:ident),* $(,)?) => {
        #[allow(unused_imports)]
        pub(crate) use crate::commands::{ $($cmd),* };
    };
}

/// 消费者宏 ②：展开为 `tauri_specta::collect_commands![...]`（注册 + TS 绑定）。
#[macro_export]
macro_rules! collect_all {
    ($($cmd:ident),* $(,)?) => {
        ::tauri_specta::collect_commands![ $($cmd),* ]
    };
}
