//! 本体校验对抗 IPC 命令（VALIDATION-DESIGN.md v0.3.1）。
//!
//! 薄封装 `crates/ontology-store`（adversarial.rs / ttl_coverage.rs）与
//! `crates/agent-core`（validation_llm.rs）——业务在 crates/，此处只 IPC
//! （AGENTS.md 工程结构硬约束）。命令从 AppState 取 `Arc<TtlStore>` 与
//! `RwLock<Option<ChatService>>`，不持锁跨 await。
//!
//! 覆盖三层对抗的 UI 侧入口：
//!   - L3 答辩：dispute_issue / resolve_dispute / list_issues / persist_issues
//!   - L4.3 语义评审：review_ontology / a_revise / b_recheck / escalate
//!   - L4.1 覆盖矩阵：coverage_matrix
//!   - 模型独立性：independence_tier

use tauri::State;

use ontology_store::ttl_coverage::CoverageMatrix;
use ontology_store::validation::{ValidationIssue, ValidationReview};
use ontology_store::TtlStore;

use crate::commands::error::{AppError, AppResult};
use crate::state::AppState;

/// 运行中评审任务注册表：`ontology_iri → review_id`。
///
/// 评审是 `spawn` 出去的 tokio 任务。进程一退任务就没了，但库里的 review 行还停在
/// `in_progress`——前端按 `deriveStep` 倒推会永远显示"B 评审 进行中"，用时还从第一次
/// 运行起算，看着就是卡死（2026-10-26 现场：一行在 in_progress 上挂了 44.8 小时，
/// 三个 step_ms 全为 0，日志里连一条失败都没有——它不是失败，是消失）。
///
/// 桌面应用是单进程：注册表里没有的 `in_progress` **定义上**就是僵尸，不需要心跳
/// 超时阈值来猜（B 评审在 1416 三元组本体上可能跑很久，任何阈值都会误判）。
static REVIEW_TASKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::OnceLock::new();

fn review_tasks(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    REVIEW_TASKS.get_or_init(Default::default)
}

/// 注销守卫：Drop 时移除登记。**用 Drop 而非手写 remove**——任务即便 panic 展开，
/// 注册表也会清干净，下一次 reap 就能把那一行标成中断；手写清理在 panic 路径上会跳过，
/// 那正是我们要修的故障本身。
struct ReviewTaskGuard {
    ontology_iri: String,
}

impl Drop for ReviewTaskGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = review_tasks().lock() {
            m.remove(&self.ontology_iri);
        }
    }
}

/// 把无主的 `in_progress` 评审标为 `interrupted`。
///
/// 调用时机：① 应用启动（setup hook，ttl_store 就绪后）；② 每次前端读评审状态。
/// 后者是为了兜住"任务被 tokio 取消但进程还活着"这类没有重启的情形。
pub(crate) fn reap_orphan_reviews(store: &TtlStore) {
    let live: Vec<String> = review_tasks()
        .lock()
        .map(|m| m.values().cloned().collect())
        .unwrap_or_default();
    if let Err(e) = store.reap_interrupted(&live) {
        tracing::warn!(error = %e, "reap_orphan_reviews 失败");
    }
}

fn get_ttl_store(state: &AppState) -> AppResult<std::sync::Arc<TtlStore>> {
    Ok(state.ttl_store.clone())
}

// ════════════════════════════════════════════════════════════════
// L3 规则答辩（第二层对抗）
// ════════════════════════════════════════════════════════════════
// 合并方案 v0.8：独立答辩通道已并入语义评审的 A 修订环节——A 对规则告警的
// 异议在 A 修订时提交（带证据引用，机械匹配把关），B 复核裁决；不再有
// dispute_issue / resolve_dispute_llm 命令。resolve_dispute 保留供人工仲裁。

/// 裁决答辩（第 ② 步）。由人工仲裁调用——**不作为 A 的工具暴露**。
/// verdict: "overruled"（撤销）| "accepted"（升级 error）；resolved_by: "verifier"|"human"。
#[tauri::command]
#[specta::specta]
pub async fn resolve_dispute(
    state: State<'_, AppState>,
    issue_id: String,
    verdict: String,
    resolved_by: String,
) -> AppResult<String> {
    let store = get_ttl_store(&state)?;
    store
        .resolve_dispute(&issue_id, &verdict, &resolved_by)
        .map_err(AppError::from)
}

