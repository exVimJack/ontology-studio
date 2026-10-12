// UI 层：TitleBar.tsx（§20.5 / §17.2 移动端降级）
// 桌面：拖拽区 + 品牌 + 模型显示 + ⌘K 入口 + 侧栏切换。
// 移动：汉堡菜单（触发 Sidebar Drawer）+ 标题 + 搜索图标（触发 ⌘K）。
//   - 移动端无窗口拖拽需求（系统全屏管理），隐藏 data-tauri-drag-region
//   - 移动端无全局快捷键（§17.2），⌘K 入口降级为图标按钮触发面板
//
// 约束（§12.1）：经 hooks/store，不直接 invoke。

import { PanelLeft, Menu, Search } from "lucide-react"
import { useUiStore } from "@/stores/ui-store"
import { useIsMobile } from "@/hooks/useIsMobile"
import { shortcut } from "@/lib/keyboard"

export function TitleBar() {
  const isMobile = useIsMobile()
  const toggleSidebar = useUiStore((s) => s.toggleSidebar)
  const setMobileSidebarOpen = useUiStore((s) => s.setMobileSidebarOpen)
  const setPaletteOpen = useUiStore((s) => s.setCommandPaletteOpen)
  const sidebarCollapsed = useUiStore((s) => s.sidebarCollapsed)

  // ── 移动端：紧凑标题栏 ──
  if (isMobile) {
    return (
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-border bg-bg-elevated px-2 shadow-card pt-[env(safe-area-inset-top)]">
        <button
          onClick={() => setMobileSidebarOpen(true)}
          title="菜单"
          className="rounded p-2 text-fg hover:bg-bg-hover"
        >
          <Menu size={18} />
        </button>
        <span className="text-sm font-semibold tracking-tight select-none">onto-studio</span>
        <div className="flex-1" />
        <button
          onClick={() => setPaletteOpen(true)}
          title="搜索"
          className="rounded p-2 text-fg-muted hover:bg-bg-hover hover:text-fg"
        >
          <Search size={18} />
        </button>
      </div>
    )
  }

  // ── 桌面端：拖拽标题栏 ──
  return (
    <div
      data-tauri-drag-region
      className="flex h-10 shrink-0 items-center gap-2 border-b border-border bg-bg-elevated px-3 shadow-card"
    >
      {/* 左：侧栏切换 + 品牌 */}
      <button
        onClick={toggleSidebar}
        title="切换侧栏"
        className={`rounded p-1.5 hover:bg-bg-hover ${sidebarCollapsed ? "text-fg-subtle" : "text-fg"}`}
      >
        <PanelLeft size={16} />
      </button>
      <span className="text-sm font-semibold tracking-tight select-none">onto-studio</span>

      {/* 中：拖拽区（留空） */}
      <div data-tauri-drag-region className="flex-1" />

      {/* ⌘K 入口：图标按钮（快捷键提示放 title） */}
      <button
        onClick={() => setPaletteOpen(true)}
        title={`命令面板 (${shortcut(["mod", "K"])})`}
        className="rounded-md border border-border p-1.5 text-fg-muted hover:bg-bg-hover hover:text-fg"
      >
        <Search size={13} />
      </button>
    </div>
  )
}
