// Hooks 层：useChat.ts（§12.2 / §14.1 / §15 状态机）
// 对话状态机：send / stream / stop / retry。
// 流式 chunk 经 Channel 回调，增量 patch 消息缓存（乐观更新，§14.1 性能要点）。

import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useCallback, useEffect, useRef } from "react"
import { ipc, isCancelled } from "@/lib/ipc/commands"
import { useGenerateTitle, useRenameConversation } from "@/hooks/useConversations"
import { deriveFallbackTitle } from "@/lib/title-fallback"
import type { ChatStreamChunk, MessageRow, SendMessageInput } from "@/lib/domain"
import { useRunStatusStore } from "@/stores/run-status-store"

const QK_MESSAGES = (id: string) => ["messages", id] as const
const QK_CONVERSATIONS = ["conversations"] as const

/** 流式 chunk 合并器：同一帧内多次 setQueryData 合并为一次提交。
 *
 * 流式 delta 到达频率远高于显示刷新率（~500/s vs 60fps），逐个 setQueryData
 * 会让 React 每帧提交几十次状态更新，重渲染堆积卡顿。本 hook 把更新排队到
 * requestAnimationFrame 回调里批量执行——React 19 自动批处理同一回调内的
 * 多次 setQueryData → 每次刷新最多一次重渲染。后台标签页 rAF 自动暂停，
 * 省电同时不产生堆积；onSuccess invalidate / done chunk 会兑底刷新。
 */
function useChunkCoalescer() {
  const pendingRef = useRef<Array<() => void>>([])
  const rafRef = useRef<number | null>(null)

  const schedule = useCallback((fn: () => void) => {
    pendingRef.current.push(fn)
    if (rafRef.current !== null) return
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null
      const fns = pendingRef.current
      pendingRef.current = []
      for (const f of fns) f()
    })
  }, [])

  /** 立即应用所有挂起更新（done/error/stop 前调用，保证终态不被挂起 patch 覆盖）。 */
  const flush = useCallback(() => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current)
      rafRef.current = null
    }
    if (pendingRef.current.length === 0) return
    const fns = pendingRef.current
    pendingRef.current = []
    for (const f of fns) f()
  }, [])

  // 卸载时取消挂起的 rAF，丢弃未应用更新（组件销毁后不再需要）
  useEffect(() => {
    return () => {
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current)
      pendingRef.current = []
    }
  }, [])

  return { schedule, flush }
}

/** 发送消息时携带的上下文（挂载文档路径 / 图片）。 */
export interface SendContext {
  /** 挂载文档的路径列表（`@` 挂载，后端按 path 从 documents 表读全文）。 */
  mounted_paths: SendMessageInput["mounted_paths"]
  /** 图片上下文（base64 + mime，走 VLM）。 */
  context_images: SendMessageInput["context_images"]
}

/** 发送消息的变量（content + 上下文）。 */
export interface SendVariables {
  content: string
  ctx?: SendContext
  /** 是否开启深度思考（reasoning）。默认 false。 */
  enableReasoning?: boolean
}

/** 活跃 run 的会话 id 集合（模块级单例）。
 *  用途：useMessages refetch 保护——turn 是「整轮结束后才落库」，
 *  运行中切走再切回时 refetch 会从 DB 拉到空列表，把流式中的消息缓存
 *  清空（表现为"正在运行的会话不见了"）。有活跃 run 时跳过该清空。 */
export const activeRunConversations = new Set<string>()

/** 流式中消息 ID → 当前累积文本的快速引用（避免反复读缓存）。 */
function useStreamingBuffer() {
  const buf = useRef<Map<string, string>>(new Map())
  return buf
}

/** 流式中消息 ID → 当前累积 reasoning 的快速引用。 */
function useReasoningBuffer() {
  const buf = useRef<Map<string, string>>(new Map())
  return buf
}

