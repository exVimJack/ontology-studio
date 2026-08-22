---
name: ontology-modeling
description: >-
  面向本体平台的离线建模助手。当需要从业务材料（Word/PDF/PPT/Excel/TXT
  文档、业务描述、结构化数据源 schema 等）抽象出符合业务本质的本体模型
  （ObjectType + LinkType + ActionType + Dataset + DataSource）时使用。产物是
  结构化的 OntologyPayload JSON，通过三个 agent 工具（export_ontology /
  preview_ontology_import / import_ontology）落库——支持冷启动建模和可持续增量更新。
  严格遵循命名规范与 schema 契约，默认兼容 Gaia 本体平台的 export/import 语义。
  吸收通用 Palantir 方法论，对业务人员友好、对平台 schema 严格。
  适合数据工程师、本体设计者、自动化建模流水线离线产出本体定义。
license: MIT
metadata:
  version: 1.0.0
  output_format: OntologyPayload JSON（通过 export/preview/import 工具落库）
  supersedes: ~/.pi/agent/skills/ontology-modeling（通用 Palantir 版，已被本项目级版本覆盖）
---

# Ontology Modeling（本体离线建模）

> **把业务材料抽象成结构化本体定义**——通过三个 agent 工具（`export_ontology` / `preview_ontology_import` / `import_ontology`）完成冷启动建模和可持续增量更新。产物格式 `OntologyPayload` 严格对齐 Gaia 本体平台的 export/import 语义：命名规范、schema 契约、引用完整性校验、overwrite upsert 语义全部内建于工具链。导入细节（字段映射、api_name→UUID 转换、DAG 顺序）见 [references/ontology-package-format.md §10](references/ontology-package-format.md)。

## 这个 Skill 解决什么问题

外部 Agent（数据工程师、本体设计者、自动化建模流水线）手里有业务材料：

- **非结构化文档**：Word / PDF / PPT / TXT（业务方案、需求文档、操作手册、规章制度）
- **半结构化文档**：Excel / CSV（数据字典、字段说明、清单）
- **业务描述**：用户口述 / 自然语言段落
- **结构化数据源 schema**：数据库 DDL、表结构、API 报文、JSON Schema

需要从中抽象出**符合业务本质**的本体模型。难点不在"建模"本身，而在**产物必须严格对齐输出格式的 schema 契约和命名规范**——api_name 大小写、主键约束、M:N 拆分、storage_type 取值、backing_mapping 字段结构……任一不对，导入目标平台就会失败。

本 skill 提供这套**从材料到结构化本体定义**的完整方法论 + 精确 schema 契约 + 产物格式定义。

## 何时加载本 Skill

- 用户给了业务文档 / 数据源 schema，要求“抽象成本体 / 建模 / 生成本体定义”
- 需要产出结构化的 OntologyPayload JSON 并通过工具落库
- 不确定某个实体的 api_name 该怎么命名、storage_type 选 MANAGED 还是 VIRTUAL
- 拿到一个 M:N 关系，不确定是否要拆中间实体
- 要把文档里的字段映射成 DataType（金额用 DECIMAL 还是 DOUBLE？）
- 增量更新已有本体（改几个实体、新增属性、调整关系）

**不需要加载的场景**：

- 只是查询/探索已有本体 → 直接调 `export_ontology` 工具拿当前定义，无需本 skill
- 要连接外部 Gaia 平台在线建模 → 用对应平台的 Agent skill

## 核心约束（产物必须满足，否则输出格式校验失败）

1. **api_name 严格按 pattern**：
   - Ontology / ObjectType：`^[A-Z][a-zA-Z0-9]{0,99}$`（PascalCase，首字母大写）
   - Property / LinkType / ActionType / 参数：`^[a-z][a-zA-Z0-9]{0,99}$`（camelCase，首字母小写）
   - Dataset / DataSource / Credential / SyncTask api_name：`^[a-z][a-z0-9_]{0,99}$`（snake_case，全小写保词界）
