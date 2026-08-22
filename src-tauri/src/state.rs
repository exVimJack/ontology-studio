//! AppState：跨命令共享的业务状态。
//!
//! 架构（见 ARCHITECTURE.md §四 / §六）：
//!   - memory：SQLite 存储（会话/消息），Arc 包装跨线程共享
//!   - chat：当前激活的对话服务（一期单 provider；二期多 provider 矩阵）
//!
//! 业务逻辑全在 crates/，此处只持句柄。

use std::collections::HashMap;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::{ChatService, McpManager, ToolServer, ToolServerHandle};
use federation::FederationService;
use memory::Memory;
use ontology_store::OntologyStore;
use tokio::sync::{oneshot, RwLock};

use crate::commands::error::AppResult;

/// 应用全局状态。Tauri manage() 注册后，命令通过 `State<'_, AppState>` 取用。
pub struct AppState {
    /// 数据库句柄（rusqlite Connection 在 Mutex 内串行化，Memory 已封装）
    pub memory: Arc<Memory>,

    /// 当前对话服务。None 表示未配置 provider（引导用户去设置页）。
    /// RwLock：读多写少（每次发消息读，配置变更时写）。
    pub chat: RwLock<Option<ChatService>>,

    /// provider 配置（一期存内存 + tauri-plugin-store 持久化；二期落 SQLite 加密）
    pub provider_config: RwLock<Option<agent_core::ProviderConfig>>,

    /// 共享工具句柄：MCP server 的工具注册到此，ChatService 构建时注入 agent。
    /// 始终存在（即使无 MCP server 也是空 handle），避免 Option 包裹。
    pub tool_handle: ToolServerHandle,

    /// MCP 连接管理器。None 表示未配置任何 MCP server。
    /// 工具句柄独立存在（tool_handle），McpManager 只负责保活连接。
    pub mcp: RwLock<Option<McpManager>>,

    /// 联邦查询服务。None 表示尚未初始化完成（FederationService::new 是 async，
    /// 在 setup 的异步任务中构造后写入）。命令内通过 federation() 取用。
    pub federation: RwLock<Option<FederationService>>,

    /// 本体存储（会话页面引用本体）。setup hook 同步 open 后写入。
    /// 会话模式挂 5 个只读 drill-in 工具（不挂建模组），始终可用（无激活集过滤）。
    pub ontology_store: Arc<OntologyStore>,

    /// Skill 管理器（决策 20）。setup hook 构造后写入，始终存在（即使内置 skill
    /// 目录缺失也是空 manager）。命令通过 state.skill_manager 取用。
    pub skill_manager: Arc<agent_core::SkillManager>,

    /// 流式取消信号：assistant_id → oneshot sender。
    /// send_message 启动流式时注册 sender，cancel_stream 命令触发接收端 → 流循环 select! 退出。
    /// Mutex 保护 HashMap；流结束后 remove。
    pub cancel_signals: std::sync::Mutex<HashMap<String, oneshot::Sender<()>>>,
}

impl AppState {
    /// 初始化：接收已 open 的 memory + 已构造的 SkillManager。
    ///
    /// setup hook 先 open memory（供 SkillManager 与 AppState 共用同一连接），
    /// 再构造 SkillManager（需 AppHandle 解析内置 skill 目录），最后调本方法组装 AppState。
    pub fn new_with_memory(
        memory: Arc<Memory>,
        ontology_store: Arc<OntologyStore>,
        skill_manager: Arc<agent_core::SkillManager>,
    ) -> AppResult<Self> {
        let tool_handle = ToolServer::new().run();
        Ok(Self {
            memory,
            chat: RwLock::new(None),
            provider_config: RwLock::new(None),
            tool_handle,
            mcp: RwLock::new(None),
            federation: RwLock::new(None),
            ontology_store,
            skill_manager,
            cancel_signals: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// 内存库（测试用）。skill_manager 用空目录构造（无内置 skill）。
    #[cfg(test)]
    pub fn in_memory() -> AppResult<Self> {
        let memory = Arc::new(Memory::open_in_memory()?);
        let ontology_store = Arc::new(OntologyStore::open_in_memory()?);
        let tool_handle = ToolServer::new().run();
        let skill_manager = Arc::new(agent_core::SkillManager::new(
            std::sync::Arc::clone(&memory),
            PathBuf::from("/nonexistent/builtin"),
            PathBuf::from("/nonexistent/user"),
            vec![],
        ));
        Ok(Self {
            memory,
            chat: RwLock::new(None),
            provider_config: RwLock::new(None),
            tool_handle,
            mcp: RwLock::new(None),
            federation: RwLock::new(None),
            ontology_store,
            skill_manager,
            cancel_signals: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// 异步初始化联邦查询服务（恢复已注册的数据源）。在 setup 异步任务中调用。
    pub async fn init_federation(&self) -> AppResult<()> {
        let svc = FederationService::new(self.memory.clone()).await?;
        let svc_arc = std::sync::Arc::new(svc.clone());
        *self.federation.write().await = Some(svc);
        // 若 ChatService 已就绪（provider 已 restore），注入联邦查询工具 + 本体工具。
        // 否则后续 configure_provider 时会注入（双路径覆盖）。
        if let Some(chat) = self.chat.write().await.as_mut() {
            chat.set_federation(svc_arc.clone());
            chat.set_ontology_store(self.ontology_store.clone());
            tracing::info!("federation + ontology tools injected into ChatService (init_federation)");
        }
        Ok(())
    }
}
