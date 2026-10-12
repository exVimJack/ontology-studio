# 本体建模证据链：按用户输入建模，不臆想（Evidence Chain）

> 本文件是 onto-studio「本体建模证据链」的设计文档。
> 目标：保证本体建模的**准确性、可追溯**——所有建模产物（类/属性/关系）必须能追溯到用户输入的原文（场景描述、文档、数据库 schema 等），机械校验拦截臆想。
> 架构总纲见 `ARCHITECTURE.md`（本文件对应新增「决策 24」）；变更闸门语义见决策 23（surgery_import / preview 校验失败不落库）。
>
> 创建：2026-09

---

## 一、问题与目标

### 1.1 问题

当前从「用户输入 → LLM 建模」的链路上，**没有任何机制强制产物对应原文**：

1. **臆想无法拦截**：LLM 说"根据您的描述新增了 `OrderStatus` 类"，但描述里可能根本没有这个概念。prompt 约束（"请不要臆想"）对齐率天花板约 90%，且**事后无法验证**——这不是"保证"，只是"祈祷"。
2. **追溯断裂**：现有追溯设施只有两条，都不指向原文——
   - Charter 四字段记录的是**业务意图**（为什么建这个本体），不是逐条证据；
   - changelog 记录的是**设计说明**（为什么改），`conversation_id` 只能跳回会话，不能定位到"依据哪段原文"。
3. **人工审计成本高**：验收一个建模结果只能靠人通读输入再逐个核对产物，没有"点开一个类，看到它来自哪句话"的能力。

### 1.2 核心设计哲学

**不信任 LLM 的自我声明，只信逐字匹配。**

LLM 对 evidence 的自我报告不可信，但"引用原文是否存在于源文档中"是**机械可判定**的。本方案把"防臆想"从 prompt 层（概率约束）下沉到落库闸门（确定性约束）——臆想在 `preview_ontology_import` / `surgery_import` 时**必然被打回**，这与决策 23 已确立的「校验失败不落库」闸门语义是同一哲学的延伸。

### 1.3 目标

- **无引用不产出**：每个建模元素必须携带 evidence（来源 + 原文引用），否则带 `ungrounded` 标记显式可见
- **引用必真**：evidence 的 quote 必须能在源内容中机械匹配到，匹配失败 = 落库阻断
- **全程可追溯**：任何类/属性/关系都能查到"来自哪个源、哪段原文、哪次会话、哪个 revision"
- **不破坏既有闸门**：复用 preview/surgery 现有校验管道，evidence 校验是新增项，不是替代

### 1.4 范围边界

| 做 | 不做（留后续） |
|---|---|
| 三类输入源登记：场景描述 / 文档 chunk / DB schema（表、列） | 网页、音视频转写等新源类型 |
| 建模工具 schema 强制 evidence 字段 | 强制所有历史存量本体补录 evidence |
| quote 机械校验（归一化匹配）闸门 | 语义级校验（判断引用是否"切题"——机械匹配已能挡住臆想主体） |
| evidence 双写：TTL 注解 + SQLite 索引表 | evidence 的版本 diff / 冲突合并 |
| 详情面板证据展示（🟢🟡🔴 三态）+ 出处跳转 | 跨本体 evidence 复用图谱 |
| ungrounded 元素灰显标记 + 一键清理 | 自动删除 ungrounded（必须人工确认） |

---

## 二、总体架构：证据链四层

```
┌─────────────────────────────────────────────────────────────────┐
│ L1 源登记（Source Registry）                                     │
│   场景/文档/schema → Source { source_id, kind, content, hash }   │
│   快照不可变，引用指向登记时刻的版本                                │
├─────────────────────────────────────────────────────────────────┤
│ L2 生成侧强制引用（无引用不产出）                                  │
│   建模 tool schema: evidence 为必填结构化字段                     │
│   Evidence { source_id, quote, span?, role }                     │
├─────────────────────────────────────────────────────────────────┤
│ L3 机械校验闸门（防臆想的根本）                                    │
│   preview / surgery_import 管道新增 evidence 校验器：              │
│   errors 阻断（源不存在 / quote 匹配失败）                        │
│   warnings 放行标记（ungrounded / inferred）                      │
├─────────────────────────────────────────────────────────────────┤
│ L4 存储 + 展示（追溯闭环）                                        │
│   双写：TTL 注解（可导出）+ element_evidence 表（可查询）           │
│   详情面板三态展示 + changelog revision 关联                      │
└─────────────────────────────────────────────────────────────────┘
```