2. **每个 ObjectType 必须有且仅一个主键**——`is_primary_key: true` 的属性有且仅有一个（平台自动设其 `nullable: false`）。主键 DataType 用 `STRING`，禁止自增数值。
3. **M:N 严禁直接建**——LinkType 的 `cardinality` 只有 `ONE`/`MANY`。多对多必须引入中间 ObjectType（命名如 `SupplierMaterialRel`），含自身主键 + 两端外键属性 + 关联属性，然后建两组 1:N。
4. **storage_type 二选一**：`MANAGED`（平台托管存储，可读写）/ `VIRTUAL`（外部数据源联邦代理，**只读**，禁止定义写入动作）。
5. **ActionType 是语义契约，禁运行时策略**——合法字段：`api_name`/`display_name`/`description`/`affected_object_type_api_name`（**必填** api_name string）/`parameters`/`rules`/`submission_criteria`/`effects`/`ontology_rules`/`risk_level`/`operation_kind`/`batch_enabled`。**禁止** `idempotent`/`atomic`/`retry_strategy`/`rollback_action`/`timeout_seconds`，也禁止 Palantir 的 `modifies`/`constraints`/`side_effects`（用 `ontology_rules` 表达变更、`rules` 表达派生/约束、`effects` 表达副作用）。`submission_criteria` 是**可执行 simpleeval 表达式**（`{expression, error_message, description}`），不是只命名。运行时策略属于平台 Function 层。详见 [references/gaia-schema-contract.md](references/gaia-schema-contract.md) 第六节。
6. **backing_mapping 结构固定**——`{dataset_api_name, backing_catalog, backing_schema, backing_table, backing_column}` 五字段，缺一不可（可为空串，但键必须存在）。物理列名用 snake_case。三段式定位语义：`backing_catalog`（数据联邦层注册的顶层目录名，**不是**数据源引擎自身的库名）→ `backing_schema`（引擎层 schema/库名）→ `backing_table`（表）→ `backing_column`（列）。catalog 和 schema 属于不同抽象层级：catalog 是数据联邦跨引擎统一编目的命名空间（目标平台对接外部数据源时注册的名字），schema 是引擎内部命名空间——切勿把引擎库名填进 `backing_catalog`。
7. **DataType 从枚举取值**——只能用 `STRING/INTEGER/SHORT/LONG/BOOLEAN/BYTE/FLOAT/DOUBLE/DECIMAL/DATE/TIMESTAMP/ARRAY/STRUCT/VECTOR/GEOPOINT/GEOSHAPE/GEOTEMPORAL_SERIES/TIME_SERIES/MEDIA_REFERENCE/ATTACHMENT`。**无别名**——禁用 `BIG_INTEGER`/`BIGINT`/`INT`/`INT64`/`UINT8` 等变体（不在枚举里即非法，导入预检阶段 500）。大整数（>2³¹，如芯片 HBM/L2 容量字节）用 `LONG`（64-bit）。金额强制 `DECIMAL`（禁 DOUBLE），时间强制 `TIMESTAMP`（禁 STRING），布尔强制 `BOOLEAN`（禁 0/1）。
8. **ObjectTypeGroup 是纯分类原语**（ADR-022）——无任何权限语义（可见性继承 Ontology，命名不暗示权限）。成员关系是 M:N 的元数据分组缓存表，**不是业务 LinkType**（与本 Skill「M:N 只能通过中间 ObjectType 建模」规则不冲突）。`api_name` PascalCase、本体范围内唯一、**不可改**。
9. **敏感属性在 description 标注**——敏感属性（证件号/手机号/金额等）在 `description` 文本里显式标注"【敏感】"。
10. **置信度标记产出质量**——每个产出的实体标注 `confirmed`/`high`/`tentative`；`tentative` 项必须在产物的 `open_questions` 里列出待确认问题。
11. **字段集以 schema 契约为真相源，禁自创字段**——每个实体的合法字段集以 [references/gaia-schema-contract.md](references/gaia-schema-contract.md) 字段表为准，**不得自创字段**（如 LinkType 的 `cross_domain`、Dataset 的 `description`、DataSource 的 `connection` 等）。Gaia pydantic 默认 `extra='ignore'` 会静默丢弃多余字段（不会 500），但导入脚本不预期这些字段，且违反「产物即契约」原则。如需标注额外元信息（如跨域标记、领域分类），写进实体的 `description` 文本，不要新增字段。
12. **本体设计宪章（charter）是不变点，禁随意修改**——charter（business_scenario/business_essence/design_intent/invariants）记录业务本质说明，**不随实体增删改而变更**。冷启动首导后用 `set_ontology_charter` 落库一次；增量更新时**只读不写**（用 `describe_ontology` 读作约束基线），**只有用户明确要求调整不变点时才调 `set_ontology_charter` 修订**。常规增量更新调 `set_ontology_charter` 是违规——会让 AI 失去稳定的业务认知基线。

