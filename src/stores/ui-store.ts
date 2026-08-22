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

interface UiState {
    theme: Theme;
    sidebarCollapsed: boolean;
    // 移动端 Sidebar Drawer 开关（§17.2）。桌面端用 sidebarCollapsed 折叠，二者语义独立。
    mobileSidebarOpen: boolean;
    commandPaletteOpen: boolean;
    settingsOpen: boolean;
    federationOpen: boolean;
    skillsOpen: boolean;
    ontologyOpen: boolean;
    // Library 上传时的目标文件夹（临时态，不持久化）。
    // LibraryView 选中文件夹时同步写入；FileDropZone 上传时读取。
    // null = 根目录散文件。会话页上传时忽略此值（仍落 /Inbox）。
    libraryUploadFolder: string | null;
    libraryViewActive: boolean;

    setTheme: (t: Theme) => void;
    toggleSidebar: () => void;
    setMobileSidebarOpen: (open: boolean) => void;
    setCommandPaletteOpen: (open: boolean) => void;
    setSettingsOpen: (open: boolean) => void;
    setFederationOpen: (open: boolean) => void;
    setSkillsOpen: (open: boolean) => void;
    setOntologyOpen: (open: boolean) => void;
    setLibraryUploadFolder: (folder: string | null) => void;
    setLibraryViewActive: (active: boolean) => void;
}

export const useUiStore = create<UiState>()(
    persist(
        (set) => ({
            theme: "system",
            sidebarCollapsed: false,
            mobileSidebarOpen: false,
            commandPaletteOpen: false,
            settingsOpen: false,
            federationOpen: false,
            skillsOpen: false,
            ontologyOpen: false,
            libraryUploadFolder: null,
            libraryViewActive: false,

            setTheme: (theme) => set({ theme }),
            toggleSidebar: () =>
                set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
            setMobileSidebarOpen: (mobileSidebarOpen) =>
                set({ mobileSidebarOpen }),
            setCommandPaletteOpen: (commandPaletteOpen) =>
                set({ commandPaletteOpen }),
            setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
            setFederationOpen: (federationOpen) => set({ federationOpen }),
            setSkillsOpen: (skillsOpen) => set({ skillsOpen }),
            setOntologyOpen: (ontologyOpen) => set({ ontologyOpen }),
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
