// UI 层：ChatArea.tsx
// 对话主区域：无会话时空态 + 有会话时 Thread(assistant-ui) + Composer。
// ChatRuntimeProvider 把 useMessages+useChat 桥接到 assistant-ui runtime。

import { useCurrentConversationId } from "@/hooks/useCurrentConversationId"
import { Thread } from "@/components/chat/Thread"
import { Composer } from "@/components/chat/Composer"
import { ChatEmptyState } from "@/components/chat/ChatEmptyState"
import { IngestStatusBoard } from "@/components/library/IngestStatusBoard"
import { ScopeChip } from "@/components/chat/ScopeChip"
import { ExportConversationButton } from "@/components/chat/ExportConversationButton"
import { ChatRuntimeProvider } from "@/components/chat/ChatRuntime"
import { ThreadErrorBoundary } from "@/components/chat/ThreadErrorBoundary"

export function ChatArea() {
  const conversationId = useCurrentConversationId()

  if (!conversationId) {
    return (
      <div className="flex h-full flex-col bg-bg">
        <ChatEmptyState />
      </div>
    )
  }

  return (
    <ThreadErrorBoundary key={conversationId}>
      <ChatRuntimeProvider conversationId={conversationId}>
        <div className="flex h-full flex-col bg-bg">
          <div className="flex items-center gap-2 border-b border-border px-3 py-1.5 max-md:px-2">
            <ScopeChip conversationId={conversationId} />
            <div className="ml-auto">
              <ExportConversationButton conversationId={conversationId} />
            </div>
          </div>
          <div className="min-h-0 flex-1">
            <Thread />
          </div>
          <IngestStatusBoard conversationId={conversationId} />
          <Composer conversationId={conversationId} />
        </div>
      </ChatRuntimeProvider>
    </ThreadErrorBoundary>
  )
}
