// UI 层：Thread.tsx
// 用 assistant-ui Primitives 组装消息流（替换手写 MessageScroller）。
// auto-scroll / end-anchor / jump-to-latest 全部由 ThreadPrimitive.Viewport 原生处理。
//
// 消息渲染：MessagePrimitive.Parts 按 part 类型分派：
//   - text → AssistantText / UserText（流式渲染 + 旋转指示）
//   - reasoning → Reasoning（可折叠，流式时展开、结束折叠）
//   - tool-call → 不渲染（工具卡片已下线，convertMessage 不产出该 part）
//
// 关键：所有 components 对象和子组件都在模块作用域定义，
// 避免流式时每次 render 新建引用导致 assistant-ui 内部 memo 失效/死循环。
// （官方文档：Define the components object once at module scope.）

import {
  ThreadPrimitive,
  MessagePrimitive,
  MessagePartPrimitive,
  ComposerPrimitive,
  ActionBarPrimitive,
  AuiIf,
  useAuiState,
  useThreadViewportStore,
} from "@assistant-ui/react"
import { useEffect, useRef, useState } from "react"
import { ArrowDown, Brain, Loader2, User, Bot, Copy, Check, RotateCcw, Pencil, FileDown } from "lucide-react"
import { MarkdownText } from "./MarkdownText"
import { saveTextFile } from "@/lib/save-file"
import { useChatRunStatus } from "./ChatRuntime"
import { formatMessageTime } from "@/lib/format"
import {
  ELAPSED_VISIBLE_AFTER_S,
  elapsedSeconds,
  runPhaseText,
} from "@/lib/chat-run-status"

export function Thread() {
  // 切会话/加载历史后滚动到底部。
  //
  // 背景：assistant-ui 的 `scrollToBottomOnThreadSwitch` 依赖 `threadListItem.switchedTo`
  // 事件，但本项目用 ExternalStoreRuntime + 路由切换 conversationId，不触发该事件。
  // `scrollToBottomOnInitialize` 在 hasMessages 从 false 变 true 时触发，但对于异步
  // 加载的历史，调用时 scrollHeight 可能还没稳定。
  //
  // 这里在消息数量变化时主动调一次 scrollToBottom。assistant-ui 的 useOnResizeContent
  // 会在 scrollingToBottomBehaviorRef intent 存续期间持续跟随高度变化（markdown
  // 渲染导致的高度增长），直到 handleScroll 确认抵达底部才清除 intent。所以一次
  // 主动触发即可，不需要我们重复调度。
  const viewportStore = useThreadViewportStore()
  const messageCount = useAuiState((s) => s.thread.messages.length)
  const lastSeenCountRef = useRef(0)
  useEffect(() => {
    if (messageCount === 0) {
      lastSeenCountRef.current = 0
      return
    }
    if (messageCount === lastSeenCountRef.current) return
    lastSeenCountRef.current = messageCount
    // 延到下一帧：等 React commit + 浏览器 layout，scrollHeight 准确后再滚
    const rafId = window.requestAnimationFrame(() => {
      viewportStore.getState().scrollToBottom({ behavior: "instant" })
    })
    return () => window.cancelAnimationFrame(rafId)
  }, [messageCount, viewportStore])

  return (
    <ThreadPrimitive.Root className="flex h-full flex-col">
      <ThreadPrimitive.Viewport className="relative flex-1 overflow-y-auto overflow-x-hidden">
        {/* 空态 */}
        <AuiIf condition={(s) => s.thread.isEmpty}>
          <div className="flex h-full items-center justify-center text-sm text-fg-subtle">
            输入消息开始对话
          </div>
        </AuiIf>

        {/* 消息列自适应窗口宽度（不限宽居中），左右留白与 Composer(p-3)对齐。
            Messages primitive 不收 className,故外包约束层。
            min-w-0 + overflow-hidden：flex/grid 子项默认 min-width:auto，
            宽内容（长表格/代码块）会把祖先撑出窗口右边界。 */}
        <div className="w-full min-w-0 overflow-hidden px-3">
          <ThreadPrimitive.Messages>
            {({ message }) => {
              if (message.role === "user") return <UserMessage />
              return <AssistantMessage />
            }}
          </ThreadPrimitive.Messages>
        </div>

        {/* 回到最新按钮：ViewportFooter sticky 钉在 viewport 底部，
            auto-scroll 系统会扣除其高度避免遮挡最后一条消息 */}
        <ThreadPrimitive.ViewportFooter className="pointer-events-none sticky bottom-4 z-10 flex justify-center">
          <ThreadPrimitive.ScrollToBottom className="pointer-events-auto flex items-center gap-1.5 rounded-full border border-border bg-bg-elevated px-3 py-1.5 text-xs shadow-md hover:bg-bg-hover">
            <ArrowDown size={14} /> 回到最新
          </ThreadPrimitive.ScrollToBottom>
        </ThreadPrimitive.ViewportFooter>
      </ThreadPrimitive.Viewport>
    </ThreadPrimitive.Root>
  )
}

