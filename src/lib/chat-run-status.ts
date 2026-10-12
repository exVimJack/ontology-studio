// 纯逻辑层：chat-run-status.ts
// 长任务 liveness 的可测纯函数（不依赖 React / Tauri，供 useChat / Thread 与 vitest 共用）。
// 设计见 docs/LONG-RUNNING-LIVENESS.md。
// 注：formatMessageTime 在 ./format.ts（时间戳功能引入），此处不重复定义。

export { formatMessageTime } from "./format"

/** 本轮 turn 的运行状态（长任务 liveness 指示）。
 *  - thinking：已发送、尚无任何输出（模型思考中 / 首个 chunk 未到）
 *  - streaming：正在产出文本 / reasoning
 *  - tool：正在执行工具调用（toolName 为当前工具名）
 *  - idle：本轮结束（done / error / cancelled）
 *  lastChunkAt：最近一次收到 chunk 的本地时间戳（ms），供前端判「静默但仍在跑」。 */
export interface RunStatus {
  phase: "idle" | "thinking" | "streaming" | "tool"
  toolName?: string
  lastChunkAt: number
}

export const RUN_STATUS_IDLE: RunStatus = { phase: "idle", lastChunkAt: 0 }

/** lastChunkAt 刷新节流间隔：phase 不变时至多 1s setState 一次。 */
export const RUN_TOUCH_THROTTLE_MS = 1_000

/** 空占位上「已 N 秒」的显示阈值：短任务不显示秒数，避免读数闪烁。
 *  需 <= STALL_HINT_THRESHOLD_MS（先有读数后有静默提示，见单测常量约束）。 */
export const ELAPSED_VISIBLE_AFTER_S = 1

/** 静默判定阈值（ms）。流式中连续无 chunk 超过该值即视为“停滞”，显示「仍在处理」提示。 */
export const STALL_HINT_THRESHOLD_MS = 2_000

/** 计算下一个运行状态（touchRunStatus 的纯函数形式）。
 *
 * 规则：
 *  - phase 或 toolName 变化 → 立即返回新状态（lastChunkAt = now）
 *  - phase 不变且距上次刷新 < throttle → 返回 null（调用方跳过 setState）
 *  - phase 不变且距上次刷新 >= throttle → 仅刷新 lastChunkAt
 *
 * @param cur    当前状态
 * @param phase  新阶段
 * @param toolName 工具阶段下的工具名（其他阶段忽略）
 * @param now    当前时间戳（ms），由调用方注入以便测试
 */
export function nextRunStatus(
  cur: RunStatus,
  phase: RunStatus["phase"],
  toolName: string | undefined,
  now: number,
  throttleMs: number = RUN_TOUCH_THROTTLE_MS,
): RunStatus | null {
  if (phase !== "tool") toolName = undefined
  if (cur.phase !== phase || cur.toolName !== toolName) {
    return { phase, toolName, lastChunkAt: now }
  }
  if (now - cur.lastChunkAt >= throttleMs) {
    return { ...cur, lastChunkAt: now }
  }
  return null
}

/** 阶段文案（RunningIndicator / StalledHint 共用）。 */
export function runPhaseText(phase: RunStatus["phase"], toolName?: string): string {
  switch (phase) {
    case "thinking":
      return "正在思考…"
    case "tool":
      return toolName ? `正在执行 ${toolName}…` : "正在执行工具…"
    case "streaming":
      return "正在生成回复…"
    default:
      return "处理中…"
  }
}

/** 是否应显示静默提示：running 中且距最近 chunk 超过阈值。 */
export function shouldShowStallHint(
  status: RunStatus,
  now: number,
  thresholdMs: number = STALL_HINT_THRESHOLD_MS,
): boolean {
  if (status.phase === "idle") return false
  return now - status.lastChunkAt >= thresholdMs
}

/** 已耗时秒数（占位气泡 / 静默提示读数）。 */
export function elapsedSeconds(since: number, now: number): number {
  return Math.max(0, Math.floor((now - since) / 1_000))
}
