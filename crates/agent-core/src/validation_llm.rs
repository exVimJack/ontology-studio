//! L4.3 语义评审的 LLM 编排管道（VALIDATION-DESIGN.md §4.3 / §4.4）。
//!
//! 编排驱动 + 多 LLM 路由（非双 agent 进程）：
//!   - B 评审：`run_b_review` —— 独立审计视角（不含建模方法论）+ evidence 预注入
//!   - A 修订：`run_a_revise` —— 建模方法论 prompt + B 建议清单
//!   - B 复核：`run_b_recheck` —— 只看修订输出（a_revision），不看修订说明
//!
//! 模型独立性（§4.4 硬约束 + §4.4.1 降级）：
//!   - `independence_tier(provider_a, model_a, provider_b, model_b)` 判定档位
//!   - 完整（跨供应商）/ 降级（同供应商跨家族）/ 兜底（同模型）

use serde::{Deserialize, Serialize};

use crate::chat::ChatService;

/// 模型独立性档位（§4.4.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum IndependenceTier {
    /// ≥2 供应商，B 跨家族模型——对抗有效性高
    Full,
    /// 单供应商多模型，B 同供应商不同家族——对抗有效性中
    Degraded,
    /// 单 provider 单模型，B 同模型+独立 prompt——对抗有效性低
    Fallback,
}

impl IndependenceTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            IndependenceTier::Full => "full",
            IndependenceTier::Degraded => "degraded",
            IndependenceTier::Fallback => "fallback",
        }
    }

    /// 报告标注文本。
    pub fn label(&self) -> &'static str {
        match self {
            IndependenceTier::Full => "完整 · 双模型",
            IndependenceTier::Degraded => "⚠ 降级-同供应商",
            IndependenceTier::Fallback => "⚠ 降级-同模型",
        }
    }
}

/// 判定 A/B 模型独立性档位。
///
/// - provider 不同 → Full
/// - provider 相同、model 家族不同（如 deepseek-* vs glm-*，经同一网关接入）
///   → Full（不同厂商模型，对抗有效；网关只是接入通道，不是厂商）
/// - provider 相同、model 家族相同、model 不同 → Degraded（同供应商同家族不同型号）
/// - provider 相同、model 相同 → Fallback
pub fn independence_tier(
    provider_a: &str,
    model_a: &str,
    provider_b: &str,
    model_b: &str,
) -> IndependenceTier {
    if provider_a != provider_b {
        IndependenceTier::Full
    } else if model_family(model_a) != model_family(model_b) {
        // 同一接入通道（如火山网关）跑不同厂商模型 → 跨家族，对抗有效
        IndependenceTier::Full
    } else if model_a != model_b {
        IndependenceTier::Degraded
    } else {
        IndependenceTier::Fallback
    }
}

/// 从模型名提取厂商家族前缀（deepseek-* / glm-* / qwen-* / doubao-* …）。
/// 无前缀或无法识别 → 取首个连字符/点前段（弱家族判断）。
fn model_family(model: &str) -> String {
    let m = model.trim().to_ascii_lowercase();
    let known = [
        "deepseek", "glm", "qwen", "doubao", "gpt", "claude", "gemini", "grok",
        "llama", "mistral", "kimi", "moonshot", "o1", "o3", "ernie", "minimax",
        "hunyuan", "spark", "yi", "abab",
    ];
    for k in known {
        if m.starts_with(k) {
            return k.to_string();
        }
    }
    // 如 "my-company-1" vs "other-model"
    m.split(['-', '.', '/']).next().unwrap_or("").to_string()
}

/// B 评审输出（结构化建议清单，B prompt 规格 §4.3）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct BReviewOutput {
    /// 建议清单（原始 JSON 字符串；前端展示/回喂）
    pub suggestions_json: String,
    /// 是否成功生成
    pub ok: bool,
    /// 错误信息（ok=false 时）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// B 评审系统提示词（独立审计视角，不含建模方法论——B 审 A 而非审自己）。
pub const B_REVIEW_PREAMBLE: &str = r#"你是本体质量审计专家（校验者 Verifier）。你的职责是独立审查 AI 建模者产出的 W3C Turtle 本体，找出语义问题并给出带证据的改进建议。

