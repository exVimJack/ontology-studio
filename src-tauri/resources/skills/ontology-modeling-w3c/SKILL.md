---
name: ontology-modeling-w3c
description: >-
  基于「7+1 语义规范」与「29 句话」范式的 W3C 语义网本体离线建模助手。当需要从
  业务材料（Word/PDF/PPT/Excel/TXT 文档、业务描述、结构化数据源 schema 等）抽象出
  覆盖"事实-事理-行动"全链路的企业本体模型时使用。产物是符合 RDF 1.1 + RDFS +
  OWL 2 + SKOS + SWRL 标准的 Turtle（.ttl）文件，涵盖 7 类业务语义（RDF 资源描述 /
  OWL 分层约束 / SKOS 术语 / SWRL 控制规则 / OWL-S 流程操作 / ODRL 权限管控 /
  SPARQL CRUD 操作）+ 1 类目标评估。遵循《本体驱动的AI数据管理》方法论，支持
  AI Agent「明事实、懂事理、会行动」。适合语义网工程师、知识图谱设计者、企业
  AI 数据资产管理、本体资产入库与运营场景。
license: MIT
metadata:
  version: 2.0.0
  methodology_source: 《本体驱动的AI数据管理》（机械工业出版社，ISBN 978-7-111-81075-9）第5-6章
  output_format: Turtle（.ttl）文件，覆盖 7+1 语义规范
  supersedes: 无（与 ontology-modeling Palantir 风格并列，方法论根源不同）
---

# Ontology Modeling W3C（基于 7+1 语义规范的本体建模）

> **把业务材料抽象成覆盖"事实-事理-行动"全链路的 W3C 标准本体**——以《本体驱动的AI数据管理》第 5-6 章的「7+1 语义规范」和「29 句话」范式为方法论根基，产出符合 RDF 1.1 + RDFS + OWL 2 + SKOS + SWRL 标准的 Turtle 文件。核心不是"画类图"，而是让 AI Agent 能**明事实（RDF/OWL/SKOS）、懂事理（SWRL）、会行动（OWL-S/ODRL/SPARQL）**。

## 这个 Skill 解决什么问题

外部 Agent（语义网工程师、企业数据架构师、AI 应用设计者）手里有业务材料：

- **显性知识**：结构化数据（业务表单、数据库 DDL）、半结构化文档（接口、流程图、BPMN）、非结构化资料（需求规格书、操作手册、历史案例）
- **隐性知识**：业务专家未书面化的经验、决策逻辑、上下文认知

需要从中抽象出**覆盖"事实-事理-行动"三位一体**的企业本体。难点不在"建模"本身，而在**产物必须严格对齐 W3C 全语义栈**，且要回答三个递进问题：

1. **业务存在什么实体、它们如何关联**（事实层 → RDF + SKOS）
2. **业务规则是什么、何时触发什么**（事理层 → OWL 约束 + SWRL 规则）
3. **业务流程怎么走、谁能做什么、数据怎么取用**（行动层 → OWL-S + ODRL + SPARQL）

这正是「7+1 语义规范」要解决的问题：7 类业务语义规范把业务事理映射到 7 个 W3C 标准，+1 类目标评估确保本体与 AI 意图对齐。

## 「7+1 语义规范」框架（方法论根基）

| 类 | 规范名 | W3C 标准 | 解决什么问题 | 句子数 |
|----|--------|----------|-------------|--------|
| 1 | 业务对象描述语义 | RDF | 系统中存在什么实体、它们有什么特征、如何关联 | 2 |
| 2 | 业务分层约束语义 | OWL | 对象如何归属和约束、属性如何推导组合 | 7 |
| 3 | 业务术语 | SKOS | 统一语言，建立共识，避免同物异名/同名异物 | 3 |
| 4 | 业务控制规则语义 | SWRL | 什么条件触发什么业务动作 | 1 |
| 5 | 流程活动和业务操作语义 | OWL-S + BPMN 2.0 | 业务怎么做，原子操作、输入输出、前提效果、流程组合 | 5 |
| 6 | 权限管控语义 | ODRL | 谁能对什么资产做什么，权限规则与操作边界 | 4 |
| 7 | CRUD 数据操作语义 | SPARQL | 本体数据的全生命周期操作（查/算/更新） | 7 |
| +1 | 目标与评估 | （非 W3C，AI 对齐层） | AI 业务意图、评估标准、关联事理模型 | — |

