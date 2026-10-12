# Onto-Studio 本体建模结果校验方案（三阶段流程 + 多轮协同对抗）

> **版本**: v0.7（答辩归阶段① + 统一发现池） | **日期**: 2026-09-23
> **v0.7 变更**：① **warning 答辩从阶段②移入阶段①**——规则检查产出 warning 即可逐条反驳（提论据 → 机械匹配 → B 机器裁决），不再等语义评审先行；阶段①的「零 LLM」精确为「**检测零 LLM**」（检测引擎确定性，答辩裁决走 B）；② **统一发现池**：`validation_issues` 加 `source` 列（`rules`|`review`），B 语义评审的建议转写为 issue 行与规则发现同池同生命周期，A 修订说明逐条回填 `a_action`，**A rejected 的发现自动转 disputed 进仲裁队列**；③ 评审状态机补 `b_verdict`（B 复核原始输出落库）与 `step_ms_*`（环节耗时）；④ 同步实现现实：`resolve_dispute_llm`（B 机器裁决）与 §2.4 出口检查点均已实现，§5 schema/SQL 对齐代码。
> **v0.6 变更**：① 全文去掉 L0–L4 层级编号——非业界通用概念且各处范围不一，改用检查项本名（layer 字段仅作实现层标记保留在报告 schema）；② 阶段①只保留已实现内容（语法/结构/逻辑/覆盖矩阵/纠错循环），数据可落地性、CQ 反测、不变量编译均为**未实现的规划项**，移入「四、扩展校验维度」；方法论合规检查经取舍裁定不单独成层（标注完备性/SWRL 已随结构规范实现，tentative 并入覆盖矩阵，启发式三项由语义评审覆盖，检查项不单独成层）；③ 模型独立性独立成「三、模型独立性约束」章；④「不变量核验」明确不是独立流程环节——编译是一次性准备，产物随规则检查执行。
> **v0.5 变更**：「一、总体设计」重组（总览提为 §1.1、现状移 §6、缺口压缩为设计目标引言）；「二」按全流程阶段展开。
> **定位**: AI 辅助本体建模（W3C Turtle）生成结果的质量闸门设计——如何结合**输入（29 句话 / 宪章）**、**建模原则（skill 方法论）**、**数据源（datasets/data_sources）**对产出做机器可执行 + 语义级的校验，并以**多轮协同对抗**方式闭环迭代。
> **上游文档**:
> - 建模方法论：`src-tauri/resources/skills/ontology-modeling-w3c/SKILL.md`（五环联动流程）
> - 双模型协同校验：`src-tauri/resources/skills/ontology-modeling-w3c/references/modeling-workflow.md`（生成-审计-仲裁闭环）
> - 输入范式：`references/material-to-turtle.md`（29 句话 7 类语义规范）
> - 宪章不变点：`ARCHITECTURE.md` 决策 22/23（charter 两字段、AI 永不写宪章）
> - 证据链机制：`docs/EVIDENCE-CHAIN.md`（决策 24，Evidence 三级匹配）
> - 校验实现：`crates/ontology-store/src/ttl/`（validate/crud/charter 职责切分）
> - 数据源挂载：`crates/ontology-store`（datasets/data_sources 表）——数据可落地性校验（§4.1）的输入基础

---

## 角色命名（全局约定）

对抗流程中两个角色统一为一对命名，全文其他处用速记 A/B：

| 角色 | 界面文案 | 代码/类型 | 速记 | 职责 |
|---|---|---|---|---|
| 本体建模角色（生成） | 建模者 | `Generator` | **A** | 基于输入生成/修订本体（Turtle） |
| 本体校验角色（验证） | 校验者 | `Verifier` | **B** | 独立审查建模结果，产出问题 + 证据 |

- 对齐原方案图「本体建模（生成）⇄ 本体结果校验（验证）」的对抗语义，不引入 GAN 判别器等额外命名
- 两者是**编排流程中的对抗角色**（模型 + 独立 prompt + 构造输入），不是常驻的独立智能体
- A/B 必须跨供应商或至少跨模型家族（硬化约束见 §3.1）
- **规则检查（检测引擎）既不是 A 也不是 B**——是确定性代码（规则引擎），检测环节零 LLM；A 触发并按结果修复，B 不重跑（同样代码结果必然相同），B 通过评审 prompt 注入规则报告获取上下文；B 的介入点有二——①a warning 答辩裁决（阶段①内）与②多轮语义评审（阶段②）。两类发现进**统一发现池**（`source=rules|review`，§5.2）共享 issue 生命周期；真正的二次把关在出口检查点（§2.4）由系统内联重跑

---

### 1.1 全流程总览（三阶段 + 出口检查点，对齐实现）

三个阶段、一个出口检查点。**入口层**三个对等入口（对话命令 /validate、工具栏按钮、侧栏「校验任务」）都汇到同一个校验任务面板——命令只是入口，执行过程在面板展示，对话不刷刷屏。

```
入口层（对等三入口）
├── 对话斜杠命令 /validate
├── 对话工具栏盾牌按钮
└── NavRail「校验任务」
         │
         ▼
┌─────────────────────────────────────────────┐
│ 第一阶段：规则检查 + warning 答辩              │
│  ① 「运行校验」显式触发（检测零 LLM，毫秒级）    │
│     ├─ 语法解析 + 结构规范十项 lint             │
│     ├─ 逻辑一致（无环/主体类型/悬空引用）    │
│     ├─ issue 快照落库（error/warning/suggestion）│
│     └─ 覆盖矩阵随 ① 一并计算（§2.1.3）         │
│  ①b error 纠错循环（A 单方，≤3 轮，§2.1.4）    │
│  ①c warning 答辩（对规则 warning 逐条反驳，     │
│     issue 级单轮：论据 + 反证来源 + 原文引用      │
│     → 机械匹配 → B 机器裁决，§2.1.5）           │
└─────────────────────────────────────────────┘
         │
         ▼
┌─────────────────────────────────────────────┐
│ 第二阶段：对抗验证（B 裁决一切机器能解决的分歧） │
│                                             │
│  ② 多轮语义评审（本体级，后端驱动自动轮转）      │
│     B 评审（八维度，注入真实宪章 + 规则报告上下文，│
│     走 B 独立 ChatService）→ A 修订（接受/拒绝，│
│     理由对 B 不可见）→ B 复核（只看修订后 Turtle，│
│     防 A 说服 B）→ 收敛或轮次封顶              │
│                                             │
│  B 的评审发现转写入统一发现池（source=review）   │
│  A rejected 的发现自动 disputed → 升级仲裁（③） │
└─────────────────────────────────────────────┘
         │ 只升两样东西上来
         ▼
┌─────────────────────────────────────────────┐
│ 第三阶段：人工仲裁（殿后，只收机器解决不了的）   │
│  ├─ 对抗轮次封顶仍未收敛的分歧                 │
│  ├─ B 复核判不定的争议（含答辩 uncertain）      │
│  争议项两源并列：规则/答辩来源 + 评审来源        │
│  两步裁决（§2.3）：先判"该不该驳回"再定         │
│  accepted/overruled；裁决权在 B，人工执行       │
└─────────────────────────────────────────────┘
         │
         ▼
   ✋ 出口检查点（不是工作阶段，import 时强制）
      validation_gate：宪章先行 + 内联重跑防绕过
      + 无未闭环 error + 无 in_progress 仲裁
      （报告/历史随时可看，不构成流程步骤）
```