## 标准产出流程

```
材料输入 → 实体识别 → 命名规范化 → 关系/动作建模 → 数据源绑定 → 自检
         → preview_ontology_import（校验）→ import_ontology（落库）
```

### 工具链（agent 可调用的 DynamicTool）

**建模组（增量/冷启动）：**

| 工具 | 作用 | 何时调 |
| ------ | ------ | -------- |
| `export_ontology(ontology_api_name)` | 导出当前本体完整定义（write-view JSON） | 增量更新前拿当前快照 |
| `preview_ontology_import(payload, overwrite_object_types?)` | dry-run 预演导入，返回 per-entity 预测 + 引用完整性 errors + warnings | **落库前必调**，errors 非空则先修正 |
| `import_ontology(payload, overwrite_object_types?)` | 执行导入（DAG 顺序落库，best-effort 部分失败） | preview 无 errors 后调 |
| `set_ontology_charter(ont, business_scenario, business_essence, design_intent, invariants)` | 写入/更新本体设计宪章（不变点） | **冷启动首导后调一次**；增量更新时**不调**（除非用户明确要求调整不变点） |

**删除组（import 只能 upsert，删除实体/关系必须用这组）：**

| 工具 | 作用 | 何时调 |
| ------ | ------ | -------- |
| `delete_object_type(ont, ot)` | 删 ObjectType，连带 properties/分组成员/引用它的 Link 和 Action | 用户要求删实体时；**删除前先向用户确认连删范围** |
| `delete_link_type(ont, link)` | 删单个 LinkType | 用户要求删关系时 |
| `delete_action_type(ont, action)` | 删单个 ActionType | 用户要求删动作时 |
| `delete_dataset(dataset)` | 删全局 Dataset；被 OT backing 或 view 派生引用时拒绝（错误信息列引用方） | 先解绑/删引用方再删 |
| `delete_data_source(source)` | 删全局 DataSource；被 Dataset 引用时拒绝 | 先删/解绑相关 Dataset |

删除工具全部幂等（不存在返回 deleted=false），落库后自动触发 ontology-changed 事件刷新前端。

**冷启动**：先收集 charter（见下「本体设计宪章（不变点）」），再 `preview_ontology_import(payload)` → `import_ontology(payload)` → `set_ontology_charter(...)`。

**增量更新**：`export_ontology(ont)` → **先调 `describe_ontology(ont)` 读 charter 作为本次变更的约束基线** → 改 payload 里需要变更的实体 → 把要覆写的 OT 列入 `overwrite_object_types` → `preview_ontology_import` → `import_ontology`。未列入 overwrite 的同名 OT 默认 skip，保护已有成果。**增量更新不调 `set_ontology_charter`**（charter 是不变点，不随实体变更而变）。

## 本体设计宪章（不变点）

> **本体始终扮演「向 AI 说明业务本质」的角色**——charter 是这个角色的载体。
> 与 changelog（变化点）分离：changelog 记每次变更随历史增长，charter 记业务本质说明不随历史变化。

### 为什么需要 charter

本体是「够用且可扩展」的——围绕**业务意图目标**建最小必要本体，保留可扩展性。但「意图」本身如果不显式记录下来，增量更新几轮后就会漂移：AI 可能为了当前需求添加与初衷冲突的实体、或偏离业务本质。charter 把**业务意图、本质、设计取舍、业务约束**固化为不变点，让每一次增量更新都对照检查。

### 四字段语义

| 字段 | 语义 | 示例 |
| ------ | ------ | -------- |
| `business_scenario` | 业务场景：服务于什么业务目标、谁用、解决什么问题 | 「供应链采购场景：采购员向供应商下达采购单并跟踪到货」 |
| `business_essence` | 业务本质：核心业务对象/状态/关系/动态行为的一句话本质概括 | 「核心对象：供应商/零件/采购单；状态流转：草稿→已下单→已到货」 |
| `design_intent` | 设计意图：为什么这样建模、够用且可扩展的取舍、可扩展方向 | 「先建供应商+零件+采购单三角，后续可扩展质检/物流；未建中间表因为暂无多对多」 |
| `invariants` | 补充说明（自由文本）：不可违反的业务约束、边界条件等 | 「采购单金额必须 >0；供应商状态为活跃时才可下单；不可物理删除已下单采购单」 |

