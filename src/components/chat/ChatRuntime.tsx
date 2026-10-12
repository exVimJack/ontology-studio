// UI 层：ChatRuntime.tsx
// 把现有 useMessages(TanStack Query) + useChat(Tauri Channel 流式) 桥接到
// assistant-ui 的 ExternalStoreRuntime，供 Thread/Message Primitives 渲染。
//
// 职责分离（少包装原则）：
//   - 本文件只负责「把 messages + isRunning 喂给 runtime 供渲染」
//   - 发送逻辑保留在 Composer（调 useChat.sendAsync），不经 runtime 的 onNew
//   - reasoning 开关由 composer-store 管理，Composer 读取并随 sendAsync 传递
//
// 直接传原始 MessageRow[] 给 runtime，convertMessage 做 MessageRow→ThreadMessageLike。
// toolCalls（运行时态，不落库）通过 ref 注入 convertMessage 闭包。

import { createContext, useContext, type ReactNode } from "react"
import {
  AssistantRuntimeProvider,
  useExternalStoreRuntime,
  type AppendMessage,
  type ThreadMessageLike,
} from "@assistant-ui/react"
import { useMessages } from "@/hooks/useConversations"
import { useChat } from "@/hooks/useChat"
import { useRunStatusStore } from "@/stores/run-status-store"
import type { MessageRow } from "@/lib/domain"
import type { RunStatus } from "@/lib/chat-run-status"

/** 长任务 liveness 状态下发：useChat 在 Composer（发送方）与 ChatRuntimeProvider
 *  （渲染方）各持一个 hook 实例，状态互不可见，故 runStatus 走全局 zustand store
 *  （run-status-store），此处直接读 store 并经 context 下发给 Thread 消息组件。 */
interface ChatRunStatusValue {
  runStatus: RunStatus
  isRunning: boolean
}

const ChatRunStatusContext = createContext<ChatRunStatusValue | null>(null)

/** Thread / 消息组件内读取本轮 turn 运行状态（phase / 工具名 / 最近 chunk 时间）。 */
export function useChatRunStatus(): ChatRunStatusValue {
  const v = useContext(ChatRunStatusContext)
  if (!v) throw new Error("useChatRunStatus 必须在 ChatRuntimeProvider 内使用")
  return v
}

/** MessageRow → assistant-ui ThreadMessageLike。
 *  content parts 顺序：reasoning（思考链）→ text（正文）。
 *  工具调用不产出 part（卡片已下线）——此前 tool-call part 恒插在 text 前，
 *  多 turn 流（导语→工具→正文）中卡片出现瞬间导语从卡片上方跳到下方重新渲染，
 *  视觉上等于已输出的文本被“重复输出一遍”。 */
function convertMessage(
  msg: MessageRow,
): ThreadMessageLike {
  const parts: Exclude<ThreadMessageLike["content"], string>[number][] = []

  // reasoning（独立 part，assistant-ui 自动渲染为可折叠块）
  if (msg.reasoning && msg.reasoning.length > 0) {
    parts.push({ type: "reasoning", text: msg.reasoning })
  }

  // 正文
  if (msg.content && msg.content.length > 0) {
    parts.push({ type: "text", text: msg.content })
  }

  return {
    role: msg.role === "assistant" ? "assistant" : "user",
    content: parts,
    id: msg.id,
    // status 只允许 assistant 消息携带；user 消息传 status 会触发
    // fromThreadMessageLike 的 “status is only supported for assistant messages” 错误。
    ...(msg.role === "assistant" ? { status: statusOf(msg) } : {}),
    createdAt: new Date(msg.created_at),
  }
}

/** MessageRow.status → assistant-ui MessageStatus。 */
function statusOf(msg: MessageRow): ThreadMessageLike["status"] {
  switch (msg.status) {
    case "streaming":
      return { type: "running" }
    case "error":
      return { type: "incomplete", reason: "error", error: msg.error ?? "出错了" }
    case "cancelled":
      return { type: "incomplete", reason: "cancelled" }
    case "complete":
    default:
      return { type: "complete", reason: "stop" }
  }
}

/** 从 AppendMessage.content（part 数组）提取所有 text part 的文本拼接。 */
function extractTextFromParts(
  content: AppendMessage["content"],
): string {
  if (typeof content === "string") return content
  return content
    .filter((p) => p.type === "text")
    .map((p) => (p as { type: "text"; text: string }).text)
    .join("")
}

/** 单会话的 assistant-ui runtime（仅渲染用）。
 *  useChat 提供消息、isSending；发送由 Composer 直接调 useChat。 */
export function useChatRuntime(conversationId: string) {
  const { data: messages = [] } = useMessages(conversationId)
  const chat = useChat(conversationId)

  const runtime = useExternalStoreRuntime({
    messages,
    convertMessage: convertMessage,
    isRunning: chat.isSending,
    onNew: async () => {
      // 发送走 Composer → useChat.sendAsync，runtime 不主动调
    },
    onCancel: async () => {
      await chat.stop()
    },
    // 重新生成 assistant 回复：parentId = 前序 user 消息 id
    onReload: async (parentId) => {
      if (parentId) await chat.reload(parentId)
    },
    // 编辑 user 消息后重发：AppendMessage.parentId = 被编辑 user 消息 id
    onEdit: async (message: AppendMessage) => {
      const editedId = message.parentId
      if (!editedId) return
      // 从 content parts 提取文本（仅取 text part，忽略 image/file）
      const text = extractTextFromParts(message.content)
      await chat.editAndResend(editedId, text)
    },
  })

  return { runtime, chat }
}

export function ChatRuntimeProvider({
  conversationId,
  children,
}: {
  conversationId: string
  children: ReactNode
}) {
  const { runtime, chat } = useChatRuntime(conversationId)
  const runStatus = useRunStatusStore((s) => s.runStatus)
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatRunStatusContext.Provider
        value={{ runStatus, isRunning: chat.isSending }}
      >
        {children}
      </ChatRunStatusContext.Provider>
    </AssistantRuntimeProvider>
  )
}
