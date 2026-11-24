//! 多轮协同对抗状态机（VALIDATION-DESIGN.md §3）。
//!
//! 三层对抗的存储与流转逻辑：
//!   - 第一层 纠错循环（L0-L2 error）：纯规则，无状态机（validate → A 修订 → 再 validate）
//!   - 第二层 规则答辩（L3 warning）：`dispute_issue` + `resolve_dispute` 两步裁决
//!   - 第三层 语义对抗（L4.3）：`review_ontology` / `a_revise` / `b_recheck` / `escalate`
//!
//! 核心原则（v0.3.1 硬化）：
//!   - **裁决权在 B，不在代码，更不在 A 自己**——counterEvidence 机械匹配失败即驳回
//!     （代码层）；argument 可信度由 B 复核判定（LLM 层）
//!   - **A 不能自我翻案**：resolve_dispute 不作为 A 的工具暴露
//!   - B 复核只看修订输出（Turtle V2），不看修订说明（a_reasoning 对 B 不可见）

use rusqlite::Connection;
use rusqlite::OptionalExtension;

use crate::error::StoreResult;
use crate::ttl::TtlStore;
use crate::validation::{DisputeRecord, ValidationEvidenceRef, ValidationIssue, ValidationReview};

/// 容错提取 JSON 数组文本：剥前导 `---` 分隔残留与 ```json 围栏，取首 `[` 到末 `]`。
///
/// LLM 实测输出（2026-09-23 真实评审）：a_reasoning 形如
/// `"---\n\n```json\n[...]\n```"`——run_a_revise 的 `trim_start_matches("---\n")`
/// 因前导 `\n`（双分隔符输出）剥不掉，严格 `serde_json::from_str` 全军覆没。
fn extract_json_array(s: &str) -> Option<&str> {
    let t = s.trim().trim_start_matches('-').trim();
    let t = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t)
        .trim();
    let start = t.find('[')?;
    let end = t.rfind(']')?;
    (start < end).then(|| &t[start..=end])
}

impl TtlStore {
    /// A 对 warning 级 issue 发起答辩（第二层对抗）。
    ///
    /// 两步裁决的第 ① 步在此完成：counterEvidence 必须引用已登记的 evidence_source
    /// 且 quote 通过机械匹配（`evidence::match_quote`）。匹配失败直接返回错误——
    /// **不进入** argument 判定（证据不成立，无辩论空间）。
    ///
    /// 返回：issue 更新后的状态（"disputed"）。
    pub fn dispute_issue(
        &self,
        issue_id: &str,
        argument: &str,
        counter_source_id: &str,
        counter_quote: &str,
    ) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();

        // 0. 定位 issue
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT severity, status FROM validation_issues WHERE id = ?1",
                rusqlite::params![issue_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let Some((severity, status)) = row else {
            return Ok(format!("错误：issue {issue_id} 不存在"));
        };
        if severity != "warning" {
            return Ok(format!(
                "错误：只有 warning 级 issue 可答辩（{issue_id} 是 {severity} 级）"
            ));
        }
        if status == "disputed" {
            return Ok(format!("错误：issue {issue_id} 已在答辩中"));
        }
        if status == "overruled" || status == "accepted" {
            return Ok(format!("错误：issue {issue_id} 已裁决（{status}），不可再次答辩"));
        }

        // ① counterEvidence 机械匹配（裁决第一步，代码层）
        let source: Option<(String,)> = conn
            .query_row(
                "SELECT content FROM evidence_sources WHERE source_id = ?1",
                rusqlite::params![counter_source_id],
                |r| Ok((r.get(0)?,)),
            )
            .ok();
        let Some((content,)) = source else {
            return Ok(format!(
                "错误：证据源 {counter_source_id} 未登记——counterEvidence 必须引用已登记的 evidence_source"
            ));
        };
        if crate::evidence::match_quote(&content, counter_quote, None) == crate::evidence::MatchLevel::None {
            return Ok(format!(
                "答辩被驳回（第①步机械匹配失败）：counterEvidence 引用的原文「{counter_quote}」\
                 在源 {counter_source_id} 中未匹配到——证据不成立，不进入 argument 判定"
            ));
        }

        // ② 记录答辩，状态 → disputed（argument 可信度留给 B 复核判定）
        conn.execute(
            "UPDATE validation_issues
             SET status = 'disputed',
                 dispute_argument = ?2,
                 dispute_counter_source = ?3,
                 dispute_counter_quote = ?4,
                 updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![
                issue_id,
                argument,
                counter_source_id,
                counter_quote,
                crate::ttl::now_ms(),
            ],
        )?;
        Ok("disputed".to_string())
    }

    /// 裁决答辩（第二层对抗第 ② 步）。**由 B 复核或人工调用，不作为 A 的工具暴露。**
    ///
    /// - `verdict = "overruled"`：B 认可 argument → issue 撤销（不再阻断）
    /// - `verdict = "accepted"`：B 不认可 argument → 升级 error（阻断）
    ///
    /// `resolved_by`：`"verifier"`（B 复核）或 `"human"`（人工仲裁）。
    /// 取一条 disputed issue 的裁决上下文（issue 描述 + A 的论据 + 反证原文）。
    /// 供 `resolve_dispute_llm` 命令构造 B 裁决 prompt。
    pub fn get_dispute_context(
        &self,
        issue_id: &str,
    ) -> StoreResult<Option<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(String, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT message, dispute_argument, dispute_counter_quote
                 FROM validation_issues WHERE id = ?1 AND status = 'disputed'",
                rusqlite::params![issue_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(crate::error::StoreError::from)?;
        Ok(row.and_then(|(m, a, q)| {
            Some((m, a?, q?))
        }))
    }

    pub fn resolve_dispute(
        &self,
        issue_id: &str,
        verdict: &str,
        resolved_by: &str,
    ) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        Self::resolve_dispute_conn(&conn, issue_id, verdict, resolved_by)
    }