> 7 类共 29 句标准化描述（2+7+3+1+5+4+7=29），即「29 句话」范式——业务专家用这 29 种自然语言模板讲清楚业务，AI 再翻译成 W3C 本体。详见 [references/29-sentences.md](references/29-sentences.md)。

## 何时加载本 Skill

- 用户给了业务文档/数据源 schema，要求"建本体/语义网本体/OWL 本体/Turtle"，且材料涉及业务规则、流程、权限（不只是静态实体）
- 需要本体不只是"类图"，而是能支撑 AI Agent 推理与行动（"事实-事理-行动"闭环）
- 需要把业务专家的隐性经验（决策逻辑、操作规程）沉淀为机器可执行语义
- 需要按「7+1 语义规范」或「29 句话」范式做本体建模
- 需要产出覆盖 SWRL 规则/OWL-S 流程/ODRL 权限的完整 Turtle（不只是 OWL 类和属性）
- 增量更新已有 W3C 本体（新增类/属性/规则/流程/权限）

**不需要加载的场景**：

- 只查/探索已有本体 → 直接用 SPARQL 查询，无需本 skill
- 要建 Palantir 风格平台私有本体（ObjectType/LinkType/ActionType/Dataset）→ 用 `ontology-modeling` skill
- 只要静态类图，无业务规则/流程/权限需求 → 用通用 OWL 教程即可，本 skill 重在"事理+行动"

## 五环联动建模流程（对齐书 6 章）

```
① 预处理（29句话）→ ② AI建模+双模型校验 → ③ 入库 → ④ 平台（skill不实现）→ ⑤ 点线面实施
```

本 skill 聚焦 **①② 两环的方法论**（产出 Turtle），③④⑤ 是落地工程层（见 [references/implementation.md](references/implementation.md)）。

### ① 预处理：从材料到「29 句话」

**目标**：把多源业务知识（显性+隐性）提炼为符合「29 句话」范式的标准化描述集，作为 AI 建模的精准输入。

**六步抽取**（详见 [references/material-to-turtle.md](references/material-to-turtle.md)）：

1. **显性知识 AI 提炼**：大模型从结构化/半结构化/非结构化材料中，按 7 类语义规范自动抽取标准化描述。
2. **隐性知识专家补充**：业务专家依据 29 句话范式，从"有哪些对象/任务怎么做/有哪些规则"三维度萃取经验。
3. **审核准入**：四标准——**描述清晰**（术语唯一、对齐 7+1 模板）、**逻辑正确**（无闭环冲突）、**内容完备**（串联事实-事理-行动）、**极简表达**（满足 AI 意图即可，去噪）。
4. **协同评审**：初审（建模工程师查语义规范）+ 复核（业务专家查逻辑闭环）形成质量闭环。
5. **29 句话产出**：每条带置信度（confirmed/high/tentative）。
6. **tentative 项**：进附属 `open_questions` 文件，不写入 Turtle。

### ② AI 建模与双模型协同校验

**目标**：把 29 句话翻译成符合 W3C 标准的 Turtle，并通过"生成-审计-仲裁"闭环保证质量。

**流程**（详见 [references/modeling-workflow.md](references/modeling-workflow.md)）：

1. **大模型 A 生成**：输入 29 句话 + 业务术语词典 + 系统指令，输出 Turtle（含 owl:Class / ObjectProperty / DatatypeProperty / swrl:Imp 等）。
2. **大模型 B 审计**：独立查找语法规范、逻辑一致性、业务贴合度问题，输出结构化报告。
3. **逻辑博弈**：A 针对 B 的质疑辩护或修订，输出修订版 + 辩护说明。
4. **专家共识仲裁**：双模型结论一致则通过；核心规则争议推送人工终审。
5. **可视化校验**（可选）：拓扑审查（对象与关系）→ 局部语义穿透（属性与约束）→ 规则路径回溯（规则与行动）。
6. **测试用例验证**：专家定义"黄金测试用例" + AI 生成边缘/异常/压力用例，运行验证。

### ③④⑤ 落地工程层（skill 不实现，见 implementation.md）

