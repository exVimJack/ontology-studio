// ValidationPanel.tsx：校验任务面板（线框 02 屏 · 主工作台）
// 结构对齐线框：顶部五步状态条（规则检查→覆盖矩阵→B评审→A修订→B复核）
// + 对抗时间线（回合制 A/B/系统）+ Issue 列表（error 只读 / warning 可答辩）。
// 答辩面板独立（线框 04 屏，由 issue 卡片的「答辩」按钮唤起，可下拉选证据源）。

import { useState } from "react";
import {
  useCoverageMatrix,
  useLatestValidationReview,
  useRunValidation,
  useValidationIssues,
  type ValidationIssue,
  layerLabel,
} from "@/hooks/useOntologyValidation";
import styles from "./ValidationPanel.module.css";
import own from "./ValidationPanel.module.css";

type Props = {
  ontologyIri: string | null;
  /** Turtle 全文（运行校验的输入；编辑器当前内容） */
  content?: string;
  onOpenReview: () => void;
  /** 定位到 Turtle 源码行（线框 03 屏：issue → 琥珀高亮对应行） */
  onJumpToSource?: (termIri: string) => void;
};

const STATUS_TEXT: Record<string, string> = {
  open: "open",
  disputed: "disputed",
  accepted: "accepted",
  overruled: "overruled",
};

function Step({
  n,
  label,
  state,
}: {
  n: string;
  label: string;
  state: "done" | "active" | "wait";
}) {
  return (
    <div className={own.step}>
      <span className={`${own.stepCircle} ${styles[`step${state}`]}`}>
        {state === "done" ? "✓" : n}
      </span>
      <span className={`${own.stepLabel} ${styles[`stepLabel${state}`]}`}>{label}</span>
    </div>
  );
}

function StepLine({ done }: { done: boolean }) {
  return <span className={`${own.stepLine} ${done ? own.stepLineDone : ""}`} />;
}

function TimelineItem({
  avatar,
  avatarCls,
  who,
  meta,
  text,
  dim,
}: {
  avatar: string;
  avatarCls: string;
  who: string;
  meta: string;
  text: string;
  dim?: boolean;
}) {
  return (
    <div className={`${own.tlItem} ${dim ? own.tlDim : ""}`}>
      <div className={`${own.tlAvatar} ${avatarCls}`}>{avatar}</div>
      <div className={own.tlBodyMain}>
        <div className={own.tlMeta}>
          <span className={own.tlWho}>{who}</span>
          <span>{meta}</span>
        </div>
        <div className={own.tlText}>{text}</div>
      </div>
    </div>
  );
}

