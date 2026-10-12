# 设计宪章用户主权（Charter Sovereignty）——约束与实现

> 状态：已落地（MR #541 / #542 / #547，2026-09）
> 前置：ARCHITECTURE.md 决策 22（charter 表设计）。本文记录其后的**修改权约束**——为什么 AI 不能改宪章、堵了哪些路径、改的时候别破坏什么。

## 一、约束（不变点，改动前必读）

**设计宪章（charter）是本体的"不变点"**——**业务字段仅两个：业务场景（business_scenario）/ 业务本质（business_essence）**（2026-09 用户决策；design_intent/invariants 废弃不再下发 UI，结构与 TTL 头部四谓词保留兼容存量）。全生命周期修改极少且属用户设计决策。

**红线：宪章写入路径唯一——用户在 CharterPanel 点「保存」。**

AI（对话 agent）在任何情况下不得写宪章，包括：

1. 用户明确授权也不行——守卫在 Rust 工具层，不在提示词层，LLM 无法绕过
2. 借任何通用工具间接改写也不行（见威胁模型）
3. 字段部分更新不允许——全量写入（业务字段两字段；TTL 头部四谓词载体不变），防止置空覆盖

AI 允许做的是：**读宪章、给修改建议、生成精确的手工修改指引**。决定权始终在用户。

## 二、威胁模型（为什么堵这些路径）

宪章数据有两个载体，写入路径各自被约束：

| 载体 | 位置 | AI 可达的写路径 | 状态 |
|------|------|----------------|------|
| Palantir 本体 | `ontology_charter` 表 | `set_ontology_charter` agent 工具 | 已移除（P1） |
| W3C TTL 本体 | `ontology_ttl_charter` 表 **＋** `.ttl` 文本 `owl:Ontology` 头部内嵌（dcterms:description / skos:definition / rdfs:comment / skos:scopeNote） | `set_ontology_ttl_charter` agent 工具；`import_ontology_ttl(overwrite=true)` 整块覆盖 | 已移除（P1）；import 加守卫（#547） |

TTL 本体的宪章**双载体**是关键：表是结构化索引，TTL 头部是 W3C 原生词汇的原文。只堵工具不堵 import 是实测发现的侧门——AI 会想到"export → 改 owl:Ontology 头 → import(overwrite)"整条链路。

## 三、实现分层（改哪里都要对上）

### 第 1 层：agent 工具移除（P1，#542）

- `crates/agent-core/src/ontology_ttl_tools.rs`：`set_ontology_ttl_charter_tool` 注册与实现已删除，工具组 8→7（`tool_group_has_seven_tools` 断言，含反向断言：`set_ontology_ttl_charter` 不得出现在工具组）
- `crates/agent-core/src/ontology_tools.rs`：`set_ontology_charter_tool` 同样删除，工具组 4→3（`modeling_tools_constructs_three`）
- **想加回来？** 测试会拦——先想清楚是否要推翻本约束，再改测试并说明理由

### 第 2 层：import 侧门守卫（#547）

- `crates/ontology-store/src/ttl.rs`：pub `extract_charter_predicates(ttl)` —— 扫描 owl:Ontology 声明块（到引号外首个 `.`），提取宪章四谓词原文（支持前缀缩写与全 IRI）；`extract_ontology_meta` 改 pub
- `crates/agent-core/src/ontology_ttl_tools.rs`：`charter_guard()` 接入 import 工具闭包，**仅当「同 IRI 已存在 + overwrite=true + 宪章四字段与库内原文不一致」时拒绝**，拒绝信息列出变更字段并指引面板
- 放行场景（AI 建模能力不受影响的保证）：
  - 新本体首次导入（宪章随本体建立，属正常创建）
  - overwrite=false（同 IRI skip，无写入）
  - 覆盖但宪章四字段与库内原文完全一致（改类/属性/关系/规则随意）
- **前端 IPC `import_ontology_ttl` 命令不加守卫**——用户在 UI 手动导入/粘贴全文不设限（用户主权包括用户自己直接写）
- Palantir 侧无需守卫：`Store::import` 本来就不覆盖 charter（决策 22 写入路径分离）

