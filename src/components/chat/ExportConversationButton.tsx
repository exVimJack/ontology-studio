// UI 层：ExportConversationButton.tsx
// 会话导出按钮：下拉选 HTML / Markdown，调用 plugin-dialog save() + plugin-fs
// writeTextFile() 走原生保存对话框（与单条消息导出 ExportButton 同一路径，§13）。
//
// 导出是 UI 展示行为（非业务核心），数据从现有 hooks 拿：
//   - 标题：useConversations 缓存里 find 当前会话
//   - 消息：显式 ipc.listMessages(id, null) 拉全部历史（侧栏默认只取 50 条）

import { useState } from "react"
import { FileDown, ChevronDown, FileCode, FileText, Loader2 } from "lucide-react"
import { useConversations } from "@/hooks/useConversations"
import { ipc } from "@/lib/ipc/commands"
import { saveTextFile } from "@/lib/save-file"
import {
  extractConversation,
  renderHtml,
  renderMarkdown,
} from "@/lib/export"
import type { MessageRow } from "@/lib/domain"

interface Props {
  conversationId: string
}

type Format = "html" | "markdown"

export function ExportConversationButton({ conversationId }: Props) {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const { data: conversations = [] } = useConversations()

  const safeTitle = () => {
    const c = conversations.find((x) => x.id === conversationId)
    return c?.title || "未命名会话"
  }

  /** 文件名安全化：去掉路径分隔符等非法字符。 */
  const safeFileName = (title: string) =>
    title.replace(/[\\/:*?"<>|]/g, "_").slice(0, 60) || "未命名会话"

  const doExport = async (format: Format) => {
    if (busy) return
    setOpen(false)
    setBusy(true)
    try {
      // 拉全部历史（null = 不限条数）
      const messages: MessageRow[] = await ipc.listMessages(conversationId, null)
      const title = safeTitle()

      // 提取 IR（唯一碰 MessageRow 的地方），HTML/MD 两条路径共享
      const data = extractConversation(title, messages)

      let content: string
      let defaultPath: string
      let filters: { name: string; extensions: string[] }[]

      if (format === "html") {
        content = renderHtml(data)
        defaultPath = `${safeFileName(title)}.html`
        filters = [{ name: "HTML", extensions: ["html"] }]
      } else {
        content = renderMarkdown(data)
        defaultPath = `${safeFileName(title)}.md`
        filters = [{ name: "Markdown", extensions: ["md"] }]
      }

      await saveTextFile(content, defaultPath, filters)
    } catch (e) {
      console.error("导出会话失败", e)
      // 简单反馈：复用 notification
      try {
        const { sendNotification } = await import("@tauri-apps/plugin-notification")
        sendNotification({
          title: "导出失败",
          body: e instanceof Error ? e.message : String(e),
        })
      } catch {
        // notification 插件不可用时静默
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="relative">
      <button
        onClick={() => setOpen((v) => !v)}
        disabled={busy}
        className="flex items-center gap-1.5 rounded-full border border-border px-3 py-1 text-xs text-fg-subtle transition-colors hover:bg-bg-hover hover:text-fg disabled:opacity-50"
        title="导出本次会话"
      >
        {busy ? <Loader2 size={12} className="animate-spin" /> : <FileDown size={12} />}
        <span className="max-md:hidden">导出</span>
        <ChevronDown size={12} className={`transition-transform ${open ? "rotate-180" : ""}`} />
      </button>

      {open && !busy && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setOpen(false)} />
          <div className="absolute right-0 top-full z-50 mt-1 w-52 rounded-lg border border-border bg-bg-elevated shadow-xl">
            <button
              onClick={() => doExport("html")}
              className="flex w-full items-start gap-2.5 px-3 py-2 text-left text-xs hover:bg-bg-hover"
            >
              <FileCode size={14} className="mt-0.5 shrink-0 text-accent" />
              <span className="min-w-0">
                <span className="block font-medium text-fg">HTML（自包含）</span>
                <span className="block text-fg-subtle">单文件，样式与应用一致，便于分享</span>
              </span>
            </button>
            <button
              onClick={() => doExport("markdown")}
              className="flex w-full items-start gap-2.5 px-3 py-2 text-left text-xs hover:bg-bg-hover"
            >
              <FileText size={14} className="mt-0.5 shrink-0 text-fg-muted" />
              <span className="min-w-0">
                <span className="block font-medium text-fg">Markdown</span>
                <span className="block text-fg-subtle">通用格式，便于二次编辑</span>
              </span>
            </button>
          </div>
        </>
      )}
    </div>
  )
}
