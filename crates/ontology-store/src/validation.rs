//! 校验结果结构化类型（VALIDATION-DESIGN.md v0.3.1）。
//!
//! 职责：把 `TtlValidation` 从纯 `Vec<String>` 升级为结构化 `ValidationIssue[]`，
//! 支撑三层对抗（纠错循环 / 规则答辩 / 语义多轮复核）与 import 校验门禁。
//!
//! 向后兼容：`TtlValidation` 保留 `errors`/`warnings` 字段，新增 `issues` 结构化
//! 数组（旧消费者不受影响，新消费者用 issues）。
//!
//! 表结构（复用 TtlStore 同 SQLite 库，见 schema.rs）：
//!   - `validation_issues`：issue 对抗生命周期（open→disputed→accepted/overruled）
//!   - `validation_reviews`：L4.3 多轮状态机（round/max_rounds/b_report/a_revision…）

use serde::{Deserialize, Serialize};

/// 校验 issue 严重级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    /// 阻断性：非空禁止 import（L0-L2 error）
    Error,
    /// 非阻断：可答辩（L3 warning）
    Warning,
    /// 建议级：进入 L4.3 语义评审（B 提建议，不阻断）
    Suggestion,
}

impl IssueSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueSeverity::Error => "error",
            IssueSeverity::Warning => "warning",
            IssueSeverity::Suggestion => "suggestion",
        }
    }

    pub fn from_str(s: &str) -> IssueSeverity {
        match s {
            "error" => IssueSeverity::Error,
            "warning" => IssueSeverity::Warning,
            _ => IssueSeverity::Suggestion,
        }
    }
}

/// issue 对抗生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum IssueStatus {
    /// 初始：待处理（error 直接修订；warning 可答辩）
    #[default]
    Open,
    /// A 已发起答辩，等待裁决（warning 专属）
    Disputed,
    /// 已接受修订 / 答辩被驳回升级 error
    Accepted,
    /// 答辩成功，issue 撤销（warning 专属）
    Overruled,
}

impl IssueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueStatus::Open => "open",
            IssueStatus::Disputed => "disputed",
            IssueStatus::Accepted => "accepted",
            IssueStatus::Overruled => "overruled",
        }
    }

    pub fn from_str(s: &str) -> IssueStatus {
        match s {
            "disputed" => IssueStatus::Disputed,
            "accepted" => IssueStatus::Accepted,
            "overruled" => IssueStatus::Overruled,
            _ => IssueStatus::Open,
        }
    }
}

/// 原则出处引用（ValidationIssue.principle）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct PrincipleRef {
    /// 出处来源："skill" | "charter" | "input" | "w3c"
    #[serde(default)]
    pub source: String,
    /// 出处定位（skill 章节 / charter 字段 / 输入文档段落 / W3C 标准条款）
    #[serde(default)]
    pub ref_: String,
}

/// 证据引用（对应 evidence_sources 表；source_id 必须已登记）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct ValidationEvidenceRef {
    /// FK → evidence_sources.source_id（register_source 返回）
    pub source_id: String,
    /// 原文逐字摘录（≤200 字符；答辩 counterEvidence 机械匹配用）
    pub quote: String,
}

/// 答辩记录（IssueStatus::Disputed 时非空）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct DisputeRecord {
    /// A 的辩护理由（argument，由 B 复核判定可信度）
    pub argument: String,
    /// 反驳证据：必须引用已登记 evidence_source + 逐字原文（机械匹配）
    pub counter_evidence: ValidationEvidenceRef,
    /// 裁决者："verifier" | "human"
    #[serde(default)]
    pub resolved_by: String,
    /// 裁决结果："overruled"（撤销） | "accepted"（驳回，升级 error）
    #[serde(default)]
    pub verdict: String,
}

/// issue 默认来源（serde default）：阶段① 规则检查。
fn default_issue_source() -> String {
    "rules".to_string()
}