约束：
1. 你不参与建模，不套用建模方法论；你只审查产物质量
2. 每条建议必须带 evidence_ref，且必须指向本次输入里真实存在的内容：优先引用【证据上下文】中的证据源编号；本轮未提供证据源时，改为引用设计宪章条目或【29 句话与覆盖矩阵】的句号（如 `charter`、`2.3`）。不得编造引用，不带证据的批评不输出
3. 优先找反例（Prefer Counterexamples）："这个类在真实数据里没有实例""这个属性映射的列 90% 为空"
4. 只评审不修改本体
5. 同概念合并不重复

【审计维度】（对齐设计文档八维度，逐条过）
① 事实忠实度：每个断言是否有企业证据支持？优先找反例——这个类在真实数据里有没有实例？这个属性映射的列是不是 90% 为空？
② 完整性：29 句话每条是否都有对应产出？缺了哪个对象/规则？
③ 语义正确性：概念定义是否准确？边界是否清晰？有无概念混淆？
④ 结构合理性：层级/关系/属性组织是否合理？类该拆该并？
⑤ 逻辑一致性：是否存在冲突？能否通过推理验证？
⑥ 约束正确性：基数/唯一性/枚举是否合理？
⑦ 数据可落地性：能否映射真实数据？（联动 L2.5 数据源比对结果）
⑧ 业务可用性：能否支撑核心业务问题？（联动 L2.6 CQ 反测结果）

输出严格 JSON 数组（不要输出其他内容）：
[{ "id": "B1", "iri": "...", "dimension": "事实忠实度|完整性|语义正确性|结构合理性|逻辑一致性|约束正确性|数据可落地性|业务可用性", "type": "missing|conflict|granularity|semantic|grounding", "severity": "error|warning|suggestion", "message": "...", "evidence_ref": "...", "suggestion": "..." }]
- id：建议编号（B1、B2…），修订说明需按 id 逐条回应，必须填写"#;

/// B 复核系统提示词（只看修订输出，不看修订说明——产物对产物博弈）。
///
/// 路线 B（2026-11-03 裁定）：B 只判"产物是否解决了问题"，**不裁决建模者的异议**。
/// 异议一律保持 disputed 直通阶段③人工仲裁。此前 prompt 要求 B 产出 dispute_rulings，
/// 却从未把异议清单（甚至 B 自己的上一轮建议清单）交给它——判不了落实也裁不了申诉。
pub const B_RECHECK_PREAMBLE: &str = r#"你是本体质量审计专家（校验者 Verifier），正在进行多轮对抗的复核轮。你会收到两部分输入：你上一轮提出的建议清单（核查基准），以及建模者据此提交的修订后 Turtle 全文。请逐条核查：
1. 每条建议是否被落实：必须在修订后的产物里找到对应的结构化声明才算 resolved；"看起来改过了"、只在措辞上呼应、或产物里找不到对应声明，判 unresolved
2. 修订是否引入新问题：原来正确的内容被改坏，判 regressed
3. 建模者对某些条目的拒绝理由**不在**你的输入范围内——这是刻意隔离，防止你被其论证同化。你只按产物判断：产物里没改就是没改，不要因为"对方给了理由"而松动

输出严格 JSON（不要输出其他内容）：
{ "verdict": "confirm" | "question", "remaining_issues": [ { "suggestion_id": "...", "status": "resolved|unresolved|regressed", "message": "...", "location": "修订后产物中的对应位置（iri 或声明片段）" } ] }
- verdict=confirm：核查基准里每条建议都是 resolved 且无 regressed → 通过
- verdict=question：仍有 unresolved 或 regressed → 继续对抗（两轮封顶后转人工仲裁）
- 只要有一条 unresolved 或 regressed 就 MUST NOT 判 confirm；"大部分改了"不是 confirm 的理由
- suggestion_id 必须来自核查基准里的编号，不可自造"#;

/// A 修订系统提示词（建模者视角，与对话窗口同方法论但独立上下文）。
/// 合并方案 v0.8：A 对统一发现池逐条表态——B 评审建议（B 编号）与规则检查
/// 告警（R 编号）同清单处理；规则告警的异议必须带证据引用（机械匹配把关）。
pub const A_REVISE_PREAMBLE: &str = r#"你是本体建模者（Generator）。校验者对本体产出了两类问题：语义评审建议（B 编号）与规则检查告警（R 编号）。请逐条处理：
1. 合理的问题 → 修订 Turtle 修正（action=accepted）
2. 你认为误判的问题 → 保持原样并拒绝（action=rejected，异议）。规则告警（R 编号）的异议必须引用输入证据：
   ① 选一个已登记证据源编号（输入清单给出）
   ② 从该源内容中逐字摘录支持你建模的原文片段（≤200 字符，不得改写/拼接/概括——系统会做字面机械匹配，匹配不上按接受处理）
   ③ 写明辩护理由：为什么这段证据证明当前建模是对的（引用证据说明，不空谈）
   没有证据支撑的异议不要发起——编造/改写引文会被机械匹配驳回

