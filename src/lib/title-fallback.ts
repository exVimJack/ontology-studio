// Domain 层：title-fallback.ts
// 标题降级兜底：LLM 生成标题失败时，用首条用户消息截断生成标题。
// 与 LLM 概括（agent-core::generate_title）相对，仅作 fallback，非主路径。

/** 首条用户消息为空时的默认标题。 */
export const DEFAULT_CONVERSATION_TITLE = "新会话"

/**
 * 降级标题生成：从首条用户消息截断出一个简洁标题。
 *
 * 仅在 LLM 概括失败（provider 未配置 / 网络错误 / 返回空）时使用。
 * 主路径是后端 `generate_conversation_title` 命令（LLM 概括）。
 *
 * 处理：取首行 → 去 markdown 标记 → 去多余空白 → 截断到 30 字。
 */
export function deriveFallbackTitle(content: string): string {
  const firstLine = content.split("\n")[0] ?? content
  const cleaned = firstLine
    .replace(/^#{1,6}\s*/, "") // 标题标记
    .replace(/^[\s>*\-+]+/, "") // 引用/列表/引用标记
    .replace(/[`*_~]/g, "") // 行内格式标记
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1") // 链接只留文字
    .trim()
    .replace(/\s+/g, " ")
  if (!cleaned) return DEFAULT_CONVERSATION_TITLE
  const MAX = 30
  return cleaned.length > MAX ? cleaned.slice(0, MAX) + "…" : cleaned
}