### 冷启动场景：先收集 charter，再详细建模

**流程**：

1. **从历史对话/材料中提取 charter 信息**——如果用户在对话里已描述过业务场景、业务对象、建模目标，应主动提取整理成四字段，**不要重复问用户已说过的内容**。
2. **信息不足时向用户确认**——如果历史对话/材料无法推断出 `business_scenario` 或 `business_essence`，向用户提问收集（「这个本体主要服务于什么业务？核心业务对象有哪些？」）。不要凭空臆测 charter。
3. **charter 落库**——首导 `import_ontology(payload)` 成功后，调 `set_ontology_charter(...)` 把 charter 写入。后续增量更新不再调。
4. **然后开始详细实体建模**——charter 是建模的北极星，实体识别/命名/关系建模都应对照 charter 的 `business_essence` 和 `invariants`。

### 增量更新场景：读 charter 作约束，不修改它

**流程**：

1. **先调 `describe_ontology(ont)` 读取 charter**——拿到 `business_scenario`/`business_essence`/`design_intent`/`invariants`，作为本次变更的约束基线。
2. **对照 `invariants` 自检本次变更**——新增/修改的实体是否违反业务约束？（如 charter 说「采购单金额必须 >0」，本次新增的属性不应允许负金额。）软约束：违反时在交付说明里提醒用户，不阻断落库。
3. **不调 `set_ontology_charter`**——charter 是不变点，增量更新不触碰。
4. **除非用户明确要求调整不变点**——只有当用户明说「更新业务场景说明」「调整设计意图」「补充业务约束」时，才调 `set_ontology_charter` 修订对应字段。

**删除实体/关系**：不需要走 export/import——直接调对应的 delete 工具。删 OT 前用 `describe_object_type` 看清连删范围（链接/动作），向用户确认后执行。

### Step 1：实体识别与术语统一

- 从材料中识别业务实体（ObjectType）。**只有现实业务实体/事件/流程节点/资源**才算 ObjectType——数据库表、中间表、接口报文结构**不是** ObjectType。
- 识别语义分组需求（ObjectTypeGroup，ADR-022）：本体内的**纯分类原语**，把相关 ObjectType 归组以便浏览搜索（如 ER 域按「应收」「应付」「基础数据」分组）。Group **无任何权限语义**（权限靠 Project + Marking），不涉及 DataSource/Dataset/物理存储。
- 一个业务唯一概念全局只建一个 ObjectType，禁止重复。
- 用 `display_name`（中文友好名）统一术语，从 display_name 推导 `api_name`（中文需翻译为英文 PascalCase）。

### Step 2：命名规范化

- ObjectType/Ontology api_name：PascalCase（`ProductionOrder`、`Supplier`）
- ObjectTypeGroup api_name：PascalCase（`CoreEntities`、`Receivables`），本体范围内唯一，不可改，命名反映语义分组意图不暗示权限
- Property/LinkType/ActionType api_name：camelCase（`orderDate`、`supplies`、`submitOrder`）
- Dataset/DataSource api_name：snake_case（`customer_order`、`erp_supplier_master`）
- 关系命名用小驼峰动词短语（`contains`/`belongsTo`），正反向语义互逆，禁用 `has`/`rel`/`link`。

**ObjectTypeGroup 建模要点**（ADR-022）：

- 纯语义分类，**不涉及 DataSource/Dataset/物理存储**。
- `api_name` PascalCase、本体范围内唯一、**不可改**（重命名须删除后重建）。
- 成员关系是 M:N 的元数据分组（一个 ObjectType 可属多 Group），**不是业务 LinkType**——与本 Skill「M:N 只能通过中间 ObjectType 建模」规则不冲突。
- Group 命名反映语义分类意图，**不暗示权限**（如 `AdminGroup` ❌；`CoreEntities` ✅）。
- 产物可产出 `members[]` 表示期望初始成员（ObjectType api_name 列表），导入时先建 Group 再通过 `POST /.../members` 绑定（**幂等**）。

**ObjectType 建模要点**：

