// 浏览器开发兜底：当不在 Tauri WebView 内运行时（如直接 `npm run dev` 用浏览器访问），
// window.__TAURI_INTERNALS__ 不存在，@tauri-apps/api/core 的 invoke 会抛
// "Cannot read properties of undefined (reading 'invoke')"。
//
// 本模块在浏览器环境注入一个占位 invoke，让调用方拿到可读错误而非白屏崩溃。
// 真正要跑通功能请用 `npm run tauri dev`。

const TAURI_INTERNALS = "__TAURI_INTERNALS__" as const

function isTauriWebview(): boolean {
  return typeof window !== "undefined" && !!(window as any)[TAURI_INTERNALS]
}

/**
 * 若处于浏览器（非 Tauri）环境，注入一个 mock 的 __TAURI_INTERNALS__.invoke，
 * 使所有 command 返回一个明确的错误，便于 UI 调试。
 *
 * 在 Tauri WebView 中此函数为 no-op。
 */
export function installBrowserMock(): void {
  if (isTauriWebview()) return
  if (typeof window === "undefined") return

  // 已安装则跳过
  if ((window as any).__TAURI_MOCK_INSTALLED__) return

  const mockInvoke = (cmd: string) => {
    return Promise.reject(
      new Error(
        `[browser-mock] Tauri command "${cmd}" 未执行：当前运行在浏览器而非 Tauri WebView。\n` +
          `请改用 \`npm run tauri dev\` 启动以调用后端。`,
      ),
    )
  }

  Object.defineProperty(window, TAURI_INTERNALS, {
    value: { invoke: mockInvoke, transformCallback: () => 0, convertFileSrc: (p: string) => p },
    configurable: true,
  })

  ;(window as any).__TAURI_MOCK_INSTALLED__ = true

  // 仅在开发环境打印一次提示
  if (import.meta.env?.DEV) {
    // eslint-disable-next-line no-console
    console.warn(
      "[browser-mock] 检测到浏览器环境，已注入 Tauri 占位 invoke。后端命令不会真正执行，请用 `npm run tauri dev` 跑完整功能。",
    )
  }
}