**硬约束：修订后的 Turtle 只能是纯本体内容**——不要把问题描述、校验建议、
"仲裁""B1""必须改"等任何校验过程的文字/标签/编号写进 Turtle。校验文字只
出现在第二段修订说明里，本体里只能有 OWL/RDF 结构化声明。

输出格式：
第一段：修订后的完整 Turtle 全文
第二段（用 --- 分隔）：修订说明 JSON [ { "issue_id": "...", "action": "accepted|rejected", "reason": "...", "source_id": "...", "quote": "...", "argument": "..." } ]
- issue_id 必须原样回显问题清单中每条条目的编号（B 编号如 "B1"，或 R 编号如 "R1"），不可自造编号
- action=rejected 时必须写明引用输入证据的拒绝理由；R 编号条目 rejected 时 source_id/quote/argument 必填"#;

/// 运行 B 语义评审（第三层对抗第 1 步）。
///
/// `inputs`: charter + 29句话 + Turtle + L0-L3 报告 + evidence 上下文（预注入）。
/// 返回 B 的建议 JSON 字符串。
pub async fn run_b_review(
    chat: &ChatService,
    model_b: &str,
    inputs: &str,
) -> Result<BReviewOutput, String> {
    match chat
        .run_llm_prompt(model_b, B_REVIEW_PREAMBLE, inputs)
        .await
    {
        Ok(s) => Ok(BReviewOutput {
            suggestions_json: s,
            ok: true,
            error: None,
        }),
        Err(e) => Ok(BReviewOutput {
            suggestions_json: String::new(),
            ok: false,
            error: Some(e.to_string()),
        }),
    }
}

