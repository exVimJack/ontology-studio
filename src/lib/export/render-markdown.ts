// 渲染工具层：export/render-markdown.ts
// 通用 markdown→HTML 字符串转换 + 页面 CSS 收集。
//
// 不含任何会话语义（不知道 MessageRow/ConversationExportData），可被任意
// 需要字符串化 markdown 的功能复用。对应 pi export-html 里 vendor/marked + template.js
// 中的 markdown 渲染部分。
//
// 引擎选择：micromark（react-markdown 的底层，项目已有传递依赖）+ gfm 扩展
// （对齐应用内 remark-gfm）。citation [n] 用正则复刻 remarkCitations 语义：
// 应用内由 remark 插件转 #cite-n 链接再由 React 组件渲染为上标；导出 HTML
// 无交互来源面板，统一渲染为静态上标。

import { micromark } from "micromark"
import { gfm, gfmHtml } from "micromark-extension-gfm"

/**
 * 把正文 markdown 渲染成 HTML 片段。
 *
 * - micromark + gfm：表格/任务列表/删除线/autolink，对齐应用内 remark-gfm。
 * - citation `[n]`（1-3 位数字）→ `<sup class="cite">n</sup>` 静态上标。
 *   allowDangerousHtml 让该原始 HTML 通过（内容受控：仅数字）。
 */
export function renderMarkdownToHtml(md: string): string {
  if (!md) return ""
  const withCites = md.replace(/\[(\d{1,3})\]/g, (_m, n: string) => {
    return `<sup class="cite">${n}</sup>`
  })
  return micromark(withCites, {
    allowDangerousHtml: true,
    extensions: [gfm()],
    htmlExtensions: [gfmHtml()],
  })
}
