import { describe, expect, it } from "vitest";
import {
  describeReview,
  reviewFlowTag,
  reviewStage,
  reviewVerdictTag,
} from "./review-outcome";

describe("describeReview", () => {
  it("in_progress 按轮次给文案（round 1=首轮，2+=第 N-1 轮修订复核）", () => {
    expect(describeReview({ status: "in_progress", round: 1 })).toBe("首轮评审中");
    expect(describeReview({ status: "in_progress", round: 2 })).toBe("第 1 轮修订复核中");
    expect(describeReview({ status: "in_progress", round: 3 })).toBe("第 2 轮修订复核中");
  });

  it("interrupted 说成可重跑，不说成复核未收敛", () => {
    const t = describeReview({ status: "interrupted", round: 1 });
    expect(t).toContain("已中断");
    expect(t).toContain("重新发起");
    expect(t).not.toContain("未收敛");
  });

  it("escalated 按归因分支，不再共用一句话", () => {
    expect(
      describeReview({ status: "escalated", round: 3, outcome_reason: "verdict_capped" }),
    ).toBe("复核两轮未收敛，已升级人工仲裁");
    expect(
      describeReview({ status: "escalated", round: 1, outcome_reason: "step_failed" }),
    ).toBe("评审环节失败，已转人工处理");
    expect(
      describeReview({ status: "escalated", round: 1, outcome_reason: "manual_escalation" }),
    ).toBe("已手动升级人工仲裁");
  });

  it("归因缺失/unknown 保持中性，MUST NOT 猜测", () => {
    for (const reason of [undefined, "", "unknown"]) {
      const t = describeReview({ status: "escalated", round: 3, outcome_reason: reason });
      expect(t).toBe("已结束（归因未记录）");
      expect(t).not.toContain("未收敛");
    }
  });

  it("resolved 区分写回冲突", () => {
    expect(describeReview({ status: "resolved", round: 2 })).toBe("复核通过");
    expect(
      describeReview({
        status: "resolved",
        round: 2,
        outcome_reason: "write_back_conflict",
      }),
    ).toContain("修订稿已保留、未写回");
  });
});

describe("reviewStage", () => {
  it("中断归阶段②（重跑），不是阶段③（仲裁）", () => {
    expect(reviewStage("in_progress")).toBe(2);
    expect(reviewStage("interrupted")).toBe(2);
    expect(reviewStage("resolved")).toBe(3);
    expect(reviewStage("escalated")).toBe(3);
    expect(reviewStage(null)).toBe(1);
    expect(reviewStage(undefined)).toBe(1);
  });
});

describe("reviewFlowTag / reviewVerdictTag", () => {
  it("只有真有任务在跑才标进行中", () => {
    expect(reviewFlowTag("in_progress")).toBe("进行中");
    expect(reviewFlowTag("interrupted")).toBe("已中断");
    expect(reviewFlowTag("resolved")).toBe("已收敛");
    expect(reviewFlowTag(null)).toBe("");
  });

  it("轮次徽标覆盖 interrupted", () => {
    expect(reviewVerdictTag("resolved")).toBe("✅ 收敛");
    expect(reviewVerdictTag("escalated")).toBe("⚖ 升级仲裁");
    expect(reviewVerdictTag("interrupted")).toBe("⚠ 已中断");
    expect(reviewVerdictTag(null)).toBe("");
  });
});