**阶段 ↔ 章节映射**：

| 阶段 | 子流程 | 设计章节 | 实现载体 |
|------|--------|---------|---------|
| 一 规则检查 + 答辩 | 语法解析 + 结构规范 + 逻辑一致性 | §2.1.1–2.1.2 | `ttl/validate.rs` |
| | 覆盖矩阵（含增量） | §2.1.3 | `ttl_coverage.rs` |
| | error 纠错循环（≤3 轮） | §2.1.4 | validate → A 修订 → 重跑 |
| | warning 答辩（issue 级单轮，B 机器裁决） | §2.1.5 | `dispute_issue` + `resolve_dispute_llm` |
| 二 对抗验证 | ② 多轮语义评审 + 统一发现池 | §2.2 | `validation_llm.rs` + ReviewPanel |
| 三 人工仲裁 | 两步裁决（只收升级项，两源争议并列） | §2.3 | ArbitrationPanel + resolve_dispute |
| 出口检查点 | import 闸门 | §2.4 | validation_gate（ttl/validate.rs） |
| 横切 | 模型独立性 / 报告历史 | §3 / §5 | IndependenceTier + ValidationReport |

**实施状态**：三阶段 + 出口检查点均已实现。已知的实现简化（不阻断流程）：多轮评审当前
固定单轮 B→A→B（round<max_rounds 的重试分支未做，见 §8）；覆盖矩阵增量 diff 的实体列表
前端仍写死 `["modified"]` 占位（后端已支持 diff_entities 参数）；降级档标注未进验证报告。

**触发原则**：全部子流程均为**用户显式触发**（打开页面不自动跑 LLM/检查；
「运行校验」一次带出阶段①全部检查；对抗与仲裁各自显式进入）。

---

### 1.2 设计目标

**设计针对的缺口**——语法/结构检查只回答「这个 TTL 是不是**合法的本体**」，不回答「是不是**对的**」：① 覆盖遗漏（29 句话声明的对象/规则漏建，AI 最常见失败模式）；② 语义漂移（多次增量更新后偏离 charter 业务意图）；③ 原则违反（skill 方法论红线）；④ 无溯源链（只有错误串，无法结构化回喂修复）；⑤ 单向纠错（无对抗仲裁机制）。详见 §6 实现载体与各阶段设计。

```
输入（29句话 + charter + 术语词典 + 证据源）
        │
        ▼
┌─────────────────────────────────────────────┐
│ 阶段② 对抗验证  多轮语义评审（B 机器裁决）      │ ← 统一发现池
│ 阶段① 规则检查  语法/结构/逻辑/覆盖矩阵/纠错    │ ← error 阻断
│                + warning 答辩（B 机器裁决）    │
│ （扩展规划）    数据落地性 + CQ 反测 + 不变量编译│ ← 未实现，§4
└─────────────────────────────────────────────┘
        │
        ▼
结构化校验报告（含对抗状态）→ 可回喂 AI / 可视化 / 人工终审
```

**核心原则**：

1. **规则可解释、对抗有证据**——检测引擎是确定性代码，error 阻断落库；评审建议默认不阻断（A accepted/rejected 表态流转），B 判不定与 A 拒绝的分歧升级人裁（对齐决策 23 的用户主权精神）
2. **错误结构化回喂**——校验报告不是只给人看，是构造进大模型 A 下一轮修订 prompt 的结构化输入（对齐 modeling-workflow.md 的生成-审计-仲裁闭环，把「大模型 B 审计」中可规则化的部分下沉为确定性检查，降低 LLM 审计负担与幻觉风险）
3. **charter 即规格**——宪章字段天然是语义评审的验收基准，不新增输入格式（charter 业务字段两个：`business_scenario`/`business_essence`；`design_intent`/`invariants` 已废弃不下发，TTL 头部四谓词保留）
4. **证据对抗，不是观点对抗**——每一层交锋都必须带证据（规则引用/原文摘录/反例），不带证据的意见不进入下一轮（对齐 EVIDENCE-CHAIN.md 决策 24 的机械匹配底线）

---

## 二、分阶段设计

按 §1.1 全流程的阶段展开。每个阶段小节内按执行顺序排列子流程。**只收录已实现（或已接入流程）的环节**；未实现的规划维度统一放 §4 扩展章，实现后按性质归位。

### 2.1 阶段①：规则检查 + warning 答辩（检测零 LLM，显式触发）

> 检测环节全部确定性代码，毫秒级；「运行校验」一次带出本阶段全部检查。产出结构化 issue 快照落库（error/warning/suggestion 三级），落库时打 `source='rules'` 标记进统一发现池。答辩（§2.1.5）是阶段①内唯一的 B 介入点——对规则 warning 逐条裁决，不依赖语义评审先行（语义评审在阶段②做本体级大清洗，两者并行不互锁）。检测引擎既不是 A 也不是 B——见「角色命名」末条。

#### 2.1.1 语法解析与结构规范（已实现）

`validate_ttl`（Rust 侧真相源）已实现的确定性检查：

| 检查项 | 内容 | 性质 |
|---|---|---|
| 语法解析 | oxigraph Turtle 解析（含双 # IRI 合法化） | 机器层 |
| 结构规范 | 10 项：`owl:Class` 使用规范、属性不混用、IRI 命名、中文标签语言标签、SWRL 标准词表、subClassOf/subPropertyOf 无环 + 主语类型、悬空属性引用 | 规则层 |

#### 2.1.2 逻辑一致性（已实现）

> 对齐 SKILL.md「核心约束」的 OWL 层要求。实现载体：SPARQL 模板跑在 oxigraph temp store 上（与结构规范同机制，`check_*` 函数族扩展）。

| 检查 | SPARQL 策略 | 严重级 |
|---|---|---|
| domain/range 违反 | 属性声明了 `rdfs:domain X`，但存在三元组 `<s prop ?>` 且 `s rdf:type Y`、`Y 与 X 无 subClassOf 关联` | error |
| 基数约束违反 | `owl:cardinality`/`qualifiedCardinality` 声明 vs 实际实例三元组计数 | error |
| disjoint 冲突 | `owl:disjointWith` 的两类同时被同一实例断言 | error |
| FunctionalProperty 多值 | functional 属性对同一 subject 有多个不同 value | error |
| 对称/传递属性自反冲突 | symmetric 属性缺反向三元组、transitive 属性断链（warning 级） | warning |

> 完整 OWL DL 推理（hermit 级）**不做**——Rust 生态无成熟推理机，且性价比低。上表覆盖 AI 生成最容易犯的 5 类。


#### 2.1.3 覆盖矩阵（含增量，已实现）

- **正向覆盖**：从 29 句话（7 类逐条）+ charter `business_scenario`/`business_essence` 抽取业务名词 → 术语词典比对 → 每个名词是否落地为对应实体（类/属性/规则）。**这是完整性检查的确定性打底**：名词级「建没建」由矩阵机械判定；29 句话中 `tentative` 项的「不入库」校验（应进 `open_questions.json` 而非 Turtle）同属输入比对，随矩阵一并执行，语义评审（§2.2）不重复报矩阵已列出的缺失，只补语义级遗漏（见下节）
  - 产出物：`29句话条目 × Turtle 实体` 的映射表，缺失项高亮
