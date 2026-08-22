// Hooks 层：useIngest.ts（§14.2）
// 摄取入口：ingest(paths) → Channel 进度回调 → 完成后 mountDocument 挂载到会话。
// 拖拽落点 / 文件选择器 / ⌘O 都调此 hook。
//
// 一期收尾后：全文由后端 ingest 时 upsert 到 documents 表，前端只需 mountDocument
// 记录会话↔文档关联（持久化）。不再用内存 store 存全文。
//
// 注：图片输入一期不走 ingest 管道（图片无需"解析"，只需 base64 编码传给 VLM）。
// 图片经 Composer 直接读文件转 base64 → context_images，见 useChat.send。

import { useMutation, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type { IngestProgress, IngestResultItem } from "@/lib/domain"
import { useIngestStore } from "@/stores/ingest-store"

/**
 * 摄取文件并挂载到指定会话。
 *
 * @param conversationId 归属会话；未选会话时传 null（仅入库不挂载）
 */
export function useIngest(conversationId: string | null) {
  const upsertJob = useIngestStore((s) => s.upsertJob)
  const qc = useQueryClient()

  const onProgress = (p: IngestProgress) => {
    upsertJob({
      jobId: p.job_id,
      conversationId,
      path: p.path,
      fileName: p.file_name,
      stage: p.stage,
      charCount: p.char_count,
      error: p.error ?? undefined,
      phase: p.phase ?? undefined,
      current: p.current ?? undefined,
      total: p.total ?? undefined,
      fileSize: p.file_size ?? undefined,
      updatedAt: Date.now(),
    })
  }

  return useMutation({
    mutationFn: async ({ paths: rawPaths, folderPath }: { paths: string[]; folderPath: string | null }) => {
      // OS 拖拽事件的 paths 顺序不保证（Windows 按文件系统枚举，常乱序），
      // 按文件名 locale 排序后传入 Rust，使 Queued 事件顺序稳定、UI 可预期。
      const paths = [...rawPaths].sort((a, b) =>
        a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" }),
      )
      const results: IngestResultItem[] = await ipc.ingestFiles(paths, onProgress, conversationId, folderPath)
      // batch 部分失败不抛错（成功的已挂载，抛错会让成功项的 chip 闪烁消失），
      // 但必须留痕：控制台汇总失败文件，便于排查。
      // 失败判据只看 format 标记（后端 ingest_files 对 error/cancelled 返回对应 format），
      // 不再用 r.text.length 判断——全文已由后端 upsert，前端无需也不应依赖返回的 text
      // 是否为空来决定挂载（二期大文件可能只返摘要，text 为空但文档已入库）。
      const failed = results.filter(
        (r) => r.format === "error" || r.format === "cancelled",
      )
      if (failed.length > 0) {
        console.warn(
          `[ingest] ${failed.length}/${results.length} 个文件摄取失败：`,
          failed.map((f) => ({ file: f.file_name, format: f.format })),
        )
      }
      // 成功项挂载到当前会话（全文已由后端 upsert 到 documents 表）。
      // mountDocument 后端会校验 documents 表存在该 path，返回 true=已挂载/false=未入库。
      if (conversationId) {
        let mountedCount = 0
        for (const r of results) {
          if (r.format === "error" || r.format === "cancelled") continue
          try {
            const ok = await ipc.mountDocument(conversationId, r.path)
            if (ok) {
              mountedCount += 1
            } else {
              // 未入库（罕见：ingest 成功但 upsert 失败，或 path 规范化不一致）。
              console.warn(
                `[ingest] mountDocument 返回 false，文档未入库：${r.file_name} (path=${r.path})`,
              )
            }
          } catch (e) {
            // 挂载失败不阻断摄取（用户可在 Library 手动挂载），但留痕便于排查。
            console.error(`[ingest] mountDocument 抛错：${r.file_name}`, e)
          }
        }
        if (mountedCount > 0) {
          console.info(`[ingest] 已挂载 ${mountedCount} 个文档到会话 ${conversationId}`)
        }
        // 刷新挂载列表 + 全局文档列表缓存
        qc.invalidateQueries({ queryKey: ["mounted-docs", conversationId] })
        qc.invalidateQueries({ queryKey: ["all-documents"] })
      } else {
        // 无会话时只刷新全局文档列表
        qc.invalidateQueries({ queryKey: ["all-documents"] })
      }
      return results
    },
  })
}

/**
 * 取消指定摄取任务。
 *
 * 供 IngestStatusBoard 等组件调用，避免组件直接 import `@/lib/ipc/commands`
 * （§12.1 硬约束：components 不得直接调 ipc）。
 */
export function useCancelIngest() {
  return useMutation({
    mutationFn: async (jobId: string) => {
      await ipc.cancelIngest(jobId)
    },
  })
}
