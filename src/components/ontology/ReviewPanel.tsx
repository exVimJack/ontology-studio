// ReviewPanel.tsx：多轮语义评审对抗面板（VALIDATION-DESIGN.md §2.2.1）
// 后端驱动轮转：B 评审 → A 修订 → B 复核，每步落库；前端只轮询 review 展示。
//
// v0.7 统一发现池：B 评审发现同时落 validation_issues（source=review），A 修订时
// 逐条回填 accepted/rejected——本面板在 B/A 卡片之外，以「B 建议 → A 回应」
// 逐条卡片呈现双方博弈（数据直接来自 issue 列表，不再手工解析 JSON）。
//
// 进度恢复哲学（P0-1 修复）：runningRef 只在「本组件存活期间」区分计时展示——
// 后端任务不随组件卸载而中断，切页回来轮询到 in_progress 即恢复 active 展示，
// 不再误判「上次会话中断」。elapsed 由 step_ms/updated_at 推导，不依赖本地计时器。

import { useEffect, useMemo, useState } from "react";
import {
  useLatestValidationReview,
  useValidationIssues,
  type ValidationIssue,
  type ValidationReview,
} from "@/hooks/useOntologyValidation";
import styles from "./ValidationPanel.module.css";
import own from "./ReviewPanel.module.css";

type Props = {
  ontologyIri: string | null;
  /** 启动入口已上移至子流程条（ReviewLaunchButton），面板只读轮询展示 */
  modelA: string;
  modelB: string;
  onEscalated?: () => void;
};

const STATUS_TEXT: Record<string, string> = {
  in_progress: "进行中",
  resolved: "已通过",
  escalated: "已转人工仲裁",
};

type Step = "b_review" | "a_revise" | "b_recheck";

