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
2. 每条建议必须带 evidence_ref（引用已提供的证据源编号）——不带证据的批评不输出
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
pub const B_RECHECK_PREAMBLE: &str = r#"你是本体质量审计专家（校验者 Verifier），正在进行多轮对抗的复核轮。上一轮你提出了 N 条建议，建模者已按建议修订。现在给你修订后的 Turtle 全文，请核查：
1. 每条建议是否被落实（修订后产物中对应位置是否已改进）
2. 修订是否引入新问题

输出严格 JSON（不要输出其他内容）：
{ "verdict": "confirm" | "question", "remaining_issues": [ { "suggestion_id": "...", "status": "resolved|unresolved|regressed", "message": "..." } ] }
- verdict=confirm：全部落实且无新问题 → 通过
- verdict=question：仍有分歧 → 转人工仲裁"#;

/// A 修订系统提示词（建模者视角，与对话窗口同方法论但独立上下文）。
pub const A_REVISE_PREAMBLE: &str = r#"你是本体建模者（Generator）。校验者对你产出的 Turtle 提出了问题清单。请逐条处理：
1. 合理的问题 → 修订 Turtle 修正
2. 你认为误判的问题 → 保持原样，在修订说明中注明拒绝理由（引用输入证据）

**硬约束：修订后的 Turtle 只能是纯本体内容**——不要把问题描述、校验建议、
"仲裁""B1""必须改"等任何校验过程的文字/标签/编号写进 Turtle。校验文字只
出现在第二段修订说明里，本体里只能有 OWL/RDF 结构化声明。

输出格式：
第一段：修订后的完整 Turtle 全文
第二段（用 --- 分隔）：修订说明 JSON [ { "issue_id": "...", "action": "accepted|rejected", "reason": "..." } ]
- issue_id 必须原样回显校验者建议清单中每条建议的 id（如 "B1"），不可自造编号
- rejected 的条目必须写明引用输入证据的拒绝理由"#;

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
/// `ttl_v1`: 当前 Turtle；`b_suggestions`: B 的建议清单（上一轮输出）。
/// 返回 (修订后 Turtle, 修订说明)。
pub async fn run_a_revise(
    chat: &ChatService,
    model_a: &str,
    ttl_v1: &str,
    b_suggestions: &str,
) -> Result<(String, String), String> {
    let prompt = format!(
        "当前 Turtle：\n```turtle\n{ttl_v1}\n```\n\n校验者建议：\n{b_suggestions}\n\n请按约束输出修订结果。"
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
/// 只传修订后的 Turtle（a_revision），**不传修订说明**（a_reasoning 对 B 不可见）。
/// 返回复核 JSON 文本。
pub async fn run_b_recheck(
    chat: &ChatService,
    model_b: &str,
    a_revision: &str,
) -> Result<String, String> {
    let prompt = format!(
        "修订后的 Turtle 全文：\n```turtle\n{a_revision}\n```\n\n请核查建议落实情况并输出 JSON 判定。"
    );
    chat.run_llm_prompt(model_b, B_RECHECK_PREAMBLE, &prompt).await.map_err(|e| e.to_string())
}

/// A 答辩构造系统提示词（§2.1.5 答辩的发起方是 A——建模者对规则 warning 逐条反驳）。
///
/// 输入：warning issue + 证据源清单（id + 标题 + 内容摘录）。A 从中选源并
/// **逐字摘录**反证原文（提交时过 match_quote 机械匹配，摘错即驳回）；
/// 无可用证据时输出 skip，不硬编。
pub const A_DISPUTE_PREAMBLE: &str = r#"你是本体建模者（Generator）。规则检查引擎对你建模的 Turtle 提出了 warning，请逐条决定：接受（按建议修订）还是答辩（引用输入证据反驳）。

给你：
1. 待处理的 warning 列表（规则引擎产出）
2. 已登记的证据源清单（编号 + 标题 + 内容全文）

决策规则：
- warning 确有道理或你没有证据反驳 → 接受（后续按建议修订）
- 有输入证据（原始材料/宪章/术语）能证明当前建模正确 → 答辩：
  ① 选一个证据源编号
  ② 从该源内容中**逐字摘录**支持你建模的原文片段（≤200 字符，不得改写/拼接/概括——系统会做字面机械匹配，匹配不上直接驳回）
  ③ 写明辩护理由：为什么这段证据证明当前建模是对的（引用证据说明，不空谈）
- 没有证据支撑的反驳不要发起——编造/改写引文会被机械匹配当场驳回

输出严格 JSON 数组（不要输出其他内容）：
[ { "issue_id": "...", "action": "dispute" | "accept", "source_id": "...", "quote": "...", "argument": "..." } ]
- action=dispute 时 source_id/quote/argument 必填；action=accept 时只需 issue_id"#;

/// A 答辩输出（单条 warning 的决策）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct ADisputeDecision {
    /// 目标 warning 的 issue id
    pub issue_id: String,
    /// "dispute"（发起答辩）| "accept"（接受，按建议修订）
    pub action: String,
    /// 反证证据源 id（action=dispute 必填）
    #[serde(default)]
    pub source_id: String,
    /// 反证原文逐字摘录（action=dispute 必填；机械匹配对象）
    #[serde(default)]
    pub quote: String,
    /// 辩护理由（action=dispute 必填）
    #[serde(default)]
    pub argument: String,
}

/// 运行 A 答辩构造：对 warning 清单逐条决策（dispute/accept），返回原始 JSON。
pub async fn run_a_dispute(
    chat: &ChatService,
    model_a: &str,
    warnings_json: &str,
    evidence_context: &str,
) -> Result<String, String> {
    let prompt = format!(
        "【待处理 warning 列表】\n{warnings_json}\n\n【已登记证据源清单】\n{evidence_context}\n\n请输出决策 JSON 数组。"
    );
    chat.run_llm_prompt(model_a, A_DISPUTE_PREAMBLE, &prompt)
        .await
        .map_err(|e| e.to_string())
}

/// 解析 A 答辩输出（容错：剥 `---` 分隔残留与 ```json 围栏，取首 `[` 末 `]`；
/// 实测 LLM 会带双分隔符前缀，严格 from_str 全军覆没——同 adversarial.rs 的坑）。
/// 解析失败返回空清单（调用方按「A 未回应」逐条 accept 保守处理）。
pub fn parse_a_dispute(raw: &str) -> Vec<ADisputeDecision> {
    let t = raw.trim().trim_start_matches('-').trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t)
        .trim();
    let Some(start) = t.find('[') else {
        return Vec::new();
    };
    let Some(end) = t.rfind(']') else {
        return Vec::new();
    };
    if start >= end {
        return Vec::new();
    }
    serde_json::from_str(&t[start..=end]).unwrap_or_default()
}
/// B 答辩裁决系统提示词（§2.1.5 答辩的机器裁决——B 判不定才升级人工仲裁）。
pub const DISPUTE_VERDICT_PREAMBLE: &str = r#"你是本体质量审计专家（校验者 Verifier），正在对建模者（A）就一条校验 warning 提出的答辩做裁决。给你：
1. 被答辩的 issue（规则引擎产出的问题描述）
2. A 的辩护理由（argument）
3. A 引用的反驳证据原文（counter_evidence，已通过机械匹配）

