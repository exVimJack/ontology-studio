// Hooks 层：useOntologyValidation.ts（本体校验对抗，VALIDATION-DESIGN.md v0.3.1）
// 对齐 useOntologyTtl.ts 模式：queryKey 常量 + useQuery/useMutation。
// 覆盖对抗全链：warning 答辩（dispute/resolve/list/persist）、
// 多轮语义评审（review/a_revise/b_recheck/escalate/latest）、
// 覆盖矩阵（coverage_matrix）、模型独立性（independence_tier）、
// LLM 编排（B 评审/A 修订/B 复核）。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc/commands";
import type {
  ValidationIssue,
  ValidationReview,
  BReviewOutput,
  AReviseResult,
  TtlValidation,
  AutoDisputeItem,
} from "@/lib/ipc/bindings";

// 类型转发（§12.1：components 不得直接 import bindings，经 hooks 层取类型）
export type { ValidationIssue, ValidationReview, BReviewOutput, AReviseResult, TtlValidation, AutoDisputeItem };

const QK_ISSUES = (iri: string) => ["validation-issues", iri] as const;
const QK_REVIEW = (iri: string) => ["validation-review", iri] as const;
const QK_COVERAGE = (iri: string) => ["validation-coverage", iri] as const;

/** 本体的全部 issue（对抗状态展示）。 */
/** 本体 issue 列表（统一发现池：rules + review 域）。
 *  `pollWhen` 为真时 2s 轮询——答辩/评审进行中 issue 状态逐条落库，轮询让统计实时刷新。 */
export function useValidationIssues(ontologyIri: string | null, pollWhen = false) {
  return useQuery({
    queryKey: QK_ISSUES(ontologyIri ?? ""),
    queryFn: () => ipc.listValidationIssues(ontologyIri!),
    enabled: !!ontologyIri,
    refetchInterval: pollWhen ? 2000 : false,
  });
}

/**
 * 运行校验（规则检查）→ 持久化 issue 快照 → 刷新列表。
 *
 * 这是校验工作台的入口动作：validate_ttl 本身不落库，必须显式 persist，
 * 否则 validation_issues 表为空——答辩、闸门、仲裁面板全部空转。
 * persist 是整本体替换语义（旧快照清除），重复运行安全。
 */
export function useRunValidation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({
      ontologyIri,
      content,
    }: {
      ontologyIri: string;
      content: string;
    }): Promise<TtlValidation> => {
      const result = await ipc.validateOntologyTtl(content);
      // validation_id 仅作分组标识（表内整本体替换，id 不参与清理）
      const validationId = `val-${Date.now()}`;
      const issues = result.issues ?? [];
      await ipc.persistValidationIssues(ontologyIri, validationId, issues);
      return result;
    },
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: QK_ISSUES(vars.ontologyIri) });
      void qc.invalidateQueries({ queryKey: QK_COVERAGE(vars.ontologyIri) });
    },
  });
}

/** 本体的最新 review（可为 undefined）。 */
export function useLatestValidationReview(ontologyIri: string | null) {
  return useQuery({
    queryKey: QK_REVIEW(ontologyIri ?? ""),
    queryFn: () => ipc.latestValidationReview(ontologyIri!),
    enabled: !!ontologyIri,
    // 后端驱动评审（run_review_loop）进行中时轮询进度：review 状态非 in_progress
    // 时后端任务已完成，轮询停（数据不再变化，refetchInterval 回调返回 false）
    refetchInterval: (query) => {
      const d = query.state.data as { status?: string } | undefined;
      return d && d.status === "in_progress" ? 5000 : false;
    },
  });
}

/** 29 句话覆盖矩阵（全量或增量 diff）。默认不自动计算——用户显式触发
 * （「运行校验」成功后，或进入覆盖矩阵视图）才拉取，避免打开页面就跑检查。 */
export function useCoverageMatrix(
  ontologyIri: string | null,
  diffEntities: string[] = [],
  enabled = true,
) {
  return useQuery({
    queryKey: QK_COVERAGE(ontologyIri ?? ""),
    queryFn: () => ipc.coverageMatrix(ontologyIri!, diffEntities),
    enabled: !!ontologyIri && enabled,
  });
}

/** A 对 warning issue 发起答辩（②b 子阶段）。 */
export function useDisputeIssue() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      issueId,
      argument,
      counterSourceId,
      counterQuote,
    }: {
      issueId: string;
      argument: string;
      counterSourceId: string;
      counterQuote: string;
    }): Promise<string> =>
      ipc.disputeIssue(issueId, argument, counterSourceId, counterQuote),
    onSuccess: (_data, vars) => {
      // 答辩后 issue 状态变化，刷新列表
      qc.invalidateQueries({ queryKey: ["validation-issues"] });
      void vars;
    },
  });
}

