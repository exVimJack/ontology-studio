import React from "react"
import ReactDOM from "react-dom/client"
import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { RouterProvider, createRouter } from "@tanstack/react-router"
import { routeTree } from "@/routeTree.gen"
import { installBrowserMock } from "@/lib/dev/mock-tauri"
import "@/styles.css"

// 浏览器开发兜底：非 Tauri 环境注入占位 invoke，避免 invoke 调用白屏崩溃。
// 真正功能请用 `npm run tauri dev`。
installBrowserMock()

// TanStack Query 单例（服务端状态层，§12.1）。
// 导出供路由 loader 预取用（chat.$conversationId loader 预热消息缓存）。
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // 桌面应用数据相对稳定，减少频繁重取
      staleTime: 30_000,
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
})

// TanStack Router 实例（决策 F4 文件式路由，routeTree 由 @tanstack/router-plugin 生成）
// defaultPreload: 关闭 intent 预取——原设置导致鼠标滑过侧栏会话项时触发大量
// 并发 listMessages 请求（DevTools 里一排“挂起”请求）。会话切换走显式 navigate，
// loader 会在切路由时自动执行预热缓存，不需要 hover 预取。
const router = createRouter({
  routeTree,
  defaultPreload: false,
})

// 类型注册（让 useRouterState/useNavigate 等拿到全路由类型）
declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </React.StrictMode>,
)