/// 查询本体的全部 issue（对抗状态展示 + 报告）。
#[tauri::command]
#[specta::specta]
pub async fn list_validation_issues(
    state: State<'_, AppState>,
    ontology_iri: String,
) -> AppResult<Vec<ValidationIssue>> {
    let store = get_ttl_store(&state)?;
    store.list_issues(&ontology_iri).map_err(AppError::from)
}

/// 持久化一次 validate 的 issue 快照（前端校验完成后调用）。
#[tauri::command]
#[specta::specta]
pub async fn persist_validation_issues(
    state: State<'_, AppState>,
    ontology_iri: String,
    validation_id: String,
    issues: Vec<ValidationIssue>,
) -> AppResult<u32> {
    let store = get_ttl_store(&state)?;
    let n = store
        .persist_issues(&ontology_iri, &validation_id, &issues)
        .map_err(AppError::from)?;
    Ok(n as u32)
}

/// 持久化 B 评审输出（L4.3 第 1 步产物，仲裁/审计回溯用）。
#[tauri::command]
#[specta::specta]
pub async fn save_b_report(
    state: State<'_, AppState>,
    review_id: String,
    b_report: String,
) -> AppResult<ValidationReview> {
    let store = get_ttl_store(&state)?;
    store
        .save_b_report(&review_id, &b_report)
        .map_err(AppError::from)
}

// ════════════════════════════════════════════════════════════════
// L4.3 语义评审（第三层对抗）
// ════════════════════════════════════════════════════════════════

/// 启动 L4.3 语义评审：创建 validation_reviews 行（round=1, in_progress）。
#[tauri::command]
#[specta::specta]
pub async fn review_ontology(
    state: State<'_, AppState>,
    ontology_iri: String,
) -> AppResult<ValidationReview> {
    let store = get_ttl_store(&state)?;
    store.review_ontology(&ontology_iri).map_err(AppError::from)
}

/// A 提交修订（第 2 步）：存修订全文 + 修订说明（a_reasoning 对 B 复核不可见）。
#[tauri::command]
#[specta::specta]
pub async fn a_revise(
    state: State<'_, AppState>,
    review_id: String,
    a_revision: String,
    a_reasoning: String,
) -> AppResult<ValidationReview> {
    let store = get_ttl_store(&state)?;
    store
        .a_revise(&review_id, &a_revision, &a_reasoning)
        .map_err(AppError::from)
}

/// B 复核（第 3 步）：confirm=true → resolved；false → escalated（转人工仲裁）。
/// （前端「跳过评审」入口——无 LLM 复核输出，b_verdict 落空串。）
#[tauri::command]
#[specta::specta]
pub async fn b_recheck(
    state: State<'_, AppState>,
    review_id: String,
    confirm: bool,
) -> AppResult<ValidationReview> {
    let store = get_ttl_store(&state)?;
    store.b_recheck(&review_id, confirm, "").map_err(AppError::from)
}

/// 转人工仲裁（超限/分歧）。
#[tauri::command]
#[specta::specta]
pub async fn escalate_review(
    state: State<'_, AppState>,
    review_id: String,
) -> AppResult<ValidationReview> {
    let store = get_ttl_store(&state)?;
    store.escalate(&review_id, "manual_escalation", "").map_err(AppError::from)
}

/// 查询本体的最新 review（可为 None）。
#[tauri::command]
#[specta::specta]
pub async fn latest_validation_review(
    state: State<'_, AppState>,
    ontology_iri: String,
) -> AppResult<Option<ValidationReview>> {
    let store = get_ttl_store(&state)?;
    reap_orphan_reviews(&store);
    store.latest_review(&ontology_iri).map_err(AppError::from)
}

// ════════════════════════════════════════════════════════════════
// L4.1 覆盖矩阵
// ════════════════════════════════════════════════════════════════

