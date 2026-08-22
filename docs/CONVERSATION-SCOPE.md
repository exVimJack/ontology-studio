# 会话级知识范围：文件夹层级 + 激活集（Conversation Scope）

> 本文件是 onto-studio「会话级知识/数据源范围控制」的设计文档。
> 架构总纲见 `ARCHITECTURE.md`；联邦查询见 `docs/PHASE3-FEDERATION.md`。
> 本文件不修订既有决策，仅新增「会话激活集」机制 + 「文件库文件夹层级」改造。
>
> 创建：2026-08-01

---

## 一、问题与目标

### 1.1 问题

当前知识库文件是**扁平全局池**（`documents` 表无层级），会话挂载文档走 `conversation_documents` 表按 path 关联。三个 UX 痛点：

1. **历史污染**：用户开新会话只想问个通用问题，或只想查 PG 数据库，但 `@` 菜单/挂载列表把所有历史文件都列出来，模型也可能搜到本次根本没想引用的旧文件，**上下文串味**
2. **无组织**：5 本书平铺一坨，无法按主题/项目分组（"曾国藩专题""方法论"），文件多了找不到
3. **数据源与知识库割裂**：数据源有独立管理入口（`/sources`），知识库没有等价的"逻辑子集"概念，用户心智不对称

### 1.2 目标

让用户能：
- **按文件夹组织知识库**（类似云盘/文件系统，支持嵌套），对齐豆包云盘、VSCode 文件树
- **会话级激活范围**：每次会话显式选择本次"可见"的文件夹/数据源，未激活的不参与检索
- **`@` 精确引用即激活**：`@某文件` 只激活该文件本身（不连带文件夹），作为激活集的快捷补充入口
- **会话内上传落 Inbox**：零摩擦暂存，持久化但不自动归类

### 1.3 范围边界

| 做 | 不做（留后续） |
|---|---|
| 文件夹层级（路径字符串，支持嵌套） | 跨账号共享/协作 |
| 会话激活集（文件夹 + 数据源 + 单文件） | 文件夹级权限/加密 |
| Inbox 默认落点 + 手动移动 | 自动归类（AI 推荐文件夹） |
| `@` 引用即激活 | 文件版本历史 |
| 工具按激活集过滤 | 标签（tag）第二维度 |

---

## 二、核心概念

### 2.1 文件夹层级（云盘模式）

放弃"Notebook 是独立实体"概念，采用**文件系统式层级文件夹**。`documents` 表加 `folder_path` 列，用路径字符串表示层级（对齐文件系统，比 parent_id 递归简单）：

```
知识库根 /
├─ 曾国藩专题/
│   ├─ 曾国藩唐浩明.epub        folder_path = "/曾国藩专题"
│   ├─ 曾国藩智慧精髓.pdf        folder_path = "/曾国藩专题"
│   └─ 书信集/                  （文件夹由其下文件隐式定义，无独立 folders 表）
│       └─ 曾氏家书.txt          folder_path = "/曾国藩专题/书信集"
├─ 方法论/
│   ├─ 观看之道.pdf              folder_path = "/方法论"
│   └─ 法律的故事.epub           folder_path = "/方法论"
└─ Inbox/
    └─ 临时上传.docx             folder_path = "/Inbox"
```

**设计要点**：
- **无独立 `folders` 表**：文件夹由文件的 `folder_path` 隐式定义（类似文件系统：空文件夹不存在）。简化 schema，避免文件夹与文件 orphan 不一致
- **路径字符串**：`/曾国藩专题/书信集`，以 `/` 分隔，根目录是 `/`。`folder_path IS NULL` 视为根目录散文件（兼容旧数据迁移）
- **支持嵌套**：层数无硬限制（路径字符串天然支持）
- **Inbox 是固定文件夹**：路径 `/Inbox`，会话内上传的默认落点，持久化但不自动归类

### 2.2 会话激活集（Conversation Scope）

