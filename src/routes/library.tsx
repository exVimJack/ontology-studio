// 路由：/library （文件库视图，§20.8）
// 浏览 + 管理已摄入文件（聚合 ingest-store + RAG 索引源）。
// 摄入仍走 chat 路由的 Composer（⌘O / 拖拽），此视图聚焦浏览/挂载/删除。

import { createFileRoute } from "@tanstack/react-router"
import { LibraryView } from "@/components/library/LibraryView"

export const Route = createFileRoute("/library")({
  component: LibraryRoute,
})

function LibraryRoute() {
  return <LibraryView />
}
