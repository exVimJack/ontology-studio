// UI 层：ScopeChip.tsx
// 会话范围 chip（CONVERSATION-SCOPE.md §5.2）：显示当前会话激活的文件夹 + 数据源，
// 点开 popover 勾选。激活集为空时显示「未挂载知识源」（模型按通用能力回答）。
//
// 约束（§12.1）：本组件通过 hooks 调 IPC，不直接 import ipc/commands。

import { useMemo, useState } from "react"
import { ChevronDown, Folder, MessageSquare, Database, Check, FileText, X } from "lucide-react"
import { useActiveScope, useToggleActiveFolder, useToggleActiveSource } from "@/hooks/useActiveScope"
import { useFolders } from "@/hooks/useFolders"
import { useDataSources } from "@/hooks/useFederation"
import { useMountedDocuments, useUnmountDocument } from "@/hooks/useMountedDocuments"
import { getFileIcon } from "@/lib/file-icons"
import type { FolderNodeDto } from "@/lib/domain"

interface Props {
  conversationId: string
}

export function ScopeChip({ conversationId }: Props) {
  const [open, setOpen] = useState(false)
  const { data: scope } = useActiveScope(conversationId)
  const { data: tree = [] } = useFolders()
  const { data: sources = [] } = useDataSources()
  const { data: mountedDocs = [] } = useMountedDocuments(conversationId)
  const toggleFolder = useToggleActiveFolder()
  const toggleSource = useToggleActiveSource()
  const unmount = useUnmountDocument()

  const activeFolders = scope?.folders ?? []
  const activeSources = scope?.sources ?? []
  const activeDocCount = mountedDocs.length

  const totalActive = activeFolders.length + activeSources.length + activeDocCount

  const summary = useMemo(() => {
    if (totalActive === 0) return null
    const parts: string[] = []
    if (activeFolders.length > 0) {
      parts.push(activeFolders.length === 1 ? activeFolders[0] : `${activeFolders.length} 个文件夹`)
    }
    if (activeDocCount > 0) {
      parts.push(`${activeDocCount} 个文件`)
    }
    if (activeSources.length > 0) {
      parts.push(activeSources.length === 1 ? activeSources[0] : `${activeSources.length} 个数据源`)
    }
    return parts.join(" · ")
  }, [totalActive, activeFolders, activeDocCount, activeSources])

  return (
    <div className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        className={`flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs transition-colors ${
          totalActive === 0
            ? "border-border text-fg-subtle hover:bg-bg-hover"
            : "border-accent/30 bg-accent/10 text-accent hover:bg-accent/15"
        }`}
      >
        {totalActive === 0 ? (
          <>
            <MessageSquare size={12} />
            <span>未挂载知识源</span>
          </>
        ) : (
          <>
            <Folder size={12} />
            <span className="max-w-[180px] truncate max-md:max-w-[120px]">{summary}</span>
          </>
        )}
        <ChevronDown size={12} className={`transition-transform ${open ? "rotate-180" : ""}`} />
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute left-0 top-full z-50 mt-1 w-80 max-w-[calc(100vw-1.5rem)] rounded-lg border border-border bg-bg-elevated shadow-xl">
            <div className="border-b border-border px-3 py-2 text-xs font-medium text-fg">
              本次会话范围
            </div>

            <div className="max-h-96 overflow-auto py-1">
              {/* 知识库文件夹 */}
              <div className="px-3 py-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                <Folder size={10} className="mr-1 inline" />
                知识库文件夹
              </div>
              {tree.length === 0 && (
                <div className="px-3 py-1 text-xs text-fg-subtle">暂无文件夹</div>
              )}
              {tree.map((node) => (
                <FolderCheckItem
                  key={node.path}
                  node={node}
                  depth={0}
                  activeFolders={activeFolders}
                  conversationId={conversationId}
                  toggleFolder={toggleFolder}
                />
              ))}

              {/* 数据源 */}
              <div className="mt-2 border-t border-border px-3 py-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                <Database size={10} className="mr-1 inline" />
                数据源
              </div>
              {sources.length === 0 && (
                <div className="px-3 py-1 text-xs text-fg-subtle">暂无数据源</div>
              )}
              {sources.map((s) => {
                const checked = activeSources.includes(s.name)
                return (
                  <button
                    key={s.id}
                    onClick={() =>
                      toggleSource.mutate({
                        conversationId,
                        source: s.name,
                        currentSources: activeSources,
                      })
                    }
                    className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-bg-hover"
                  >
                    <span className={`w-4 ${checked ? "text-accent" : "text-transparent"}`}>
                      <Check size={12} />
                    </span>
                    <Database size={12} className="text-fg-subtle" />
                    <span className="truncate text-fg">{s.name}</span>
                    <span className="ml-auto text-[10px] text-fg-subtle">{s.kind}</span>
                  </button>
                )
              })}
            </div>

            {/* @挂载的单文件：可卸载（方案 C：原 Inspector 挂载文档面板收进此处） */}
            {activeDocCount > 0 && (
              <>
                <div className="mt-2 border-t border-border px-3 py-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                  <FileText size={10} className="mr-1 inline" />
                  挂载文件（{activeDocCount}）
                </div>
                <ul className="py-1">
                  {mountedDocs.map((d) => {
                    const { Icon, className: iconCls } = getFileIcon(d.format, d.name)
                    return (
                      <li
                        key={d.path}
                        className="group flex items-center gap-2 px-3 py-1.5 text-xs hover:bg-bg-hover"
                      >
                        <Icon size={12} className={`shrink-0 ${iconCls}`} />
                        <div className="min-w-0 flex-1">
                          <div className="truncate text-fg">{d.name}</div>
                          {d.char_count > 0 && (
                            <span className="text-[10px] text-fg-subtle">
                              {d.char_count > 1000
                                ? `${Math.round(d.char_count / 1000)}k 字`
                                : `${d.char_count} 字`}
                            </span>
                          )}
                        </div>
                        <button
                          onClick={() =>
                            unmount.mutate({ conversationId, path: d.path })
                          }
                          className="shrink-0 rounded p-0.5 text-fg-subtle opacity-0 hover:bg-bg-hover hover:text-danger group-hover:opacity-100"
                          title="移除挂载"
                        >
                          <X size={12} />
                        </button>
                      </li>
                    )
                  })}
                </ul>
              </>
            )}

            <div className="flex items-center justify-between border-t border-border px-3 py-2">
              <span className="text-[10px] text-fg-subtle">
                {totalActive === 0 ? "空范围 = 通用对话模式" : `已激活 ${totalActive} 项`}
              </span>
              <span className="text-[10px] text-fg-subtle">@ 可引用任意文件</span>
            </div>
          </div>
        </>
      )}
    </div>
  )
}