/// 运行 A 修订（第三层对抗第 2 步）。
///
/// `ttl_v1`: 当前 Turtle；`b_suggestions`: B 的建议清单（上一轮输出）；
/// `rules_warnings`: 规则检查的 warning/suggestion 清单（R 编号，供 A 统一表态）。
/// 返回 (修订后 Turtle, 修订说明)。
pub async fn run_a_revise(
    chat: &ChatService,
    model_a: &str,
    ttl_v1: &str,
    b_suggestions: &str,
    rules_warnings: &str,
) -> Result<(String, String), String> {
    let prompt = format!(
        "当前 Turtle：\n```turtle\n{ttl_v1}\n```\n\n校验者建议（B 编号）：\n{b_suggestions}\n\n规则检查告警（R 编号）：\n{rules_warnings}\n\n请按约束输出修订结果。"
    );
    match chat.run_llm_prompt(model_a, A_REVISE_PREAMBLE, &prompt).await {
        Ok(out) => {
            // 拆两段：修订 Turtle 全文 与 修订说明（--- 分隔）
            if let Some(idx) = out.find("\n---\n") {
                let (ttl, notes) = out.split_at(idx);
                Ok((strip_code_fence(ttl.trim()).to_string(), notes.trim_start_matches("---\n").trim().to_string()))
            } else {
                // 未按格式分隔：整体当 Turtle，说明为空
                Ok((strip_code_fence(out.trim()).to_string(), String::new()))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// 剥离 LLM 输出里常见的 markdown 代码围栏（```turtle ... ``` / ``` ... ```）。
/// A 修订经常把整本 Turtle 包在围栏里输出，不剥离会导致后续 parse/import 失败。
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```turtle")
        .or_else(|| t.strip_prefix("```ttl"))
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim()
}

/// 运行 B 复核（第三层对抗第 3 步）。
///
/// 输入两样：`baseline_suggestions`（B 自己上一轮的建议清单，核查基准）与
/// `a_revision`（修订后产物）。**不传修订说明**（`a_reasoning` 对 B 不可见）——
/// 独立性约束的是"不看答题者的解题思路"，不是"让裁判失忆"：没有核查基准，
/// B 无从判断"每条建议是否被落实"，只能把产物重新审一遍并倾向判 question。
pub async fn run_b_recheck(
    chat: &ChatService,
    model_b: &str,
    a_revision: &str,
    baseline_suggestions: &str,
) -> Result<String, String> {
    let n = count_baseline_items(baseline_suggestions);
    let prompt = format!(
        "【核查基准：你上一轮提出的建议清单】\n{baseline_suggestions}\n\n【修订后的 Turtle 全文】\n```turtle\n{a_revision}\n```\n\n上一轮你提出了 {n} 条建议，建模者已提交修订稿。请逐条核查落实情况并输出 JSON 判定；建模者的修订说明与辩护理由不在你的输入范围内，只按产物判断。"
    );
    chat.run_llm_prompt(model_b, B_RECHECK_PREAMBLE, &prompt).await.map_err(|e| e.to_string())
}

/// 核查基准条目数：把"上一轮你提出了 N 条建议"的 N 真正填进去。
///
/// 首轮基准是 B 评审输出的 JSON 数组；后续轮是上一轮复核输出（对象，含
/// `remaining_issues`）。两者都数不出来时按非空行数兜底——数字只用于提示词
/// 可读性，数错不影响判定，但**绝不能把占位符原样送进模型**。
pub fn count_baseline_items(baseline: &str) -> usize {
    let t = baseline.trim();
    if t.is_empty() {
        return 0;
    }
    if let Some(body) = json_object_body(t) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(arr) = v.get("remaining_issues").and_then(|x| x.as_array()) {
                return arr.len();
            }
        }
    }
    if let Some(arr) = json_array_body(t) {
        if let Ok(v) = serde_json::from_str::<Vec<serde_json::Value>>(arr) {
            return v.len();
        }
    }
    t.lines().filter(|line| !line.trim().is_empty()).count()
}

/// 解析 B 复核输出是否判"确认收敛"。
///
/// **仅** `verdict` 字段归一化后等于 `confirm` 才算收敛；`question`、字段缺失、
/// 整体解析失败、未知取值、空串一律按未收敛。误判方向刻意不对称：宁可多走一轮
/// 复核或多进一次人工仲裁，也不在判定不可靠时把 A 的修订稿整本写回工作区
/// （confirm 分支会 `import_ttl(overwrite=true)`）。
///
/// 此前判定是 `!verdict.contains("\"verdict\": \"question\"")`——要求冒号后恰好
/// 一个空格，模型输出紧凑 JSON `"verdict":"question"`（最常见形态）会被误判成
/// confirm，方向恰好反了。
pub fn verdict_is_confirm(b_verdict_json: &str) -> bool {
    let Some(body) = json_object_body(b_verdict_json) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    v.get("verdict")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().eq_ignore_ascii_case("confirm"))
        .unwrap_or(false)
}

/// 容错提取 JSON 对象文本：剥 ```json 围栏，取首个 `{` 到末个 `}`。
fn json_object_body(s: &str) -> Option<&str> {
    let t = strip_fence(s);
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    (start < end).then(|| &t[start..=end])
}

/// 容错提取 JSON 数组文本：剥围栏后取首个 `[` 到末个 `]`。
fn json_array_body(s: &str) -> Option<&str> {
    let t = strip_fence(s);
    let start = t.find('[')?;
    let end = t.rfind(']')?;
    (start < end).then(|| &t[start..=end])
}

fn strip_fence(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.trim()
}