你的职责：基于证据与业务合理性独立判定——不是附和 A，也不是维护规则。
输出严格 JSON（不要输出其他内容）：
{ "verdict": "overruled" | "accepted" | "uncertain", "reason": "一句话理由" }
- verdict=overruled：A 的证据与理由成立，撤销该 warning
- verdict=accepted：A 的辩护不成立，维持原判（升级 error）
- verdict=uncertain：证据与理由不足以判定 → 升级人工仲裁（不要强行二选一）"#;

/// B 答辩裁决：对一条 disputed 的 issue 做机器裁决，返回原始输出（含 JSON 判定）。
pub async fn run_dispute_verdict(
    chat: &ChatService,
    model_b: &str,
    issue_message: &str,
    argument: &str,
    counter_evidence: &str,
) -> Result<String, String> {
    let prompt = format!(
        "被答辩的 issue：\n{issue_message}\n\nA 的辩护理由：\n{argument}\n\nA 引用的反驳证据原文：\n{counter_evidence}\n\n请输出裁决 JSON。"
    );
    chat.run_llm_prompt(model_b, DISPUTE_VERDICT_PREAMBLE, &prompt)
        .await
        .map_err(|e| e.to_string())
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
    fn parse_a_dispute_plain_json() {
        let raw = r#"[{"issue_id":"vi-1","action":"dispute","source_id":"src-a","quote":"供应商编号全局唯一","argument":"输入明示"}]"#;
        let out = parse_a_dispute(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].action, "dispute");
        assert_eq!(out[0].source_id, "src-a");
    }

    #[test]
    fn parse_a_dispute_tolerates_fence_and_separator() {
        // 实测 LLM 会带 "---\n\n```json" 前缀（同 adversarial.rs 的坑）
        let raw = "---\n\n```json\n[{\"issue_id\":\"vi-1\",\"action\":\"accept\"}]\n```";
        let out = parse_a_dispute(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].action, "accept");
        assert_eq!(out[0].source_id, "", "accept 分支证据字段为默认空");
    }

    #[test]
    fn parse_a_dispute_garbage_returns_empty() {
        assert!(parse_a_dispute("抱歉我不能输出 JSON").is_empty());
        assert!(parse_a_dispute("").is_empty());
    }

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
}