export function ValidationPanel({
  ontologyIri,
  content,
  onOpenReview,
  onJumpToSource,
}: Props) {
  const issues = useValidationIssues(ontologyIri);
  const review = useLatestValidationReview(ontologyIri);
  const runValidation = useRunValidation();
  // 覆盖矩阵不自动计算：点「运行校验」成功后才拉取（用户显式触发原则）
  const [coverageOn, setCoverageOn] = useState(false);
  const coverage = useCoverageMatrix(ontologyIri, [], coverageOn);
  // 校验结果提示条（替代原生 alert；按严重级着色，几秒后自动消失）
  const [resultMsg, setResultMsg] = useState<{ kind: "ok" | "err"; text: string } | null>(
    null,
  );
  const showResult = (kind: "ok" | "err", text: string) => {
    setResultMsg({ kind, text });
    window.setTimeout(() => setResultMsg(null), 6000);
  };

  const errorCount =
    (issues.data ?? []).filter((i: ValidationIssue) => i.severity === "error").length ?? 0;
  const warningCount =
    (issues.data ?? []).filter((i: ValidationIssue) => i.severity === "warning").length ?? 0;
  const suggestionCount =
    (issues.data ?? []).filter((i: ValidationIssue) => i.severity === "suggestion").length ?? 0;
  const done = (issues.data ?? []).filter((i: ValidationIssue) =>
    ["accepted", "overruled"].includes(String(i.status)),
  ).length;

  return (
    <div className={styles.wrap}>
      <div className={styles.toolbar}>
        <span className={styles.toolbarTitle}>{ontologyIri ? "对抗校验" : "校验"}</span>
        <span className={styles.tagLive}>进行中</span>
        <span className={styles.toolbarRight}>
          <button
            type="button"
            className={styles.btnGhostSm}
            disabled={!ontologyIri || !content || runValidation.isPending}
            title={!ontologyIri || !content ? "需要本体 IRI 和 Turtle 内容" : "运行规则检查并持久化 issue 快照"}
            onClick={() => {
              if (!ontologyIri || !content) return;
              runValidation.mutate(
                { ontologyIri, content },
                {
                  onSuccess: (v) => {
                    setCoverageOn(true);
                    showResult(
                      v.errors.length === 0 ? "ok" : "err",
                      `校验完成：triple_count=${v.triple_count}，error ×${v.errors.length} · warning ×${v.warnings.length}`,
                    );
                  },
                  onError: (e) => showResult("err", `校验失败：${(e as Error).message}`),
                },
              );
            }}
          >
            {runValidation.isPending ? "校验中…" : "运行校验"}
          </button>
          <button type="button" className={styles.btnGhostSm} onClick={onOpenReview}>
            跳过语义评审
          </button>
        </span>
      </div>

      {/* 校验结果提示条（替代原生 alert） */}
      {resultMsg && (
        <div
          className={`${own.resultToast} ${
            resultMsg.kind === "ok" ? own.resultOk : own.resultErr
          }`}
        >
          {resultMsg.text}
        </div>
      )}

      {/* 五步状态条（线框 02，数据驱动） */}
      <div className={own.steps}>
        {/* 1 规则检查：issues 已加载即完成（无 error 未闭环才算通过，但面板仅展示进度） */}
        <Step n="1" label="规则检查" state={issues.data ? "done" : "wait"} />
        <StepLine done={!!issues.data} />
        {/* 2 覆盖矩阵：已加载即完成 */}
        <Step n="2" label="覆盖矩阵" state={coverage.data ? "done" : "wait"} />
        <StepLine done={!!coverage.data} />
        {/* 3-5 语义评审对抗：按 review 真实状态推进（未发起=wait，进行中=active，已结束=done） */}
        {!review.data ? (
          <>
            <Step n="3" label="B 评审" state="wait" />
            <StepLine done={false} />
            <Step n="4" label="A 修订" state="wait" />
            <StepLine done={false} />
            <Step n="5" label="B 复核" state="wait" />
          </>
        ) : review.data.status === "in_progress" ? (
          <>
            <Step n="3" label="B 评审" state="active" />
            <StepLine done={false} />
            <Step n="4" label="A 修订" state="wait" />
            <StepLine done={false} />
            <Step n="5" label="B 复核" state="wait" />
          </>
        ) : (
          <>
            <Step n="3" label="B 评审" state="done" />
            <StepLine done />
            <Step n="4" label="A 修订" state="done" />
            <StepLine done />
            <Step n="5" label="B 复核" state="done" />
          </>
        )}
        <span className={own.stepsNote}>
          {issues.data?.length ?? 0} issues · {done} 已闭环
          {review.data && ` · ${review.data.status}`}
        </span>
      </div>

      {/* 对抗时间线（线框 02） */}
      <div className={own.tl}>
        <div className={own.tlHead}>
          <span className={own.tlTitle}>
            对抗时间线<span className={own.tlCount}>（回合制）</span>
          </span>
          <span className={own.tlMeta}>规则检查 → 覆盖矩阵 → B/A 对抗</span>
        </div>
        <div className={own.tlBody}>
          <TimelineItem
            avatar="L2"
            avatarCls={own.tlAvatarSys}
            who="规则引擎"
            meta="· validate"
            text={`规则检查完成：error ×${errorCount} · warning ×${warningCount} · suggestion ×${suggestionCount}。error 直接进入修订列表（B 不参与），warning 可发起答辩。`}
          />
          <TimelineItem
            avatar="M"
            avatarCls={own.tlAvatarSys}
            who="覆盖矩阵"
            meta="· 覆盖矩阵"
            text={`29 句话产物形态检查：${coverage.data ? `${coverage.data.covered_count}/${coverage.data.total_count} 已落地` : "–"}。缺失项进入修订建议。`}
          />
          <TimelineItem
            avatar="B"
            avatarCls={own.tlAvatarVer}
            who="校验者 Verifier"
            meta="· 语义评审"
            text="点击「语义评审」进入多轮对抗（B 评审 → A 修订 → B 复核）。"
            dim
          />
        </div>
      </div>

      {/* Issue 列表 */}
      <div className={own.listHead}>
        <span className={own.listTitle}>Issue 列表</span>
        <span className={own.listHint}>点击卡片展开证据与操作 · warning 可答辩</span>
      </div>
      {issues.isLoading && <div className={styles.hint}>加载中…</div>}
      {issues.isError && (
        <div className={styles.errBox}>{(issues.error as Error).message}</div>
      )}
      {issues.data?.length === 0 && (
        <div className={styles.hint}>暂无校验问题（validate 通过）</div>
      )}
      {issues.data?.map((issue: ValidationIssue) => (
        <IssueCard
          key={issue.id}
          issue={issue}
          onJumpToSource={onJumpToSource}
        />
      ))}
    </div>
  );
}