---

## 三、L1 源登记（Source Registry）

### 3.1 数据模型

新增统一源登记表（**不改动**现有 `data_sources` / `datasets`，通过外键关联复用）：

```sql
CREATE TABLE evidence_sources (
    source_id    TEXT PRIMARY KEY,          -- 稳定 id：src-{ulid}
    ontology_id  TEXT NOT NULL,             -- 归属本体（一个源可服务多个本体时不设唯一约束）
    kind         TEXT NOT NULL,             -- 'scenario' | 'document' | 'db_table' | 'db_column'
    title        TEXT NOT NULL,             -- 人类可读出处：消息时间 / 文件名+chunk路径 / 库.表.列
    content      TEXT NOT NULL,             -- 源文本快照
    content_hash TEXT NOT NULL,             -- sha256，登记时快照指纹
    origin_ref   TEXT,                      -- 关联既有设施：document_id / dataset_id / conversation_message_id
    created_at   INTEGER NOT NULL
);
```

设计决策：

1. **快照不可变**：`content` 是登记时刻的副本。用户后来改了文档/删了消息，不影响已登记引用的有效性——evidence 引用的是"建模当时看到的输入"。`origin_ref` 保留跳转到现网实体的能力，跳转失败时降级展示快照。
2. **复用而非替代**：文档走已有 ingest 管道（chunk 粒度登记，chunk id 写入 `origin_ref`）；DB schema 走已有 datasets（按表/列两级粒度登记，`db_table` 的 content 为 `CREATE TABLE` 摘要 + 列注释，`db_column` 为列定义）；场景描述是新增能力，从对话中按"用户建模指令块"切分登记（用户说"帮我建模"到"开始生成"之间的描述文本）。
3. **粒度即引用粒度**：DB schema 登记到列级，是为了后续"建模出的属性必须能在 schema 源中找到对应列"的反向校验（见 P3）。

### 3.2 登记时机

- **场景描述**：用户在会话中触发建模（提到"建模/本体/类"等意图）时，由前端将该轮及前序相关轮的用户消息块打包登记，`source_id` 随建模请求传给 agent
- **文档**：挂载到本体建模会话时自动按 chunk 登记（幂等：hash 相同跳过）
- **DB schema**：数据源绑定到本体时自动登记表/列两级

---

## 四、L2 生成侧强制引用（无引用不产出）

### 4.1 Evidence 结构

```rust
#[derive(Serialize, Deserialize)]
pub struct Evidence {
    pub source_id: String,      // 必填，必须指向已登记的 evidence_sources
    pub quote: String,          // 必填，原文逐字引用，≤200 字符
    pub span: Option<(u32, u32)>, // 可选，在源 content 中的字符偏移
    pub role: EvidenceRole,     // direct | inferred
}

pub enum EvidenceRole {
    Direct,    // 直接来自原文（quote 必须逐字匹配）
    Inferred,  // 由多条原文组合推断（须人工确认，见 4.3）
}
```

### 4.2 tool schema 改造

现有建模工具（`ontology_modeling_tools`，`ontology_tools.rs:262`）在**入参 schema** 上强制 evidence：

- `import_ontology` 的 `PropertyDef` / `LinkDef` / 对象类型定义，各增加可选 `evidence: Evidence[]` 字段（TTL 侧对应 `ttl_edit` 的注解参数）
- **agent 系统提示词**同步声明建模规约：每个类/属性/关系必须给出 `role=direct` 的 evidence；确属抽象设计（如纯结构中间层）时允许省略，但产物会带 ungrounded 标记需用户确认
- 关键点：**evidence 是结构化字段而非自由文本**。臆想在结构上无处藏身——LLM 要么给出真实 source_id + quote，要么显式承认 ungrounded

### 4.3 inferred 的处理

组合推断（如从"客户""下单""三天未付款"三段原文推断出 `OverdueOrder` 中间层）允许存在，但：

- `role=inferred` 的 evidence 强制要求 `quote` 覆盖全部参与推断的原文片段（quote 为多段拼接，用 ` ⟪…⟫ ` 分隔，校验器逐段匹配）
- 落库后详情面板显示 🟡 态，需用户手动确认才转为正式（确认动作记入 changelog）

---

