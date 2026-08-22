// UI 层：IngestStatusBoard.tsx（§20.8）
// 摄取进度看板：只展示进行中 / 异常任务（Queued/Parsing/Error/Cancelled）。
// Done 任务完成后即从看板消失，文件挂载后由 ScopeChip popover 的挂载文件区展示，
// 避免列表与卡片重复。渲染在 Composer 上方（有任务时显示）。

import { useIngestStore, type IngestJob } from "@/stores/ingest-store"
import { useCancelIngest } from "@/hooks/useIngest"
import { formatBytes, formatCharCount, progressPercent, compareIngestJobs } from "@/lib/format"
import { getFileIcon } from "@/lib/file-icons"
import { Loader2, CheckCircle2, AlertCircle, X, Ban, ChevronRight, ChevronDown } from "lucide-react"
import { useMemo, useState } from "react"

export function IngestStatusBoard({ conversationId }: { conversationId?: string | null } = {}) {
  const jobs = useIngestStore((s) => s.jobs)
  const clearResolved = useIngestStore((s) => s.clearResolved)

  // 看板只展示“还需关注”的任务：进行中（Queued/Parsing）与异常（Error/Cancelled）。
  // Done 任务一旦完成即从看板消失——文件已挂载，ScopeChip popover 的挂载文件区展示，
  // 看板里再留一行 Done 会与右侧面板信息重复。
  // 失败/取消的任务不进挂载列表，故保留在看板供用户查看错误。
  //
  // conversationId 非空时只显示本会话任务（ChatArea）；为空时显示全部（LibraryView）。
  const visibleJobs = useMemo(
    () =>
      jobs
        .filter((j) => j.stage !== "Done")
        .filter((j) => (conversationId == null ? true : j.conversationId === conversationId))
        .sort(compareIngestJobs),
    [jobs, conversationId],
  )
  const active = visibleJobs.filter((j) => j.stage === "Queued" || j.stage === "Parsing")
  const showBoard = visibleJobs.length > 0

  if (!showBoard) {
    return null
  }

  return (
    <div className="border-b border-border bg-bg-elevated px-3 py-2">
      <div className="mb-1.5 flex items-center justify-between">
        <span className="text-xs font-medium text-fg-muted">摄入进度</span>
        {active.length === 0 && (
          <button
            onClick={clearResolved}
            className="flex items-center gap-1 text-[11px] text-fg-subtle hover:text-fg"
          >
            <X size={11} /> 清除
          </button>
        )}
      </div>
      <div className="space-y-1">
        {visibleJobs.map((j) => (
          <JobRow key={j.jobId} job={j} />
        ))}
      </div>
    </div>
  )
}

/** 单个任务行：状态图标 + 文件名（附大小）+ 进度 + 取消按钮。错误详情折叠。 */
function JobRow({ job }: { job: IngestJob }) {
  const [cancelling, setCancelling] = useState(false)
  const [errorExpanded, setErrorExpanded] = useState(false)
  const cancelIngest = useCancelIngest()

  const onCancel = async () => {
    setCancelling(true)
    try {
      await cancelIngest.mutateAsync(job.jobId)
    } catch (e) {
      console.error("cancel ingest failed", e)
      setCancelling(false)
    }
  }

  // 解析阶段右侧状态文案：有 total 显示百分比，否则展示阶段名 + 已产出字符数
  const parsingStatus = (() => {
    if (job.stage !== "Parsing") return null
    const pct = progressPercent(job.current, job.total)
    if (pct != null) return `${pct}%${job.phase ? ` · ${job.phase}` : ""}`
    // 无确切 total：不伪造百分比（假进度破坏信任），展示已产出字符数 + 阶段
    const chars = job.charCount > 0 ? ` · ${formatCharCount(job.charCount)}` : ""
    return `${job.phase ?? "准备中…"}${chars}`
  })()

  return (
    <div>
      <div className="flex items-center gap-2 text-xs">
        {job.stage === "Queued" && (() => {
          const { Icon, className } = getFileIcon("", job.fileName)
          return <Icon size={12} className={`shrink-0 ${className}`} />
        })()}
        {job.stage === "Parsing" && <Loader2 size={12} className="shrink-0 animate-spin text-accent" />}
        {job.stage === "Done" && <CheckCircle2 size={12} className="shrink-0 text-accent" />}
        {job.stage === "Error" && <AlertCircle size={12} className="shrink-0 text-danger" />}
        {job.stage === "Cancelled" && <Ban size={12} className="shrink-0 text-fg-subtle" />}
        <span className="min-w-0 flex-1 truncate">{job.fileName}</span>
        {/* 文件大小：固定值，紧贴文件名作为副信息，弱色小字与进度区分 */}
        {job.fileSize != null && (
          <span className="shrink-0 text-[10px] text-fg-subtle/70">
            {formatBytes(job.fileSize)}
          </span>
        )}
        {parsingStatus && (
          <span className="shrink-0 truncate text-fg-subtle">{parsingStatus}</span>
        )}
        {(job.stage === "Queued" || job.stage === "Parsing") && (
          <button
            onClick={onCancel}
            disabled={cancelling}
            title="取消"
            className="shrink-0 text-fg-subtle hover:text-danger disabled:opacity-40"
          >
            {cancelling ? <Loader2 size={12} className="animate-spin" /> : <X size={12} />}
          </button>
        )}
        {job.stage === "Done" && (
          <span className="shrink-0 text-fg-subtle">{formatCharCount(job.charCount)}</span>
        )}
        {job.stage === "Cancelled" && <span className="shrink-0 text-fg-subtle">已取消</span>}
        {job.stage === "Error" && (
          <button
            onClick={() => setErrorExpanded((v) => !v)}
            title={errorExpanded ? "收起错误详情" : "展开错误详情"}
            className="flex shrink-0 items-center gap-1 text-danger hover:text-danger/80"
          >
            <span>解析失败</span>
            {errorExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
        )}
      </div>
      {/* 错误详情：默认收起，点击展开独立成行，不挤压文件名 */}
      {job.stage === "Error" && errorExpanded && job.error && (
        <div className="mt-1 ml-5 rounded bg-danger/10 px-2 py-1 text-[11px] leading-relaxed text-danger/90">
          {job.error}
        </div>
      )}
    </div>
  )
}
