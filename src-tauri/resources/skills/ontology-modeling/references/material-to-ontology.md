# 从业务材料到本体的方法论

> 把 Word/PDF/PPT/Excel/TXT 文档、业务描述、结构化数据源 schema 抽象成 OntologyPackage 的实战方法论。

## 一、六步法总览

```
① 通读材料 → ② 识别实体 → ③ 识别关系 → ④ 识别动作 → ⑤ 绑定数据 → ⑥ 自检产出
```

每一步都产出中间产物，最后汇总成 OntologyPackage JSON。

## 二、按材料类型分流的抽取策略

### 2.1 非结构化文档（Word / PDF / TXT）

**特征**：自然语言段落，业务方案/需求文档/操作手册/规章制度。

**抽取策略**：
1. **名词扫描**：圈出所有"业务名词"（供应商、订单、工单、客户）。这些是 ObjectType 候选。
2. **动词扫描**：圈出"业务动作"（提交订单、审批、发货、归档）。这些是 ActionType 候选。
3. **属性提取**：每个名词的描述里提取特征（"订单包含订单号、下单时间、金额"）→ PropertyDef 候选。
4. **关系识别**：找"属于/包含/关联/隶属于"等表述 → LinkType 候选。注意识别 M:N（"员工可以担任多个岗位，每个岗位可有多人" → 拆 `EmployeePostRel`）。
5. **状态识别**：找"状态/阶段/级别"等 → STRING + description 列枚举值。

**易错点**：
- ❌ 把"操作手册步骤"当 ObjectType（"第一步：登录系统"不是实体）
- ❌ 把"系统模块名"当 ObjectType（"采购模块"是功能划分，不是业务实体）
- ✅ 只取**有独立业务身份、能被唯一标识、有生命周期**的名词

### 2.2 半结构化文档（Excel / CSV）

**特征**：数据字典、字段说明表、清单。

**抽取策略**：
1. **一行一实体候选**：Excel 的 sheet/区块标题常对应 ObjectType，列对应 PropertyDef。
2. **列名→display_name，列说明→description，数据类型→DataType**：
   - "金额/价格/费用"列 → `DECIMAL`
   - "时间/日期"列 → `TIMESTAMP`/`DATE`
   - "是/否"列 → `BOOLEAN`
   - "状态"列 → `STRING` + description 列枚举值
3. **主键列识别**：标注"主键/ID/编号"的列 → `is_primary_key: true`，DataType 强制 `STRING`。
4. **关联列识别**：形如"供应商ID"且引用另一张表的 → 外键属性，对应 LinkType。

**易错点**：
- ❌ 把 Excel 的每一张 sheet 都建成 ObjectType（有些 sheet 是配置/字典/中间计算）
- ❌ 把"行号/序号"当主键（业务主键应是唯一业务编号）
- ✅ 区分"主数据表"（→ MANAGED ObjectType）和"事务表"（→ MANAGED ObjectType，通常 1:N 被引用）

### 2.3 演示文档（PPT）

**特征**：业务架构图、流程图、列表要点。

**抽取策略**：
1. **架构图节点** → ObjectType 候选；**连线** → LinkType 候选。
2. **流程图步骤** → ActionType 候选（不是 ObjectType！步骤是动作）。
3. **列表要点** → ObjectType 的属性或枚举值。
4. PPT 信息密度低，常需配合 Word/Excel 补全属性细节。

### 2.4 业务描述（自然语言段落/口述）

**特征**：用户口述或文字描述业务场景。

**抽取策略**：同 2.1，但更依赖 LLM 的语义理解：
1. 解析"主谓宾"：主语/宾语 → ObjectType，谓语 → LinkType 或 ActionType。
2. 解析"每个X有Y个Z"：识别基数（1:1 / 1:N / M:N）。
3. 解析"当...时执行..."：识别 ActionType 的 `submission_criteria`（前置条件，写可执行 simpleeval 表达式）。
4. 解析"敏感/机密/隐私"：标注敏感属性。

### 2.5 结构化数据源 schema（DDL / 表结构 / API 报文 / JSON Schema）