## 五、L3 机械校验闸门（本方案灵魂）

### 5.1 校验器规格

新增 `evidence::validate` 纯函数（对齐 `naming.rs` 预检的"落库前机械校验"模式），挂入 `preview_ontology_import` 与 `surgery_import` 既有校验管道：

```
输入：payload 中的全部 evidence + evidence_sources 表
输出：errors: Vec<String>（阻断） / warnings: Vec<String>（放行标记）
```

**errors（阻断，errors 非空不落库）**：

| # | 条件 | 错误信息 |
|---|---|---|
| E1 | `source_id` 在 evidence_sources 中不存在 | `引用了未登记的源 {source_id}` |
| E2 | `role=direct` 且 quote 匹配失败（见 5.2） | `引用原文不存在于源 {title}，疑似臆想："{quote 前 50 字}…"` |
| E3 | `role=inferred` 且拼接段中任一段匹配失败 | 同 E2，逐段报 |
| E4 | evidence 元素在 payload 中找不到对应目标（挂错位置） | `evidence 挂载目标 {api_name} 不存在` |

**warnings（放行但显式标记）**：

| # | 条件 | 含义 |
|---|---|---|
| W1 | 元素无任何 evidence | `ungrounded`，灰显 |
| W2 | 全部 evidence 均为 inferred | `unconfirmed`，需人工确认 |
| W3 | 同一 quote 被超过 20 个元素引用 | 可疑的"一段话撑起半个本体"，提示复核 |

**严格模式（strict，per-ontology 开关，默认开启）**：

| # | 条件 | 行为 |
|---|---|---|
| S1 | 严格模式下元素无任何 evidence | 升级为 **errors 阻断**（不再走 W1 放行） |
| S2 | 宽松模式（用户显式关闭 strict） | ungrounded 降回 W1 warning 放行，但预演页必须展示 ungrounded 清单 |

**豁免注解（P2 补充，已实施）**：确属抽象设计、用户在对话中显式确认的元素，可写
`skos:editorialNote "evidence: none | direct | 「用户确认理由」"`。strict 闸门放行，
落库 `matched_by='manual'`（source_id 空串），UI 显示 ⚪ 手认——保留人工自主权，
且豁免必须有「理由」载体可审计。

设计动机：若 ungrounded 永远只是 warning，LLM 可以对所有产物**一律不给 evidence** 来绕过闸门——"必须按输入建模"被架空。严格模式使"不臆想"成为默认行为，宽松是显式例外且全程可见。

### 5.2 quote 匹配算法（分级匹配，容忍格式抖动、不容忍改写）

```
1. 精确匹配：quote in source.content                 → 通过
2. 归一化匹配：双方做 normalize() 后 substring 匹配   → 通过
   normalize：trim + 连续空白折叠为单空格 + 全半角统一 + Unicode NFKC
3. 均失败                                            → 匹配失败（E2/E3）
```

设计要点：

- 归一化**只处理空白与字符宽度**，不做事（stemm/同义词）匹配。LLM 把"客户下单后三天未付款"改写成"用户逾期三天"会**匹配失败被打回**——这正是我们要的行为：不允许改写，只允许复制
- 归一化匹配通过时校验器返回 `matched_by: "normalized"`，evidence 表记录匹配等级，便于审计区分
- `span` 若提供则以 span 切片做二次确认，span 与 quote 不一致按 E2 处理（防止 LLM 给出"指向别处"的假定位）
- **性能约束**：预演校验按 `source_id` 分组，每个源只 normalize 一次并缓存结果；大文档（MB 级）源在元素多时避免 O(N×M) 重复扫描
- quote 跨 chunk 场景：同一源相邻 chunk 拼接文本中重试一次（P2 落地）

### 5.3 阻断后的修复回路（指导性打回）

闸门打回时，错误信息回传给 agent（对齐 surgery_import 既有回路），且 E2/E3 的错误信息**必须附带源原文上下文**：定位 quote 归一化匹配的最佳近似位置，截取其前后 ±200 字符随错误返回。这样 agent 下一轮直接从真实原文重新摘录，而不是在看不到源内容的情况下编造"看起来像原文"的引用陷入阻断循环。

错误信息同时携带 `matched_by` 尝试等级（exact / normalized / none），便于区分"空白差异误伤"与"真改写"，指导不同修复策略。用户侧在对话中看到阻断原因，可补充输入或确认放行。

