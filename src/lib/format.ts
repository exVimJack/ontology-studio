// 工具层：format.ts
// 纯函数格式化器（字节大小、字符数、进度百分比）。无依赖，可在任何层使用。

/**
 * 把字节数格式化为人类可读字符串（二进制 1024 进制，IEC 单位）。
 *
 * 用 1024 进制而非 1000：文件管理器（资源管理器/Finder）惯例，
 * 用户对 "15.3 MB" 的预期来自系统显示，与磁盘字节一致。
 *
 * @example formatBytes(0)           → "0 B"
 * @example formatBytes(1536)        → "1.5 KB"
 * @example formatBytes(16_000_000)  → "15.3 MB"
 * @example formatBytes(null)        → ""
 */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || bytes < 0 || !Number.isFinite(bytes)) return ""
  if (bytes === 0) return "0 B"
  const units = ["B", "KB", "MB", "GB", "TB", "PB"]
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1,
  )
  const value = bytes / Math.pow(1024, i)
  // B 不保留小数；KB 起 1 位小数；≥100 时降为 0 位避免过长
  const digits = i === 0 ? 0 : value >= 100 ? 0 : 1
  return `${value.toFixed(digits)} ${units[i]}`
}

/**
 * 把字符数格式化为紧凑可读字符串。
 *
 * 用 `Intl.NumberFormat` 的 compact 记法：中文环境下 106640 → "10.7万"，
 * 比 "106,640" 更短、更易扫读。小数字（<1万）直接显示整数，避免 "9.9K" 这类
 * 英文紧凑符号在中文界面里突兀。
 *
 * @example formatCharCount(0)        → "0 字符"
 * @example formatCharCount(950)      → "950 字符"
 * @example formatCharCount(106640)   → "10.7万 字符"
 * @example formatCharCount(2354035)  → "235.4万 字符"
 */
export function formatCharCount(count: number | null | undefined): string {
  if (count == null || count < 0 || !Number.isFinite(count)) return ""
  if (count < 10000) return `${count} 字符`
  const compact = new Intl.NumberFormat("zh-CN", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(count)
  return `${compact} 字符`
}

/**
 * 计算解析进度百分比（0–100，整数）。
 *
 * 仅当 `current` 与 `total` 均已知且 total>0 时返回百分比；
 * 否则返回 null——前端应回退到非确定态（spinner + 已产出字符数），
 * 而非伪造百分比（假进度条会破坏信任，见 NN/g 进度指示器研究）。
 *
 * @example progressPercent(50, 134)   → 37
 * @example progressPercent(50, null)  → null
 * @example progressPercent(null, 134) → null
 */
export function progressPercent(
  current: number | null | undefined,
  total: number | null | undefined,
): number | null {
  if (current == null || total == null || total <= 0) return null
  const pct = Math.round((current / total) * 100)
  // 钳制到 [0, 100]，防御 total 估算偏差
  return Math.max(0, Math.min(100, pct))
}

/**
 * 摄入任务的状态优先级（值越小越靠前展示）。
 *
 * 排序策略（活动置顶，业界最佳实践，参考 Nexus Mods 下载器、UploadKit）：
 *   Parsing → Queued → Error → Cancelled → Done
 *
 * - 活动任务（解析中/排队）置顶：用户最关心“还在跑的、卡没卡”
 * - Error 紧跟活动项：需关注的次高优先级，但不能压过进行中的
 * - Done 沉底：已完成的挂载后由 ScopeChip popover 展示，看板里不抢注意力；
 *   避免大量 Done 项淹没少量活动项（如 50 个文件 49 完成时）
 *
 * 未知的 stage 兜底为最低优先级（沉底），防御未来新增状态。
 */
const STAGE_RANK: Record<string, number> = {
  Parsing: 0,
  Queued: 1,
  Error: 2,
  Cancelled: 3,
  Done: 4,
}

/**
 * 摄入任务列表的稳定排序比较器：先按状态优先级，同组内按文件名 locale 自然序。
 *
 * “稳定”指同组同名的任务保持原相对顺序（Array.prototype.sort 在 V8 是稳定排序）。
 * 不依赖时间戳，避免 progress 高频更新时引起同组内重排跳动。
 *
 * @example compareIngestJobs({stage:"Parsing",fileName:"b"}, {stage:"Done",fileName:"a"}) → -1（Parsing 优先）
 * @example compareIngestJobs({stage:"Done",fileName:"a"}, {stage:"Done",fileName:"b"}) → -1（同组按名）
 */
export function compareIngestJobs<
  T extends { stage: string; fileName: string },
>(a: T, b: T): number {
  const ra = STAGE_RANK[a.stage] ?? 99
  const rb = STAGE_RANK[b.stage] ?? 99
  if (ra !== rb) return ra - rb
  return a.fileName.localeCompare(b.fileName, undefined, {
    numeric: true,
    sensitivity: "base",
  })
}