每个会话有一份**激活集**，记录本次会话"可见"的知识范围：

```
激活集 = {
  folders: ["/曾国藩专题", "/方法论"],   // 文件夹路径（含子目录递归）
  documents: ["读通鉴论.epub 的 path"],  // 单文件（@ 触发，精确引用）
  sources: ["ontology", "mydb"],         // 数据源名（catalog 名）
}
```

**默认空**：新会话不预选任何范围。避免历史文件强制参与每次会话（解决 §1.2 痛点 1）。

**激活方式（两个入口，互补）**：
1. **"会话范围"chip**（对话页顶部）：弹出文件树 + 数据源列表，勾选文件夹/数据源。勾文件夹 = 该文件夹下所有文件（含子目录递归）对本次会话可见
2. **`@文件`**：在输入框 `@某文件` → 该文件**本身**自动加入激活集 `documents`（不连带文件夹，精确引用）

**激活集持久化**：存 `conversations` 表两列（`active_folders`/`active_sources` JSON）+ 复用现有 `conversation_documents` 表存 `documents` 部分。切回该会话恢复激活状态。

**激活集为空时的行为**：
- `stream_with_memory` **不挂** `document_tools` / `federation_tools`（避免模型瞎调返回空）
- 模型按通用能力回答
- chip 明示 `💬 未挂载知识源`（状态可见，避免用户误以为在查知识库）

### 2.3 `@` 引用语义（决策 17 延伸）

`@` 是**位置语义引用 token**，原有语义不变（`@fileName` 原位保留 + user message 尾部 `<mounted-documents>` 注脚）。本次新增：

- **`@` 候选范围**：**所有文件**（含 Inbox、未激活文件夹）+ 所有数据源表。不限于激活集内
- **`@` 触发激活**：用户 `@某文件` → 该文件 path 加入会话激活集 `documents` 部分（如果尚未在激活集里）→ 后续 `search_documents`/`read_document` 可见
- **`@` 不连带文件夹**：只激活该文件本身。要批量见整个文件夹，去 chip 勾选

这样 `@` 是"精确引用即挂载"入口，chip 是"批量挂载"入口，两者互补（对齐 Cursor `@Files` 范式）。

### 2.4 数据源激活

数据源（PG/MySQL 连接）独立顶层，无文件夹归属。激活方式同文件夹：
- chip 里勾选数据源 → 加入 `active_sources`
- `@sourceName.tableName` → 该数据源自动加入 `active_sources`

激活后 `list_data_sources`/`describe_table`/`execute_sql` 工具才可见该源。

---

## 三、数据模型

### 3.1 `documents` 表改动

```sql
-- 新增列：文件夹路径
ALTER TABLE documents ADD COLUMN folder_path TEXT;  -- NULL = 根目录散文件（兼容旧数据）

-- 索引：按文件夹列出文件
CREATE INDEX IF NOT EXISTS idx_documents_folder ON documents(folder_path);
```

`folder_path` 语义：
- `NULL` → 根目录散文件（旧数据迁移默认值）
- `"/Inbox"` → Inbox 暂存
- `"/曾国藩专题"` → 该文件夹下
- `"/曾国藩专题/书信集"` → 嵌套子文件夹

### 3.2 `conversations` 表改动

```sql
-- 新增列：激活集（JSON）
ALTER TABLE conversations ADD COLUMN active_folders TEXT;  -- JSON: ["/曾国藩专题", "/方法论"]
ALTER TABLE conversations ADD COLUMN active_sources TEXT;  -- JSON: ["ontology", "mydb"]
```

- `NULL` → 激活集为空（默认，兼容旧会话）
- 激活集的 `documents` 部分（`@` 触发的单文件）复用现有 `conversation_documents` 表，不新增列

### 3.3 现有数据迁移

**5 本书的 `folder_path`**：首次迁移设为 `/Inbox`（视为暂存，用户后续自行归类）。或保留 NULL（根目录散文件）——待定，倾向 `/Inbox`（语义清晰：未分类）。