- **反向孤儿**：Turtle 中无任何 incoming/outgoing 断言的「孤儿类」、无 domain 关联的属性 → 疑似 AI 幻觉或残留
- **增量场景（迭代更新）**：本体是增量演进（export → 改 → validate → import overwrite），覆盖矩阵不必每次全量重跑：
  - diff 输入：复用 changelog 的 `change_summary` JSON（`{"modified":["Anchor","StoreManager",...]}`）——已实现，直接作为「本次变更内容」
  - 增量校验范围：只对 `change_summary` 中变更的实体及其关联（直接邻居 + 依赖属性路径）跑覆盖检查，避免全量噪音
  - 全量覆盖矩阵保留为「冷启动 / 定期回归」模式，增量时折叠展示


#### 2.1.4 error 纠错循环

error 级 issue 是确定性违反，**无辩空间**——直接进入修订循环：

```
validate error → 构造修订 prompt（引用 principle.ref 逐条修订）→ A 修订 → 重新 validate
                                                                      ↓
                                                          通过 → 进入阶段② ｜ 超 3 轮 → 转人工
```

- A 单方修正，B 不参与（确定性问题的修复没有对抗价值）
- 回喂闭环的完整数据流见 §5.3

#### 2.1.5 warning 答辩（issue 级，单轮定胜负，已实现）

规则检查产出的 warning 级 issue 由 **A（建模者）**引用输入证据进行**反驳**，构成「规则级对抗」（error 级是确定性违反，不可辩）——**答辩的发起方是 A，不是人**：被指控的是 A 的建模产物，翻案权自然在 A；裁决权在 B；人只在阶段③ 仲裁 B 判不定的残留。v0.7 起答辩归阶段①——规则 warning 一产出即可逐条反驳，不必等语义评审先行；语义评审（阶段②）关注本体级问题，两者并行不互锁。

**A 自动答辩链（`run_auto_dispute`，后端一次跑完）**：对本体的 rules 来源 open warning——

```
环节 1  A 决策（run_a_dispute）：逐条决定 dispute / accept
        输入 = warning 清单 + 证据源全文（id+标题+内容）
        dispute 必须选源 + 逐字摘录 quote（≤200 字）+ argument
        无证据支撑的 warning → accept（不硬编——编造会被环节 2 当场驳回）
环节 2  提交（dispute_issue）：机械匹配把关，摘录对不上源 → 驳回维持 open
环节 3  B 裁决（resolve_dispute_llm）：overruled 撤销 / accepted 升级 error /
        uncertain 保持 disputed 进人工仲裁
```

- A 漏答的条目按 accept 保守处理（不落 disputed）
- DisputeModal 保留为**人工精修入口**（替 A 补答辩/复核个别条目），默认路径是 A 自动答辩
- UI：阶段①答辩区「交 A 答辩（N warning）」按钮一键触发，结果折叠展示（撤销/驳回/待人工/接受/摘录未匹配 逐条列明）

```
规则触发 warning
     │
     ├─ A 无异议 → 更新（常规修正）
     │
     └─ A 有异议 → 提交 dispute（含 argument + counterEvidence）
           │
           ▼
     裁决分两步（缺一不可）：
     ① counterEvidence 机械匹配（代码层，快）
        —— 必须引用已登记的 evidence_source ID + quote 通过三级匹配
        —— 匹配失败 = 该证据不成立，dispute 直接驳回（不进入②）
     ② argument 可信度判定（LLM 层，B 机器裁决）
        —— 机械匹配通过后，A 的辩护理由是否成立由 B 判定（resolve_dispute_llm）
        —— B 确认 → overruled（撤销）；B 不认可 → accepted（升级 error）
        —— B 判不定（uncertain）→ 保持 disputed，进人工仲裁（阶段③）
```

**实现逻辑**：
- `ValidationIssue.status` 初始为 `open`
- 答辩提交 `dispute_issue(id, argument, counter_source_id, counter_quote)`：A 自动链（环节 2）或人工精修（DisputeModal）都走此口，状态置 `disputed` 后自动触发 B 机器裁决
- **裁决归属**：`resolve_dispute` 不是一步代码判断——它先由 Rust 层做 counterEvidence 的机械匹配（复用 `match_quote` 三级匹配，对齐 EVIDENCE-CHAIN.md），匹配通过后再由 B（`resolve_dispute_llm`，走校验者独立 ChatService）判定 argument 的自然语言可信度。**argument 不是机器可判的，裁决权在 B，不在代码，更不在 A 自己**——否则 A（同一个 LLM）等于自己给自己翻案
- 被 `overruled` 的 issue 不进入最终报告，但记入审计日志供回溯
- 本机制只对 **rules 来源的 warning 级**开放——error 级是阻断性问题，不可辩（语法不通过、逻辑不成立，不存在辩护空间）；review 来源的 B 评审发现不适用本通道（A 对 B 评审建议的异议走 §2.2 的修订说明 rejected → 仲裁，设计如此：评审分歧是 A/B 对抗，不是 A 对规则引擎的翻案）
- 降级模式（单 provider，见 §3.2）下 B 与 A 同模型，答辩独立性打折，需在报告中标注「降级裁决」

---

### 2.2 阶段②：对抗验证（多轮语义评审，B 裁决机器能解决的分歧）

> 阶段② = 多轮语义评审（本体级大清洗）。warning 答辩已前移至阶段①（§2.1.5），与本阶段并行不互锁。B 的评审发现不再只留在 `b_report` JSON 里——每条转写为 issue 行进**统一发现池**，与规则发现同池同生命周期；A 修订时逐条表态（accepted/rejected），rejected 的发现自动升级人工仲裁（§2.3）。

#### 2.2.1 ② 多轮语义评审（本体级）+ 统一发现池

> 这是原方案中「多轮对抗迭代」的核心落地层，区别于单次审计。角色：B = `Verifier`（校验者）、A = `Generator`（建模者）。

- **输入**：charter + 29 句话 + Turtle + 规则检查报告 + 覆盖矩阵 + 答辩记录（dispute 状态）+ 已登记的 evidence 源上下文
- **任务**：评审规则查不出的语义问题——概念混淆、粒度不当（类该拆该并）、业务贴合度、建模最优性
- **输出**：结构化建议清单（定位到实体 IRI + 问题类型 + 修改建议 + 证据引用），**不阻断落库**——且每条建议同步转写为 `source='review'` 的 issue 行（统一发现池，见下）
- **多轮对抗流程**：

