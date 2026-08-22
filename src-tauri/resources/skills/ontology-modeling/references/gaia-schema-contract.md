# Gaia Schema 契约（产物字段对齐真相源）

> 本文档是 OntologyPackage JSON 产物的**字段级真相源**。每个字段的名称、类型、约束都严格对齐 Gaia 的 pydantic schema（`src/ontology/core/schemas/`）。产出 JSON 时逐字段核对，**不得改字段名大小写、不得自创字段**。

源 schema 文件（Gaia 仓库内，外部 Agent 看不到，本文档是其精确摘要）：
- `core/schemas/ontology.py` — Ontology / ObjectType / PropertyDef / LinkTypeDef / ObjectTypeGroup (ADR-022)
- `core/schemas/action.py` — ActionType / ActionTypeParameter / ActionRule / OntologyRule
- `core/schemas/datasource.py` — DataSource / DatasetGovernance / Credential / SyncTask
- `core/naming.py` — api_name pattern 常量

---

## 一、DataType 枚举（属性数据类型，只能取这些值）

```
STRING | INTEGER | SHORT | LONG | BOOLEAN | BYTE | FLOAT | DOUBLE | DECIMAL |
DATE | TIMESTAMP | ARRAY | STRUCT | VECTOR | GEOPOINT | GEOSHAPE |
GEOTEMPORAL_SERIES | TIME_SERIES | MEDIA_REFERENCE | ATTACHMENT
```

**红线映射**：
- 金额 → `DECIMAL`（禁 `DOUBLE`，精度丢失）
- 业务时间 → `TIMESTAMP`；纯日期 → `DATE`（禁 `STRING`）
- 布尔 → `BOOLEAN`（禁 `0/1`/`STRING`）
- 主键 → `STRING`（禁自增数值）
- 经纬度点位 → `GEOPOINT`（禁两个 DECIMAL 拼）
- 时序指标 → `TIME_SERIES`；含位置的轨迹 → `GEOTEMPORAL_SERIES`
- **大整数（>2³¹）→ `LONG`（64-bit）**——禁用 `BIG_INTEGER`/`BIGINT`/`INT64` 等变体（不在枚举里即非法，导入预检阶段 500）。如芯片 HBM/L2 容量字节、文件大小、大 ID 序列号

**整数型选择**：`BYTE`（8-bit，0~255）｜ `SHORT`（16-bit）｜ `INTEGER`（32-bit，±2³¹）｜ `LONG`（64-bit，±2⁶³）。无 `BIG_INTEGER` 别名。
- 文档/附件 → `MEDIA_REFERENCE` / `ATTACHMENT`
- 固定分类/状态 → `STRING` + 在 `description` 列出枚举值（Gaia 无独立 enumeration 类型，枚举值写在 description）

---

## 二、Ontology（本体容器）

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[A-Z][a-zA-Z0-9]{0,99}$` PascalCase | 本体命名空间，用户自选（如 S3 bucket） |
| `display_name` | string | ✅ | - | 中文友好名 |
| `description` | string | ❌ | - | 本体说明 |

> Ontology 是顶层容器，一个 OntologyPackage 只产出一个 Ontology（多本体场景产出多个包）。

---

## 三、ObjectType（对象类型）

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[A-Z][a-zA-Z0-9]{0,99}$` PascalCase | 如 `ProductionOrder` |
| `display_name` | string | ✅ | - | 中文友好名 |
| `description` | string | ❌ | - | 对象说明；敏感属性在此标注"【敏感】" |
| `primary_key` | string | ❌ | 属性 api_name | 可省略——service 从 `is_primary_key` 属性推导 |
| `title_property` | string | ❌ | 属性 api_name | 可省略——service 从 `is_title_property` 属性推导 |
| `storage_type` | enum | ✅ | `MANAGED` \| `VIRTUAL` | MANAGED=落地 Iceberg+Doris 可读写；VIRTUAL=外部联邦**只读** |
| `visibility` | enum | ❌ | `NORMAL`\|`PROMINENT`\|`HIDDEN`，默认 `NORMAL` | UI 显著度 |
| `status` | enum | ❌ | `ACTIVE`\|`ENDORSED`\|`EXPERIMENTAL`\|`DEPRECATED`，默认 `ACTIVE` | 生命周期状态 |
| `backing_dataset_api_name` | string | ❌ | snake_case dataset api_name | 主数据集；通常创建时留空，首次 link_dataset 时填 |
| `capabilities` | object | ❌ | 见下 | 增强索引开关，默认全 false |
| `project_id` | string | ❌ | runtime-only | **离线建模不产出**；导入后由管理员在 Gaia UI 分配 Project（ADR-016） |
| `properties` | array | ✅ | ≥1 | PropertyDef 数组（见下） |
| `links` | array | ❌ | - | LinkTypeDef 数组（见下），可在 ObjectType 创建后再建 |