/** 裁决答辩（B 复核或人工调用；不作为 A 工具暴露）。 */
export function useResolveDispute() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      issueId,
      verdict,
      resolvedBy,
    }: {
      issueId: string;
      verdict: string;
      resolvedBy: string;
    }): Promise<string> => ipc.resolveDispute(issueId, verdict, resolvedBy),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["validation-issues"] });
    },
  });
}

/** 仲裁后交 A 修订落盘（后端 spawn，立即返回；前端轮询 usePostArbitrationState 恢复状态）。 */
export function useRunPostArbitrationRevise() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (ontologyIri: string) => ipc.runPostArbitrationRevise(ontologyIri),
    onSuccess: (_data, ontologyIri) => {
      // 触发状态查询 → running=true 启动轮询
      void qc.invalidateQueries({ queryKey: ["post-arbitration-state", ontologyIri] });
      void qc.invalidateQueries({ queryKey: QK_ISSUES(ontologyIri) });
    },
  });
}

/** 仲裁后 A 修订状态（running + 已用时 + 结果消息）——切窗回来回显、显示耗时。
 *  running=true 时 2s 轮询刷新已用时/结果。 */
export function usePostArbitrationState(ontologyIri: string | null) {
  return useQuery({
    queryKey: ["post-arbitration-state", ontologyIri],
    queryFn: () => ipc.getPostArbitrationState(ontologyIri ?? ""),
    enabled: !!ontologyIri,
    refetchInterval: (query) => (query.state.data?.running ? 2000 : false),
  });
}

/** 持久化一次 validate 的 issue 快照。 */
export function usePersistValidationIssues() {
  return useMutation({
    mutationFn: ({
      ontologyIri,
      validationId,
      issues,
    }: {
      ontologyIri: string;
      validationId: string;
      issues: ValidationIssue[];
    }): Promise<number> =>
      ipc.persistValidationIssues(ontologyIri, validationId, issues),
  });
}

/** 启动多轮语义评审（②a 子阶段）。 */
export function useReviewOntology() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (ontologyIri: string): Promise<ValidationReview> =>
      ipc.reviewOntology(ontologyIri),
    onSuccess: (_data, iri) => {
      qc.invalidateQueries({ queryKey: QK_REVIEW(iri) });
    },
  });
}

/** 持久化 B 评审输出（第 1 步产物——此前只在前端 state，刷新即丢）。 */
export function useSaveBReport() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ reviewId, bReport }: { reviewId: string; bReport: string }): Promise<ValidationReview> =>
      ipc.saveBReport(reviewId, bReport),
    onSuccess: (data) => {
      // data 是更新后的 ValidationReview——按其 ontology_iri 精确失效最新评审查询
      if (data?.ontology_iri) {
        void qc.invalidateQueries({ queryKey: QK_REVIEW(data.ontology_iri) });
      }
    },
  });
}

/** A 提交修订（第 2 步）。 */
export function useARevise() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      ontologyIri: _ontologyIri,
      reviewId,
      aRevision,
      aReasoning,
    }: {
      ontologyIri: string;
      reviewId: string;
      aRevision: string;
      aReasoning: string;
    }): Promise<ValidationReview> =>
      ipc.aRevise(reviewId, aRevision, aReasoning),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: QK_REVIEW(vars.ontologyIri) });
    },
  });
}

/** B 复核（第 3 步）：confirm=true → resolved；false → escalated。 */
export function useBRecheck() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      ontologyIri: _ontologyIri,
      reviewId,
      confirm,
    }: {
      ontologyIri: string;
      reviewId: string;
      confirm: boolean;
    }): Promise<ValidationReview> => ipc.bRecheck(reviewId, confirm),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: QK_REVIEW(vars.ontologyIri) });
    },
  });
}

/** B 自动裁决答辩（LLM）：overruled/accepted 落库；uncertain 保持 disputed 升级人工。 */
export function useResolveDisputeLlm() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      ontologyIri: _ontologyIri,
      issueId,
      modelB,
    }: {
      ontologyIri: string;
      issueId: string;
      modelB: string;
    }): Promise<string> => ipc.resolveDisputeLlm(issueId, modelB),
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: QK_ISSUES(vars.ontologyIri) });
    },
  });
}

/** A 自动答辩（§2.1.5）：后端 spawn 跑「A 决策→机械匹配→B 裁决」，立即返回。
 *  前端靠 useAutoDisputeResult 轮询（running=true 时 2s）恢复进度——切窗不丢状态。
 *  后端完成后写 running=false + 最终 items，并 issue 状态已逐条落库。 */
