// 提取层：export/extract.ts
// MessageRow[] → ConversationExportData（导出 IR）。
//
// 这是唯一知道 MessageRow 结构的地方——HTML/Markdown 模板都只消费 IR，
// 改 DB schema 不波及模板，改展示皮肤不动数据提取。
//
// 对应 pi export-html 里 `exportSessionToHtml` 取 `sm.getEntries()` 的角色：
// pi 的数据源是 SessionManager（entries 树），onto-studio 的数据源是扁平 MessageRow。

import type { MessageRow } from "@/lib/domain"
import type { ConversationExportData, ExportMessageData } from "./types"

/** 从 MessageRow[] 提取导出 IR。title 为会话标题（可空，由调用方决定兜底）。 */
export function extractConversation(
  title: string,
  messages: MessageRow[],
): ConversationExportData {
  return {
    title,
    exportedAt: Date.now(),
    messages: messages.map(toExportMessage),
  }
}

function toExportMessage(msg: MessageRow): ExportMessageData {
  // role 收敛为 user/assistant（system 消息不入导出，后端不产生可见 system 行）
  const role: ExportMessageData["role"] =
    msg.role === "assistant" ? "assistant" : "user"

  return {
    role,
    content: msg.content,
    reasoning: msg.reasoning ?? null,
    model: msg.model ?? null,
    totalTokens: msg.total_tokens && msg.total_tokens > 0 ? msg.total_tokens : null,
    error: msg.status === "error" ? (msg.error ?? null) : null,
    createdAt: msg.created_at,
  }
}