### 5.4 两条建模路径的 evidence 入口收敛

| 路径 | evidence 载体 | 校验器获取方式 |
|---|---|---|
| Palantir 结构化 payload（import_ontology） | `PropertyDef` / `LinkDef` / 对象类型定义的 `evidence` 字段 | 直接读字段 |
| W3C 文本手术（ttl_edit / surgery_import） | 产物 TTL 中的 `prov:hadPrimarySource` + `skos:editorialNote` 注解（LLM 在手术文本中写入） | 校验器解析产物 TTL 提取注解后统一校验 |

即：LLM 无论走哪条路径，evidence 的**表达形态**只有"payload 字段"或"TTL 注解"两种，校验器一视同仁。W3C 路径的 ttl_edit 工具提示词必须给出注解书写模板，避免 LLM 不会写 PROV 词汇。

---

## 六、L4 存储与展示（追溯闭环）

### 6.1 双写存储（对齐 charter 的既定模式）

对齐 `TtlCharter` 的「TTL 内嵌 + 表索引」双写约定（`ttl.rs:77` 注释）：

**SQLite 侧**——新增索引表：

```sql
CREATE TABLE element_evidence (
    id           TEXT PRIMARY KEY,
    ontology_id  TEXT NOT NULL,
    element_type TEXT NOT NULL,     -- 'object_type' | 'property' | 'link'
    element_id   TEXT NOT NULL,     -- api_name
    source_id    TEXT NOT NULL REFERENCES evidence_sources(source_id),
    quote        TEXT NOT NULL,
    span         TEXT,              -- JSON [start,end] 或 null
    role         TEXT NOT NULL,     -- 'direct' | 'inferred'
    matched_by   TEXT NOT NULL,     -- 'exact' | 'normalized'
    revision     INTEGER NOT NULL,  -- 引入该 evidence 的 changelog revision
    confirmed    INTEGER NOT NULL DEFAULT 0,  -- inferred 是否已人工确认
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_ee_element ON element_evidence(ontology_id, element_type, element_id);
```

**TTL 侧**——W3C 原生词汇注解（export 可带走、import 可还原，round-trip 不丢）：

```turtle
<#OrderStatus> a owl:Class ;
    rdfs:label "订单状态"@zh ;
    prov:hadPrimarySource <urn:evidence:src-01HXXXX> ;   # 指向 source
    skos:editorialNote "evidence: 「订单创建后处于待支付状态」（scenario, direct, rev 3）" .
```

- `prov:hadPrimarySource` 是 W3C PROV 标准词汇，语义精确；`skos:editorialNote` 承载 quote 摘要（人类可读，兼容不识别 PROV 的第三方工具）
- import 时解析注解回填 `element_evidence` 表；source 快照不在 TTL 里（体积考虑），`urn:evidence:` IRI 指向库内登记

### 6.2 changelog 关联

`commit_ontology_change`（`ontology_tools.rs:659`）的 `change_summary` JSON 增加可选字段：

```json
{ "created": ["OrderStatus"], "evidence_refs": ["src-01HXXXX", "src-01HYYYY"] }
```

实现"revision → 本次引用了哪些源"的反查。已有 `conversation_id` 与 evidence 的 `origin_ref`（消息 id）交叉，形成完整追溯链：

```
类/属性/关系 → element_evidence → evidence_sources → (origin_ref) → 文档/数据集/对话消息
             → revision → changelog → conversation_id → 会话
```

### 6.3 详情面板三态展示

OntologyView / OntologyTtlView 的元素详情卡片新增「依据」区块：

| 状态 | 视觉 | 内容 |
|---|---|---|
| 🟢 grounded（direct 已确认） | 绿点 | 可展开 quote 原文 + 出处（文件名/表名/会话时间），点击跳转 origin_ref |
| 🟡 inferred / unconfirmed | 黄点 | 显示"由哪些原文组合推断"，附「确认采纳」按钮（写 confirmed + changelog） |
| 🔴 ungrounded | 灰显 + 角标「无输入依据」 | 附「补充依据」「删除」两个动作，删除需确认 |

列表页在元素行尾以同色小图标给出状态总览；`preview_ontology_import` 预演结果页同步展示 warnings，让用户**落库前**就能看到 ungrounded 清单。

---

## 七、实施分期

