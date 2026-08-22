// State 层：composer-store.ts（§12.2 / §20.6）
// 输入草稿 + 深度思考开关，按会话 ID 分键。切会话/重启不丢。

import { create } from "zustand"
import { persist } from "zustand/middleware"

interface ComposerState {
  drafts: Record<string, string>
  /** 深度思考开关（按会话 id 分键）。默认关闭。 */
  reasoningEnabled: Record<string, boolean>
  getDraft: (conversationId: string) => string
  setDraft: (conversationId: string, text: string) => void
  clearDraft: (conversationId: string) => void
  getReasoningEnabled: (conversationId: string) => boolean
  setReasoningEnabled: (conversationId: string, enabled: boolean) => void
}

export const useComposerStore = create<ComposerState>()(
  persist(
    (set, get) => ({
      drafts: {},
      reasoningEnabled: {},
      getDraft: (id) => get().drafts[id] ?? "",
      setDraft: (id, text) =>
        set((s) => ({ drafts: { ...s.drafts, [id]: text } })),
      clearDraft: (id) =>
        set((s) => {
          const next = { ...s.drafts }
          delete next[id]
          return { drafts: next }
        }),
      getReasoningEnabled: (id) => get().reasoningEnabled[id] ?? false,
      setReasoningEnabled: (id, enabled) =>
        set((s) => ({
          reasoningEnabled: { ...s.reasoningEnabled, [id]: enabled },
        })),
    }),
    { name: "onto-studio-composer" },
  ),
)
