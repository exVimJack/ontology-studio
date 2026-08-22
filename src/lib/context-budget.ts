// 工具层：context-budget.ts
// 发送前的上下文体积预算与截断。
//
// 背景：选中的已摄入文档全文 + base64 图片会被拼进单次发送消息的 payload，
// 再由 Rust 侧拼进单个 HTTP 请求发给 LLM provider。大批量文件时 body 极易
// 超过 provider 网关（openresty 等）的限制（常见 1–10MB），触发 413 Payload Too Large。
//
// 一期不引入 tokenizer（二期 RAG 范畴），用**字节预算**做粗粒度截断即可覆盖绝大多数场景：
//   - 文档文本：单篇硬上限 + 总量上限，超限尾部截断并标注，模型仍可见前文
//   - 图片：单张字节硬上限，超限前端直接拒绝加入（base64 后约为原文件 1.37 倍）
//
// 预算取值偏保守：留出用户问题 + 历史消息 + 模型回复的空间。
// 1 token ≈ 3–4 字节（英文）/ ≈ 1.5–2 字节（中文），按字节估算偏松但安全。

/** 单篇文档文本的硬上限（字节）。超过则尾部截断。
 *  512 KiB 文本约 13–17 万 token，远超多数单文档所需，足够覆盖大书单章。 */
export const MAX_DOC_TEXT_BYTES = 512 * 1024

/** 全部文档文本的总量上限（字节）。
 *  2 MiB 文本约 50–70 万 token，是多数模型上下文窗口的量级上限，
 *  超过此量级应走二期 RAG 检索而非全文塞入。 */
export const MAX_TOTAL_DOC_BYTES = 2 * 1024 * 1024

/** 单张图片的硬上限（原始字节，非 base64）。
 *  base64 编码后体积约为原文件 4/3 倍，2 MiB 原图 ≈ 2.7 MiB base64。
 *  多数 VLM 对单图有 5–20MB 限制，这里取保守的 2 MiB，兼顾网关 body 限制。 */
export const MAX_IMAGE_BYTES = 2 * 1024 * 1024

/** 截断标注模板（中文，附原始字符数让模型知道被截断的规模）。 */
function truncationNote(originalChars: number): string {
  return `\n\n…[已截断，原文共 ${originalChars} 字符，仅发送前半部分]`
}

/** UTF-8 字节长度（JS string 是 UTF-16，需按 UTF-8 估算传输体积）。 */
export function utf8ByteLength(s: string): number {
  // TextEncoder 输出 UTF-8 字节，长度即字节数；避免逐字符 codePoint 算
  return new TextEncoder().encode(s).length
}

/** 截取字符串使其 UTF-8 字节长度 ≤ maxBytes，从尾部丢弃（保留前文）。 */
function sliceByUtf8Bytes(s: string, maxBytes: number): string {
  if (utf8ByteLength(s) <= maxBytes) return s
  // 二分截到字符边界：先按比例估算再逐字符回退
  const enc = new TextEncoder()
  const bytes = enc.encode(s)
  // 找到 ≤ maxBytes 的最后一个 UTF-8 字符边界（首字节 < 0x80 或延续字节 0x80–0xBF 之后的起点）
  let cut = maxBytes
  while (cut > 0 && (bytes[cut] & 0xc0) === 0x80) cut--
  const dec = new TextDecoder("utf-8", { fatal: false })
  return dec.decode(bytes.subarray(0, cut))
}

export interface BudgetedContextText {
  file_name: string
  text: string
  /** 是否被截断（用于日志/调试，当前不展示给用户）。 */
  truncated: boolean
}

/**
 * 对文档文本上下文做总量预算截断。
 *
 * 策略：
 *   1. 每篇先按 MAX_DOC_TEXT_BYTES 截断（单篇硬上限）
 *   2. 累计超过 MAX_TOTAL_DOC_BYTES 时，对后续篇目按剩余预算截断；
 *      若剩余预算 ≤ 0，该篇只保留文件名 + “[因总量超限未发送正文]”占位
 *
 * 保留文件名让模型知道存在哪些文件（哪怕正文被截），符合“参考材料”语义。
 * 截断是静默的——标注写进文本里，模型可见，无需打断用户。
 */
export function budgetContextTexts(
  texts: ReadonlyArray<{ file_name: string; text: string }>,
): BudgetedContextText[] {
  let remaining = MAX_TOTAL_DOC_BYTES
  return texts.map((t) => {
    const originalChars = t.text.length
    // 单篇硬上限
    let budgetForDoc = Math.min(MAX_DOC_TEXT_BYTES, remaining)
    if (budgetForDoc <= 0) {
      // 总量已耗尽：只留文件名 + 占位，不再发送正文
      return {
        file_name: t.file_name,
        text: `[因上下文总体积超限，本文档正文未发送]`,
        truncated: true,
      }
    }
    if (utf8ByteLength(t.text) <= budgetForDoc) {
      remaining -= utf8ByteLength(t.text)
      return { file_name: t.file_name, text: t.text, truncated: false }
    }
    // 需截断：留出标注的空间
    const note = truncationNote(originalChars)
    const noteBytes = utf8ByteLength(note)
    const body = sliceByUtf8Bytes(t.text, Math.max(0, budgetForDoc - noteBytes))
    remaining -= utf8ByteLength(body) + noteBytes
    return {
      file_name: t.file_name,
      text: body + note,
      truncated: true,
    }
  })
}

/**
 * 校验单张图片原始字节是否在硬上限内。
 * @returns null 表示通过；否则返回错误文案。
 */
export function validateImageSize(byteLength: number, fileName: string): string | null {
  if (byteLength > MAX_IMAGE_BYTES) {
    const mb = (MAX_IMAGE_BYTES / 1024 / 1024).toFixed(0)
    return `图片「${fileName}」过大（${(byteLength / 1024 / 1024).toFixed(1)} MiB），超过单图上限 ${mb} MiB，已跳过`
  }
  return null
}