/// 运行 29 句话覆盖矩阵（全量或增量 diff）。
/// `diff_entities`: 增量模式传 change_summary 的实体列表；空 = 全量。
#[tauri::command]
#[specta::specta]
pub async fn coverage_matrix(
    state: State<'_, AppState>,
    ontology_iri: String,
    diff_entities: Vec<String>,
) -> AppResult<CoverageMatrix> {
    let store = get_ttl_store(&state)?;
    // 从库内取本体 TTL，解析到内存 oxigraph 后跑覆盖检查
    let ttl = store.export_ttl(&ontology_iri).map_err(AppError::from)?;
    let temp = ontology_store::ttl::parse_ttl_to_memory(&ttl)
        .map_err(|e| AppError::Ontology(format!("解析失败: {e}")))?;
    Ok(ontology_store::ttl_coverage::coverage_matrix(&temp, &diff_entities))
}

// ════════════════════════════════════════════════════════════════
// 模型独立性判定（§4.4 / §4.4.1）
// ════════════════════════════════════════════════════════════════

/// 判定 A/B 模型独立性档位（前端设置页实时显示）。
#[tauri::command]
#[specta::specta]
pub async fn independence_tier(
    provider_a: String,
    model_a: String,
    provider_b: String,
    model_b: String,
) -> AppResult<String> {
    let tier = agent_core::validation_llm::independence_tier(
        &provider_a,
        &model_a,
        &provider_b,
        &model_b,
    );
    Ok(tier.label().to_string())
}

// ════════════════════════════════════════════════════════════════
// 语义评审 LLM 编排（B/A 调用，需要 ChatService）
// ════════════════════════════════════════════════════════════════