export function useChat(conversationId: string | null) {
  const qc = useQueryClient()
  const streamingBuf = useStreamingBuffer()
  const reasoningBuf = useReasoningBuffer()
  const abortRef = useRef(false)
  const { schedule: schedulePatch, flush: flushPatches } = useChunkCoalescer()
  // 本轮发送的用户文本（供 onSuccess 时 LLM 标题生成失败的降级兑底用）
  const lastSentContentRef = useRef<string>("")
  // 本轮是否为该会话的首条消息（发送前判定，供 onSuccess 自动命名用）
  const isFirstTurnRef = useRef<boolean>(false)
  // 标题生成 / 改名（自动命名用，§20.5）
  const generateTitle = useGenerateTitle()
  const renameConv = useRenameConversation()
  // 工具调用不再产出 UI（卡片已下线）；tool_call chunk 仅用于建立消息行/维持 liveness。
  // 长任务 liveness 状态：
  // 存全局 zustand store（run-status-store）：useChat 在 Composer（发送）与
  // ChatRuntimeProvider（渲染）各持一个 hook 实例，hook 内 state 实例隔离互不可见
  // （安装自测实测：发送侧 touch 不生效到渲染侧）。store 单例跨实例共享。
  const touchRunStatus = useRunStatusStore((s) => s.touch)
  const setRunStatusIdle = useRunStatusStore((s) => s.reset)
  // 本轮发送的 assistant 临时占位 id（发送即插入，首个真实 chunk 到达时被替换）
  const tempAssistantIdRef = useRef<string | null>(null)

  /** 增量 patch 单条消息（基于缓存函数式更新）。 */
  const patchMessage = useCallback(
    (msgId: string, updater: (m: MessageRow) => MessageRow) => {
      if (!conversationId) return
      qc.setQueryData<MessageRow[]>(QK_MESSAGES(conversationId), (old) =>
        old?.map((m) => (m.id === msgId ? updater(m) : m)),
      )
    },
    [qc, conversationId],
  )

  /** 若缓存里不存在该 id 的消息，追加一条占位（流式首个 chunk 到达时用）。
   *  后端 turn 结束才落库，流式过程中 DB 里没有 assistant 消息，
   *  前端必须自己建占位才能让逐 delta 的 patch 生效（否则全部落空）。
   *  首个真实 chunk 到达时，同时移除发送时插入的临时 assistant 占位（temp-assistant-*）。 */
  const ensureMessage = useCallback(
    (msg: MessageRow) => {
      if (!conversationId) return
      qc.setQueryData<MessageRow[]>(QK_MESSAGES(conversationId), (old) => {
        if (old?.some((m) => m.id === msg.id)) return old
        const tempId = tempAssistantIdRef.current
        const rest = (old ?? []).filter((m) => m.id !== tempId)
        if (tempId) tempAssistantIdRef.current = null
        return [...rest, msg]
      })
    },
    [qc, conversationId],
  )

  /** 处理一个流式 chunk。 */
  const handleChunk = useCallback(
    (chunk: ChatStreamChunk) => {
      const { message_id, kind, text, tool_call } = chunk
      if (kind === "text_delta") {
        touchRunStatus("streaming")
        const prev = streamingBuf.current.get(message_id) ?? ""
        const next = prev + text
        streamingBuf.current.set(message_id, next)
        // 排队到下一帧批量提交（避免每 chunk 一次 setQueryData）
        schedulePatch(() => {
          ensureMessage({
            id: message_id,
            conversation_id: conversationId ?? "",
            role: "assistant",
            status: "streaming",
            content: next,
            reasoning: null,
            error: null,
            model: null,
            created_at: Date.now(),
            updated_at: Date.now(),
            prompt_tokens: undefined,
            completion_tokens: undefined,
            total_tokens: undefined,
          })
          patchMessage(message_id, (m) => ({ ...m, content: next, status: "streaming" }))
        })
      } else if (kind === "reasoning_delta") {
        touchRunStatus("streaming")
        // 思考链独立累积到 reasoning，不混入 content
        const prev = reasoningBuf.current.get(message_id) ?? ""
        const next = prev + text
        reasoningBuf.current.set(message_id, next)
        schedulePatch(() => {
          ensureMessage({
            id: message_id,
            conversation_id: conversationId ?? "",
            role: "assistant",
            status: "streaming",
            content: "",
            reasoning: next,
            error: null,
            model: null,
            created_at: Date.now(),
            updated_at: Date.now(),
            prompt_tokens: undefined,
            completion_tokens: undefined,
            total_tokens: undefined,
          })
          patchMessage(message_id, (m) => ({ ...m, reasoning: next, status: "streaming" }))
        })
      } else if (kind === "tool_call_start" && tool_call) {
        touchRunStatus("tool", tool_call.name)
        // 首个 chunk 可能就是工具调用（模型直接调工具、无前导文本/reasoning）：
        // 与 text_delta 同样需要 ensureMessage 建立后端真实 id 的消息行
        // （并移除 temp-assistant 占位），否则渲染中消息 id 对不上，
        // 后续 delta 与 done 的 patch 全部落空。
        schedulePatch(() => {
          ensureMessage({
            id: message_id,
            conversation_id: conversationId ?? "",
            role: "assistant",
            status: "streaming",
            content: "",
            reasoning: null,
            error: null,
            model: null,
            created_at: Date.now(),
            updated_at: Date.now(),
            prompt_tokens: undefined,
            completion_tokens: undefined,
            total_tokens: undefined,
          })
        })
      } else if (kind === "tool_call_result" && tool_call) {
        // 工具结果到达：仅维持 liveness（无 UI 状态可回填）
        touchRunStatus("streaming")
      } else if (kind === "done") {
        setRunStatusIdle()
        streamingBuf.current.delete(message_id)
        reasoningBuf.current.delete(message_id)
        // 先应用挂起的 delta 再标 complete，避免终态被挂起 patch 覆盖
        flushPatches()
        patchMessage(message_id, (m) => ({ ...m, status: "complete" }))
      } else if (kind === "error") {
        setRunStatusIdle()
        streamingBuf.current.delete(message_id)
        reasoningBuf.current.delete(message_id)
        flushPatches()
        patchMessage(message_id, (m) => ({
          ...m,
          status: "error",
          error: text || "流式出错",
        }))
      }
    },
    [patchMessage, ensureMessage, streamingBuf, reasoningBuf, conversationId, touchRunStatus, setRunStatusIdle, schedulePatch, flushPatches],
  )

  const sendMutation = useMutation({
    mutationFn: async ({ content, ctx, enableReasoning }: SendVariables) => {
      if (!conversationId) throw new Error("未选择会话")
      activeRunConversations.add(conversationId)
      abortRef.current = false
      streamingBuf.current.clear()
      reasoningBuf.current.clear()
      // 新一轮 turn：清掉上一轮的工具调用（运行时态不落库，残留会串到新消息渲染）
      lastSentContentRef.current = content
      // 进入 thinking 阶段：发送后尚未收到任何输出（长任务 liveness 起点）
      touchRunStatus("thinking")
      // 记录本轮是否为首条消息（发送前的会话状态）供 onSuccess 自动命名判定
      const convsBefore = qc.getQueryData<{ id: string; title: string; message_count: number }[]>(
        QK_CONVERSATIONS,
      )
      const convBefore = convsBefore?.find((c) => c.id === conversationId)
      isFirstTurnRef.current =
        !convBefore || (convBefore.title === "新会话" && convBefore.message_count === 0)
      const input: SendMessageInput = {
        conversation_id: conversationId,
        content,
        context_images: ctx?.context_images ?? [],
        mounted_paths: ctx?.mounted_paths ?? [],
        enable_reasoning: enableReasoning ?? false,
      }
      // 乐观插入 user 占位（前端临时 id）：让用户立刻看到自己发的消息。
      // turn 结束后端落库真实 user 消息（真实 id），invalidate 后临时占位被替换。
      const tempUserId = `temp-user-${Date.now()}`
      // 同时插入 assistant 占位（空内容 + streaming）：发送即显示「正在思考…」气泡。
      // 否则长任务（模型思考 / 慢工具）期间首个 chunk 迟迟不到，用户消息后一片空白，
      // 用户无法区分「在跑」还是「卡死」（业界通用做法，ChatGPT/Claude 同款）。
      // 首个真实 chunk 到达时由 ensureMessage 用后端 message_id 替换。
      const tempAssistantId = `temp-assistant-${Date.now()}`
      tempAssistantIdRef.current = tempAssistantId
      const now = Date.now()
      qc.setQueryData<MessageRow[]>(QK_MESSAGES(conversationId), (old) => [
        ...(old ?? []),
        {
          id: tempUserId,
          conversation_id: conversationId,
          role: "user",
          status: "complete",
          content,
          reasoning: null,
          error: null,
          model: null,
          created_at: now,
          updated_at: now,
          prompt_tokens: undefined,
          completion_tokens: undefined,
          total_tokens: undefined,
        },
        {
          id: tempAssistantId,
          conversation_id: conversationId,
          role: "assistant",
          status: "streaming",
          content: "",
          reasoning: null,
          error: null,
          model: null,
          created_at: now,
          updated_at: now,
          prompt_tokens: undefined,
          completion_tokens: undefined,
          total_tokens: undefined,
        },
      ])
      // assistant 占位由首个流式 delta 到达时按后端 message_id 创建（见 handleChunk）
      const promise = ipc.sendMessage(input, handleChunk)
      return promise
    },
    onSuccess: () => {
      tempAssistantIdRef.current = null
      // 流式结束后，assistant 消息已由 done chunk 收尾；最终 invalidate 确保一致
      if (conversationId) activeRunConversations.delete(conversationId)
      if (conversationId) {
        qc.invalidateQueries({ queryKey: QK_MESSAGES(conversationId) })
        qc.invalidateQueries({ queryKey: QK_CONVERSATIONS })

        // 自动命名：首条消息且标题仍为默认 “新会话” 时，调 LLM 概括生成标题。
        // 主路径：后端 generate_conversation_title（语言跟随输入，≤8词）。
        // 降级：LLM 调用失败时用截断式 deriveFallbackTitle（不阻断主流程）。
        if (isFirstTurnRef.current) {
          isFirstTurnRef.current = false
          const convId = conversationId
          const fallbackContent = lastSentContentRef.current
          generateTitle.mutate(convId, {
            onError: (e) => {
              // LLM 标题生成失败：降级截断式
              console.warn("[useChat] auto title gen failed, fallback:", e)
              const title = deriveFallbackTitle(fallbackContent)
              if (title && title !== "新会话") {
                renameConv.mutate({ id: convId, title })
              }
            },
          })
        }
      }
    },
    onError: (e) => {
      if (conversationId) activeRunConversations.delete(conversationId)
      setRunStatusIdle()
      tempAssistantIdRef.current = null
      // 先应用挂起的 delta 再标 error，避免挂起 patch 覆盖终态
      flushPatches()
      // Cancelled 静默；其他错误标到 assistant 占位（若已创建）
      if (isCancelled(e)) {
        if (conversationId) qc.invalidateQueries({ queryKey: QK_MESSAGES(conversationId) })
        return
      }
      const msg = e instanceof Error ? e.message : String(e)
      // 把 streaming 中的消息标为 error
      streamingBuf.current.forEach((_v, id) => {
        patchMessage(id, (m) => ({ ...m, status: "error", error: msg }))
      })
      streamingBuf.current.clear()
      // 流都没启动（后端 stream init failed 已落 user + error assistant）或
      // 中途异常：invalidate 拉后端真实消息，替换临时 user 占位 + 显示 error assistant
      if (conversationId) {
        qc.invalidateQueries({ queryKey: QK_MESSAGES(conversationId) })
      }
    },
  })

  /** 中断当前流式：调 cancel_stream 命令发取消信号 → 后端 select! 退出 → 落库 Cancelled。
   *  前端同步把 streaming 消息标 cancelled（保留 partial，§15）。 */
  const stop = useCallback(async () => {
    abortRef.current = true
    setRunStatusIdle()
    // 先应用挂起的流式 delta 再标 cancelled，避免挂起 patch 覆盖终态
    flushPatches()
    if (conversationId) {
      const msgs = qc.getQueryData<MessageRow[]>(QK_MESSAGES(conversationId))
      msgs?.forEach((m) => {
        if (m.status === "streaming") {
          patchMessage(m.id, (mm) => ({ ...mm, status: "cancelled" }))
          // 后端取消信号（oneshot）→ send_message 流循环退出
          ipc.cancelStream({ messageId: m.id }).catch((e) =>
            console.warn("[useChat] cancel_stream failed:", e),
          )
        }
      })
    }
  }, [conversationId, qc, patchMessage, setRunStatusIdle, flushPatches])

  /** 重新生成指定 assistant 回复。
   *
   * assistant-ui onReload(parentId) 的 parentId 是该 assistant 的前序 user 消息 id。
   * 策略：截断删除该 user 及其之后所有消息（含目标 assistant），再用原 user 文本重发。
   * 这样历史变为「…原 user（重建）, 新 assistant」，语义等价于「重发这条 user」。
   *
   * 限制：原 user 携带的图片上下文无法从 MessageRow 恢复，reload 仅重发纯文本 +
   * 当前会话挂载状态。多数 reload 场景不涉及图片，可接受。
   *
   * @param parentMessageId 前序 user 消息 id（onReload 的 parentId）
   */
  const reload = useCallback(
    async (parentMessageId: string) => {
      if (!conversationId) return
      const msgs = qc.getQueryData<MessageRow[]>(QK_MESSAGES(conversationId)) ?? []
      const parentUser = msgs.find((m) => m.id === parentMessageId)
      if (!parentUser || parentUser.role !== "user") {
        console.warn("[useChat] reload: parent user message not found", parentMessageId)
        return
      }
      const content = parentUser.content
      if (!content || !content.trim()) return
      // 截断：删该 user 及其后所有消息（单事务，后端原子完成）
      await ipc.deleteMessageAndAfter(parentMessageId)
      // invalidate 拉取截断后的历史（清除被删消息的前端缓存）
      qc.invalidateQueries({ queryKey: QK_MESSAGES(conversationId) })
      // 重发原文本（上下文无法恢复，用空；reasoning 沿用发送态 ref 不可得，默认 false）
      await sendMutation.mutateAsync({ content, ctx: { mounted_paths: [], context_images: [] } })
    },
    [conversationId, qc],
  )

  /** 编辑指定 user 消息后重发。
   *
   * assistant-ui onEdit(message) 的 message.parentId 是被编辑 user 消息自身 id。
   * 策略：截断删除该 user 及其之后所有消息，用编辑后文本重发。
   *
   * @param messageId 被编辑的 user 消息 id（AppendMessage.parentId）
   * @param newContent 编辑后的文本（从 AppendMessage.content parts 提取）
   */
  const editAndResend = useCallback(
    async (messageId: string, newContent: string) => {
      if (!conversationId) return
      const content = newContent.trim()
      if (!content) return
      await ipc.deleteMessageAndAfter(messageId)
      qc.invalidateQueries({ queryKey: QK_MESSAGES(conversationId) })
      await sendMutation.mutateAsync({ content, ctx: { mounted_paths: [], context_images: [] } })
    },
    [conversationId, qc],
  )

  return {
    send: sendMutation.mutate,
    sendAsync: sendMutation.mutateAsync,
    isSending: sendMutation.isPending,
    stop,
    reload,
    editAndResend,
  }
}
