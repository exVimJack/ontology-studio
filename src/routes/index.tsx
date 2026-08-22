// 路由：/ （空态/首页）
// 无会话时渲染 ChatArea 的空态（conversationId 为 null）。
// 新建会话由 ⌘N / Sidebar + 按钮触发，导航到 /chat/$id。

import { createFileRoute } from "@tanstack/react-router"
import { ChatArea } from "@/components/chat/ChatArea"

export const Route = createFileRoute("/")({
  component: () => <ChatArea />,
})
