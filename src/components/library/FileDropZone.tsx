// UI 居：FileDropZone.tsx（§14.2 / 决策 F10）
// 全局拖拽落点：监听 Tauri webview 的 onDragDropEvent 拿真实文件路径。
// drop 时调 useIngest.ingest(paths)。enter/over 显示遮罩，leave 隐藏。
// 移动端无拖拽（§F10），退化为文件选择器（此处不处理，由 Composer 的 + 按钮覆盖）。

import { useEffect, useRef, useState } from "react"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import { isTauri } from "@tauri-apps/api/core"
import { UploadCloud } from "lucide-react"
import { useIngest } from "@/hooks/useIngest"
import { useCurrentConversationId } from "@/hooks/useCurrentConversationId"
import { useUiStore } from "@/stores/ui-store"

export function FileDropZone() {
  const [dragging, setDragging] = useState(false)
  const conversationId = useCurrentConversationId()
  const ingest = useIngest(conversationId)
  const libraryUploadFolder = useUiStore((s) => s.libraryUploadFolder)
  const libraryViewActive = useUiStore((s) => s.libraryViewActive)
  // ref 存最新值，避免 folder 切换时重注册 onDragDropEvent
  const folderRef = useRef(libraryUploadFolder)
  const activeRef = useRef(libraryViewActive)
  folderRef.current = libraryUploadFolder
  activeRef.current = libraryViewActive

  useEffect(() => {
    // 非在 Tauri 环境（浏览器 dev）跳过，避免 getCurrentWebview() 崩溃
    if (!isTauri()) return

    let unlisten: (() => void) | undefined
    let active = true

    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (!active) return
        const e = event.payload
        if (e.type === "enter" || e.type === "over") {
          setDragging(true)
        } else if (e.type === "leave") {
          setDragging(false)
        } else if (e.type === "drop") {
          setDragging(false)
          if (e.paths.length > 0) {
            // 会话页上传 → folderPath=null（后端落 /Inbox）；
            // Library 上传 → 用当前选中的文件夹（folderRef），null=根目录散文件
            const folderPath =
              !conversationId && activeRef.current ? folderRef.current : null
            ingest.mutate({ paths: e.paths, folderPath })
          }
        }
      })
      .then((fn) => {
        if (!active) {
          fn()
        } else {
          unlisten = fn
        }
      })
      .catch(() => {
        // 监听失败静默
      })

    return () => {
      active = false
      unlisten?.()
    }
  }, [ingest])

  if (!dragging) return null

  return (
    <div className="pointer-events-none fixed inset-0 z-40 flex items-center justify-center bg-accent/10 backdrop-blur-sm">
      <div className="flex flex-col items-center gap-3 rounded-2xl border-2 border-dashed border-accent bg-bg/90 px-12 py-8 shadow-xl">
        <UploadCloud size={48} className="text-accent" />
        <div className="text-lg font-medium">松开以摄入文件</div>
        <div className="text-xs text-fg-muted">PDF / Office / 文本 / 图片 / 压缩包</div>
      </div>
    </div>
  )
}
