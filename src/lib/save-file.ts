// 工具层：save-file.ts
// 原生保存文本文件：plugin-dialog save() 弹系统保存框 + plugin-fs writeTextFile()。
//
// 背景（§13）：Tauri WebView 下浏览器原生 `<a download>`+blob 下载常被吞
// （WebView2/WKWebView 对 programmatic download 支持不一致、不弹系统保存框），
// 改走 Tauri 原生插件。
//
// 抽出为共享工具：单条消息导出（Thread.ExportButton）与会话整体导出
// （ExportConversationButton）复用同一实现，避免逻辑漂移。

/** 保存对话框的文件类型过滤项。 */
export interface SaveFileFilter {
  name: string
  extensions: string[]
}

/**
 * 弹出原生保存对话框，用户确认后写入文本文件。
 *
 * @param content    文件内容
 * @param defaultPath 默认文件名（可含目录）
 * @param filters    文件类型过滤
 * @returns 实际保存路径；用户取消返回 null
 */
export async function saveTextFile(
  content: string,
  defaultPath: string,
  filters: SaveFileFilter[],
): Promise<string | null> {
  const { save } = await import("@tauri-apps/plugin-dialog")
  const filePath = await save({ defaultPath, filters })
  if (!filePath) return null // 用户取消

  const { writeTextFile } = await import("@tauri-apps/plugin-fs")
  await writeTextFile(filePath, content)
  return filePath
}