**特征**：已有物理表结构、接口定义。

**抽取策略**：
1. **物理表 → ObjectType 候选**（但需业务化重命名：`t_sup_master` → `Supplier`）。
2. **物理列 → PropertyDef**，类型映射见下表。
3. **物理主键 → `is_primary_key`**，但 DataType 统一转 `STRING`（主键强制 STRING）。
4. **物理外键 → LinkType**（cardinality 由业务判断，不是物理 FK 约束）。
5. **视图/中间表 → 不建 ObjectType**（除非有独立业务身份）。
6. **API 报文字段 → PropertyDef**（若报文对应业务实体）。

**SQL 类型 → DataType 映射**：

| SQL 类型 | DataType | 备注 |
|----------|---------------|------|
| `VARCHAR`/`CHAR`/`TEXT`/`CLOB` | `STRING` | |
| `INT`/`INTEGER` | `INTEGER` | |
| `BIGINT` | `LONG` | |
| `SMALLINT` | `SHORT` | |
| `DECIMAL`/`NUMERIC` | `DECIMAL` | 金额必须用 |
| `FLOAT` | `FLOAT` | |
| `DOUBLE`/`REAL` | `DOUBLE` | 金额禁用，改 `DECIMAL` |
| `BOOLEAN`/`BIT` | `BOOLEAN` | |
| `DATE` | `DATE` | |
| `TIMESTAMP`/`DATETIME` | `TIMESTAMP` | |
| `BYTEA`/`BLOB` | `BYTE` | |
| `JSON`/`JSONB` | `STRUCT` 或 `STRING` | 结构化用 STRUCT，否则 STRING 存原文 |
| `ARRAY` | `ARRAY` | |
| `UUID` | `STRING` | |

## 三、关系基数判定

| 业务表述 | 基数 | 处理 |
|----------|------|-----------|
| "一个订单包含多个订单明细" | 1:N | `Order --contains(MANY)--> OrderItem` |
| "每个员工有一个主岗位" | 1:1 | `Employee --hasPrimaryPost(ONE)--> Post` |
| "一个供应商供应多种物料，一种物料有多个供应商" | M:N | 拆中间实体 `SupplierMaterialRel`（含主键+供应商ID+物料ID+价格）+ 两组 1:N |
| "订单归属于客户" | N:1 | `Order --belongsTo(MANY)--> Customer`（一对多关系中存在 MANY 端：一个 Customer 有多个 Order。cardinality 标记「多」在哪端存在，不区分 1:N/N:1） |

**M:N 拆分标准**：
- 中间实体命名：`主体+客体+Rel`（如 `EmployeePostRel`）
- 中间实体必含：自身主键（STRING）+ 两端外键属性
- 如关系本身附带属性（价格/级别/数量/生效时间等），放在中间实体上
- **纯 M:N 无附加属性**（如「学生选课」「用户加群」）：中间实体**仍必须创建**（仅含 PK + 两端 FK），不能跳过直接建 M:N LinkType——否则导入时平台校验失败
- 拆分后建两组 1:N：中间实体 → 两端各一个 `MANY`

> **Cardinality 语义说明**：Gaia 的 `cardinality` 字段**不区分 1:N 还是 N:1**——只要关系中存在「多」的一端，都填 `MANY`。`ONE` 仅用于严格 1:1。关系的实际方向由 `source_object_type_api_name`/`target_object_type_api_name` + `direction` 组合决定，`cardinality` 只标记基数大小，不代表「目标端有多个」。

## 四、storage_type 判定

| 场景 | storage_type | 理由 |
|------|:---:|------|
| 数据需要平台管理、可写入、可建索引 | `MANAGED` | 平台托管存储，可读写 |
| 数据在外部系统、只读查询、不想搬运 | `VIRTUAL` | 联邦查询，零落地，**禁写** |
| 高频写入的业务单据（订单/工单） | `MANAGED` | Action 写入需落地 |
| 只读的主数据（供应商/物料字典）且已有权威源 | `VIRTUAL` 可选 | 视是否需要写入/索引而定 |
| 需要图遍历/空间查询的 | `MANAGED` | graph/geotime capability 仅 MANAGED 支持 |

