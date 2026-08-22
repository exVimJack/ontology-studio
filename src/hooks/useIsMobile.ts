// State 层：useIsMobile.ts（ARCHITECTURE.md §17.2 移动端降级 / 决策 F11）
//
// 移动端判定：matchMedia (max-width: 768px)。与 Tailwind 默认 sm=640/md=768 断点对齐，
// 768px 以下视为移动端布局（单栏 + Drawer + 全屏覆盖层）。
//
// 设计权衡：
// - 用 CSS 断点（hidden md:flex 等）处理纯样式切换，零 JS 开销；
// - 用本 hook 处理需要 JS 分支的逻辑（如 AppShell 桌面渲染 resizable 两栏 vs 移动渲染单栏+Drawer，
//   Sidebar 桌面常驻 vs 移动 Drawer 抽出），避免两套 DOM 同时存在导致的状态/事件冲突。
// - SSR 安全：初始值 false（桌面优先，决策 §17.2），挂载后 matchMedia 校正。
//
// 约束（§12.1）：本 hook 属 State 层，只依赖 react，不 import components/ipc。

import { useEffect, useState } from "react"

const MOBILE_QUERY = "(max-width: 768px)"

export function useIsMobile(): boolean {
  const [isMobile, setIsMobile] = useState(() =>
    typeof window !== "undefined" ? window.matchMedia(MOBILE_QUERY).matches : false,
  )

  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY)
    const handler = (e: MediaQueryListEvent) => setIsMobile(e.matches)
    // 初始校正（SSR/首次渲染窗口尺寸已变）
    setIsMobile(mq.matches)
    mq.addEventListener("change", handler)
    return () => mq.removeEventListener("change", handler)
  }, [])

  return isMobile
}