- `visibility`：`NORMAL`（默认，常规实体）/ `PROMINENT`（核心实体，如 Customer/Order，在导航/首页优先展示）/ `HIDDEN`（纯技术中间实体，如 M:N 拆分的 `SupplierMaterialRel`，不在用户可浏览列表出现）。
- `capabilities`：`graph_indexing_enabled` 仅在有 LinkType 连接的 MANAGED ObjectType 上开（启用 Neo4j 图节点，支持 searchAround 遍历）；`geotime_indexing_enabled` 仅在有 GEOPOINT/GEOSHAPE/TIME_SERIES/GEOTEMPORAL_SERIES 属性的 MANAGED ObjectType 上开（启用 PostGIS/TimescaleDB 索引）。默认全部 `false`——盲目打开有额外存储和同步成本。

**Property 建模要点**：

- `is_title_property`：每个 ObjectType 至多一个，设给最能代表对象身份的属性（如 `Supplier.name`、`PurchaseOrder.orderNo`）。前端在对象列表/引用处用此属性作标题展示；不设时平台降级用主键展示。
- **敏感属性设 `searchable: false`**：默认 `searchable: true` 会建搜索索引——对证件号/手机号/税号等敏感字段应显式关闭，防止通过搜索接口泄漏。
- **VECTOR 属性须填 `vector_config`**：仅 `data_type: "VECTOR"` 时使用。`source_expression` 指定哪些属性值拼接为 embedding 输入文本（如 `["name", "description"]`，**语义必填**）；`dimension` 默认 384（对齐 OnnxEmbeddingProvider）；`similarity_function` 默认 `"cosine"`。非 VECTOR 属性填 `null`。

### Step 3：关系与动作建模

- LinkType：`cardinality` ∈ {ONE, MANY}，`direction` ∈ {OUTGOING, INCOMING}。关系非 1:1 时一律填 `MANY`（无论 1:N 还是 N:1）；仅严格 1:1 填 `ONE`。产物用 `source_object_type_api_name`/`target_object_type_api_name`（api_name，人类可读 + 跨包稳定）；导入时由导入脚本负责 api_name→UUID 转换（详见 [references/ontology-package-format.md §10](references/ontology-package-format.md)）。`direction` 默认 `OUTGOING`：从持有外键方指向被引用方；`INCOMING` 用于从被引用方反向声明引用的场景（如 Supplier 反向列出被哪些 PurchaseOrder 引用），大部分场景 `OUTGOING` 即可。`foreign_key_property_api_name`（可选）指定当前 ObjectType 上充当外键的属性 api_name（camelCase），如 `PurchaseOrder` 上的 `supplierId`——平台可用此字段自动推断引用语义。
- M:N → 引入中间 ObjectType（含自身主键 + 两端外键 + 关联属性）+ 两组 1:N。
- 禁止循环依赖（A→B→C→A）。
- ActionType（可选/进阶）：声明 `parameters`（入参）/`rules`（派生/约束/校验规则）/`submission_criteria`（**可执行 simpleeval 表达式**，非只命名）/`effects`（副作用）/`ontology_rules`（声明式变更）/`affected_object_type_api_name`（必填 api_name）/`risk_level`/`operation_kind`/`batch_enabled`，详见 references/gaia-schema-contract.md。`OntologyRule` 字段用 `target_parameter`/`target_object_type`/`properties`/`link_type`/`source_parameter`/`target_link_parameter`（**不是**旧文档的 object_type_api_name/match_key/property_values）。

### Step 4：数据源绑定

- 每个 MANAGED ObjectType 应产出对应的 Dataset 定义（放入 `datasets[]`），api_name 从 ObjectType 推导 snake_case（`PurchaseOrder` → `purchase_order`，`SupplierMaterialRel` → `supplier_material_rel`）。ObjectType 上的 `backing_dataset_api_name` 可填也可留空——填了便于导入脚本一键绑定，留空则导入后由管理员手动 `link_dataset`。仅当数据源完全未确认时，整个 Dataset 标记 `tentative` 并进 `open_questions`。
- 属性级 `backing_mapping` 指向该 Dataset 的物理列（五个字段必填，值可为空串——但列名确认后应填上）。
- VIRTUAL ObjectType 绑定外部数据源表（`storage_location` 三段式 `catalog.schema.table`，**只读**）。三段式第一段 `catalog` = 数据联邦层注册的顶层目录名（与 `backing_mapping.backing_catalog` 同源，**不是**引擎库名）；第二段 `schema` = 引擎层 schema/库名；第三段 `table` = 物理表名。三段必须与该 Dataset 下属性的 `backing_mapping.backing_catalog`/`backing_schema`/`backing_table` 完全一致——`storage_location` 与 `backing_mapping` 是同一物理表的两种引用方式，catalog 段不一致会导致联邦查询 Catalog not found。
- DataSource 描述外部系统连接（`connector_type` + `connector_config`），凭据用占位符（`***`），不产出真实凭据。

