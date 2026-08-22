//! onto-studio Tauri 壳：IPC 薄层 + 平台能力（见 ARCHITECTURE.md §六）。
//!
//! 业务逻辑禁止放进这里（AGENTS.md 工程结构硬约束），仅做 #[tauri::command] 薄封装。
//! 一期 MVP 落地内容见 §九。

use commands::provider::restore_provider;
use specta_typescript::Typescript;
use tauri::{Emitter, Manager};
use tauri_specta::collect_commands;
use tracing::info;

pub mod commands;
mod pdfium;
mod skill;
mod state;

use state::AppState;

/// 生成的 TS 绑定路径（src/lib/ipc/bindings.ts，对齐 §12.2）
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/ipc/bindings.ts");

/// 生成 TS 绑定（供 example / CI 调用，不启动 Tauri）。
///
/// `collect_commands!` 宏依赖同 crate 内 `#[specta::specta]` 生成的辅助宏，
/// 必须在本 crate 内调用，故不能放到独立 example。
pub fn gen_bindings(path: &str) -> std::result::Result<(), String> {
    #[allow(unused_imports)]
    use commands::{
        cancel_ingest, create_conversation, delete_conversation, delete_message,
        delete_message_and_after,
        generate_conversation_title, get_mcp_servers, get_provider,
        ingest_files, list_all_documents, list_conversations, list_messages, list_mcp_tools, list_mounted_documents,
        mount_document, read_document, rename_conversation, send_message, set_conversation_pinned, set_mcp_servers,
        set_message_status, set_provider, unmount_document, delete_document, cancel_stream,
        register_data_source, test_data_source, deregister_data_source, list_data_sources,
        get_data_source, browse_federation_schema, describe_federation_table,
        execute_federation_query, explain_federation_query,
        // 文件夹操作 + 会话激活集（CONVERSATION-SCOPE.md）
        create_folder, list_folders, list_documents_by_folder, move_document, rename_folder, delete_folder,
        get_active_scope, set_active_folders, set_active_sources, set_active_ontologies,
        // Skill 系统（决策 20）
        list_skills, import_skill_from_dir, import_skill_from_zip, uninstall_skill,
        set_skill_conversation_enabled, set_skill_globally_disabled,
        // 本体建模（三期：ontology-store IPC）
        list_ontologies, export_ontology, preview_ontology_import, import_ontology,
        delete_ontology,
        list_ontology_changelog,
        list_ontology_datasets, list_ontology_data_sources,
        get_ontology_charter, set_ontology_charter,
    };
    let builder = tauri_specta::Builder::<tauri::Wry>::new().commands(collect_commands![
        create_conversation,
        list_conversations,
        rename_conversation,
        generate_conversation_title,
        set_conversation_pinned,
        delete_conversation,
        list_messages,
        delete_message,
        set_message_status,
        send_message,
        cancel_stream,
        set_provider,
        get_provider,
        set_mcp_servers,
        get_mcp_servers,
        list_mcp_tools,
        ingest_files,
        cancel_ingest,
        mount_document,
        unmount_document,
        list_mounted_documents,
        list_all_documents,
        delete_document,
        read_document,
        register_data_source,
        test_data_source,
        deregister_data_source,
        list_data_sources,
        get_data_source,
        browse_federation_schema,
        describe_federation_table,
        execute_federation_query,
        explain_federation_query,
        // 文件夹操作 + 会话激活集（CONVERSATION-SCOPE.md）
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
        // Skill 系统（决策 20）
        list_skills,
        import_skill_from_dir,
        import_skill_from_zip,
        uninstall_skill,
        set_skill_conversation_enabled,
        set_skill_globally_disabled,
        delete_message_and_after,
        // 本体建模（三期：ontology-store IPC）
        list_ontologies,
        export_ontology,
        preview_ontology_import,
        import_ontology,
        delete_ontology,
        list_ontology_changelog,
        list_ontology_datasets,
        list_ontology_data_sources,
        get_ontology_charter,
        set_ontology_charter,
    ]);
    // 时间戳等 i64 字段已由 memory::Timestamp 包装（specta 导出为 number），
    // 故用默认 Typescript 配置即可。
    builder
        .export(specta_typescript::Typescript::default(), path)
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
// 命令注册（与 gen_bindings 保持一致）
use commands::{
cancel_ingest, create_conversation, delete_conversation, delete_message,
delete_message_and_after,
generate_conversation_title, get_provider, get_mcp_servers,
ingest_files, list_all_documents, list_conversations, list_messages, list_mcp_tools, list_mounted_documents,
mount_document, read_document, rename_conversation, send_message, set_conversation_pinned, set_mcp_servers,
set_message_status, set_provider, unmount_document, delete_document, cancel_stream,
register_data_source, test_data_source, deregister_data_source, list_data_sources,
get_data_source, browse_federation_schema, describe_federation_table,
execute_federation_query, explain_federation_query,
// 文件夹操作 + 会话激活集（CONVERSATION-SCOPE.md）
create_folder, list_folders, list_documents_by_folder, move_document, rename_folder, delete_folder,
get_active_scope, set_active_folders, set_active_sources, set_active_ontologies,
// Skill 系统（决策 20）
list_skills, import_skill_from_dir, import_skill_from_zip, uninstall_skill,
set_skill_conversation_enabled, set_skill_globally_disabled,
// 本体建模（三期：ontology-store IPC）
list_ontologies, export_ontology, preview_ontology_import, import_ontology,
delete_ontology,
list_ontology_changelog,
list_ontology_datasets, list_ontology_data_sources,
get_ontology_charter, set_ontology_charter,
};
    let builder = tauri_specta::Builder::<tauri::Wry>::new().commands(collect_commands![
        create_conversation,
        list_conversations,
        rename_conversation,
        generate_conversation_title,
        set_conversation_pinned,
        delete_conversation,
        list_messages,
        delete_message,
        set_message_status,
        send_message,
        cancel_stream,
        set_provider,
        get_provider,
        set_mcp_servers,
        get_mcp_servers,
        list_mcp_tools,
        ingest_files,
        cancel_ingest,
        mount_document,
        unmount_document,
        list_mounted_documents,
        list_all_documents,
        delete_document,
        read_document,
        register_data_source,
        test_data_source,
        deregister_data_source,
        list_data_sources,
        get_data_source,
        browse_federation_schema,
        describe_federation_table,
        execute_federation_query,
        explain_federation_query,
        // 文件夹操作 + 会话激活集（CONVERSATION-SCOPE.md）
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
        // Skill 系统（决策 20）
        list_skills,
        import_skill_from_dir,
        import_skill_from_zip,
        uninstall_skill,
        set_skill_conversation_enabled,
        set_skill_globally_disabled,
        delete_message_and_after,
        // 本体建模（三期：ontology-store IPC）
        list_ontologies,
        export_ontology,
        preview_ontology_import,
        import_ontology,
        delete_ontology,
        list_ontology_changelog,
        list_ontology_datasets,
        list_ontology_data_sources,
        get_ontology_charter,
        set_ontology_charter,
    ]);

    // debug 模式下 dev 运行也刷新 bindings（CI/首次用 examples/gen_bindings）
    #[cfg(debug_assertions)]
    if let Err(e) = builder.export(Typescript::default(), BINDINGS_PATH) {
        eprintln!("[onto-studio] warn: export bindings on dev run failed: {e}");
    }

    // 仅生成 bindings 然后退出（CI/脚本用）：ONTO_GEN_BINDINGS=1 cargo run
    if std::env::var_os("ONTO_GEN_BINDINGS").is_some() {
        eprintln!("[onto-studio] ONTO_GEN_BINDINGS set, exporting bindings to {BINDINGS_PATH}");
        if let Err(e) = builder.export(Typescript::default(), BINDINGS_PATH) {
            eprintln!("[onto-studio] ERROR: export bindings failed: {e}");
            return;
        }
        eprintln!("[onto-studio] bindings exported, exiting");
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            // 初始化日志
            let _ = tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "onto_studio=info,agent_core=info,memory=info,federation=info,tauri=warn".into()),
                )
                .try_init();

            // 数据库路径：app data dir / onto-studio.db
            let app_data = app.path().app_data_dir().expect("no app data dir");
            let db_path = app_data.join("onto-studio.db");
            info!(db_path = %db_path.display(), "opening database");
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // 先 open memory（Arc 包装），供 AppState 与 SkillManager 共用同一连接。
            let memory = std::sync::Arc::new(memory::Memory::open(&db_path)?);

            // open ontology store（三期：本体建模作为 agent 工具）。独立 DB 文件
            // （ontology.db），与 onto-studio.db 职责正交——本体定义 vs 会话/文档。
            let ontology_db_path = app_data.join("ontology.db");
            let ontology_store = std::sync::Arc::new(ontology_store::OntologyStore::open(&ontology_db_path)?);

            // Skill 系统初始化（决策 20）：解析各 skill 目录路径，构造 SkillManager。
            // builtin_dir 缺失时用空目录（manager 降级为空列表，不阻断启动）。
            let builtin_skills_dir = skill::builtin_dir(app.handle())
                .unwrap_or_else(|| std::path::PathBuf::from("/nonexistent/builtin"));
            let user_skills_dir = skill::user_dir();
            let external_skill_dirs = skill::external_dirs();
            if let Err(e) = std::fs::create_dir_all(&user_skills_dir) {
                tracing::warn!(error = %e, dir = %user_skills_dir.display(), "create user skills dir failed");
            }
            let skill_manager = std::sync::Arc::new(agent_core::SkillManager::new(
                std::sync::Arc::clone(&memory),
                builtin_skills_dir,
                user_skills_dir,
                external_skill_dirs,
            ));

            let state = AppState::new_with_memory(memory, ontology_store, skill_manager)?;

            // 本体落库变更通知（前端 query 失效用）：
            // 会话内 agent 工具（import_ontology）不经过 IPC 命令层，
            // 因此在 store 层注入回调，统一从装配点发 event。
            {
                let handle = app.handle().clone();
                state
                    .ontology_store
                    .set_on_change(Box::new(move || {
                        if let Err(e) = handle.emit("ontology-changed", ()) {
                            tracing::warn!(error = %e, "emit ontology-changed failed");
                        }
                    }));
            };
            // 从 store 恢复 provider 配置
            if let Err(e) = restore_provider(app.handle(), &state) {
                tracing::warn!(error = %e, "restore provider failed");
            }

            app.manage(state);
            app.manage(commands::ingest::CancelRegistry::default());

            // 初始化 PDFium（决策 5）：定位打包的动态库并加载。失败不阻断启动。
            let _ = pdfium::init(app.handle());

            // manage 后恢复 MCP server 连接（需访问 managed AppState 的 tool_handle/mcp）
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::mcp::restore_mcp_servers(app_handle.clone()).await;
            });

            // 异步初始化联邦查询服务（恢复已注册数据源，§2.5）
            let app_handle2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = app_handle2.try_state::<AppState>() {
                    if let Err(e) = state.init_federation().await {
                        tracing::warn!(error = %e, "init federation failed");
                    }
                }
            });

            // debug 构建自动开 devtools，方便排查白屏等前端错误
            #[cfg(debug_assertions)]
            {
                use tauri::Manager;
                if let Some(win) = app.get_webview_window("main") {
                    win.open_devtools();
                }
            }

            // 挂载事件（一期无自定义 event，但调用以保证 builder 状态正确）
            builder.mount_events(app);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// State::new 返回 AppResult，但 setup 闭包需返回 Result<_, Box<dyn std::error::Error>>。
// AppError 实现 std::error::Error（thiserror），Box<dyn Error> 的 From blanket impl 已存在，
// 无需手动 impl。setup 内 `AppState::new(db_path)?` 可直接通过。