// ───────── 模块作用域的 part 组件（避免流式时重建引用） ─────────

function UserText() {
  return (
    <p style={{ whiteSpace: "pre-line" }}>
      <MessagePartPrimitive.Text />
    </p>
  )
}

/** user 消息的 parts 配置（模块作用域常量）。 */
const USER_PARTS_COMPONENTS = {
  Text: UserText,
} as const

function AssistantText() {
  return (
    <div className="text-sm">
      <LazyMarkdownText />
    </div>
  )
}

/** 超长文本懒渲染：视口外不解析 markdown。
 *
 * react-markdown 是同步解析（每次 render 都 parse + runSync），单条 120KB
 * 消息首次渲染要 400~900ms 阻塞主线程。历史会话含多条超长消息时切会话卡几秒。
 *
 * 方案：在 part scope 内拿到 text，超过阈值时用 IntersectionObserver 懒渲染——
 * 视口外渲染纯文本截断预览（不走 markdown 解析），视口内才挂 MarkdownText。
 * 一旦挂载过完整内容就保持（避免反复进出视口重复解析）。流式中的 part 始终完整渲染。 */
const LAZY_MARKDOWN_THRESHOLD = 8_000

function LazyMarkdownText() {
  const part = useAuiState((s) => {
    if (s.part.type !== "text") return null
    return s.part
  })
  const text = part?.text ?? ""
  const isStreaming = part?.status?.type === "running"
  const shouldLazy = text.length >= LAZY_MARKDOWN_THRESHOLD && !isStreaming

  const containerRef = useRef<HTMLDivElement | null>(null)
  const [visible, setVisible] = useState(false)

  useEffect(() => {
    if (!shouldLazy) return
    const el = containerRef.current
    if (!el) return
    // 已经可见就直接渲染
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setVisible(true)
          io.disconnect()
        }
      },
      // 提前一点加载，滚入时几乎无感
      { rootMargin: "200px 0px" },
    )
    io.observe(el)
    return () => io.disconnect()
  }, [shouldLazy])

  // 短文本 / 流式 / 已进视口：渲染完整 markdown
  if (!shouldLazy || visible) {
    return <MarkdownText />
  }

  // 视口外的超长消息：纯文本截断预览（不走 markdown 解析）
  const PREVIEW_CHARS = 600
  const preview = text.slice(0, PREVIEW_CHARS)
  const truncated = text.length > PREVIEW_CHARS
  return (
    <div ref={containerRef} className="text-fg-subtle">
      <p className="whitespace-pre-wrap break-words leading-relaxed">{preview}</p>
      {truncated && (
        <p className="mt-2 text-xs text-fg-muted">
          …（{text.length - PREVIEW_CHARS} 字未显示，滚动到此处展开完整内容）
        </p>
      )}
    </div>
  )
}

