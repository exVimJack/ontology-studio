// UI 层：ContextUsageIndicator.tsx
// 会话上下文用量指示（借鉴 Hermes TUI 状态栏「28k/128k tok · 45%」）。
//
// 展示最近一次请求实测的 prompt+completion token / 当前压缩窗口。
// 颜色分级对齐压缩阈值（agent-core memory_bridge.rs 分档）：
//   < 75%  muted（正常）
//   ≥ 75%  warning（接近主流窗口的压缩触发点）
//   ≥ 90%  danger（高危：窗口即将耗尽）
// 无 usage 数据（provider 未报告）时整条隐藏，不占位。
//
// 两个变体：
//   - ContextUsageIndicator：完整徽标（百分比 + 进度条 + token 数）
//   - ContextUsageInline：紧凑文字版（仅百分比 + token 数），用于 Composer 工具行

import { useMessages } from "@/hooks/useConversations"
import {
  computeContextUsage,
  formatTokens,
  useContextWindow,
} from "@/hooks/useContextUsage"

function useUsage(conversationId: string) {
  const { data: window } = useContextWindow()
  const { data: messages } = useMessages(conversationId)
  return computeContextUsage(messages, window ?? null)
}

const TITLE =
  "最近一次请求 tokens（prompt + completion）／上下文窗口。达到 75%（主流窗口压缩阈值）后历史将被自动摘要压缩。"

function toneClass(ratio: number): string {
  return ratio >= 0.9
    ? "text-danger-text"
    : ratio >= 0.75
      ? "text-warning-text"
      : "text-fg-subtle"
}

/** 完整徽标（百分比 + 进度条 + token 数）。 */
export function ContextUsageIndicator({ conversationId }: { conversationId: string }) {
  const { used, max, ratio } = useUsage(conversationId)
  if (used == null || max == null || ratio == null) return null

  const pct = Math.round(ratio * 100)
  const barTone = ratio >= 0.9 ? "bg-danger-text" : ratio >= 0.75 ? "bg-warning-text" : "bg-accent"

  return (
    <div
      className={`flex items-center gap-2 rounded-full border border-border bg-bg-hover px-2.5 py-0.5 text-[11px] leading-none ${toneClass(ratio)}`}
      title={TITLE}
    >
      <span className="tabular-nums">{pct}%</span>
      <span className="h-1 w-10 overflow-hidden rounded-full bg-border">
        <span className={`block h-full ${barTone}`} style={{ width: `${Math.min(pct, 100)}%` }} />
      </span>
      <span className="tabular-nums whitespace-nowrap">
        {formatTokens(used)}/{formatTokens(max)}
      </span>
    </div>
  )
}

/** 紧凑内联版（百分比 + 进度条 + token 数），用于 Composer 工具行。 */
export function ContextUsageInline({ conversationId }: { conversationId: string }) {
  const { used, max, ratio } = useUsage(conversationId)
  if (used == null || max == null || ratio == null) return null

  const pct = Math.round(ratio * 100)
  const barTone = ratio >= 0.9 ? "bg-danger-text" : ratio >= 0.75 ? "bg-warning-text" : "bg-accent"

  return (
    <span
      className={`hidden shrink-0 items-center gap-1.5 rounded-md bg-bg-hover px-2 py-1 text-[11px] leading-none tabular-nums sm:flex ${toneClass(ratio)}`}
      title={TITLE}
    >
      <span>{pct}%</span>
      <span className="h-1 w-12 overflow-hidden rounded-full bg-border">
        <span className={`block h-full ${barTone}`} style={{ width: `${Math.min(pct, 100)}%` }} />
      </span>
      <span className="whitespace-nowrap text-fg-subtle">
        {formatTokens(used)}/{formatTokens(max)}
      </span>
    </span>
  )
}