### Step 5：合规自检（产出前必过）

- [ ] 每个 ObjectType 有且仅一个 `is_primary_key: true` 的属性？（平台自动设主键 nullable=false，产物里**不要**写 `nullable`/`indexed` 字段）
- [ ] 主键 DataType 是 `STRING`（禁自增数值）？
- [ ] 所有 api_name 符合对应 pattern（PascalCase/camelCase/snake_case）？
- [ ] 中文 display_name 都显式提供了 api_name？（否则兜底成 `property0`）
- [ ] M:N 是否都拆成了中间实体 + 两组 1:N？
- [ ] 所有 `data_type` 都在 DataType 枚举里（无 `BIG_INTEGER`/`BIGINT`/`INT`/`INT64` 等变体）？金额用 `DECIMAL`？时间用 `TIMESTAMP`/`DATE`？布尔用 `BOOLEAN`？大整数（>2³¹）用 `LONG`？
- [ ] ActionType 没有混入运行时策略字段（idempotent/retry/timeout/rollback）？也没用旧文档错误的 OntologyRule 字段名（object_type_api_name/match_key/property_values 等）？`submission_criteria` 写的是可执行 `expression` 而非只 `{name,description}`？
- [ ] VIRTUAL ObjectType 没有定义写入动作？
- [ ] ObjectTypeGroup 有产出时的 api_name 是 PascalCase、本体范围内唯一？
- [ ] Group 命名不暗示权限语义？
- [ ] 敏感属性在 description 标注了"【敏感】"？
- [ ] 敏感属性设了 `searchable: false`？
- [ ] DataSource 的 `connector_config` 敏感字段用了 `***` 占位？
- [ ] backing_mapping 的五个键（dataset_api_name/backing_catalog/backing_schema/backing_table/backing_column）都存在（值可为空串，键不能缺）？`backing_catalog` 填的是数据联邦层注册名（非引擎库名）？
- [ ] VIRTUAL Dataset 的 `storage_location` 三段式首段 = 联邦层注册名（与 `backing_catalog` 同源对齐，非引擎库名）？`storage_location` 的 catalog/schema/table 三段与该 Dataset 下属性的 `backing_mapping` 三段完全一致？
- [ ] MANAGED ObjectType 的 `backing_dataset_api_name` 是否已尽量填写（非空）？未填的是否标记 `tentative` 并进 `open_questions`？
- [ ] Group 命名不暗示权限？
- [ ] 所有实体的字段集与 [gaia-schema-contract.md](references/gaia-schema-contract.md) 字段表一致（无自创字段如 `cross_domain`/`connection`/Dataset.`description`）？额外元信息写进 `description` 文本而非新增字段？
- [ ] `tentative` 项都在 `open_questions` 里列出了？

### Step 6：preview → import 落库

- 自检通过后，构造 `OntologyPayload` JSON（格式见 [references/ontology-package-format.md](references/ontology-package-format.md)）。
- **先调 `preview_ontology_import(payload)`**：检查 errors 是否为空。errors 非空（如引用完整性违反、主键缺失）则修正 payload 重试，**不要**跳过 preview 直接 import。
- preview 无 errors 后调 `import_ontology(payload)`。返回 `ImportResult` 检查 per-entity 状态：`failed` 的实体记入 errors，其他成功的不受影响（best-effort 部分失败）。
- **`confidence` 是 SKILL 扩展字段**，不在 Gaia schema 中——工具会自动忽略，仅用于标注产物质量和驱动 `open_questions`。
- 落库后在 `open_questions` 汇总所有 `tentative` 项和需要人工确认的决策。

## 置信度标记