- **③ 入库**：本体作为资产存入语义数据库（推荐 oxigraph），规范化注册 + 质量审查 + 服务化发布。
- **④ 平台**：多模态输入（Talk/Code/Diagram/Data）→ 智能转化 → 实时自校验 + 可视化。
- **⑤ 点线面实施**：点（小切口场景 MVP）→ 线（领域纵轴整合，四层架构：公共层/领域层/实例层/动作层）→ 面（跨领域横轴贯通，联邦查询）。

## 产物范围（分阶段）

按决策：核心五类主产，OWL-S/ODRL 进阶可选。

### 主产物（核心五类，必产）

| 7+1 类 | 是否主产 | Turtle 词汇 | 29 句话覆盖 |
|--------|---------|------------|-------------|
| 1 业务对象描述（RDF） | ✅ 主产 | `owl:Class` / `owl:DatatypeProperty` / `owl:ObjectProperty` / `rdfs:domain` / `rdfs:range` | 1.1-1.2 |
| 2 业务分层约束（OWL） | ✅ 主产 | `rdfs:subClassOf` / `owl:FunctionalProperty` 等 / `owl:Restriction` / `owl:cardinality` | 2.1-2.7 |
| 3 业务术语（SKOS） | ✅ 主产 | `skos:ConceptScheme` / `skos:prefLabel` / `skos:altLabel` / `skos:exactMatch` | 3.1-3.3 |
| 4 业务控制规则（SWRL） | ✅ 主产 | `swrl:Imp` / `swrl:body` / `swrl:head` / `swrl:Variable` / `swrlb:` 内置谓词 | 4.1 |
| 7 CRUD 操作模板（SPARQL） | ✅ 主产 | SPARQL 查询模板（附属文件，不进 Turtle 主文件） | 7.1-7.7 |

### 进阶产物（可选，材料明确要求时才产）

| 7+1 类 | 是否进阶 | 词汇 | 29 句话覆盖 |
|--------|---------|------|-------------|
| 5 流程活动操作（OWL-S） | ⚠️ 进阶可选 | `owl:Process` / `owl:hasInput` / `hasOutput` / `hasPrecondition` / `hasEffect` | 5.1-5.5 |
| 6 权限管控（ODRL） | ⚠️ 进阶可选 | `odrl:Policy` / `odrl:Permission` / `odrl:Prohibition` / `odrl:Duty` | 6.1-6.4 |

### +1 目标与评估（必产，但不进 Turtle）

- 附属 `open_questions.json` 承载：AI 业务意图、评估标准、关联事理模型、tentative 项、业务操作记录（书第 6 章明确业务操作不入本体层）。
- 格式见 [references/ttl-package-format.md](references/ttl-package-format.md) §4。

## 核心约束（产物必须满足，否则 W3C 校验失败）

