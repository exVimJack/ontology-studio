//! onto-studio Tauri 壳：IPC 薄层 + 平台能力（见 ARCHITECTURE.md §六）。
//!
//! 业务逻辑禁止放进这里（AGENTS.md 工程结构硬约束），仅做 #[tauri::command] 薄封装。
//! 一期 MVP 落地内容见 §九。

use commands::provider::restore_provider;
use specta_typescript::Typescript;
use tauri::{Emitter, Manager};
use tauri_specta::collect_commands;
use tracing::{info, warn};

pub mod commands;
mod pdfium;
mod skill;
mod state;

use state::AppState;

/// 生成的 TS 绑定路径（src/lib/ipc/bindings.ts，对齐 §12.2）
const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/ipc/bindings.ts");

// ──────────────────────────────────────────────────────────────
// 日志初始化 + 耗时测量辅助（保留现场用）
// ──────────────────────────────────────────────────────────────

/// 默认日志过滤级别。prod 用此默认；dev/CI 可用 RUST_LOG 覆盖。
const DEFAULT_LOG_FILTER: &str =
    "onto_studio=info,onto_studio_lib=info,agent_core=info,memory=info,federation=info,ingest=info,ontology_store=info,tauri=warn";

/// 初始化日志：prod 写文件（app_data_dir/logs/app.log），dev 额外输出 stderr。
///
/// 文件按天滚动（tracing-appender），保留 7 天。prod 双击启动看不到 stderr，
/// 文件日志是排查 bug 的唯一现场来源。dev 下也写文件，便于复现问题。
///
/// 必须在 setup hook 最开头调用（先于所有业务初始化），否则早期日志丢失。
fn init_logging(app: &tauri::AppHandle) {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));

    // app_data_dir/logs/。失败则退回 stderr-only（不阻断启动）。
    let log_dir = app.path().app_data_dir().ok().map(|d| d.join("logs"));
    let (file_layer, _guard) = match &log_dir {
        Some(dir) => {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("[onto-studio] warn: create log dir failed: {e}");
            }
            let file_appender = tracing_appender::rolling::daily(dir, "app.log");
            // NonBlocking 会 spawn 写线程，guard 必须保活（返回 guard 防止 drop）
            let (nb, guard) = tracing_appender::non_blocking(file_appender);
            (Some(fmt::layer().with_writer(nb).with_ansi(false)), Some(guard))
        }
        None => (None, None),
    };

    // stderr：dev 必出（方便调试），prod 也出（命令行启动时可见）
    let stderr_layer = Some(fmt::layer().with_writer(std::io::stderr));

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(file_layer);

    if let Err(e) = registry.try_init() {
        // 已有 subscriber 初始化过（如测试）。退回 stderr-only，不 panic。
        eprintln!("[onto-studio] warn: tracing init failed: {e}, fallback to stderr-only");
        let _ = fmt().with_env_filter(DEFAULT_LOG_FILTER).try_init();
    }

    // guard 泄漏：进程生命周期内不 drop（否则日志线程退出，丢失尾部日志）
    if let Some(g) = _guard {
        std::mem::forget(g);
    }

    info!(
        log_dir = ?log_dir.as_ref().map(|d| d.display().to_string()),
        build_profile = if cfg!(debug_assertions) { "debug" } else { "release" },
        version = env!("CARGO_PKG_VERSION"),
        "tracing initialized (file + stderr)"
    );
}