/// 一次性触发完整语义评审循环（后端驱动，前端零环节链）。
///
/// 设计动机（v0.4.2 架构修正）：此前评审流程在前端 ReviewPanel 的 async 链里
/// 逐步驱动——切页面组件卸载后 setState 失效、恢复逻辑误判「上次会话中断」，
/// 且用户点重新评审会重复跑已完成环节。改为**后端 tokio 任务**：
///   startReview → B评审(run_b_review) → save_b_report → A修订(run_a_revise)
///   → a_revise → B复核(run_b_recheck) → b_recheck
/// 每步落库，任务不随前端组件生命周期中断；前端只需触发一次，然后
/// 轮询 latest_validation_review 展示进度（b_report/a_revision/status
/// 已持久化，任何时刻重挂载都能恢复真实状态）。
///
/// 返回 review id（前端轮询用）。若已有 in_progress review（残留/进行中），
/// 复用该行继续（不重复 startReview）；review_ontology 的残留重置防护
/// 只对非 in_progress 生效，这里显式处理。
#[tauri::command]
#[specta::specta]
pub async fn run_review_loop(
    state: State<'_, AppState>,
    ontology_iri: String,
    ttl_content: String,
    charter_summary: String,
    issues_json: String,
) -> AppResult<String> {
    // 取 A（建模者）与 B（校验者）通道
    let chat_a = {
        let c = state.chat.read().await;
        // ChatService 已 derive Clone（内部全 Arc/String），clone 供 spawn 闭包 move
        c.clone()
            .ok_or_else(|| AppError::Ontology("未配置建模者 provider，无法运行语义评审".into()))?
    };
    let chat_b = crate::commands::provider::verifier_chat(&state)
        .await
        .ok_or_else(|| AppError::Ontology("未配置校验者 provider，无法运行语义评审".into()))?;
    let cfg_a = {
        let c = state.provider_config.read().await;
        c.clone()
            .ok_or_else(|| AppError::Ontology("未配置建模者 provider".into()))?
    };
    let model_a = cfg_a.model;
    let model_b = {
        let v = state.verifier_config.read().await;
        v.as_ref().map(|c| c.model.clone()).unwrap_or_else(|| model_a.clone())
    };
    let store = get_ttl_store(&state)?;
    // 先回收无主评审：否则「复用已有 in_progress」分支会捡起一个任务早就不在的僵尸行，
    // 在它上面重跑，界面继续显示"进行中"且用时从第一次运行起算。
    reap_orphan_reviews(&store);

    // 复用已有 in_progress review，否则新建
    let review_id = {
        let existing = store.latest_review(&ontology_iri)?;
        match existing {
            Some(r) if r.status == "in_progress" => r.id,
            _ => store.review_ontology(&ontology_iri)?.id,
        }
    };
    // 登记任务归属（进程内注册表）；任务结束时由 Drop 守卫注销。
    if let Ok(mut m) = review_tasks().lock() {
        m.insert(ontology_iri.clone(), review_id.clone());
    }

    // 后台任务：完整循环（每步落库；失败标记 escalated 保留审计轨迹）。
    // 多轮对抗（§2.2.1，2026-09-24）：B 复核追问且未达轮次封顶 → 拿残留问题
    // 喂 A 再修订一轮、B 再复核；封顶才升级仲裁（store 层 b_recheck 状态机管封顶）。
    let store2 = store.clone();
    let chat_a2 = chat_a.clone();
    let chat_b2 = chat_b.clone();
    let review_id2 = review_id.clone();
    let ontology_iri2 = ontology_iri.clone();
    tauri::async_runtime::spawn(async move {
        // 注销守卫：任务返回（含 panic 展开）即从注册表移除，下一次 reap 就能把
        // 这一行标成 interrupted——不靠手写清理，否则 panic 路径会跳过清理。
        let _task_guard = ReviewTaskGuard {
            ontology_iri: ontology_iri2.clone(),
        };
        let run = async {
            // 合并方案 v0.8：规则告警（R 编号）并入 A 修订输入——A 对统一发现池
            // （B 评审建议 + 规则 warning/suggestion）逐条表态，不再走独立答辩通道。
            // 必须在 issues_json move 进 issue_lines 之前构建。
            let rules_warnings = build_rules_warnings(&issues_json);
            // ── 环节 1：B 评审（八维度）──
            // 输入必须五段齐全（VALIDATION-DESIGN §2.2.1）：宪章 + 29 句话与覆盖矩阵
            // + Turtle + L0-L3 报告 + 证据上下文。此前这里自拼了三段，缺后两类输入，
            // 导致 ①事实忠实度 与 ②完整性 两个维度无从执行（build_b_review_inputs
            // 早已实现并带单测，只是从没被接上）。
            let issue_lines = if issues_json.trim().is_empty() {
                "（无规则检查报告）".to_string()
            } else {
                issues_json
            };
            let coverage = render_coverage_context(&ttl_content);
            let evidence = render_evidence_context(
                &store2
                    .list_evidence_sources_for_dispute(&ontology_iri2)
                    .unwrap_or_default(),
            );
            let inputs = agent_core::validation_llm::build_b_review_inputs(
                &charter_summary,
                &coverage,
                &ttl_content,
                &issue_lines,
                &evidence,
            );
            let b_out = agent_core::validation_llm::run_b_review(&chat_b2, &model_b, &inputs)
                .await
                .map_err(|e| e.to_string())?;
            if !b_out.ok {
                return Err(b_out.error.unwrap_or_else(|| "B 评审失败".into()));
            }
            store2.save_b_report(&review_id2, &b_out.suggestions_json).map_err(|e| e.to_string())?;

            // ── 环节 2/3：A 修订 → B 复核，追问未封顶则循环（≤2 轮修订复核）──
            // 当前待修订的 Turtle：首轮用原稿，后续轮用上一轮修订稿（b_recheck
            // 进下一轮时会清 a_revision，修订链以本地变量为准）
            let mut current_ttl = ttl_content.clone();
            // 后续轮 A 的输入建议 = 上一轮 B 复核的残留问题清单（remaining_issues）
            let mut pending_suggestions = b_out.suggestions_json.clone();
            loop {
                let (revised_ttl, reasoning) = agent_core::validation_llm::run_a_revise(
                    &chat_a2,
                    &model_a,
                    &current_ttl,
                    &pending_suggestions,
                    &rules_warnings,
                )
                .await
                .map_err(|e| e.to_string())?;
                store2.a_revise(&review_id2, &revised_ttl, &reasoning).map_err(|e| e.to_string())?;

                let verdict = agent_core::validation_llm::run_b_recheck(
                    &chat_b2,
                    &model_b,
                    &revised_ttl,
                    &pending_suggestions,
                )
                .await
                .map_err(|e| e.to_string())?;
                // 判定走解析而非子串匹配：仅显式 verdict=confirm 算收敛，
                // 解析失败/缺字段/未知值一律按未收敛（宁可多走一轮或进仲裁，
                // 也不在判定不可靠时把修订稿整本写回工作区）。
                let confirm = agent_core::validation_llm::verdict_is_confirm(&verdict);
                // b_recheck 三态：确认 → resolved；追问未封顶 → 保持 in_progress
                //（返回行 status 仍为 in_progress）；封顶 → escalated
                let after = store2
                    .b_recheck(&review_id2, confirm, &verdict)
                    .map_err(|e| e.to_string())?;
                if after.status != "in_progress" {
                    return Ok::<(), String>(());
                }
                // 追问进下一轮：残留问题清单作为 A 下一轮的输入建议
                current_ttl = revised_ttl;
                pending_suggestions = verdict;
            }
        };
        if let Err(e) = run.await {
            // 失败 → 标记 escalated（转人工仲裁，保留已落库产物与审计轨迹）
            let _ = store2.escalate(&review_id2, "step_failed", &e);
            tracing::error!("run_review_loop {review_id2} 失败: {e}");
        }
    });

    Ok(review_id)
}