### capabilities（ObjectTypeCapabilities）

| 字段 | 类型 | 默认 | 启用前提 |
|------|------|:---:|------|
| `graph_indexing_enabled` | bool | `false` | MANAGED + 至少一个 LinkType + indexed 属性 |
| `geotime_indexing_enabled` | bool | `false` | MANAGED + GEOPOINT/GEOSHAPE 属性（PostGIS）或 TIME_SERIES/GEOTEMPORAL_SERIES 属性（TimescaleDB） |

> Doris 基础索引对 MANAGED 类型**永远开启**，不在此开关控制。

> ⚠️ **批量 vs 单创建端点字段差异**：`ObjectTypeBatchCreate`（对应 `POST /ontologies/{ont}/object-types/create` 批量端点）**不接收** `visibility`/`status`/`capabilities`/`backing_dataset_api_name`——这些字段仅在 `ObjectTypeCreate`（单创建端点 `POST /ontologies/{ont}/object-types`）中有效。pydantic v2 默认 `extra='ignore'`，向批量端点传入这些字段会被**静默丢弃**。导入脚本应对策：如产物指定了非默认值（如 `visibility: "PROMINENT"`、`status: "EXPERIMENTAL"`、`capabilities.graph_indexing_enabled: true`），需在批量创建后走 `PATCH /ontologies/{ont}/object-types/{type_name}` 单独设置。`backing_dataset_api_name` 另可通过 `PATCH .../dataset-link`（link_dataset）绑定。
>
> 💡 **`project_id` 是 runtime-only 字段**（ADR-016）：将 ObjectType 归属到 Gaia 内部的 Space/Project 组织容器——离线建模时这些容器不存在。产物中**不产出** `project_id`；导入后由管理员在 Gaia UI 中分配到对应 Project。

---

## 三·补、ObjectTypeGroup（对象类型分组）