/// 测量同步操作耗时并打 info 日志，返回操作结果。
///
/// 用于 setup hook 中每个关键步骤（DB open、skill init、pdfium load 等），
/// 启动卡住时能从日志直接看出哪一步耗时久。
#[track_caller]
fn measure<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = std::time::Instant::now();
    let result = f();
    info!(step = label, elapsed_ms = start.elapsed().as_millis() as u64, "setup step done");
    result
}

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
        // 本体建模 W3C（Turtle，对齐 skill ontology-modeling-w3c）
        list_ontology_ttl, export_ontology_ttl, validate_ontology_ttl, import_ontology_ttl,
        delete_ontology_ttl, query_ontology_sparql,
        get_ontology_ttl_charter, set_ontology_ttl_charter,
        list_ontology_ttl_changelog, commit_ontology_ttl_change,
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
        // 本体建模 W3C（Turtle）
        list_ontology_ttl,
        export_ontology_ttl,
        validate_ontology_ttl,
        import_ontology_ttl,
        delete_ontology_ttl,
        query_ontology_sparql,
        get_ontology_ttl_charter,
        set_ontology_ttl_charter,
        list_ontology_ttl_changelog,
        commit_ontology_ttl_change,
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
// 本体建模 W3C（Turtle，对齐 skill ontology-modeling-w3c）
list_ontology_ttl, export_ontology_ttl, validate_ontology_ttl, import_ontology_ttl,
delete_ontology_ttl, query_ontology_sparql,
get_ontology_ttl_charter, set_ontology_ttl_charter,
list_ontology_ttl_changelog, commit_ontology_ttl_change,
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
        // 本体建模 W3C（Turtle）
        list_ontology_ttl,
        export_ontology_ttl,
        validate_ontology_ttl,
        import_ontology_ttl,
        delete_ontology_ttl,
        query_ontology_sparql,
        get_ontology_ttl_charter,
        set_ontology_ttl_charter,
        list_ontology_ttl_changelog,
        commit_ontology_ttl_change,
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
            // ── 初始化日志（核心：prod 也写文件，保留现场） ──
            // 日志写到 app_data_dir/logs/app.log，按天滚动，保留 7 天。
            // dev 额外输出 stderr，prod 仅文件（双击启动看不到 stderr）。
            init_logging(&app.handle().clone());
            let app_data = app.path().app_data_dir().expect("no app data dir");

            // 数据库路径：app data dir / onto-studio.db
            let db_path = app_data.join("onto-studio.db");
            info!(db_path = %db_path.display(), "opening database");
            if let Some(parent) = db_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            // 先 open memory（Arc 包装），供 AppState 与 SkillManager 共用同一连接。
            let memory = std::sync::Arc::new(measure("open memory db", || memory::Memory::open(&db_path))?);

            // open ontology store（三期：本体建模作为 agent 工具）。独立 DB 文件
            // （ontology.db），与 onto-studio.db 职责正交——本体定义 vs 会话/文档。
            let ontology_db_path = app_data.join("ontology.db");
            let ontology_store = std::sync::Arc::new(measure("open ontology db", || {
                ontology_store::OntologyStore::open(&ontology_db_path)
            })?);

            // open W3C Turtle store（ontology-modeling-w3c skill 闭环）。复用 ontology.db
            // 文件——表族不冲突（OntologyStore 用 Gaia 表族，TtlStore 用 ontology_ttl 表），
            // SQLite 多连接同文件在 WAL 模式下安全（两个 connection 独立 Mutex 串行化）。
            let ttl_store = std::sync::Arc::new(ontology_store::TtlStore::open(&ontology_db_path)?);

            // Skill 系统初始化（决策 20）：解析各 skill 目录路径，构造 SkillManager。
            // builtin_dir 缺失时用空目录（manager 降级为空列表，不阻断启动）。
            let builtin_skills_dir = skill::builtin_dir(app.handle())
                .unwrap_or_else(|| std::path::PathBuf::from("/nonexistent/builtin"));
            let user_skills_dir = skill::user_dir();
            let external_skill_dirs = skill::external_dirs();
            info!(
                builtin = %builtin_skills_dir.display(),
                user = %user_skills_dir.display(),
                external = ?external_skill_dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
                "skill dirs resolved"
            );
            if let Err(e) = std::fs::create_dir_all(&user_skills_dir) {
                warn!(error = %e, dir = %user_skills_dir.display(), "create user skills dir failed");
            }
            let skill_manager = std::sync::Arc::new(measure("create SkillManager", || {
                agent_core::SkillManager::new(
                    std::sync::Arc::clone(&memory),
                    builtin_skills_dir,
                    user_skills_dir,
                    external_skill_dirs,
                )
            }));

            let state = AppState::new_with_memory(memory, ontology_store, ttl_store, skill_manager)?;

            // 本体落库变更通知（前端 query 失效用）：
            // 会话内 agent 工具（import_ontology）不经过 IPC 命令层，
            // 因此在 store 层注入回调，统一从装配点发 event。
            {
                let handle = app.handle().clone();
                state
                    .ontology_store
                    .set_on_change(Box::new(move || {
                        if let Err(e) = handle.emit("ontology-changed", ()) {
                            warn!(error = %e, "emit ontology-changed failed");
                        }
                    }));
            }
            // TtlStore 落库变更同样发 event（前端 query 失效，与 Palantir store 统一路径）。
            {
                let handle = app.handle().clone();
                state
                    .ttl_store
                    .set_on_change(Box::new(move || {
                        if let Err(e) = handle.emit("ontology-changed", ()) {
                            tracing::warn!(error = %e, "emit ontology-changed (ttl) failed");
                        }
                    }));
            }
            // 从 store 恢复 provider 配置
            if let Err(e) = restore_provider(app.handle(), &state) {
                warn!(error = %e, "restore provider failed");
            }

            app.manage(state);
            app.manage(commands::ingest::CancelRegistry::default());
            info!("app state managed, commands ready");

            // 初始化 PDFium（决策 5）：定位打包的动态库并加载。失败不阻断启动。
            let _ = measure("pdfium init", || pdfium::init(app.handle()));

            // manage 后恢复 MCP server 连接（需访问 managed AppState 的 tool_handle/mcp）
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let t = std::time::Instant::now();
                commands::mcp::restore_mcp_servers(app_handle.clone()).await;
                info!(elapsed_ms = t.elapsed().as_millis() as u64, "restore_mcp_servers done");
            });

            // 异步初始化联邦查询服务（恢复已注册数据源，§2.5）
            let app_handle2 = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let t = std::time::Instant::now();
                if let Some(state) = app_handle2.try_state::<AppState>() {
                    match state.init_federation().await {
                        Ok(()) => info!(elapsed_ms = t.elapsed().as_millis() as u64, "init_federation done"),
                        Err(e) => warn!(error = %e, elapsed_ms = t.elapsed().as_millis() as u64, "init federation failed"),
                    }
                } else {
                    warn!("init_federation: AppState not yet managed");
                }
            });

            info!("setup hook completed");

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