```
第一轮：B 评审
  └─ 输出建议清单（每条含 id + evidence，引用 charter/29句/输入证据）
  └─ save_b_report 同步转写为统一发现池 issue 行（id="{review_id}:{B条目id}"）

第二轮：A 修订（基于 B 建议）
  └─ 输出 Turtle V2 + 修订说明（逐条注明接受/拒绝及理由，issue_id 回显 B 的 id）
  └─ a_revise 把修订说明逐条回填 issue.a_action；
     rejected 条目 → status=disputed → 自动进人工仲裁队列

第三轮：B 复核（≤2 轮封顶）
  └─ B 看到 A 的修订内容（不含修订思考过程）
  └─ 确认 → 通过 | 追问 → 分歧点 → 人工仲裁
  └─ b_verdict（B 复核原始输出 JSON）+ step_ms_*（各环节耗时）落库

约束：
- B 的 prompt **包含**第一轮的 evidence 引用（校验方比被校验方信息更多，这是对抗的关键逆直觉——审计者拥有企业文档/数据库等额外证据源）
- B **不包含** A 的建模方法论 prompt（§3.1 硬性约束）
- 第三轮 B 复核时 **能看到 A 的输出**但看不到 A 的修订说明/辩护词——防止 B 被 A 的逻辑同化
- 编排为后端 tokio 任务（run_review_loop）：切页面/最小化不中断，前端轮询恢复进度
```

- **实现载体**：复用 modeling-workflow.md 的「大模型 B 审计」角色，但 B 的 prompt 增加 evidence 检索上下文和 A 修订版本输入
- **与覆盖矩阵的分工（去重）**：完整性与覆盖矩阵目标相同（查 29 句话覆盖遗漏）、手段互补——矩阵做**名词级机械比对**（确定性、零成本），B 做**语义级判断**（该名词建的类型/粒度对不对、规则与约束类遗漏——名词抽取抓不住的部分）。B prompt 已注入矩阵结果，**完整性维度以矩阵为底、只报语义级增量**，不重复罗列矩阵已高亮的缺失项

**统一发现池（v0.7 核心）**——B 评审发现与规则发现同池同生命周期：

| 机制 | 说明 |
|---|---|
| `source` 列 | `validation_issues.source` = `'rules'`（阶段①规则检查）\| `'review'`（② B 语义评审） |
| 转写（B 报告 → issue 行） | `save_b_report` 时把 B 建议清单逐条 INSERT：id=`{review_id}:{B条目id}`、layer=`L4`、rule=`review:{dimension}`、severity 由 B 声明（未知值降 suggestion）、evidence_ref 存 principle_source='review' |
| 按源分域替换 | 重跑规则检查只清 `source='rules'` 的行——review 域的生命周期由评审状态机管理，规则重跑不冲掉 |
| A 逐条表态（a_action） | A 修订说明 JSON 逐条回填：accepted → issue.status=accepted；**rejected → status=disputed，自动进仲裁队列**（A 的异议不是翻案通道，是升级仲裁的证据） |
| id 匹配容错 | A 回显的 issue_id 与 B 的 bid 做归一化双向包含匹配（实测 A 会自造编号），最长 bid 优先 |
| 展示 | ReviewPanel 渲染「B 建议 → A 裁决」成对卡片（接受绿/拒绝红 + 理由）；ArbitrationPanel 队列中 review 来源条目标 `评审·{bid}`，与规则/答辩来源的争议并列 |

> 设计取舍（2026-09-23 用户裁定）：**答辩该合的是"发现池"不是"检测引擎"**——阶段①的检测引擎保持零 LLM（import 闸门/成本/确定性三理由），但两类发现的**生命周期**统一：都能被 A 表态、都能进仲裁、都在同一张表流转。检测与对抗是流水线上的两段，发现的归宿是同一个池。

**B 评审 prompt 规格（review_ontology 的输入规范，承接原方案审计模板）**：

```
【角色】本体质量审计专家（Verifier）。独立审查，不套用建模方法论。
【输入】
  1. charter（business_scenario/business_essence）
  2. 29 句话（7 类逐条）
  3. Turtle 本体
  4. 规则检查报告 + 覆盖矩阵 + 答辩记录（dispute 状态）
  5. 已登记的 evidence 源上下文（企业文档/DB schema 摘录）
【审计维度】（对齐原模板八维度，逐条过）
  ① 事实忠实度：每个断言是否有企业证据支持？（Prefer Counterexamples——
     优先找反例："这个类在真实数据里没有实例""这个属性映射的列 90% 为空"）
  ② 完整性：29 句话每条是否都有对应产出？缺了哪个对象/规则？
  ③ 语义正确性：概念定义是否准确？边界是否清晰？有无概念混淆？
  ④ 结构合理性：层级/关系/属性组织是否合理？类该拆该并？
  ⑤ 逻辑一致性：是否存在冲突？能否通过推理验证？
  ⑥ 约束正确性：基数/唯一性/枚举是否合理？
  ⑦ 数据可落地性：能否映射真实数据？（联动数据可落地性比对结果（规划，§4.1））
  ⑧ 业务可用性：能否支撑核心业务问题？（联动 CQ 反测结果）
【输出】严格 JSON 数组（建议清单）：
  [{ id, iri, dimension, type: "missing"|"conflict"|"granularity"|"semantic"|"grounding",
     severity: "error"|"warning"|"suggestion", message, evidence_ref, suggestion }]
  - id：建议编号（B1、B2…），修订说明需按 id 逐条回应，必须填写
【约束】
  - 不带证据的批评不输出（每条必须引用 charter/29句/evidence 源）
  - 不修改本体，只评审
  - 同一概念多次出现时合并为一条，不重复
```

> 不补这个规格，B 的质量全凭实施时即兴发挥——本规格是 `review_ontology` 的硬输入，随 skill 的审计视角 prompt 模板沉淀（独立于 modeling-workflow.md 旧模板，新增 evidence/CQ 维度）。

> 语义评审必须在规则检查之后运行——LLM 只看规则筛过的问题域，报告才可信且不噪音化。


### 2.3 阶段③：人工仲裁（殿后，只收升级项）

> **人只出现在最后**——人工注意力是最稀缺资源，A/B 机器交互能达成一致的不到人。

**三个升级入口**（仲裁队列中三源并列，标签区分来源）：

1. **对抗轮次封顶**：② 多轮评审超上限（B 复核 >2 轮）仍未收敛的分歧——评审任务整体 escalated，B 复核原始输出 `b_verdict`（含 remaining_issues）供仲裁时查看
2. **A 拒绝评审建议**：统一发现池中 review 来源 issue 被 A rejected → status=disputed，标签 `评审·{bid}`（v0.7 新增——A 的拒绝不是翻案，是把分歧升级给人）
3. **答辩 B 判不定**：①c 答辩中 B 机器裁决返回 uncertain 的争议 issue（标签 `答辩·{id}`），保持 disputed 等人工

**两步裁决（缺一不可）**：

```
① 阅读分歧双方论据（A vs B 的结构化主张 + 证据）
② 裁决 accepted / overruled
```

**裁决归属**：机械匹配在 `dispute_issue` 提交时完成（§2.1.5 步骤①），裁决阶段由 B（`resolve_dispute_llm`，独立 ChatService）判定 argument 的可信度。**argument 不是机器可判的，裁决权在 B，不在代码，更不在 A 自己**——否则 A（同一个 LLM）等于自己给自己翻案。人工执行的是 B 已给出的裁决方向（B 判 uncertain 的保持 disputed 等人）。

**对抗的核心原则**：