/** assistant 消息的 parts 配置（模块作用域常量）。 */
const ASSISTANT_PARTS_COMPONENTS = {
  Text: AssistantText,
  Reasoning: Reasoning,
} as const

// ───────── 消息外壳 ─────────

// 时间戳 / 阶段文案 / 静默判定等纯逻辑在 @/lib/chat-run-status.ts（含单测）。

/** 每秒重渲染以刷新已耗时读数（长任务 liveness）。 */
function useElapsed(since: number | undefined, active: boolean): number {
  const [, tick] = useState(0)
  useEffect(() => {
    if (!active || since == null) return
    const t = window.setInterval(() => tick((n) => n + 1), 1_000)
    return () => window.clearInterval(t)
  }, [since, active])
  if (since == null) return 0
  return elapsedSeconds(since, Date.now())
}

/** 空内容 assistant 消息的运行指示（发送即显示，替代空白气泡）。
 *  spinner + 阶段文案 + 已耗时秒数，业界通用做法（ChatGPT/Claude 同款）。 */
function RunningIndicator() {
  const { runStatus } = useChatRunStatus()
  const createdAt = useAuiState((s) => s.message.createdAt)
  const since = createdAt instanceof Date ? createdAt.getTime() : undefined
  const elapsed = useElapsed(since, true)
  return (
    <div className="flex min-w-[140px] items-center gap-2 rounded-lg bg-bg-elevated px-3.5 py-2.5 text-sm text-fg-subtle">
      <Loader2 size={14} className="animate-spin text-accent" />
      <span>{runPhaseText(runStatus.phase, runStatus.toolName)}</span>
      {elapsed > ELAPSED_VISIBLE_AFTER_S && (
        <span className="text-xs text-fg-muted">已 {elapsed} 秒</span>
      )}
    </div>
  )
}

/** 运行中的常驻耗时提示（挂在有内容的消息下方）。
 *  从消息创建时刻持续计时，任务结束（isRunning=false）即消失——
 *  ChatGPT/Claude 同款：不做“静默检测”，运行期间始终可见。
 *  （此前的 StalledHint 按静默阈值出现/消失，有新 chunk 就闪没，读数忽有忽无。） */
function StalledHint() {
  const { runStatus } = useChatRunStatus()
  const createdAt = useAuiState((s) => s.message.createdAt)
  const since = createdAt instanceof Date ? createdAt.getTime() : undefined
  const elapsed = useElapsed(since, true)
  return (
    <div className="flex items-center gap-1.5 text-xs text-fg-muted">
      <span className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-accent" />
      {runPhaseText(runStatus.phase, runStatus.toolName)} 已 {elapsed} 秒
    </div>
  )
}

function UserMessage() {
  const createdAt = useAuiState((s) => s.message.createdAt)
  return (
    <MessagePrimitive.Root className="group flex flex-row-reverse gap-3 py-3">
      <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-bg-hover text-fg">
        <User size={15} />
      </div>
      {/* 气泡宽=容器宽-头像列(40px),右缘与 Composer 输入框右缘对齐 */}
      <div className="flex min-w-0 max-w-[calc(100%-2.5rem)] flex-col items-end gap-1 overflow-hidden">
        {/* 编辑态：行内 textarea + 保存/取消 */}
        <ComposerPrimitive.If editing>
          <UserEditComposer />
        </ComposerPrimitive.If>
        {/* 非编辑态：原消息气泡 */}
        <ComposerPrimitive.If editing={false}>
          <div className="min-w-0 overflow-hidden rounded-lg bg-accent px-3.5 py-2.5 text-accent-fg">
            <MessagePrimitive.Parts components={USER_PARTS_COMPONENTS} />
          </div>
        </ComposerPrimitive.If>
        {/* 发送时间戳与操作图标同行：时间戳常显，复制/编辑 hover 浮现 */}
        <UserActionBar createdAt={createdAt instanceof Date ? createdAt : undefined} />
      </div>
    </MessagePrimitive.Root>
  )
}

