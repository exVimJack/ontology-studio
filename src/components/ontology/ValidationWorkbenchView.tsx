// UI 层：ValidationWorkbenchView.tsx（工作台 v2，对齐 validation-workbench-v2-mockup.html）
//
// 结构（自上而下）：
//   ① 顶层标题栏（瘦）：标题 + 独立性档位徽章 + 历史/报告图标（抽屉）
//   ② 校验对象行：本体下拉与"当前本体"合一（v0.4.2：从顶层标题栏下沉）
//   ③ 主流程一条线：① 规则检查（含 warning 答辩） → ② 对抗验证（多轮语义评审） → ③ 人工仲裁 + 出口检查点
//      ——子菜单栏移除，流程线即导航，节点可点击（done/active/blocked 三态）
//   ④ 阶段内容区：先「子流程示意条」（本步在干什么 + 实时状态），再完整功能
//
// 流程语义（VALIDATION-DESIGN v0.7）：全部显式触发；人只出现在最后。
// 答辩在阶段①（规则检查产出 warning 即可逐条反驳，不依赖语义评审先行——
// 语义评审在阶段②做本体级大清洗，两者并行不互锁）。

import { useEffect, useMemo, useRef, useState } from "react";
import { Clock, FileText, Loader2, Play, ShieldCheck, X } from "lucide-react";
import { useUiStore } from "@/stores/ui-store";
import { useProvider, useVerifierProvider } from "@/hooks/useProvider";
import {
  useOntologyTtls,
  useOntologyTtlContent,
  useOntologyTtlCharter,
  useImportOntologyTtl,
} from "@/hooks/useOntologyTtl";
import {
  useValidationIssues,
  useCoverageMatrix,
  useRunValidation,
  useRunAReviseLlm,
  useRunAutoDispute,
  useRunReviewLoop,
  useLatestValidationReview,
  useAutoDisputeResult,
  useIndependenceTier,
  type AutoDisputeItem,
  layerLabel,
  type ValidationIssue,
} from "@/hooks/useOntologyValidation";
import { IssueCard } from "./ValidationPanel";
import { ReviewPanel } from "./ReviewPanel";
import { ArbitrationPanel } from "./ArbitrationPanel";
import { CoverageView, ValidationHistory, ValidationReport } from "./ValidationReport";
import styles from "./WorkbenchView.module.css";
import vp from "./ValidationPanel.module.css";

type Stage = 1 | 2 | 3;
type Drawer = null | "history" | "report";

