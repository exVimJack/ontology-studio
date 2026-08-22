// export/ barrel：会话导出三层（数据 IR / 提取 / 模板）统一出口。
//
// 分层（对齐 pi export-html 的数据/模板/渲染分离）：
//   types.ts            数据 IR（ConversationExportData），不依赖 MessageRow 也不依赖 HTML
//   extract.ts          MessageRow[] → IR（唯一知道 MessageRow 的地方）
//   render-markdown.ts  通用 markdown→HTML 工具（不含会话语义）
//   html-template.ts    IR → 自包含 HTML（消费 render-markdown）
//   markdown-template.ts IR → Markdown

export type { ConversationExportData, ExportMessageData } from "./types"
export { extractConversation } from "./extract"
export { renderMarkdownToHtml } from "./render-markdown"
export { renderHtml } from "./html-template"
export type { RenderHtmlOptions } from "./html-template"
export { renderMarkdown } from "./markdown-template"
