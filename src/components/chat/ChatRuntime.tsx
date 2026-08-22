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

import { useRef, type ReactNode } from "react"
import {
  AssistantRuntimeProvider,
  useExternalStoreRuntime,
  type AppendMessage,
  type ThreadMessageLike,
} from "@assistant-ui/react"
import { useMessages } from "@/hooks/useConversations"
import { useChat } from "@/hooks/useChat"
import type { MessageRow, ToolCallInfo } from "@/lib/domain"

/** MessageRow → assistant-ui ThreadMessageLike。
 *  content parts 顺序：reasoning（思考链）→ tool-call → text（正文）。 */
function convertMessage(
  msg: MessageRow,
  toolCallsByMsg: Record<string, ToolCallInfo[]>,
): ThreadMessageLike {
  const parts: Exclude<ThreadMessageLike["content"], string>[number][] = []

  // reasoning（独立 part，assistant-ui 自动渲染为可折叠块）
  if (msg.reasoning && msg.reasoning.length > 0) {
    parts.push({ type: "reasoning", text: msg.reasoning })
  }

  // tool-call parts（运行时状态，不落库；按消息 id 查找）
  const tcs = toolCallsByMsg[msg.id]
  if (tcs && tcs.length > 0 && msg.role === "assistant") {
    for (const tc of tcs) {
      parts.push({
        type: "tool-call",
        toolCallId: tc.call_id,
        toolName: tc.name,
        argsText: tc.arguments,
        result:
          tc.result != null
            ? tc.is_error
              ? { type: "error", message: tc.result }
              : tc.result
            : undefined,
        isError: tc.is_error || undefined,
      })
    }
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
 *  useChat 提供消息、isSending、toolCallsByMsg；发送由 Composer 直接调 useChat。 */
export function useChatRuntime(conversationId: string) {
  const { data: messages = [] } = useMessages(conversationId)
  const chat = useChat(conversationId)

  // toolCallsByMsg 每次渲染可能变（state），用 ref 让 convertMessage 闭包始终读到最新值，
  // 同时保持 convertMessage 引用稳定（避免 runtime 重建）。
  const toolCallsRef = useRef(chat.toolCallsByMsg)
  toolCallsRef.current = chat.toolCallsByMsg

  const runtime = useExternalStoreRuntime({
    messages,
    convertMessage: (m: MessageRow) => convertMessage(m, toolCallsRef.current),
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
  const { runtime } = useChatRuntime(conversationId)
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      {children}
    </AssistantRuntimeProvider>
  )
}