> **默认 MANAGED**。仅当明确"只读 + 已有外部权威源 + 不需平台索引增强"时才选 VIRTUAL。

## 五、动作（ActionType）抽取要点

Action 是**进阶产物**，仅当材料中有明确业务操作流程时才产出：

1. **识别业务操作**：从"提交/审批/创建/修改/删除/取消/激活"等动词识别。
2. **声明语义契约**（不写运行时实现）：
   - `parameters`：操作需要什么输入（如 `customerId`、`items`）
   - `affected_object_type_api_name`：主要影响哪个 ObjectType（**必填**，api_name）
   - `ontology_rules`：声明式变更（CreateObject/ModifyObject/...），字段用 `target_parameter`/`target_object_type`/`properties`/`link_type`/`source_parameter`/`target_link_parameter`（**不是**旧文档的 object_type_api_name/match_key/property_values）
   - `submission_criteria`：前置条件，写**可执行 simpleeval 表达式** `{expression, error_message, description}`（如 `"quantity > 0"`），不是只命名
   - `rules`：派生/约束/校验规则（可选）
   - `effects`：副作用配置（可选，字段名 `effects` 不是 `side_effects`）
3. **标注风险等级**：
   - `low`：查询/只读操作（免审批）
   - `medium`：单条创建/修改（列影响确认）
   - `high`：删除/批量操作（输入名称确认）
4. **绝不写运行时策略**：`idempotent`/`retry`/`timeout`/`rollback` 属于平台 Function 层。

> 如果材料里只有"业务流程描述"而无明确操作语义，**宁可产出空 action_types，也不要硬造**。Action 可在导入后由业务方补充。

## 六、数据源绑定要点

1. **MANAGED ObjectType**：
   - 产出对应 Dataset（api_name = ObjectType 的 snake_case，如 `PurchaseOrder` → `purchase_order`）
   - `kind: MANAGED`，`storage_location` 留空（平台自动生成）
   - 属性的 `backing_mapping` 指向该 Dataset 的物理列（`backing_column` 用 snake_case）
2. **VIRTUAL ObjectType**：
   - 产出对应 Dataset，`kind: VIRTUAL`，`storage_location` = `catalog.schema.table`（三段式）
   - **三段式取值规则**：第一段 `catalog` = 数据联邦层注册的顶层目录名（目标平台对接外部数据源时注册的名字，**不是**引擎库名——如 PostgreSQL 数据源注册为 `xxx_postgres`，填注册名而非 PG 的 database 名）；第二段 `schema` = 引擎层 schema/库名（如 PG 的 schema、MySQL 的 database）；第三段 `table` = 物理表名。三段必须与该 Dataset 下属性的 `backing_mapping.backing_catalog`/`backing_schema`/`backing_table` 完全一致——`storage_location` 与 `backing_mapping` 是同一物理表的两种引用方式，catalog 段不一致会导致联邦查询 Catalog not found。
   - 绑定一个 DataSource（`connector_type` 按 2.5 节映射）
   - 属性 `backing_mapping` 指向外部表的物理列
3. **DataSource**：
   - `connector_config` 里敏感字段（password/access_key/token）**必须用 `***` 占位**
   - 不产出真实凭据；凭据在导入目标平台后由管理员配置

## 七、示例：从采购场景文档到产物

**材料片段**：
> 采购系统管理供应商、物料和采购订单。每个供应商有供应商编号、名称、联系人、税号（敏感）。物料有物料编码、名称、规格、单价。一个采购订单属于一个供应商，包含多个订单明细；每个明细引用一种物料，记录数量和成交价。一个供应商可供应多种物料，一种物料可由多个供应商供应（记录供货价）。操作包括：创建采购订单、审批订单、取消订单。

**抽取过程**：

1. **实体识别**：Supplier、Material、PurchaseOrder、OrderItem、SupplierMaterialRel（M:N 拆分）
2. **关系识别**：
   - PurchaseOrder --belongsTo--> Supplier（N:1）
   - PurchaseOrder --contains--> OrderItem（1:N）
   - OrderItem --references--> Material（N:1）
   - SupplierMaterialRel --relatesTo--> Supplier（N:1）
   - SupplierMaterialRel --relatesTo--> Material（N:1）
