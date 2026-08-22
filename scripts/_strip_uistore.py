# -*- coding: utf-8 -*-
"""一次性脚本：从 ui-store.ts 移除 currentConversationId / setCurrentConversation（路由为真相源）。"""
import io

path = "src/stores/ui-store.ts"
with io.open(path, "r", encoding="utf-8") as f:
    content = f.read()

# 1. interface 移除 currentConversationId + setCurrentConversation
old_iface = """interface UiState {
  theme: Theme
  sidebarCollapsed: boolean
  inspectorCollapsed: boolean
  currentConversationId: string | null
  commandPaletteOpen: boolean
  settingsOpen: boolean

  setTheme: (t: Theme) => void
  toggleSidebar: () => void
  toggleInspector: () => void
  setCurrentConversation: (id: string | null) => void
  setCommandPaletteOpen: (open: boolean) => void
  setSettingsOpen: (open: boolean) => void
}"""
new_iface = """interface UiState {
  theme: Theme
  sidebarCollapsed: boolean
  inspectorCollapsed: boolean
  commandPaletteOpen: boolean
  settingsOpen: boolean

  setTheme: (t: Theme) => void
  toggleSidebar: () => void
  toggleInspector: () => void
  setCommandPaletteOpen: (open: boolean) => void
  setSettingsOpen: (open: boolean) => void
}"""
assert content.count(old_iface) == 1, f"iface count={content.count(old_iface)}"
content = content.replace(old_iface, new_iface)

# 2. 实现移除 currentConversationId 字段 + setCurrentConversation setter
old_impl = """    theme: "system",
    sidebarCollapsed: false,
    inspectorCollapsed: true,
    currentConversationId: null,
    commandPaletteOpen: false,
    settingsOpen: false,

    setTheme: (theme) => set({ theme }),
    toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
    toggleInspector: () => set((s) => ({ inspectorCollapsed: !s.inspectorCollapsed })),
    setCurrentConversation: (currentConversationId) => {
      set({ currentConversationId })
      // 切换会话时清理上一会话残留的已结束摄入任务（Done/Error/Cancelled），
      // 保留进行中的。jobs 全局不分会话，不清理则新会话看板仍显示旧任务，
      // 造成“历史还在”观感。动态 import 避免 store 间静态依赖环。
      void import("@/stores/ingest-store").then((m) =>
        m.useIngestStore.getState().clearFinished(),
      )
    },
    setCommandPaletteOpen: (commandPaletteOpen) => set({ commandPaletteOpen }),
    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),"""
new_impl = """    theme: "system",
    sidebarCollapsed: false,
    inspectorCollapsed: true,
    commandPaletteOpen: false,
    settingsOpen: false,

    setTheme: (theme) => set({ theme }),
    toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
    toggleInspector: () => set((s) => ({ inspectorCollapsed: !s.inspectorCollapsed })),
    // 当前会话切换由路由驱动（决策 F4），清理摄入残留的副作用在 __root.tsx 的
    // useCurrentConversationId effect 中触发，不再由 store setter 承担。
    setCommandPaletteOpen: (commandPaletteOpen) => set({ commandPaletteOpen }),
    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),"""
assert content.count(old_impl) == 1, f"impl count={content.count(old_impl)}"
content = content.replace(old_impl, new_impl)

# 3. partialize 注释（移除 currentConversationId 提及）
old_part = """      name: "onto-studio-ui",
      // 只持久化偏好类字段，currentConversationId 不持久化（避免脏状态）
      partialize: (s) => ({"""
new_part = """      name: "onto-studio-ui",
      // 只持久化偏好类字段
      partialize: (s) => ({"""
assert content.count(old_part) == 1, f"part count={content.count(old_part)}"
content = content.replace(old_part, new_part)

with io.open(path, "w", encoding="utf-8") as f:
    f.write(content)
print("OK: ui-store currentConversationId removed")
