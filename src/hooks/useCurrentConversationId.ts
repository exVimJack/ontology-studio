// Hooks 层：useCurrentConversationId.ts（决策 F4）
// 从路由匹配派生当前会话 ID —— 路由为唯一真相源，无 Zustand 同步。
//
// 替代原 useUiStore.currentConversationId。所有需要“当前会话”的组件改用此 hook。
// 空态（/ 路由）返回 null。
//
// 实现说明：用 useRouterState 的 matches 数组（结构化路由匹配表）而非正则切 pathname。
// matches 包含当前匹配的所有路由（含父→子），每个 match 有 params。
// 从中找 conversationId 参数。routeTree.gen.ts 生成后此读取类型安全。

import { useRouterState } from "@tanstack/react-router"

/**
 * 当前会话 ID，从路由 `/chat/$conversationId` 派生。
 * 非 chat 路由（如 / 或 /library）返回 null。
 *
 * 路由为真相源：刷新/后退栈/深链/⌘N 新建都经路由，此 hook 自动反映。
 */
export function useCurrentConversationId(): string | null {
  return useRouterState({
    select: (s) => {
      // matches 按从根到叶的顺序；遍历找 conversationId 参数（chat.$conversationId 路由产生）
      for (const m of s.matches) {
        const id = (m.params as Record<string, unknown>).conversationId
        if (typeof id === "string") return id
      }
      return null
    },
  })
}