/// 运行 B 语义评审（LLM）。需要已配置的 ChatService。
/// `inputs` 由前端/编排层构造（charter + 29句话 + Turtle + 报告 + evidence 上下文）。
#[tauri::command]
#[specta::specta]
pub async fn run_b_review_llm(
    state: State<'_, AppState>,
    model_b: String,
    inputs: String,
) -> AppResult<agent_core::validation_llm::BReviewOutput> {
    // B 评审走校验者独立通道（verifier_provider 独立配置；未配置兜底回落建模者）
    let chat = crate::commands::provider::verifier_chat(&state)
        .await
        .ok_or_else(|| AppError::Ontology("未配置 provider，无法运行语义评审".into()))?;
    agent_core::validation_llm::run_b_review(&chat, &model_b, &inputs)
        .await
        .map_err(|e| AppError::Ontology(e))
}

/// A 修订结果（命名结构，避免 specta 导出匿名 tuple）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct AReviseResult {
    /// 修订后的 Turtle 全文
    pub revised_ttl: String,
    /// 修订说明 JSON（A 的接受/拒绝理由，对 B 复核不可见）
    pub reasoning: String,
}

/// 运行 A 修订（LLM）。返回命名结构（修订后 Turtle, 修订说明）。
#[tauri::command]
#[specta::specta]
pub async fn run_a_revise_llm(
    state: State<'_, AppState>,
    model_a: String,
    ttl_v1: String,
    b_suggestions: String,
) -> AppResult<AReviseResult> {
    let chat = state.chat.read().await;
    let Some(chat) = chat.as_ref() else {
        return Err(AppError::Ontology("未配置 provider，无法运行修订".into()));
    };
    // 合并方案 v0.8：此命令供 error 纠错/仲裁后修订使用，不处理规则告警（rules_warnings 传空）
    let (revised_ttl, reasoning) =
        agent_core::validation_llm::run_a_revise(chat, &model_a, &ttl_v1, &b_suggestions, "")
            .await
            .map_err(|e| AppError::Ontology(e))?;
    Ok(AReviseResult { revised_ttl, reasoning })
}

/// 运行 B 复核（LLM）。传修订后 Turtle（a_revision）+ 核查基准（上一轮建议清单），
/// **不传修订说明**（a_reasoning 对 B 不可见）。
///
/// 遗留的单步命令：阶段② 已改为后端 `run_review_loop` 编排，UI 不再逐步驱动。
/// 保留它时也必须带上核查基准——否则这个入口会重演"B 失忆"缺陷。
#[tauri::command]
#[specta::specta]
pub async fn run_b_recheck_llm(
    state: State<'_, AppState>,
    model_b: String,
    a_revision: String,
    baseline_suggestions: String,
) -> AppResult<String> {
    let chat = crate::commands::provider::verifier_chat(&state)
        .await
        .ok_or_else(|| AppError::Ontology("未配置 provider，无法运行复核".into()))?;
    agent_core::validation_llm::run_b_recheck(&chat, &model_b, &a_revision, &baseline_suggestions)
        .await
        .map_err(|e| AppError::Ontology(e))
}