/// 单条结构化校验 issue。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ValidationIssue {
    /// 稳定 ID（如 "L3-3"，本体内唯一，持久化主键；
    /// review 来源为 "{review_id}:{B条目id}"，如 "vr-xxx:B1"）
    pub id: String,
    /// 层级："L0" | "L1" | "L2" | "L2.5" | "L2.6" | "L3" | "L4"
    pub layer: String,
    /// 发现来源："rules"（阶段① 规则检查）| "review"（②a B 语义评审）
    /// ——统一发现池（v0.7）：两类发现并列同一生命周期，互不清除
    #[serde(default = "default_issue_source")]
    pub source: String,
    /// 严重级
    pub severity: IssueSeverity,
    /// 规则 ID（check_* 函数名，如 "check_class_uses_owl_class"；
    /// review 来源为 "review:{dimension}"，如 "review:完整性"）
    pub rule: String,
    /// 人类可读描述
    pub message: String,
    /// 定位目标（实体 IRI / Turtle 行号，可空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 证据引用（可空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ValidationEvidenceRef>,
    /// 原则出处（可空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principle: Option<PrincipleRef>,
    /// 修复建议（可空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// 对抗生命周期状态
    #[serde(default)]
    pub status: IssueStatus,
    /// 答辩记录（disputed 时非空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispute: Option<DisputeRecord>,
    /// A 的裁决回应（②a review 来源专属）：B 建议落成 issue 后，A 修订时
    /// 逐条回填 accepted/rejected + reason（B 复核不可见，仲裁回溯用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_action: Option<AActionRecord>,
}

/// A 对 review 来源 issue 的逐条回应（统一发现池 v0.7）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct AActionRecord {
    /// "accepted" | "rejected"
    pub action: String,
    /// A 的理由（rejected 时 = 拒绝依据；B 复核不可见）
    pub reason: String,
}

impl ValidationIssue {
    /// 构造 error 级 issue（简化构造器，S2 规则用）。
    pub fn error(id: &str, layer: &str, rule: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            layer: layer.to_string(),
            source: default_issue_source(),
            severity: IssueSeverity::Error,
            rule: rule.to_string(),
            message: message.into(),
            target: None,
            evidence: None,
            principle: None,
            suggestion: None,
            status: IssueStatus::Open,
            dispute: None,
            a_action: None,
        }
    }

    /// 构造 warning 级 issue。
    pub fn warning(id: &str, layer: &str, rule: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            layer: layer.to_string(),
            source: default_issue_source(),
            severity: IssueSeverity::Warning,
            rule: rule.to_string(),
            message: message.into(),
            target: None,
            evidence: None,
            principle: None,
            suggestion: None,
            status: IssueStatus::Open,
            dispute: None,
            a_action: None,
        }
    }

    /// 构造 suggestion 级 issue。
    pub fn suggestion(id: &str, layer: &str, rule: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            layer: layer.to_string(),
            source: default_issue_source(),
            severity: IssueSeverity::Suggestion,
            rule: rule.to_string(),
            message: message.into(),
            target: None,
            evidence: None,
            principle: None,
            suggestion: None,
            status: IssueStatus::Open,
            dispute: None,
            a_action: None,
        }
    }
}

/// L4.3 多轮语义评审状态（validation_reviews 表读取视图）。
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, PartialEq)]
pub struct ValidationReview {
    /// 评审任务 ID（同一次对抗校验分组）
    pub id: String,
    /// 归属本体 IRI
    pub ontology_iri: String,
    /// 当前轮次（1=初始评审，2=A修订，3=B复核）
    #[specta(type = specta_typescript::Number)]
    pub round: u32,
    /// 复核轮数封顶
    #[specta(type = specta_typescript::Number)]
    pub max_rounds: u32,
    /// B 的评审输出（结构化建议 JSON）
    pub b_report: String,
    /// A 修订后的 Turtle V2 全文
    pub a_revision: String,
    /// A 的修订说明（B 复核时不可见）
    pub a_reasoning: String,
    /// B 复核原始输出 JSON（verdict + remaining_issues；此前不落库，仲裁时
    /// 无从得知 B 复核到底提了哪些残留问题——v0.7 补）
    #[serde(default)]
    pub b_verdict: String,
    /// 各环节耗时（unix ms；b_review/a_revise/b_recheck 完成时刻的耗时快照，
    /// 0=未完成）——此前不落库，切页回来"上次会话"环节无耗时可显示
    #[serde(default)]
    pub step_ms: ReviewStepMs,
    /// "in_progress" | "resolved" | "escalated"
    pub status: String,
    /// unix ms
    #[specta(type = specta_typescript::Number)]
    pub created_at: i64,
    /// unix ms
    #[specta(type = specta_typescript::Number)]
    pub updated_at: i64,
}

