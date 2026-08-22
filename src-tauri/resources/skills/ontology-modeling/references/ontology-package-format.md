# OntologyPackage 格式定义

> OntologyPackage 是外部 Agent 产出的、可被 Gaia 导入的本体包格式。本文档定义其 JSON 结构、字段约束、完整示例和导入说明。
>
> **与工具接口的关系**：onto-studio 的三个 agent 工具（`export_ontology` / `preview_ontology_import` / `import_ontology`）使用 `OntologyPayload` 格式——即 Gaia 平台 `OntologyExportPayload` 的同构格式。`OntologyPackage` 是本 skill 的**扩展格式**，在 `OntologyPayload` 基础上增加了 `metadata` / `open_questions` / `confidence` 等 SKILL 专有字段。工具调用时自动忽略这些扩展字段，只消费 Gaia 标准字段。两者的映射见 [§11 OntologyPackage 与 OntologyPayload 映射](#十一ontologypackage-与-ontologypayload-映射)。

## 一、顶层结构

```jsonc
{
  "$schema": "gaia.ontology-package/v1",
  "ontology": { /* OntologyDef — 必填，一个包一个本体 */ },
  "object_types": [ /* ObjectTypeDef[] — 必填，≥1 */ ],
  "object_type_groups": [ /* ObjectTypeGroupDef[] — 可选 */ ],
  "link_types": [ /* LinkTypeDef[] — 可选 */ ],
  "action_types": [ /* ActionTypeDef[] — 可选/进阶 */ ],
  "datasets": [ /* DatasetDef[] — 可选 */ ],
  "data_sources": [ /* DataSourceDef[] — 可选 */ ],
  "credentials": [ /* CredentialDef[] — 可选，通常不产出 */ ],
  "open_questions": [ /* string[] — tentative 项和待确认决策 */ ],
  "metadata": {
    "generated_by": "string",
    "generated_at": "ISO-8601 timestamp",
    "source_materials": ["string"],
    "confidence_summary": { "confirmed": 0, "high": 0, "tentative": 0 }
  }
}
```

**字段说明**：

- `ontology` / `object_types` 必填；其余可选。
- `object_type_groups` 是语义分类原语（ADR-022），纯分类无权限语义；产物里标注分组意图，成员绑定可在 ObjectType 创建后走独立的 Group REST 端点完成。
- `link_types` 在顶层（不在 ObjectType 内），用 `*_api_name` 引用 ObjectType。
- `open_questions` 汇总所有 `tentative` 置信度项和需人工确认的决策，**必填**（即使为空数组）。
- `metadata` 记录产物元信息，便于追溯。

## 二、OntologyDef

```jsonc
{
  "api_name": "Procurement",           // PascalCase, 必填
  "display_name": "采购管理",            // 必填
  "description": "采购业务本体"           // 可选
}
```

## 三、ObjectTypeDef

```jsonc
{
  "api_name": "PurchaseOrder",          // PascalCase, 必填
  "display_name": "采购订单",             // 必填
  "description": "记录采购交易的主单据",    // 可选
  "storage_type": "MANAGED",            // MANAGED|VIRTUAL, 必填
  "visibility": "NORMAL",               // NORMAL|PROMINENT|HIDDEN, 可选默认 NORMAL
  "status": "ACTIVE",                   // ACTIVE|ENDORSED|EXPERIMENTAL|DEPRECATED, 可选默认 ACTIVE
  "backing_dataset_api_name": "purchase_order",  // snake_case, MANAGED 推荐填写（从 OT api_name 推导）；仅数据源未确认时留空
  "capabilities": {                     // 可选, 默认全 false
    "graph_indexing_enabled": false,
    "geotime_indexing_enabled": false
  },
  "properties": [ /* PropertyDef[] — 必填, ≥1 */ ],
  "confidence": "confirmed"             // confirmed|high|tentative, 可选
}
```

### PropertyDef（嵌在 properties 数组）

```jsonc
{
  "api_name": "orderDate",              // camelCase, 可选(中文 display_name 必填)
  "display_name": "下单时间",             // 必填
  "description": "",                    // 可选; 枚举值/敏感标注在此
  "data_type": "TIMESTAMP",             // DataType 枚举, 必填
  "searchable": true,                   // 可选默认 true
  "is_primary_key": false,              // 可选; 每个 OT 有且仅一个 true (Gaia 自动设主键 nullable=false)
  "is_title_property": false,           // 可选; 每个 OT 至多一个 true
  "backing_mapping": {                  // 可选; 物理列引用（三段式定位）
    "dataset_api_name": "purchase_order",
    "backing_catalog": "procurement_pg",     // 数据联邦层注册的 catalog 名（非引擎库名；外部数据源在联邦层注册的顶层目录名）
    "backing_schema": "procurement",      // 引擎层 schema 名
    "backing_table": "purchase_order",
    "backing_column": "order_date"
  },
  "vector_config": null,                // 仅 data_type=VECTOR 时填
  "confidence": "confirmed"             // 可选
}
```

## 三·补、ObjectTypeGroupDef（可选）

> ObjectTypeGroup 是纯语义分类原语（ADR-022），**不涉及 DataSource/Dataset/物理存储**，无任何权限语义（可见性继承所属 Ontology）。离线建模产物产出 Group 定义（api_name + display_name + description + 期望的初始成员列表）；导入脚本先调 `POST /ontologies/{ont}/object-type-groups` 创建 Group，再调 `POST .../members` 批量绑定成员。
>
> ⚠️ Group 成员是 M:N 的元数据关联（`ObjectTypeGroupMemberModel`），**不是业务 LinkType**——与本 Skill「M:N 只能通过中间 ObjectType 建模」规则不冲突。

```jsonc
{
  "api_name": "Receivables",          // PascalCase, 必填，本体范围内唯一，不可改
  "display_name": "应收对象组",         // 必填
  "description": "应收账款相关对象分组",  // 可选默认 ""
  "members": ["Invoice", "Receipt", "Customer"],  // 可选默认 []，期望的初始成员 api_name 列表
  "confidence": "high"                 // 可选（SKILL 扩展字段，非 Gaia schema）
}
```

**字段**：

| 字段 | 类型 | 必填 | 约束 | 说明 |
| ------ | ------ | :---: | ------ | ------ |
| `api_name` | string | ✅ | PascalCase `^[A-Z][a-zA-Z0-9]{0,99}$` | 组名，不可改（重命名须删除后重建） |
| `display_name` | string | ✅ | - | 中文友好名 |
| `description` | string | ❌ | 默认 `""` | 组说明 |
| `members` | string[] | ❌ | 默认 `[]` | 期望的初始成员 api_name 列表（导入脚本按此调 add members，幂等） |
| `confidence` | string | ❌ | `confirmed`\|`high`\|`tentative` | SKILL 扩展字段，导入脚本剥离 |

> **导入路径**：`POST /ontologies/{ont}/object-type-groups`（body: `ObjectTypeGroupCreate`，返回 `ObjectTypeGroupWithMembers`），随后若 `members` 非空则调 `POST .../members` 批量添加（幂等）。`api_name` 不可改；`PATCH /.../object-type-groups/{group_name}` 只能改 `display_name`/`description`。

---

## 四、LinkTypeDef

```jsonc
{
  "api_name": "contains",               // camelCase, 可选(中文 display_name 必填)
  "display_name": "包含",                 // 必填
  "description": "订单包含订单明细",        // 可选
  "source_object_type_api_name": "PurchaseOrder",   // PascalCase, 必填
  "target_object_type_api_name": "OrderItem",       // PascalCase, 必填
  "foreign_key_property_api_name": "orderId",       // camelCase, 可选
  "cardinality": "MANY",                // ONE|MANY, 必填
  "direction": "OUTGOING",              // OUTGOING|INCOMING, 必填
  "weight_property": null,              // camelCase, 可选
  "temporal": false,                    // 可选默认 false
  "confidence": "confirmed"             // 可选
}
```

## 五、ActionTypeDef（可选/进阶）

> 对齐 Gaia `ActionTypeCreate`（端点 `POST /actions/definitions/{ontology}/{action_type}`）。`api_name` 同时出现在 URL path 和 request body 中——pydantic schema 的 `api_name` 是**必填字段**，body 中也必须传（route 随后以 path 值覆盖）。`affected_object_type_api_name` 是**必填** string（api_name）。

```jsonc
{
  "api_name": "createPurchaseOrder",    // camelCase, 必填（导入时放进 URL path）
  "display_name": "创建采购订单",          // 必填
  "description": "供应商创建采购订单",      // 可选默认 ""
  "affected_object_type_api_name": "PurchaseOrder",  // PascalCase, 必填
  "parameters": [ /* ActionTypeParameter[] — 可选默认 [] */ ],
  "rules": [ /* ActionRule[] — 可选默认 [] */ ],
  "submission_criteria": [              // 可选默认 [], 可执行表达式
    { "expression": "supplierId != null and len(items) > 0", "error_message": "供应商和明细不能为空", "description": "基础校验" }
  ],
  "effects": [ /* ActionEffectConfig[] — 可选默认 [] */ ],
  "ontology_rules": [ /* OntologyRule[] — 可选默认 [] */ ],
  "risk_level": "medium",               // low|medium|high, 可选默认 low
  "operation_kind": "create",           // create|update|delete|mixed, 可选默认 mixed
  "batch_enabled": false,               // 可选默认 false
  "confidence": "high"                  // 可选（SKILL 扩展字段，非 Gaia schema）
}
```

### ActionTypeParameter

```jsonc
{
  "api_name": "supplierId",             // camelCase, 必填
  "display_name": "供应商",               // 必填
  "data_type": "STRING",                // DataType 枚举, 必填
  "required": true,                     // 可选默认 true
  "default": null,                      // 可选默认 null (default_source != static 时被忽略)
  "description": "选择供应商",          // 可选默认 ""
  "default_source": "static",           // static|current_user|current_timestamp|workspace_id|selected_object_field, 可选默认 static
  "default_source_field": null,         // 可选
  "readonly": false,                    // 可选默认 false
  "hidden": false,                      // 可选默认 false
  "pattern": null,                      // 可选, 正则
  "error_message": null,               // 可选, 校验失败自定义错误信息
  "enum_values": null,                  // 可选, string[]
  "object_type_ref": "Supplier",        // PascalCase, 可选
  "is_object_set": false                // 可选默认 false
}
```

### ActionRule（派生/约束/校验规则）

```jsonc
{
  "type": "derivation",               // constraint|derivation|validation, 必填
  "target": "totalAmount",            // 目标参数名或属性名, 必填
  "expression": "unit_price * quantity",  // simpleeval 表达式, 必填
  "description": "明细金额自动派生"     // 可选默认 ""
}
```

### SubmissionCriterion（提交前置条件，可执行表达式）

```jsonc
{
  "expression": "quantity > 0 and unit_price >= 0",  // simpleeval 表达式, 必填
  "error_message": "数量必须大于 0",     // 必填, min_length=1
  "description": "明细参数校验"          // 可选默认 ""
}
```

> ⚠️ 旧版本曾写 `{name, description}`「只命名不写实现」——错误，已修正。Gaia 的 submission_criteria 必须写可执行 `expression`。

### OntologyRule（声明式变更规则）

```jsonc
// CreateObject：显式指定 target_object_type
{
  "type": "CreateObject",
  "target_object_type": "PurchaseOrder",  // PascalCase, Create 用
  "properties": {                       // {属性 api_name: ValueSource}, 默认 {}
    "orderNo": { "source": "SYSTEM_GENERATED", "value": "uuid" },
    "supplierId": { "source": "PARAMETER", "value": "supplierId" },
    "orderDate": { "source": "PARAMETER", "value": "orderDate" },
    "status": { "source": "STATIC_VALUE", "value": "DRAFT" }
  },
  "condition": null,                   // 可选, simpleeval 条件; null=无条件
  "on_missing": "raise_not_found",     // raise_not_found|create, 可选默认 raise_not_found
  "description": "创建采购订单主单"     // 可选默认 ""
}

// ModifyObject：用 target_parameter 取主键值匹配
{
  "type": "ModifyObject",
  "target_parameter": "orderNo",        // 参数 api_name, Modify/Upsert/Delete 用
  "properties": {                       // 主键不可出现在 Modify 的 properties
    "status": { "source": "STATIC_VALUE", "value": "APPROVED" }
  }
}

// CreateLink：link_type + source/target 参数
{
  "type": "CreateLink",
  "link_type": "contains",             // camelCase, CreateLink/DeleteLink 用
  "source_parameter": "orderNo",       // 参数 api_name
  "target_link_parameter": "itemRef"   // 参数 api_name
}
```

> ⚠️ 旧版本字段名 `object_type_api_name`/`match_key`/`match_value_source`/`property_values`/`link_type_api_name`/`target_object_type_api_name` 在 Gaia schema 中**不存在**，已修正为上表真实字段名。

### ValueSource

```jsonc
// 6 种 source 形态：
{ "source": "PARAMETER", "value": "supplierId" }              // 参数 api_name
{ "source": "OBJECT_PROPERTY", "value": "newOrder.status" }   // 参数名.属性名
{ "source": "STATIC_VALUE", "value": "PENDING" }              // 字面量
{ "source": "SYSTEM_CONTEXT", "value": "CURRENT_USER_ID" }    // CURRENT_USER_ID|CURRENT_TIMESTAMP
{ "source": "SYSTEM_GENERATED", "value": "uuid" }             // 生成主键
{ "source": "EXPRESSION", "value": "unit_price * quantity" }  // simpleeval 表达式
```

## 六、DatasetDef

```jsonc
{
  "api_name": "purchase_order",         // snake_case, 必填
  "display_name": "采购订单数据集",        // 可选
  "storage_location": "",               // VIRTUAL 用三段式 catalog.schema.table（首段=联邦层注册名，非引擎库名，与 backing_mapping.backing_catalog 同源对齐）; MANAGED 留空
  "partition_config": null,             // 可选
  "source_dataset_api_name": null,      // snake_case, lineage, 可选
  "data_source_api_name": null,         // snake_case, 来源数据源, 可选
  "kind": "MANAGED",                    // MANAGED|VIRTUAL, 可选默认 MANAGED
  "is_view": false,                     // 可选默认 false
  "confidence": "confirmed"             // 可选
}
```

## 七、DataSourceDef

```jsonc
{
  "api_name": "erp_master",             // snake_case, 必填
  "display_name": "ERP主数据库",          // 必填
  "description": "企业ERP系统的供应商/物料主数据",  // 可选
  "connector_type": "mysql",            // 见 schema-contract §七, 必填
  "connector_config": {                 // 可选; 敏感字段必须 *** 占位
    "host": "erp-db.example.com",
    "port": 3306,
    "database": "erp",
    "username": "readonly",
    "password": "***"
  },
  "credential_id": null,                // 可选, 产物里通常 null
  "confidence": "confirmed"             // 可选
}
```

## 八、CredentialDef（通常不产出）

```jsonc
{
  "api_name": "erp_credential",         // snake_case, 必填
  "credential_type": "username_password",  // username_password|access_key|token, 必填
  "secret_data": { "password": "***" }  // 必须占位, 禁真实值
}
```

## 九、完整示例（采购管理本体）

```json
{
  "$schema": "gaia.ontology-package/v1",
  "ontology": {
    "api_name": "Procurement",
    "display_name": "采购管理",
    "description": "采购业务本体：管理供应商、物料、采购订单及供应关系"
  },
  "object_types": [
    {
      "api_name": "Supplier",
      "display_name": "供应商",
      "description": "提供物料或服务的企业",
      "storage_type": "MANAGED",
      "capabilities": { "graph_indexing_enabled": false, "geotime_indexing_enabled": false },
      "properties": [
        { "api_name": "supplierId", "display_name": "供应商编号", "data_type": "STRING", "is_primary_key": true, "searchable": true },
        { "api_name": "name", "display_name": "名称", "data_type": "STRING", "is_title_property": true },
        { "api_name": "contactPerson", "display_name": "联系人", "data_type": "STRING" },
        { "api_name": "taxId", "display_name": "税号", "data_type": "STRING", "description": "【敏感】供应商税务登记号" }
      ],
      "confidence": "confirmed"
    },
    {
      "api_name": "Material",
      "display_name": "物料",
      "description": "被采购的物品或服务",
      "storage_type": "MANAGED",
      "properties": [
        { "api_name": "materialCode", "display_name": "物料编码", "data_type": "STRING", "is_primary_key": true },
        { "api_name": "name", "display_name": "名称", "data_type": "STRING", "is_title_property": true },
        { "api_name": "spec", "display_name": "规格", "data_type": "STRING" },
        { "api_name": "unitPrice", "display_name": "单价", "data_type": "DECIMAL", "description": "标准单价" }
      ],
      "confidence": "confirmed"
    },
    {
      "api_name": "PurchaseOrder",
      "display_name": "采购订单",
      "description": "向供应商下达的采购交易主单据",
      "storage_type": "MANAGED",
      "properties": [
        { "api_name": "orderNo", "display_name": "订单号", "data_type": "STRING", "is_primary_key": true },
        { "api_name": "supplierId", "display_name": "供应商", "data_type": "STRING", "description": "归属供应商编号" },
        { "api_name": "orderDate", "display_name": "下单时间", "data_type": "TIMESTAMP" },
        { "api_name": "status", "display_name": "状态", "data_type": "STRING", "description": "状态：DRAFT/SUBMITTED/APPROVED/CANCELLED" }
      ],
      "confidence": "confirmed"
    },
    {
      "api_name": "OrderItem",
      "display_name": "订单明细",
      "description": "采购订单中的明细行",
      "storage_type": "MANAGED",
      "properties": [
        { "api_name": "itemId", "display_name": "明细编号", "data_type": "STRING", "is_primary_key": true },
        { "api_name": "orderNo", "display_name": "订单号", "data_type": "STRING", "description": "所属订单号" },
        { "api_name": "materialCode", "display_name": "物料", "data_type": "STRING", "description": "引用物料编码" },
        { "api_name": "quantity", "display_name": "数量", "data_type": "INTEGER" },
        { "api_name": "dealPrice", "display_name": "成交价", "data_type": "DECIMAL", "description": "本明细的成交单价" }
      ],
      "confidence": "confirmed"
    },
    {
      "api_name": "SupplierMaterialRel",
      "display_name": "供应商物料关系",
      "description": "供应商与物料的多对多供应关系（M:N 拆分中间实体）",
      "storage_type": "MANAGED",
      "properties": [
        { "api_name": "relId", "display_name": "关系编号", "data_type": "STRING", "is_primary_key": true },
        { "api_name": "supplierId", "display_name": "供应商", "data_type": "STRING" },
        { "api_name": "materialCode", "display_name": "物料", "data_type": "STRING" },
        { "api_name": "supplyPrice", "display_name": "供货价", "data_type": "DECIMAL" }
      ],
      "confidence": "confirmed"
    }
  ],
  "object_type_groups": [
    {
      "api_name": "CoreEntities",
      "display_name": "核心实体",
      "description": "采购业务核心对象",
      "members": ["Supplier", "Material", "PurchaseOrder"],
      "confidence": "confirmed"
    },
    {
      "api_name": "RelationshipEntities",
      "display_name": "关系实体",
      "description": "M:N 拆分的关联实体",
      "members": ["SupplierMaterialRel"],
      "confidence": "confirmed"
    }
  ],
  "link_types": [
    {
      "api_name": "belongsTo",
      "display_name": "归属于",
      "source_object_type_api_name": "PurchaseOrder",
      "target_object_type_api_name": "Supplier",
      "foreign_key_property_api_name": "supplierId",
      "cardinality": "MANY",
      "direction": "OUTGOING",
      "confidence": "confirmed"
    },
    {
      "api_name": "contains",
      "display_name": "包含",
      "source_object_type_api_name": "PurchaseOrder",
      "target_object_type_api_name": "OrderItem",
      "foreign_key_property_api_name": "orderNo",
      "cardinality": "MANY",
      "direction": "OUTGOING",
      "confidence": "confirmed"
    },
    {
      "api_name": "references",
      "display_name": "引用",
      "source_object_type_api_name": "OrderItem",
      "target_object_type_api_name": "Material",
      "foreign_key_property_api_name": "materialCode",
      "cardinality": "MANY",
      "direction": "OUTGOING",
      "confidence": "confirmed"
    },
    {
      "api_name": "supplies",
      "display_name": "供应",
      "source_object_type_api_name": "SupplierMaterialRel",
      "target_object_type_api_name": "Supplier",
      "foreign_key_property_api_name": "supplierId",
      "cardinality": "MANY",
      "direction": "OUTGOING",
      "confidence": "confirmed"
    },
    {
      "api_name": "suppliesMaterial",
      "display_name": "供应物料",
      "source_object_type_api_name": "SupplierMaterialRel",
      "target_object_type_api_name": "Material",
      "foreign_key_property_api_name": "materialCode",
      "cardinality": "MANY",
      "direction": "OUTGOING",
      "confidence": "confirmed"
    }
  ],
  "action_types": [
    {
      "api_name": "createPurchaseOrder",
      "display_name": "创建采购订单",
      "description": "供应商创建采购订单及明细",
      "affected_object_type_api_name": "PurchaseOrder",
      "parameters": [
        { "api_name": "supplierId", "display_name": "供应商", "data_type": "STRING", "required": true, "object_type_ref": "Supplier" },
        { "api_name": "orderDate", "display_name": "下单时间", "data_type": "TIMESTAMP", "default_source": "current_timestamp" },
        { "api_name": "items", "display_name": "明细列表", "data_type": "ARRAY", "required": true }
      ],
      "ontology_rules": [
        {
          "type": "CreateObject",
          "target_object_type": "PurchaseOrder",
          "properties": {
            "orderNo": { "source": "SYSTEM_GENERATED", "value": "uuid" },
            "supplierId": { "source": "PARAMETER", "value": "supplierId" },
            "orderDate": { "source": "PARAMETER", "value": "orderDate" },
            "status": { "source": "STATIC_VALUE", "value": "DRAFT" }
          }
        }
      ],
      "submission_criteria": [
        { "expression": "supplierId != null and len(items) > 0", "error_message": "供应商和明细不能为空" }
      ],
      "risk_level": "medium",
      "operation_kind": "create",
      "confidence": "high"
    },
    {
      "api_name": "approveOrder",
      "display_name": "审批订单",
      "affected_object_type_api_name": "PurchaseOrder",
      "parameters": [
        { "api_name": "orderNo", "display_name": "订单号", "data_type": "STRING", "required": true, "object_type_ref": "PurchaseOrder" }
      ],
      "ontology_rules": [
        {
          "type": "ModifyObject",
          "target_parameter": "orderNo",
          "properties": { "status": { "source": "STATIC_VALUE", "value": "APPROVED" } }
        }
      ],
      "submission_criteria": [
        { "expression": "orderStatus == 'SUBMITTED'", "error_message": "订单须为 SUBMITTED 状态" }
      ],
      "risk_level": "medium",
      "operation_kind": "update",
      "confidence": "high"
    }
  ],
  "datasets": [
    { "api_name": "supplier", "display_name": "供应商数据集", "kind": "MANAGED", "confidence": "confirmed" },
    { "api_name": "material", "display_name": "物料数据集", "kind": "MANAGED", "confidence": "confirmed" },
    { "api_name": "purchase_order", "display_name": "采购订单数据集", "kind": "MANAGED", "confidence": "confirmed" },
    { "api_name": "order_item", "display_name": "订单明细数据集", "kind": "MANAGED", "confidence": "confirmed" },
    { "api_name": "supplier_material_rel", "display_name": "供应商物料关系数据集", "kind": "MANAGED", "confidence": "confirmed" }
  ],
  "data_sources": [],
  "credentials": [],
  "open_questions": [
    "成交价 dealPrice 是否含税？(tentative: 含税，需确认)",
    "供应商税号 taxId 是否需要脱敏存储？(tentative: 需要，需确认脱敏方式)",
    "取消订单 cancelOrder 是否需要记录取消原因？(tentative: 需要)"
  ],
  "metadata": {
    "generated_by": "ontology-modeling skill v1.0",
    "generated_at": "2026-07-26T12:00:00Z",
    "source_materials": ["采购系统业务方案.docx", "供应商主数据字段说明.xlsx"],
    "confidence_summary": { "confirmed": 20, "high": 2, "tentative": 3 }
  }
}
```

## 十、导入说明

### 10.1 现状：Gaia 当前没有一键导入端点

**重要**：Gaia 当前**没有** `POST /ontologies/import` 这样的 OntologyPackage 一键导入端点。本产物格式是面向未来导入端点的**导入契约**，但在该端点实现之前，导入需由**外部导入脚本**组合调用 Gaia 现有端点完成。下文 10.2 给出可执行的导入路径（全部为 Gaia 现有真实端点）。

### 10.2 导入路径（组合现有端点）

导入脚本按以下顺序调用 Gaia 现有端点。**关键预处理**：先 `GET /ontologies/{ont}/object-types` 拿到 `api_name → id` 映射，后续所有需要 UUID 的字段都查这个映射转换。

| 步骤 | Gaia 现有端点 | body schema | 产物字段来源 | 字段转换 |
| ------ | -------------- | ----------- | ------------ | -------- |
| 1. 创建 Ontology | `POST /ontologies` | `OntologyCreate` | `ontology` | 无（api_name 原样传） |
| 2. 创建 DataSources | `POST /api/datasources` | `DataSourceCreate` | 每个 `data_sources[]` | 无；`connector_config` 敏感字段传 `***` 占位（导入后管理员在 UI 重填真实值） |
| 3. 注册 Datasets | `POST /api/datasets` | `DatasetGovernanceCreate` | 每个 `datasets[]` | 无 |
| 4. 批量创建 ObjectTypes（含 properties + 跨对象 links） | `POST /ontologies/{ont}/object-types/create` | `ObjectTypeBatchCreate` | 每个 `object_types[]`（**注意**：`visibility`/`status`/`capabilities`/`backing_dataset_api_name` 不在 `ObjectTypeBatchCreate` 中，传入会被静默丢弃——见步骤 4b。`project_id` 为 runtime-only，产物不产出） | properties 原样；`links[]` 的 `target_object_type_api_name` → 查映射转 `target_object_type_id`；source 自动取当前 OT（不需传） |
| 4b.（按需）补设 ObjectType 元字段 | `PATCH /ontologies/{ont}/object-types/{type_name}` | `dict[str, Any]` | 产物 `object_types[]` 中 `visibility`/`status`/`capabilities` 非默认值者 | 对步骤 4 创建的每个 OT，若产物指定了非默认值，发送 PATCH `{"visibility": "PROMINENT", ...}`（仅传需覆盖的字段）；默认值可跳过 |
| 5. 创建独立 LinkTypes（source/target 都已存在的跨对象关系，未随 OT 批量建的） | `POST /ontologies/{ont}/link-types` | `LinkTypeDefCreate` | `link_types[]` 中未在步骤 4 处理的项 | `source_object_type_api_name` → `source_object_type_id`；`target_object_type_api_name` → `target_object_type_id` |
| 6. 创建 ActionTypes | `POST /actions/definitions/{ont}/{action_type}` | `ActionTypeCreate` | 每个 `action_types[]` | `api_name` 同时出现在 URL path 和 body 中（**body 中必须传**——pydantic 必填字段；route 随后以 path 值覆盖）；其余字段原样 |
| 6a. 创建 ObjectTypeGroups（ADR-022，可选） | `POST /ontologies/{ont}/object-type-groups` | `ObjectTypeGroupCreate` | 每个 `object_type_groups[]`（仅 `api_name`/`display_name`/`description`） | 无（api_name 原样传）；返回含 members 的视图 |
| 6b.（按需）绑定 Group 成员 | `POST /ontologies/{ont}/object-type-groups/{group_name}/members` | `ObjectTypeGroupMemberRequest` | 每个 `object_type_groups[].members` 非空时 | body `{"object_types": [...]}`, 幂等——已在组的 no-op，全部对象必须属同一本体 |
| 7.（可选）绑定数据集 | `PATCH /ontologies/{ont}/object-types/{type_name}/dataset-link` | `DatasetLinkRequest` | `backing_dataset_api_name` | 调 `link_dataset` 把主数据集绑上；路径参数名为 `type_name`（ObjectType 的 api_name） |

> **关于步骤 4b（补设元字段）**：`ObjectTypeBatchCreate` 不接收 `visibility`/`status`/`capabilities`/`backing_dataset_api_name`（这些字段仅在 `ObjectTypeCreate` 单创建端点中有效），pydantic 默认 `extra='ignore'` 会**静默丢弃**传入的这些字段。导入脚本需检查产物中的 ObjectType 是否有非默认值：
>
> - `visibility` 非 `NORMAL` / `status` 非 `ACTIVE` / `capabilities` 非全 `false` → 走步骤 4b PATCH
> - `backing_dataset_api_name` 非空 → 走步骤 7 `link_dataset`
> - `visibility=NORMAL` / `status=ACTIVE` / `capabilities={false,false}`（默认值）→ 可跳过步骤 4b
>
> 💡 **`project_id` 是 runtime-only 字段**（ADR-016）：离线建模产物**不产出**此字段；导入后由管理员在 Gaia UI 中分配。
>
> **关于 LinkType 的两种处理**：`link_types[]` 里某条 link 如果其 source OT 正在本包步骤 4 创建，优先随该 OT 批量建（走 `ObjectTypeBatchCreate.links`，只需 target id）；其余跨已存在 OT 的 link 走步骤 5 独立端点（需 source + target 两个 id）。导入脚本需据此分流。

### 10.3 字段映射注意

- `object_types[].properties[]` 的字段与 Gaia `PropertyInput` 一一对应：`display_name`/`api_name`/`description`/`data_type`/`searchable`/`is_primary_key`/`is_title_property`/`backing_mapping`/`vector_config`。**不传** `nullable`/`indexed`（`PropertyInput` 不接受；Gaia 自动推导：主键 nullable=false，indexed 由 searchable 推导）。
- `link_types[]` 产物用 `source_object_type_api_name`/`target_object_type_api_name`（人类可读 + 跨包稳定），但 Gaia 内部 `LinkTypeDefCreate`/`LinkInput` 用 `source_object_type_id`/`target_object_type_id`（UUID）。导入脚本负责 api_name→id 转换。
- `action_types[]` 的 `parameters[]`/`rules[]`/`submission_criteria[]`/`effects[]`/`ontology_rules[]` 分别与 Gaia `ActionTypeParameter`/`ActionRule`/`SubmissionCriterion`/`ActionEffectConfig`/`OntologyRule` 一一对应。注意 `OntologyRule` 用 `target_parameter`/`target_object_type`/`properties`/`link_type`/`source_parameter`/`target_link_parameter`（**不是** 旧文档的 `object_type_api_name`/`match_key`/`property_values`/`link_type_api_name`）。
- `action_types[].api_name` 在 `ActionTypeCreate` 中为**必填字段**——pydantic 反序列化要求 body 中也传 `api_name`（即使 route 随后会用 URL path 的值覆盖它）。导入脚本**两处都传**（值保持一致即可）。
- `action_types[].affected_object_type_api_name` 是 api_name（不是 UUID），`ActionTypeCreate` 直接接受。
- `object_types[].visibility` / `status` / `capabilities` / `backing_dataset_api_name` 不在 `ObjectTypeBatchCreate` schema 中（仅 `ObjectTypeCreate` 单创建端点接受）。导入脚本需在步骤 4 的批量请求 body 中**剥离这些字段**（否则被 pydantic 静默丢弃），再通过步骤 4b PATCH / 步骤 7 link_dataset 单独设置（见上文步骤表）。`project_id` 同理不在批量 schema 中，但属于 runtime-only 字段，产物不产出。
- `object_type_groups[]` 是纯语义分类原语（ADR-022），不涉及 DataSource/Dataset/物理存储。`ObjectTypeGroupCreate` 只接受 `api_name`/`display_name`/`description`——`members` 不在创建 body 中，导入脚本需在步骤 6a 建 Group 后，若 `members` 非空则调步骤 6b `POST .../members` 追加（幂等）。`api_name` 不可改（`ObjectTypeGroupUpdate` 只能改 `display_name`/`description`）。
- 该 endpoint 在 Ontology 完成后即可用（不依赖 ObjectType 先建）——但 `members` 涉及的 ObjectType 须在步骤 4 后才存在，因此在导入流程里把 Group 创建放在步骤 6a，成员绑定放在步骤 6b（在 ObjectType 创建之后）。
- `confidence` 是 SKILL 扩展字段（用于标注产出质量），**不是** Gaia schema 字段——导入脚本应剥离后再提交给 Gaia 端点。

### 10.4 转通用格式

OntologyPackage 是 Gaia 专有格式，但字段设计对齐 Palantir Foundry Ontology 概念（ObjectType/LinkType/ActionType/Property/Dataset）。如需转其他通用格式（如 JSON-LD、OSDK TypeScript 定义、OpenAPI），可基于本格式做适配层——核心实体语义一致，差异主要在命名约定和 Action 实现机制。

## 十一、OntologyPackage 与 OntologyPayload 映射

onto-studio 的三个 agent 工具（`export_ontology` / `preview_ontology_import` / `import_ontology`）使用 `OntologyPayload` 格式（对齐 Gaia `OntologyExportPayload`），与本 skill 产出的 `OntologyPackage` 格式有如下差异：

| 维度 | OntologyPackage（SKILL 扩展格式） | OntologyPayload（工具接口格式） |
| ------ | -------------------------------------- | ------------------------------------ |
| 本体信息 | 嵌套在 `ontology: { api_name, display_name, description }` | 扁平在顶层 `api_name` / `display_name` / `description` |
| LinkType | 顶层 `link_types[]`，含 `source_object_type_api_name` | 嵌在 `object_types[].links[]`，source 隐含为所属 OT |
| 扩展字段 | `metadata` / `open_questions` / 实体级 `confidence` | 无（工具自动忽略这些字段） |
| `$schema` | 有（`gaia.ontology-package/v1`） | 无 |

**工具调用时的转换**：agent 构造 `OntologyPayload` 时，把 `OntologyPackage.ontology` 拍平到顶层，把顶层 `link_types[]` 按 `source_object_type_api_name` 分组塞进对应 `object_types[].links[]`，丢弃 `metadata` / `open_questions` / `$schema`（`confidence` 字段会被工具自动忽略，可保留也可丢弃）。

**反向（export 后转 OntologyPackage）**：`export_ontology` 返回的 `OntologyPayload` 可逆向构造 `OntologyPackage`——顶层本体信息嵌回 `ontology`，`object_types[].links[]` 拍回顶层 `link_types[]`（补上 `source_object_type_api_name`），`metadata` / `open_questions` 按需补充。