/** 递归渲染文件夹树 + 勾选（后端已构建层级 + 排序）。 */
function FolderCheckItem({
  node,
  depth,
  activeFolders,
  conversationId,
  toggleFolder,
}: {
  node: FolderNodeDto
  depth: number
  activeFolders: string[]
  conversationId: string
  toggleFolder: ReturnType<typeof useToggleActiveFolder>
}) {
  const checked = activeFolders.includes(node.path)
  return (
    <>
      <button
        onClick={() =>
          toggleFolder.mutate({
            conversationId,
            folder: node.path,
            currentFolders: activeFolders,
          })
        }
        className="flex w-full items-center gap-2 py-1.5 pr-2 text-left text-xs hover:bg-bg-hover"
        style={{ paddingLeft: depth * 12 + 12 }}
      >
        <span className={`w-4 ${checked ? "text-accent" : "text-transparent"}`}>
          <Check size={12} />
        </span>
        <Folder size={12} className="text-fg-subtle" />
        <span className="truncate text-fg">{node.name}</span>
      </button>
      {node.children.map((c) => (
        <FolderCheckItem
          key={c.path}
          node={c}
          depth={depth + 1}
          activeFolders={activeFolders}
          conversationId={conversationId}
          toggleFolder={toggleFolder}
        />
      ))}
    </>
  )
}
