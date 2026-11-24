/**
 * 对抗评审（阶段②）的状态与结束归因 → 界面文案 / 阶段推导。
 *
 * 为什么单独成 lib：这些判断此前散在 ReviewPanel 的三元表达式里，导致
 * "任何 escalated 都显示成复核未收敛"——而 escalated 至少有三种成因
 * （复核封顶未收敛 / 环节调用失败 / 人工升级）。2026-10-26 那次实际是后台
 * 任务随进程消失，界面却说"复核未收敛"，把排查方向整个带偏。
 * 抽成纯函数后，状态扩散（新增 interrupted）只要改这一处，并可单测。
 */

/** 评审状态值（与 Rust `validation_reviews.status` 对齐）。 */
export const REVIEW_STATUSES = [
  "in_progress",
  "resolved",
  "escalated",
  "interrupted",
] as const;

/** 结束归因值（与 Rust `outcome_reason` 对齐；in_progress 期间为空串）。 */
export const REVIEW_OUTCOMES = [
  "verdict_capped",
  "step_failed",
  "interrupted",
  "manual_escalation",
  "write_back_conflict",
  "unknown",
] as const;

export type ReviewView = {
  status: string;
  round: number;
  outcome_reason?: string;
};

/** 评审状态 + 结束归因 → 一句人话。 */
export function describeReview(r: ReviewView): string {
  if (r.status === "in_progress") {
    return r.round === 1 ? "首轮评审中" : `第 ${r.round - 1} 轮修订复核中`;
  }
  if (r.status === "interrupted") {
    return "上次评审已中断（后台任务不在），可重新发起";
  }
  if (r.status === "resolved") {
    return r.outcome_reason === "write_back_conflict"
      ? "复核通过，但草稿期间有改动——修订稿已保留、未写回"
      : "复核通过";
  }
  if (r.status === "escalated") {
    switch (r.outcome_reason) {
      case "verdict_capped":
        return "复核两轮未收敛，已升级人工仲裁";
      case "step_failed":
        return "评审环节失败，已转人工处理";
      case "manual_escalation":
        return "已手动升级人工仲裁";
      default:
        // 历史行归因不可追溯：保持中性，MUST NOT 猜测成"复核未收敛"
        return "已结束（归因未记录）";
    }
  }
  return r.status;
}

/** 工作台阶段推导：in_progress / interrupted → ②；resolved / escalated → ③；其余 → ①。
 *  interrupted 归②而不是③：它没有可仲裁的分歧，下一步动作是重跑。 */
export function reviewStage(status: string | null | undefined): 1 | 2 | 3 {
  if (status === "in_progress" || status === "interrupted") return 2;
  if (status === "resolved" || status === "escalated") return 3;
  return 1;
}

/** 阶段② 卡片上的运行态标签：只有真有任务在跑才算"进行中"。 */
export function reviewFlowTag(status: string | null | undefined): string {
  if (status === "in_progress") return "进行中";
  if (status === "interrupted") return "已中断";
  if (status === "resolved") return "已收敛";
  if (status === "escalated") return "升级仲裁";
  return "";
}

/** 轮次清单每轮的 verdict 徽标。 */
export function reviewVerdictTag(verdict: string | null | undefined): string {
  if (verdict === "resolved") return "✅ 收敛";
  if (verdict === "escalated") return "⚖ 升级仲裁";
  if (verdict === "interrupted") return "⚠ 已中断";
  if (verdict === "in_progress") return "进行中";
  return verdict ?? "";
}