function AssistantMessage() {
  // 无任何 part（发送后、首 chunk 前的空占位）→ 显示运行指示而非空气泡。
  // 注意含 tool-call part：模型直接调工具（无前导文本）时 content 只有
  // tool-call part，也视为“有内容”——这样工具执行期间的 StalledHint
  // （挂在 hasContent 分支下）才能显示。
  const hasContent = useAuiState((s) => s.message.content.length > 0)
  const isRunning = useAuiState((s) => s.message.status?.type === "running")
  const isIncompleteError = useAuiState(
    (s) => s.message.status?.type === "incomplete" && s.message.status.reason === "error",
  )
  const errorText = useAuiState((s): string => {
    const st = s.message.status
    if (st?.type === "incomplete" && st.reason === "error") {
      const e = st.error ?? "出错了"
      return typeof e === "string" ? e : JSON.stringify(e)
    }
    return ""
  })
  return (
    <MessagePrimitive.Root className="group flex gap-3 py-3">
      <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-accent/15 text-accent">
        <Bot size={15} />
      </div>
      {/* 气泡宽=容器宽-头像列(40px),左缘与 Composer 输入框左缘对齐 */}
      <div className="flex min-w-0 max-w-[calc(100%-2.5rem)] flex-col items-start gap-1 overflow-hidden">
        {hasContent ? (
          <>
            <div className="min-w-0 overflow-hidden rounded-lg bg-bg-elevated px-3.5 py-2.5">
              <MessagePrimitive.Parts components={ASSISTANT_PARTS_COMPONENTS} />
            </div>
            {/* 流式中但静默超阈值：显示「仍在处理」提示，避免长任务看起来卡死 */}
            {isRunning && <StalledHint />}
          </>
        ) : (
          <RunningIndicator />
        )}
        {/* 消息级错误横幅：后端流 Err / stream init failed 落 status=error，
            此前无任何渲染分支——用户只看到“不动了”，错误只在导出时才可见。 */}
        {isIncompleteError && errorText && (
          <div className="w-full rounded-md border border-danger/40 bg-danger/5 px-2.5 py-1.5 text-xs text-danger">
            ⚠️ {errorText}
          </div>
        )}
        <AssistantActionBar />
      </div>
    </MessagePrimitive.Root>
  )
}

// ───────── 消息操作栏（ActionBar） ─────────
// autohide="not-last" + autohideFloat="always"：除最后一条外平时隐藏，悬停浮现。
// MessagePrimitive.Root 的 group class 驱动 group-hover 透明度过渡。

/** 复制按钮：复制消息文本，点击后 2s 内显示勾号反馈。 */
function CopyButton() {
  return (
    <ActionBarPrimitive.Copy copiedDuration={2000} className="group/copy rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-fg" title="复制">
      <Copy size={13} className="group-data-[copied]/copy:hidden" />
      <Check size={13} className="hidden text-accent group-data-[copied]/copy:block" />
    </ActionBarPrimitive.Copy>
  )
}

/** 导出为 Markdown 文件按钮。
 *  走原生保存对话框（save-file.ts，与单条消息导出同一路径，§13）。 */
function ExportButton() {
  const handleExport = async (content: string) => {
    await saveTextFile(content, `message-${Date.now()}.md`, [
      { name: "Markdown", extensions: ["md"] },
    ])
  }
  return (
    <ActionBarPrimitive.ExportMarkdown
      onExport={handleExport}
      className="rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-fg"
      title="导出 Markdown"
    >
      <FileDown size={13} />
    </ActionBarPrimitive.ExportMarkdown>
  )
}

/** 重新生成按钮（仅 assistant）。运行时自动禁用。 */
function ReloadButton() {
  return (
    <ActionBarPrimitive.Reload className="rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-fg" title="重新生成">
      <RotateCcw size={13} />
    </ActionBarPrimitive.Reload>
  )
}

