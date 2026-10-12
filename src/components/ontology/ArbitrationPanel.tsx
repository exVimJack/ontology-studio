// ArbitrationPanel.tsx：人工仲裁面板（VALIDATION-DESIGN.md §4.3，线框 06）
// 分歧点进仲裁队列，左右并排辩论视图（A 的主张/证据 vs B 的主张/证据），
// 专家一键裁决：采纳 A / 采纳 B / 稍后处理。
//
// v0.7 统一发现池：两条升级通道在同一队列汇合——
//   ① 答辩升级（②b uncertain）→ disputed 的 rules 域 issue
//   ② 评审升级（②a A rejected / B 复核未收敛）→ disputed 的 review 域 issue
//     （a_action 携带 A 的拒绝理由，b_verdict 携带 B 复核残留问题）
// 此前评审升级的 review 无任何 UI 家（仲裁面板只查 disputed rules issue，
// 永远空转）——桥接后 A 拒绝的 B 建议自动出现在仲裁队列。

import { useEffect, useState } from "react";
import { Loader2, Play } from "lucide-react";
import {
  useValidationIssues,
  useLatestValidationReview,
  useResolveDispute,
  useRunPostArbitrationRevise,
  usePostArbitrationState,
  type ValidationIssue,
  layerLabel,
} from "@/hooks/useOntologyValidation";
import { useProvider } from "@/hooks/useProvider";
import styles from "./ValidationPanel.module.css";
import own from "./ArbitrationPanel.module.css";

type Props = {
  ontologyIri: string | null;
  onResolved: () => void;
};
function DebateView({
  issue,
  bVerdict,
  onVerdict,
  onLater,
  onCustomRevise,
}: {
  issue: ValidationIssue;
  /** B 复核原始输出（escalated review 的残留问题清单；rules 来源为空） */
  bVerdict?: string;
  onVerdict: (issue: ValidationIssue, verdict: string) => void;
  onLater: (issue: ValidationIssue) => void;
  onCustomRevise: (issue: ValidationIssue) => void;
}) {
  const isReview = issue.source === "review";
  // A 的主张：评审升级走 a_action.reason（A 的拒绝理由）；答辩升级走 dispute.argument
  const aClaim = isReview
    ? issue.a_action?.reason || "（A 未说明拒绝理由）"
    : issue.dispute?.argument ?? "（A 未提供辩护理由）";
  const bClaim = issue.message;
  const aEvidence = !isReview && issue.dispute?.counter_evidence.quote
    ? `counterEvidence: ${issue.dispute.counter_evidence.quote}`
    : "（A 无引用证据）";
  const bEvidence = issue.evidence
    ? `证据: ${issue.evidence.quote}`
    : isReview && issue.principle?.ref_
      ? `证据引用: ${issue.principle.ref_}`
      : "（B 未附证据）";

  return (
    <div className={own.arbWrap}>
      <div className={own.arbTitle}>
        {issue.id} · {isReview ? "语义评审" : layerLabel(issue.layer)} ·{" "}
        <span className={styles.sevWarning}>{issue.severity}</span>
      </div>
      {bVerdict && (
        <details className={own.arbVerdictFold} open>
          <summary>B 复核判定（verdict + remaining_issues）</summary>
          <pre className={own.arbVerdictPre}>{bVerdict}</pre>
        </details>
      )}
      <div className={own.arbGrid}>
        <div className={`${own.arbCol} ${own.arbA}`}>
          <div className={own.arbWho}>建模者 A</div>
          <div className={own.arbClaim}>{aClaim}</div>
          <div className={own.arbEv}>{aEvidence}</div>
        </div>
        <div className={`${own.arbCol} ${own.arbB}`}>
          <div className={own.arbWho}>校验者 B</div>
          <div className={own.arbClaim}>{bClaim}</div>
          <div className={own.arbEv}>{bEvidence}</div>
        </div>
      </div>
      <div className={own.arbBtns}>
        <button
          type="button"
          className={own.btnSuccess}
          onClick={() => onVerdict(issue, "overruled")}
        >
          采纳 A
        </button>
        <button
          type="button"
          className={styles.btnPrimary}
          onClick={() => onVerdict(issue, "accepted")}
        >
          采纳 B
        </button>
        <button
          type="button"
          className={styles.btnGhost}
          onClick={() => onCustomRevise(issue)}
        >
          自定义修订…
        </button>
        <button type="button" className={styles.btnGhost} onClick={() => onLater(issue)}>
          稍后处理
        </button>
      </div>
    </div>
  );
}

