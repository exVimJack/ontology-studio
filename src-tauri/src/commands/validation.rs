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

fn get_ttl_store(state: &AppState) -> AppResult<std::sync::Arc<TtlStore>> {
    Ok(state.ttl_store.clone())
}

// ════════════════════════════════════════════════════════════════
// L3 规则答辩（第二层对抗）
// ════════════════════════════════════════════════════════════════

/// A 对 warning 级 issue 发起答辩。两步裁决第 ① 步（counterEvidence 机械匹配）
/// 在此完成——匹配失败直接返回错误，不进入 argument 判定。
#[tauri::command]
#[specta::specta]
pub async fn dispute_issue(
    state: State<'_, AppState>,
    issue_id: String,
    argument: String,
    counter_source_id: String,
    counter_quote: String,
) -> AppResult<String> {
    let store = get_ttl_store(&state)?;
    store
        .dispute_issue(&issue_id, &argument, &counter_source_id, &counter_quote)
        .map_err(AppError::from)
}

/// 裁决答辩（第 ② 步）。由 B 复核或人工调用——**不作为 A 的工具暴露**。
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

/// B 自动裁决答辩（②b 机器裁决）：LLM 判定 overruled/accepted；uncertain 保持
/// disputed（进入人工仲裁队列）。**resolved_by 记 verifier**——裁决权在 B。
#[tauri::command]
#[specta::specta]
pub async fn resolve_dispute_llm(
    state: State<'_, AppState>,
    issue_id: String,
    model_b: String,
) -> AppResult<String> {
    let store = get_ttl_store(&state)?;
    judge_disputed_issue(&store, &state, &issue_id, &model_b).await
}

/// 裁决核心：取答辩上下文 → B LLM 裁决 → overruled/accepted 落库，
/// uncertain 保持 disputed（升级人工仲裁）。resolve_dispute_llm 与
/// dispute_and_judge 共用。
async fn judge_disputed_issue(
    store: &std::sync::Arc<TtlStore>,
    state: &State<'_, AppState>,
    issue_id: &str,
    model_b: &str,
) -> AppResult<String> {
    let Some((message, argument, quote)) = store
        .get_dispute_context(issue_id)
        .map_err(AppError::from)?
    else {
        return Err(AppError::Ontology(
            "issue 不存在、不处于 disputed 状态，或答辩材料不完整".into(),
        ));
    };
    let chat = crate::commands::provider::verifier_chat(state)
        .await
        .ok_or_else(|| AppError::Ontology("未配置校验者 provider，无法自动裁决".into()))?;
    let out = agent_core::validation_llm::run_dispute_verdict(
        &chat, model_b, &message, &argument, &quote,
    )
    .await
    .map_err(AppError::Ontology)?;
    if out.contains("\"overruled\"") {
        store
            .resolve_dispute(issue_id, "overruled", "verifier")
            .map_err(AppError::from)?;
        Ok("overruled".into())
    } else if out.contains("\"uncertain\"") {
        // B 判不定 → 保持 disputed，进入人工仲裁队列
        Ok("uncertain".into())
    } else {
        store
            .resolve_dispute(issue_id, "accepted", "verifier")
            .map_err(AppError::from)?;
        Ok("accepted".into())
    }
}

/// 答辩提交 + B 自动机器裁决（一步到位）。
///
/// 前端提交后即可离开页面：裁决在 Rust 侧完成并落库，切窗/切页不影响结果，
/// 回来后按 issue 状态回显（overruled/accepted 已闭环，uncertain 在仲裁队列）。
#[tauri::command]
#[specta::specta]
pub async fn dispute_and_judge(
    state: State<'_, AppState>,
    issue_id: String,
    argument: String,
    counter_source_id: String,
    counter_quote: String,
    model_b: String,
) -> AppResult<String> {
    let store = get_ttl_store(&state)?;
    // 第一步：提交答辩（含反证机械匹配，不匹配直接报错）
    store
        .dispute_issue(&issue_id, &argument, &counter_source_id, &counter_quote)
        .map_err(AppError::from)?;
    // 第二步：B 机器裁决（overruled/accepted 落库；uncertain 保持 disputed）
    judge_disputed_issue(&store, &state, &issue_id, &model_b).await
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
    store.escalate(&review_id).map_err(AppError::from)
}