1. **证据对抗，不是观点对抗**——每次交锋必须带证据（规则引用/原文摘录/反例），不带证据的意见不进入下一轮
2. **轮次有限，防死循环**——error 纠错 ≤3 轮、语义评审复核 ≤2 轮、答辩 1 次（失败升级），超出即转人工，保护用户主权（决策 23）
3. **仲裁兜底**——模型间无法达成一致时，分歧点以结构化形式（issue + 双方主张 + 证据）提交，专家可一键裁决

#### 2.3.1 状态机流转（issue 生命周期）

一条 issue 从产生到裁决的完整生命周期：

```
validation_issues[]（统一发现池，source=rules|review）
    │
    ├─ rules · severity=error ──→ blocking，不入 dispute（直接进 A 修订 prompt，§2.1.4）
    │
    ├─ rules · severity=warning ──→ A 可选调用 dispute_issue(id, argument, counter_evidence)
    │     │                        ↓
    │     │                 resolve_dispute（两步裁决，见 §2.1.5）：
    │     │                   ① counterEvidence 机械匹配（match_quote 三级匹配）
    │     │                      匹配失败 → 直接驳回（不进入②）
    │     │                   ② argument 可信度由 B 机器裁决（resolve_dispute_llm）
    │     │                      B 认可 → status=overruled（撤销）
    │     │                      B 不认可 → status=accepted（升级 error 阻断）
    │     │                      B 判不定 → 保持 disputed → 人工仲裁（阶段③）
    │     │
    │     └── A 不 dispute → 常规修正（status=accepted）
    │
    └─ review · severity=任意 ──→ B 语义评审（ValidationReview 多轮状态机）转写产生
               │
               │  round=1: B 评审（prompt_text + evidence 上下文）
               │      ↓ 输出 b_report → 同步转写 issue 行（source=review）
               │  round=2: A 修订（a_revise）
               │      ↓ 输出 a_revision（Turtle V2）+ a_reasoning
               │      ↓ 修订说明逐条回填 a_action：
               │         accepted → status=accepted
               │         rejected → status=disputed → 仲裁队列（标签 评审·{bid}）
               │  round=3: B 复核（b_recheck）
               │      ↓ 只看修订输出，不看 reasoning；b_verdict + step_ms 落库
               │    ┌─ 确认 → status=resolved → import 落库
               │    └─ 追问 → status=escalated → 分歧点入队列 → 人工仲裁
               │
               └── 任意轮超限 → escalate
```

---

### 2.4 出口检查点（不是工作阶段，import 时强制，已实现）

> **import 校验门禁**：`validation_gate`（`ttl/validate.rs`）在 import 工具（`import_ontology_ttl_tool`——A 的 DynamicTool 落库路径）内联执行，与「宪章先行」「证据」闸门串联：
> - **内联重跑 validate**（旁路堵漏）：对传入 TTL 直接跑一次 `validate_ttl`，error 非空即拒绝——validate 是纯函数，重跑即防绕过，无需签名比对存储
> - **未闭环的 error issue**（`validation_issues` 中 severity=error 且 status ∈ {open, disputed}，**不分 source**——review 来源的 error 发现同样拦）→ 拒绝导入
> - **未 resolved 的 review**（`validation_reviews` 中 status=in_progress）→ 拒绝导入，提示先完成对抗流程或人工裁决
> - warning 不拦（与「warning 可继续 import」设计一致）
>
> 已知缺口：前端 Tauri 命令 `import_ontology_ttl`（`commands/ontology.rs`）直连 `store.import_ttl` 不过此闸——UI 落库路径待补（P2）

---

## 三、模型独立性约束（贯穿 LLM 环节：①c 答辩裁决 + ② 语义评审 + ③ 仲裁）

### 3.1 硬性约束

建模与审计**必须由不同模型完成**（对齐 modeling-workflow.md 双模型设计，本条为实施层面的硬化约束；A=`Generator`、B=`Verifier`）：

1. **A/B 必须跨供应商或至少跨模型家族**：同系列模型共享训练偏差与思维定势，A 漏掉的概念混淆 B 大概率也漏掉。例：A 用 Claude，B 用 GPT/Gemini/DeepSeek；禁止同底座不同版本互审
2. **B 的 prompt 隔离**：B 的系统指令**不得包含** A 的建模方法论（ontology-modeling-w3c skill 内容）。B 使用独立的审计视角（批判性推理、找矛盾），而不是用同一套方法论做「合规性复读」——否则等于 A 审自己
3. **B 不看 A 的中间产物**：B 的输入只有「charter + 29 句话 + Turtle + 规则检查报告」，不携带 A 的生成过程说明/辩护词，避免锚定效应
4. **回喂单向**：A 修订时可以看到 B 的报告，B 审计时看不到 A 的历史修订理由，保持审计方信息劣势（审计者应比被审计者信息更少，而非更多）

> 确定性层（阶段①检测引擎）由 Rust 代码执行，天然满足独立性；本约束仅针对 LLM 环节（② 语义评审与 ①c 答辩裁决）。

**输入约束的轮次区分（消除与 §2.2 的表述冲突）**：
- **第一轮（初始评审）**：B 输入只有「charter + 29 句话 + Turtle + 规则检查报告」，不携带 A 的任何生成过程说明（§3.1 第 3 条）
- **复核轮（对抗迭代）**：B 额外看到 A 的**修订输出**（Turtle V2），但仍**不看到** A 的修订说明/辩护词/思考过程——这保证对抗是「产物对产物」的客观博弈，而非「解释对解释」的修辞博弈
- 证据优势：B 在第一轮就拥有 evidence 引用（企业文档/数据库证据），信息上强于 A；A 靠 counterEvidence 答辩才能反转——这正是「校验方举证、建模方反证」的对抗结构，与 §3.1 第 4 条「审计方信息劣势」的旧表述不同（旧表述只适用于**第一轮**，修订为上述轮次区分后更精确）

### 3.2 单 provider 降级路径（现实约束）

**现状冲突**：用户可能只配置了一个 provider（如火山 glm-5.3-flash），硬约束「A/B 跨供应商」在单 provider 下使 对抗验证 无法执行。需要降级策略，按可用 provider 数量分三档：

| 档位 | provider 情况 | B 的配置 | 对抗有效性 | 报告标注 |
|---|---|---|---|---|
| 完整 | ≥2 供应商 | 跨家族模型（§3.1 全约束） | 高 | — |
| 降级 | 单供应商多模型 | 同供应商不同模型家族（如 GLM + 豆包） | 中 | ⚠ 降级-同供应商 |
| 兜底 | 单 provider 单模型 | 同模型 + 全新会话 + 独立审计 prompt + temperature 拉高（0.7-0.9） | 低 | ⚠ 降级-同模型（对抗有效性低） |

**兜底档的约束补强**（对抗有效性低，必须显式降级）：
- B 的输入构造与 A 完全隔离：全新会话、无 A 的上下文、无 A 的中间产物——只给「charter + 29 句话 + Turtle + 报告」
- B 的审计 prompt 与 A 的建模 prompt 完全独立（§3.1 第 2 条仍生效）
- **降级运行必须在报告中标注风险等级**，语义评审 结论降权为「建议」（兜底档下 B 的语义评审仅作参考，人工终审权重上调）
- 连锁影响：warning 答辩的 argument 裁决（§2.1.5 步骤②）在兜底档也由同模型判定，答辩独立性打折——同步标注「降级裁决」
- 产品层：设置页提示「配置第二个 provider 可获得完整对抗验证」；升级到完整档后历史降级报告保留标注不回溯