/** 编辑按钮（仅 user）。编辑中自动禁用。 */
function EditButton() {
  return (
    <ActionBarPrimitive.Edit className="rounded p-1 text-fg-subtle hover:bg-bg-hover hover:text-fg" title="编辑">
      <Pencil size={13} />
    </ActionBarPrimitive.Edit>
  )
}

/** assistant 消息操作栏：复制 + 导出 + 重新生成。 */
function AssistantActionBar() {
  return (
    <ActionBarPrimitive.Root
      hideWhenRunning
      autohide="not-last"
      autohideFloat="always"
      className="flex items-center gap-0.5 opacity-100 transition-opacity data-[floating]:opacity-0 data-[floating]:group-hover:opacity-100"
    >
      <CopyButton />
      <ExportButton />
      <ReloadButton />
    </ActionBarPrimitive.Root>
  )
}

/** user 消息操作栏：时间戳 + 复制 + 编辑（同一行）。
 *  时间戳常显；复制/编辑由 autohide 控制（非末条消息 hover 浮现，末条常显）。 */
function UserActionBar({ createdAt }: { createdAt?: Date }) {
  return (
    <div className="flex items-center gap-1">
      {createdAt && (
        <span className="mr-1 text-[11px] leading-none text-fg-muted">
          {formatMessageTime(createdAt)}
        </span>
      )}
      <ActionBarPrimitive.Root
        hideWhenRunning
        autohide="not-last"
        autohideFloat="always"
        className="flex items-center gap-0.5 opacity-100 transition-opacity data-[floating]:opacity-0 data-[floating]:group-hover:opacity-100"
      >
        <CopyButton />
        <EditButton />
      </ActionBarPrimitive.Root>
    </div>
  )
}

/** user 消息行内编辑器：textarea + 保存/取消。
 *  ComposerPrimitive.Input 在 Message 内部会自动绑定到 edit composer runtime，
 *  提交时 assistant-ui 调 onEdit(AppendMessage)。 */
function UserEditComposer() {
  return (
    <ComposerPrimitive.Root className="w-full rounded-lg border border-border bg-bg-elevated px-3 py-2 transition-[border-color] duration-150 focus-within:border-accent focus-within:shadow-[0_0_0_2px_var(--accent-bg,rgba(30,64,175,0.08))]">
      <ComposerPrimitive.Input
        asChild
        autoFocus
        className="max-h-[200px] min-h-[60px] w-full resize-none bg-transparent text-sm outline-none"
      >
        <textarea />
      </ComposerPrimitive.Input>
      <div className="mt-2 flex justify-end gap-2">
        <ComposerPrimitive.Cancel className="rounded-md border border-border px-3 py-1 text-xs text-fg-muted hover:bg-bg-hover">
          取消
        </ComposerPrimitive.Cancel>
        <ComposerPrimitive.Send className="rounded-md bg-accent px-3 py-1 text-xs text-accent-fg hover:opacity-90">
          保存并发送
        </ComposerPrimitive.Send>
      </div>
    </ComposerPrimitive.Root>
  )
}

/** reasoning 可折叠块：流式时展开、结束自动折叠（assistant-ui 原生行为）。 */
function Reasoning() {
  return (
    <details
      className="mb-2 rounded-md border border-border bg-bg px-2.5 py-1.5 text-xs text-fg-muted"
      open
    >
      <summary className="flex cursor-pointer items-center gap-1.5 select-none">
        <Brain size={12} className="text-accent" />
        <span>思考过程</span>
        <MessagePartPrimitive.InProgress>
          <Loader2 size={10} className="animate-spin text-accent" />
        </MessagePartPrimitive.InProgress>
      </summary>
      <div className="mt-1.5 whitespace-pre-wrap text-fg-subtle">
        <MessagePartPrimitive.Text />
      </div>
    </details>
  )
}
