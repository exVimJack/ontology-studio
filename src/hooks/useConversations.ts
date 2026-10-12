// Hooks 层：useConversations.ts（§12.2）
// TanStack Query 封装会话列表 CRUD。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useNavigate } from "@tanstack/react-router"
import { ipc } from "@/lib/ipc/commands"
import type { CreateConversationInput, MessageRow } from "@/lib/domain"
import { removeMountedCache } from "@/hooks/useMountedDocuments"
import { activeRunConversations } from "@/hooks/useChat"

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
  const qc = useQueryClient()
  return useQuery({
    queryKey: conversationId ? QK_MESSAGES(conversationId) : ["messages", "none"],
    queryFn: async () => {
      if (!conversationId) return []
      const rows = await ipc.listMessages(conversationId, DEFAULT_MESSAGE_LIMIT)
      // 运行中会话保护：turn 整轮结束才落库，运行中 DB 拉到的是上一轮为止的消息；
      // 若 DB 结果比缓存少（切走再切回的 remount refetch），保留缓存——否则流式中的
      // 消息会被空/旧列表覆盖（"正在运行的会话不见了"）。run 结束时 onSuccess
      // 会 invalidate，彼时 DB 已有本轮数据，正常刷新。
      if (activeRunConversations.has(conversationId)) {
        const cached = qc.getQueryData<MessageRow[]>(QK_MESSAGES(conversationId))
        if (cached && cached.length > rows.length) return cached
      }
      return rows
    },
    enabled: !!conversationId,
  })
}

/** 拉取会话全部消息（limit=null，导出等需要完整历史的场景；不参与运行中保护缓存）。 */
export function useAllMessages(conversationId: string | null) {
  return useQuery({
    queryKey: ["messages-all", conversationId ?? ""],
    queryFn: async () => {
      if (!conversationId) return []
      return await ipc.listMessages(conversationId, null)
    },
    enabled: !!conversationId,
  })
}