> 降级不是设计妥协的遮羞布，而是「无独立性宁可明示降权」——用户主权（决策 23）优先于自动化完成度。

---

## 四、扩展校验维度（规划中，未实现）

> 以下维度**不在三阶段流程内**——均未实现（P1–P3，见 §8）。放这里是为了设计完整性：
> 它们要么依赖未建设的基础（CQ 模板库、datasets 比对管道、SHACL 编译），要么含 LLM/半自动成分，
> 不满足阶段①「确定性、毫秒级」或阶段②「B 裁决」的进入条件。实现后按性质归入对应阶段。

### 4.1 数据可落地性（规则 + 数据源比对）

> 对齐原方案校验维度「G 数据可落地性」：**能否映射真实数据**。复用 onto-studio 已挂载的 `datasets` / `data_sources` 能力（`list_ontology_datasets` / `list_ontology_data_sources` 已存在），用真实数据给本体找反例——这是原方案六维度缺掉的一环，也是方案图「数据可落地性」维度的工程化。

| 检查 | 策略 | 严重级 |
|---|---|---|
| 类无实例支撑 | 本体声明了 `owl:Class X`，但挂载数据源中不存在任何映射到 X 的表/列/记录（如 `backing_mapping` 缺失） | warning |
| 属性空值率高 | `owl:DatatypeProperty` 映射的数据列 90%+ 为空 → 疑似过度建模或错误映射 | warning |
| 属性类型不匹配 | 属性 `rdfs:range xsd:integer` 但真实数据列为字符串/日期 | error |
| 映射断裂 | 数据源 schema 变更后（列改名/删除），本体与数据的映射引用悬空 | error |

**输入**：本体 Turtle + `datasets`/`data_sources` 元数据（含列名、类型、非空统计）；不要求实际拉取全量数据（成本可控），有抽样统计即可。

> 与 evidence.rs 的关系：数据源本身可作为 `kind="db_table"/"db_column"` 的证据源登记，反例证据（"这个类在真实数据里没有实例"）也走 Evidence 三级匹配落库。


### 4.2 业务可用性 / CQ 反测（规则 + LLM 混合）

> 对齐原方案校验维度「H 业务可用性 + Competency Questions（CQ）」：**是否能支持核心业务问题**。本体通过了全部语法/逻辑检查，不代表能回答业务问题——CQ 反测用业务问题打本体，验证"本体是否对的"最贴近业务的一环。

- **CQ 来源**：charter `business_scenario`/`business_essence` + 29 句话 + 专家预置黄金用例（modeling-workflow.md §5「黄金测试用例」）
- **SPARQL 模板反测**：每条 CQ 编译为 SPARQL 模板 → 在 oxigraph temp store 跑 → 检查能否返回非空/正确形状的结果。无法回答 = 本体缺类/缺属性路径/缺规则
- **两级执行**：
  - 模板级（确定性）：CQ → SPARQL 模板 → 跑出结果 | error（SPARQL 语法/空结果）
  - 语义级（LLM）：模板跑通但结果语义不符（答非所问）→ 交 B（语义评审）复核 | suggestion
- **产物**：本体的第 7 类 SPARQL 模板本身就是 CQ 反测的载体——一个本体"自带业务问题验证集"，每次校验可回归

> 排期考量：CQ 反测依赖 CQ 模板库的建设（专家预置 + AI 生成），实现成本偏高，放 P2 合理。


### 4.3 宪章不变量编译（半自动）

> 回应「不变量核验是不是一个独立环节」：**不是**。编译是宪章保存时的一次性准备动作（人工确认后落库），编译产物之后随阶段①的规则检查自动执行——它本身不占用流程中的位置。

- charter `invariants` 是自由文本，**建模时人工把每条不变量确认为可执行的 SHACL/SPARQL 约束**（一次性成本），之后每次生成自动执行
- 存储：charter 表扩展 `compiled_constraints` 字段（或附属 JSON），与 charter 同生命周期，AI 不可写（对齐决策 23）
- **写路径（补强）**：用户在前端 CharterPanel 保存宪章时，同时展示 invariants 列表 → 每条可选「生成编译约束」→ 由编译动作（前端调 `compile_invariant(iri, invariant_text)` 命令 → 后端 LLM 生成 SHACL/SPARQL + 人工确认）产出 `compiled_constraints`。编译是人确认后落库，AI 只提草稿不直接写（对齐决策 23）；UI 落点见 §6.3 答辩面板同层级的「不变量编译」入口

---

## 五、结构化校验报告与数据模型

### 5.1 报告 schema（替换现有 `TtlValidation` 的 string[] 形态，向后兼容保留）

```typescript
type ValidationIssue = {
  id: string;                    // 稳定 ID；rules 来源如 "vi-xxx"，review 来源为 "{review_id}:{B条目id}"
  layer: string;                 // 实现层标记（"L0"语法 … "L4"评审），仅供筛选定位，不是流程概念
  source: "rules" | "review";    // 发现来源（统一发现池 v0.7）：rules=阶段①规则检查，review=② B 语义评审
  severity: "error" | "warning" | "suggestion";
  rule: string;                  // 机器可读规则名（check_* 函数名；review 来源为 "review:{dimension}"）
  message: string;               // 人读描述
  target?: string;               // 涉及实体 IRI / 行号
  evidence?: {                   // 证据引用（对应 evidence_sources 表）
    source_id: string;           // FK → evidence_sources（register_source 返回值）
    quote: string;               // 原文逐字摘录（≤200 字符；答辩机械匹配用）
  };
  principle?: {                  // 溯源：违反了哪条输入/原则（review 来源为 source='review'）
    source: "skill" | "charter" | "input" | "w3c" | "review";
    ref_: string;                // 如 "SKILL.md §核心约束-4" / B 建议的 evidence_ref
  };
  suggestion?: string;           // 修复建议（直接可进 AI 修订 prompt）
  status: "open" | "disputed" | "accepted" | "overruled";  // 对抗生命周期状态
  dispute?: {                    // A 的辩护理据（rules 来源 warning 专属，disputed 时非空）
    argument: string;            // A 引用输入的辩护理由
    counter_evidence: {          // 反证：已登记源 + 逐字原文（机械匹配对象）
      source_id: string;
      quote: string;
    };
    resolved_by?: "verifier" | "human";
    verdict?: "overruled" | "accepted";
  };
  a_action?: {                   // review 来源专属（v0.7）：A 对 B 建议的逐条表态
    action: "accepted" | "rejected";  // rejected → status=disputed → 仲裁队列
    reason: string;              // rejected 时 = 拒绝依据（B 复核不可见）
  };
};

type ValidationReview = {
  id: string; ontology_iri: string;
  round: number; max_rounds: number;   // 1=B评审 2=A修订 3=B复核；复核封顶 2
  b_report: string;                     // B 评审输出（结构化建议 JSON）
  a_revision: string;                   // A 修订后的 Turtle V2 全文
  a_reasoning: string;                  // A 修订说明（B 复核不可见）
  b_verdict: string;                    // B 复核原始输出 JSON（verdict + remaining_issues；v0.7 落库）
  step_ms: {                            // 环节耗时（unix ms，0=未完成；v0.7 落库）
    b_review: number; a_revise: number; b_recheck: number;
  };
  status: "in_progress" | "resolved" | "escalated";
  created_at: number; updated_at: number;
};
```