| 期 | 内容 | 交付物 | 工作量评估 |
|---|---|---|---|
| **P1 防臆想闸门（含最小源登记）** | `evidence_sources` 表（先只支撑 scenario 源的手动/半自动登记）+ evidence 结构 + tool schema / ttl_edit 注解改造 + `evidence::validate` 校验器挂入 preview/surgery 管道 + 严格模式默认开 + agent 提示词规约 | 即使没有 UI，臆想已在落库前被机械拦截；ungrounded 默认阻断 | 中（校验器依赖源登记表，二者必须同期交付） |
| **P2 登记 + 追溯闭环** | 文档 chunk / DB schema 自动登记 + `element_evidence` 表 + TTL 注解双写 + 详情面板三态展示 + 出处跳转 | 人工可审计：点开任意元素看到原文出处 | 中 |
| **P3 深度校验** | DB schema 反向校验（建模属性必须对应已登记列，对不上报 warning）；文档 chunk 定位跳转（span → 高亮原文位置）；场景描述自动切分登记增强 | 属性级 schema 对齐 + 原文级跳转 | 中 |

注意：P1 的校验器 E1 需要查 `evidence_sources` 表，因此**最小源登记必须纳入 P1**（仅 scenario 源 + 手动登记入口），否则闸门无法运行——这是从早期「P1 可独立交付」版本修订得出的结论。

---

## 八、测试与验收

**校验器单测**（对齐 `palantir-modeling.test.ts` 的纯逻辑层测试风格）：

- 精确 / 归一化 / 改写三档 quote 的匹配判定
- E1–E4 / W1–W3 全分支
- inferred 多段拼接逐段匹配
- span 与 quote 不一致按失败处理

**集成测试**：

- TTL 注解 export → import round-trip，evidence 不丢
- 闸门阻断 → agent 修复回路（错误信息含原文提示）
- 存量本体（无 evidence）import 不被误伤（evidence 为空 = W1 放行，不阻断历史数据）

**人工验收标准**：

1. 让 LLM 基于"客户、订单、付款"三段描述建模，故意诱导其发明描述中不存在的"物流跟踪"类 → 落库必须被 E2 阻断
2. 点开任一正常建模的类 → 3 秒内看到其来源原文与出处
3. export 本体为 TTL，在另一环境 import → 证据链完整还原

---

## 九、风险与对策

| 风险 | 对策 |
|---|---|
| LLM 大量产出 ungrounded（偷懒不给引用）导致体验差 | 严格模式默认阻断（S1），agent 被迫补引用；提示词规约 + 预演结果页 ungrounded 清单显式曝光 |
| 外部导入的第三方 TTL 全部 ungrounded，严格模式直接阻断导致无法导入 | 外部 TTL import 时该本体默认宽松模式（S2），落库后整体呈 ungrounded 态提示补录；用户可手动开启严格 |
| 归一化匹配仍误伤（原文含表格/Markdown 结构，quote 提取后空白差异大） | normalize 已覆盖空白折叠与全半角；预留 `matched_by` 记录实际等级，误伤案例驱动规则迭代；仍不引入模糊匹配（保住"不容忍改写"的底线） |
| 大文档 chunk 过碎，quote 跨 chunk 无法匹配 | 登记时保留跨 chunk 场景：quote 校验失败后允许在**同源相邻 chunk 拼接文本**中再试一次（P3 项） |
| TTL 注解体积膨胀 | editorialNote 只存 quote 摘要（首 50 字 + 截断标记），完整 quote 在 SQLite 表；PROV IRI 引用不膨胀 |
| 与外部导入的第三方 TTL 不兼容 | 本方案只对"LLM 建模路径"强制 evidence；外部 TTL import 不要求 evidence（落库后整体呈 ungrounded 态，提示用户补录） |

---

## 十、未决问题（Open Questions）

1. **场景描述的切分边界**：用户一段长对话里既有闲聊又有建模需求，自动切分的准确率需要实测；P2 先用"触发建模的那一轮 + 用户显式 @ 的消息"做保守切分，观察后再决定是否引入语义切分。
2. **inferred 确认的粒度**：逐元素确认还是按 revision 批量确认？倾向按 revision 批量（一次建模一轮确认），待 UI 原型后定。
3. **证据链的删除与改名语义**：`element_id` 用 api_name（决策 23 已锁定不可改，改名断链风险低），但**删除重建**（Palantir overwrite 路径 = 删旧 OT 重建）必然断链——element_evidence 需 tombstone 语义：删除时保留记录 + 置 tombstone，重建后按 api_name 重挂或由用户补录，待实现时确认表设计。

