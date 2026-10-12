// State 层：ui-store.ts（§12.2）
// 全局 UI 状态：主题、侧栏折叠、面板开关。
// 服务端状态（会话列表/消息）走 TanStack Query，不放这里。
//
// 注：当前会话切换已由路由驱动（决策 F4），不再存于 store。
// useCurrentConversationId() hook 从路由参数派生，清理摄入残留的副作用
// 在 __root.tsx 的 effect 中触发。

import { create } from "zustand";
import { persist } from "zustand/middleware";
// 一期用 zustand/middleware persist + localStorage 的等价物。
// 架构约束（§F8）：禁止 localStorage/IndexedDB，UI 偏好走 @tauri-store/zustand。
// 但 @tauri-store/zustand 的 API 在 web/dev 环境需要 Tauri runtime 才可用，
// 一期 dev 阶段先用内存态 + persist（dev 期 web 模式无 Tauri），Tauri 集成后切 store。
// TODO: 接入 @tauri-store/zustand 替换 persist（见 ARCHITECTURE.md §18）

export type Theme = "light" | "dark" | "system";

// 左侧导航栏（NavRail）当前激活的功能视图。
// "chat" 恢复会话两栏布局；其余对应主区整页视图。
export type ActiveNav =
  | "chat"
  | "library"
  | "federation"
  | "skills"
  | "ontology"
  | "validation";

interface UiState {
    theme: Theme;
    sidebarCollapsed: boolean;
    // 左侧导航栏（NavRail）：当前激活视图 + 展开/收起（微信桌面版式）
    activeNav: ActiveNav;
    /** 校验任务工作台当前选中的本体 IRI（对话命令/工具栏触发时带入）。 */
    validationIri: string | null;
    navRailExpanded: boolean;
    // 移动端 Sidebar Drawer 开关（§17.2）。桌面端用 sidebarCollapsed 折叠，二者语义独立。
    mobileSidebarOpen: boolean;
    commandPaletteOpen: boolean;
    // 模型设置弹窗（设置重构 ①：从总设置拆出的独立入口，⌘M / NavRail 左下角菜单）
    modelSettingsOpen: boolean;
    // MCP 工具服务器弹窗（设置重构 ③：与模型设置同形态，⌘⇧M / NavRail 左下角菜单）
    mcpSettingsOpen: boolean;
    // Library 上传时的目标文件夹（临时态，不持久化）。
    // LibraryView 选中文件夹时同步写入；FileDropZone 上传时读取。
    // null = 根目录散文件。会话页上传时忽略此值（仍落 /Inbox）。
    libraryUploadFolder: string | null;
    libraryViewActive: boolean;

    setTheme: (t: Theme) => void;
    toggleSidebar: () => void;
    setActiveNav: (nav: ActiveNav) => void;
    setValidationIri: (iri: string | null) => void;
    toggleNavRail: () => void;
    setMobileSidebarOpen: (open: boolean) => void;
    setCommandPaletteOpen: (open: boolean) => void;
    setModelSettingsOpen: (open: boolean) => void;
    setMcpSettingsOpen: (open: boolean) => void;
    setLibraryUploadFolder: (folder: string | null) => void;
    setLibraryViewActive: (active: boolean) => void;
}

export const useUiStore = create<UiState>()(
    persist(
        (set) => ({
            theme: "system",
            sidebarCollapsed: false,
            activeNav: "chat",
            validationIri: null,
            navRailExpanded: false,
            mobileSidebarOpen: false,
            commandPaletteOpen: false,
            modelSettingsOpen: false,
            mcpSettingsOpen: false,
            libraryUploadFolder: null,
            libraryViewActive: false,

            setTheme: (theme) => set({ theme }),
            toggleSidebar: () =>
                set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
            setActiveNav: (activeNav) => set({ activeNav }),
            setValidationIri: (validationIri) => set({ validationIri }),
            toggleNavRail: () =>
                set((s) => ({ navRailExpanded: !s.navRailExpanded })),
            setMobileSidebarOpen: (mobileSidebarOpen) =>
                set({ mobileSidebarOpen }),
            setCommandPaletteOpen: (commandPaletteOpen) =>
                set({ commandPaletteOpen }),
            setModelSettingsOpen: (modelSettingsOpen) => set({ modelSettingsOpen }),
            setMcpSettingsOpen: (mcpSettingsOpen) => set({ mcpSettingsOpen }),
            setLibraryUploadFolder: (libraryUploadFolder) =>
                set({ libraryUploadFolder }),
            setLibraryViewActive: (libraryViewActive) =>
                set({ libraryViewActive }),
        }),
        {
            name: "onto-studio-ui",
            // 只持久化偏好类字段
            partialize: (s) => ({
                theme: s.theme,
                sidebarCollapsed: s.sidebarCollapsed,
                navRailExpanded: s.navRailExpanded,
            }),
        },
    ),
);

/** 把 theme 应用到 <html>（system 跟随 prefers-color-scheme）。 */
export function applyTheme(theme: Theme) {
    const root = document.documentElement;
    const isDark =
        theme === "dark" ||
        (theme === "system" &&
            window.matchMedia("(prefers-color-scheme: dark)").matches);
    root.classList.toggle("dark", isDark);
}