> 兼容策略：`TtlValidation` 保留 `errors: Vec<String>` / `warnings: Vec<String>` 字段不删（旧消费者不受影响），新增 `issues: Vec<ValidationIssue>` 结构化字段。

### 5.2 存储层

复用 TtlStore 同 SQLite 库（与 `evidence_sources` / `element_evidence` / `ontology_changelog` 同库同生命周期），两张表（DDL 对齐 `schema.rs` 实际实现）：

```sql
-- 统一发现池：rules 与 review 来源的 issue 同表同生命周期（v0.7）
CREATE TABLE IF NOT EXISTS validation_issues (
    id                     TEXT PRIMARY KEY,            -- 稳定 id：vi-{uuid}；review 来源为 "{review_id}:{B条目id}"
    ontology_iri           TEXT NOT NULL,
    validation_id          TEXT NOT NULL,               -- 同一次 validate 分组（review 来源 = review_id）
    layer                  TEXT NOT NULL,               -- 'L0'|'L1'|'L2'|'L2.5'|'L2.6'|'L3'|'L4'
    source                 TEXT NOT NULL DEFAULT 'rules', -- 'rules'|'review'（统一发现池 v0.7）
    severity               TEXT NOT NULL,               -- 'error'|'warning'|'suggestion'
    rule                   TEXT NOT NULL,               -- check_* 函数名；review 来源 'review:{dimension}'
    message                TEXT NOT NULL,
    target                 TEXT,
    evidence_source_id     TEXT,                        -- FK→evidence_sources
    evidence_quote         TEXT,                        -- 证据原文（denormalize 免 JOIN）
    principle_source       TEXT,                        -- 'skill'|'charter'|'input'|'w3c'
    principle_ref          TEXT,
    suggestion             TEXT,
    status                 TEXT NOT NULL DEFAULT 'open', -- open|disputed|accepted|overruled
    a_action               TEXT,                        -- review 来源：A 的 accepted/rejected JSON
    dispute_argument       TEXT,
    dispute_counter_source TEXT,
    dispute_counter_quote  TEXT,
    dispute_resolved_by    TEXT,                        -- 'verifier'|'human'
    dispute_verdict        TEXT,                        -- 'overruled'|'accepted'
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_vi_source ON validation_issues(ontology_iri, source);

-- 多轮语义评审状态机：B 评审 → A 修订 → B 复核（后端驱动，切页不中断）
CREATE TABLE IF NOT EXISTS validation_reviews (
    id            TEXT PRIMARY KEY,            -- 稳定 id：vr-{uuid}
    ontology_iri  TEXT NOT NULL,
    round         INTEGER NOT NULL DEFAULT 1,
    max_rounds    INTEGER NOT NULL DEFAULT 2,
    b_report      TEXT,                        -- B 的评审输出（结构化建议 JSON）
    a_revision    TEXT,                        -- A 修订后的 Turtle V2 全文
    a_reasoning   TEXT,                        -- A 的修订说明（B 复核不可见）
    b_verdict     TEXT,                        -- B 复核原始输出 JSON（v0.7）
    step_ms_b     INTEGER NOT NULL DEFAULT 0,  -- B 评审耗时 ms（0=未完成）
    step_ms_a     INTEGER NOT NULL DEFAULT 0,  -- A 修订耗时 ms
    step_ms_re    INTEGER NOT NULL DEFAULT 0,  -- B 复核耗时 ms
    status        TEXT NOT NULL DEFAULT 'in_progress', -- in_progress|resolved|escalated
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
```

> **存量库迁移**：`migrate_validation_unified_pool`（schema.rs，ttl/mod.rs 同步一份）对旧库补列（source/a_action/b_verdict/step_ms_*）并回填 `source='rules'`。
> **实现坑**：`ttl/mod.rs` 的 `init_ttl_schema` 有一份同构 DDL 副本（in-memory 测试路径），**改表两处必须同步**。

### 5.3 回喂闭环（对抗式，非纯单向）

```
生成 → validate → issues(error 非空)
        │
        ▼
error 级：构造修订 prompt：「以下 N 条违反 <principle.ref>，逐条修订后重新输出完整 Turtle」
        │
        ▼
warning 级：进入 warning 答辩（§2.1.5，阶段①内）——A 可 dispute（引输入证据反驳）或直接修订
        │
        ▼
大模型 A 修订 → 重新 validate → 循环（上限 3 轮，超出转人工）
        │
        ▼
多轮语义评审 → 建议清单（不阻断）→ 用户确认 → A 修订 → B 复核（≤2 轮）
```

对齐 modeling-workflow.md 的「逻辑博弈」环节：确定性规则检查的 error 直接作为 A 的修订输入；rules 来源 warning 开放答辩（§2.1.5）；LLM 语义博弈（§2.2）不限定严重级——B 评审发现的 severity 由 B 声明，A rejected 的分歧进仲裁。

---

## 六、实现载体

### 6.1 命令 / Agent 工具清单

每个能力同时暴露为 **Tauri command**（前端 UI 调用）与 **DynamicTool**（agent loop 内调用，沿 `ontology_ttl_tools.rs` 的 `DynamicTool::new` 闭包风格注入）：

| 能力 | 职责 | 所属环节 |
|---|---|---|
| `validate_ttl(content)` | 确定性规则检查（已有，扩展 issues 字段；快照带 source='rules'） | 阶段① |
| `run_auto_dispute(iri)` | A 自动答辩链：A 决策 → dispute_issue → B 裁决，对全部 rules 来源 open warning 后端一次跑完 | 阶段①（§2.1.5） |
| `dispute_issue(id, argument, counter_source_id, counter_quote)` | 答辩提交口（自动链环节 2 / 人工精修同款）：先机械匹配（Rust 内联，失败即驳回），通过置 disputed | 阶段①（§2.1.5） |
| `resolve_dispute(id, verdict, resolved_by)` | 落裁决（verifier/human）；**不作为 A 的工具暴露**——A 只有 `dispute_issue`，裁决权在 B/人工，防自我翻案 | 阶段①/③ |
| `resolve_dispute_llm(id, model_b)` | B 机器裁决：overruled/accepted 落库，uncertain 保持 disputed 进仲裁 | 阶段①（§2.1.5） |
| `review_ontology(iri)` | 启动 B（Verifier）语义评审，创建 ValidationReview | 阶段② |
| `run_review_loop(iri, ttl, charter, issues)` | 后端 tokio 任务一次跑完 B评审→A修订→B复核，每步落库；切页不中断，前端轮询 | 阶段② |
| `a_revise(review_id, ttl, reasoning)` | A 提交修订（Turtle V2 + 说明）；a_reasoning 逐条回填 a_action，rejected→disputed | 阶段② |
| `b_recheck(review_id, confirm, verdict)` | B 复核（只看修订输出，不看 reasoning）；b_verdict + step_ms_re 落库 | 阶段② |
| `escalate(review_id)` | 分歧转人工仲裁，进入仲裁队列 | 阶段②→③ |