---

## 附：与既有决策的关系

- **不修订决策 23**：evidence 校验是 surgery_import / preview 管道的**新增校验项**，闸门语义（errors 非空阻断）完全复用
- **对齐决策 18/22 的复用哲学**：不为证据链新建交互范式，详情面板扩展现有卡片；纯逻辑层（evidence::validate / 前端 evidence 格式化）独立可单测
- **对齐 charter 双写约定**（`ttl.rs` TtlCharter 注释）：TTL 内嵌可导出 + 表索引可查询，两条通道不混用
- **对齐 `naming.rs` 预检模式**：落库前机械校验，错误信息可读且可指导修复

---

## 七、证据链状态矩阵（W3C 路径，已实施）

> 对应 docs/EVIDENCE-CHAIN.md 与 PR !578。证据注解格式：
> `skos:editorialNote "evidence: {source_id|none} | {direct|inferred} | 「quote/理由」"`

### A. 导入已有 TTL

| 场景 | 操作者 | 闸门行为 | 落库结果 | UI 显示 |
|---|---|---|---|---|
| **初始导入**（TTL 无注解） | 用户在 UI 导入 | 不校验（人工=真相源） | 无证据行 | 全部 ⚪ 手工确认（可用，不阻断；用户导入即背书） |
| | agent 导入 | **S1 拒绝**：每个类/属性必须有注解 | — | agent 收到逐条修复指引 |
| **增加元素**（类/属性/关系） | agent（带注解+原句引用） | S1 + quote 逐字校验 → 通过 | 🟢 行（exact/normalized） | 🟢 有据 |
| | agent（引用改写） | **E2 拒绝**（疑似臆想，附源原文片段） | — | agent 修正后重导 |
| | agent（抽象设计+用户确认） | 豁免注解 `none \| 「理由」` 放行 | ⚪ 行（manual） | ⚪ 手认 |
| | 用户 UI 手工编辑 | 不校验 | 无证据行 | 新元素 ⚪ 手工确认 |
| **修改元素**（label/range 等） | agent 重导入 | 整本体 S1 全量重校验；注解在文本里即保留 | 证据不变 | 原状态保持 |
| | agent 删注解再导 | 该元素无注解 → S1 拒绝 | — | 修复指引 |
| | 用户 UI 手工改 | 不校验，注解不被触碰 | 证据保留 | 原状态保持 |
| **删除元素** | 任意 | 随文本消失 | 重导入时证据行清空 | 元素消失 |

### B. 新建本体模型（元素三态 + 边界）

前提：本体创建走宪章向导（用户主权，AI 不能 0→1 建库），agent 只能增量建模。每个元素落到三态之一：

| 状态 | 产生条件 | 闸门 | UI 标识 |
|---|---|---|---|
| 🟢 **有据**（direct） | 引用已登记源原句，逐字匹配通过 | exact / normalized | 🟢 + 依据区块（出处+quote） |
| 🟡 **推断**（inferred） | 多段组合推断，每段仍须逐字匹配源 | 逐段校验 | 🟡 + 全 inferred 时提示需人工确认 |
| ⚪ **手认**（manual） | 豁免注解 `none \| 「理由」`（用户显式确认） | 放行，理由可审计 | ⚪ + tooltip 人工确认设计 |

> **用户导入/手工编辑的元素不显示 🔴**：TTL 视图下无证据行的元素一律视为
> **⚪ 手工确认**——用户亲手导入/编辑 = 用户为定义背书，属显式人工确认，
> 不是「未经输入佐证」。⚪（手认）由此有两个来源：
> ① agent 豁免注解 `evidence: none | 「理由」`（落库 manual 行，理由可审计）；
> ② 用户手工导入/编辑的元素（无证据行，TTL 视图显示层判定）。
>
> 🔴 **无据** 在 TTL 视图仅表示「有证据行但引用校验失败」（matched_by 为空：
> 源被删除或 quote 对不上）——即曾经有据、现在失效，必须修复的状态。
>
> **记忆口诀**：agent 建模必三选一（🟢引用 / 🟡推断 / ⚪豁免）；
> 用户手动 = ⚪ 手工确认（用户即真相源）；🔴 = 引用失效待修复。
### 状态语义澄清（🔴 与 🟡 的存在意义）

