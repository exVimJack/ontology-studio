// 工具层：file-icons.tsx
// 文件类型 → lucide 图标 + 语义色。
//
// 解析前只有文件名（按扩展名判断），解析后有 ingest 返回的 `format` 字段。
// 两条路径统一收口到这里，避免 IngestStatusBoard / Composer / ScopeChip 各写一遍。
//
// 图标选择遵循“一眼可辨”原则；颜色对齐主流文件管理器直觉（VS Code / Finder）：
//   pdf      → FileText  · rose（红，PDF 品牌色）
//   doc/docx → FileText  · blue（Office Word 蓝）
//   ppt      → Presentation · amber（Office PPT 橙）
//   xls/xlsx/csv/tsv → FileSpreadsheet · green（表格绿）
//   epub     → BookOpen · violet（电子书紫）
//   json     → FileJson · yellow（代码黄）
//   md/txt   → FileText · slate（纯文本中性）
//   zip/tar  → FileArchive · orange（压缩包橙）
//   image    → FileImage · pink（图片粉）
//   其他     → File · fg-subtle（中性兜底）
//
// 颜色用 Tailwind 固定色阶（-500），浅深主题下均可读，不污染主题 token。
// 注意：颜色类必须静态出现在源码里（Tailwind v4 JIT 按字面量扫描），
// 故此处显式写出每个 className 字符串，不做字符串拼接。

import {
  FileText,
  File,
  FileSpreadsheet,
  FileArchive,
  FileImage,
  FileJson,
  BookOpen,
  Presentation,
} from "lucide-react"
import type { LucideIcon } from "lucide-react"

export interface FileIconInfo {
  Icon: LucideIcon
  /** Tailwind 文字色类，如 "text-rose-500"。 */
  className: string
}

/** 图片类扩展名（与 Composer 中 IMAGE_EXTENSIONS 对齐）。 */
const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "webp", "bmp", "gif"])

/** 从文件名提取小写扩展名（无点返回 ""）。 */
function extOf(fileName: string): string {
  const dot = fileName.lastIndexOf(".")
  return dot < 0 ? "" : fileName.slice(dot + 1).toLowerCase()
}

/**
 * 取文件对应的图标 + 颜色。
 *
 * 优先用 ingest 解析返回的 `format`（更可靠，已归一化）；
 * 解析前（Queued/上传中）只有文件名，则按扩展名兜底判断。
 *
 * @param format  ingest 返回的 format 字段（如 "pdf"/"docx"/"image"）；未知传 ""
 * @param fileName 文件名（用于扩展名兜底）
 */
export function getFileIcon(format: string, fileName: string): FileIconInfo {
  const f = format.toLowerCase()
  const ext = extOf(fileName)

  // format 优先
  switch (f) {
    case "pdf":
      return { Icon: FileText, className: "text-rose-500" }
    case "docx":
    case "doc":
      return { Icon: FileText, className: "text-blue-500" }
    case "pptx":
    case "ppt":
      return { Icon: Presentation, className: "text-amber-500" }
    case "xlsx":
    case "xls":
    case "csv":
    case "tsv":
      return { Icon: FileSpreadsheet, className: "text-green-600" }
    case "epub":
      return { Icon: BookOpen, className: "text-violet-500" }
    case "json":
      return { Icon: FileJson, className: "text-yellow-600" }
    case "text":
    case "markdown":
      return { Icon: FileText, className: "text-slate-500" }
    case "zip":
    case "tar":
    case "tgz":
      return { Icon: FileArchive, className: "text-orange-500" }
    case "image":
      return { Icon: FileImage, className: "text-pink-500" }
    case "":
      break // 落到扩展名兜底
    default:
      return { Icon: File, className: "text-fg-subtle" }
  }

  // 扩展名兜底（解析前）
  if (IMAGE_EXTS.has(ext)) return { Icon: FileImage, className: "text-pink-500" }
  switch (ext) {
    case "pdf":
      return { Icon: FileText, className: "text-rose-500" }
    case "docx":
    case "doc":
      return { Icon: FileText, className: "text-blue-500" }
    case "pptx":
    case "ppt":
      return { Icon: Presentation, className: "text-amber-500" }
    case "xlsx":
    case "xls":
    case "csv":
    case "tsv":
      return { Icon: FileSpreadsheet, className: "text-green-600" }
    case "epub":
      return { Icon: BookOpen, className: "text-violet-500" }
    case "json":
      return { Icon: FileJson, className: "text-yellow-600" }
    case "txt":
    case "md":
    case "markdown":
      return { Icon: FileText, className: "text-slate-500" }
    case "zip":
    case "tar":
    case "tgz":
      return { Icon: FileArchive, className: "text-orange-500" }
    default:
      return { Icon: File, className: "text-fg-subtle" }
  }
}
