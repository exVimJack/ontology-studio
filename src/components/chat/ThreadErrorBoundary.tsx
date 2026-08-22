// UI 层：ThreadErrorBoundary.tsx
// 捕获 assistant-ui Thread 子树渲染异常，避免整页白屏。
// 开发期直接把错误栈贴出来便于定位。

import { Component, type ErrorInfo, type ReactNode } from "react"

interface State {
  error: Error | null
}

export class ThreadErrorBoundary extends Component<
  { children: ReactNode },
  State
> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("[ThreadErrorBoundary] 渲染异常:", error, info)
  }

  render() {
    if (this.state.error) {
      return (
        <div className="m-4 rounded-md border border-danger/40 bg-danger/10 p-4 text-sm">
          <div className="mb-2 font-medium text-danger">对话区渲染出错</div>
          <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-all text-xs text-fg-muted">
            {this.state.error.message}
            {"\n\n"}
            {this.state.error.stack}
          </pre>
          <button
            onClick={() => this.setState({ error: null })}
            className="mt-3 rounded-md border border-border px-2.5 py-1 text-xs hover:bg-bg-hover"
          >
            重试
          </button>
        </div>
      )
    }
    return this.props.children
  }
}
