// UI 层：NavRail.tsx（微信桌面版式左侧导航栏）
// 六个功能入口（会话/知识库/数据源/技能/本体/校验任务），两种模式：
//   - 收起（默认）：固定 52px 纯图标，hover 自绘气泡 tooltip（微信观感）
//   - 展开：约 168px 图标+文字，不参与拖宽
// 底部切换按钮；选中态高亮左侧竖条。窄窗口（移动端）恒为收起模式（方案 A）。
//
// 切换语义：activeNav 是唯一状态源。会话视图恢复 AppShell 原有两栏布局；
// 其余视图接管主区（覆盖层组件由 __root 按 activeNav 条件渲染）。
// 兼容既有入口（CommandPalette / 快捷键 / SkillPopover / Sidebar）：全部改走
// setActiveNav，同一状态源不打架。

import {
  Boxes,
  ShieldCheck,
  Database,
  FolderOpen,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  Server,
  Sparkles,
  Sun,
  Moon,
  MonitorCog,
  Cpu,
  MoreHorizontal,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useNavigate } from "@tanstack/react-router";
import { useUiStore, applyTheme, type ActiveNav } from "@/stores/ui-store";
import { useIsMobile } from "@/hooks/useIsMobile";
import { useState } from "react";

interface NavItem {
  key: ActiveNav;
  label: string;
  icon: LucideIcon;
}

const NAV_ITEMS: NavItem[] = [
  { key: "chat", label: "会话", icon: MessageSquare },
  { key: "library", label: "知识库", icon: FolderOpen },
  { key: "federation", label: "数据源", icon: Database },
  { key: "skills", label: "技能", icon: Sparkles },
  { key: "ontology", label: "本体", icon: Boxes },
  { key: "validation", label: "校验任务", icon: ShieldCheck },
];

export function NavRail() {
  const activeNav = useUiStore((s) => s.activeNav);
  const setActiveNav = useUiStore((s) => s.setActiveNav);
  const navRailExpanded = useUiStore((s) => s.navRailExpanded);
  const toggleNavRail = useUiStore((s) => s.toggleNavRail);
  const setModelSettingsOpen = useUiStore((s) => s.setModelSettingsOpen);
  const setMcpSettingsOpen = useUiStore((s) => s.setMcpSettingsOpen);
  const theme = useUiStore((s) => s.theme);
  const setTheme = useUiStore((s) => s.setTheme);
  const [menuOpen, setMenuOpen] = useState(false);
  const navigate = useNavigate();
  // 窄窗口恒为收起模式（方案 A）：不占主区空间，功能照常切换
  const isMobile = useIsMobile();
  const expanded = navRailExpanded && !isMobile;

  function select(key: ActiveNav) {
    if (key === "chat") {
      // 会话视图恢复路由驱动的主区（空态或最近会话）
      // 无当前会话时路由可能停在 /library 等，导航回 / 保证 ChatArea 渲染
      setActiveNav("chat");
      navigate({ to: "/" });
    } else {
      setActiveNav(key);
      if (key === "library") navigate({ to: "/library" });
    }
  }

  return (
    <nav
      className={
        "flex shrink-0 flex-col items-stretch border-r border-border bg-bg-elevated py-2 shadow-card " +
        (expanded ? "w-[168px]" : "w-[52px]")
      }
    >
      {/* 功能入口 */}
      <div className="flex flex-1 flex-col gap-1 px-1.5">
        {NAV_ITEMS.map((item) => (
          <NavButton
            key={item.key}
            item={item}
            expanded={expanded}
            active={activeNav === item.key}
            onClick={() => select(item.key)}
          />
        ))}
      </div>

      {/* 底部：设置菜单（微信桌面版式——主界面左下角弹出菜单） */}
      <div className="px-1.5">
        <div className="relative">
          <button
            onClick={() => setMenuOpen(!menuOpen)}
            className={
              "flex w-full items-center justify-center gap-2 rounded-md py-2.5 transition " +
              (menuOpen
                ? "bg-bg-hover text-fg"
                : "text-fg-subtle hover:bg-bg-hover/50 hover:text-fg")
            }
            title={expanded ? "" : "设置"}
          >
            {expanded ? (
              <>
                <MoreHorizontal size={18} />
                <span className="text-sm">设置</span>
              </>
            ) : (
              <MoreHorizontal size={20} />
            )}
          </button>
          {menuOpen && (
            <>
              {/* 点击外部关闭 */}
              <div className="fixed inset-0 z-40" onClick={() => setMenuOpen(false)} />
              {/* 弹出菜单：向右上方弹出（微信左下角菜单形态） */}
              <div className="absolute bottom-0 left-[calc(100%+8px)] z-50 w-48 overflow-hidden rounded-lg border border-border bg-bg-elevated py-1 shadow-lg">
                <MenuEntry icon={Cpu} label="模型设置" onClick={() => { setMenuOpen(false); setModelSettingsOpen(true); }} />
                <MenuEntry icon={Server} label="工具服务器 (MCP)" onClick={() => { setMenuOpen(false); setMcpSettingsOpen(true); }} />
                <div className="my-1 border-t border-border" />
                <MenuEntry
                  icon={theme === "light" ? Sun : theme === "dark" ? Moon : MonitorCog}
                  label={theme === "light" ? "主题：浅色" : theme === "dark" ? "主题：深色" : "主题：跟随系统"}
                  sub
                  onClick={() => {
                    // 循环切换 浅→深→系统
                    const next = theme === "light" ? "dark" : theme === "dark" ? "system" : "light";
                    setTheme(next);
                    applyTheme(next);
                  }}
                />
              </div>
            </>
          )}
        </div>
      </div>

      {/* 底部：展开/收起切换（窄窗口隐藏，恒收起不可切） */}
      {!isMobile && (
        <div className="px-1.5">
          <button
            onClick={toggleNavRail}
            className="flex w-full items-center justify-center gap-2 rounded-md py-2 text-fg-subtle hover:bg-bg-hover hover:text-fg"
            title={expanded ? "收起导航栏" : "展开导航栏"}
          >
            {expanded ? (
              <>
                <PanelLeftClose size={16} />
                <span className="text-xs">收起</span>
              </>
            ) : (
              <PanelLeftOpen size={16} />
            )}
          </button>
        </div>
      )}
    </nav>
  );
}

