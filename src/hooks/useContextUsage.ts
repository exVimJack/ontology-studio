// Hooks 层：useContextUsage.ts
// 会话上下文用量（借鉴 Hermes TUI 状态栏「28k/128k tok · 45%」）。
//
// 数据来源：
//   - max：后端 get_context_window（ChatService 压缩策略实际生效的窗口值）
//   - used：最近一条 provider 报告过 usage 的消息的 prompt_tokens + completion_tokens
//     —— 该请求包含了完整历史 + 本轮 prompt，是「下一次请求体积」最准的实测值。
//     provider 未报 usage（旧消息/部分 provider）时为 null（不显示）。
//
// 用途：Composer 上方的用量条。颜色分级对齐压缩阈值（memory_bridge.rs 分档）：
//   ≥75%（主流窗口触发压缩的比例）转 warning，≥90% 转 danger。

import { useQuery } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc/commands";
import type { MessageRow } from "@/lib/domain";

const QK_CTX_WINDOW = ["context-window"] as const;

/** 后端解析的上下文窗口（token）。null = 未配置 provider。 */
export function useContextWindow() {
  return useQuery<number | null>({
    queryKey: QK_CTX_WINDOW,
    queryFn: () => ipc.getContextWindow(),
    // 窗口只在重设 provider 时变；发送/切会话无需重拉（发送后 invalidate 即可）
    staleTime: Infinity,
  });
}

export interface ContextUsage {
  /** 已用 token（最近一次请求的实测 prompt+completion）。null = 无 usage 数据。 */
  used: number | null;
  /** 窗口 token。null = 未知（未配置 provider）。 */
  max: number | null;
  /** 占用率 0~1。null = used/max 任一缺失。 */
  ratio: number | null;
}

/** 从消息列表计算会话上下文用量（见文件头注释的口径）。 */
export function computeContextUsage(
  messages: MessageRow[] | undefined,
  window: number | null,
): ContextUsage {
  if (!window || !messages) return { used: null, max: window ?? null, ratio: null };
  // 从最新往回找第一条报过 prompt_tokens 的消息（通常是最后的 assistant 回复）
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m.prompt_tokens != null && m.prompt_tokens > 0) {
      const used = m.prompt_tokens + (m.completion_tokens ?? 0);
      return { used, max: window, ratio: Math.min(used / window, 1) };
    }
  }
  return { used: null, max: window, ratio: null };
}

/** 格式化 token 数（十进制，与模型官方口径一致）：
 *  1000000 → "1M"，128000 → "128k"，59400 → "59.4k"，999 → "999"。
 *  不用 1024 进制——模型窗口的 "1M/128K" 是十进制营销数字，
 *  二进制格式化会把 1000000 显示成 "977k"，用户观感"缩水"且易误导。 */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return `${m >= 10 ? Math.round(m) : Math.round(m * 10) / 10}M`;
  }
  if (n >= 1000) {
    const k = n / 1000;
    return `${k >= 100 ? Math.round(k) : Math.round(k * 10) / 10}k`;
  }
  return String(n);
}
