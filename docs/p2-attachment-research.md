# P2 调研：ingest 产物与会话的关联策略

> **状态**：本调研的「持久化 + RAG 检索」方向已在二期 A2 落地（见 PROGRESS.md「A2 RAG 检索增强」/ ARCHITECTURE.md 决策 15）。ingest 产物现自动切片+向量化存 SQLite，对话时 KNN 检索注入。本文件保留作设计参考。
>
> 背景：当前已实现"按会话 + 选中"的 ingest 产物管理（P0）。本调研面向 P2——
> 探讨 ingest 产物（文件/文档/图片）与对话更成熟的关联、持久化与检索策略，
> 参考开源成熟产品的设计模式。

## 一、当前架构（P0 落地后）

```
ingest(文件) ──► IngestedDoc { text, charCount } ──► ingest-store.ingestedByConv[convId]
                                                          │
                                                          ▼ selected=true
                                                    send_message(context_texts)
                                                          │
                                                          ▼
                                          text_prompt_with_context() 拼前缀
```

- 产物按会话分组，每条带 `selected` 布尔
- 发消息时只带 `selected` 的文档文本（拼成"参考文档"前缀）
- 图片走独立通道（Composer 读 base64 → context_images → VLM）
- **产物不持久化**（刷新即丢），**不参与检索**（全文塞进 prompt）

## 二、成熟产品的设计模式

### 1. ChatGPT（2025 文件上传）
- **作用域**：per-chat，最多 20 个文件 / 会话
- **持久化**：文件存云端，会话内持久
- **使用方式**：文件作为 "bubbles" 展示在 composer 上方，可删
- **检索**：Advanced Data Analysis（Code Interpreter）按需读取，非全量塞 prompt
- **参考**：[datastudios 对比](https://www.datastudios.org/post/chatgpt-vs-claude-for-file-upload-reading-capabilities-full-comparison-and-report-models-support-uses-cases-pricing)

### 2. Claude（Anthropic）
- **作用域**：per-message（每次发送时选择本次携带的附件）
- **持久化**：附件随消息持久
- **使用方式**：composer 上方展示附件 chips，强调"本次上下文"
- **设计哲学**：composer 是"上下文空间"，附件是核心输入
- **参考**：[Claude vs ChatGPT Composer Design](https://aiuxplayground.substack.com/p/claude-vs-chatgpt-a-deep-dive-into)

### 3. LobeChat（开源，最成熟）
- **三层消息架构**（RFC 142）：
  - DB Message（持久化）
  - UI Message（渲染）
  - LLM Message（发给模型）
  - **分离关注点，附件在不同层有不同表示**
- **附件解析**：`resolveAttachmentsByFileIds` → 统一 ingestion 路径
  - 检测图片/视频 → 启用多模态
  - 文件转 base64 / URL 两种形态
- **上下文工程**：`contextEngineering.ts` 统一管理 history + attachments + memories
- **参考**：[lobe-chat File Management](https://deepwiki.com/lobehub/lobe-chat/5.1-file-and-document-management)、[RFC 142](https://github.com/lobehub/lobehub/discussions/9888)

### 4. Chatbox（开源）
- **Context Management**：控制哪些历史消息 + 附件进入 API 请求
- **核心权衡**：上下文深度 vs token 限制 vs 响应效率
- **按消息粒度**：附件绑定到具体消息，历史附件可选包含
- **参考**：[chatbox Context Management](https://deepwiki.com/chatboxai/chatbox/6.5-context-management)

### 5. LibreChat（开源）
- **File Management + 向量库**：文件上传 → 向量化 → 语义检索
- **作用域**：跨会话可复用（文件库概念）
- **检索**：RAG，按相关性取片段，非全量
- **参考**：[LibreChat File Management](https://deepwiki.com/intelequia/LibreChat/4.4-file-management)

## 三、关键设计维度对比

| 维度 | ChatGPT | Claude | LobeChat | Chatbox | LibreChat | **onto-studio 现状** |
|---|---|---|---|---|---|---|
| 附件作用域 | per-chat | per-message | per-message(可复用) | per-message | per-user(库) | **per-chat** |
| 持久化 | 云端 | 随消息 | DB | 本地 | DB+向量库 | **无（内存）** |
| 检索方式 | Code Interpreter | 全量塞 | 全量/按需 | 全量 | **RAG 语义检索** | **全量塞前缀** |
| 多模态 | ✓ | ✓ | ✓ | 部分 | ✓ | ✓(一期) |
| 文件库复用 | ✗ | ✗ | 部分 | ✗ | **✓** | ✗ |

## 四、对 onto-studio 的建议（分阶段）

### P2.1（近期，一期收尾）：持久化 + per-message 附件记录
**问题**：当前 ingested 不持久化，刷新丢失；且无法回溯"哪条消息用了哪些文件"。

**建议**：
1. **ingested 落 SQLite**：复用 memory crate，新增 `attachments` 表
   - `id, conversation_id, message_id(可空), file_name, format, char_count, text, created_at`
   - 摄取完成即落库，前端从 DB 读，不再纯内存
2. **附件绑定到消息**：发送消息时记录 `message_attachments` 关联
   - user 消息可查"本次引用了哪些文档"
   - 为未来 Citation 渲染打基础（二期）
3. **保留 selected 语义**：selected 是 UI 态，落库的是"已发送"的事实

**参考**：LobeChat 的 DB Message 层 + Chatbox 的 per-message 绑定。

### P2.2（二期 RAG）：向量检索替代全量塞
**问题**：文档大时全量塞 prompt 会爆 token；多文档时模型抓不住重点。

**建议**（对齐 ARCHITECTURE.md 二期）：
1. **向量化**：ingest 产物切片 → sqlite-vec 存向量（已有基础设施）
2. **RAG 检索**：发消息时按 query 检索相关片段，只塞 top-k
3. **Citation**：回复中标注引用来源（对齐二期 Citation 渲染）

**参考**：LibreChat 的向量库 + 语义检索模式。

### P2.3（可选）：跨会话文件库
**问题**：同一文件在不同会话重复上传。

**建议**：
1. **文件库**：ingest 产物按内容 hash 去重，跨会话共享
2. **引用**：会话引用文件库的文件，而非各自持有副本
3. **管理 UI**：独立文件管理页

**参考**：LibreChat 的 per-user 文件库。**一期不建议做**，增加复杂度但 MVP 价值低。

## 五、推荐路线

```
P2.1（近期）  持久化 attachments 表 + per-message 绑定
    │
    │  ← 一期收尾，让"上传文件→提问"可追溯、刷新不丢
    ▼
二期 RAG      向量化 + 检索 + Citation（对齐 ARCHITECTURE.md 二期）
    │
    ▼
P2.3（可选）  跨会话文件库（按需）
```

**P2.1 是最该先做的**——它补齐了"持久化"这个一期缺口，且为二期 RAG/Citation 铺路。
当前 P0 的"按会话+选中"内存方案是 P2.1 的前置 UI 验证，数据结构已对齐（`ingestedByConv`
映射到 `attachments WHERE conversation_id=?`）。
