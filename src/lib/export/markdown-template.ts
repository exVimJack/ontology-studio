// 模板层：export/markdown-template.ts
// ConversationExportData → Markdown 字符串。
//
// 纯数据→文本转换，不碰 HTML/CSS。与 html-template.ts 共享同一份 IR
// （extract.ts 提取），消除原本两份重复的 meta/reasoning/error 取舍逻辑。

import type { ConversationExportData, ExportMessageData } from "./types"

function formatTime(ms: number): string {
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

function messageToMarkdown(msg: ExportMessageData): string {
  const time = formatTime(msg.createdAt)
  const role = msg.role === "user" ? "🧑 你" : "🤖 助手"
  const metaParts = [role, time]
  if (msg.model) metaParts.push(msg.model)
  if (msg.totalTokens) metaParts.push(`${msg.totalTokens} tokens`)

  const lines: string[] = []
  lines.push(`### ${metaParts.join(" · ")}`)
  lines.push("")

  if (msg.reasoning && msg.reasoning.length > 0) {
    // <details> 大多数 md 渲染器支持，折叠思考链
    lines.push("<details><summary>思考过程</summary>")
    lines.push("")
    lines.push(msg.reasoning)
    lines.push("")
    lines.push("</details>")
    lines.push("")
  }

  if (msg.content) {
    lines.push(msg.content)
    lines.push("")
  }

  if (msg.error) {
    lines.push(`> ⚠ ${msg.error}`)
    lines.push("")
  }

  lines.push("---")
  lines.push("")
  return lines.join("\n")
}

/**
 * 生成 Markdown 字符串。每条消息用三级标题 + 正文，reasoning 折叠。
 */
export function renderMarkdown(data: ConversationExportData): string {
  const title = data.title || "未命名会话"
  const lines: string[] = []
  lines.push(`# ${title}`)
  lines.push("")
  lines.push(`> 共 ${data.messages.length} 条消息 · 由 onto-studio 导出 · ${formatTime(data.exportedAt)}`)
  lines.push("")
  lines.push("---")
  lines.push("")
  for (const msg of data.messages) {
    lines.push(messageToMarkdown(msg))
  }
  return lines.join("\n")
}
