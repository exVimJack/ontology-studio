// Hooks 层：useMountedDocuments.ts
// 会话已挂载文档（`@` 持久化）的查询 + 挂载/卸载 mutation。
//
// 数据流：ingest 摄取后后端已 upsert documents 表全文 + 前端 mountDocument 记关联。
// 切走会话再回来，本 hook 重查 listMountedDocuments 恢复挂载列表（不再依赖内存）。
// 发送消息时 context_texts 由 Composer 按挂载列表从 documents 表读全文（见 useChat）。
//
// 设计：用 TanStack Query 管理服务端状态（挂载列表是持久化数据，非纯 UI 状态）。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type { MountedDocDto } from "@/lib/domain"

const QK_MOUNTED = (id: string) => ["mounted-docs", id] as const
const QK_ALL_DOCS = ["all-documents"] as const
const QK_DOC_CONTENT = (id: string) => ["document-content", id] as const

/** 列出会话已挂载的文档（不含全文）。切会话自动重查。 */
export function useMountedDocuments(conversationId: string | null) {
  return useQuery({
    queryKey: conversationId ? QK_MOUNTED(conversationId) : ["mounted-docs", "none"],
    queryFn: () =>
      conversationId
        ? ipc.listMountedDocuments(conversationId)
        : Promise.resolve<MountedDocDto[]>([]),
    enabled: !!conversationId,
  })
}

/** 列出知识库全部入库文档（不含全文，`@` 菜单 / Library 视图用）。 */
export function useAllDocuments() {
  return useQuery({
    queryKey: QK_ALL_DOCS,
    queryFn: () => ipc.listAllDocuments(),
  })
}

/** 按 id 读文档全文（预览抽屉用）。 */
export function useDocumentContent(id: string | null) {
  return useQuery({
    queryKey: id ? QK_DOC_CONTENT(id) : ["document-content", "none"],
    queryFn: () => (id ? ipc.readDocument(id) : Promise.resolve(null)),
    enabled: !!id,
  })
}

/** 挂载文档到会话（path 必须已 ingest 入库）。成功后刷新挂载列表 + 全局列表。 */
export function useMountDocument() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ conversationId, path }: { conversationId: string; path: string }) =>
      ipc.mountDocument(conversationId, path),
    onSuccess: (_, { conversationId }) => {
      qc.invalidateQueries({ queryKey: QK_MOUNTED(conversationId) })
    },
  })
}

/** 卸载会话下的某篇文档。成功后刷新挂载列表。 */
export function useUnmountDocument() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ conversationId, path }: { conversationId: string; path: string }) =>
      ipc.unmountDocument(conversationId, path),
    onSuccess: (_, { conversationId }) => {
      qc.invalidateQueries({ queryKey: QK_MOUNTED(conversationId) })
    },
  })
}

/** 删除知识库文档（清全文 + FTS5 + 所有会话挂载关联）。 */
export function useDeleteDocument() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (path: string) => ipc.deleteDocument(path),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_ALL_DOCS })
      // 删除文件后：当前文件夹文件列表 + 文件夹树（可能空文件夹消失）都需刷新
      qc.invalidateQueries({ queryKey: ["folder-docs"] })
      qc.invalidateQueries({ queryKey: ["folders"] })
      // 所有会话的挂载列表可能含该 path，统一 invalidate（后端已清关联）
      qc.invalidateQueries({ queryKey: ["mounted-docs"] })
    },
  })
}

/** 供 deleteConversation 清理 mounted query cache 用。 */
export function removeMountedCache(qc: ReturnType<typeof useQueryClient>, id: string) {
  qc.removeQueries({ queryKey: QK_MOUNTED(id) })
}