// ════════════════════════════════════════════════════════════════
// 仲裁后 A 修订落盘（后端驱动：spawn + 状态缓存，切窗不丢）
// ════════════════════════════════════════════════════════════════

/// 仲裁后 A 修订的状态缓存（按本体 IRI）——running 标志 + 已用时 + 结果消息，
/// 切窗回来轮询恢复"进行中"展示 + 显示耗时（对齐答辩/评审后端驱动模式）。
#[derive(Debug, Clone, Default, serde::Serialize, specta::Type)]
pub struct PostArbitrationState {
    /// 后端修订任务是否进行中
    pub running: bool,
    /// 任务开始时间戳 ms（进行中时前端用 now-started_at 算实时已用时；
    /// 完成后不再用，elapsed_ms 是最终耗时）
    #[serde(default)]
    #[specta(type = specta_typescript::Number)] // u64 时间戳 < 2^53，IPC 导出降级 number（BigInt 公约）
    pub started_at: u64,
    /// 已用时 ms（完成后写入最终耗时；进行中为 0，前端用 started_at 算）
    #[serde(default)]
    #[specta(type = specta_typescript::Number)] // 同上
    pub elapsed_ms: u64,
    /// 完成后的结果消息（ok/err）
    #[serde(default)]
    pub msg: String,
    /// ok=成功 / err=失败 / ""=进行中
    #[serde(default)]
    pub kind: String,
    /// A 修订说明（run_a_revise 返回的 notes：改了哪些内容；成功后填充）
    #[serde(default)]
    pub revise_notes: String,
}

static POST_ARB_RESULTS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, PostArbitrationState>>,
> = std::sync::OnceLock::new();