> ObjectTypeGroup 是本体内的**纯语义分类原语**（ADR-022），把 ObjectType 归组（如 ER 域下的「应收对象组」），帮用户搜索/浏览本体。对齐 Gaia `ObjectTypeGroupCreate`（创建端点 `POST /ontologies/{ont}/object-type-groups`）。
>
> ⚠️ **定位与语义边界（必须遵守）**：
> - **纯分类，无任何权限语义**——可见性继承所属 Ontology（能看本体就能看到它全部 Group）。
> - **与权限模型正交**：权限细分靠 `Project`（RBAC）+ `Marking`（MAC），**不能用 Group 表达权限**（permission-governance-design.md）。
> - **M:N 成员关系**：一个 ObjectType 可属多个 Group，一个 Group 含多个 ObjectType。这是元数据分组关联（`ObjectTypeGroupMemberModel`），**不是业务 LinkType**——与本 skill「M:N 拆中间实体」规则不冲突（那条规则管 LinkType，不管分组缓存表）。
> - 不涉及 DataSource/Dataset/物理存储，是纯分类元数据。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[A-Z][a-zA-Z0-9]{0,99}$` PascalCase | 组名，**本体范围内唯一**，**不可改**（重命名须删除后重建，与 Palantir api_name 稳定标识约定一致） |
| `display_name` | string | ✅ | - | 中文友好名 |
| `description` | string | ❌ | 默认 `""` | 组说明 |
| `members` | array[string] | ❌ | 默认 `[]` | 成员 ObjectType 的 api_name 列表（读取/响应视图 `ObjectTypeGroupWithMembers` 携带；创建接口返回该视图） |

> 批量/单端点字段无此差异（Group 只有 `POST /ontologies/{ont}/object-type-groups` 一条创建路径）。`ObjectTypeGroupUpdate` 仅支持改 `display_name`/`description`（`api_name` 不可变）。
>
> **成员管理的两条幂等端点**（ADR-022）：
> - `POST /ontologies/{ont}/object-type-groups/{group_name}/members`，body `{object_types: [api_name,...]}`，幂等——已在组的成员是 no-op，全部对象必须属同一本体否则整单拒绝（无部分分配）
> - `DELETE /ontologies/{ont}/object-type-groups/{group_name}/members/{type_name}`，删单个成员，幂等

---

## 四、PropertyDef（属性定义，嵌在 ObjectType.properties 里）

> 产物里的属性字段严格对齐 Gaia `PropertyInput`（批量创建 ObjectType 时用的输入 schema，对应端点 `POST /ontologies/{ont}/object-types/create`）。
>
> **关于 `nullable` / `indexed`**：`PropertyInput`（批量导入路径）**不接受**这两个字段——导入时由 Gaia 推导（主键隐含 `nullable=false`；`indexed` 由 `searchable` 推导，见 service 层 `indexed=dprop.searchable`）。所以**产物里不要写 `nullable` / `indexed`**。注意：Gaia 的单属性创建 schema `PropertyDefCreate` 和读取 schema `PropertyDef` **是**有这两个字段的，但本产物走批量导入路径，不走单属性创建，故不产出。
>
> `searchable`（是否可搜索/过滤）**是** `PropertyInput` 的合法字段，会被 service 映射为 ORM 的 `indexed`——产物里可写，默认 `true`。
>
> ⚠️ **`data_type` 实际为自由 `str`**：`PropertyInput.data_type` 在 pydantic schema 中类型为 `str`（非 `DataType` 枚举），而 `ActionTypeParameter.data_type` 才是 `DataType` 枚举。本产物仍按 DataType 枚举约束（更严格），传入合法枚举值即可。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `display_name` | string | ✅ | - | 中文友好名 |
| `api_name` | string | ❌ | `^[a-z][a-zA-Z0-9]{0,99}$` camelCase | 可省略——service 从 display_name/backing_column 推导；**中文 display_name 必须显式给 api_name** |
| `description` | string | ❌ | - | 属性说明；枚举值在此列出（如"状态：DRAFT/SUBMITTED/APPROVED"）；敏感属性标注"【敏感】" |
| `data_type` | string | ✅ | DataType 枚举值（注：pydantic 层为 `str`，但必须为合法枚举值） | 见第一节 |
| `searchable` | bool | ❌ | 默认 `true` | 是否可搜索/过滤 |
| `is_primary_key` | bool\|null | ❌ | - | **每个 ObjectType 有且仅一个为 true**；为 true 时 Gaia 自动设 `nullable: false` |
| `is_title_property` | bool\|null | ❌ | - | 标题属性，每个 ObjectType 至多一个 |
| `backing_mapping` | object\|null | ❌ | 见下 | 物理列引用；VIRTUAL/未绑定数据源时为 null |
| `vector_config` | object\|null | ❌ | 仅 `data_type=VECTOR` 时填 | 语义检索配置 |

### backing_mapping（BackingColumnRef，物理列引用）

| 字段 | 类型 | 必填 | 说明 |
|------|------|:---:|------|
| `dataset_api_name` | string | ❌ | 所属 Dataset 的 api_name（snake_case），可空串 |
| `backing_catalog` | string | ✅ | 数据联邦层注册的 catalog 名（即目标平台对接外部数据源时注册的顶层目录名，**不是**数据源引擎自身的库名。如 PostgreSQL 数据源注册为 `xxx_postgres`，则填注册名而非 PG 的 database 名） |
| `backing_schema` | string | ✅ | 物理表所在 schema 名（引擎层概念，如 PostgreSQL 的 schema、MySQL 的 database） |
| `backing_table` | string | ✅ | 物理表名（snake_case） |
| `backing_column` | string | ✅ | 物理列名（snake_case） |

> 五个键必须都存在（值可为空串），缺键会导致校验失败。
>
> **三段式定位语义**：`backing_catalog`（联邦层目录）→ `backing_schema`（引擎层库/schema）→ `backing_table`（表）→ `backing_column`（列）。前两层属于不同抽象层级：catalog 是数据联邦的顶层命名空间（跨引擎统一编目），schema 是引擎内部的命名空间。切勿把引擎库名填进 `backing_catalog`。

### vector_config（VectorPropertyConfig，仅 VECTOR 属性）

| 字段 | 类型 | 必填 | 说明 |
|------|------|:---:|------|
| `dimension` | int | ❌ | 默认 384，须与 EmbeddingProvider.dim 一致 |
| `similarity_function` | enum | ❌ | `cosine`\|`l2`，默认 `cosine` |
| `source_expression` | array[string] | ✅（语义必填） | 拼接成 embedding 输入文本的 api_name 列表，如 `["name","description"]`。pydantic 层有 `default_factory=list` 不强制，但**语义上 VECTOR 属性必须提供**——否则 embedding 输入文本无来源 |

> 非 VECTOR 属性 `vector_config` 必须为 `null`。

---

## 五、LinkTypeDef（关系类型）

> LinkType 有两种导入路径，产物字段需兼顾两者：
>
> 1. **随 ObjectType 批量创建**（推荐）：放在 `ObjectTypeBatchCreate.links[]`，source 自动取当前正在创建的 ObjectType——**此时只需 `target_object_type_api_name`**。对应端点 `POST /ontologies/{ont}/object-types/create`。注意批量路径用的 `LinkInput` schema **没有 `description`/`foreign_key_property_api_name`** 字段（只有 display_name/api_name/target/cardinality/direction/weight_property/temporal）。
> 2. **独立创建**（source/target 都已存在的跨对象关系）：对应端点 `POST /ontologies/{ont}/link-types`，schema 为 `LinkTypeDefCreate`，需要 source + target 两个 UUID。
>
> 两种路径的 Gaia 内部 schema 都用 **UUID**（`source_object_type_id`/`target_object_type_id`）引用 ObjectType，**不直接接受 api_name**。产物里用 api_name 是为了人类可读 + 跨包稳定，导入时由导入脚本先查 `GET /ontologies/{ont}/object-types` 拿到 api_name→id 映射，再转换。
>
> ⚠️ **批量路径 `LinkInput` 的类型宽松**：`LinkInput.cardinality` 和 `LinkInput.direction` 在 pydantic schema 中为自由 `str`（非 `Literal` 枚举），而独立创建路径 `LinkTypeDefCreate` 才是 `Literal["ONE","MANY"]` / `Literal["OUTGOING","INCOMING"]`。本产物仍按枚举约束（更严格），传入合法枚举值即可。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `display_name` | string | ✅ | - | 中文友好名 |
| `api_name` | string | ❌ | `^[a-z][a-zA-Z0-9]{0,99}$` camelCase | 动词短语（`supplies`/`contains`）；中文必须显式给 |
| `description` | string | ❌ | - | 关系说明（仅独立创建路径接受；批量路径 `LinkInput` 无此字段，导入脚本可拼进 display_name 或丢弃） |
| `source_object_type_api_name` | string | ✅ | PascalCase | 源 ObjectType api_name；批量路径下导入脚本可校验==当前 OT |
| `target_object_type_api_name` | string | ✅ | PascalCase | 目标 ObjectType api_name；导入脚本解析为 UUID |
| `foreign_key_property_api_name` | string\|null | ❌ | camelCase | 外键属性 api_name（仅独立创建路径 `LinkTypeDefCreate` 接受；批量路径 `LinkInput` 无此字段） |
| `cardinality` | enum | ✅ | `ONE`\|`MANY`（注：批量路径 `LinkInput` 为 `str`，但须为合法枚举值） | 目标端计数；1:1=ONE，1:N=MANY。**无 MANY:MANY** |
| `direction` | enum | ✅ | `OUTGOING`\|`INCOMING`（注：批量路径 `LinkInput` 为 `str`，但须为合法枚举值） | 从源到目标的方向 |
| `weight_property` | string\|null | ❌ | camelCase | 权重属性名（图遍历加权），null=等权 |
| `temporal` | bool | ❌ | 默认 `false` | 是否时态关系（含有效期） |

> 产物里 LinkType 用 `*_api_name` 引用 ObjectType（人类可读 + 跨包稳定）。Gaia 现有端点不直接接受 api_name，导入脚本需做 `api_name → id` 转换。

---

## 六、ActionType（动作类型，可选/进阶）

> Action schema 较重，外部从文档抽象 Action 难度高。**核心产物可不包含 Action**，仅当材料中有明确的业务操作流程时才产出。
>
> 对齐 Gaia `ActionTypeCreate`（创建端点 `POST /actions/definitions/{ontology}/{action_type}`）。`api_name` 同时出现在 URL path 和 request body 中：pydantic schema 的 `api_name` 是**必填字段**，必须出现在 body 里才能通过反序列化校验；route 随后会用 URL path 的值覆盖 body 值（`definition.api_name = action_type`）。因此导入脚本**两个位置都必须传** `api_name`（值保持一致即可）。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[a-z][a-zA-Z0-9]{0,99}$` camelCase | 如 `submitOrder`；导入时放进 URL path |
| `display_name` | string | ✅ | - | 中文友好名 |
| `description` | string | ❌ | 默认 `""` | 动作说明 |
| `affected_object_type_api_name` | string | ✅ | PascalCase | 主要影响的 ObjectType **api_name**（注意：`ActionTypeCreate` 中为必填 string，不是可选、不是 UUID） |
| `parameters` | array[ActionTypeParameter] | ❌ | 默认 `[]` | 入参定义（见下） |
| `rules` | array[ActionRule] | ❌ | 默认 `[]` | 派生/约束/校验规则（见下） |
| `submission_criteria` | array[SubmissionCriterion] | ❌ | 默认 `[]` | 提交前置条件（**可执行表达式**，见下） |
| `effects` | array[ActionEffectConfig] | ❌ | 默认 `[]` | 副作用配置（见下） |
| `ontology_rules` | array[OntologyRule] | ❌ | 默认 `[]` | 声明式变更规则（见下） |
| `risk_level` | enum | ❌ | `low`\|`medium`\|`high`，默认 `low` | 驱动 HITL 审批：low=免审批，medium=列影响确认，high=输入名称确认 |
| `operation_kind` | enum | ❌ | `create`\|`update`\|`delete`\|`mixed`，默认 `mixed` | 操作分类 |
| `batch_enabled` | bool | ❌ | 默认 `false` | 是否支持批量执行 |

