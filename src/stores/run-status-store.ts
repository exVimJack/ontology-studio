// State 层：run-status-store.ts
// 长任务 liveness 运行状态的全局 store（zustand）。
//
// 为什么不用 useChat 内部 state：useChat 在 Composer（发送方）与
// ChatRuntimeProvider（渲染方）各持一个 hook 实例，React hook 实例的
// state 互相隔离——Composer 实例里 touchRunStatus 更新不到渲染实例，
// RunningIndicator/StalledHint 永远读不到变化（安装自测实测发现）。
// 消息数据走 TanStack Query 全局缓存无此问题，故时间戳功能正常。
//
// 方案：runStatus 挪到本 store（模块级单例，跨组件共享），
// useChat 两个实例都写同一个 store，Thread 读同一个 store。
// 纯逻辑（nextRunStatus 等）在 @/lib/chat-run-status.ts。

import { create } from "zustand"
import {
  RUN_STATUS_IDLE,
  nextRunStatus,
  type RunStatus,
} from "@/lib/chat-run-status"

interface RunStatusState {
  runStatus: RunStatus
  /** 更新运行状态（nextRunStatus 的 setState 包装：phase/toolName 变化立即生效，
   *  lastChunkAt 至多 1s 更新一次；无变化时不触发订阅者重渲）。 */
  touch: (phase: RunStatus["phase"], toolName?: string) => void
  /** 置回 idle（done / error / stop / onError 时调用）。 */
  reset: () => void
}

export const useRunStatusStore = create<RunStatusState>((set, get) => ({
  runStatus: RUN_STATUS_IDLE,
  touch: (phase, toolName) => {
    const next = nextRunStatus(get().runStatus, phase, toolName, Date.now())
    if (next) {
      set({ runStatus: next })
    }
  },
  reset: () => {
    set({ runStatus: RUN_STATUS_IDLE })
  },
}))
