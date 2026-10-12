// UI 层：AppShell.tsx（§12.2 / §20.5 / §17.2 移动端降级）
// 桌面：两栏 resizable（Sidebar | Outlet），react-resizable-panels v4。
// 移动：单栏 Outlet + Sidebar 作为 Drawer 抽出（点击遮罩/选会话关闭）。
//
// 决策 F11：单套代码 + Tailwind 断点 + 自适应原语，不做双套代码。
// 第三栏 Inspector 已移除（方案 C），挂载文档管理收进 ScopeChip popover，
// 会话级 skill 开关收进 Composer 的 SkillPopover。
//
// 约束（§12.1）：本组件经 hooks/store 读 UI 状态，不直接 invoke。

import { Group, Panel, Separator } from "react-resizable-panels"
import { Outlet } from "@tanstack/react-router"
import { X } from "lucide-react"
import { Sidebar } from "@/components/shell/Sidebar"
import { NavRail } from "@/components/shell/NavRail"
import { lazy, Suspense } from "react"

const FederationView = lazy(() =>
  import("@/components/federation/FederationView").then((m) => ({ default: m.FederationView })),
)
const SkillView = lazy(() =>
  import("@/components/skill/SkillView").then((m) => ({ default: m.SkillView })),
)
const OntologyView = lazy(() =>
  import("@/components/ontology/OntologyView").then((m) => ({ default: m.OntologyView })),
)
const ValidationWorkbenchView = lazy(() =>
  import("@/components/ontology/ValidationWorkbenchView").then((m) => ({
    default: m.ValidationWorkbenchView,
  })),
)
import { useUiStore } from "@/stores/ui-store"
import { useIsMobile } from "@/hooks/useIsMobile"

// 加载占位（懒加载视图 fallback）
function ViewLoading() {
  return (
    <div className="flex h-full items-center justify-center text-xs text-fg-subtle">
      加载中…
    </div>
  )
}

export function AppShell() {
  const isMobile = useIsMobile()
  const sidebarCollapsed = useUiStore((s) => s.sidebarCollapsed)
  const activeNav = useUiStore((s) => s.activeNav)
  // 移动端 Drawer 开关（桌面端用 sidebarCollapsed 折叠，二者语义独立）
  const mobileSidebarOpen = useUiStore((s) => s.mobileSidebarOpen)
  const setMobileSidebarOpen = useUiStore((s) => s.setMobileSidebarOpen)

  // ── 移动端：单栏 + Drawer（NavRail 恒收起模式，方案 A）──
  if (isMobile) {
    return (
      <div className="flex h-full w-full flex-row-reverse overflow-hidden bg-bg text-fg">
        {/* NavRail 收起模式恒渲染（flex-row-reverse 使它视觉上仍在左侧） */}
        <div className="relative min-h-0 flex-1">
          <Outlet />
          {/* 功能视图：absolute 覆盖主区，fade-in 柔化切换 */}
          {activeNav === "federation" && (
            <div className="fade-in absolute inset-0">
              <Suspense fallback={<ViewLoading />}><FederationView /></Suspense>
            </div>
          )}
          {activeNav === "skills" && (
            <div className="fade-in absolute inset-0">
              <Suspense fallback={<ViewLoading />}><SkillView /></Suspense>
            </div>
          )}
          {activeNav === "ontology" && (
            <div className="fade-in absolute inset-0">
              <Suspense fallback={<ViewLoading />}><OntologyView /></Suspense>
            </div>
          )}
          {activeNav === "validation" && (
            <div className="fade-in absolute inset-0">
              <Suspense fallback={<ViewLoading />}><ValidationWorkbenchView /></Suspense>
            </div>
          )}
        </div>
        <NavRail />

        {/* Sidebar Drawer：从左侧滑出，遮罩点击关闭 */}
        {mobileSidebarOpen && (
          <div className="fixed inset-0 z-50 flex">
            {/* 遮罩 */}
            <div
              className="absolute inset-0 bg-black/40"
              onClick={() => setMobileSidebarOpen(false)}
            />
            {/* 抽屉：占 85% 宽，最大 360px，左侧安全区内缩 */}
            <div className="relative flex w-[85vw] max-w-[360px] flex-col border-r border-border bg-bg-elevated pl-[env(safe-area-inset-left)]">
              <div className="flex items-center justify-end border-b border-border px-3 py-2">
                <button
                  onClick={() => setMobileSidebarOpen(false)}
                  className="rounded p-1.5 text-fg-subtle hover:bg-bg-hover hover:text-fg"
                  title="关闭"
                >
                  <X size={18} />
                </button>
              </div>
              <div className="min-h-0 flex-1">
                <Sidebar onNavigate={() => setMobileSidebarOpen(false)} />
              </div>
            </div>
          </div>
        )}
      </div>
    )
  }

  // ── 桌面端：NavRail + （会话视图时）两栏 resizable ──
  // 非 chat 视图：NavRail + 主区整页（知识库走路由 Outlet；本体/数据源/技能
  // 由 __root 按 activeNav 条件渲染为覆盖层，盖住主区但不再盖住 NavRail）。
  return (
    <div className="flex h-full w-full overflow-hidden bg-bg text-fg">
      <NavRail />
      <Group orientation="horizontal" className="flex h-full min-w-0 flex-1" id="onto-studio-layout">
        {activeNav === "chat" && !sidebarCollapsed && (
          <>
            <Panel defaultSize="300px" minSize="260px" maxSize="440px" collapsible>
              <Sidebar />
            </Panel>
            <Separator className="w-px cursor-col-resize bg-border hover:bg-accent/40 transition-colors" />
          </>
        )}

        <Panel minSize="40%">
          <div className="relative h-full">
            <Outlet />
            {/* 功能视图：absolute 覆盖主区（NavRail 仍可见可切换）。
                知识库走路由 Outlet（library.tsx），无需覆盖层。fade-in 柔化切换。 */}
            {activeNav === "federation" && (
              <div className="fade-in absolute inset-0">
                <Suspense fallback={<ViewLoading />}><FederationView /></Suspense>
              </div>
            )}
            {activeNav === "skills" && (
              <div className="fade-in absolute inset-0">
                <Suspense fallback={<ViewLoading />}><SkillView /></Suspense>
              </div>
            )}
            {activeNav === "ontology" && (
              <div className="fade-in absolute inset-0">
                <Suspense fallback={<ViewLoading />}><OntologyView /></Suspense>
              </div>
            )}
          {activeNav === "validation" && (
            <div className="fade-in absolute inset-0">
              <Suspense fallback={<ViewLoading />}><ValidationWorkbenchView /></Suspense>
            </div>
          )}
          </div>
        </Panel>
      </Group>
    </div>
  )
}