### 第 3 层：AI 建议（P0，#541，纯前端）

- `src/lib/charter-suggest.ts`：四模式（起草/补全/完善/查漏）直调 LLM（OpenAI 兼容 + Anthropic 双协议），system prompt 写死红线：**上下文不足的字段返回 null + 提问，严禁虚构业务事实**
- `src/components/ontology/OntologyTtlView.tsx` `CharterAiSuggest` 组件：编辑模式内默认折叠；建议以逐字段对比卡片呈现，采纳→进草稿（仍可手改），保存→才落库
- 建议永不自动落库；失败 toast，无静默写入

### 第 4 层：TTL 内嵌宪章回退显示（#511，MR 已合并）

`ontology_ttl_charter` 表为空时，前端解析 TTL 头部内嵌宪章直接展示（`src/lib/ttl-charter.ts`）。用户在面板保存后以表为准。**改 import 流程时注意**：不要让 import 悄悄写 charter 表，否则用户手改的面板内容会被覆盖。

### 第 5 层：宪章先行闸门（决策 24）

- `new_ontology_gate_ttl`（`crates/agent-core/src/ontology_ttl_tools.rs`）/
  `new_ontology_gate_palantir`（`crates/agent-core/src/ontology_tools.rs`）：
  import 工具对库内不存在的 IRI/api_name 拒绝——AI 不得从 0 创建本体
- 唯一创建路径：前端宪章向导 `CreateOntologyWizard` → IPC
  `create_ontology_ttl_with_charter` / `create_ontology_with_charter`
  （agent 工具层无此命令，物理不可达）
- 存量本体放行；SKILL 冷启动指引已重写（不再引用已删除的 set_ontology_charter）

## 四、实测验证（2026-09，用户验收）

对话要求 AI"改业务场景"，实测回复符合预期的完整行为链：

1. 尝试 SPARQL UPDATE → 失败（query 端点只读，架构固有）
2. 评估 import 路径 → 读到工具描述的守卫语义，主动放弃（未烧 token 强闯）
3. 确认"我的工具集里没有 set_charter 类工具"
4. 明确告知"即使你明确授权也不能改——守卫在平台层"
5. 给出精确的面板手工修改指引 + 语义影响提醒（删掉后半句会丢失分析用途描述）

## 五、写入路径全景（改宪章相关代码前对照）

```
❌ set_ontology_ttl_charter / set_ontology_charter 工具  → 已移除（P1）
❌ import(overwrite) 改 owl:Ontology 头部                → charter_guard 拒绝（#547）
❌ SPARQL UPDATE                                          → query 端点只读（架构固有）
❌ 用户授权 social-engineering                            → 守卫在 Rust 层，提示词无效
❌ import 创建不存在的本体                                 → 宪章先行闸门拒绝（决策 24）
✅ 前端 IPC set_ontology_ttl_charter / set_ontology_charter → CharterPanel「保存」唯一入口
✅ 前端宪章向导 create_*_with_charter                       → 新建本体唯一路径（用户确认宪章）
✅ 用户 UI 手动 import 全文                                → 不设限（用户自己写自己的）
```

## 六、相关测试（动这些代码必须跑）

```bash
cargo test -p agent-core   # 127 passed：lib 119（含 charter_guard 3 场景、
                           # tool_group_has_seven_tools、modeling_tools_constructs_three）
cargo check --workspace    # 0 error
```

关键断言：`charter_guard_rejects_charter_edit_via_import`（改宪章必须被拒）、
`tool_group_has_seven_tools`（工具组不得重新出现 charter 写工具）。

## 七、遗留与后续（可选，均不紧急）

- P2：把「AI 建议」入口加到对话回复中（AI 被拒后一键跳转面板并预填建议）——纯前端
- P2：`suggest_ontology_charter` 服务端化（当前前端直调 LLM，api_key 前端可见，对齐 useHasProvider 既有约定）
- 冷启动 SKILL（ontology-modeling-w3c）中"主动提取落库"的旧指引若仍存在，应改为引导用户走面板