| 标记 | 含义 |
|------|------|
| `confirmed` | 材料中明确出现或用户明确提供 |
| `high` | 基于行业最佳实践 + 材料强推断，极大概率正确 |
| `tentative` | 存在多种选择，需用户确认——必须进 `open_questions` |

## 禁止清单（红线）

1. ❌ 定义无业务含义的空实体、空属性、空关系
2. ❌ 用 STRING 替代 TIMESTAMP/BOOLEAN/DECIMAL
3. ❌ 直接建 M:N（必须拆中间实体）
4. ❌ 实体间循环依赖
5. ❌ 同一业务语义重复创建多套 ObjectType
6. ❌ ActionType 混入 idempotent/retry/timeout/rollback；或用旧文档错误的 OntologyRule 字段名（object_type_api_name/match_key/match_value_source/property_values/link_type_api_name/target_object_type_api_name）；或 submission_criteria 只写 `{name,description}` 不写可执行 `expression`
7. ❌ VIRTUAL ObjectType 定义写入动作
8. ❌ 自增数值做主键（必须 STRING 主键）
9. ❌ api_name 不符合 pattern（PascalCase/camelCase/snake_case 混用）
10. ❌ ObjectTypeGroup 命名暗示权限（如 `AdminGroup`/`SensitiveData`——权限靠 Project + Marking）
11. ❌ 产物里塞真实凭据（必须用 `***` 占位）
12. ❌ `tentative` 项不进 `open_questions` 就交付
13. ❌ 常规增量更新调 `set_ontology_charter`（只有用户明确要求调整不变点时才允许）
14. ❌ 冷启动不收集 charter 就开始详细建模（失去业务认知基线，后续增量更新必漂移）
15. ❌ 改输出格式字段名大小写或自创字段

## 参考资料（按需加载）

| 文档 | 内容 | 加载时机 |
| ------ | ------ | ---------- |
| [Schema 契约](references/gaia-schema-contract.md) | Ontology/ObjectType/Property/LinkType/ActionType/Dataset/DataSource 完整字段表 + DataType 枚举 + 取值约束 | 产出 JSON 时逐字段对齐 |
| [命名规范](references/naming-conventions.md) | api_name pattern + api_name 推导规则 | 命名实体时 |
| [材料到本体方法论](references/material-to-ontology.md) | 六步法详解 + 各材料类型（Word/PDF/Excel/DDL/业务描述）的抽取策略 + 示例 | 拿到材料不知如何下手时 |
| [OntologyPackage 格式](references/ontology-package-format.md) | 产物 JSON Schema + 完整示例 + 导入说明（含 Gaia 导入路径） | 产出/校验产物时 |

## 数据类型速查

| 业务场景 | 正确 DataType | 禁止 |
| ---------- | --------------- | ------ |
| 金额/单价/费用 | `DECIMAL` | `DOUBLE`/`STRING` |
| 业务时间 | `TIMESTAMP` | `STRING` |
| 日期（无时间） | `DATE` | `STRING` |
| 布尔状态 | `BOOLEAN` | `0/1`/`STRING` |
| 主键 | `STRING` | 自增数值 |
| 数量/序号 | `INTEGER`/`LONG` | `STRING` |
| 经纬度点位 | `GEOPOINT` | 两个 DECIMAL 拼接 |
| 固定分类/状态 | `STRING` + description 列枚举值 | 散落数字 |
| 文档/附件引用 | `MEDIA_REFERENCE`/`ATTACHMENT` | `STRING` 存路径 |

## 与其他 Skill 的关系

- **本 skill + ontology-store 工具链**：在 onto-studio 内完成从材料到落库的全流程（export/preview/import 三个工具）。冷启动建模 + 增量更新都走同一条工具链，增量更新不丢失已有成果（未列入 overwrite 的同名 OT 默认 skip）。
- **平台 Agent skill**（如 `gaia-agent-integration`）：在线调平台 MCP/REST 工具（已连接外部 Gaia 平台）
- **平台内置建模能力**：目标平台自己的 Agent 在线对话式建模

三者互补：本 skill 通过 `import_ontology` 落库的 `OntologyPayload`，与 Gaia 平台的 `export` 产物格式完全一致——可导出后提交给外部 Gaia 平台的 `import` 端点，反之亦然。转其他格式（JSON-LD、OSDK 等）见 [references/ontology-package-format.md §10.4](references/ontology-package-format.md)。