### ActionTypeParameter（动作入参）

> 对齐 Gaia `ActionTypeParameter`。`api_name` pattern 强制校验（`^[a-z][a-zA-Z0-9]{0,99}$`）。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[a-z][a-zA-Z0-9]{0,99}$` camelCase | 参数名 |
| `display_name` | string | ✅ | - | 中文友好名 |
| `data_type` | string | ✅ | DataType 枚举 | 参数类型 |
| `required` | bool | ❌ | 默认 `true` | 是否必填 |
| `default` | any | ❌ | 默认 `null` | 静态默认值（`default_source != static` 时被忽略） |
| `description` | string | ❌ | 默认 `""` | 参数说明 |
| `default_source` | enum | ❌ | `static`\|`current_user`\|`current_timestamp`\|`workspace_id`\|`selected_object_field`，默认 `static` | 动态默认值来源 |
| `default_source_field` | string\|null | ❌ | - | `default_source=selected_object_field` 时读取的属性名 |
| `readonly` | bool | ❌ | 默认 `false` | 表单只读 |
| `hidden` | bool | ❌ | 默认 `false` | 表单隐藏 |
| `pattern` | string\|null | ❌ | - | 正则约束 |
| `error_message` | string\|null | ❌ | - | 校验失败时的自定义错误信息 |
| `enum_values` | array[string]\|null | ❌ | - | 枚举值（data_type=STRING 时） |
| `object_type_ref` | string\|null | ❌ | PascalCase | 引用的 ObjectType api_name（参数持有该对象 id） |
| `is_object_set` | bool | ❌ | 默认 `false` | true=对象集合（id 列表） |

### ActionRule（派生/约束/校验规则）

> 对齐 Gaia `ActionRule`。用于参数间派生计算、参数组合约束、业务规则校验。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `type` | enum | ✅ | `constraint`\|`derivation`\|`validation` | 规则类型：constraint=校验参数组合，derivation=从已有参数算新值，validation=业务规则校验 |
| `target` | string | ✅ | - | 目标参数名或属性名 |
| `expression` | string | ✅ | - | safeeval 表达式，如 `"value > 0"`、`"unit_price * quantity"` |
| `description` | string | ❌ | 默认 `""` | 规则说明 |

### SubmissionCriterion（提交前置条件）

> 对齐 Gaia `SubmissionCriterion`。在参数校验 + 规则求值之后、mutation 应用之前运行的全局校验。**是可执行表达式，不是只命名**——这与早期 Palantir 语义层不同。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `expression` | string | ✅ | - | simpleeval 表达式，如 `"quantity > 0 and status == 'open'"` |
| `error_message` | string | ✅ | min_length=1 | 校验失败时返回给用户的错误信息 |
| `description` | string | ❌ | 默认 `""` | 条件说明 |

> 旧版本文档曾说 submission_criteria 是 `{name, description}`「只命名不写实现」——**这是错误的**，已修正。Gaia 的 submission_criteria 必须写可执行 `expression`。

### OntologyRule（声明式变更规则）

> 对齐 Gaia `OntologyRule`（`action.py`）。声明式描述「对哪个对象、按什么主键匹配、做 CREATE/UPDATE/Upsert/DELETE、属性值从哪来」，由 `ActionService._build_mutations_from_rules` 解析为 Mutation。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `type` | enum | ✅ | `CreateObject`\|`ModifyObject`\|`UpsertObject`\|`DeleteObject`\|`CreateLink`\|`DeleteLink` | 变更类型 |
| `target_parameter` | string\|null | ❌ | 参数 api_name | Modify/Upsert/Delete 用：取该 ObjectReference 参数的值作主键，匹配 `ObjectType.primary_key` |
| `target_object_type` | string\|null | ❌ | PascalCase | Create 用：显式目标对象类型 api_name |
| `target_path` | string\|null | ❌ | - | 跨对象路径（本期不支持，执行期忽略；字段保留向后兼容） |
| `properties` | object | ❌ | 默认 `{}` | {属性 api_name: ValueSource}；**主键不可出现在 Modify 的 properties** |
| `link_type` | string\|null | ❌ | camelCase | CreateLink/DeleteLink 专用：关系 api_name |
| `source_parameter` | string\|null | ❌ | 参数 api_name | CreateLink/DeleteLink 专用：源端 ObjectReference 参数 |
| `target_link_parameter` | string\|null | ❌ | 参数 api_name | CreateLink/DeleteLink 专用：目标端 ObjectReference 参数 |
| `condition` | string\|null | ❌ | - | simpleeval 条件表达式（如 `"$isUrgent = true"`）；null=无条件执行 |
| `on_missing` | enum | ❌ | `raise_not_found`\|`create`，默认 `raise_not_found` | Upsert 命中 0 行时的行为 |
| `description` | string | ❌ | 默认 `""` | 规则说明 |

> ⚠️ 旧版本文档曾用字段名 `object_type_api_name`/`match_key`/`match_value_source`/`property_values`/`link_type_api_name`/`target_object_type_api_name`——**这些字段名在 Gaia schema 中不存在**，已修正为上表的真实字段名。

### ValueSource（属性值来源）

> 对齐 Gaia `ValueSource`。出现在 `OntologyRule.properties` 的值里。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `source` | enum | ✅ | `PARAMETER`\|`OBJECT_PROPERTY`\|`STATIC_VALUE`\|`SYSTEM_CONTEXT`\|`SYSTEM_GENERATED`\|`EXPRESSION` | 值来源类型 |
| `value` | string\|null | ❌ | 默认 `null` | 各 source 的 value 语义见下 |

**各 source 的 `value` 语义**：
- `PARAMETER` → value=参数 api_name，如 `"delay_minutes"`
- `OBJECT_PROPERTY` → value=`"参数名.属性名"`，如 `"newAircraft.status"`（读 ObjectReference 参数引用对象的属性）
- `STATIC_VALUE` → value=字面量，如 `"Delayed"`、`"PENDING"`
- `SYSTEM_CONTEXT` → value ∈ `{CURRENT_USER_ID, CURRENT_TIMESTAMP}`
- `SYSTEM_GENERATED` → value=`"uuid"`（生成主键）
- `EXPRESSION` → value=simpleeval 表达式，命名空间=所有参数 + 引用对象属性

### ActionEffectConfig（副作用配置）

> 对齐 Gaia `ActionEffectConfig`。可选的副作用，在本体变更 BEFORE/AFTER 触发。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `type` | enum | ✅ | `webhook`\|`write_back`\|`sub_action`\|`kafka_topic`\|`notification` | 副作用类型 |
| `config` | object | ❌ | 默认 `{}` | 类型特定的配置 |
| `trigger` | enum | ❌ | `BEFORE_ONTOLOGY_CHANGE`\|`AFTER_ONTOLOGY_CHANGE`，默认 `AFTER_ONTOLOGY_CHANGE` | 触发时机 |
| `condition` | string\|null | ❌ | - | simpleeval 条件表达式；null=无条件 |

> **关于 `effects` vs Palantir `side_effects`**：Gaia 的字段叫 `effects`（不是 `side_effects`），是 `ActionTypeCreate` 的合法可选字段。SKILL 主文档红线禁止的是运行时策略字段（idempotent/retry/timeout/rollback），不是禁止 effects——产出 Action 时如有外部副作用需求可正常使用 `effects`。

> **禁止字段**（任一出现即红线）：`idempotent`、`atomic`、`retry_strategy`、`rollback_action`、`timeout_seconds`。这些运行时策略属于 Gaia Function 层，不属于 ActionType 定义。也禁止 Palantir 的 `modifies`/`constraints`/`side_effects`（用 `ontology_rules`/`rules`/`effects` 替代）。

---

## 七、DataSource（外部数据源连接）

> 对齐 Gaia `DataSourceCreate`（创建端点 `POST /api/datasources`）。`status`/`gravitino_catalog_name`/`capabilities` 是读取 schema `DataSource` 的派生/运行时字段，创建时不传。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[a-z][a-z0-9_]{0,99}$` snake_case | 如 `erp_supplier_master` |
| `display_name` | string | ✅ | - | 中文友好名 |
| `description` | string | ❌ | - | 数据源说明 |
| `connector_type` | string | ✅ | 见下表 | 连接器类型，决定 capabilities |
| `connector_config` | object | ❌ | - | 连接配置；**敏感字段（password/access_key/token 等）用 `***` 占位** |
| `credential_id` | string\|null | ❌ | - | 凭据 id，产物里通常留 null（导入后绑定） |