export function useRunAutoDispute() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (ontologyIri: string): Promise<AutoDisputeItem[]> =>
      ipc.runAutoDispute(ontologyIri),
    onSuccess: (_data, ontologyIri) => {
      // 触发 getAutoDisputeResult 重查 → running=true 启动轮询
      void qc.invalidateQueries({ queryKey: ["auto-dispute-result", ontologyIri] });
    },
  });
}

/** 自动答辩状态（后端缓存，含 running 标志 + 逐条结果）——切页/切窗回来回显，
 *  不依赖组件存活。后端任务进行中（running=true）时 2s 轮询刷新逐条进度。 */
export function useAutoDisputeResult(ontologyIri: string | null) {
  const q = useQuery({
    queryKey: ["auto-dispute-result", ontologyIri],
    queryFn: () => ipc.getAutoDisputeResult(ontologyIri ?? ""),
    enabled: !!ontologyIri,
    refetchInterval: (query) => (query.state.data?.running ? 2000 : false),
  });
  return q;
}

/** 答辩提交 + B 自动机器裁决（一步到位）。裁决在 Rust 侧落库——提交后切窗/
 *  切页不影响结果；回来按 issue 状态回显。verdict: overruled/accepted/uncertain。 */
export function useDisputeAndJudge() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: {
      ontologyIri: string;
      issueId: string;
      argument: string;
      counterSourceId: string;
      counterQuote: string;
      modelB: string;
    }): Promise<string> => {
      void v.ontologyIri; // 仅用于 onSuccess 的 queryKey
      return ipc.disputeAndJudge(v.issueId, v.argument, v.counterSourceId, v.counterQuote, v.modelB);
    },
    onSuccess: (_data, vars) => {
      void qc.invalidateQueries({ queryKey: QK_ISSUES(vars.ontologyIri) });
    },
  });
}

/** 转人工仲裁。 */
export function useEscalateReview() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      ontologyIri: _ontologyIri,
      reviewId,
    }: {
      ontologyIri: string;
      reviewId: string;
    }): Promise<ValidationReview> => ipc.escalateReview(reviewId),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: QK_REVIEW(vars.ontologyIri) });
    },
  });
}

/** 模型独立性判定（设置页实时显示档位）。 */
export function useIndependenceTier() {
  return useMutation({
    mutationFn: ({
      providerA,
      modelA,
      providerB,
      modelB,
    }: {
      providerA: string;
      modelA: string;
      providerB: string;
      modelB: string;
    }): Promise<string> =>
      ipc.independenceTier(providerA, modelA, providerB, modelB),
  });
}

/** 运行 B 语义评审（LLM 编排）。 */
export function useRunBReviewLlm() {
  return useMutation({
    mutationFn: ({
      modelB,
      inputs,
    }: {
      modelB: string;
      inputs: string;
    }): Promise<BReviewOutput> => ipc.runBReviewLlm(modelB, inputs),
  });
}

/** 一次性触发后端完整语义评审循环（后端驱动：B评审→A修订→B复核→裁决）。
 *  返回 review id，前端轮询 latest_validation_review 看进度。
 *  任务在后端 tokio 中跑，不随前端组件生命周期中断（切页面/最小化不影响）。 */
export function useRunReviewLoop() {
  return useMutation({
    mutationFn: ({
      ontologyIri,
      ttlContent,
      charterSummary,
      issuesJson,
    }: {
      ontologyIri: string;
      ttlContent: string;
      charterSummary: string;
      issuesJson: string;
    }): Promise<string> =>
      ipc.runReviewLoop(ontologyIri, ttlContent, charterSummary, issuesJson),
  });
}

/** 运行 A 修订（LLM 编排）。 */
export function useRunAReviseLlm() {
  return useMutation({
    mutationFn: ({
      modelA,
      ttlV1,
      bSuggestions,
    }: {
      modelA: string;
      ttlV1: string;
      bSuggestions: string;
    }): Promise<AReviseResult> => ipc.runAReviseLlm(modelA, ttlV1, bSuggestions),
  });
}

/** 运行 B 复核（LLM 编排）。 */
export function useRunBRecheckLlm() {
  return useMutation({
    mutationFn: ({ modelB, aRevision }: { modelB: string; aRevision: string }): Promise<string> =>
      ipc.runBRecheckLlm(modelB, aRevision),
  });
}

/** 后端 layer 标记（L0 语法/L1 结构/L2 逻辑…）→ 界面可读名。 */
export function layerLabel(layer: string | null | undefined): string {
  const map: Record<string, string> = {
    L0: "语法",
    L1: "结构",
    L2: "逻辑",
    L3: "方法论",
    L4: "语义评审",
  };
  if (!layer) return "";
  return map[layer] ?? layer;
}