1. **前缀声明必填**——`rdf`/`rdfs`/`owl`/`xsd` 四个标准前缀必须声明；主产物再加 `skos`/`swrl`/`swrlb`；进阶加 `owlfs`/`odrl`；业务用 `:` 默认前缀指向稳定 IRI 命名空间。前缀放文件最前。
2. **类必须 `a owl:Class`**——禁用 `rdfs:Class`（不参与 OWL 推理）。每类带 `rdfs:label`（`@zh`）+ `rdfs:comment`。
3. **属性区分 Datatype/Object**——`owl:DatatypeProperty`（值域字面量，`rdfs:range` 指向 `xsd:`）vs `owl:ObjectProperty`（值域类实例，`rdfs:range` 指向 `owl:Class`），两者不可混用（退化为 OWL Full）。
4. **XSD 严格对齐**——金额 `xsd:decimal`（禁 double/float），时间 `xsd:dateTime`，日期 `xsd:date`，布尔 `xsd:boolean`（禁 0/1），大整数 `xsd:long`。映射见 [references/w3c-schema-contract.md](references/w3c-schema-contract.md) §1。
5. **开放世界原则**——不强制主键（实例由 IRI 标识）；不强制必填属性（必填用 `owl:minCardinality` 且仅在需推理时）；纯 M:N 直接 `owl:ObjectProperty` 不拆中间类（关系有属性才拆关联类）。
6. **IRI 命名规范**——类 PascalCase（`:Supplier`），属性 camelCase（`:supplierId`），IRI 命名见 [references/iri-naming.md](references/iri-naming.md)。
7. **中文标签带 `@zh`**——`rdfs:label`/`rdfs:comment` 用 `"中文"@zh`，禁裸中文字符串。
8. **敏感属性在 `rdfs:comment` 标注 `【敏感】`**——如 `:taxId ... ; rdfs:comment "【敏感】供应商税号 [confirmed]"@zh`。
9. **SKOS 术语统一**——同名异物拆概念（`skos:Concept` 各自独立），同物异名用 `skos:prefLabel`/`skos:altLabel`，跨本体对齐用 `skos:exactMatch`/`skos:closeMatch`。见 [references/w3c-schema-contract.md §3](references/w3c-schema-contract.md)。
10. **SWRL 规则用标准词汇**——`swrl:Imp` 包 `swrl:body`（前件）+ `swrl:head`（后件），变量用 `swrl:Variable`，内置谓词用 `swrlb:`（`swrlb:greaterThan`/`swrlb:equal` 等）。见 [references/w3c-schema-contract.md §4](references/w3c-schema-contract.md)。
11. **跨本体复用用标准谓词**——`owl:imports`/`owl:equivalentClass`/`owl:equivalentProperty`/`rdfs:seeAlso`，禁自创谓词。
12. **置信度标记在 `rdfs:comment` 末尾**——`[confirmed]`/`[high]`/`[tentative]`，`tentative` 必须进附属 `open_questions` 文件（不写入 Turtle 主文件的语义部分，但可保留在 comment 文本里作为标注）。
13. **禁自创谓词**——额外元信息用 Dublin Core（`dc:`/`dcterms:`）或 PROV（`prov:`）标准词汇，不要 `:myCustomField`。
14. **业务操作不入本体层**——材料中的"创建订单/审批/取消"等业务动作，记入附属 `open_questions` 的 `business_operations`，不在本体产出对应实体（W3C 描述逻辑不表达命令式操作；如需流程语义走 OWL-S 进阶层）。

## 标准产出流程（操作步骤）

```
材料输入 → 29句话抽取（7类逐条） → 审核准入 → AI生成Turtle → 双模型校验 → 自检
         → validate_ontology_ttl（校验） → import_ontology_ttl（落库，可选）
         → set_ontology_ttl_charter（写宪章，冷启动时） → commit_ontology_ttl_change（记变更日志）
```

### 可用工具（W3C 路线专用，8 个）

| 工具 | 用途 | 何时调 |
|---|---|---|
| `validate_ontology_ttl(ttl_content)` | dry-run 校验（7+1 语义规范） | import 前必调 |
| `import_ontology_ttl(ttl_content, overwrite?)` | 落库（UPSERT，按 IRI） | validate 通过后 |
| `export_ontology_ttl(ontology_iri)` | 取出 Turtle 文本 | 增量更新前 |
| `list_ontology_ttl()` | 本体列表摘要 | 看有哪些本体 |
| `query_ontology_sparql(ontology_iri, sparql)` | 跑 SPARQL 查询 | 验证/抽查 |
| `set_ontology_ttl_charter(ontology_iri, business_scenario, business_essence, design_intent, invariants)` | 写宪章（不变点） | 冷启动时 + 用户明确要求调整时 |
| `commit_ontology_ttl_change(ontology_iri, title, body, change_summary?, conversation_id?)` | 记变更日志 | 每次 import/delete 后 |
| `list_ontology_ttl_changelog(ontology_iri)` | 列变更历史 | 回溯演进过程 |

### Step 1：29 句话抽取

- 按 7 类语义规范逐类抽取，每类对应 W3C 标准。29 句话清单见 [references/29-sentences.md](references/29-sentences.md)。
- 显性知识用 AI 提炼，隐性知识用专家访谈（"有哪些对象/任务怎么做/有哪些规则"三维度）。
- 每条标注置信度。tentative 项进 `open_questions`。

### Step 2：审核准入（四标准）

- [ ] **描述清晰**：术语全局唯一，对齐 29 句话模板
- [ ] **逻辑正确**：对象/规则/动作分类清晰，无闭环冲突
- [ ] **内容完备**：串联"AI意图→事实-事理-行动"完整链条
- [ ] **极简表达**：满足 AI Agent 意图即可，去噪

### Step 3：AI 生成 Turtle

