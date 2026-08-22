// 路由：/chat/$conversationId （对话视图）
// conversationId 由路由参数提供（唯一真相源，决策 F4）。
// ChatArea 经 useCurrentConversationId() 读取路由 params 渲染对话。
//
// loader：进入会话前预取消息历史（F4 承诺的“进入会话前预取”场景）。
//
// 重要：loader **不 await** prefetchQuery——TanStack Router 的 loader 是阻塞的，
// await 会延迟路由切换直到 IPC 返回全部消息（超长会话 270KB+ 会卡几秒）。
// 改为不 await：fire-and-forget 预热缓存，路由立即切换，useMessages 拿到缓存命中
// 就直接渲染，拿不到就走正常加载态（isLoading）。切会话不再阻塞。

import { createFileRoute } from "@tanstack/react-router"
import { ChatArea } from "@/components/chat/ChatArea"
import { ipc } from "@/lib/ipc/commands"
import { queryClient } from "@/main"

export const Route = createFileRoute("/chat/$conversationId")({
  // 不 await：fire-and-forget 预热缓存。路由切换不等 IPC 返回，避免超长会话卡顿。
  // useMessages 用相同 queryKey，缓存命中则直接渲染，未命中走 isLoading 加载态。
  loader: ({ params }) => {
    void queryClient.prefetchQuery({
      queryKey: ["messages", params.conversationId],
      queryFn: () => ipc.listMessages(params.conversationId, 50),
    })
  },
  component: () => <ChatArea />,
})