fn post_arb_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, PostArbitrationState>> {
    POST_ARB_RESULTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 读取仲裁后 A 修订状态（切窗回来回显 + 显示耗时）。
#[tauri::command]
#[specta::specta]
pub async fn get_post_arbitration_state(
    ontology_iri: String,
) -> AppResult<PostArbitrationState> {
    Ok(post_arb_cache()
        .lock()
        .map(|m| m.get(&ontology_iri).cloned().unwrap_or_default())
        .unwrap_or_default())
}

/// 仲裁完成后交 A 修订落盘（后端 spawn，立即返回；前端轮询 get_post_arbitration_state）。
/// 已裁决 issue（accepted 必须改、overruled 不用改）作为 B 建议喂 A，A 修订后
/// import_ttl overwrite 落盘。
#[tauri::command]
#[specta::specta]
pub async fn run_post_arbitration_revise(
    state: State<'_, AppState>,
    ontology_iri: String,
) -> AppResult<()> {
    let store = get_ttl_store(&state)?;
    let chat_a = {
        let c = state.chat.read().await;
        c.clone()
            .ok_or_else(|| AppError::Ontology("未配置建模者 provider".into()))?
    };
    let cfg_a = {
        let c = state.provider_config.read().await;
        c.clone()
            .ok_or_else(|| AppError::Ontology("未配置建模者 provider".into()))?
    };
    let model_a = cfg_a.model;

    // 取当前本体 TTL + 已裁决 issue
    let ttl = store
        .export_ttl(&ontology_iri)
        .map_err(AppError::from)?;
    let adjudicated = store
        .list_issues(&ontology_iri)
        .map_err(AppError::from)?
        .into_iter()
        .filter(|i| i.status == ontology_store::validation::IssueStatus::Accepted
            || i.status == ontology_store::validation::IssueStatus::Overruled)
        .collect::<Vec<_>>();
    if adjudicated.is_empty() {
        return Ok(());
    }
    // 构造 A 输入：只给问题描述 + 修订建议，不带 issue id / 过程标签（如"仲裁
    // Bxx"）——这些是校验过程信息，不能泄漏进 Turtle 本体。
    let lines = adjudicated
        .iter()
        .filter(|i| i.status == ontology_store::validation::IssueStatus::Accepted)
        .map(|i| {
            let mut s = format!("- {}", i.message);
            if let Some(sug) = i.suggestion.as_ref() {
                s.push_str(&format!("（建议: {sug}）"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n");
    let b_suggestions = format!(
        "以下问题经人工仲裁确认成立，请据此修订本体（只改本体内容，不要把问题描述写进本体）：\n{lines}"
    );

    // 置 running=true，spawn 后台修订
    {
        if let Ok(mut m) = post_arb_cache().lock() {
            m.insert(ontology_iri.clone(), PostArbitrationState {
                running: true, started_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0), elapsed_ms: 0,
                msg: String::new(), kind: String::new(), revise_notes: String::new(),
            });
        }
    }
    let store2 = store.clone();
    let ont = ontology_iri.clone();
    let started = std::time::Instant::now();
    tauri::async_runtime::spawn(async move {
        // provider 偶发返回 "error decoding response body"（连接被网关掐断/响应体
        // 不完整），加重试最多 3 次——整本重写耗时长，首次失败重试通常能成功。
        let mut last_err = String::new();
        let mut revise_notes = String::new();
        let mut ok = false;
        for attempt in 1..=3 {
            match agent_core::validation_llm::run_a_revise(
                &chat_a, &model_a, &ttl, &b_suggestions, "",
            )
            .await
            {
                Ok((revised_ttl, notes)) => match store2.import_ttl(&revised_ttl, true) {
                    Ok(_) => {
                        ok = true;
                        revise_notes = notes;
                        // 落一条 changelog（本体工作台「历史」Tab 可查修订记录）；
                        // title+body ≤500 chars 的约束做截断保护
                        let body = truncate_chars(&revise_notes, 480);
                        let _ = store2.commit_change(
                            &ont,
                            "仲裁后 A 修订落盘",
                            &body,
                            r#"{"arbitration_revise": true}"#,
                            None,
                            "agent",
                        );
                        break;
                    }
                    Err(e) => {
                        last_err = format!("落盘失败：{e}");
                        break;
                    }
                },
                Err(e) => {
                    last_err = e;
                    tracing::warn!("仲裁后 A 修订第 {attempt} 次失败：{last_err}");
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                }
            }
        }
        let elapsed = started.elapsed().as_millis() as u64;
        let (kind, msg) = if ok {
            ("ok".to_string(), "A 修订完成，修订稿已落盘".to_string())
        } else {
            ("err".to_string(), format!("A 修订失败（已重试 3 次）：{last_err}"))
        };
        if let Ok(mut m) = post_arb_cache().lock() {
            if let Some(s) = m.get_mut(&ont) {
                s.running = false;
                s.elapsed_ms = elapsed;
                s.msg = msg;
                s.kind = kind;
                s.revise_notes = revise_notes;
            }
        }
    });
    Ok(())
}

/// 按字符数截断（CJK 安全，不按字节切）。
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}


/// B 评审证据上下文的注入预算。上限依据 1416 三元组本体的实测（2026-11-04）：
/// Turtle 正文 65,758 字符 + 矩阵约 2,600 + 宪章与规则报告约 3,000；证据源 5 个合计
/// 43,978 字符（单个最大 33,004）。按下面的预算注入后总输入约 77k 字符（≈26k–45k
/// token），比改动前增加约 12%，在 128k 窗口内有余量。
const EVIDENCE_PER_SOURCE_CHARS: usize = 1200;
const EVIDENCE_TOTAL_CHARS: usize = 6000;

// 已知遗留：摘录取的是**头部** N 字符。33k 字符的文档只取头部 1,200 字符，相关片段
// 大概率不在其中——"B 比 A 信息更多"目前只做到"有信息"，还没做到"给对信息"。
// 按相关性检索片段需要另一套取段策略，留待后续 change。

/// 渲染 29 句话 + 覆盖矩阵为 B 评审的完整性基准段。
///
/// 未落地条目逐条列（B 据此判断缺失是否属语义级问题）；已落地条目折叠为编号列表
/// ——设计 §2.2.1 的去重约定：完整性以矩阵为底，B 只报矩阵抓不住的语义级增量，
/// 不重复罗列矩阵已高亮的缺失项。一次调用同时满足"注入 29 句话"与"注入矩阵结果"。
fn render_coverage_context(ttl: &str) -> String {
    let parsed = match ontology_store::ttl::parse_ttl_to_memory(ttl) {
        Ok(s) => s,
        Err(e) => {
            return format!(
                "（覆盖矩阵不可用：本体解析失败 {e}。完整性维度请按 29 句话规范逐条核对产物）"
            );
        }
    };
    let m = ontology_store::ttl_coverage::coverage_matrix(&parsed, &[]);
    let mut missing: Vec<String> = Vec::new();
    let mut covered_ids: Vec<String> = Vec::new();
    for item in &m.items {
        if item.covered {
            covered_ids.push(item.sentence_id.clone());
        } else {
            missing.push(format!(
                "- {}（类别 {}）{}｜期望产物形态：{}",
                item.sentence_id, item.category, item.description, item.product_shape
            ));
        }
    }
    let mut out = format!(
        "覆盖率 {}/{}（{:.0}%）。未落地判定来自名词级机械比对，语义级的类型/粒度错配需要你补充。\n",
        m.covered_count,
        m.total_count,
        m.coverage * 100.0
    );
    if missing.is_empty() {
        out.push_str("未落地条目：无（机械比对全部命中）。\n");
    } else {
        out.push_str("未落地条目：\n");
        out.push_str(&missing.join("\n"));
        out.push('\n');
    }
    if !covered_ids.is_empty() {
        out.push_str(&format!(
            "已落地条目（仅编号，不要重复罗列）：{}\n",
            covered_ids.join("、")
        ));
    }
    out
}

/// 渲染已登记证据源为 B 评审的证据上下文（B 不做自主检索，由编排预注入）。
///
/// 无证据源时 MUST 显式声明缺失、MUST NOT 留空：留空会让模型用先验补位，产出
/// 看着合理实则无据的批评；而 B 的"每条建议必须带证据"约束在没有证据源时
/// 会直接憋死输出——所以缺失声明里同时给出替代的引用方式。
fn render_evidence_context(sources: &[(String, String, String)]) -> String {
    if sources.is_empty() {
        return "（未登记证据源——①事实忠实度维度无法核查，仅按宪章与 Turtle 评审；evidence_ref 请引用宪章条目或 29 句话句号）".to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for (idx, (id, title, content)) in sources.iter().enumerate() {
        let body = truncate_chars(content, EVIDENCE_PER_SOURCE_CHARS);
        let cost = body.chars().count();
        if used + cost > EVIDENCE_TOTAL_CHARS {
            out.push_str(&format!(
                "…（其余 {} 个证据源因注入预算未展开）\n",
                sources.len() - idx
            ));
            break;
        }
        used += cost;
        out.push_str(&format!("E{} 编号={id}｜{title}\n{body}\n\n", idx + 1));
    }
    out
}

/// 合并方案 v0.8：从规则检查报告提取 warning/suggestion 清单并编号 R1..Rn，
/// 供 A 修订统一表态（R 编号回传后按序映射到 rules 域 issue）。
/// 非 JSON（LLM/前端文本）→ 原文透传（A 按内容表态，后端按 rule 名匹配）。
fn build_rules_warnings(issues_json: &str) -> String {
    let trimmed = issues_json.trim();
    if trimmed.is_empty() {
        return "（无规则告警）".to_string();
    }
    if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let severity = item.get("severity").and_then(|v| v.as_str()).unwrap_or("");
            if severity == "error" {
                continue;
            }
            let rule = item.get("rule").and_then(|v| v.as_str()).unwrap_or("");
            let message = item.get("message").and_then(|v| v.as_str()).unwrap_or("");
            out.push(format!("R{} [{}] {}: {}", i + 1, severity, rule, message));
        }
        if out.is_empty() {
            return "（无待处理规则告警）".to_string();
        }
        return out.join("\n");
    }
    trimmed.to_string()
}

/// 轮次清单（合并方案 v0.8）：按轮次返回该评审的 issue 快照 + 统计，
/// 前端「轮次清单」可查看每轮 发现/已修改/异议/裁决。
#[tauri::command]
#[specta::specta]
pub async fn list_review_rounds(
    state: State<'_, AppState>,
    review_id: String,
) -> AppResult<Vec<ontology_store::validation::ReviewRound>> {
    let store = get_ttl_store(&state)?;
    store.list_review_rounds(&review_id).map_err(AppError::from)
}