function fmtMs(ms: number): string {
  if (!ms) return "";
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec} 秒`;
  return `${Math.floor(sec / 60)} 分 ${sec % 60} 秒`;
}

/** 从 review 状态推导当前环节（后端驱动进度，非本地状态机）。
 *  多轮对抗：B 复核追问未封顶时 store 清 a_revision 保持 in_progress——
 *  此时 b_report 已有、a_revision 空 → 回到「A 修订」环节（下一轮修订中）。 */
function deriveStep(review: ValidationReview): Step | null {
  if (review.status !== "in_progress") return null;
  if (review.a_revision) return "b_recheck";
  if (review.b_report) return "a_revise";
  return "b_review";
}

function RoundCard({
  role,
  model,
  status,
  time,
  children,
}: {
  role: string;
  model: string;
  status: "done" | "active" | "wait";
  time?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className={own.roundCard}>
      <div className={own.roundHead}>
        <span className={own.roundRole}>{role}</span>
        <span className={own.roundModel}>{model}</span>
        {time && <span className={own.roundModel}>⏱ {time}</span>}
        <span className={own.roundState}>{status === "done" ? "✓ 完成" : status === "active" ? "进行中…" : "等待"}</span>
      </div>
      {children && <div className={own.roundBody}>{children}</div>}
    </div>
  );
}

/** B 建议 → A 回应 逐条卡片（P1-2：数据驱动，不再手工解析 JSON）。
 * 数据来自 validation_issues 的 review 域行——B 报告落库时转写、A 修订时回填。 */
function SuggestionPairCard({ issue }: { issue: ValidationIssue }) {
  const a = issue.a_action;
  const short = issue.id.split(":").slice(1).join(":") || issue.id;
  return (
    <div className={own.pairCard}>
      <div className={own.pairHead}>
        <span className={own.pairId}>{short}</span>
        <span className={own.pairDim}>{issue.rule.replace(/^review:/, "")}</span>
        <span className={issue.severity === "error" ? styles.sevError : styles.sevWarning}>
          {issue.severity}
        </span>
        {issue.target && <span className={own.pairTarget}>{issue.target.split(/[#/]/).pop()}</span>}
      </div>
      <div className={own.pairB}>
        <b>B：</b>
        {issue.message}
      </div>
      {issue.suggestion && (
        <div className={own.pairSug}>
          <b>建议：</b>
          {issue.suggestion}
        </div>
      )}
      {a ? (
        <div className={a.action === "rejected" ? own.pairARej : own.pairAAcc}>
          <b>A（{a.action === "rejected" ? "拒绝" : "接受"}）：</b>
          {a.reason || "（无说明）"}
        </div>
      ) : (
        <div className={own.pairWait}>等待 A 回应…</div>
      )}
    </div>
  );
}

export function ReviewPanel({ ontologyIri, modelA, modelB }: Props) {
  const review = useLatestValidationReview(ontologyIri);
  const issues = useValidationIssues(ontologyIri);

  const current = review.data;

  // 当前环节：后端 review 状态推导（P0-1 核心——不再有本地 runningRef 门卫，
  // 切页回来轮询到 in_progress 立即恢复 active）
  const step = current ? deriveStep(current) : null;
  const running = !!step;

  // 环节耗时：优先 review.step_ms（后端落库实测）；进行中环节用 now-created_at 估算
  const [, forceTick] = useState(0);
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => forceTick((x) => x + 1), 1000);
    return () => window.clearInterval(t);
  }, [running]);

  const reviewIssues = useMemo(
    () => (issues.data ?? []).filter((i: ValidationIssue) => i.source === "review"),
    [issues.data],
  );
  const rejectedCount = reviewIssues.filter(
    (i: ValidationIssue) => i.a_action?.action === "rejected",
  ).length;
  const acceptedCount = reviewIssues.filter(
    (i: ValidationIssue) => i.a_action?.action === "accepted",
  ).length;

  const stepTime = (s: Step): string | undefined => {
    if (!current) return undefined;
    const ms =
      s === "b_review"
        ? current.step_ms?.b_review
        : s === "a_revise"
          ? current.step_ms?.a_revise
          : current.step_ms?.b_recheck;
    if (ms && ms > 0) return fmtMs(ms);
    // 进行中环节：从评审创建时刻起估算（后端未落库前无精确分段计时）
    if (step === s && running) {
      const elapsed = Date.now() - current.created_at;
      return `已用时 ${fmtMs(elapsed)}`;
    }
    return undefined;
  };

  // 轮次文案：round 1=首轮评审中；2+ = 第 N-1 轮修订复核中（多轮对抗）
  const roundText = current
    ? current.status === "in_progress"
      ? current.round === 1
        ? "首轮评审中"
        : `第 ${current.round - 1} 轮修订复核中`
      : current.status === "escalated"
        ? "复核未收敛，已升级仲裁"
        : "复核通过"
    : "";

  const roundStatus = (s: Step): "done" | "active" | "wait" => {
    if (step === s) return "active";
    const done =
      s === "b_review"
        ? !!current?.b_report
        : s === "a_revise"
          ? !!current?.a_revision
          : !!current && current.status !== "in_progress";
    return done ? "done" : "wait";
  };

  return (
    <div className={styles.wrap}>
      <div className={styles.head}>
        <div className={styles.title}>语义评审</div>
        <div className={styles.stats}>
          {current && (
            <span className={styles.coverage}>
              {STATUS_TEXT[current.status] ?? current.status} · {roundText}
            </span>
          )}
          <span className={styles.statSug}>A: {modelA || "未配置"}</span>
          <span className={styles.statWarning}>B: {modelB || "未配置"}</span>
        </div>
      </div>

      <div className={styles.issueList}>
        {!current && !step && (
          <div className={styles.hint}>
            未启动评审。点击上方「启动评审」自动运行完整对抗循环：B 评审 → A 修订 → B 复核
            （追问未封顶自动进下一轮，最多 2 轮修订复核；封顶或失败转人工仲裁）。
            全程无需人工介入，各环节耗时会显示在对应卡片上。
          </div>
        )}
        {running && <div className={styles.hint}>自动轮转中，无需人工介入。</div>}
        {!running && current?.status === "escalated" && (
          <div className={styles.hint}>
            上一轮评审复核未收敛，已升级人工仲裁。B 建议与 A 回应保留在下方逐条卡片，
            可到阶段③人工仲裁逐条裁决。
          </div>
        )}

        <RoundCard
          role="校验者 B · 评审"
          model={modelB}
          status={roundStatus("b_review")}
          time={stepTime("b_review")}
        >
          {current?.b_report && (
            <details className={own.fold}>
              <summary className={own.foldSummary}>
                B 评审建议（八维度 JSON，{reviewIssues.length} 条已入发现池）
              </summary>
              <pre className={own.pre}>{current.b_report}</pre>
            </details>
          )}
        </RoundCard>

        <RoundCard
          role="建模者 A · 修订"
          model={modelA}
          status={roundStatus("a_revise")}
          time={stepTime("a_revise")}
        >
          {current?.a_reasoning && (
            <details className={own.fold}>
              <summary className={own.foldSummary}>
                修订说明（B 复核不可见；接受 {acceptedCount} · 拒绝 {rejectedCount}）
              </summary>
              <pre className={own.preSmall}>{current.a_reasoning}</pre>
            </details>
          )}
        </RoundCard>

        <RoundCard
          role="校验者 B · 复核"
          model={modelB}
          status={roundStatus("b_recheck")}
          time={stepTime("b_recheck")}
        >
          {current?.b_verdict && (
            <details className={own.fold}>
              <summary className={own.foldSummary}>复核判定 JSON</summary>
              <pre className={own.pre}>{current.b_verdict}</pre>
            </details>
          )}
          {current && current.status !== "in_progress" && !current.b_verdict && (
            <div className={styles.hint}>
              {current.status === "resolved"
                ? "✓ 复核通过"
                : "复核未收敛，已升级人工仲裁"}
            </div>
          )}
        </RoundCard>

        {/* ── 对抗详情：B 建议 → A 回应 逐条卡片（统一发现池）── */}
        {reviewIssues.length > 0 && (
          <details className={own.fold} open={current?.status === "escalated"}>
            <summary className={own.foldSummary}>
              对抗详情：B 建议 → A 回应（{reviewIssues.length} 条 · 拒绝 {rejectedCount}）
            </summary>
            <div className={own.pairList}>
              {reviewIssues.map((i: ValidationIssue) => (
                <SuggestionPairCard key={i.id} issue={i} />
              ))}
            </div>
          </details>
        )}
      </div>
    </div>
  );
}
