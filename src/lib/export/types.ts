// 数据层：export/types.ts
// 导出中间表示（IR）——介于 MessageRow（DB 行）与最终格式（HTML/Markdown）之间。
//
// 设计参照 pi 的 export-html：数据（SessionManager entries）、模板（template.html/css）、
// 渲染（generateHtml 占位填充）三层分离。onto-studio 适配为：
//   extract.ts 唯一知道 MessageRow → ConversationExportData
//   html-template.ts / markdown-template.ts 只消费 IR，不碰 MessageRow
//   render-markdown.ts 通用 markdown→HTML 工具，不含会话语义
//
// 这样改 DB schema 不动模板，改展示皮肤不动数据提取；HTML/MD 两条路径共享同一份数据取舍。

/** 单条消息的导出 IR（从 MessageRow 提取，去掉 DB 杂项字段）。 */
export interface ExportMessageData {
  role: "user" | "assistant"
  /** 正文（markdown 文本）。 */
  content: string
  /** reasoning 思考链（仅 assistant，可空）。 */
  reasoning: string | null
  /** assistant 所用模型，可空。 */
  model: string | null
  /** provider 报告的总 token，可空（旧消息/未报告）。 */
  totalTokens: number | null
  /** status=error 时的错误信息，可空。 */
  error: string | null
  /** 创建时间（Unix ms）。 */
  createdAt: number
}

/** 整段会话的导出 IR。 */
export interface ConversationExportData {
  title: string
  /** 导出动作发生的时间（Unix ms），用于页脚"导出于 …"。 */
  exportedAt: number
  messages: ExportMessageData[]
}
