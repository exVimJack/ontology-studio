// State 层：ingest-store.ts（§14.2）
// 摄取任务进度状态（IngestStatusBoard 数据源）。
//
// 一期收尾后：挂载文档持久化到 conversation_documents 表（后端），
// 前端不再用内存 ingestedByConv 存文档全文。本 store 只保留瞬态摄取任务进度。

import { create } from "zustand"
import type { IngestStage } from "@/lib/domain"

export interface IngestJob {
  jobId: string
  /** 归属会话（null = 无会话/全局，仅 LibraryView 看板可见）。 */
  conversationId: string | null
  path: string
  fileName: string
  stage: IngestStage
  charCount: number
  error?: string
  /** 当前解析阶段描述（如 "提取 PDF 文本"）。 */
  phase?: string | null
  /** 细粒度进度：已处理单元数（页/章/条目）。 */
  current?: number | null
  /** 细粒度进度：总单元数；未知时为 null。 */
  total?: number | null
  /** 文件大小（字节）。Queued 时即上报，供前端展示。 */
  fileSize?: number | null
  /** 最近一次进度更新时间戳（ms），前端心跳检测用。 */
  updatedAt?: number
}

interface IngestState {
  jobs: IngestJob[]

  upsertJob: (job: IngestJob) => void
  clearFinished: () => void
  clearResolved: () => void
}

export const useIngestStore = create<IngestState>((set) => ({
  jobs: [],

  upsertJob: (job) =>
    set((s) => {
      const idx = s.jobs.findIndex((j) => j.jobId === job.jobId)
      const next = [...s.jobs]
      if (idx >= 0) next[idx] = job
      else next.push(job)
      return { jobs: next }
    }),

  clearFinished: () =>
    // 只清 Done（纯瞬态完成态，看板本就不显示）。Error/Cancelled 保留供回看，
    // 由看板"清除"按钮（clearResolved）主动清理。
    set((s) => ({
      jobs: s.jobs.filter((j) => j.stage !== "Done"),
    })),

  /** 清理已结束的失败/取消任务（Error/Cancelled/Done）。看板"清除"按钮用。 */
  clearResolved: () =>
    set((s) => ({
      jobs: s.jobs.filter((j) => j.stage === "Queued" || j.stage === "Parsing"),
    })),
}))