**现有会话的 `active_*`**：NULL = 空激活集。旧会话回看时，历史消息的 `<mounted-documents>` 注脚仍按 `conversation_documents` 表恢复（向后兼容），但工具检索按空激活集（旧会话不再自动查全文，仅展示历史）。

### 3.4 文件夹操作（无独立 folders 表）

文件夹 CRUD 由文件移动隐式完成：
- **新建文件夹**：上传文件时指定新 `folder_path`（如 `/新专题`）→ 文件夹自动"存在"
- **重命名文件夹**：`UPDATE documents SET folder_path = replace(folder_path, '/旧名', '/新名') WHERE folder_path LIKE '/旧名%'`
- **删除文件夹**：批量删除该文件夹下所有文件（`folder_path LIKE '/文件夹名%'`），每个文件走 `deleteDocument`（清 documents 行 + FTS5 索引 + conversation_documents 关联）。文件被会话激活/引用也不拦截——删除后 path 外键失效，`conversation_documents` LEFT JOIN 取不到即跳过，不报错
- **移动文件**：`UPDATE documents SET folder_path = '/目标' WHERE path = ?`

---

## 四、Agent 工具过滤

### 4.1 激活集传递路径

```
前端 send_message(conv_id, prompt, active_scope)
  ↓ IPC
src-tauri send_message → ChatService.stream_with_memory(prompt, conv_id, active_scope)
  ↓
agent-core 构造工具时传入 active_scope：
  - document_tools(memory, active_doc_paths)  // 只查激活的文件
  - federation_tools(federation, active_source_names)  // 只查激活的数据源
  - active_scope 为空 → 两者都不挂
```

### 4.2 工具改动

**`document_tools.rs`**（`list_documents`/`search_documents`/`read_document`）：
- `document_tools(memory, allowed_paths: Vec<String>)` —— 构造时传入激活文件 path 列表
- `list_documents`：只返回 `allowed_paths` 内的文档
- `search_documents`：FTS5 MATCH 结果 `WHERE path IN (allowed_paths)` 过滤
- `read_document`：`path NOT IN allowed_paths` 时拒（避免模型读未激活的文件）

**`federation_tools.rs`**（`list_data_sources`/`describe_table`/`execute_sql`）：
- `federation_tools(svc, allowed_sources: Vec<String>)` —— 构造时传入激活数据源名
- `list_data_sources`：只返回 `allowed_sources` 内的源
- `describe_table`/`execute_sql`：`source_name NOT IN allowed_sources` 时拒

### 4.3 激活集为空

`stream_with_memory` 里：
```rust
let active_scope = ...; // 从 conversations.active_folders/active_sources + conversation_documents 解析
let doc_tools = if active_scope.doc_paths.is_empty() {
    Vec::new()  // 不挂文档工具
} else {
    document_tools(memory.clone(), active_scope.doc_paths)
};
let fed_tools = if active_scope.source_names.is_empty() {
    Vec::new()  // 不挂联邦工具
} else {
    federation_tools(federation.clone(), active_scope.source_names)
};
```

模型无知识库/联邦工具 → 按通用能力回答。chip 明示状态。

---

## 五、前端设计

### 5.1 Library 改造：文件树视图

`LibraryView.tsx` 从扁平列表改为**两栏文件树**（对齐云盘/VSCode 资源管理器）：

```
┌─ 左栏（~240px）─────────┬─ 右栏 ────────────────┐
│ 📁 知识库               │ 文件夹：/曾国藩专题    │
│  ├─ 📁 曾国藩专题 (3)   │ ┌──────────────────┐ │
│  ├─ 📁 方法论 (2)       │ │ 曾国藩唐浩明.epub │ │
│  ├─ 📁 Inbox (1)        │ │ 曾国藩智慧精髓.pdf│ │
│  └─ 📄 读通鉴论.epub    │ │ 📁 书信集 (1)     │ │
│                        │ └──────────────────┘ │
│ [+ 新建文件夹]         │ [上传到此文件夹]      │
└────────────────────────┴───────────────────────┘
```