export function ValidationWorkbenchView() {
  const validationIri = useUiStore((s) => s.validationIri);
  const setValidationIri = useUiStore((s) => s.setValidationIri);
  const listQ = useOntologyTtls();
  const ontologies = useMemo(() => listQ.data ?? [], [listQ.data]);

  // validationIri 为空或已失效时，回落到第一个本体
  const iri =
    validationIri && ontologies.some((o) => o.ontology_iri === validationIri)
      ? validationIri
      : (ontologies[0]?.ontology_iri ?? null);
  const contentQ = useOntologyTtlContent(iri);
  const summary = ontologies.find((o) => o.ontology_iri === iri) ?? null;

  const [drawer, setDrawer] = useState<Drawer>(null);

  const issues = useValidationIssues(iri);
  const rulesRun = issues.data != null; // ① 是否已运行过（落库快照存在）

  // 流程状态从持久化 review 推导（切页/重启不丢）：有未收敛评审 → 阶段②；
  // 评审已结束（resolved/escalated）→ 阶段③；否则阶段①。用户点击 FlowLine 可改。
  const reviewQ = useLatestValidationReview(iri);
  const reviewStatus = reviewQ.data?.status ?? null;
  const [stage, setStage] = useState<Stage>(() =>
    reviewStatus === "in_progress"
      ? 2
      : reviewStatus === "resolved" || reviewStatus === "escalated"
        ? 3
        : 1,
  );
  useEffect(() => {
    if (stage !== 1) return; // 用户已手动选过阶段，不覆盖
    setStage(
      reviewStatus === "in_progress"
        ? 2
        : reviewStatus === "resolved" || reviewStatus === "escalated"
          ? 3
          : 1,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 只在初始推导
  }, [reviewStatus]);

  if (!iri) {
    return (
      <div className={styles.workbench}>
        <TopBar iri={null} onDrawer={setDrawer} />
        <div className={styles.empty}>
          <ShieldCheck size={32} className="opacity-30" />
          <p>还没有可校验的本体</p>
          <p className="text-xs">
            在对话中先建模（如「帮我建一个采购本体」），完成后输入
            <code className="mx-1 rounded bg-bg-elevated px-1 py-0.5 text-accent">/validate</code>
            触发校验
          </p>
        </div>
        {drawer && <Drawer kind={drawer} iri={null} onClose={() => setDrawer(null)} />}
      </div>
    );
  }

  return (
    <div className={styles.workbench}>
      <TopBar iri={iri} onDrawer={setDrawer} />
      {/* 校验对象行：下拉即当前本体（合并"当前本体 xxx"，不放顶层标题栏） */}
      <div className={styles.ontoRow}>
        <span className={styles.ontoLbl}>校验对象</span>
        <select
          className={styles.ontoSel}
          value={iri}
          onChange={(e) => setValidationIri(e.target.value)}
        >
          {ontologies.map((o) => (
            <option key={o.ontology_iri} value={o.ontology_iri}>
              {o.ontology_iri}
            </option>
          ))}
        </select>
        {summary && (
          <span className={styles.ontoMeta}>
            V{summary.version || "—"} · <b>{summary.triple_count}</b> 三元组 ·{" "}
            {summary.has_charter ? "宪章已填写" : "⚠ 宪章未填写"}
          </span>
        )}
      </div>

      <FlowLine
        stage={stage}
        rulesRun={rulesRun}
        reviewStatus={reviewStatus}
        onSelect={(s) => setStage(s)}
      />

      <div className={styles.stageBody}>
        {stage === 1 && <StageRule iri={iri} content={contentQ.data} />}
        {stage === 2 && <StageAdversarial iri={iri} />}
        {stage === 3 && <StageArbitration iri={iri} />}
      </div>

      {drawer && <Drawer kind={drawer} iri={iri} onClose={() => setDrawer(null)} />}
    </div>
  );
}

/* ── 独立性档位徽章（A=建模者 provider / B=校验者独立 provider）── */
function TierBadge() {
  const provider = useProvider();
  const verifierQ = useVerifierProvider();
  const independence = useIndependenceTier();
  const modelA = provider.data?.model ?? "";
  const modelB = verifierQ.data?.model || modelA;
  const kindA = provider.data?.kind ?? "";
  const kindB = verifierQ.data?.kind ?? kindA;
  const ready = !!modelA;
  useMemo(() => {
    if (!ready) return;
    independence.mutate({ providerA: kindA, modelA, providerB: kindB, modelB });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- mutate 稳定引用
  }, [ready, kindA, modelA, kindB, modelB]);
  const label = independence.data;
  if (!label) return null;
  return (
    <span className={`${styles.tierBadge} ${label.includes("降级") ? styles.tierWarn : styles.tierOk}`}>
      {label}
    </span>
  );
}

function TopBar({
  iri,
  onDrawer,
}: {
  iri: string | null;
  onDrawer: (d: "history" | "report") => void;
}) {
  return (
    <div className={styles.topbar}>
      <span className={styles.topTitle}>
        <ShieldCheck size={16} className="text-accent" />
        校验任务
      </span>
      {iri && <TierBadge />}
      <div className={styles.topRight}>
        <button className={styles.iconBtn} onClick={() => onDrawer("history")} title="校验历史">
          <Clock size={14} /> 历史
        </button>
        <button className={styles.iconBtn} onClick={() => onDrawer("report")} title="验证报告">
          <FileText size={14} /> 报告
        </button>
      </div>
    </div>
  );
}

/* ── 主流程一条线：三阶段节点 + 出口检查点 ── */
function FlowLine({
  stage,
  rulesRun,
  reviewStatus,
  onSelect,
}: {
  stage: Stage;
  rulesRun: boolean;
  /** 最新评审状态（in_progress/resolved/escalated/null）——阶段②标签数据源 */
  reviewStatus: string | null;
  onSelect: (s: Stage) => void;
}) {
  return (
    <div className={styles.flowline}>
      <button
        type="button"
        className={`${styles.fstage} ${stage === 1 ? styles.active : rulesRun ? styles.done : ""}`}
        onClick={() => onSelect(1)}
      >
        <span className={styles.fsTag}>检测零 LLM</span>
        <div className={styles.fsNum}>阶段 ①</div>
        <div className={styles.fsTitle}>规则检查</div>
        <div className={styles.fsSub}>语法 · 结构 · 逻辑 · 覆盖矩阵 · 纠错 · 答辩</div>
      </button>
      <button
        type="button"
        className={`${styles.fstage} ${stage === 2 ? styles.active : rulesRun ? "" : styles.blocked}`}
        onClick={() => onSelect(2)}
      >
        {stage !== 2 && rulesRun && reviewStatus === "resolved" && (
          <span className={styles.fsTag}>已收敛</span>
        )}
        {stage === 2 && reviewStatus === "in_progress" && <span className={styles.fsTag}>进行中</span>}
        <div className={styles.fsNum}>阶段 ②</div>
        <div className={styles.fsTitle}>对抗验证</div>
        <div className={styles.fsSub}>多轮语义评审 · B 裁决</div>
      </button>
      <button
        type="button"
        className={`${styles.fstage} ${stage === 3 ? styles.active : rulesRun ? "" : styles.blocked}`}
        onClick={() => onSelect(3)}
      >
        <div className={styles.fsNum}>阶段 ③</div>
        <div className={styles.fsTitle}>人工仲裁</div>
        <div className={styles.fsSub}>只收升级项 · 殿后</div>
      </button>
      <span className={styles.gatechip}>✋ 出口检查点 · import 时强制</span>
    </div>
  );
}

/* ── 子步骤 tips：悬停说明「这一步在做什么」（三阶段共用） ── */
const STEP_TIPS: Record<string, string> = {
  "语法解析": "先用 oxigraph 解析 Turtle：语法错误在这里被拦下，解析失败直接报错返回",
  "结构规范十项": "十项静态规则：IRI 命名规范、类声明完整性、subClassOf/subPropertyOf 循环与悬挂引用、中英标签、SWRL 标准词汇等",
  "逻辑一致性": "SPARQL 规则集：domain/range、基数约束、disjoint、functional、对称/传递一致性",
  "覆盖矩阵": "对照 29 句话标准，逐条核对产物实体是否落地；缺项标红可回喂修订",
  "error → 纠错 → 重跑": "error 级问题必须先修再继续——纠错后重跑校验（最多 3 轮），不能带错进入评审",
  "B 评审（八维度）": "校验者独立审计，逐条过八维度：①事实忠实度（断言有无企业证据，优先找反例）②完整性（29句话每条有无对应产出）③语义正确性（概念定义/边界/混淆）④结构合理性（层级/关系/拆并）⑤逻辑一致性（冲突/推理验证）⑥约束正确性（基数/唯一性/枚举）⑦数据可落地性（联动数据源比对）⑧业务可用性（联动 CQ 反测）——每条建议必须带证据引用",
  "A 修订": "建模者根据 B 的建议修订 Turtle：接受则改、拒绝则说明理由（给 B 复核看）",
  "B 复核": "校验者只看修订后的 Turtle（不看 A 的理由），确认修订是否解决原问题；仍不通过则封顶升级仲裁",
  "提论据 + 反证来源 + 原文引用": "针对单条 issue 提出反证：选已登记证据源 + 摘录原文——论据充分才能推翻机器判定",
  "机械匹配（match_quote）": "系统先做字面匹配：你引用的原文必须真实存在于所选证据源中，匹配不上直接驳回（防编造证据）",
  "B 复核裁决": "B 结合反证与原文裁决该 issue：证据充分则 overruled（推翻原判），判不定则升级人工",
  "阅读分歧双方论据（A vs B）": "A 的建模立场 vs B 的审计立场并排展示——人工仲裁只看分歧项，不看已收敛项",
  "裁决": "采纳 A（accepted，撤销告警）或采纳 B（overruled，维持告警并关闭）——仲裁是最后出口",
};

/* ── 阶段①：规则检查 ── */
function StageRule({
  iri,
  content,
}: {
  iri: string;
  content: string | undefined;
}) {
  const issues = useValidationIssues(iri);
  // 统一发现池 v0.7：validation_issues 同时存 rules + review 域发现。
  // 阶段① 规则检查只看 source='rules'——review 域的 B 评审发现归阶段② ReviewPanel，
  // 不该带「语义评审」标签混进规则校验结果里。
  const ruleIssues = useMemo(
    () => (issues.data ?? []).filter((i: ValidationIssue) => i.source !== "review"),
    [issues.data],
  );
  const runValidation = useRunValidation();
  const provider = useProvider();
  const verifierQ = useVerifierProvider();
  const modelA = provider.data?.model ?? "";
  const modelB = verifierQ.data?.model || modelA;
  // 覆盖矩阵显式触发：「运行校验」成功后一并计算；有结果则内嵌展示（无开关）
  const [coverageOn, setCoverageOn] = useState(false);
  const coverage = useCoverageMatrix(iri, [], coverageOn);
  const [resultMsg, setResultMsg] = useState<{ kind: "ok" | "err"; text: string } | null>(null);
  const showResult = (kind: "ok" | "err", text: string) => {
    setResultMsg({ kind, text });
    window.setTimeout(() => setResultMsg(null), 6000);
  };

  return (
    <div>
      {/* 子流程示意条：「运行校验」在左侧，是本阶段唯一入口（sticky 钉顶） */}
      <div className={`${styles.subflow} ${styles.subflowSticky}`}>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
          disabled={!content || runValidation.isPending}
          onClick={() => {
            if (!content) return;
            runValidation.mutate(
              { ontologyIri: iri, content },
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
          {runValidation.isPending ? <Loader2 size={13} className="animate-spin" /> : <Play size={13} />}
          运行校验
        </button>
        <span className={styles.sfTitle}>子流程 · 一次运行 =</span>
        <span className={styles.sfn} title={STEP_TIPS["语法解析"]}>语法解析</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>→</span>
        <span className={styles.sfn} title={STEP_TIPS["结构规范十项"]}>结构规范十项</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>→</span>
        <span className={styles.sfn} title={STEP_TIPS["逻辑一致性"]}>逻辑一致性</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>→</span>
        <span className={styles.sfn} title={STEP_TIPS["覆盖矩阵"]}>覆盖矩阵</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`} title={STEP_TIPS["error → 纠错 → 重跑"]}>⟲ 纠错重跑（≤3 轮）</span>
        <span className={styles.sfHint}>规则部分全确定性、毫秒级；答辩对 warning 逐条进行，B 机器裁决</span>
      </div>
      {resultMsg && (
        <p className={`mb-2 text-xs ${resultMsg.kind === "ok" ? "text-green-600" : "text-red-600"}`}>
          {resultMsg.text}
        </p>
      )}


      {/* 规则结果（warning 答辩入口在下方「答辩」子段，与结果清单同页）；可折叠。
          统一发现池 v0.7：此处只列 source='rules' 发现——review 域 B 评审发现归阶段②。 */}
      {issues.isLoading && <p className="text-xs text-fg-subtle">加载中…</p>}
      <details className={styles.fold} open>
        <summary className={styles.foldSummary}>
          校验结果（{issues.data === undefined ? "未运行" : `${ruleIssues.length} 条`}
          {(() => {
            const e = ruleIssues.filter((i: ValidationIssue) => i.severity === "error").length;
            const w = ruleIssues.filter((i: ValidationIssue) => i.severity === "warning").length;
            return e + w > 0 ? ` · error ×${e} · warning ×${w}` : "";
          })()}
          ）
        </summary>
        <div className={styles.foldBody}>
          {issues.data === undefined && (
            <p className="text-xs text-fg-subtle">
              尚未运行校验——点击「运行校验」开始规则检查（L0 语法 → 结构规范 → 逻辑一致性 → 覆盖矩阵）
            </p>
          )}
          {issues.data !== undefined && ruleIssues.length === 0 && (
            <p className="text-xs text-fg-subtle">暂无校验问题（validate 通过）</p>
          )}
          {ruleIssues.map((issue: ValidationIssue) => (
            <IssueCard key={issue.id} issue={issue} onJumpToSource={undefined} />
          ))}
        </div>
      </details>

      {/* 覆盖矩阵折叠框：跑过就有，缺失明细收在框内 */}
      {coverageOn && coverage.isPending && <p className="mt-3 text-xs text-fg-subtle">覆盖矩阵计算中…</p>}
      {coverage.data && (
        <details className={styles.fold} open>
          <summary className={styles.foldSummary}>
            覆盖矩阵（{coverage.data.covered_count}/{coverage.data.total_count} · 缺失{" "}
            {coverage.data.total_count - coverage.data.covered_count}）
          </summary>
          <div className={styles.foldBody}>
            <CoverageView iri={iri} />
          </div>
        </details>
      )}

      {/* 答辩（阶段①内）：交 A 修复 + 交 A 答辩都在这里；对 warning 逐条反驳，B 机器裁决，判不定升级仲裁（阶段③） */}
      <div className="mt-4 border-t border-border pt-4">
        <StageDispute iri={iri} content={content} modelA={modelA} modelB={modelB} />
      </div>
    </div>
  );
}

/* ── 阶段②：对抗验证（多轮语义评审，B 评审 → A 修订 → B 复核自动轮转）── */
function StageAdversarial({ iri }: { iri: string }) {
  const contentQ = useOntologyTtlContent(iri);
  const provider = useProvider();
  const verifierQ = useVerifierProvider();
  const charterQ = useOntologyTtlCharter(iri);
  const charter = charterQ.data;
  const charterSummary = charter
    ? `业务场景：${charter.business_scenario || "（未填写）"}\n业务本质：${charter.business_essence || "（未填写）"}`
    : "（宪章加载中）";
  const modelA = provider.data?.model ?? "";
  const modelB = verifierQ.data?.model || modelA;

  return (
    <div>
      {/* 子流程示意条（sticky：滚动时钉在顶部，按钮不随内容滚走） */}
      <div className={`${styles.subflow} ${styles.subflowSticky}`}>
        <ReviewLaunchButton
          iri={iri}
          ttlContent={contentQ.data ?? null}
          charterSummary={charterSummary}
        />
        <span className={styles.sfTitle}>子流程 · 三环节自动轮转：</span>
        <span className={styles.sfn} title={STEP_TIPS["B 评审（八维度）"]}>B 评审（八维度）</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>──▶</span>
        <span className={styles.sfn} title={STEP_TIPS["A 修订"]}>A 修订</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>──▶</span>
        <span className={styles.sfn} title={STEP_TIPS["B 复核"]}>B 复核</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>⟲ 收敛</span>
        <span className={styles.sfHint}>
          后端驱动自动轮转（切页不中断）；B 复核追问未封顶进下一轮，封顶升级仲裁
        </span>
      </div>

      <ReviewPanel ontologyIri={iri} modelA={modelA} modelB={modelB} onEscalated={() => undefined} />
    </div>
  );
}

/** 「启动/重新评审」按钮（阶段②唯一入口，放子流程条最前——同阶段①「运行校验」位次）。
 *  启动逻辑从 ReviewPanel 提出共用：拼规则检查报告喂 B，后端自动轮转。 */
function ReviewLaunchButton({
  iri,
  ttlContent,
  charterSummary,
}: {
  iri: string;
  ttlContent: string | null;
  charterSummary: string;
}) {
  const issues = useValidationIssues(iri);
  const review = useLatestValidationReview(iri);
  const runReviewLoop = useRunReviewLoop();
  const provider = useProvider();
  const verifierQ = useVerifierProvider();
  const modelB = verifierQ.data?.model || provider.data?.model || "";
  const [err, setErr] = useState("");
  const starting = useRef(false);
  const current = review.data;
  const running = current?.status === "in_progress";

  const launch = async () => {
    if (!iri || !ttlContent) return;
    setErr("");
    starting.current = true;
    const issueLines = (issues.data ?? [])
      .map((i) => `- [${i.severity}] ${i.layer} ${i.id}: ${i.message}（status=${i.status}）`)
      .join("\n");
    try {
      await runReviewLoop.mutateAsync({
        ontologyIri: iri,
        ttlContent,
        charterSummary,
        issuesJson: (issues.data ?? []).length === 0 ? "" : issueLines,
      });
    } catch (e) {
      setErr((e as Error).message || "启动评审失败");
    } finally {
      starting.current = false;
    }
  };

  return (
    <>
      <button
        type="button"
        className="inline-flex items-center gap-1.5 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
        disabled={!iri || !ttlContent || !modelB || running || starting.current}
        onClick={() => void launch()}
      >
        {runReviewLoop.isPending ? (
          <Loader2 size={13} className="animate-spin" />
        ) : (
          <Play size={13} />
        )}
        {running ? "评审进行中" : current ? "重新评审" : "启动评审"}
      </button>
      {err && <span className="text-xs text-red-600">{err}</span>}
    </>
  );
}

/* ── 答辩（阶段①内）：A 逐条决策（自动）+ 人工精修入口（弹窗）──
   完整对抗语义：答辩发起方是 A（建模者）——A 构造论据+反证，B 裁决；
   「交 A 答辩」按钮一键跑完后端链（A 决策 → 机械匹配 → B 三态裁决），
   人只在阶段③ 仲裁 B 判不定的残留。 */
function StageDispute({
  iri,
  content,
  modelA,
  modelB,
}: {
  iri: string;
  content: string | undefined;
  modelA: string;
  modelB: string;
}) {
  const autoDispute = useRunAutoDispute();
  const cachedResult = useAutoDisputeResult(iri);
  // 后端驱动进度（对齐 ReviewPanel）：running 由缓存 running 标志驱动，不依赖
  // 前端 mutation 状态——切窗/切页回来轮询到 running=true 即恢复"进行中"展示。
  // issue 状态逐条落库，pollWhen=true 让 warnings/settled 统计实时刷新（Bug 2 修复）。
  const disputeRunning = !!cachedResult.data?.running;
  const issues = useValidationIssues(iri, disputeRunning);
  const runValidation = useRunValidation();
  const runARevise = useRunAReviseLlm();
  const importTtl = useImportOntologyTtl();
  const coverage = useCoverageMatrix(iri, [], true);
  const ruleIssues = (issues.data ?? []).filter((i: ValidationIssue) => i.source !== "review");
  const warnings = ruleIssues.filter(
    (i: ValidationIssue) =>
      i.severity === "warning" &&
      (!i.status || i.status === "open" || i.status === "disputed"),
  );
  const settled = ruleIssues.filter((i: ValidationIssue) =>
    ["accepted", "overruled"].includes(String(i.status)),
  );
  // 展示用结果：后端缓存 items（进行中已完成的条目 / 完成后的完整结果）
  const disputeResult: AutoDisputeItem[] | null =
    cachedResult.data?.items?.length ? cachedResult.data.items : null;

  // ── 交 A 修复：error 纠错循环（≤3 轮）——error 是确定性违反、无辩空间，
  // A 单方修订；中间稿不落库（修好才进库），覆盖缺失一并交给 A。──
  const [fixing, setFixing] = useState(false);
  const [fixRound, setFixRound] = useState<{ round: number; notes: string; ms: number } | null>(null);
  const [fixErr, setFixErr] = useState("");
  const unclosed = ruleIssues.filter(
    (i: ValidationIssue) =>
      i.severity === "error" && (!i.status || i.status === "open" || i.status === "disputed"),
  );
  const fmtErrors = (list: ValidationIssue[]) =>
    list.map((i) => `- [${i.severity}] ${layerLabel(i.layer)} ${i.id}: ${i.message}`).join("\n");
  const missingCoverage = () => {
    const miss = (coverage.data?.items ?? []).filter((it) => !it.covered);
    if (miss.length === 0) return "";
    return miss
      .map((it) => `- [覆盖缺失] ${it.sentence_id}: ${it.description}（${it.product_shape} 未落地）`)
      .join("\n");
  };
  const runFix = async () => {
    if (!content || unclosed.length === 0) return;
    setFixing(true);
    setFixErr("");
    setFixRound(null);
    let cur = content;
    let errs = unclosed;
    try {
      for (let round = 1; round <= 3 && errs.length > 0; round++) {
        const t0 = Date.now();
        setFixRound({ round, notes: "", ms: 0 });
        const res = await runARevise.mutateAsync({
          modelA,
          ttlV1: cur,
          bSuggestions: [fmtErrors(errs), missingCoverage()].filter(Boolean).join("\n"),
        });
        cur = res.revised_ttl;
        const v = await runValidation.mutateAsync({ ontologyIri: iri, content: cur });
        errs = (v.issues ?? []).filter(
          (i) =>
            i.severity === "error" && (!i.status || i.status === "open" || i.status === "disputed"),
        ) as ValidationIssue[];
        setFixRound({ round, notes: res.reasoning || "（无修订说明）", ms: Date.now() - t0 });
      }
      if (errs.length > 0) {
        setFixErr(`3 轮纠错后仍有 ${errs.length} 个 error 未闭环——建议回到建模对话中人工处理这几项`);
        return;
      }
      await importTtl.mutateAsync({ ttl: cur, overwrite: true });
      setFixRound(null);
      setFixErr("");
      setFixOk("A 修复完成：error 已全部闭环，修订稿已落库");
    } catch (e) {
      setFixErr(`纠错循环失败：${(e as Error).message}`);
    } finally {
      setFixing(false);
    }
  };
  const [fixOk, setFixOk] = useState("");

  return (
    <div className={vp.wrap}>
      {/* 头部：与语义评审同构（标题 + 统计 + 两个动作按钮） */}
      <div className={vp.head}>
        <div className={vp.title}>答辩</div>
        <div className={vp.stats}>
          <span className={vp.statWarning}>待答辩 {warnings.length}</span>
          <span className={vp.statSug}>已裁决 {settled.length}</span>
          <span className={vp.statSug}>A: {modelA || "未配置"}</span>
          <span className={vp.statWarning}>B: {modelB || "未配置"}</span>
        </div>
        <div className={vp.headBtns}>
          <button
            type="button"
            className={vp.btnGhost}
            disabled={!content || unclosed.length === 0 || fixing || !modelA}
            title={
              unclosed.length === 0
                ? "没有未闭环的 error"
                : `把 ${unclosed.length} 个未闭环 error（含覆盖缺失）交给 A 修订（≤3 轮自动循环）`
            }
            onClick={() => void runFix()}
          >
            {fixing ? <Loader2 size={13} className="animate-spin" /> : <ShieldCheck size={13} />}
            交 A 修复（{unclosed.length} error）
          </button>
          <button
            type="button"
            className={vp.btnPrimary}
            disabled={warnings.length === 0 || disputeRunning || fixing}
            title={
              warnings.length === 0
                ? "没有待答辩的 warning"
                : "A 逐条决策（答辩/接受）→ 机械匹配 → B 机器裁决，一键全自动；切页/切窗不中断"
            }
            onClick={() => autoDispute.mutate(iri)}
          >
            {disputeRunning ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Play size={13} />
            )}
            交 A 答辩（{warnings.length} warning）
          </button>
        </div>
      </div>

      <div className={vp.issueList}>
        {fixing && fixRound && (
          <p className="mb-2 text-xs text-fg-subtle">
            ⏳ 纠错循环第 {fixRound.round} 轮：A 修订中…（error 是确定性违反，A 单方修订、B 不参与）
          </p>
        )}
        {fixOk && !fixing && <p className="mb-2 text-xs text-green-600">{fixOk}</p>}
        {fixErr && <p className="mb-2 text-xs text-red-600">{fixErr}</p>}
        {!fixing && fixRound?.notes && (
          <details className={styles.fold} open>
            <summary className={styles.foldSummary}>
              第 {fixRound.round} 轮修订说明（⏱ {Math.round(fixRound.ms / 1000)}s）
            </summary>
            <div className={styles.foldBody}>
              <pre className="m-0 whitespace-pre-wrap text-xs text-fg-subtle" style={{ fontFamily: "inherit" }}>{fixRound.notes}</pre>
            </div>
          </details>
        )}

        {disputeRunning && (
          <p className="mb-2 text-xs text-fg-subtle">
            ⏳ A 逐条决策中（选证据源 → 摘录原文 → 构造论据）→ 机械匹配 → B 裁决…（整链后端跑，切页不中断）
          </p>
        )}
        {disputeResult && !disputeRunning && (
          <details className={styles.fold} open>
            <summary className={styles.foldSummary}>
              答辩结果（
              {disputeResult.filter((d) => d.outcome === "overruled").length} 撤销 ·{" "}
              {disputeResult.filter((d) => d.outcome === "accepted").length} B 驳回 ·{" "}
              {disputeResult.filter((d) => d.outcome === "uncertain").length} 待人工 ·{" "}
              {disputeResult.filter((d) => d.outcome === "accept").length} A 接受
              {disputeResult.some((d) => d.outcome === "rejected_match")
                ? ` · ${disputeResult.filter((d) => d.outcome === "rejected_match").length} 摘录未匹配`
                : ""}
              ）
            </summary>
            <div className={styles.foldBody}>
              <ul className="m-0 list-none space-y-1 p-0 text-xs text-fg-subtle">
                {disputeResult.map((d) => (
                  <li key={d.issue_id}>
                    <b className="text-fg">{d.issue_id}</b> —{" "}
                    {d.outcome === "overruled"
                      ? "✅ B 撤销"
                      : d.outcome === "accepted"
                        ? "❌ B 驳回（升级 error）"
                        : d.outcome === "uncertain"
                          ? "⚖ B 判不定 → 人工仲裁"
                          : d.outcome === "rejected_match"
                            ? "⚠ 摘录未过机械匹配，维持 open"
                            : d.outcome === "error"
                              ? "✗ 处理失败"
                              : "A 接受"}
                    {d.detail ? `：${d.detail}` : ""}
                  </li>
                ))}
              </ul>
            </div>
          </details>
        )}

        {warnings.length === 0 && unclosed.length === 0 && !disputeRunning && !disputeResult && (
          <div className={vp.hint}>
            没有待处理的 warning 与 error（先运行上方「运行校验」）
          </div>
        )}
        {warnings.length > 0 && !disputeResult && !disputeRunning && (
          <div className={vp.hint}>
            待答辩 warning 的明细见上方「校验结果」折叠框（卡片右侧有「答辩」按钮可人工精修）；
            点「交 A 答辩」让 A 全自动逐条处理。
          </div>
        )}
      </div>
    </div>
  );
}

/* ── 阶段③：人工仲裁 ── */
function StageArbitration({ iri }: { iri: string }) {
  return (
    <div>
      <div className={`${styles.subflow} ${styles.subflowSticky}`}>
        <span className={styles.sfTitle}>子流程 · 两步裁决：</span>
        <span className={styles.sfn} title={STEP_TIPS["阅读分歧双方论据（A vs B）"]}>阅读分歧双方论据（A vs B）</span>
        <span className={`${styles.sfn} ${styles.sfnLoop}`}>→</span>
        <span className={styles.sfn} title={STEP_TIPS["裁决"]}>裁决</span>
        <span className={styles.sfHint}>
          裁决权在 B（Verifier），人工执行；裁决后出口检查点放行
        </span>
      </div>
      <ArbitrationPanel ontologyIri={iri} onResolved={() => undefined} />
    </div>
  );
}

/* ── 历史 / 报告抽屉（跨任务横切能力，不入流程线）── */
function Drawer({
  kind,
  iri,
  onClose,
}: {
  kind: "history" | "report";
  iri: string | null;
  onClose: () => void;
}) {
  return (
    <>
      <div className={styles.drawerMask} onClick={onClose} />
      <div className={styles.drawer}>
        <div className={styles.drawerHead}>
          <span>{kind === "history" ? "🕘 校验历史" : "📋 验证报告"}</span>
          <button className={styles.iconBtn} onClick={onClose} title="关闭">
            <X size={15} />
          </button>
        </div>
        <div className={styles.drawerBody}>
          {kind === "history" ? (
            <ValidationHistory ontologyIri={iri} />
          ) : (
            iri && <ValidationReport ontologyIri={iri} />
          )}
        </div>
      </div>
    </>
  );
}
