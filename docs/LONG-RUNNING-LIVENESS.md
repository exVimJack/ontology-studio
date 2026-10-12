# 长任务流式 Liveness 方案（防「看起来卡死」）

> 创建：2026-08。解决「长程任务（深度思考 / 慢工具调用 / 长时间无输出）期间流式界面停住，
> 用户无法区分『在跑』与『挂了』」的问题。对齐业界通用做法（ChatGPT / Claude / Gemini /
> Claude Code / Cursor）。

---

## 一、问题分析

### 1.1 现状（改造前）

1. **发送后到首个 chunk 之间完全空白**：assistant 消息占位由首个流式 delta 到达时才创建
   （`useChat.handleChunk → ensureMessage`）。深度思考模型（reasoning 模型）首 token 可达
   30~120s，期间用户消息后没有任何反馈，只有 Composer 的停止按钮有微弱暗示。
2. **静默期无进度感**：文本流到中途停下来（agent 多步工具调用间、模型长思考续写前），
   最后一段文字静止不动，无任何「仍在执行」的信号。
3. **工具调用只有 spinner**：`ToolFallback` 卡片有转圈图标但无耗时，慢工具
   （联邦查询 30s 超时、MCP 慢端点、read_document 大文档）看起来像卡死。
4. **无法区分「在忙」与「断流」**：前端没有任何通道感知后端/网络是否还活着。

### 1.2 业界通用做法（2026 调研）

| 产品 | 长任务反馈形态 |
|---|---|
| ChatGPT | 发送即显示 shimmer 占位气泡；思考期 "Thinking…" → 长时间后显示已耗时；工具卡片带 spinner + 耗时 |
| Claude (web) | 助手消息内嵌脉动文字流（"Thinking…"、"Searching the web…" 动态阶段文案） |
| Gemini | 底部 shimmer + "Generating response"，慢时转为已耗时 |
| Claude Code (CLI) | 每个工具调用 spinner + 实时已耗时秒数；esc 随时可中断 |
| Cursor | 状态栏阶段文案 + 已耗时 |

共性三要素：**① 立即出现的占位反馈；② 阶段化文案（thinking/tool/streaming）；③ 持续跳动的已耗时计数器**。

---

## 二、方案分层

```
层 0（已实现）前端占位 + 阶段文案 + 已耗时 + 静默提示   ← 纯前端，无后端改动
层 1（已实现）工具调用卡片耗时                          ← 纯前端
层 2（规划）  后端 heartbeat chunk                      ← 需改 Rust + bindings
层 3（规划）  断流检测与提示                            ← 依赖层 2
```

### 层 0：前端 liveness 指示（已落地）

**useChat.ts**：

- 发送时（`mutationFn`）乐观插入 `temp-assistant-*` 空占位（status=streaming），
  与既有 `temp-user-*` 同批；首个真实 chunk 到达时由 `ensureMessage` 用后端
  `message_id` 原位替换。
- 新增 `RunStatus { phase, toolName?, lastChunkAt }` 运行状态机：
  - `thinking`：发送后尚无任何输出
  - `streaming`：收到 text/reasoning delta 或 tool_result
  - `tool`：收到 tool_call_start（带工具名）
  - `idle`：done / error / stop / onError
- `lastChunkAt` 更新按 1s 节流（phase 变化则立即），避免逐 delta setState。

**ChatRuntime.tsx**：`ChatRunStatusContext` 把 `runStatus` / `isRunning` 下发给
assistant-ui 树内的 Thread 组件（hook 实例只有一份，状态才互通）。

**Thread.tsx**：

- `RunningIndicator`：空内容 assistant 占位的渲染体——spinner + 阶段文案
  （正在思考… / 正在执行 {tool}… / 正在生成回复…）+ 已耗时秒数（>3s 才显示，避免闪烁）。
- `StalledHint`：有内容且仍在 running、且距 `lastChunkAt` 静默超过 **4s** 时，
  在气泡下方显示脉动圆点 + 「{阶段文案} 已 {N} 秒」。
- `useElapsed(since, active)`：1s interval 的小 hook，驱动读数刷新。

**层 1（顺带）**：user 消息气泡下方显示发送时间戳（同日 `HH:mm`，跨日 `MM-DD HH:mm`）。

### 层 2（规划）：后端 heartbeat chunk

前端 `lastChunkAt` 只能证明「最近有过活动」，不能证明「现在还活着」。补后端心跳：

1. `crates/agent-core/src/chat.rs` 的 `StreamKind` 增加 `Heartbeat` 变体
   （bindings.ts 同步更新——specta 导出流程 `ONTO_GEN_BINDINGS=1`）。
2. `src-tauri/src/commands/chat.rs` 的流循环 `tokio::select!` 增加
   `tokio::time::interval(5s)` 分支：仅在「距上一真实 chunk > 5s」时推
   `Heartbeat` chunk（有真实输出时不推，省 IPC 噪声）。
   Heartbeat 不写 `content_buf`，前端收到后仅刷新 `lastChunkAt`（touchRunStatus）。
3. 取消信号分支语义不变。

> rig 的流本身不产心跳，所以在 `send_message` 的消费循环加 interval 是唯一挂点，
> 与取消信号的 select! 结构天然兼容。

### 层 3（规划）：断流检测

- 前端：`running` 状态下若 **15s 无任何 chunk（含 heartbeat）**，提示
  「连接可能已中断，可点击停止后重试」。
- 可选：后端 reqwest 流式 read 超时（`read_timeout`）主动断开并推 Error chunk，
  避免半开连接永久挂起。

---

## 三、边界与取舍

| 议题 | 决策 |
|---|---|
| 占位气泡误闪（首 chunk 极快到达） | 占位与真实消息同帧替换，且耗时 >3s 才显示秒数，视觉无闪烁 |
| temp-assistant 泄漏（无任何 chunk 的异常 turn） | `onSuccess`/`onError`/`stop` 均清 ref；invalidate 后以 DB 为准 |
| 心跳频率 | 5s：远小于用户焦虑阈值（~10s），IPC 开销可忽略 |
| 是否做进度百分比 | 不做。LLM 无法预知总步数，假进度条是反模式（业界共识） |
| 中断入口 | 既有 stop 按钮/esc 语义不变，长任务随时可取消 |