- **左栏**：文件夹树（react-arborist，ARCHITECTURE.md §17.2 已指定），根目录散文件也列出
- **右栏**：选中文件夹内的文件 + 子文件夹
- **上传到此文件夹**：右栏上传按钮，指定 `folder_path`
- **移动文件**：拖拽到左栏文件夹（或右键菜单"移动到…"）

### 5.2 对话页"会话范围"chip

对话页顶部（Composer 上方或 Inspector 顶部）加 chip：

```
┌──────────────────────────────────────────┐
│ 📎 曾国藩专题 · 方法论 · ontology   ▾    │  ← 激活时
│ 💬 未挂载知识源                    ▾    │  ← 空激活集
└──────────────────────────────────────────┘
```

点开 popover：
```
┌─ 本次会话范围 ────────────────┐
│ 📁 知识库                     │
│  ☑ 曾国藩专题 (3 文件)       │
│  ☑ 方法论 (2 文件)           │
│  ☐ Inbox (1 文件)            │
│  ☐ 读通鉴论.epub             │  ← 根目录散文件，单文件可选
│                               │
│ 🗄 数据源                     │
│  ☑ ontology (PG, 57 表)      │
│  ☐ mydb (MySQL, 12 表)       │
│                               │
│ [全选] [清空]                 │
└───────────────────────────────┘
```

- 勾文件夹 = 该文件夹下所有文件（含子目录递归）进激活集 `folders`
- 勾单文件 = 该文件进激活集 `documents`
- 勾数据源 = 进激活集 `sources`
- 关闭 popover 即生效（持久化到会话）

### 5.3 `@` 菜单改动

`MentionMenu.tsx`：
- **候选范围**：所有文件（不限激活集）+ 所有数据源表
- 选中 `@文件` → 调 IPC 把该 path 加入会话激活集 `documents`（若未在）+ 原位插入 `@fileName`
- 选中 `@sourceName.tableName` → 把 sourceName 加入激活集 `sources` + 插入 `@sourceName.tableName`

### 5.4 上传行为

**会话内上传**（Composer 拖拽/⌘O）：
- 落到 `/Inbox`，持久化
- 不弹"选文件夹"框，零摩擦
- 上传后该文件自动加入当前会话激活集 `documents`（本次会话立即可用）

**Library 上传**（右栏"上传到此文件夹"）：
- 落到指定 `folder_path`
- 不自动加入任何会话激活集（用户要用时 `@` 或 chip 挂载）

---

## 六、IPC 契约

### 6.1 新增/改动 command

```rust
// ── 文件夹操作（无独立 folders 表，由文件移动隐式完成） ──
#[tauri::command] #[specta::specta]
pub async fn move_document(path: String, target_folder: String) -> AppResult<()>;
// 重命名文件夹：批量 move
#[tauri::command] #[specta::specta]
pub async fn rename_folder(old_path: String, new_path: String) -> AppResult<()>;
// 列出所有文件夹（DISTINCT folder_path，用于树视图）
#[tauri::command] #[specta::specta]
pub async fn list_folders() -> AppResult<Vec<String>>;

// ── 会话激活集 ──
#[tauri::command] #[specta::specta]
pub async fn get_active_scope(conversation_id: String) -> AppResult<ActiveScope>;
#[tauri::command] #[specta::specta]
pub async fn set_active_scope(conversation_id: String, scope: ActiveScope) -> AppResult<()>;
// ActiveScope = { folders: Vec<String>, documents: Vec<String>, sources: Vec<String> }

// ── 上传时指定 folder_path（改动现有 ingest_file） ──
// ingest_file(path, folder_path: Option<String>)  // None = /Inbox（会话上传默认）
```