export function ArbitrationPanel({ ontologyIri, onResolved }: Props) {
  const issues = useValidationIssues(ontologyIri);
  const reviewQ = useLatestValidationReview(ontologyIri);
  const resolve = useResolveDispute();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [customTarget, setCustomTarget] = useState<ValidationIssue | null>(null);
  const [customText, setCustomText] = useState("");
  // 仲裁队列：全部 disputed issue（rules 答辩升级 + review 评审升级，统一发现池）
  const pending: ValidationIssue[] = (issues.data ?? []).filter(
    (i: ValidationIssue) => i.status === "disputed",
  );
  // escalated review 的 B 复核原始输出（评审升级项的补充上下文）
  const bVerdict = reviewQ.data?.status === "escalated" ? reviewQ.data.b_verdict : "";
  const selected: ValidationIssue | null =
    pending.find((i: ValidationIssue) => i.id === selectedId) ?? pending[0] ?? null;

  const doVerdict = (issue: ValidationIssue, verdict: string) => {
    resolve.mutate(
      { issueId: issue.id, verdict, resolvedBy: "human" },
      {
        onSuccess: () => {
          onResolved();
        },
      },
    );
  };

  const doCustomRevise = () => {
    if (!customTarget || !customText.trim()) return;
    // 自定义修订视为裁决记录（verdict=accepted 升级语义由仲裁人自定义文本承载）
    resolve.mutate(
      { issueId: customTarget.id, verdict: "accepted", resolvedBy: "human" },
      {
        onSuccess: () => {
          setCustomTarget(null);
          setCustomText("");
          onResolved();
        },
      },
    );
  };

  // ── 仲裁完成后交 A 修订落盘（后端驱动：spawn + 状态缓存，切窗不丢 + 显示耗时）──
  // 已裁决 issue（accepted 必须改、overruled 不用改）作为 B 建议喂 A，A 修订后
  // import_ttl overwrite 落盘。后端 spawn，前端轮询 usePostArbitrationState 恢复
  // 进行中状态 + 显示已用时——切窗不丢（此前前端 mutateAsync 切窗即丢状态）。
  const provider = useProvider();
  const modelA = provider.data?.model ?? "";
  const postArbRevise = useRunPostArbitrationRevise();
  const postArbState = usePostArbitrationState(ontologyIri);
  const arbRunning = !!postArbState.data?.running;
  // 进行中 issue 也轮询（issue 状态可能在变）
  const issuesQ = useValidationIssues(ontologyIri, arbRunning);
  // 已裁决清单（accepted = 人工裁定采纳 B，A 必须改；overruled = 采纳 A 撤销，不用改）
  const adjudicated: ValidationIssue[] = (issuesQ.data ?? []).filter(
    (i: ValidationIssue) => i.status === "accepted" || i.status === "overruled",
  );
  const allDone = pending.length === 0 && adjudicated.length > 0;
  // 进行中：用 started_at 算实时已用时（后端 elapsed_ms 只完成时写）；完成：用 elapsed_ms
  const [, forceTick] = useState(0);
  useEffect(() => {
    if (!arbRunning) return;
    const t = window.setInterval(() => forceTick((x) => x + 1), 1000);
    return () => window.clearInterval(t);
  }, [arbRunning]);
  const elapsedMs = arbRunning
    ? Date.now() - (postArbState.data?.started_at ?? Date.now())
    : postArbState.data?.elapsed_ms ?? 0;
  const reviseMsg = postArbState.data?.msg ?? "";
  const reviseKind = postArbState.data?.kind ?? "";

  const runPostArbitrationRevise = () => {
    if (!ontologyIri) return;
    postArbRevise.mutate(ontologyIri);
  };


  return (
    <div className={styles.wrap}>
      <div className={styles.head}>
        <div className={styles.title}>人工仲裁</div>
        <div className={styles.stats}>
          <span className={styles.statWarning}>待处理 {pending.length}</span>
          {adjudicated.length > 0 && (
            <span className={styles.statSug}>已裁决 {adjudicated.length}</span>
          )}
        </div>
        {allDone && (
          <button
            type="button"
            className={styles.btnPrimary}
            disabled={!modelA || arbRunning || postArbRevise.isPending}
            title="根据仲裁裁决结果让 A 修订本体并落盘（accepted 的必须改、overruled 的保留原样）"
            onClick={runPostArbitrationRevise}
          >
            {arbRunning || postArbRevise.isPending ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Play size={13} />
            )}
            交 A 修订
          </button>
        )}
      </div>
      <div className={styles.body}>
        {arbRunning && (
          <p className="mb-2 text-xs text-fg-subtle">
            ⏳ A 修订中…（已用时 {Math.floor(elapsedMs / 60000)} 分 {Math.round((elapsedMs % 60000) / 1000)} 秒，后端运行切页不中断）
          </p>
        )}
        {reviseKind && reviseKind !== "" && reviseMsg && (
          <p className={`mb-2 text-xs ${reviseKind === "ok" ? "text-green-600" : "text-red-600"}`}>
            {reviseMsg}（⏱ {Math.floor(elapsedMs / 60000)} 分 {Math.round((elapsedMs % 60000) / 1000)} 秒）
          </p>
        )}
        {/* 左：仲裁队列 */}
        <div className={own.arbQueue}>
          {pending.length === 0 && <div className={styles.hint}>暂无待仲裁分歧</div>}
          {pending.map((issue: ValidationIssue) => (
            <button
              key={issue.id}
              type="button"
              className={`${own.arbItem} ${selected?.id === issue.id ? own.arbItemActive : ""}`}
              onClick={() => setSelectedId(issue.id)}
            >
              <span className={own.arbItemId}>
                {issue.source === "review" ? "评审" : "答辩"}·{issue.id.split(":").slice(-1)[0]}
              </span>
              <span className={own.arbItemMsg}>{issue.message}</span>
            </button>
          ))}
        </div>
        {/* 右：辩论视图 */}
        <div className={styles.issueList}>
          {selected ? (
            <DebateView
              issue={selected}
              bVerdict={selected.source === "review" ? bVerdict : undefined}
              onVerdict={doVerdict}
              onLater={() => setSelectedId(null)}
              onCustomRevise={setCustomTarget}
            />
          ) : (
            <div className={styles.hint}>选择左侧分歧点进行裁决</div>
          )}
          {resolve.isError && (
            <div className={styles.errBox}>{(resolve.error as Error).message}</div>
          )}
        </div>
      </div>

      {/* 自定义修订弹层 */}
      {customTarget && (
        <div className={styles.modalMask} onClick={() => setCustomTarget(null)}>
          <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
            <div className={styles.modalTitle}>自定义修订 · {customTarget.id}</div>
            <div className={styles.modalBody}>
              <div className={styles.formLabel}>修订方案（作为最终裁决记录）</div>
              <textarea
                className={styles.formInput}
                rows={4}
                placeholder="描述仲裁人自定义的修订方案…"
                value={customText}
                onChange={(e) => setCustomText(e.target.value)}
              />
            </div>
            <div className={styles.modalBtns}>
              <button
                type="button"
                className={styles.btnGhost}
                onClick={() => setCustomTarget(null)}
              >
                取消
              </button>
              <button
                type="button"
                className={styles.btnPrimary}
                onClick={doCustomRevise}
                disabled={!customText.trim() || resolve.isPending}
              >
                确认修订
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