/// 评审环节耗时快照（unix ms，0=未完成）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, specta::Type, PartialEq, Default)]
pub struct ReviewStepMs {
    /// B 评审（环节 1）耗时
    #[serde(default)]
    #[specta(type = specta_typescript::Number)]
    pub b_review: i64,
    /// A 修订（环节 2）耗时
    #[serde(default)]
    #[specta(type = specta_typescript::Number)]
    pub a_revise: i64,
    /// B 复核（环节 3）耗时
    #[serde(default)]
    #[specta(type = specta_typescript::Number)]
    pub b_recheck: i64,
}

impl Default for ValidationReview {
    fn default() -> Self {
        Self {
            id: String::new(),
            ontology_iri: String::new(),
            round: 1,
            max_rounds: 2,
            b_report: String::new(),
            a_revision: String::new(),
            a_reasoning: String::new(),
            b_verdict: String::new(),
            step_ms: ReviewStepMs::default(),
            status: "in_progress".to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_roundtrip() {
        assert_eq!(IssueSeverity::from_str("error"), IssueSeverity::Error);
        assert_eq!(IssueSeverity::from_str("warning"), IssueSeverity::Warning);
        assert_eq!(IssueSeverity::from_str("suggestion"), IssueSeverity::Suggestion);
        assert_eq!(IssueSeverity::Error.as_str(), "error");
    }

    #[test]
    fn status_roundtrip() {
        assert_eq!(IssueStatus::from_str("open"), IssueStatus::Open);
        assert_eq!(IssueStatus::from_str("disputed"), IssueStatus::Disputed);
        assert_eq!(IssueStatus::from_str("accepted"), IssueStatus::Accepted);
        assert_eq!(IssueStatus::from_str("overruled"), IssueStatus::Overruled);
        assert_eq!(IssueStatus::from_str("unknown"), IssueStatus::Open);
    }

    #[test]
    fn constructors_set_defaults() {
        let e = ValidationIssue::error("L1-1", "L1", "check_x", "err");
        assert_eq!(e.severity, IssueSeverity::Error);
        assert_eq!(e.status, IssueStatus::Open);
        assert!(e.dispute.is_none());

        let w = ValidationIssue::warning("L3-1", "L3", "check_y", "warn");
        assert_eq!(w.severity, IssueSeverity::Warning);

        let s = ValidationIssue::suggestion("L4-1", "L4", "check_z", "sug");
        assert_eq!(s.severity, IssueSeverity::Suggestion);
    }

    #[test]
    fn serde_json_shape() {
        let issue = ValidationIssue::error("L1-1", "L1", "check_x", "err");
        let json = serde_json::to_value(&issue).unwrap();
        assert_eq!(json["severity"], "error");
        assert_eq!(json["status"], "open");
        // 空 Option 字段 skip 掉（前端 TS 类型 Option<T> → T | null）
        assert!(json.get("target").is_none());
        // source 缺省 rules（旧 JSON 反序列化兼容：缺字段回填默认值）
        assert_eq!(json["source"], "rules");
    }

    #[test]
    fn source_defaults_rules_on_missing_field() {
        // 旧前端 persist 的 JSON 无 source 字段 → serde default 回填 "rules"
        let legacy = r#"{"id":"L3-1","layer":"L3","severity":"warning","rule":"check_x","message":"旧数据"}"#;
        let issue: ValidationIssue = serde_json::from_str(legacy).unwrap();
        assert_eq!(issue.source, "rules");
    }

    #[test]
    fn review_defaults_on_missing_new_fields() {
        // 旧 DB 行（无 b_verdict/step_ms）反序列化不炸——serde default
        let legacy = r#"{"id":"vr-x","ontology_iri":"iri","round":1,"max_rounds":2,"b_report":"","a_revision":"","a_reasoning":"","status":"in_progress","created_at":0,"updated_at":0}"#;
        let r: ValidationReview = serde_json::from_str(legacy).unwrap();
        assert_eq!(r.b_verdict, "");
        assert_eq!(r.step_ms.b_review, 0);
    }
}