### 6.2 现有 command 兼容

- `listMountedDocuments` / `mountDocument` / `unmountDocument`：保留，`conversation_documents` 表语义不变（`@` 触发的单文件激活走这张表）
- `send_message`：加 `active_scope` 参数透传给后端（或后端从 conversations 表读，前端只传 conv_id）——**倾向后者**：激活集已持久化到会话表，后端直接读，前端不必每轮传

---

## 七、落地路径

### 7.1 顺序

```
阶段 1：后端数据模型（memory crate）
  ├─ 1a. documents 加 folder_path 列 + 迁移（旧数据 → /Inbox）
  ├─ 1b. conversations 加 active_folders/active_sources 列
  ├─ 1c. 文件夹操作 repo（move/rename/list_folders）
  └─ 1d. 激活集读写 repo（get/set_active_scope）

阶段 2：agent-core 工具过滤
  ├─ 2a. document_tools 加 allowed_paths 参数
  ├─ 2b. federation_tools 加 allowed_sources 参数
  └─ 2c. stream_with_memory 解析激活集 + 空集不挂工具

阶段 3：IPC 薄层
  └─ move_document / rename_folder / list_folders / get_active_scope / set_active_scope
     + ingest_file 加 folder_path 参数

阶段 4：前端
  ├─ 4a. LibraryView 改文件树两栏视图（react-arborist）
  ├─ 4b. 对话页"会话范围"chip + popover
  ├─ 4c. MentionMenu 候选全量 + @触发激活
  └─ 4d. 上传落 /Inbox + 激活集联动
```

### 7.2 验证标准

- 旧库迁移：5 本书 `folder_path = /Inbox`，旧会话 `active_* = NULL`（空激活集）
- 新会话默认空激活集 → 模型无文档/联邦工具 → 通用回答
- chip 勾"曾国藩专题" → `search_documents` 只查该文件夹文件
- `@读通鉴论` → 该文件进激活集，模型可 `read_document`
- 会话内上传 → 落 `/Inbox` + 自动激活
- Library 文件树两栏，拖拽移动文件改 `folder_path`
- 切走会话再回来 → 激活集恢复

---

## 八、与既有架构的关系

- **不修订 ARCHITECTURE.md 既有决策**：决策 17（`@` 挂载）语义不变，本次延伸"`@` 触发激活" + "激活集过滤工具"
- **`conversation_documents` 表保留**：语义从"挂载文档"扩展为"激活集 documents 部分"，向后兼容
- **PHASE3-FEDERATION.md §3.1 工具签名**：`list_data_sources`/`describe_table`/`execute_sql` 加 `allowed_sources` 过滤参数，不破坏现有契约
- **决策 17 的 `<mounted-documents>` 注脚**：保留，仍由 `send_message` 在 user message 尾部追加。激活集的 `documents` 部分就是注脚来源

---

## 九、待定/风险

| 项 | 状态 | 说明 |
|---|---|---|
| 旧 5 本书迁移到 `/Inbox` 还是根目录 `/` | **`/Inbox`**（用户拍板） | 语义清晰（未分类），用户可自行归类 |
| 文件夹删除时文件去哪 | **一并删除**（用户拍板） | 删文件夹 = 确认这组文件不要了。批量 deleteDocument：清 documents 行 + FTS5 索引 + conversation_documents 关联。被激活/引用的文件删除后关联自动失效（path 外键 LEFT JOIN 取不到即跳过，不报错） |
| `active_scope` 前端传还是后端读 | **后端读**（用户拍板） | 已持久化到 conversations 表，后端 stream_with_memory 时直接读 active_folders/active_sources + conversation_documents，前端只传 conv_id |
| 大量文件时文件树性能 | 留后续 | 桌面单用户，文件数预期 <1000，react-arborist 虚拟滚动够用 |
| 文件夹路径中的特殊字符（`/`） | 文件名禁 `/` | 文件系统惯例，上传时校验 |