- 输入：29 句话 + 业务术语词典 + 系统指令（Prompt 模板见 [references/modeling-workflow.md](references/modeling-workflow.md) §1）。
- 大模型充当"语义编译官"，输出 Turtle（主五类 + 进阶可选）。
- 输出必须含：`owl:Class` 及层级、`owl:Object/DatatypeProperty`、`skos:ConceptScheme`、`swrl:Imp` 规则。

### Step 4：双模型协同校验

- **大模型 A 生成** → **大模型 B 审计**（语法规范/逻辑一致/业务贴合）→ **A 辩护修订** → **专家仲裁**。
- 审计维度见 [references/modeling-workflow.md](references/modeling-workflow.md) §2。

### Step 5：合规自检（产出前必过）

- [ ] 前缀声明完整（`rdf`/`rdfs`/`owl`/`xsd` + `skos`/`swrl`/`swrlb` + 业务 `:`）？
- [ ] 所有类 `a owl:Class`（非 `rdfs:Class`）？
- [ ] 所有属性区分 `owl:DatatypeProperty`/`owl:ObjectProperty`（无混用）？
- [ ] `rdfs:domain`/`rdfs:range` 指向合法类或 xsd 类型？
- [ ] IRI 局部名符合规范（类 PascalCase、属性 camelCase）？
- [ ] 中文标签带 `@zh`（无裸中文字符串）？
- [ ] 金额 `xsd:decimal`？时间 `xsd:dateTime`/`xsd:date`？布尔 `xsd:boolean`？
- [ ] 纯 M:N 未硬拆中间类（除非关系有属性）？
- [ ] 无循环继承（`rdfs:subClassOf` 链无环）？
- [ ] 敏感属性在 `rdfs:comment` 标注 `【敏感】`？
- [ ] SKOS 术语用 `skos:prefLabel`/`altLabel`/`exactMatch`（同名异物拆概念）？
- [ ] SWRL 规则用 `swrl:Imp`/`swrl:body`/`swrl:head`/`swrlb:` 标准词汇？
- [ ] 置信度标记在 `rdfs:comment` 末尾？`tentative` 进附属 `open_questions`？
- [ ] 业务操作记入附属 `open_questions`（不入本体层）？
- [ ] 无自创谓词（元信息用 `dc:`/`dcterms:`/`prov:`）？

### Step 6：validate → import（可选落库）

- 构造 Turtle 文件（格式见 [references/ttl-package-format.md](references/ttl-package-format.md)）。
- **先调 `validate_ontology_ttl(ttl_content)`**：检查 errors 是否为空。errors 非空（Turtle 语法错、类非 `owl:Class`、属性混用、XSD 非法、SWRL 词汇误用、IRI 重复、**subClassOf 主语未声明 a owl:Class**）则修正重试。
- validate 通过后调 `import_ontology_ttl(ttl_content)`（如需落库）。返回 `ImportResult` 检查 per-entity 状态（best-effort 部分失败）。
- SPARQL 查询模板（第 7 类 CRUD）用 `sh:select` 序列化进 .ttl 主文件（W3C SHACL 1.2 标准），不独立 `.sparql` 文件。
- 冷启动建模时调 `set_ontology_ttl_charter` 写宪章（业务场景/本质/意图/不变点），同步用 W3C 原生词汇（`dcterms:` + `skos:`）写进 .ttl。
- 每次 import 后调 `commit_ontology_ttl_change` 记变更日志（git commit message 式，记本次变更的为什么和整体设计）。
- 详细闭环见 [references/modeling-workflow.md 第七节](references/modeling-workflow.md)。

## 置信度标记

| 标记 | 含义 | 写法 |
|------|------|------|
| `confirmed` | 材料明确出现或用户明确提供 | `rdfs:comment "说明 [confirmed]"@zh` |
| `high` | 行业最佳实践+材料强推断，极大概率正确 | `rdfs:comment "说明 [high]"@zh` |
| `tentative` | 存在多种选择，需用户确认——必须进附属 `open_questions` | `rdfs:comment "说明 [tentative]"@zh` |

## 禁止清单（红线）