function NavButton({
  item,
  expanded,
  active,
  onClick,
}: {
  item: NavItem;
  expanded: boolean;
  active: boolean;
  onClick: () => void;
}) {
  const Icon = item.icon;
  if (expanded) {
    return (
      <button
        onClick={onClick}
        className={
          "flex items-center gap-2.5 rounded-md px-3 py-2 text-sm transition " +
          (active
            ? "bg-bg-hover font-medium text-accent"
            : "text-fg-subtle hover:bg-bg-hover/50 hover:text-fg")
        }
      >
        <Icon size={18} />
        <span>{item.label}</span>
      </button>
    );
  }
  // 收起模式：纯图标 + hover 自绘气泡 tooltip（微信观感，非原生 title）
  return (
    <div className="group/rail relative flex justify-center">
      <button
        onClick={onClick}
        className={
          "relative flex w-full items-center justify-center rounded-md py-2.5 transition " +
          (active
            ? "bg-bg-hover text-accent"
            : "text-fg-subtle hover:bg-bg-hover/50 hover:text-fg")
        }
      >
        {active && (
          <span className="absolute left-[-6px] top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-full bg-accent" />
        )}
        <Icon size={20} />
      </button>
      {/* 气泡：hover 显示，出现在图标右侧；peer 技巧不适合按钮组，用 group-hover */}
      <span className="pointer-events-none absolute left-[calc(100%+8px)] top-1/2 z-50 -translate-y-1/2 whitespace-nowrap rounded-md border border-border bg-bg-elevated px-2.5 py-1 text-xs text-fg opacity-0 shadow-md transition-opacity duration-150 group-hover/rail:opacity-100">
        {item.label}
      </span>
    </div>
  );
}

/** 左下角设置弹出菜单的条目（微信菜单风格：图标 + 文字 + 右侧状态）。 */
function MenuEntry({
  icon: Icon,
  label,
  onClick,
  sub,
}: {
  icon: LucideIcon;
  label: string;
  onClick: () => void;
  sub?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-[13px] text-fg transition hover:bg-bg-hover"
    >
      <Icon size={15} className="shrink-0 text-fg-subtle" />
      <span className="flex-1 truncate">{label}</span>
      {sub && <span className="text-[11px] text-fg-subtle">点击切换</span>}
    </button>
  );
}
