// 单元测试：chat-run-status.ts（长任务 liveness 纯逻辑）
// 设计见 docs/LONG-RUNNING-LIVENESS.md。运行：npm test（vitest）。

import { describe, expect, it } from "vitest"
import {
  ELAPSED_VISIBLE_AFTER_S,
  RUN_STATUS_IDLE,
  RUN_TOUCH_THROTTLE_MS,
  STALL_HINT_THRESHOLD_MS,
  elapsedSeconds,
  nextRunStatus,
  runPhaseText,
  shouldShowStallHint,
} from "./chat-run-status"
// formatMessageTime 的单测在 format.test.ts（时间戳功能归属）

const T0 = 1_700_000_000_000

/** 构造运行状态（默认 streaming）。 */
function st(phase: "idle" | "thinking" | "streaming" | "tool", at = T0, toolName?: string) {
  return { phase, toolName, lastChunkAt: at }
}

// ── nextRunStatus：状态机 + 节流 ──

describe("nextRunStatus", () => {
  it("phase 变化时立即返回新状态并刷新 lastChunkAt", () => {
    const next = nextRunStatus(st("thinking", T0), "streaming", undefined, T0 + 100)
    expect(next).toEqual({ phase: "streaming", toolName: undefined, lastChunkAt: T0 + 100 })
  })

  it("进入 tool 阶段时携带工具名", () => {
    const next = nextRunStatus(st("streaming", T0), "tool", "search_documents", T0 + 50)
    expect(next).toEqual({
      phase: "tool",
      toolName: "search_documents",
      lastChunkAt: T0 + 50,
    })
  })

  it("tool → tool 换工具名视为 phase 变化（立即更新）", () => {
    const next = nextRunStatus(st("tool", T0, "search_documents"), "tool", "read_document", T0 + 10)
    expect(next?.toolName).toBe("read_document")
    expect(next?.lastChunkAt).toBe(T0 + 10)
  })

  it("tool → tool 同名且未到节流间隔 → 返回 null（跳过 setState）", () => {
    const next = nextRunStatus(st("tool", T0, "search_documents"), "tool", "search_documents", T0 + RUN_TOUCH_THROTTLE_MS - 1)
    expect(next).toBeNull()
  })

  it("同 phase 超过节流间隔 → 仅刷新 lastChunkAt", () => {
    const next = nextRunStatus(st("streaming", T0), "streaming", undefined, T0 + RUN_TOUCH_THROTTLE_MS)
    expect(next).toEqual({ phase: "streaming", toolName: undefined, lastChunkAt: T0 + RUN_TOUCH_THROTTLE_MS })
  })

  it("非 tool 阶段忽略传入的 toolName（不残留旧工具名）", () => {
    const next = nextRunStatus(st("tool", T0, "search_documents"), "streaming", "search_documents", T0 + 5)
    expect(next?.toolName).toBeUndefined()
  })

  it("边界：恰好等于节流间隔时刷新（>= 而非 >）", () => {
    expect(nextRunStatus(st("streaming", T0), "streaming", undefined, T0 + RUN_TOUCH_THROTTLE_MS)).not.toBeNull()
    expect(nextRunStatus(st("streaming", T0), "streaming", undefined, T0 + RUN_TOUCH_THROTTLE_MS - 1)).toBeNull()
  })
})

// ── runPhaseText：阶段文案 ──

describe("runPhaseText", () => {
  it("各阶段文案正确", () => {
    expect(runPhaseText("thinking")).toBe("正在思考…")
    expect(runPhaseText("streaming")).toBe("正在生成回复…")
    expect(runPhaseText("tool")).toBe("正在执行工具…")
    expect(runPhaseText("tool", "read_document")).toBe("正在执行 read_document…")
    expect(runPhaseText("idle")).toBe("处理中…")
  })
})

// ── shouldShowStallHint：静默判定 ──

describe("shouldShowStallHint", () => {
  it("running 且静默超过阈值 → true", () => {
    expect(shouldShowStallHint(st("streaming", T0), T0 + STALL_HINT_THRESHOLD_MS)).toBe(true)
    expect(shouldShowStallHint(st("tool", T0), T0 + STALL_HINT_THRESHOLD_MS + 1)).toBe(true)
  })

  it("静默未超阈值 → false", () => {
    expect(shouldShowStallHint(st("streaming", T0), T0 + STALL_HINT_THRESHOLD_MS - 1)).toBe(false)
  })

  it("idle 状态永不提示（本轮已结束）", () => {
    expect(shouldShowStallHint(st("idle", T0), T0 + 60_000)).toBe(false)
  })

  it("边界：恰好等于阈值时显示（>= 而非 >）", () => {
    expect(shouldShowStallHint(st("streaming", T0), T0 + STALL_HINT_THRESHOLD_MS)).toBe(true)
  })
})

// ── elapsedSeconds：已耗时读数 ──

describe("elapsedSeconds", () => {
  it("向下取整且不为负", () => {
    expect(elapsedSeconds(T0, T0 + 0)).toBe(0)
    expect(elapsedSeconds(T0, T0 + 999)).toBe(0)
    expect(elapsedSeconds(T0, T0 + 1_000)).toBe(1)
    expect(elapsedSeconds(T0, T0 + 65_400)).toBe(65)
    // 时钟回拨保护
    expect(elapsedSeconds(T0, T0 - 5_000)).toBe(0)
  })
})

// ── 常量合理性（阈值回归保护）──

describe("常量", () => {
  it("节流 < 静默阈值 < 用户焦虑量级", () => {
    expect(RUN_TOUCH_THROTTLE_MS).toBeLessThan(STALL_HINT_THRESHOLD_MS)
    // 秒数显示阈值也应小于静默阈值（先有读数后有提示）
    expect(ELAPSED_VISIBLE_AFTER_S * 1_000).toBeLessThanOrEqual(STALL_HINT_THRESHOLD_MS)
  })

  it("IDLE 是冻结常量（lastChunkAt=0 不会误判为最近活动）", () => {
    expect(RUN_STATUS_IDLE.phase).toBe("idle")
    expect(RUN_STATUS_IDLE.lastChunkAt).toBe(0)
  })
})