> import 校验门禁的完整规则见 §2.4 出口检查点——`import_ontology_ttl_tool`（DynamicTool）内联检查；前端 Tauri 命令路径待补（§8 P2′）。

### 6.2 关键复用点（零新基建）

| 已有设施 | 位置 | 复用方式 |
|---|---|---|
| 证据链机制 | `crates/ontology-store/src/evidence.rs` | `ValidationIssue.evidence` 的 `source_id` 即 `register_source` 返回值；`quote` 走 `match_quote` 三级匹配（Exact/Normalized/None）校验 |
| 非流式 LLM 调用 | `crates/agent-core/src/chat.rs` `prompt_text()` | B 评审 / A 修订都走同一管道，仅 prompt 与 model 不同（A 建模视角 / B 独立审计视角） |
| 规则框架 | `ttl.rs` 的 `check_*` 函数族 | 逻辑一致性等新规则照此模式：SPARQL 跑在 oxigraph temp store |
| 证据源登记 | `ontology_ttl_tools.rs` `register_evidence_source_tool` | 已在 agent 工具组中，语义评审直接加载已登记源作上下文 |

### 6.3 前端落点

| 位置 | 内容 |
|---|---|
| 校验任务·阶段① | 「运行校验」→ issue 快照卡片 → 覆盖矩阵内嵌 → **答辩区**（warning 卡片带「答辩」入口，DisputeModal 提交后 B 自动裁决）——检测与答辩同页（v0.7） |
| 校验任务·阶段② | ReviewPanel：三环节自动轮转进度（环节耗时展示）+「B 建议 → A 裁决」成对卡片（接受绿/拒绝红 + 理由） |
| 校验任务·阶段③ | ArbitrationPanel：仲裁队列三源并列（轮次封顶 / 评审·{bid} / 答辩·{id}），双方论据并排 + B 复核 verdict 折叠 |
| 验证报告抽屉 | 三栏统计（规则+答辩 / 对抗验证 / 覆盖）+ 对抗轨迹 + issue 明细 + 复制 Markdown |
| 历史抽屉 | 每次校验的报告存档（与 changelog 关联），含对抗状态变化轨迹 |

---

## 七、工作流集成（onto-studio 落地形态）

```
AI 生成 TTL（skill：ontology-modeling-w3c）
    │
    ▼
[显式触发] 「运行校验」validate_ttl——打开页面不自动跑（检测零 LLM）
    │
    ├─ error ── 阻断 + 结构化报告回喂 AI（≤3 轮纠错循环）
    │
    ├─ warning ── A 可 dispute（引输入证据反驳）或接受修订
    │     │  dispute → 机械匹配 → B 机器裁决 → overruled（撤销）/ accepted
    │     │  （升级 error）/ uncertain（保持 disputed → 人工仲裁）
    │     └─ 接受 → 直接修正
    │
    └─ pass ──► [覆盖矩阵] 29句话 × 实体覆盖表（同页内嵌展示）
                  │
                  ▼
              [LLM 语义评审] B 审计（八维度，带 evidence）——统一发现池
                  │
                  ▼
              A 修订（接受/拒绝建议 + 理由，逐条回填 a_action）
                  │
                  ▼
              B 复核（≤2 轮；A rejected → 仲裁队列）
                  │
            ┌─────┴─────┐
         确认一致      仍有分歧
            │            │
            ▼            ▼
         通过        [人工仲裁] 分歧点结构化提交专家
                        │
                    专家裁决
                        │
                        ▼
                     修订/通过
                        │
                        ▼
              [人工终审] → import 落库（charter 不动，AI 永不写宪章——决策 23）
```

---

## 八、实施优先级

| 优先级 | 内容 | 载体 | 状态 |
|---|---|---|---|
| P0 | 报告结构化 + 结构规范十项 + 逻辑一致性（5条）+ import 校验门禁 | Rust `check_*` + `validation_gate` | ✅ 已实现 |
| P1 | warning 答辩（dispute/resolve + B 机器裁决 resolve_dispute_llm）+ 覆盖矩阵 | 新表 + 前端卡片 | ✅ 已实现（v0.7 归阶段①） |
| P2 | 多轮语义评审状态机（后端驱动 run_review_loop）+ 统一发现池（source/a_action/b_verdict/step_ms） | `validation_reviews` 表 + ReviewPanel 重写 | ✅ 已实现（v0.7） |
| P2′ | 前端 import 命令补校验闸门（UI 落库路径不过 gate 的缺口，§2.4） | `commands/ontology.rs` | ⬜ 待办 |
| P2″ | 多轮重试分支（round<max_rounds，当前固定单轮 B→A→B） | run_review_loop | ⬜ 待办 |
| P2‴ | 覆盖矩阵增量 diff 接 changelog change_summary（前端仍写死占位） | CoverageView | ⬜ 待办 |
| P3 | 数据可落地性 + CQ 反测（§4）+ 降级档标注进报告 + 不变量 SHACL 编译 | datasets 比对 + charter 扩展 | ⬜ 规划 |

**成本预算（语义评审全流程）**：一次完整对抗验证 = 3~5 次 LLM 全文调用（B 评审 + A 修订 + B 复核，每次携带 TTL + 证据上下文；另有阶段①答辩按 warning 条数各 1 次 B 裁决调用）。以典型 10K token 本体估算：单次调用 ~20-40K token 输入，全流程 ~60-160K token，时延增加 1-3 分钟。这是质量闸门的固定成本，P2 落地时需在设置页展示预估消耗与「跳过语义评审（仅规则校验）」选项。

**验收口径**：P0 落地后，`validate_ttl` 测试（`ttl.rs` 内嵌测试）扩展覆盖新增规则，且 import 对未闭环 error/review 拒绝；P1 落地后，warning 级 issue 可在 UI 发起答辩并持久化状态，裁决走两步归属；P2 落地后，概览页可看到 29 句话覆盖矩阵（增量模式折叠），语义评审可多轮对抗闭环，数据可落地性/CQ 反测给出数据与业务维度的反例；P3 落地后，分歧点可结构化提交专家仲裁，invariants 可编译为约束。

---

## 九、与已有文档/机制的关系

| 已有机制 | 本方案的关系 |
|---|---|
| `validate_ttl`（ttl/validate.rs） | 语法与结构即其现状，本方案是其扩展路线；`validation_gate` 亦在此 |
| modeling-workflow.md 双模型校验 | 本方案把 B 审计中可规则化的部分下沉为确定性规则，B 专注语义评审；模型独立性硬化约束见 §3.1 |
| EVIDENCE-CHAIN.md（决策 24） | warning 答辩的 counterEvidence（机械匹配在 dispute_issue 提交时）与 B 评审的 evidence_ref 都走其三级匹配机制，`evidence_sources`/`element_evidence` 表复用 |
| charter（决策 22/23） | 语义评审的验收基准来源；invariants 编译约束遵循「AI 永不写宪章」 |
| 29 句话范式（material-to-turtle.md） | 覆盖矩阵 正向覆盖矩阵的输入 |
| 双 # IRI 合法化、IRI 命名（iri-naming.md） | 已在 结构规范，不动 |