**🔴 引用失效——数据血缘的断线检测器**。

agent 路径产生不了 🔴（闸门当场拦截），所以 🔴 全部来自「环境变化/时间推移」：

| 场景 | 怎么发生 | 常见度 |
|---|---|---|
| 跨环境迁移 | TTL 带注解导出后导入另一台机器/新库——`evidence_sources` 是本库本地的，源不存在 → 引用全部失效 | ★★★ 最常见 |
| 人工导入带坏注解 | UI 导入注解 quote 与源对不上的 TTL（人工路径不过闸门，但落库时机械重算 matched_by） | ★★ |
| 源内容变更 | 同名源重新登记（内容更新），旧 quote 失配 | ★ |
| 源被注销 | deregister 后引用悬空 | ★ |

即：TTL 文件可以到处拷，但源登记是本地的，🔴 就是**证据会腐烂**的报警——
不是「agent 犯错」，而是「数据血缘断裂」。后续配套：重新登记源后提供
「一键重校验」修复入口，把 🔴 变成可操作的动作项（待办）。

**🟡 inferred——LLM 推理跳跃的诚实标记**。

- 🟢 direct = 源文档原话就这么说（引用即证据，强度高）
- 🟡 inferred = 源里没有这句原话，是把几段原文拼起来推出的结论——每段引用都真实，
  但**段与段之间的推理跳跃是 LLM 做的**，这一步没有机械校验能覆盖

它给 LLM 的「综合推理」留一个诚实申报通道，同时标记需人审。砍掉它的两个替代都有问题：
强迫标 direct（状态撒谎，稀释「逐字引用」含金量）或禁止合成结论（agent 会滥用 ⚪ 豁免，
证据链退化）。

使用成色判断：它是三个状态里唯一 agent 自我申报的，但**造假无利可图**——每段仍须
逐字匹配，走 direct 反而更省事，所以实际只出现在 agent 确实做了组合推理的地方，
语义可信。真正的配套动作是 🟡 元素的「确认升级流」：用户确认后可升级 direct 或转 ⚪
（待办，目前仅 tooltip 提示）。

> 一句话：🔴 = 血缘断线检测器；🟡 = 推理跳跃的诚实标记。两者都不拦截、都不可伪造，
> 缺了哪个证据链都会漏一个面。

---

## 八'、欠账清单（核心闭环已完成，以下为体验打磨项）

> 最后更新：状态矩阵 + skill 证据环接入后。核心闸门/三态/持久化/skill 路径已可用。

### 一、闸门/工具层

| 事项 | 现状与影响 | 优先级 |
|---|---|---|
| 🟡 确认升级流 | inferred 仅 tooltip 提示；用户确认后升级 direct / 转 ⚪ 无入口 | 中 |
| Palantir strict per-ontology 开关 | strict 硬编码在 agent 工具；S2 宽松模式无 UI/无配置 | 低 |

### 二、UI 层

| 事项 | 现状与影响 | 优先级 |
|---|---|---|
| 源登记管理界面 | evidence_sources 无 UI（查看/内容/注销全靠对话）；🔴 排障体验差 | 高 |
| 🔴 一键重校验 | 重新登记源后无重算 matched_by 入口，🔴 不自愈 | 高 |
| TTL 导入对话框结构化错误 | 闸门拒绝的逐条指引只在对话流；UI 导入无「哪条失败+怎么改」清单 | 中 |
| TTL 概览瓦片状态点 | 类详情/属性表已有，概览瓦片无 | 低 |

### 三、自动化（L1 无感化）

| 事项 | 现状与影响 | 优先级 |
|---|---|---|
| DB schema 自动登记 | federation 浏览 schema 不登记 db_column 源——P3 反向校验经常「无源不告警」，威力打折 | 高 |
| 文档 chunk 自动登记 + span 跳转 | 挂载文档不登记 doc 源；TTL 侧 span 全空；「点依据→跳文档高亮」未做 | 中（最大块） |

### 四、验证关注点（打包测试建议）

- agent 建模会话：合规率（撞墙次数）、首次到位率（skill 证据环生效与否）
- 跨环境迁移：TTL 带注解导出→新库导入 → 🔴 引用失效是否如期呈现
- 豁免注解：none + 理由 → ⚪ 手认显示与审计
- P3：登记 db_column 源后，臆造列是否告警