export function IssueCard({
  issue,
  onJumpToSource,
}: {
  issue: ValidationIssue;
  onJumpToSource?: (termIri: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  // 提取目标 IRI：message 里第一个 :xxx 前缀形式或 <http...> 完整 IRI
  const extractIri = (): string | null => {
    const full = issue.message.match(/<([^>]+)>/);
    if (full) return full[1];
    const prefix = issue.message.match(/:([a-zA-Z][a-zA-Z0-9_]*)/);
    if (prefix) {
      // 有本体 IRI 前缀时拼全 IRI；未知前缀时原样传给定位（后端按含前缀匹配）
      return `:${prefix[1]}`;
    }
    return null;
  };
  const targetIri = extractIri();
  const sevCls =
    issue.severity === "error"
      ? styles.sevError
      : issue.severity === "warning"
        ? styles.sevWarning
        : styles.sevSuggestion;
  const stCls =
    String(issue.status) === "disputed"
      ? styles.stDisputed
      : String(issue.status) === "accepted"
        ? styles.stAccepted
        : String(issue.status) === "overruled"
          ? styles.stOverruled
          : styles.stOpen;
  // 展开区有无内容：无详情字段的 issue 不渲染箭头、不可展开（否则点了像没反应）
  const hasDetail = !!(
    (targetIri && onJumpToSource) ||
    issue.evidence ||
    issue.principle ||
    issue.suggestion ||
    issue.dispute
  );
  return (
    <div className={own.issue}>
      <div
        role={hasDetail ? "button" : undefined}
        tabIndex={hasDetail ? 0 : -1}
        className={own.issueHd}
        onClick={() => hasDetail && setExpanded(!expanded)}
      >
        <span className={`${own.sev} ${sevCls}`}>{issue.severity}</span>
        <span className={own.layer}>{layerLabel(issue.layer)}</span>
        <span className={own.msg}>{issue.message}</span>
        <span className={`${own.status} ${stCls}`}>
          {STATUS_TEXT[String(issue.status)] ?? String(issue.status)}
        </span>
        {hasDetail && <span className={own.arrow}>{expanded ? "▾" : "▸"}</span>}
      </div>
      {expanded && (
        <div className={own.issueBd}>
          {targetIri && onJumpToSource && (
            <div className={styles.btns}>
              <button
                type="button"
                className={styles.btnPrimarySm}
                onClick={() => onJumpToSource(targetIri)}
              >
                定位到 Turtle 行
              </button>
            </div>
          )}
          {issue.evidence && (
            <div className={own.evBox}>
              <div className={own.evSrc}>📄 {issue.evidence.source_id}</div>
              <div className={own.evQuote}>{issue.evidence.quote}</div>
            </div>
          )}
          {issue.principle && (
            <div className={own.prBox}>
              <span className={own.pr}>{issue.principle.source}</span>
              <span>{issue.principle.ref_}</span>
            </div>
          )}
          {issue.suggestion && (
            <div className={own.sugBox}>{issue.suggestion}</div>
          )}
          {issue.dispute && (
            <div className={styles.disputeBox}>
              <div className={own.disputeArg}>
                <b>A 辩护：</b>
                {issue.dispute.argument}
              </div>
              {issue.dispute.verdict && (
                <div className={own.disputeVerdict}>
                  <b>裁决（{issue.dispute.resolved_by}）：</b>
                  {issue.dispute.verdict === "overruled" ? "撤销" : "升级 error"}
                </div>
              )}
            </div>
          )}
          {issue.severity === "error" && (
            <div className={own.errReadonly}>error 不可辩——确定性校验问题，按建议修订后重新 validate</div>
          )}
        </div>
      )}
    </div>
  );
}