### connector_type 取值与 capabilities

| connector_type | 类别 | capabilities |
|----------------|------|--------------|
| `mysql`/`mariadb`/`postgresql`/`postgres` | relational | explore, batch_sync, cdc, virtual_table |
| `opengauss`/`gaussdb`/`tidb` | relational（国产） | explore, batch_sync, cdc, virtual_table |
| `oceanbase`/`starrocks`/`kingbase` | relational（国产） | explore, batch_sync, virtual_table |
| `dameng` | relational（国产） | explore, batch_sync（无 Gravitino provider） |
| `generic_jdbc` | generic | explore, batch_sync |
| `iceberg`/`delta`/`hudi`/`paimon` | lakehouse | explore, virtual_table |
| `hive` | lakehouse | explore, batch_sync, virtual_table |
| `s3`/`minio`/`oss`/`hdfs` | file_object | explore, file_sync |
| `kafka` | messaging | explore, streaming_sync, virtual_table |
| `elasticsearch` | nosql | explore, batch_sync |
| `analyticdb_pg`/`gaussdb_dws` | cloud_warehouse | explore, batch_sync, virtual_table |
| `maxcompute` | cloud_warehouse | explore, batch_sync |

> capabilities 由 connector_type 自动推导，产物里**不需要**显式写 `capabilities` 字段。
>
> ⚠️ **`connector_type` 实际为自由 `str`**：`DataSourceCreate.connector_type` 在 pydantic schema 中类型为 `str`（非 Literal 枚举），上表列出的是 Gaia 后端 `CAPABILITY_MAP` 中已注册的值。传入未注册的 connector_type 不会 422（pydantic 层面通过），但能力推导会回退到默认 `["explore"]`。产物请使用上表中的值以确保能力正确。

