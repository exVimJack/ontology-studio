// Hooks 层：useConversations.ts（§12.2）
// TanStack Query 封装会话列表 CRUD。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useNavigate } from "@tanstack/react-router"
import { ipc } from "@/lib/ipc/commands"
import type { CreateConversationInput } from "@/lib/domain"
import { removeMountedCache } from "@/hooks/useMountedDocuments"

const QK_CONVERSATIONS = ["conversations"] as const
const QK_MESSAGES = (id: string) => ["messages", id] as const
export function useConversations() {
  return useQuery({
    queryKey: QK_CONVERSATIONS,
    queryFn: () => ipc.listConversations(),
  })
}

export function useCreateConversation() {
  const qc = useQueryClient()
  const navigate = useNavigate()
  return useMutation({
    mutationFn: (input: CreateConversationInput) => ipc.createConversation(input),
    onSuccess: (conv) => {
      qc.invalidateQueries({ queryKey: QK_CONVERSATIONS })
      // 路由为真相源：新建后导航到该会话（决策 F4）
      navigate({ to: "/chat/$conversationId", params: { conversationId: conv.id } })
    },
  })
}

export function useRenameConversation() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, title }: { id: string; title: string }) =>
      ipc.renameConversation(id, title),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK_CONVERSATIONS }),
  })
}

/** 自动生成会话标题（LLM 概括首条用户消息）。
 *
 * 触发条件由调用方判定（首条 AI 回复结束 + 标题仍为默认 “新会话”）。
 * 失败时调用方应降级为截断式兜底（deriveTitle）。
 * 成功后 invalidate 会话列表，侧栏立即更新。 */
export function useGenerateTitle() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => ipc.generateConversationTitle(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK_CONVERSATIONS }),
  })
}

export function useTogglePinned() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, pinned }: { id: string; pinned: boolean }) =>
      ipc.setConversationPinned({ id, pinned }),
    onSuccess: () => qc.invalidateQueries({ queryKey: QK_CONVERSATIONS }),
  })
}

export function useDeleteConversation() {
  const qc = useQueryClient()
  const navigate = useNavigate()
  return useMutation({
    mutationFn: (id: string) => ipc.deleteConversation(id),
    onSuccess: (_, id) => {
      qc.invalidateQueries({ queryKey: QK_CONVERSATIONS })
      qc.removeQueries({ queryKey: QK_MESSAGES(id) })
      // 清理挂载列表缓存（后端 ON 会话删除已清关联，这里只清前端 cache）
      removeMountedCache(qc, id)
      // 路由为真相源：删除后回到空态（决策 F4）
      navigate({ to: "/" })
    },
  })
}

/** 加载对话历史时默认取最近 N 条消息（避免超长会话一次性拉全量卡顿）。 */
const DEFAULT_MESSAGE_LIMIT = 50;

export function useMessages(conversationId: string | null) {
  return useQuery({
    queryKey: conversationId ? QK_MESSAGES(conversationId) : ["messages", "none"],
    queryFn: () => (conversationId ? ipc.listMessages(conversationId, DEFAULT_MESSAGE_LIMIT) : Promise.resolve([])),
    enabled: !!conversationId,
  })
}
