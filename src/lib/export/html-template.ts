// 模板层：export/html-template.ts
// ConversationExportData → 自包含 HTML 字符串。
//
// 对应 pi export-html 的 generateHtml：模板是独立 .html 文件（?raw import），
// 用占位符填充。模板只消费 IR，不直接碰 MessageRow；markdown→HTML 走
// render-markdown.ts。这样展示皮肤可独立替换，数据层变更不波及模板。
//
// 自包含：CSS 内联（应用页面 CSS + 导出专用布局），离线打开即可看，
// 样式尽量与会话窗口一致。

import htmlTemplate from "./html-template.html?raw"
import type { ConversationExportData, ExportMessageData } from "./types"
import { renderMarkdownToHtml } from "./render-markdown"

// ────────────────────────────────────────────────────────────
// 工具
// ────────────────────────────────────────────────────────────

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;")
}

function formatTime(ms: number): string {
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

// SVG 头像（与 Thread.tsx 的 lucide User/Bot 图标一致）
const USER_AVATAR_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>`
const BOT_AVATAR_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="10" x="3" y="11" rx="2"/><circle cx="12" cy="5" r="2"/><path d="M12 7v4"/><line x1="8" x2="8" y1="16" y2="16"/><line x1="16" x2="16" y1="16" y2="16"/></svg>`

// ────────────────────────────────────────────────────────────
// 单条消息 → HTML 片段（复刻 Thread.tsx 的 UserMessage / AssistantMessage）
// ────────────────────────────────────────────────────────────

function messageToHtml(msg: ExportMessageData): string {
  const time = formatTime(msg.createdAt)
  const roleLabel = msg.role === "user" ? "你" : "助手"

  if (msg.role === "user") {
    // user：右对齐 + accent 底气泡
    return `
<div class="msg user">
  <div class="avatar user-avatar">${USER_AVATAR_SVG}</div>
  <div class="bubble-wrap user-wrap">
    <div class="bubble user-bubble">${escapeHtml(msg.content)}</div>
    <div class="meta user-meta">${escapeHtml(roleLabel)} · ${time}</div>
  </div>
</div>`.trim()
  }

  // assistant：左对齐 + elevated 底气泡
  const reasoningHtml =
    msg.reasoning && msg.reasoning.length > 0
      ? `
<details class="reasoning">
  <summary>思考过程</summary>
  <div class="reasoning-body">${renderMarkdownToHtml(msg.reasoning)}</div>
</details>`.trim()
      : ""

  const bodyHtml = renderMarkdownToHtml(msg.content)
  const errorHtml = msg.error
    ? `<div class="error-box">⚠ ${escapeHtml(msg.error)}</div>`
    : ""

  // 元信息：模型 + token（如有）
  const metaParts: string[] = [escapeHtml(roleLabel), time]
  if (msg.model) metaParts.push(escapeHtml(msg.model))
  if (msg.totalTokens) metaParts.push(`${msg.totalTokens} tokens`)

  return `
<div class="msg assistant">
  <div class="avatar assistant-avatar">${BOT_AVATAR_SVG}</div>
  <div class="bubble-wrap assistant-wrap">
    <div class="bubble assistant-bubble">
      ${reasoningHtml}
      <div class="aui-md">${bodyHtml}</div>
      ${errorHtml}
    </div>
    <div class="meta assistant-meta">${metaParts.join(" · ")}</div>
  </div>
</div>`.trim()
}

// ────────────────────────────────────────────────────────────
// 编排：填充模板占位符
// ────────────────────────────────────────────────────────────

export interface RenderHtmlOptions {
  /** 主题：'light' | 'dark' | 'auto'。auto 跟随查看者 prefers-color-scheme。 */
  theme?: "light" | "dark" | "auto"
}

/** 主题 → html class（auto 不加 class，靠 @media prefers-color-scheme 触发）。 */
function themeClass(theme: RenderHtmlOptions["theme"]): string {
  if (theme === "light") return "theme-light"
  if (theme === "dark") return "theme-dark"
  return "" // auto
}

/**
 * 生成自包含 HTML 字符串。
 *
 * 占位符：{{THEME}} {{TITLE}} {{SUBTITLE}} {{MESSAGES}}。
 * 用 replaceAll 填充（{{TITLE}} 在 <title> 和 <h1> 出现两次）。
 *
 * 自包含：CSS 全部内联在模板里（主题 token + aui-md 排版 + 布局），
 * 不再运行时收集页面 CSS——避免带入 overflow:hidden / 流式光标 /
 * 选不中的 Tailwind 工具类，同时让深色主题真正生效。

/**
 * 生成自包含 HTML 字符串。
 *
 * 占位符：{{THEME}} {{TITLE}} {{SUBTITLE}} {{MESSAGES}} {{PAGE_CSS}}。
 * 简单 .replace 填充（对应 pi generateHtml 的占位符替换）。
 */
export function renderHtml(
  data: ConversationExportData,
  opts: RenderHtmlOptions = {},
): string {
  const { theme = "auto" } = opts
  const titleEsc = escapeHtml(data.title || "未命名会话")
  const subtitle = `共 ${data.messages.length} 条消息 · 由 onto-studio 导出 · ${formatTime(data.exportedAt)}`
  const messagesHtml = data.messages.map(messageToHtml).join("\n")
  const cls = themeClass(theme)

  return htmlTemplate
    .replaceAll("{{THEME}}", cls)
    .replaceAll("{{TITLE}}", titleEsc)
    .replaceAll("{{SUBTITLE}}", escapeHtml(subtitle))
    .replaceAll("{{MESSAGES}}", messagesHtml)
}