3. **动作识别**：createPurchaseOrder、approveOrder、cancelOrder
4. **数据源绑定**：全部 MANAGED（业务单据需写入）
5. **敏感属性**：Supplier.taxId 标注"【敏感】"
6. **置信度**：成交价是否含税 → `tentative`，进 open_questions

**产物结构**（精简示意，完整格式见 ontology-package-format.md）：

```jsonc
{
  "ontology": { "api_name": "Procurement", "display_name": "采购管理" },
  "object_types": [
    {
      "api_name": "Supplier",
      "display_name": "供应商",
      "storage_type": "MANAGED",
      "properties": [
        { "api_name": "supplierId", "display_name": "供应商编号", "data_type": "STRING", "is_primary_key": true },
        { "api_name": "name", "display_name": "名称", "data_type": "STRING", "is_title_property": true },
        { "api_name": "contactPerson", "display_name": "联系人", "data_type": "STRING" },
        { "api_name": "taxId", "display_name": "税号", "data_type": "STRING", "description": "【敏感】供应商税号" }
      ]
    }
    // Material / PurchaseOrder / OrderItem / SupplierMaterialRel ...
  ],
  "link_types": [
    { "api_name": "belongsTo", "source_object_type_api_name": "PurchaseOrder", "target_object_type_api_name": "Supplier", "cardinality": "MANY", "direction": "OUTGOING" }
    // contains / references / relatesTo ×2 ...
  ],
  "action_types": [
    { "api_name": "createPurchaseOrder", "display_name": "创建采购订单", "risk_level": "medium", "operation_kind": "create" }
    // approveOrder / cancelOrder ...
  ],
  "datasets": [
    { "api_name": "supplier", "kind": "MANAGED", "data_source_api_name": null }
    // material / purchase_order / order_item / supplier_material_rel ...
  ],
  "open_questions": [
    "订单明细的成交价是否含税？(tentative: 含税)",
    "供应商税号是否需要脱敏存储？(tentative: 需要)"
  ]
}
```

## 八、自检清单（产出前必过）

- [ ] 每个 ObjectType 有且仅一个 `is_primary_key: true` 的属性？（产物里不写 `nullable`/`indexed`，由平台自动处理）
- [ ] 主键 DataType 是 `STRING`？
- [ ] 所有 api_name 符合对应 pattern（PascalCase/camelCase/snake_case）？
- [ ] 中文 display_name 都显式提供了 api_name？
- [ ] M:N 是否都拆成了中间实体 + 两组 1:N？
- [ ] 所有 `data_type` 都在 DataType 枚举里（无 `BIG_INTEGER`/`BIGINT`/`INT`/`INT64` 等变体）？金额用 `DECIMAL`？时间用 `TIMESTAMP`/`DATE`？布尔用 `BOOLEAN`？大整数（>2³¹）用 `LONG`？
- [ ] ActionType 没有混入 `idempotent`/`retry`/`timeout`/`rollback`？
- [ ] VIRTUAL ObjectType 没有定义写入动作？
- [ ] 敏感属性在 description 标注了"【敏感】"？
- [ ] DataSource 的 `connector_config` 敏感字段用了 `***` 占位？
- [ ] 所有 `tentative` 项都在 `open_questions` 里列出了？
- [ ] backing_mapping 的五个键（dataset_api_name/backing_catalog/backing_schema/backing_table/backing_column）都存在？`backing_catalog` 填的是数据联邦层注册名（非引擎库名）？
- [ ] VIRTUAL Dataset 的 `storage_location` 三段式首段 = 联邦层注册名（与 `backing_catalog` 同源对齐）？`storage_location` 与 `backing_mapping` 的 catalog/schema/table 三段完全一致？
- [ ] 所有实体的字段集与 schema 契约字段表一致（无自创字段如 `cross_domain`/`connection`/Dataset.`description`）？额外元信息写进 `description` 文本？