1. ❌ 用 `rdfs:Class` 替代 `owl:Class`
2. ❌ `owl:DatatypeProperty` 与 `owl:ObjectProperty` 混用
3. ❌ 用 `xsd:string` 替代 `xsd:dateTime`/`xsd:date`/`xsd:boolean`/`xsd:decimal`
4. ❌ 中文标签不加 `@zh` 语言标签
5. ❌ 硬拆纯 M:N 为中间类（除非关系有属性）
6. ❌ `rdfs:subClassOf` 循环继承
7. ❌ `rdfs:subClassOf` 子类未显式声明 `a owl:Class`（oxigraph 无推理，未声明会从类查询中遗漏）
8. ❌ `rdfs:subPropertyOf` 子属性未显式声明为 `rdf:Property`/`owl:DatatypeProperty`/`owl:ObjectProperty`
9. ❌ 自创谓词（元信息用 `dc:`/`dcterms:`/`prov:`）
10. ❌ 业务操作写入本体层（记入附属 `open_questions`）
11. ❌ `tentative` 项不进附属 `open_questions` 就交付
12. ❌ SWRL 规则不用 `swrl:Imp`/`swrlb:` 标准词汇
13. ❌ 同名异物不拆 SKOS 概念（用 `skos:altLabel` 错误合并）
14. ❌ IRI 局部名不符合命名规范
15. ❌ 数据源绑定信息写入本体（概念层与数据解耦）
16. ❌ 滥用 `owl:cardinality` 基数约束（开放世界默认无约束，仅推理需要时用）

## 参考资料（按需加载）

| 文档 | 内容 | 加载时机 |
|------|------|----------|
| [29 句话清单](references/29-sentences.md) | 7 类 × 29 句完整模板 + 白话解释 + W3C 标准映射 + 每句对应的 Turtle 产出 | 抽取业务知识时逐句对齐 |
| [W3C Schema 契约](references/w3c-schema-contract.md) | OWL/RDFS/SKOS/SWRL 词汇表 + XSD 映射 + Turtle 语法 + 基数 + OWL-S/ODRL 进阶 | 产出 Turtle 时逐谓词对齐 |
| [IRI 命名规范](references/iri-naming.md) | IRI 命名空间设计 + 局部名 pattern + 语言标签 + 前缀声明 | 命名实体时 |
| [材料到 29 句话方法论](references/material-to-turtle.md) | 六步抽取法 + 各材料类型策略 + 显性/隐性知识处理 + 审核四标准 + 荔枝病虫害完整示例 | 拿到材料不知如何下手时 |
| [AI 建模与双模型校验工作流](references/modeling-workflow.md) | Prompt 模板 + 大模型 A 生成 + 大模型 B 审计 + 逻辑博弈 + 仲裁 + 可视化校验 + 测试用例 | AI 建模与质量保障时 |
| [Turtle 产物格式](references/ttl-package-format.md) | Turtle 文件结构 + 完整示例 + SPARQL 查询模板（`sh:select` 写进 .ttl） + open_questions 附属文件 + 导入工具语义 | 产出/校验产物时 |
| [实现层参考](references/implementation.md) | oxigraph 存储查询选型 + 推理暂留白决策 + 工具链设计 + 落地工程五环③④⑤ | 实现落地时 |

## 与其他 Skill 的关系

- **`ontology-modeling`（Palantir 风格）skill**：并列，方法论根源不同。Palantir 是封闭世界平台私有契约（ObjectType/LinkType/ActionType/Dataset + backing_mapping），本 skill 是开放世界 W3C 全语义栈（7+1 规范，事实-事理-行动）。产物格式不同，不互通。
- **外部 W3C 工具链**（Apache Jena / rdflib / Oxigraph / Protégé）：本 skill 产出的标准 Turtle 可被任何标准 RDF 工具消费，反之亦然。

## 落地路线（对齐 onto-studio 进度时参考）

- **一期 MVP（W3C skill 方法论）**：SKILL.md + 7 个 references → 用采购或荔枝病虫害场景跑通一次完整方法论（29 句话 → Turtle → 校验）→ 评估产物质量
- **二期（实现层）**：`crates/ontology-store` 加 oxigraph 集成（存储查询层）→ `validate_ontology_ttl` / `import_ontology_ttl` 工具 → SPARQL 查询能力 → 推理暂留白（按 oxigraph 设计决策，未来真有需求再评估 reasonable 集成）
- **三期（进阶）**：OWL-S 流程建模 + ODRL 权限建模（材料明确要求时）→ 跨本体 `owl:imports`/`owl:equivalentClass` 复用 → 点线面实施路径支持
