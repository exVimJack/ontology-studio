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
            conn.execute(
                "INSERT INTO validation_issues
                 (id, ontology_iri, validation_id, layer, source, severity, rule, message, target,
                  evidence_source_id, evidence_quote, principle_source, principle_ref, suggestion,
                  status, a_action, dispute_argument, dispute_counter_source, dispute_counter_quote,
                  dispute_resolved_by, dispute_verdict, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                         ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?22)",
                rusqlite::params![
                    issue.id, ontology_iri, validation_id, issue.layer, issue.source,
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
        // 残留 in_progress → escalated（保留审计轨迹，不删行）
        conn.execute(
            "UPDATE validation_reviews
             SET status = 'escalated', updated_at = ?2
             WHERE ontology_iri = ?1 AND status = 'in_progress'",
            rusqlite::params![ontology_iri, now],
        )?;
        let id = format!("vr-{}", uuid::Uuid::new_v4().simple());
        // max_rounds=3：round 1=首轮 B 评审，2/3=两轮修订复核（§2.3 复核 ≤2 轮封顶——
        // 第 2 次复核仍追问才升级仲裁，此前单轮实现是编排层没回环，不是封顶值小）
        conn.execute(
            "INSERT INTO validation_reviews (id, ontology_iri, round, max_rounds, status, created_at, updated_at)
             VALUES (?1, ?2, 1, 3, 'in_progress', ?3, ?3)",
            rusqlite::params![id, ontology_iri, now],
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

    /// 把 A 的修订说明（a_reasoning JSON）逐条回填到 review 域 issue 的
    /// a_action 列（`a_revise` 内部调用）。
    ///
    /// 匹配：a_reasoning 的 issue_id（B 报告条目 id 或 A 自造编号）与 issue id
    /// 的 **B 条目段**（`{review_id}:{B条目id}` 冒号后）做包含式匹配——A 实测
    /// 输出过 "issue-3_OrderExpress 缺类声明与关联" 这类自造编号，精确等值
    /// 匹配会全部落空。多对一时取最长匹配（编号最具体）。
    /// A rejected 的 issue 状态转 disputed——分歧显式化，B 复核不收敛时可
    /// 逐条进入仲裁（此前 escalated review 整体无 UI 家，仲裁面板只能空转）。
    fn apply_a_actions_locked(
        &self,
        conn: &Connection,
        review_id: &str,
        a_reasoning: &str,
    ) -> StoreResult<()> {
        let Some(json_text) = extract_json_array(a_reasoning) else {
            return Ok(()); // 非 JSON——原文保留，不视为错误
        };
        let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(json_text) else {
            return Ok(());
        };
        let now = crate::ttl::now_ms();
        for item in items {
            let Some(raw_id) = item.get("issue_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let action = item.get("action").and_then(|v| v.as_str()).unwrap_or("accepted");
            let reason = item.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            // 找 B 条目段：issue id 形如 "{review_id}:{bid}"，A 可能引用 bid
            //（"B1"）或自造编号。取该 review 的 review 域 issue，对每个 issue
            // 提取 bid 后做双向包含匹配（自造编号往往比 bid 长且含 bid 的实体名）。
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
            // 候选：raw_id 包含 bid 或 bid 包含 raw_id（去空白归一化后）
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
            let Some(target_id) = best else {
                continue; // A 引用了不存在的条目——跳过（不阻断）
            };
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
                rusqlite::params![target_id, a_json, new_status, now],
            )?;
        }
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
                created_at: 0,
                updated_at: 0,
            });
        };
        // 轮次封顶（§3.1 仲裁规则：超限转人工）——round 达到 max_rounds 仍需修订
        // 视为分歧未收敛，review 转 escalated，拒绝继续推进。
        if status == "resolved" || status == "escalated" {
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
                created_at: 0,
                updated_at: 0,
            });
        }
        if round >= max_rounds {
            conn.execute(
                "UPDATE validation_reviews SET status = 'escalated', updated_at = ?2 WHERE id = ?1",
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
        // 统一发现池 v0.7：A 的逐条裁决回填到 review 域 issue 的 a_action 列。
        // a_reasoning JSON 格式：[{"issue_id","action","reason"}]（issue_id 可能是
        // B 报告条目 id "B1" 或 A 自造编号——两种都按后缀匹配）。
        if let Err(e) = self.apply_a_actions_locked(&conn, review_id, a_reasoning) {
            tracing::warn!("a_revise 回填 a_action 失败（a_reasoning 原文已保留）: {e}");
        }
        self.get_review(&conn, review_id).map(|mut r| {
            r.ontology_iri = ontology_iri;
            r
        })
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
            // step_ms_re 以 updated_at 为锚分段计复核环节耗时（2026-09-24 修复：原先
            // ?now - created_at 是累计值，复核卡把三步总时长显示成本环节耗时）
            conn.execute(
                "UPDATE validation_reviews
                 SET status = 'resolved', b_verdict = ?2,
                     step_ms_re = step_ms_re + MAX(0, ?3 - updated_at), updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![review_id, b_verdict_json, now],
            )?;
            let rev_row: Option<(String, String)> = conn
                .query_row(
                    "SELECT ontology_iri, a_revision FROM validation_reviews WHERE id = ?1",
                    rusqlite::params![review_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            if let Some((_ont_iri, revision)) = rev_row {
                if !revision.is_empty() {
                    pending_import = Some(revision);
                }
            }
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
            // 轮次封顶（复核 ≤2 轮）→ 分歧未收敛，转人工仲裁
            conn.execute(
                "UPDATE validation_reviews
                 SET status = 'escalated', b_verdict = ?2,
                     step_ms_re = step_ms_re + MAX(0, ?3 - updated_at), updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![review_id, b_verdict_json, now],
            )?;
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

    /// 转人工仲裁（第三层对抗超限/分歧）：review → escalated（幂等，已在 escalated 则不动）。
    pub fn escalate(&self, review_id: &str) -> StoreResult<ValidationReview> {
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
                created_at: 0,
                updated_at: 0,
            });
        }
        conn.execute(
            "UPDATE validation_reviews SET status = 'escalated', updated_at = ?2 WHERE id = ?1",
            rusqlite::params![review_id, crate::ttl::now_ms()],
        )?;
        self.get_review(&conn, review_id)
    }

    /// 查询本体的最新 review（可为 None）。
    pub fn latest_review(&self, ontology_iri: &str) -> StoreResult<Option<ValidationReview>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ontology_iri, round, max_rounds, b_report, a_revision, a_reasoning, status, created_at, updated_at, b_verdict, step_ms_b, step_ms_a, step_ms_re
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
            "SELECT id, ontology_iri, round, max_rounds, b_report, a_revision, a_reasoning, status, created_at, updated_at, b_verdict, step_ms_b, step_ms_a, step_ms_re
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
        // 启动新评审 → 旧行 escalated，新行 in_progress
        let second = store.review_ontology(&format!("{iri}#")).unwrap();
        assert_eq!(second.status, "in_progress");
        assert_ne!(second.id, first.id);
        // 旧行被标记 escalated（latest 是新的 in_progress，旧行查库确认）
        let conn = store.conn.lock().unwrap();
        let old_status: String = conn
            .query_row(
                "SELECT status FROM validation_reviews WHERE id = ?1",
                rusqlite::params![first.id],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(old_status, "escalated");
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
        let again = store.escalate(&review.id).unwrap();
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
        assert_eq!(listed[0].severity.as_str(), "error");
        assert_eq!(listed[1].status, IssueStatus::Open);
        // v0.7：默认来源 rules
        assert_eq!(listed[0].source, "rules");
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
}
