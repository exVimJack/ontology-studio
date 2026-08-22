// 工具层：image-resize.ts
// 图片发送前降采样 + 重编码，控制 base64 体积。
//
// 背景：多模态请求把图片以 base64 内联进 HTTP body，base64 膨胀约 33%。
// 多图或大图极易触发 provider 网关（openresty 等）的 body 限制（常见 1–32MB），
// 返回 413 Payload Too Large。
//
// 业界共识（OpenAI vision 官方建议 + Claude Code preflight 实践）：
//   - 图片长边 ≤ 2048px、短边 ≤ 768px，模型内部会降采样到此尺寸
//   - 发更大的纯属浪费 body，且是 413 主因
//
// 本模块在前端读取图片后用 Canvas 降采样到目标尺寸并重编码为 JPEG（质量 0.85），
// 显著缩小 base64 体积。PNG 透明通道的图会因转 JPEG 丢失透明度——一期可接受
// （VLM 场景透明度信息不重要）；如需保留可按需扩展为按格式输出。

/** 降采样目标：长边上限。OpenAI 官方建议值。 */
export const MAX_IMAGE_LONG_EDGE = 2048
/** 降采样目标：短边上限。OpenAI 官方建议值。 */
export const MAX_IMAGE_SHORT_EDGE = 768
/** JPEG 重编码质量。0.85 在体积与清晰度间较好平衡。 */
const JPEG_QUALITY = 0.85

export interface ResizedImage {
  /** 重编码后的 base64（不含 data: 前缀） */
  dataB64: string
  /** 统一为 image/jpeg（降采样后格式固定） */
  mime: string
  /** 是否发生了降采样/重编码 */
  resized: boolean
  /** 原始字节大小（降采样前） */
  originalBytes: number
  /** 重编码后字节大小（base64 解码后） */
  resizedBytes: number
}

/**
 * 把原始图片字节降采样并重编码为 JPEG base64。
 *
 * 流程：Uint8Array → Blob → createImageBitmap → Canvas drawImage（缩放）→ toDataURL JPEG。
 * 失败时回退返回原图 base64（不阻断发送，交由后续体积校验兜底）。
 *
 * @param bytes 原始图片字节
 * @returns 降采样后的 base64 与元信息
 */
export async function resizeImageToBase64(bytes: Uint8Array): Promise<ResizedImage> {
  const originalBytes = bytes.length
  // 原图 base64 作为失败兜底
  const fallbackB64 = bytesToBase64(bytes)
  const fallback: ResizedImage = {
    dataB64: fallbackB64,
    mime: "image/jpeg",
    resized: false,
    originalBytes,
    resizedBytes: bytes.length,
  }

  // createImageBitmap 在 Tauri WebView 可用（Chromium/WebKit 均支持）
  let bitmap: ImageBitmap
  try {
    const blob = new Blob([bytes.slice().buffer], { type: "image/*" })
    bitmap = await createImageBitmap(blob)
  } catch {
    return fallback
  }

  const { width: ow, height: oh } = bitmap
  const { targetW, targetH, needResize } = computeTargetSize(ow, oh)
  if (!needResize) {
    // 原图尺寸已在限制内，但仍重编码为 JPEG 以统一格式、压缩体积
    return encodeCanvas(bitmap, ow, oh).then(
      (b64) => ({
        dataB64: b64,
        mime: "image/jpeg",
        resized: false,
        originalBytes,
        resizedBytes: base64ByteLength(b64),
      }),
      () => fallback,
    )
  }
  return encodeCanvas(bitmap, targetW, targetH).then(
    (b64) => ({
      dataB64: b64,
      mime: "image/jpeg",
      resized: true,
      originalBytes,
      resizedBytes: base64ByteLength(b64),
    }),
    () => fallback,
  )
}

/** 计算降采样目标尺寸。长边 ≤2048、短边 ≤768，等比缩放。 */
function computeTargetSize(ow: number, oh: number): {
  targetW: number
  targetH: number
  needResize: boolean
} {
  const longEdge = Math.max(ow, oh)
  const shortEdge = Math.min(ow, oh)
  // 已在限制内无需缩放（但仍可能需要重编码）
  if (longEdge <= MAX_IMAGE_LONG_EDGE && shortEdge <= MAX_IMAGE_SHORT_EDGE) {
    return { targetW: ow, targetH: oh, needResize: false }
  }
  // 按长边优先计算缩放比
  let scale = MAX_IMAGE_LONG_EDGE / longEdge
  // 缩放后短边仍超限，再按短边收紧
  if (shortEdge * scale > MAX_IMAGE_SHORT_EDGE) {
    scale = MAX_IMAGE_SHORT_EDGE / shortEdge
  }
  const targetW = Math.max(1, Math.round(ow * scale))
  const targetH = Math.max(1, Math.round(oh * scale))
  return { targetW, targetH, needResize: true }
}

/** Canvas 绘制并导出 JPEG dataURL，返回不含前缀的 base64。 */
function encodeCanvas(
  bitmap: ImageBitmap,
  w: number,
  h: number,
): Promise<string> {
  return new Promise((resolve, reject) => {
    try {
      const canvas = document.createElement("canvas")
      canvas.width = w
      canvas.height = h
      const ctx = canvas.getContext("2d")
      if (!ctx) {
        reject(new Error("canvas 2d context 不可用"))
        return
      }
      ctx.drawImage(bitmap, 0, 0, w, h)
      const dataUrl = canvas.toDataURL("image/jpeg", JPEG_QUALITY)
      // 去掉 "data:image/jpeg;base64," 前缀
      const comma = dataUrl.indexOf(",")
      if (comma < 0) {
        reject(new Error("toDataURL 返回格式异常"))
        return
      }
      resolve(dataUrl.slice(comma + 1))
    } catch (e) {
      reject(e)
    } finally {
      bitmap.close?.()
    }
  })
}

/** Uint8Array → base64（分块避免 call stack 溢出）。 */
function bytesToBase64(bytes: Uint8Array): string {
  let binary = ""
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk))
  }
  return btoa(binary)
}

/** base64 字符串解码后的字节长度。base64 每 4 字符 → 3 字节，考虑 padding。 */
function base64ByteLength(b64: string): number {
  const len = b64.length
  const padding = b64.endsWith("==") ? 2 : b64.endsWith("=") ? 1 : 0
  return Math.floor(len * 3 / 4) - padding
}