---

## 八、DatasetGovernance（数据集治理记录）

> 对齐 Gaia `DatasetGovernanceCreate`（创建端点 `POST /api/datasets`）。`row_count_estimate` 是运行时统计字段，创建时不传。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[a-z][a-z0-9_]{0,99}$` snake_case | 兼任物理 Iceberg 表名，全小写保词界 |
| `display_name` | string | ❌ | - | 中文友好名 |
| `storage_location` | string | ❌ | - | VIRTUAL 用三段式 `catalog.schema.table`；MANAGED 留空（平台自动生成）。三段式第一段 `catalog` = 数据联邦层注册的顶层目录名（与 `backing_mapping.backing_catalog` 同源，**不是**引擎库名），第二段 `schema` = 引擎层 schema/库名，第三段 `table` = 物理表名。三段必须与同 Dataset 下属性的 `backing_mapping` 三段完全一致（catalog/schema/table 对齐） |
| `partition_config` | object\|null | ❌ | - | 分区配置 |
| `source_dataset_api_name` | string\|null | ❌ | snake_case | 上游数据集（lineage） |
| `data_source_api_name` | string\|null | ❌ | snake_case | 来源数据源 |
| `kind` | enum | ❌ | `MANAGED`\|`VIRTUAL`，默认 `MANAGED` | 资源类型 |
| `is_view` | bool | ❌ | 默认 `false` | Managed Table 子类型标记，当前恒 false |

---

## 九、Credential（凭据，可选）

> 产物里**通常不产出 Credential**——凭据应在导入 Gaia 后由管理员配置。若材料中明确给出凭据结构，可产出占位结构。对齐 Gaia `CredentialCreate`（端点 `POST /api/credentials`）。
>
> 注意：`credential_type` 在 Gaia schema 中是**自由字符串**（`str`，无 Literal 枚举约束），下表的三个取值是**约定**而非强制校验。

| 字段 | 类型 | 必填 | 约束 | 说明 |
|------|------|:---:|------|------|
| `api_name` | string | ✅ | `^[a-z][a-z0-9_]{0,99}$` snake_case | 凭据名 |
| `credential_type` | string | ✅ | 约定 `username_password`\|`access_key`\|`token`（schema 不强制枚举） | 凭据类型 |
| `secret_data` | object | ✅ | - | **必须用占位符** `{}` 或 `{"password":"***"}`，禁真实值 |

---

## 十、字段大小写速查（最易错）

| 实体 | api_name pattern | 示例 |
|------|------------------|------|
| Ontology | PascalCase `^[A-Z]...` | `SupplyChain` |
| ObjectType | PascalCase `^[A-Z]...` | `PurchaseOrder` |
| ObjectTypeGroup | PascalCase `^[A-Z]...` | `CoreEntities` |
| Property | camelCase `^[a-z]...` | `orderDate` |
| LinkType | camelCase `^[a-z]...` | `supplies` |
| ActionType | camelCase `^[a-z]...` | `submitOrder` |
| Action 参数 | camelCase `^[a-z]...` | `customerId` |
| Dataset | snake_case `^[a-z][a-z0-9_]*$` | `purchase_order` |
| DataSource | snake_case `^[a-z][a-z0-9_]*$` | `erp_master` |
| Credential | snake_case `^[a-z][a-z0-9_]*$` | `erp_credential` |
| 物理列名（backing_column） | snake_case | `supplier_id` |
