// Hooks 层：useFolders.ts
// 文件夹操作（CONVERSATION-SCOPE.md §3.4）：独立 folders 表持久化空文件夹 +
// documents.folder_path 隐式推导有文件的文件夹（双轨合并去重）。
// listFolders = folders ∪ DISTINCT folder_path；listDocumentsByFolder = 指定文件夹下直接子文件；
// create/move/rename/delete 在 folders 表 + 文件层双轨操作。

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type { FolderNodeDto } from "@/lib/domain"

const QK_FOLDERS = ["folders"] as const
const QK_FOLDER_DOCS = (folder: string | null) => ["folder-docs", folder ?? "/"] as const
const QK_ALL_DOCS = ["all-documents"] as const

/** 列出文件夹树（后端已构建嵌套层级 + 排序，前端直接渲染）。
 *  Inbox 置顶，子文件夹递归。返回 FolderNodeDto[]（树结构）。
 */
export function useFolders() {
  return useQuery<FolderNodeDto[]>({
    queryKey: QK_FOLDERS,
    queryFn: () => ipc.listFolders(),
  })
}

/** 列出指定文件夹下的直接子文件（Library 右栏展示）。folder=null/"/" = 根目录散文件。 */
export function useDocumentsByFolder(folder: string | null) {
  return useQuery({
    queryKey: QK_FOLDER_DOCS(folder),
    queryFn: () => ipc.listDocumentsByFolder(folder),
  })
}

/** 新建空文件夹（持久化）。path 如 "/曾国藩专题"。已存在则忽略。 */
export function useCreateFolder() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (path: string) => ipc.createFolder(path),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_FOLDERS })
    },
  })
}

/** 移动单个文件到目标文件夹。targetFolder=null = 根目录散文件。 */
export function useMoveDocument() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ path, targetFolder }: { path: string; targetFolder: string | null }) =>
      ipc.moveDocument(path, targetFolder),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_FOLDERS })
      qc.invalidateQueries({ queryKey: ["folder-docs"] })
      qc.invalidateQueries({ queryKey: QK_ALL_DOCS })
    },
  })
}

/** 重命名文件夹（递归处理子文件夹）。 */
export function useRenameFolder() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ oldPath, newPath }: { oldPath: string; newPath: string }) =>
      ipc.renameFolder(oldPath, newPath),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_FOLDERS })
      qc.invalidateQueries({ queryKey: ["folder-docs"] })
      qc.invalidateQueries({ queryKey: QK_ALL_DOCS })
    },
  })
}

/** 删除文件夹及其下所有文件（含子文件夹递归）。返回删除的文件数。 */
export function useDeleteFolder() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (folder: string) => ipc.deleteFolder(folder),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_FOLDERS })
      qc.invalidateQueries({ queryKey: ["folder-docs"] })
      qc.invalidateQueries({ queryKey: QK_ALL_DOCS })
      qc.invalidateQueries({ queryKey: ["mounted-docs"] })
    },
  })
}