    /// 无锁版裁决落库（内部：b_recheck 已持锁时复用，避免 Mutex 重入死锁）。
    /// overruled → 撤销；accepted → 升级 error；uncertain 不处理（保持 disputed 进仲裁）。
    fn resolve_dispute_conn(
        conn: &Connection,
        issue_id: &str,
        verdict: &str,
        resolved_by: &str,
    ) -> StoreResult<String> {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM validation_issues WHERE id = ?1",
                rusqlite::params![issue_id],
                |r| r.get(0),
            )
            .ok();
        let Some(status) = status else {
            return Ok(format!("错误：issue {issue_id} 不存在"));
        };
        if status != "disputed" {
            return Ok(format!(
                "错误：issue {issue_id} 状态为 {status}，只有 disputed 状态可裁决"
            ));
        }
        let new_status = match verdict {
            "overruled" => "overruled",
            "accepted" => "accepted",
            _ => return Ok(format!("错误：verdict 必须是 overruled 或 accepted（收到 {verdict}）")),
        };
        conn.execute(
            "UPDATE validation_issues
             SET status = ?2, dispute_resolved_by = ?3, dispute_verdict = ?2, updated_at = ?4
             WHERE id = ?1",
            rusqlite::params![issue_id, new_status, resolved_by, crate::ttl::now_ms()],
        )?;
        Ok(new_status.to_string())
    }

    /// A 接受 warning（不走裁决）：A 自愿接受告警，直接标 accepted（告警成立）。
    /// 区别于 resolve_dispute（要求先 disputed、由 B/人工裁决）——accept_issue 是
    /// A 在 open 状态直接认领，resolved_by="modeler"。run_auto_dispute 的 accept
    /// 分支调用，使 issue 状态流转（此前 A 接受但 status 维持 open 是 bug）。
    pub fn accept_issue(&self, issue_id: &str) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM validation_issues WHERE id = ?1",
                rusqlite::params![issue_id],
                |_| Ok(()),
            )
            .is_ok();
        if !exists {
            return Ok(format!("错误：issue {issue_id} 不存在"));
        }
        conn.execute(
            "UPDATE validation_issues
             SET status = 'accepted', dispute_resolved_by = 'modeler',
                 dispute_verdict = 'accepted', updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![issue_id, crate::ttl::now_ms()],
        )?;
        Ok("accepted".to_string())
    }

    /// 批量收敛：把本体全部仍 open 的 warning 转 accepted（跳过 disputed——那是答辩
    /// 中/待裁决的，不能强行覆盖）。run_auto_dispute 收尾调用：rejected_match
    /// （摘录未过机械匹配）/ error（处理失败）的 warning 卡在 open 无出口，按
    /// "无有效证据反驳 = 告警成立" 收敛，避免悬在 open 死胡同。
    pub fn accept_open_warnings(&self, ontology_iri: &str) -> StoreResult<usize> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE validation_issues
             SET status = 'accepted', dispute_resolved_by = 'modeler',
                 dispute_verdict = 'accepted', updated_at = ?2
             WHERE ontology_iri = ?1 AND severity = 'warning' AND status = 'open'",
            rusqlite::params![ontology_iri, crate::ttl::now_ms()],
        )?;
        Ok(n)
    }

    /// 查询本体的全部 issue（校验报告 + 对抗状态展示）。
    pub fn list_issues(&self, ontology_iri: &str) -> StoreResult<Vec<ValidationIssue>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, layer, source, severity, rule, message, target,
                    evidence_source_id, evidence_quote,
                    principle_source, principle_ref, suggestion,
                    status, a_action,
                    dispute_argument, dispute_counter_source,
                    dispute_counter_quote, dispute_resolved_by, dispute_verdict
             FROM validation_issues WHERE ontology_iri = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(rusqlite::params![ontology_iri], |r| {
            let status_str: String = r.get(12)?;
            let a_action_json: Option<String> = r.get(13)?;
            let dispute_argument: Option<String> = r.get(14)?;
            let dispute_counter_source: Option<String> = r.get(15)?;
            let dispute_counter_quote: Option<String> = r.get(16)?;
            let dispute_resolved_by: Option<String> = r.get(17)?;
            let dispute_verdict: Option<String> = r.get(18)?;
            Ok(ValidationIssue {
                id: r.get(0)?,
                layer: r.get(1)?,
                source: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "rules".into()),
                severity: crate::validation::IssueSeverity::from_str(&r.get::<_, String>(3)?),
                rule: r.get(4)?,
                message: r.get(5)?,
                target: r.get(6)?,
                evidence: match (r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?) {
                    (Some(source_id), Some(quote)) => Some(ValidationEvidenceRef { source_id, quote }),
                    _ => None,
                },
                principle: match (
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                ) {
                    (Some(source), Some(ref_)) => Some(crate::validation::PrincipleRef { source, ref_ }),
                    _ => None,
                },
                suggestion: r.get(11)?,
                status: crate::validation::IssueStatus::from_str(&status_str),
                a_action: a_action_json
                    .and_then(|j| serde_json::from_str::<crate::validation::AActionRecord>(&j).ok()),
                dispute: match (
                    dispute_argument,
                    dispute_counter_source,
                    dispute_counter_quote,
                ) {
                    (Some(argument), Some(counter_source), Some(counter_quote)) => {
                        Some(DisputeRecord {
                            argument,
                            counter_evidence: ValidationEvidenceRef {
                                source_id: counter_source,
                                quote: counter_quote,
                            },
                            resolved_by: dispute_resolved_by.unwrap_or_default(),
                            verdict: dispute_verdict.unwrap_or_default(),
                        })
                    }
                    _ => None,
                },
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// 持久化一次 validate 的 issue 快照到 validation_issues。
    ///
    /// rules 域 issue id 入库统一加本体作用域前缀（"{iri}#{rule-id}"）：表主键
    /// 全局唯一，而规则稳定 id（"L2-1"/"L3-3"）只在本体内唯一，不加前缀时
    /// 第二个本体（或删本体后的孤儿残留行）落同名 id 会撞 UNIQUE。
    ///
    /// 按 source 分域替换（统一发现池 v0.7）：只清**本次写入域**的旧行——
    /// `rules` 域（阶段① 规则检查）整本体替换，否则上一轮未闭环 error 永久
    /// 残留阻断导入；`review` 域（②a B 评审发现）由 review 生命周期管理
    /// （save_b_report 时按 review_id 替换），规则检查重跑不得冲掉 B 发现行。
    /// 返回写入条数。
    pub fn persist_issues(
        &self,
        ontology_iri: &str,
        validation_id: &str,
        issues: &[ValidationIssue],
    ) -> StoreResult<usize> {
        let conn = self.conn.lock().unwrap();
        // 分域替换：本批 issue 的 source 集合（默认 rules）。若混入多 source
        //（理论上不会：persist 只从单一入口进），按各自域清。
        let mut sources: Vec<String> = Vec::new();
        for issue in issues {
            if !sources.contains(&issue.source) {
                sources.push(issue.source.clone());
            }
        }
        if sources.is_empty() {
            sources.push("rules".to_string());
        }
        for src in &sources {
            conn.execute(
                "DELETE FROM validation_issues WHERE ontology_iri = ?1 AND source = ?2",
                rusqlite::params![ontology_iri, src],
            )?;
        }
        let now = crate::ttl::now_ms();
        let mut written = 0usize;
        for issue in issues {
            let status = issue.status.as_str();
            let a_action_json = issue
                .a_action
                .as_ref()
                .map(|a| serde_json::to_string(a).unwrap_or_default());
            let (dispute_argument, dispute_counter_source, dispute_counter_quote,
                 dispute_resolved_by, dispute_verdict) = match &issue.dispute {
                Some(d) => (
                    Some(d.argument.clone()),
                    Some(d.counter_evidence.source_id.clone()),
                    Some(d.counter_evidence.quote.clone()),
                    Some(d.resolved_by.clone()),
                    Some(d.verdict.clone()),
                ),
                None => (None, None, None, None, None),
            };
            let (evidence_source_id, evidence_quote) = match &issue.evidence {
                Some(e) => (Some(e.source_id.clone()), Some(e.quote.clone())),
                None => (None, None),
            };
            let (principle_source, principle_ref) = match &issue.principle {
                Some(p) => (Some(p.source.clone()), Some(p.ref_.clone())),
                None => (None, None),
            };
            // id 全局主键 vs 规则稳定 id 只在本体内唯一：入库加本体作用域前缀，
            // 否则第二个本体（或删本体后的孤儿行）落同名 id 撞 UNIQUE。
            // review 域 id 含 review uuid，已全局唯一，不加。
            let stored_id = if issue.source == "rules" {
                format!("{ontology_iri}#{}", issue.id)
            } else {
                issue.id.clone()
            };
            conn.execute(
                "INSERT INTO validation_issues
                 (id, ontology_iri, validation_id, layer, source, severity, rule, message, target,
                  evidence_source_id, evidence_quote, principle_source, principle_ref, suggestion,
                  status, a_action, dispute_argument, dispute_counter_source, dispute_counter_quote,
                  dispute_resolved_by, dispute_verdict, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?22)",
                rusqlite::params![
                    stored_id, ontology_iri, validation_id, issue.layer, issue.source,
                    issue.severity.as_str(), issue.rule, issue.message, issue.target,
                    evidence_source_id, evidence_quote,
                    principle_source, principle_ref, issue.suggestion, status, a_action_json,
                    dispute_argument, dispute_counter_source, dispute_counter_quote,
                    dispute_resolved_by, dispute_verdict, now,
                ],
            )?;
            written += 1;
        }
        Ok(written)
    }

    /// 启动 L4.3 语义评审（第三层对抗）：创建 validation_reviews 行。
    ///
    /// 残留防护：同一本体若存在旧的 in_progress review（任务中断/进程被杀残留，
    /// a_revision 空等），先标记 escalated 转人工仲裁——避免前端 latest 复用死行，
    /// 卡在「上次会话中断」永远走不完。
    pub fn review_ontology(&self, ontology_iri: &str) -> StoreResult<ValidationReview> {
        let conn = self.conn.lock().unwrap();
        let now = crate::ttl::now_ms();
        // 残留 in_progress → interrupted（任务已不在，不是分歧未收敛）。保留审计轨迹，不删行。
        // 与 src-tauri 启动期的 reap_interrupted 同口径：进程重启后不可能有活任务，
        // 因此任何 in_progress 残留都属中断，MUST NOT 标成 escalated 冒充"复核未收敛"。
        conn.execute(
            "UPDATE validation_reviews
             SET status = 'interrupted', outcome_reason = 'interrupted', updated_at = ?2
             WHERE ontology_iri = ?1 AND status = 'in_progress'",
            rusqlite::params![ontology_iri, now],
        )?;
        let id = format!("vr-{}", uuid::Uuid::new_v4().simple());
        // max_rounds=3：round 1=首轮 B 评审，2/3=两轮修订复核（§2.3 复核 ≤2 轮封顶——
        // 第 2 次复核仍追问才升级仲裁，此前单轮实现是编排层没回环，不是封顶值小）
        //
        // 基线草稿：记录本次评审所依据的工作区全文（写回前判冲突用）。这里直接用已持有的
        // conn 查 ontology_ttl——不能调 self.export_ttl()，它会再锁同一个 Mutex。
        let base_ttl: String = conn
            .query_row(
                "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri = ?1",
                rusqlite::params![ontology_iri],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .unwrap_or_default();
        conn.execute(
            "INSERT INTO validation_reviews (id, ontology_iri, round, max_rounds, status, base_ttl, created_at, updated_at)
             VALUES (?1, ?2, 1, 3, 'in_progress', ?4, ?3, ?3)",
            rusqlite::params![id, ontology_iri, now, base_ttl],
        )?;
        Ok(ValidationReview {
            id,
            ontology_iri: ontology_iri.to_string(),
            round: 1,
            max_rounds: 3,
            b_report: String::new(),
            a_revision: String::new(),
            a_reasoning: String::new(),
            b_verdict: String::new(),
            step_ms: crate::validation::ReviewStepMs::default(),
            status: "in_progress".to_string(),
            outcome_reason: String::new(),
            outcome_detail: String::new(),
            created_at: now,
            updated_at: now,
        })
    }
    /// 持久化 B 评审输出（第 1 步产物）。此前 b_report 只在前端 state——
    /// 刷新即丢，escalated 转仲裁后审计链断裂（决策 24 证据链要求可回溯）。
    ///
    /// 统一发现池 v0.7：同时把 B 的结构化发现**转写**为 review 域 issue 行
    /// （source='review'，layer='L4'，id="{review_id}:{B条目id}"），与规则发现
    /// 并列同一生命周期——B 建议从此可逐条答辩/仲裁/追踪。转写失败不阻断
    /// （b_report 原文仍完整保留）。
    pub fn save_b_report(
        &self,
        review_id: &str,
        b_report: &str,
    ) -> StoreResult<ValidationReview> {
        let conn = self.conn.lock().unwrap();
        let started = crate::ttl::now_ms();
        conn.execute(
            // step_ms_b 以 updated_at 为锚（= 评审创建/上一动作时刻）：复用残留
            // in_progress review 重跑 B 评审时，耗时不含任务中断的死时间
            "UPDATE validation_reviews SET b_report = ?2, step_ms_b = MAX(0, ?3 - updated_at), updated_at = ?3 WHERE id = ?1",
            rusqlite::params![review_id, b_report, started],
        )?;
        // B 发现 → review 域 issue 行（解析失败静默降级：b_report 原文保留，
        // 评审流程不受影响——发现池只是增强视图，不是硬依赖）
        let review = self.get_review(&conn, review_id)?;
        if let Err(e) = self.persist_review_issues_locked(&conn, &review.ontology_iri, review_id, b_report) {
            tracing::warn!("save_b_report 转写 review 域 issue 失败（b_report 原文已保留）: {e}");
        }
        Ok(review)
    }

    /// 把 B 评审 JSON 转写为 review 域 issue 行（`save_b_report` 内部调用）。
    ///
    /// B 报告格式（B_REVIEW_PREAMBLE）：`[{ "id","iri","dimension","type",
    /// "severity","message","evidence_ref","suggestion" }]`。
    /// - id 缺失时按数组序生成 "B{n}"（旧报告兼容）
    /// - severity 映射：error/warning/suggestion 原样；其他 → suggestion
    /// - evidence_ref 是自由文本（非已登记 source_id），存 suggestion 之外的
    ///   principle.ref_ 追加展示，不填 evidence FK（机械匹配只对登记源成立）
    fn persist_review_issues_locked(
        &self,
        conn: &Connection,
        ontology_iri: &str,
        review_id: &str,
        b_report: &str,
    ) -> StoreResult<()> {
        // 同一 review 重跑 B 评审 → 替换该 review 的 issue 行
        conn.execute(
            "DELETE FROM validation_issues WHERE ontology_iri = ?1 AND source = 'review' AND validation_id = ?2",
            rusqlite::params![ontology_iri, review_id],
        )?;
        let Some(json_text) = extract_json_array(b_report) else {
            return Ok(()); // 非 JSON（LLM 输出不稳）——原文保留在 b_report，不视为错误
        };
        let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json_text) else {
            return Ok(());
        };
        let now = crate::ttl::now_ms();
        // 先收集全部 bid（去 # 前缀、空值兜底 B{序号，从 1 起}），重复的递增序号
        // 直到唯一——B 报告同 id / 无 id 重复时 INSERT 主键冲突会中断整批转写
        let mut bids: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                item.get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().trim_start_matches('#').to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("B{}", idx + 1))
            })
            .collect();
        for i in 0..bids.len() {
            let mut n = i + 1;
            while bids[..i].contains(&bids[i]) {
                bids[i] = format!("B{n}");
                n += 1;
            }
        }
        for (idx, item) in items.iter().enumerate() {
            let bid = &bids[idx];
            let id = format!("{review_id}:{bid}");
            let dimension = item.get("dimension").and_then(|v| v.as_str()).unwrap_or("语义评审");
            let severity_raw = item.get("severity").and_then(|v| v.as_str()).unwrap_or("suggestion");
            let severity = crate::validation::IssueSeverity::from_str(severity_raw).as_str();
            let message = item
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("(B 未给出描述)")
                .to_string();
            let target = item.get("iri").and_then(|v| v.as_str()).map(|s| s.to_string());
            let suggestion = item.get("suggestion").and_then(|v| v.as_str()).map(|s| s.to_string());
            let evidence_ref = item.get("evidence_ref").and_then(|v| v.as_str()).map(|s| s.to_string());
            let rule = format!("review:{dimension}");
            conn.execute(
                "INSERT INTO validation_issues
                 (id, ontology_iri, validation_id, layer, source, severity, rule, message, target,
                  principle_source, principle_ref, suggestion, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'L4', 'review', ?4, ?5, ?6, ?7,
                         'review', ?8, ?9, 'open', ?10, ?10)",
                rusqlite::params![
                    id, ontology_iri, review_id, severity, rule, message, target,
                    evidence_ref, suggestion, now,
                ],
            )?;
        }
        Ok(())
    }

    /// 把 A 的修订说明（a_reasoning JSON）逐条回填到 issue 的 a_action 列
    /// （`a_revise` 内部调用）。合并方案 v0.8：A 对统一发现池逐条表态——
    /// review 域（B 评审发现）与 rules 域（规则 warning/suggestion）都处理：
    /// - accepted：按建议修订（status → accepted）
    /// - rejected（异议）：review 域直接转 disputed（B 复核裁决）；rules 域
    ///   必须带证据引用（source_id/quote/argument），机械匹配通过才转 disputed，
    ///   失败或无证据按 accepted 收敛（告警成立，与答辩链 rejected_match 语义一致）。
    fn apply_a_actions_locked(
        &self,
        conn: &Connection,
        review_id: &str,
        ontology_iri: &str,
        a_reasoning: &str,
    ) -> StoreResult<()> {
        let Some(json_text) = extract_json_array(a_reasoning) else {
            return Ok(()); // 非 JSON——原文保留，不视为错误
        };
        let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json_text) else {
            return Ok(());
        };
        let now = crate::ttl::now_ms();
        // rules 域候选：该本体 rules 源 open 的 warning/suggestion（A 用 R 编号或 rule 名引用）
        let rules_rows: Vec<(String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT id, rule FROM validation_issues
                 WHERE ontology_iri = ?1 AND source = 'rules'
                   AND severity IN ('warning','suggestion') AND status = 'open'
                 ORDER BY created_at",
            )?;
            let it = stmt.query_map(rusqlite::params![ontology_iri], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            it.collect::<Result<Vec<_>, _>>()?
        };
        for item in items {
            let Some(raw_id) = item.get("issue_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let action = item.get("action").and_then(|v| v.as_str()).unwrap_or("accepted");
            let reason = item.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            // —— ① review 域匹配（B 条目段，双向包含，现状逻辑）——
            let rows: Vec<(String, String)> = {
                let mut stmt = conn.prepare(
                    "SELECT id, status FROM validation_issues
                     WHERE source = 'review' AND validation_id = ?1",
                )?;
                let it = stmt.query_map(rusqlite::params![review_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                it.collect::<Result<Vec<_>, _>>()?
            };
            let norm = |s: &str| s.replace([' ', '\n', '\t', '_'], "");
            let raw_norm = norm(raw_id);
            let mut best: Option<String> = None;
            let mut best_len = 0usize;
            for (row_id, _) in &rows {
                let Some(bid) = row_id.strip_prefix(&format!("{review_id}:")) else {
                    continue;
                };
                let bid_norm = norm(bid);
                if bid_norm.is_empty() || raw_norm.is_empty() {
                    continue;
                }
                if raw_norm.contains(&bid_norm) || bid_norm.contains(&raw_norm) {
                    if bid_norm.len() > best_len {
                        best = Some(row_id.clone());
                        best_len = bid_norm.len();
                    }
                }
            }
            if let Some(target_id) = best {
                self.apply_a_action_locked(conn, &target_id, &action, &reason, now)?;
                continue;
            }
            // —— ② rules 域匹配：R 编号（R1..Rn → 按序）或 rule 名包含 ——
            let mut rule_target: Option<String> = None;
            if let Some(rn) = raw_id
                .strip_prefix('R')
                .and_then(|s| s.trim().parse::<usize>().ok())
            {
                rule_target = rules_rows.get(rn.saturating_sub(1)).map(|(id, _)| id.clone());
            } else {
                for (id, rule) in &rules_rows {
                    let rn = norm(rule);
                    if !rn.is_empty() && (raw_norm.contains(&rn) || rn.contains(&raw_norm)) {
                        rule_target = Some(id.clone());
                        break;
                    }
                }
            }
            let Some(target_id) = rule_target else {
                continue; // A 引用了不存在的条目——跳过（不阻断）
            };
            let a_json = serde_json::to_string(&crate::validation::AActionRecord {
                action: action.to_string(),
                reason: reason.to_string(),
            })
            .unwrap_or_default();
            if action.trim().eq_ignore_ascii_case("rejected") {
                // rules 域异议：必须带证据引用，机械匹配把关（答辩链闸门保留）
                let source_id = item.get("source_id").and_then(|v| v.as_str()).unwrap_or("");
                let quote = item.get("quote").and_then(|v| v.as_str()).unwrap_or("");
                let argument = item.get("argument").and_then(|v| v.as_str()).unwrap_or("");
                if source_id.is_empty() || quote.is_empty() || argument.is_empty() {
                    self.apply_a_action_locked(conn, &target_id, "accepted", "异议未带证据引用，按接受处理", now)?;
                    continue;
                }
                let source_content: Option<String> = conn
                    .query_row(
                        "SELECT content FROM evidence_sources WHERE source_id = ?1",
                        rusqlite::params![source_id],
                        |r| r.get(0),
                    )
                    .ok();
                let Some(source_content) = source_content else {
                    self.apply_a_action_locked(conn, &target_id, "accepted", "异议引用的证据源未登记，按接受处理", now)?;
                    continue;
                };
                if crate::evidence::match_quote(&source_content, &quote, None) == crate::evidence::MatchLevel::None {
                    self.apply_a_action_locked(conn, &target_id, "accepted", "异议引文未通过机械匹配，按接受处理", now)?;
                    continue;
                }
                // 机械匹配通过 → 异议成立：disputed + 写 dispute 字段（B 复核裁决）
                conn.execute(
                    "UPDATE validation_issues
                     SET status = 'disputed', a_action = ?2,
                         dispute_argument = ?3, dispute_counter_source = ?4,
                         dispute_counter_quote = ?5, updated_at = ?6
                     WHERE id = ?1",
                    rusqlite::params![target_id, a_json, argument, source_id, quote, now],
                )?;
            } else {
                self.apply_a_action_locked(conn, &target_id, "accepted", reason, now)?;
            }
        }
        Ok(())
    }

    /// 单条 issue 的 A 表态落库：accepted（认领，按建议修订）/
    /// rejected（转 disputed，分歧显式化，B 复核裁决）。
    fn apply_a_action_locked(
        &self,
        conn: &Connection,
        issue_id: &str,
        action: &str,
        reason: &str,
        now: i64,
    ) -> StoreResult<()> {
        let a_json = serde_json::to_string(&crate::validation::AActionRecord {
            action: action.to_string(),
            reason: reason.to_string(),
        })
        .unwrap_or_default();
        let new_status = if action.trim().eq_ignore_ascii_case("rejected") {
            "disputed"
        } else {
            "accepted"
        };
        conn.execute(
            "UPDATE validation_issues
             SET a_action = ?2, status = ?3, updated_at = ?4 WHERE id = ?1",
            rusqlite::params![issue_id, a_json, new_status, now],
        )?;
        Ok(())
    }

    /// A 提交修订（第三层对抗第 2 步）：存修订全文 + 修订说明。
    /// a_reasoning 对 B 复核不可见（`b_recheck` 只读 a_revision）。
    pub fn a_revise(
        &self,
        review_id: &str,
        a_revision: &str,
        a_reasoning: &str,
    ) -> StoreResult<ValidationReview> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(String, i64, i64, String)> = conn
            .query_row(
                "SELECT ontology_iri, round, max_rounds, status FROM validation_reviews WHERE id = ?1",
                rusqlite::params![review_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .ok();
        let Some((ontology_iri, round, max_rounds, status)) = row else {
            return Ok(ValidationReview {
                id: review_id.to_string(),
                ontology_iri: String::new(),
                round: 0,
                max_rounds: 3,
                b_report: String::new(),
                a_revision: String::new(),
                a_reasoning: String::new(),
                b_verdict: String::new(),
                step_ms: crate::validation::ReviewStepMs::default(),
                status: "not_found".to_string(),
                    outcome_reason: String::new(),
                    outcome_detail: String::new(),
                created_at: 0,
                updated_at: 0,
            });
        };
        // 已结束（含中断）的评审不再推进：中断行可能被僵尸进程留下，继续推进会在
        // 一个没有任务认领的 review 上叠加产物。新一轮评审由 review_ontology 建行。
        if status == "resolved" || status == "escalated" || status == "interrupted" {
            return Ok(ValidationReview {
                id: review_id.to_string(),
                ontology_iri,
                round: round as u32,
                max_rounds: max_rounds as u32,
                b_report: String::new(),
                a_revision: String::new(),
                a_reasoning: String::new(),
                b_verdict: String::new(),
                step_ms: crate::validation::ReviewStepMs::default(),
                status,
                outcome_reason: String::new(),
                outcome_detail: String::new(),
                created_at: 0,
                updated_at: 0,
            });
        }
        if round >= max_rounds {
            // 封顶仍需修订 → 分歧未收敛（归因 verdict_capped，与环节失败/中断区分）
            conn.execute(
                "UPDATE validation_reviews
                 SET status = 'escalated', outcome_reason = 'verdict_capped', updated_at = ?2
                 WHERE id = ?1",
                rusqlite::params![review_id, crate::ttl::now_ms()],
            )?;
            return self.get_review(&conn, review_id);
        }
        // step_ms_a 以 updated_at 为锚分段计修订环节耗时（2026-09-24 修复：原先
        // ?now - created_at 是累计值，复核卡会把 B+A 总时长显示成本环节耗时）；
        // 多轮时 += 跨轮累计，卡上显示的是 A 修订总耗时
        conn.execute(
            "UPDATE validation_reviews
             SET round = ?2, a_revision = ?3, a_reasoning = ?4,
                 step_ms_a = step_ms_a + MAX(0, ?5 - updated_at), updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![review_id, round + 1, a_revision, a_reasoning, crate::ttl::now_ms()],
        )?;
        // 统一发现池 v0.7：A 的逐条裁决回填到 issue 的 a_action 列。
        // a_reasoning JSON 格式：[{"issue_id","action","reason"}]（issue_id 可能是
        // B 报告条目 id "B1"、R 编号 "R1" 或 A 自造编号——三种都按域匹配）。
        if let Err(e) = self.apply_a_actions_locked(&conn, review_id, &ontology_iri, a_reasoning) {
            tracing::warn!("a_revise 回填 a_action 失败（a_reasoning 原文已保留）: {e}");
        }
        // 轮次快照 v0.8：A 表态后落 review_rounds（该轮 issue 清单 + 统计），
        // 前端「轮次清单」可查看每轮 发现/已修改/异议/裁决。
        if let Err(e) = self.snapshot_round_locked(&conn, review_id, &ontology_iri, round) {
            tracing::warn!("a_revise 写轮次快照失败: {e}");
        }
        self.get_review(&conn, review_id).map(|mut r| {
            r.ontology_iri = ontology_iri;
            r
        })
    }

    /// 轮次快照（合并方案 v0.8）：把该轮 issue 清单 + 统计落 review_rounds。
    /// 快照只存状态清单（不存 b_report/a_revision 原文），轻量、可查看。
    /// 同轮重跑（异常/重试）替换该轮行，避免堆积。
    fn snapshot_round_locked(
        &self,
        conn: &Connection,
        review_id: &str,
        ontology_iri: &str,
        round: i64,
    ) -> StoreResult<()> {
        let rows: Vec<(String, String, String, String, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT id, severity, rule, status, a_action FROM validation_issues
                 WHERE (source = 'review' AND validation_id = ?1)
                    OR (ontology_iri = ?2 AND source = 'rules'
                        AND severity IN ('warning','suggestion')
                        AND status IN ('open','disputed','accepted','overruled'))
                 ORDER BY created_at",
            )?;
            let it = stmt.query_map(rusqlite::params![review_id, ontology_iri], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?;
            it.collect::<Result<Vec<_>, _>>()?
        };
        let snapshot: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, severity, rule, status, a_action)| {
                serde_json::json!({
                    "id": id,
                    "severity": severity,
                    "rule": rule,
                    "status": status,
                    "a_action": a_action,
                })
            })
            .collect();
        let found_count = rows.len() as i64;
        let accepted_count = rows.iter().filter(|(_, _, _, s, _)| s == "accepted").count() as i64;
        let disputed_count = rows.iter().filter(|(_, _, _, s, _)| s == "disputed").count() as i64;
        let now = crate::ttl::now_ms();
        conn.execute(
            "DELETE FROM review_rounds WHERE review_id = ?1 AND round = ?2",
            rusqlite::params![review_id, round],
        )?;
        conn.execute(
            "INSERT INTO review_rounds
             (id, review_id, round, snapshot_json, found_count, accepted_count, disputed_count,
              verdict, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'in_progress', ?8, ?8)",
            rusqlite::params![
                format!("rr-{}", uuid::Uuid::new_v4().simple()),
                review_id,
                round,
                serde_json::to_string(&snapshot).unwrap_or_else(|_| "[]".into()),
                found_count,
                accepted_count,
                disputed_count,
                now,
            ],
        )?;
        Ok(())
    }

    /// 轮次清单（合并方案 v0.8）：按轮次返回该评审的 issue 快照 + 统计。
    pub fn list_review_rounds(&self, review_id: &str) -> StoreResult<Vec<crate::validation::ReviewRound>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, review_id, round, snapshot_json, found_count, accepted_count,
                    disputed_count, verdict, created_at, updated_at
             FROM review_rounds WHERE review_id = ?1 ORDER BY round",
        )?;
        let it = stmt.query_map(rusqlite::params![review_id], |r| {
            Ok(crate::validation::ReviewRound {
                id: r.get(0)?,
                review_id: r.get(1)?,
                round: r.get(2)?,
                snapshot_json: r.get(3)?,
                found_count: r.get(4)?,
                accepted_count: r.get(5)?,
                disputed_count: r.get(6)?,
                verdict: r.get(7)?,
                created_at: r.get(8)?,
                updated_at: r.get(9)?,
            })
        })?;
        it.collect::<Result<Vec<_>, _>>().map_err(crate::error::StoreError::from)
    }

    /// B 复核（第三层对抗第 3 步，多轮对抗状态机）：
    /// - `confirm=true` → 复核确认，review → resolved
    /// - `confirm=false`（追问）且 `round < max_rounds` → **进下一轮**：保持
    ///   in_progress，清 a_revision（前端据此回到「A 修订」环节），created_at /
    ///   step_ms_* 复位为本轮起点（环节耗时按轮分段），b_verdict 落库供下一轮 A 参考
    /// - `confirm=false` 且 `round ≥ max_rounds` → 轮次封顶（§3.1 复核 ≤2 轮），
    ///   review → escalated（转人工仲裁）
    ///
    /// **只读 a_revision（修订全文），不读 a_reasoning（修订说明）**——产物对产物博弈。
    /// `b_verdict_json`：B 复核原始输出（v0.7 落库——仲裁时可见 B 提了哪些残留问题）。
    pub fn b_recheck(
        &self,
        review_id: &str,
        confirm: bool,
        b_verdict_json: &str,
    ) -> StoreResult<ValidationReview> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(i64, i64)> = conn
            .query_row(
                "SELECT round, max_rounds FROM validation_reviews WHERE id = ?1",
                rusqlite::params![review_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let Some((round, max_rounds)) = row else {
            return Ok(ValidationReview {
                id: review_id.to_string(),
                ontology_iri: String::new(),
                round: 0,
                max_rounds: 3,
                b_report: String::new(),
                a_revision: String::new(),
                a_reasoning: String::new(),
                b_verdict: String::new(),
                step_ms: crate::validation::ReviewStepMs::default(),
                status: "not_found".to_string(),
                    outcome_reason: String::new(),
                    outcome_detail: String::new(),
                created_at: 0,
                updated_at: 0,
            });
        };
        let now = crate::ttl::now_ms();
        // 评审收敛 → 把 A 最终修订稿（a_revision）写回本体表（overwrite）。
        // 此前修订稿只存 validation_reviews.a_revision，属性界面看不到新 schema
        // ——gate 哲学是"修好才进库"，B 复核通过即"修好"，该落盘了。
        // a_revision 已被 run_a_revise 的 strip_code_fence 剥过围栏，是干净 Turtle。
        // 先在此分支取出待落盘数据，SQL 全执行完、conn 释放后再 import_ttl（避免
        // 持锁调 import_ttl 二次锁同一 Mutex）。
        let mut pending_import: Option<String> = None;
        if confirm {
            // 写回前先判基线：评审期间草稿若被改过，整本 overwrite 会静默冲掉用户的编辑。
            // 基线用评审创建时记录的 base_ttl 做**全文比对**，不用时间戳——保存相同内容
            // 也会变时间戳，据时间戳判会误报。基线为空（历史评审行）视为"无法判定"，
            // 保守拒绝覆盖，MUST NOT 静默写回。
            let snapshot: Option<(String, String, String)> = conn
                .query_row(
                    "SELECT ontology_iri, a_revision, base_ttl
                     FROM validation_reviews WHERE id = ?1",
                    rusqlite::params![review_id],
                    |r| Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    )),
                )
                .ok();
            let mut write_back_conflict = false;
            if let Some((ont_iri, revision, base_ttl)) = snapshot {
                if !revision.is_empty() {
                    let current: String = conn
                        .query_row(
                            "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri = ?1",
                            rusqlite::params![&ont_iri],
                            |r| r.get::<_, Option<String>>(0),
                        )
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    if base_ttl.is_empty() || current != base_ttl {
                        write_back_conflict = true;
                    } else {
                        pending_import = Some(revision);
                    }
                }
            }
            // step_ms_re 以 updated_at 为锚分段计复核环节耗时（2026-09-24 修复：原先
            // ?now - created_at 是累计值，复核卡把三步总时长显示成本环节耗时）
            conn.execute(
                "UPDATE validation_reviews
                 SET status = 'resolved', b_verdict = ?2, outcome_reason = ?4,
                     step_ms_re = step_ms_re + MAX(0, ?3 - updated_at), updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![
                    review_id,
                    b_verdict_json,
                    now,
                    if write_back_conflict { "write_back_conflict" } else { "" }
                ],
            )?;
        } else if round < max_rounds {
            // 追问未封顶 → 进下一轮（§2.2.1 多轮对抗）：a_revision 清空使前端回到
            // 「A 修订」环节显示；step_ms_re 以 updated_at 为锚分段 += 累计本轮复核耗时
            conn.execute(
                "UPDATE validation_reviews
                 SET b_verdict = ?2, a_revision = '',
                     step_ms_re = step_ms_re + MAX(0, ?3 - updated_at), updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![review_id, b_verdict_json, now],
            )?;
        } else {
            // 轮次封顶（复核 ≤2 轮）→ 分歧未收敛，转人工仲裁。归因写死为 verdict_capped，
            // 与"环节失败/任务中断"区分开——三者此前共用 escalated，界面一律说"复核未收敛"。
            conn.execute(
                "UPDATE validation_reviews
                 SET status = 'escalated', b_verdict = ?2, outcome_reason = 'verdict_capped',
                     step_ms_re = step_ms_re + MAX(0, ?3 - updated_at), updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![review_id, b_verdict_json, now],
            )?;
        }
        // 路线 B（2026-11-03 裁定）：B 复核**不裁决建模者的异议**。
        // 此前这里会读 b_verdict 的 dispute_rulings 并代为 resolve_dispute——但编排层
        // 从未把异议清单（甚至 B 自己的上一轮建议清单）交给 B，等于让它对看不见的东西
        // 下判决。异议一律保持 disputed，由阶段③人工仲裁受理（队列按 status='disputed'
        // 全量入队，无需额外通道）。历史/异常输出里的 dispute_rulings 字段一律忽略。
        // 更新最新轮次快照的 verdict（同步 review 最终状态：resolved/escalated/in_progress）
        if let Ok(Some(rs)) = conn
            .query_row(
                "SELECT status FROM validation_reviews WHERE id = ?1",
                rusqlite::params![review_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
        {
            let _ = conn.execute(
                "UPDATE review_rounds SET verdict = ?2, updated_at = ?3
                 WHERE id = (SELECT id FROM review_rounds WHERE review_id = ?1
                             ORDER BY round DESC LIMIT 1)",
                rusqlite::params![review_id, rs, now],
            );
        }
        let review = self.get_review(&conn, review_id);
        drop(conn);
        // 评审收敛 → A 修订稿写回本体表（在 conn 释放后执行，避免持锁二次锁）
        if let Some(revision) = pending_import {
            if let Err(e) = self.import_ttl(&revision, true) {
                tracing::warn!("评审收敛写回本体表失败（a_revision 已保留）: {e}");
            }
        }
        review
    }

    /// 结束评审并给出归因（第三层对抗超限/分歧/失败）：review → escalated。
    ///
    /// `reason` ∈ `verdict_capped` | `step_failed` | `manual_escalation`；`detail` 存实际
    /// 错误文本（仅日志与 tooltip 用，不直接面向用户）。幂等：已 escalated 仍刷新归因，
    /// 便于给先前"无归因"的 escalated 补上真实原因。
    pub fn escalate(
        &self,
        review_id: &str,
        reason: &str,
        detail: &str,
    ) -> StoreResult<ValidationReview> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM validation_reviews WHERE id = ?1",
                rusqlite::params![review_id],
                |_| Ok(()),
            )
            .is_ok();
        if !exists {
            return Ok(ValidationReview {
                id: review_id.to_string(),
                ontology_iri: String::new(),
                round: 0,
                max_rounds: 3,
                b_report: String::new(),
                a_revision: String::new(),
                a_reasoning: String::new(),
                b_verdict: String::new(),
                step_ms: crate::validation::ReviewStepMs::default(),
                status: "not_found".to_string(),
                    outcome_reason: String::new(),
                    outcome_detail: String::new(),
                created_at: 0,
                updated_at: 0,
            });
        }
        conn.execute(
            "UPDATE validation_reviews
             SET status = 'escalated', outcome_reason = ?3, outcome_detail = ?4, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![review_id, crate::ttl::now_ms(), reason, detail],
        )?;
        self.get_review(&conn, review_id)
    }

    /// 回收无主评审：把 `in_progress` 但不在 `live_ids` 里的行标为 `interrupted`。
    ///
    /// 后台任务（`run_review_loop`）随进程消失时没人来得及改状态，于是界面按
    /// `deriveStep` 倒推会永远显示"B 评审 进行中"，用时还从第一次运行起算——看着
    /// 就是"卡死"。桌面应用单进程：注册表里没有的 in_progress 定义上就是僵尸，
    /// 不需要心跳超时阈值来猜。返回被回收的行数。
    pub fn reap_interrupted(&self, live_ids: &[String]) -> StoreResult<usize> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id FROM validation_reviews WHERE status = 'in_progress'",
        )?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(Result::ok)
            .filter(|id| !live_ids.iter().any(|live| live == id))
            .collect();
        let now = crate::ttl::now_ms();
        for id in &rows {
            conn.execute(
                "UPDATE validation_reviews
                 SET status = 'interrupted', outcome_reason = 'interrupted', updated_at = ?2
                 WHERE id = ?1",
                rusqlite::params![id, now],
            )?;
        }
        if !rows.is_empty() {
            tracing::info!(count = rows.len(), "reap_interrupted: 无主评审已标为中断");
        }
        Ok(rows.len())
    }

    /// 查询本体的最新 review（可为 None）。
    pub fn latest_review(&self, ontology_iri: &str) -> StoreResult<Option<ValidationReview>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ontology_iri, round, max_rounds, b_report, a_revision, a_reasoning, status, created_at, updated_at, b_verdict, step_ms_b, step_ms_a, step_ms_re, outcome_reason, outcome_detail
             FROM validation_reviews WHERE ontology_iri = ?1 ORDER BY rowid DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![ontology_iri], row_to_review)?;
        rows.next().transpose().map_err(Into::into)
    }

    /// 查询本体可答辩的 warning 清单（rules 来源 + open 状态）——A 自动答辩的输入。
    pub fn list_disputable_warnings(
        &self,
        ontology_iri: &str,
    ) -> StoreResult<Vec<ValidationIssue>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, layer, source, severity, rule, message, target,
                    evidence_source_id, evidence_quote, principle_source, principle_ref,
                    suggestion, status, a_action,
                    dispute_argument, dispute_counter_source, dispute_counter_quote,
                    dispute_resolved_by, dispute_verdict
             FROM validation_issues
             WHERE ontology_iri = ?1 AND source = 'rules' AND severity = 'warning' AND status = 'open'
             ORDER BY created_at",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![ontology_iri], |r| {
                // 与 list_issues 同构的行映射（19 列，下标一致）
                Ok(ValidationIssue {
                    id: r.get(0)?,
                    layer: r.get(1)?,
                    source: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "rules".into()),
                    severity: crate::validation::IssueSeverity::from_str(&r.get::<_, String>(3)?),
                    rule: r.get(4)?,
                    message: r.get(5)?,
                    target: r.get(6)?,
                    evidence: match (r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?) {
                        (Some(source_id), Some(quote)) => Some(ValidationEvidenceRef { source_id, quote }),
                        _ => None,
                    },
                    principle: match (
                        r.get::<_, Option<String>>(9)?,
                        r.get::<_, Option<String>>(10)?,
                    ) {
                        (Some(source), Some(ref_)) => Some(crate::validation::PrincipleRef { source, ref_ }),
                        _ => None,
                    },
                    suggestion: r.get(11)?,
                    status: crate::validation::IssueStatus::from_str(&r.get::<_, String>(12)?),
                    a_action: r
                        .get::<_, Option<String>>(13)?
                        .and_then(|j| serde_json::from_str::<crate::validation::AActionRecord>(&j).ok()),
                    dispute: None, // open 状态无 dispute 记录
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 查询本体可用的证据源（id + 标题 + 全文）——A 构造答辩反证的素材。
    /// TTL 侧共用同库 evidence_sources 表；ontology_iri 维度的源 + 全局共享源。
    pub fn list_evidence_sources_for_dispute(
        &self,
        ontology_iri: &str,
    ) -> StoreResult<Vec<(String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT source_id, title, content FROM evidence_sources
             WHERE ontology_api_name = ?1 OR ontology_api_name = ''
             ORDER BY created_at DESC",
        ) {
            Ok(s) => s,
            // 旧库无表：无证据源，A 只能 accept（答辩通道给出空素材）
            Err(_) => return Ok(Vec::new()),
        };
        let rows = stmt
            .query_map(rusqlite::params![ontology_iri], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn get_review(&self, conn: &Connection, review_id: &str) -> StoreResult<ValidationReview> {
        let mut stmt = conn.prepare(
            "SELECT id, ontology_iri, round, max_rounds, b_report, a_revision, a_reasoning, status, created_at, updated_at, b_verdict, step_ms_b, step_ms_a, step_ms_re, outcome_reason, outcome_detail
             FROM validation_reviews WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![review_id], row_to_review)?;
        rows.next()
            .transpose()?
            .ok_or_else(|| crate::error::StoreError::Other(anyhow::anyhow!("review not found")))
    }
}

fn row_to_review(r: &rusqlite::Row) -> rusqlite::Result<ValidationReview> {
    Ok(ValidationReview {
        id: r.get(0)?,
        ontology_iri: r.get(1)?,
        round: r.get(2)?,
        max_rounds: r.get(3)?,
        b_report: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        a_revision: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        a_reasoning: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        status: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        b_verdict: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        outcome_reason: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
        outcome_detail: r.get::<_, Option<String>>(15)?.unwrap_or_default(),
        step_ms: crate::validation::ReviewStepMs {
            b_review: r.get(11)?,
            a_revise: r.get(12)?,
            b_recheck: r.get(13)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::IssueStatus;

    fn clean_ttl(iri: &str) -> String {
        format!(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <{iri}#> .
<{iri}#> a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "x" .
"#
        )
    }

    fn seed_issue(store: &TtlStore, id: &str, iri: &str, severity: &str, status: &str) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO validation_issues (id, ontology_iri, validation_id, layer, severity, rule, message, status, created_at, updated_at)
             VALUES (?1, ?2, 'v1', 'L3', ?3, 'check_x', '测试问题', ?4, 1, 1)",
            rusqlite::params![id, format!("{iri}#"), severity, status],
        )
        .unwrap();
    }

    #[test]
    fn list_disputable_warnings_filters_rules_open_only() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/d";
        // rules+open（应命中）×2；rules+error（severity 过滤）；rules+disputed
        // （状态过滤）；review 来源（source 过滤）
        seed_issue(&store, "vi-w1", iri, "warning", "open");
        seed_issue(&store, "vi-w2", iri, "warning", "open");
        seed_issue(&store, "vi-e1", iri, "error", "open");
        seed_issue(&store, "vi-w3", iri, "warning", "disputed");
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO validation_issues (id, ontology_iri, validation_id, layer, source, severity, rule, message, status, created_at, updated_at)
                 VALUES ('vi-r1', ?1, 'vr-1', 'L4', 'review', 'warning', 'review:完整性', '评审发现', 'open', 1, 1)",
                rusqlite::params![format!("{iri}#")],
            )
            .unwrap();
        }
        let list = store.list_disputable_warnings(&format!("{iri}#")).unwrap();
        let ids: Vec<&str> = list.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, vec!["vi-w1", "vi-w2"], "只取 rules+warning+open: {ids:?}");
    }

    #[test]
    fn list_evidence_sources_for_dispute_includes_global() {
        let store = TtlStore::open_in_memory().unwrap();
        // 全局共享源（ontology_api_name=''，既有测试同款注册方式）应命中
        let src = store
            .register_evidence_source("document", "全局文档", "内容甲。", "", "")
            .unwrap();
        // 他本体的源不应命中
        store
            .register_evidence_source("document", "他本体的文档", "内容乙。", "https://other.org#", "")
            .unwrap();
        let list = store
            .list_evidence_sources_for_dispute("https://example.org/d#")
            .unwrap();
        assert_eq!(list.len(), 1, "只含全局源: {:?}", list.iter().map(|s| &s.1).collect::<Vec<_>>());
        assert_eq!(list[0].0, src);
        assert_eq!(list[0].2, "内容甲。");
    }

    #[test]
    fn dispute_requires_registered_evidence_source() {
        let store = TtlStore::open_in_memory().unwrap();
        seed_issue(&store, "vi-d1", "https://example.org/d", "warning", "open");
        // 未登记的源 → 第①步直接驳回
        let r = store
            .dispute_issue("vi-d1", "理由", "src-none", "原文")
            .unwrap();
        assert!(r.contains("未登记"), "应提示源未登记: {r}");
        let status: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status FROM validation_issues WHERE id='vi-d1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "open", "答辩失败不应改状态");
    }

    #[test]
    fn dispute_mechanical_match_failure_rejects() {
        let store = TtlStore::open_in_memory().unwrap();
        // 登记源
        let src = store
            .register_evidence_source("document", "测试文档", "供应商编号全局唯一。", "", "")
            .unwrap();
        seed_issue(&store, "vi-d2", "https://example.org/d", "warning", "open");
        // quote 是改写 → 匹配失败
        let r = store
            .dispute_issue("vi-d2", "理由", &src, "编号是唯一的")
            .unwrap();
        assert!(r.contains("匹配失败"), "改写 quote 应驳回: {r}");
    }

    #[test]
    fn dispute_success_then_resolve_overruled() {
        let store = TtlStore::open_in_memory().unwrap();
        let src = store
            .register_evidence_source("document", "测试文档", "供应商编号全局唯一。", "", "")
            .unwrap();
        seed_issue(&store, "vi-d3", "https://example.org/d", "warning", "open");
        let r = store
            .dispute_issue("vi-d3", "引用原文献策", &src, "供应商编号全局唯一")
            .unwrap();
        assert_eq!(r, "disputed");
        // A 不能自行裁决（工具层不暴露 resolve_dispute；这里直接调用模拟 B 复核）
        let verdict = store.resolve_dispute("vi-d3", "overruled", "verifier").unwrap();
        assert_eq!(verdict, "overruled");
        let status: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status FROM validation_issues WHERE id='vi-d3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "overruled");
    }

    #[test]
    fn resolve_requires_disputed_state() {
        let store = TtlStore::open_in_memory().unwrap();
        seed_issue(&store, "vi-d4", "https://example.org/d", "warning", "open");
        let r = store.resolve_dispute("vi-d4", "overruled", "verifier").unwrap();
        assert!(r.contains("只有 disputed"), "非 disputed 不可裁决: {r}");
    }

    /// A 接受 warning（open 直接转 accepted，不走裁决）：run_auto_dispute 的 accept
    /// 分支调用，使 issue 状态流转（此前 A 接受但 status 维持 open 是 bug）。
    #[test]
    fn accept_issue_transitions_open_to_accepted() {
        let store = TtlStore::open_in_memory().unwrap();
        seed_issue(&store, "vi-acc", "https://example.org/a", "warning", "open");
        let r = store.accept_issue("vi-acc").unwrap();
        assert_eq!(r, "accepted");
        // 落库验证
        let issues = store.list_issues("https://example.org/a#").unwrap();
        let it = issues.iter().find(|i| i.id == "vi-acc").unwrap();
        assert_eq!(it.status, IssueStatus::Accepted);
        // accept_issue 不写 dispute_argument（A 接受、无辩护理由），dispute 为 None。
        // resolved_by/verdict 写在 DB 列里，仲裁回读时由 dispute 字段携带——此处
        // 仅验证状态流转，不强制 dispute 非空（那是 disputed 分支的事）。
        // 不存在的 issue
        let missing = store.accept_issue("no-such").unwrap();
        assert!(missing.contains("不存在"));
    }

    /// 批量收敛：open 的 warning → accepted，跳过 disputed（答辩中不能强行覆盖）。
    /// run_auto_dispute 收尾调用，处理 rejected_match/error 卡在 open 的 warning。
    #[test]
    fn accept_open_warnings_skips_disputed() {
        let store = TtlStore::open_in_memory().unwrap();
        seed_issue(&store, "vi-o1", "https://example.org/b", "warning", "open");
        seed_issue(&store, "vi-o2", "https://example.org/b", "warning", "open");
        seed_issue(&store, "vi-d1", "https://example.org/b", "warning", "disputed");
        seed_issue(&store, "vi-e1", "https://example.org/b", "error", "open");
        let n = store.accept_open_warnings("https://example.org/b#").unwrap();
        // 只收 2 条 open warning（error 的不收、disputed 的跳过）
        assert_eq!(n, 2);
        let issues = store.list_issues("https://example.org/b#").unwrap();
        let by = |id: &str| issues.iter().find(|i| i.id == id).unwrap();
        assert_eq!(by("vi-o1").status, IssueStatus::Accepted);
        assert_eq!(by("vi-o2").status, IssueStatus::Accepted);
        // disputed 不被覆盖
        assert_eq!(by("vi-d1").status, IssueStatus::Disputed);
        // error 的 open 不收（只收 warning）
        assert_eq!(by("vi-e1").status, IssueStatus::Open);
    }



    /// 评审收敛 → A 修订稿写回本体表（overwrite）：B 复核 confirm 后属性界面
    /// 应看到新 schema。此前修订稿只存 validation_reviews.a_revision 不落盘。
    #[test]
    fn review_resolved_writes_a_revision_to_ontology_table() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/wb1";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        // 原稿有 :Supplier，修订稿改成 :Vendor（验证写回的是修订稿不是原稿）
        let revised = clean_ttl(iri).replace("Supplier", "Vendor");
        let review = store.review_ontology(&format!("{iri}#")).unwrap();
        store.a_revise(&review.id, &revised, "把 Supplier 改名 Vendor").unwrap();
        let after = store.b_recheck(&review.id, true, r#"{"verdict":"confirm"}"#).unwrap();
        assert_eq!(after.status, "resolved");
        // 本体表应已 overwrite 为修订稿
        let exported = store.export_ttl(&format!("{iri}#")).unwrap();
        assert!(exported.contains("Vendor"), "修订稿应已写回本体表: {exported}");
        assert!(!exported.contains("Supplier"), "原稿的 Supplier 应被覆盖: {exported}");
    }


    #[test]
    fn review_lifecycle_in_progress_to_resolved() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/r1";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let review = store.review_ontology(&format!("{iri}#")).unwrap();
        assert_eq!(review.status, "in_progress");
        assert_eq!(review.round, 1);
        assert_eq!(review.max_rounds, 3);

        // A 修订 → round 2
        let after_a = store
            .a_revise(&review.id, "Turtle V2 全文", "修订说明（B 不可见）")
            .unwrap();
        assert_eq!(after_a.round, 2);
        assert_eq!(after_a.a_revision, "Turtle V2 全文");
        assert_eq!(after_a.a_reasoning, "修订说明（B 不可见）");

        // B 复核确认 → resolved
        let after_b = store.b_recheck(&review.id, true, "").unwrap();
        assert_eq!(after_b.status, "resolved");
    }

    /// 多轮对抗回环（2026-09-24 实现 §2.2.1：追问未封顶 → 进下一轮而非直接升级）：
    /// B 复核追问且 round < max_rounds → 保持 in_progress、a_revision 清空（前端
    /// 回到「A 修订」环节）；第 2 次追问（round=max_rounds）→ 轮次封顶 escalated。
    #[test]
    fn review_multi_round_question_loops_then_caps() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/mr1";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let review = store.review_ontology(&format!("{iri}#")).unwrap();

        // 第 1 轮：A 修订（round→2）→ B 复核追问 → 未封顶，进下一轮
        store.a_revise(&review.id, "V2", "第 1 轮修订说明").unwrap();
        let q1 = store.b_recheck(&review.id, false, r#"{"verdict":"question"}"#).unwrap();
        assert_eq!(q1.status, "in_progress", "追问未封顶应保持 in_progress 进下一轮");
        assert_eq!(q1.round, 2, "round 保持 2（下次 a_revise 才递增）");
        assert!(q1.b_verdict.contains("question"), "追问判定应落库供下一轮参考");
        assert_eq!(q1.a_revision, "", "a_revision 清空使前端回到「A 修订」环节");

        // 第 2 轮：A 修订（round→3）→ B 复核再追问 → 封顶升级仲裁
        store.a_revise(&review.id, "V3", "第 2 轮修订说明").unwrap();
        let q2 = store.b_recheck(&review.id, false, r#"{"verdict":"question"}"#).unwrap();
        assert_eq!(q2.status, "escalated", "round=max_rounds 再追问应轮次封顶升级仲裁");
        assert!(q2.b_verdict.contains("question"));
    }

    #[test]
    fn review_stale_in_progress_reset_on_new_start() {
        // 残留防护：同一本体旧的 in_progress review（任务中断残留）在启动新评审时
        // 被标记 escalated，避免前端 latest 复用死行卡住
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/stale1";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let first = store.review_ontology(&format!("{iri}#")).unwrap();
        assert_eq!(first.status, "in_progress");
        // 残留：a_revision 空、in_progress（模拟中断）
        // 启动新评审 → 旧行 interrupted（任务已不在，不是分歧未收敛），新行 in_progress
        let second = store.review_ontology(&format!("{iri}#")).unwrap();
        assert_eq!(second.status, "in_progress");
        assert_ne!(second.id, first.id);
        // 旧行被标记 interrupted（latest 是新的 in_progress，旧行查库确认）
        let conn = store.conn.lock().unwrap();
        let old_status: String = conn
            .query_row(
                "SELECT status FROM validation_reviews WHERE id = ?1",
                rusqlite::params![first.id],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(old_status, "interrupted", "残留 in_progress 不得再标 escalated——那会让界面谎报「复核未收敛」");
    }

    #[test]
    fn review_escalate_on_disagreement() {
        // 第 2 轮修订后仍追问（round=max_rounds）→ 封顶升级；
        // 首轮追问不再直接升级（多轮回环，见 review_multi_round_question_loops_then_caps）
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/r2";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let review = store.review_ontology(&format!("{iri}#")).unwrap();
        store.a_revise(&review.id, "V2", "理由").unwrap();
        store.a_revise(&review.id, "V3", "理由2").unwrap();
        let after_b = store.b_recheck(&review.id, false, "").unwrap();
        assert_eq!(after_b.status, "escalated");
        // escalate 幂等
        let again = store.escalate(&review.id, "manual_escalation", "").unwrap();
        assert_eq!(again.status, "escalated");
    }

    #[test]
    fn persist_and_list_issues_roundtrip() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/r3#";
        let issues = vec![
            crate::validation::ValidationIssue::error("L2-1", "L2", "c1", "domain 违反"),
            crate::validation::ValidationIssue::warning("L3-3", "L3", "c2", "外键疑似"),
        ];
        let n = store.persist_issues(iri, "v-rt", &issues).unwrap();
        assert_eq!(n, 2);
        let listed = store.list_issues(iri).unwrap();
        assert_eq!(listed.len(), 2);
        // rules 域 id 入库加本体作用域前缀（全局主键 vs 本体内唯一语义对齐）
        assert_eq!(listed[0].id, format!("{iri}#L2-1"));
        assert_eq!(listed[0].severity.as_str(), "error");
        assert_eq!(listed[1].status, IssueStatus::Open);
        // v0.7：默认来源 rules
        assert_eq!(listed[0].source, "rules");
    }

    /// 回归：两个本体先后 persist 同名规则 id（"L3-3"）不再撞全局主键。
    /// 旧版直接 INSERT 裸规则 id，第二本体报 UNIQUE constraint failed:
    /// validation_issues.id（删本体后的孤儿行同样触发）。
    #[test]
    fn persist_same_rule_id_across_ontologies() {
        let store = TtlStore::open_in_memory().unwrap();
        let issue = || {
            vec![crate::validation::ValidationIssue::error(
                "L3-3",
                "L3",
                "check_l3_x",
                "同名规则 id",
            )]
        };
        let iri_a = "https://example.org/onta#";
        let iri_b = "https://example.org/ontb#";
        store.persist_issues(iri_a, "v1", &issue()).unwrap();
        store.persist_issues(iri_b, "v2", &issue()).unwrap();
        assert_eq!(
            store.list_issues(iri_a).unwrap()[0].id,
            format!("{iri_a}#L3-3")
        );
        assert_eq!(
            store.list_issues(iri_b).unwrap()[0].id,
            format!("{iri_b}#L3-3")
        );
    }

    // ── 统一发现池 v0.7 ──────────────────────────────────────────

    fn clean_ttl2(iri: &str) -> String {
        format!(
            r#"@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <{iri}#> .
<{iri}#> a owl:Ontology ; rdfs:label "测试"@zh .
:Supplier a owl:Class ; rdfs:label "供应商"@zh ; rdfs:comment "x" .
"#
        )
    }

    /// B 报告落库 → review 域 issue 行转写（含 severity 映射与 id 缺失兜底）。
    #[test]
    fn save_b_report_translates_to_review_issues() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/pool1#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let b_report = r#"[
            {"id":"B1","iri":"https://e/OrderExpress","dimension":"完整性","type":"missing","severity":"error","message":"缺 owl:Class","evidence_ref":"L2.5 列族","suggestion":"补类"},
            {"iri":"https://e/Lead","dimension":"语义正确性","severity":"warning","message":"边界模糊","evidence_ref":"charter","suggestion":"拆分"},
            {"id":"B3","dimension":"结构合理性","severity":"奇怪级别","message":"自由建议","evidence_ref":"","suggestion":""}
        ]"#;
        store.save_b_report(&review.id, b_report).unwrap();

        let listed = store.list_issues(iri).unwrap();
        let review_issues: Vec<_> = listed.iter().filter(|i| i.source == "review").collect();
        assert_eq!(review_issues.len(), 3, "B 报告 3 条全部转写");
        assert_eq!(review_issues[0].id, format!("{}:B1", review.id));
        assert_eq!(review_issues[0].severity.as_str(), "error");
        assert_eq!(review_issues[0].layer, "L4");
        assert_eq!(review_issues[0].rule, "review:完整性");
        assert_eq!(review_issues[0].target.as_deref(), Some("https://e/OrderExpress"));
        // id 缺失 → 按数组序兜底 B{idx+1}（第二条 idx=1 → B2）
        assert_eq!(review_issues[1].id, format!("{}:B2", review.id));
        // severity 未知 → suggestion
        assert_eq!(review_issues[2].severity.as_str(), "suggestion");
    }

    /// A 修订回填 a_action：accepted → accepted 状态；rejected → disputed（进仲裁队列）。
    /// 覆盖 A 回显 B 报告 id（B1）与 A 自造编号（含实体名）两种形态。
    #[test]
    fn a_revise_backfills_a_action_and_disputes_rejected() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/pool2#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        store
            .save_b_report(
                &review.id,
                r#"[{"id":"B1","dimension":"完整性","severity":"warning","message":"缺类","suggestion":"补"},{"id":"B2","dimension":"语义","severity":"warning","message":"边界","suggestion":"拆"}]"#,
            )
            .unwrap();

        // A 修订说明：B1 接受（回显 id）；B2 拒绝（A 自造编号 "issue-2_OrderExpress 缺类声明"——
        // 含 "B2"？不——自造编号不含 bid。这里模拟真实形态：A 用自造编号引用 B2。
        // 后缀匹配策略：raw_id 含 bid 或 bid 含 raw_id。自造编号 "issue-2_..." 不含
        // "B2" → 无法匹配。真实修复靠 B prompt 要求 A 回显 id；此处测试可匹配形态。
        let reasoning = r#"[{"issue_id":"B1","action":"accepted","reason":"采纳"},{"issue_id":"B2","action":"rejected","reason":"静态目录本体，无需时间维度"}]"#;
        store.a_revise(&review.id, "TTL V2", reasoning).unwrap();

        let listed = store.list_issues(iri).unwrap();
        let b1 = listed.iter().find(|i| i.id.ends_with(":B1")).unwrap();
        assert_eq!(b1.status, IssueStatus::Accepted);
        assert!(b1.a_action.is_some());
        assert_eq!(b1.a_action.as_ref().unwrap().action, "accepted");
        let b2 = listed.iter().find(|i| i.id.ends_with(":B2")).unwrap();
        assert_eq!(b2.status, IssueStatus::Disputed, "A 拒绝的进仲裁队列");
        assert_eq!(b2.a_action.as_ref().unwrap().reason, "静态目录本体，无需时间维度");
    }

    /// 分域替换：重跑规则检查不得冲掉 review 域 B 发现行。
    #[test]
    fn persist_rules_snapshot_keeps_review_rows() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/pool3#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        // 规则快照 v1：一条 error
        store
            .persist_issues(iri, "v1", &[crate::validation::ValidationIssue::error(
                "L2-1", "L2", "check_x", "旧 error",
            )])
            .unwrap();
        // B 评审发现落库
        let review = store.review_ontology(iri).unwrap();
        store
            .save_b_report(
                &review.id,
                r#"[{"id":"B1","dimension":"完整性","severity":"warning","message":"B 发现","suggestion":"改"}]"#,
            )
            .unwrap();
        // 重跑规则检查（v2 快照）
        store
            .persist_issues(iri, "v2", &[crate::validation::ValidationIssue::warning(
                "L3-9", "L3", "check_y", "新 warning",
            )])
            .unwrap();

        let listed = store.list_issues(iri).unwrap();
        let review_rows: Vec<_> = listed.iter().filter(|i| i.source == "review").collect();
        assert_eq!(review_rows.len(), 1, "规则重跑不清 review 域");
        let rule_rows: Vec<_> = listed.iter().filter(|i| i.source == "rules").collect();
        assert_eq!(rule_rows.len(), 1, "rules 域整替（v1 error 被替换为 v2 warning）");
        assert!(rule_rows[0].message.contains("新 warning"));
    }

    /// 真实脏数据形态：a_reasoning 带 `---` 分隔残留 + ```json 围栏（2026-09-23
    /// 实测 glm-5.3-flash 输出），extract_json_array 必须能剥掉；A 用自造编号
    /// （"issue-2_xxx"）引用 B 条目时靠后缀包含匹配不上——验证回显 id 形态闭环。
    #[test]
    fn a_action_parses_fenced_json_from_real_output() {
        // 2026-09-23 真实评审数据形态（verbatim 结构）
        let dirty = "---\n\n```json\n[\n  {\n    \"issue_id\": \"B1\",\n    \"action\": \"accepted\",\n    \"reason\": \"采纳\"\n  }\n]\n```";
        let extracted = extract_json_array(dirty).unwrap();
        let items: Vec<serde_json::Value> = serde_json::from_str(extracted).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["issue_id"], "B1");
        // 纯围栏无 --- 前缀
        let fenced = "```json\n[{\"issue_id\":\"B2\",\"action\":\"rejected\",\"reason\":\"r\"}]\n```";
        assert!(extract_json_array(fenced).is_some());
        // 无数组 → None
        assert!(extract_json_array("没有 JSON 的文本").is_none());
    }

    /// b_recheck 落库 B 复核原始输出 + 环节耗时；escalated review 可回读 b_verdict。
    /// step_ms 是**分段**耗时（以 updated_at 为锚：每环节只计自上一动作以来的时长，
    /// 非从评审创建起的累计值——2026-09-24 修复，真机实测复核卡曾把三步总时长
    /// 24m47s 显示成 B 复核耗时）。多轮时同角色跨轮 += 累计。
    #[test]
    fn b_recheck_persists_verdict_json_and_step_ms() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/pool4#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let gap = std::time::Duration::from_millis(40);
        std::thread::sleep(gap);
        store.save_b_report(&review.id, "[]").unwrap();
        std::thread::sleep(gap);
        let after_a = store.a_revise(&review.id, "V2", "[]").unwrap();
        // 修订分段耗时 ≈ 一次 gap；累计 bug 下会是 2×gap
        assert!(
            after_a.step_ms.a_revise < after_a.step_ms.b_review * 2,
            "a_revise 应只计修订环节自身耗时，非累计值（a={} b={}）",
            after_a.step_ms.a_revise,
            after_a.step_ms.b_review
        );
        std::thread::sleep(gap);
        // 连续两轮追问走多轮回环：第 1 次进下一轮，第 2 次封顶 escalated
        let verdict_q = r#"{ "verdict": "question", "remaining_issues": [{"suggestion_id":"B1","status":"unresolved","message":"未落实"}] }"#;
        let q1 = store.b_recheck(&review.id, false, verdict_q).unwrap();
        assert_eq!(q1.status, "in_progress", "追问未封顶进下一轮");
        assert!(q1.b_verdict.contains("B1"));
        std::thread::sleep(gap);
        store.a_revise(&review.id, "V3", "[]").unwrap();
        std::thread::sleep(gap);
        let after = store.b_recheck(&review.id, false, verdict_q).unwrap();
        assert_eq!(after.status, "escalated", "封顶追问升级仲裁");
        assert!(after.b_verdict.contains("question"));
        assert!(after.step_ms.b_review > 0, "环节耗时应已记录");
        assert!(after.step_ms.a_revise > 0);
        assert!(after.step_ms.b_recheck > 0);
        // 复核分段耗时 < 前两环节之和；累计 bug 下 step_ms_re ≈ 三步总和
        assert!(
            after.step_ms.b_recheck < after.step_ms.b_review + after.step_ms.a_revise,
            "b_recheck 应只计复核环节自身耗时（re={} b={} a={}）",
            after.step_ms.b_recheck,
            after.step_ms.b_review,
            after.step_ms.a_revise
        );
        // latest_review 同样可读
        let latest = store.latest_review(iri).unwrap().unwrap();
        assert!(latest.b_verdict.contains("remaining_issues"));
        assert!(latest.step_ms.b_recheck > 0);
    }

    // ── 合并方案 v0.8：答辩并入语义评审 ──
    // A 修订对 rules 域 warning 表态：rejected 无证据 → 按接受收敛（告警成立）
    #[test]
    fn apply_a_actions_rules_rejected_without_evidence_converges() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/merge1#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        seed_issue(&store, "vi-rw1", "https://example.org/merge1", "warning", "open");
        // A 拒绝但没带证据字段
        let reasoning = r#"[{"issue_id":"R1","action":"rejected","reason":"我认为误判"}]"#;
        // 直接调内部方法（a_revise 会走 review 域匹配失败 + rules 域 R1 映射）
        store.a_revise(&review.id, "V2", reasoning).unwrap();
        let status: String = store
            .conn.lock().unwrap()
            .query_row("SELECT status FROM validation_issues WHERE id='vi-rw1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "accepted", "无证据异议按接受收敛: {status}");
    }

    // A 修订对 rules 域 warning 表态：rejected 带证据且机械匹配通过 → disputed + 反证字段
    #[test]
    fn apply_a_actions_rules_rejected_mechanical_match_pass_disputes() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/merge2#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let src = store
            .register_evidence_source("document", "测试文档", "供应商编号全局唯一。", "", "")
            .unwrap();
        seed_issue(&store, "vi-rw2", "https://example.org/merge2", "warning", "open");
        let reasoning = format!(
            r#"[{{"issue_id":"R1","action":"rejected","reason":"输入明示","source_id":"{src}","quote":"供应商编号全局唯一","argument":"原文献策"}}]"#
        );
        store.a_revise(&review.id, "V2", &reasoning).unwrap();
        let (status, argument, quote): (String, Option<String>, Option<String>) = store
            .conn.lock().unwrap()
            .query_row(
                "SELECT status, dispute_argument, dispute_counter_quote FROM validation_issues WHERE id='vi-rw2'",
                [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "disputed", "机械匹配通过 → 异议成立: {status}");
        assert_eq!(argument.as_deref(), Some("原文献策"));
        assert_eq!(quote.as_deref(), Some("供应商编号全局唯一"));
    }

    // A 修订对 rules 域 warning 表态：rejected 带证据但 quote 改写（机械匹配失败）→ accepted
    #[test]
    fn apply_a_actions_rules_rejected_mechanical_match_fail_converges() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/merge3#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let src = store
            .register_evidence_source("document", "测试文档", "供应商编号全局唯一。", "", "")
            .unwrap();
        seed_issue(&store, "vi-rw3", "https://example.org/merge3", "warning", "open");
        let reasoning = format!(
            r#"[{{"issue_id":"R1","action":"rejected","reason":"改写","source_id":"{src}","quote":"编号是唯一的","argument":"r"}}]"#
        );
        store.a_revise(&review.id, "V2", &reasoning).unwrap();
        let status: String = store
            .conn.lock().unwrap()
            .query_row("SELECT status FROM validation_issues WHERE id='vi-rw3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "accepted", "机械匹配失败按接受收敛: {status}");
    }

    // a_revise 落轮次快照：review_rounds 有行、统计正确（发现/已修改/异议）
    #[test]
    fn a_revise_writes_review_round_snapshot() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/merge4#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        // B 评审发现 1 条（review 域）
        store
            .save_b_report(
                &review.id,
                r#"[{"id":"B1","dimension":"完整性","severity":"warning","message":"B 发现","suggestion":"改"}]"#,
            )
            .unwrap();
        // 规则域 open warning 1 条
        seed_issue(&store, "vi-rw4", "https://example.org/merge4", "warning", "open");
        // A 修订：B1 接受、R1 异议（无证据 → 收敛 accepted，仍算 disputed 之前的状态）
        let reasoning = r#"[{"issue_id":"B1","action":"accepted","reason":"采纳"},{"issue_id":"R1","action":"accepted","reason":"接受"}]"#;
        store.a_revise(&review.id, "V2", reasoning).unwrap();
        let rounds = store.list_review_rounds(&review.id).unwrap();
        assert_eq!(rounds.len(), 1, "应有一轮快照");
        let r = &rounds[0];
        assert_eq!(r.round, 1);
        assert_eq!(r.found_count, 2, "review 1 + rules 1");
        assert_eq!(r.accepted_count, 2, "B1+R1 都接受");
        assert_eq!(r.disputed_count, 0);
        // 快照 JSON 可解析
        let snapshot: serde_json::Value = serde_json::from_str(&r.snapshot_json).unwrap();
        assert_eq!(snapshot.as_array().map(|a| a.len()), Some(2));
    }

    // 路线 B（2026-11-03 裁定）：B 复核**不裁决异议**。即便复核输出带着 dispute_rulings，
    // 状态机也必须忽略——编排层从未把异议清单交给 B，让它对看不见的东西下判决是错的。
    // 异议一律保持 disputed，由阶段③人工仲裁受理。
    #[test]
    fn b_recheck_ignores_verifier_dispute_rulings() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/merge5#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let src = store
            .register_evidence_source("document", "测试文档", "供应商编号全局唯一。", "", "")
            .unwrap();
        seed_issue(&store, "vi-rw5", "https://example.org/merge5", "warning", "open");
        // A 异议（机械匹配通过 → disputed）
        let reasoning = format!(
            r#"[{{"issue_id":"R1","action":"rejected","reason":"输入明示","source_id":"{src}","quote":"供应商编号全局唯一","argument":"原文献策"}}]"#
        );
        store.a_revise(&review.id, "V2", &reasoning).unwrap();
        // 复核判 confirm，同时"裁决"该异议 overruled
        let verdict = r#"{"verdict":"confirm","remaining_issues":[],"dispute_rulings":[{"issue_id":"vi-rw5","ruling":"overruled","reason":"证据成立"}]}"#;
        let after = store.b_recheck(&review.id, true, verdict).unwrap();
        // 产物判定照常生效……
        assert_eq!(after.status, "resolved");
        // ……但异议裁决不得生效
        let (status, verdict_col): (String, Option<String>) = store
            .conn.lock().unwrap()
            .query_row(
                "SELECT status, dispute_verdict FROM validation_issues WHERE id='vi-rw5'",
                [], |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "disputed", "B 的裁决不得生效——异议归人工仲裁");
        assert_eq!(verdict_col, None, "不得写入 verifier 的裁决痕迹");
        let rounds = store.list_review_rounds(&review.id).unwrap();
        assert_eq!(rounds[0].verdict.as_deref(), Some("resolved"));
    }

    /// accepted（旧语义：驳回异议并升级 error）与 uncertain 同样被忽略。
    #[test]
    fn b_recheck_leaves_all_disputes_for_human_arbitration() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/merge6#";
        let _ = store.import_ttl(&clean_ttl2(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let src = store
            .register_evidence_source("document", "测试文档", "供应商编号全局唯一。", "", "")
            .unwrap();
        seed_issue(&store, "vi-rw6", "https://example.org/merge6", "warning", "open");
        seed_issue(&store, "vi-rw7", "https://example.org/merge6", "warning", "open");
        let reasoning = format!(
            r#"[{{"issue_id":"R1","action":"rejected","reason":"r1","source_id":"{src}","quote":"供应商编号全局唯一","argument":"a1"}},
                {{"issue_id":"R2","action":"rejected","reason":"r2","source_id":"{src}","quote":"供应商编号全局唯一","argument":"a2"}}]"#
        );
        store.a_revise(&review.id, "V2", &reasoning).unwrap();
        let verdict = r#"{"verdict":"question","remaining_issues":[],"dispute_rulings":[
            {"issue_id":"vi-rw6","ruling":"accepted","reason":"异议不成立"},
            {"issue_id":"vi-rw7","ruling":"uncertain","reason":"判不定"}]}"#;
        store.b_recheck(&review.id, false, verdict).unwrap();
        let s6: String = store.conn.lock().unwrap()
            .query_row("SELECT status FROM validation_issues WHERE id='vi-rw6'", [], |r| r.get(0)).unwrap();
        let s7: String = store.conn.lock().unwrap()
            .query_row("SELECT status FROM validation_issues WHERE id='vi-rw7'", [], |r| r.get(0)).unwrap();
        assert_eq!(s6, "disputed", "accepted 裁决不得自动升级 error");
        assert_eq!(s7, "disputed", "uncertain 同样保持 disputed");
    }

    /// 无主 in_progress 才回收；有活任务认领的不动。空注册表 = 全是僵尸。
    #[test]
    fn reap_interrupted_marks_only_orphans() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/reap1#";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let _first = store.review_ontology(iri).unwrap();
        let second = store.review_ontology(iri).unwrap();
        assert_eq!(second.status, "in_progress");

        // 注册表里有它 → 不动
        let n = store.reap_interrupted(&[second.id.clone()]).unwrap();
        assert_eq!(n, 0, "有活任务认领的不应被回收");
        assert_eq!(
            store.latest_review(iri).unwrap().unwrap().status,
            "in_progress"
        );

        // 注册表空（进程重启后的状态）→ 全部标中断
        let n2 = store.reap_interrupted(&[]).unwrap();
        assert_eq!(n2, 1);
        let after = store.latest_review(iri).unwrap().unwrap();
        assert_eq!(after.status, "interrupted", "无主评审必须脱离 in_progress");
        assert_eq!(after.outcome_reason, "interrupted");
    }

    /// review_ontology 的残留重置语义：in_progress → **interrupted**，不是 escalated。
    /// 这是"界面谎报复核未收敛"的根因修复点。
    #[test]
    fn review_ontology_resets_stale_as_interrupted() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/reap2#";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let stale = store.review_ontology(iri).unwrap();
        let fresh = store.review_ontology(iri).unwrap();
        let n = store.reap_interrupted(&[fresh.id.clone()]).unwrap();
        assert_eq!(n, 0, "fresh 有任务，stale 已被 review_ontology 重置过");
        let after = store.latest_review(iri).unwrap().unwrap();
        assert_eq!(after.id, fresh.id);
        assert_eq!(after.status, "in_progress");
        // 直接查残留那一行的状态
        let raw: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT status FROM validation_reviews WHERE id = ?1",
                rusqlite::params![stale.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, "interrupted", "残留 in_progress 应标中断而非 escalated");
    }

    /// 写回冲突：评审期间草稿又被改了一版 → confirm 不得覆盖用户改动，
    /// 修订稿保留在 a_revision 里，归因写 write_back_conflict。
    #[test]
    fn review_resolved_refuses_write_back_when_draft_changed() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/wb2";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let review = store.review_ontology(&format!("{iri}#")).unwrap();
        let revised = clean_ttl(iri).replace("Supplier", "Vendor");
        store.a_revise(&review.id, &revised, "把 Supplier 改名 Vendor").unwrap();

        // 评审跑着的时候，用户手工改了草稿（Supplier → Buyer）
        let user_edit = clean_ttl(iri).replace("Supplier", "Buyer");
        store.import_ttl(&user_edit, true).unwrap();

        let after = store
            .b_recheck(&review.id, true, r#"{"verdict":"confirm"}"#)
            .unwrap();
        assert_eq!(after.status, "resolved", "B 确实认可了产物");
        assert_eq!(after.outcome_reason, "write_back_conflict");
        let exported = store.export_ttl(&format!("{iri}#")).unwrap();
        assert!(exported.contains("Buyer"), "用户后续改动不得被覆盖: {exported}");
        assert!(!exported.contains("Vendor"), "修订稿不得被写回: {exported}");
        assert!(
            after.a_revision.contains("Vendor"),
            "修订稿必须保留供用户取用: {}",
            after.a_revision
        );
    }

    /// 基线一致时正常写回（对照组，防把冲突判定做成"永远拒绝"）。
    #[test]
    fn review_resolved_writes_back_when_baseline_unchanged() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/wb3";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let review = store.review_ontology(&format!("{iri}#")).unwrap();
        let revised = clean_ttl(iri).replace("Supplier", "Vendor");
        store.a_revise(&review.id, &revised, "改名").unwrap();
        let after = store
            .b_recheck(&review.id, true, r#"{"verdict":"confirm"}"#)
            .unwrap();
        assert_eq!(after.status, "resolved");
        assert_eq!(after.outcome_reason, "", "无异常时归因为空串");
        let exported = store.export_ttl(&format!("{iri}#")).unwrap();
        assert!(exported.contains("Vendor"), "基线未变应正常写回: {exported}");
    }

    /// escalate 带归因与错误文本；a_revise 不再推进已结束的评审。
    #[test]
    fn escalate_records_reason_and_a_revise_refuses_finished() {
        let store = TtlStore::open_in_memory().unwrap();
        let iri = "https://example.org/esc1#";
        let _ = store.import_ttl(&clean_ttl(iri), true).unwrap();
        let review = store.review_ontology(iri).unwrap();
        let after = store
            .escalate(&review.id, "step_failed", "provider 超时")
            .unwrap();
        assert_eq!(after.status, "escalated");
        assert_eq!(after.outcome_reason, "step_failed");
        assert_eq!(after.outcome_detail, "provider 超时");

        // 已结束的评审上继续 a_revise 应被拒绝（不推进 round、不落产物）
        let refused = store.a_revise(&review.id, "V2", "说明").unwrap();
        assert_eq!(refused.status, "escalated");
        assert_eq!(refused.round, 1, "已结束的评审不应推进轮次");
        assert!(refused.a_revision.is_empty(), "不应落修订稿");
    }
}