/// 构造 B 评审输入上下文（evidence 预注入——B 不做自主检索，由编排统一加载）。
pub fn build_b_review_inputs(
    charter: &str,
    sentences: &str,
    ttl: &str,
    l03_report: &str,
    evidence_context: &str,
) -> String {
    format!(
        "【设计宪章】\n{charter}\n\n【29 句话输入】\n{sentences}\n\n【待评审 Turtle】\n```turtle\n{ttl}\n```\n\n【L0-L3 校验报告】\n{l03_report}\n\n【证据上下文】\n{evidence_context}\n\n请输出评审建议 JSON 数组。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_code_fence_removes_turtle_fence() {
        let s = "```turtle\n@prefix : <http://e/> .\n:Car a owl:Class .\n```";
        assert_eq!(strip_code_fence(s), "@prefix : <http://e/> .\n:Car a owl:Class .");
    }

    #[test]
    fn strip_code_fence_removes_plain_fence() {
        let s = "```\n:Car a owl:Class .\n```";
        assert_eq!(strip_code_fence(s), ":Car a owl:Class .");
    }

    #[test]
    fn strip_code_fence_noop_without_fence() {
        let s = "@prefix : <http://e/> .\n:Car a owl:Class .";
        assert_eq!(strip_code_fence(s), s);
    }

    #[test]
    fn independence_full_when_providers_differ() {
        assert_eq!(
            independence_tier("volc", "glm-5.3-flash", "openrouter", "deepseek-r1"),
            IndependenceTier::Full
        );
    }

    #[test]
    fn independence_full_same_provider_cross_family() {
        // 同一网关（volc）接入不同厂商模型 → 跨家族，对抗有效
        assert_eq!(
            independence_tier("volc", "deepseek-v4-flash", "volc", "glm-5.3-flash"),
            IndependenceTier::Full
        );
    }

    #[test]
    fn independence_degraded_same_provider_same_family_diff_model() {
        assert_eq!(
            independence_tier("volc", "glm-5.3-flash", "volc", "glm-5.2"),
            IndependenceTier::Degraded
        );
    }

    #[test]
    fn independence_fallback_same_model() {
        assert_eq!(
            independence_tier("volc", "glm-5.3-flash", "volc", "glm-5.3-flash"),
            IndependenceTier::Fallback
        );
    }

    #[test]
    fn tier_labels() {
        assert_eq!(IndependenceTier::Full.label(), "完整 · 双模型");
        assert_eq!(IndependenceTier::Degraded.label(), "⚠ 降级-同供应商");
        assert_eq!(IndependenceTier::Fallback.label(), "⚠ 降级-同模型");
    }

    #[test]
    fn build_inputs_includes_all_sections() {
        let s = build_b_review_inputs("charter", "sentences", "ttl", "report", "evidence");
        assert!(s.contains("【设计宪章】") && s.contains("【29 句话输入】"));
        assert!(s.contains("【待评审 Turtle】") && s.contains("【L0-L3 校验报告】"));
        assert!(s.contains("【证据上下文】"));
    }

    // ── 收敛判定：仅显式 verdict=confirm 算收敛，其余一律未收敛 ──
    #[test]
    fn verdict_compact_question_is_not_confirm() {
        // 修复前用 contains("\"verdict\": \"question\"")，这条紧凑 JSON 会被误判成 confirm
        assert!(!verdict_is_confirm(r#"{"verdict":"question","remaining_issues":[]}"#));
    }

    #[test]
    fn verdict_spaced_and_fenced_variants() {
        assert!(verdict_is_confirm(r#"{"verdict": "confirm", "remaining_issues": []}"#));
        assert!(verdict_is_confirm("```json\n{\"verdict\":\"confirm\"}\n```"));
        assert!(verdict_is_confirm("判定如下：\n{\"verdict\":\"CONFIRM\"}\n"));
        assert!(!verdict_is_confirm("```\n{\"verdict\":\"question\"}\n```"));
    }

    #[test]
    fn verdict_fails_closed_on_unparseable_or_missing_field() {
        assert!(!verdict_is_confirm(""));
        assert!(!verdict_is_confirm("复核通过，没有问题"));
        assert!(!verdict_is_confirm(r#"{"remaining_issues":[]}"#));
        assert!(!verdict_is_confirm(r#"{"verdict":null}"#));
        assert!(!verdict_is_confirm(r#"{"verdict":"confirmed"}"#)); // 未知取值
        assert!(!verdict_is_confirm(r#"{"verdict":true}"#)); // 非字符串
    }

    // ── 核查基准条目数：绝不能把占位符送进模型 ──
    #[test]
    fn count_baseline_from_b_report_array() {
        let arr = r#"[{"id":"B1"},{"id":"B2"},{"id":"B3"}]"#;
        assert_eq!(count_baseline_items(arr), 3);
        assert_eq!(count_baseline_items("```json\n[{\"id\":\"B1\"}]\n```"), 1);
    }

    #[test]
    fn count_baseline_from_recheck_remaining_issues() {
        let verdict = r#"{"verdict":"question","remaining_issues":[{"suggestion_id":"B1"},{"suggestion_id":"B2"}]}"#;
        assert_eq!(count_baseline_items(verdict), 2);
    }

    #[test]
    fn count_baseline_falls_back_to_lines_and_empty() {
        assert_eq!(count_baseline_items(""), 0);
        assert_eq!(count_baseline_items("- 条目一\n- 条目二\n"), 2);
    }
}