/// 查询本体的最新 review（可为 None）。
#[tauri::command]
#[specta::specta]
pub async fn latest_validation_review(
    state: State<'_, AppState>,
    ontology_iri: String,
) -> AppResult<Option<ValidationReview>> {
    let store = get_ttl_store(&state)?;
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

    // 复用已有 in_progress review，否则新建
    let review_id = {
        let existing = store.latest_review(&ontology_iri)?;
        match existing {
            Some(r) if r.status == "in_progress" => r.id,
            _ => store.review_ontology(&ontology_iri)?.id,
        }
    };

    // 后台任务：完整循环（每步落库；失败标记 escalated 保留审计轨迹）。
    // 多轮对抗（§2.2.1，2026-09-24）：B 复核追问且未达轮次封顶 → 拿残留问题
    // 喂 A 再修订一轮、B 再复核；封顶才升级仲裁（store 层 b_recheck 状态机管封顶）。
    let store2 = store.clone();
    let chat_a2 = chat_a.clone();
    let chat_b2 = chat_b.clone();
    let review_id2 = review_id.clone();
    tauri::async_runtime::spawn(async move {
        let run = async {
            // ── 环节 1：B 评审（八维度）──
            let issue_lines = if issues_json.trim().is_empty() {
                "（无规则检查报告）".to_string()
            } else {
                issues_json
            };
            let inputs = format!(
                "【设计宪章】\n{charter_summary}\n\n【待评审 Turtle】\n{ttl_content}\n\n【规则检查报告】\n{issue_lines}"
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
                )
                .await
                .map_err(|e| e.to_string())?;
                store2.a_revise(&review_id2, &revised_ttl, &reasoning).map_err(|e| e.to_string())?;

                let verdict = agent_core::validation_llm::run_b_recheck(&chat_b2, &model_b, &revised_ttl)
                    .await
                    .map_err(|e| e.to_string())?;
                let confirm = !verdict.contains("\"verdict\": \"question\"");
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
            let _ = store2.escalate(&review_id2);
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
    let (revised_ttl, reasoning) =
        agent_core::validation_llm::run_a_revise(chat, &model_a, &ttl_v1, &b_suggestions)
            .await
            .map_err(|e| AppError::Ontology(e))?;
    Ok(AReviseResult { revised_ttl, reasoning })
}

/// 运行 B 复核（LLM）。只传修订后 Turtle（a_revision），不传修订说明。
#[tauri::command]
#[specta::specta]
pub async fn run_b_recheck_llm(
    state: State<'_, AppState>,
    model_b: String,
    a_revision: String,
) -> AppResult<String> {
    let chat = crate::commands::provider::verifier_chat(&state)
        .await
        .ok_or_else(|| AppError::Ontology("未配置 provider，无法运行复核".into()))?;
    agent_core::validation_llm::run_b_recheck(&chat, &model_b, &a_revision)
        .await
        .map_err(|e| AppError::Ontology(e))
}

/// 最近一次 run_auto_dispute 的状态缓存（按本体 IRI）——切页/切窗回来回显，
/// 不依赖前端组件存活。running=true 表示后端任务进行中（切窗不丢状态）。
/// 应用重启即清（issue 状态本身已落库）。
#[derive(Debug, Clone, Default, serde::Serialize, specta::Type)]
pub struct AutoDisputeState {
    /// 后端任务是否进行中（切窗回来据此恢复"进行中"展示，不依赖前端 mutation 状态）
    pub running: bool,
    /// 逐条结果（进行中已完成的条目 / 全部完成后的完整结果）
    #[serde(default)]
    pub items: Vec<AutoDisputeItem>,
}

static AUTO_DISPUTE_RESULTS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, AutoDisputeState>>,
> = std::sync::OnceLock::new();

fn auto_dispute_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, AutoDisputeState>> {
    AUTO_DISPUTE_RESULTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 读取缓存的自动答辩状态（切页/切窗回来回显用，含 running 标志）。
#[tauri::command]
#[specta::specta]
pub async fn get_auto_dispute_result(
    ontology_iri: String,
) -> AppResult<AutoDisputeState> {
    Ok(auto_dispute_cache()
        .lock()
        .map(|m| m.get(&ontology_iri).cloned().unwrap_or_default())
        .unwrap_or_default())
}

/// A 自动答辩的批量结果（run_auto_dispute 逐条汇总，前端进度展示）。
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
pub struct AutoDisputeItem {
    /// 目标 warning 的 issue id
    pub issue_id: String,
    /// "overruled"（B 撤销）| "accepted"（B 驳回升级 error）| "uncertain"
    /// （B 判不定进仲裁）| "accept"（A 接受不辩）| "rejected_match"（A 摘录
    /// 未过机械匹配，维持 open）| "error"（该条处理失败）
    pub outcome: String,
    /// 一句话说明（B 的 reason / 失败原因）
    #[serde(default)]
    pub detail: String,
}

/// A 自动答辩（§2.1.5 完整对抗语义——答辩的发起方是 A 建模者，不是人）：
///
/// 对本体全部 rules 来源 open warning，后端一次跑完
/// 「A 构造论据+反证 → dispute_issue（机械匹配把关）→ resolve_dispute_llm
/// （B 裁决三态）」。人只在阶段③ 仲裁 B 判不定的残留项。
///
/// - A 无证据反驳的 warning → accept（留给纠错/人工修订，不走裁决）
/// - A 摘录的 quote 没过机械匹配 → 该条维持 open（outcome=rejected_match）
///   ——不降级 A 的答辩资格，也不放水匹配，如实呈现
#[tauri::command]
#[specta::specta]
pub async fn run_auto_dispute(
    state: State<'_, AppState>,
    ontology_iri: String,
) -> AppResult<Vec<AutoDisputeItem>> {
    let store = get_ttl_store(&state)?;
    // A（建模者）通道——答辩是 A 的权利与义务
    let chat_a = {
        let c = state.chat.read().await;
        c.clone()
            .ok_or_else(|| AppError::Ontology("未配置建模者 provider，无法自动答辩".into()))?
    };
    // B（校验者）通道——裁决权在 B
    let chat_b = crate::commands::provider::verifier_chat(&state)
        .await
        .ok_or_else(|| AppError::Ontology("未配置校验者 provider，无法裁决".into()))?;
    let model_a = {
        let c = state.provider_config.read().await;
        c.as_ref().map(|c| c.model.clone())
            .ok_or_else(|| AppError::Ontology("未配置建模者 provider".into()))?
    };
    let model_b = {
        let v = state.verifier_config.read().await;
        v.as_ref().map(|c| c.model.clone()).unwrap_or_else(|| model_a.clone())
    };

    let warnings = store.list_disputable_warnings(&ontology_iri)?;
    if warnings.is_empty() {
        return Ok(Vec::new());
    }

    // 后台驱动（对齐 run_review_loop）：spawn 后立即返回，前端轮询
    // get_auto_dispute_result（含 running 标志）恢复进行中状态——切窗/切页不丢。
    // 每条处理完即写缓存（items 追加、running=true），全部完成置 running=false。
    {
        if let Ok(mut m) = auto_dispute_cache().lock() {
            m.insert(ontology_iri.clone(), AutoDisputeState { running: true, items: Vec::new() });
        }
    }
    let store2 = store.clone();
    let ont = ontology_iri.clone();
    tauri::async_runtime::spawn(async move {
        let run = async {
            // A 的决策输入：warning 清单（带建议）+ 证据源全文
            let warnings_json = serde_json::to_string_pretty(
                &warnings
                    .iter()
                    .map(|w| {
                        serde_json::json!({
                            "issue_id": w.id,
                            "rule": w.rule,
                            "message": w.message,
                            "target": w.target,
                            "suggestion": w.suggestion,
                        })
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| format!("serialize warnings: {e}"))?;
            let sources = store2.list_evidence_sources_for_dispute(&ont).map_err(|e| e.to_string())?;
            let evidence_context = if sources.is_empty() {
                "（无已登记证据源——没有证据的 warning 只能接受）".to_string()
            } else {
                sources
                    .iter()
                    .map(|(id, title, content)| format!("[{id}] {title}\n{content}"))
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n")
            };

            // 环节 1：A 逐条决策（dispute / accept）
            let a_out = agent_core::validation_llm::run_a_dispute(&chat_a, &model_a, &warnings_json, &evidence_context)
                .await
                .map_err(|e| e.to_string())?;
            let decisions: Vec<agent_core::validation_llm::ADisputeDecision> =
                agent_core::validation_llm::parse_a_dispute(&a_out)
                    .into_iter()
                    .filter(|d| warnings.iter().any(|w| w.id == d.issue_id))
                    .collect();

            let mut results = Vec::new();
            for w in &warnings {
                let Some(d) = decisions.iter().find(|d| d.issue_id == w.id) else {
                    // A 未回应（漏答/JSON 丢条）→ 按接受处理，落库 accepted
                    let _ = store2.accept_issue(&w.id);
                    results.push(AutoDisputeItem {
                        issue_id: w.id.clone(),
                        outcome: "accept".into(),
                        detail: "A 未回应，按接受处理".into(),
                    });
                    push_dispute_item(&ont, &results);
                    continue;
                };
                if d.action != "dispute" {
                    // A 接受告警 → 落库 accepted（此前只记结果不改状态，issue 维持 open 是 bug）
                    let _ = store2.accept_issue(&w.id);
                    results.push(AutoDisputeItem {
                        issue_id: w.id.clone(),
                        outcome: "accept".into(),
                        detail: "A 接受，按建议修订（不走裁决）".into(),
                    });
                    push_dispute_item(&ont, &results);
                    continue;
                }
                // 环节 2：提交答辩（机械匹配在此把关，失败即驳回不进裁决）
                let submit = store2.dispute_issue(&d.issue_id, &d.argument, &d.source_id, &d.quote);
                match submit {
                    Ok(s) if s == "disputed" => {}
                    Ok(reject) => {
                        // 机械匹配驳回 / 状态不符等——issue 维持 open，如实记录
                        results.push(AutoDisputeItem {
                            issue_id: d.issue_id.clone(),
                            outcome: "rejected_match".into(),
                            detail: reject,
                        });
                        push_dispute_item(&ont, &results);
                        continue;
                    }
                    Err(e) => {
                        results.push(AutoDisputeItem {
                            issue_id: d.issue_id.clone(),
                            outcome: "error".into(),
                            detail: e.to_string(),
                        });
                        push_dispute_item(&ont, &results);
                        continue;
                    }
                }
                // 环节 3：B 机器裁决（uncertain 保持 disputed 进仲裁）
                let (message, argument, quote) = store2
                    .get_dispute_context(&d.issue_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "答辩材料不完整".to_string())?;
                let verdict = agent_core::validation_llm::run_dispute_verdict(
                    &chat_b, &model_b, &message, &argument, &quote,
                )
                .await
                .map_err(|e| e.to_string())?;
                if verdict.contains("\"overruled\"") {
                    store2.resolve_dispute(&d.issue_id, "overruled", "verifier")
                        .map_err(|e| e.to_string())?;
                    results.push(AutoDisputeItem { issue_id: d.issue_id.clone(), outcome: "overruled".into(), detail: "B 认可——warning 撤销".into() });
                } else if verdict.contains("\"uncertain\"") {
                    results.push(AutoDisputeItem { issue_id: d.issue_id.clone(), outcome: "uncertain".into(), detail: "B 判不定——已进人工仲裁队列".into() });
                } else {
                    store2.resolve_dispute(&d.issue_id, "accepted", "verifier")
                        .map_err(|e| e.to_string())?;
                    results.push(AutoDisputeItem { issue_id: d.issue_id.clone(), outcome: "accepted".into(), detail: "B 驳回——升级 error".into() });
                }
                push_dispute_item(&ont, &results);
            }
            Ok::<(), String>(())
        };
        if let Err(e) = run.await {
            tracing::error!("run_auto_dispute {ont} 失败: {e}");
        }
        // 收尾：剩余 open 的 warning（rejected_match 摘录未匹配 / error 处理失败）
        // 视为告警成立——没有有效证据反驳的 warning 按 accepted 收敛，避免悬在 open
        // 死胡同（仲裁只收 disputed、纠错只收 error，open 的 warning 无出口）。
        // 语义与 accept 分支一致：A 拿不出过关证据 = 告警成立。跳过 disputed（答辩中）。
        if let Err(e) = store2.accept_open_warnings(&ont) {
            tracing::warn!("收尾 open warning 转 accepted 失败: {e}");
        }
        // 完成（或失败）：置 running=false，items 已逐条写入
        if let Ok(mut m) = auto_dispute_cache().lock() {
            if let Some(state) = m.get_mut(&ont) {
                state.running = false;
            }
        }
    });

    Ok(Vec::new())
}

/// 把当前 results 快照写回缓存（running 仍 true），供前端轮询实时回显逐条进度。
fn push_dispute_item(ontology_iri: &str, items: &[AutoDisputeItem]) {
    if let Ok(mut m) = auto_dispute_cache().lock() {
        if let Some(state) = m.get_mut(ontology_iri) {
            state.items = items.to_vec();
        }
    }
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
                msg: String::new(), kind: String::new(),
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
        let mut ok = false;
        for attempt in 1..=3 {
            match agent_core::validation_llm::run_a_revise(
                &chat_a, &model_a, &ttl, &b_suggestions,
            )
            .await
            {
                Ok(revised) => match store2.import_ttl(&revised.0, true) {
                    Ok(_) => {
                        ok = true;
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
            }
        }
    });
    Ok(())
}

