# 命名规范

> api_name pattern 定义与命名最佳实践。本文档是产出 OntologyPackage 时命名实体的权威参考。

## 一、api_name Pattern（业务标识符，对外可见）

三类 pattern 严格区分，**混用会导致校验失败**。

| 实体 | Pattern | 风格 | 示例 |
|------|---------|------|------|
| Ontology | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase（首字母大写） | `SupplyChain`、`FlightManagement` |
| ObjectType | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase | `PurchaseOrder`、`Supplier`、`FlightStatusLog` |
| ObjectTypeGroup | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase | `CoreEntities`、`Receivables`、`HR` |
| Property | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase（首词小写） | `orderDate`、`supplierId`、`isActive` |
| LinkType | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase | `supplies`、`contains`、`belongsTo` |
| ActionType | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase | `submitOrder`、`approveRequest` |
| Action 参数 | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase | `customerId`、`items` |
| Dataset api_name | `^[a-z][a-z0-9_]{0,99}$` | snake_case（全小写保词界） | `purchase_order`、`flight_status_log` |
| DataSource api_name | `^[a-z][a-z0-9_]{0,99}$` | snake_case | `erp_master`、`kafka_events` |
| Credential api_name | `^[a-z][a-z0-9_]{0,99}$` | snake_case | `erp_credential` |
| SyncTask api_name | `^[a-z][a-z0-9_]{0,99}$` | snake_case | `sync_purchase_order` |

**关键区分**：
- **PascalCase/camelCase**（Ontology/ObjectType/Property/LinkType/Action）是**业务标识符**，对外可见，保留词界靠大小写。
- **snake_case**（Dataset/DataSource/Credential/SyncTask）是**运维资源标识符**，兼任物理表名，全小写保词界。

> 为什么 Dataset 用 snake_case 而非 camelCase？因为 Dataset api_name 兼任物理表名，物理存储层（如 Iceberg/Trino）会把表名转小写查找——混合大小写的表名查不到。全小写 snake_case 既满足物理层要求，又保词界可读（`flight_status_log` 而非 `flightstatuslog`）。

## 二、物理资源命名（平台自动生成，产物通常不写）

以下命名由目标平台根据 api_name 自动生成，**产物里一般不需要写**（除非要预览物理资源名）。了解它们有助于理解为什么 api_name 要保词界。

| 资源 | 生成规则 | 示例 |
|------|----------|------|
| 索引表 | `idx_{ontology_snake}__{type_snake}` | `idx_supply_chain__supplier` |
| 托管表名 | == Dataset api_name（snake_case） | `purchase_order` |
| 对象存储路径 | `s3://{bucket}/{ont_snake}/{type_snake}/` | `s3://ontology-warehouse/supply_chain/supplier/` |
| 图节点标签 | `{Ontology}{ObjectType}`（PascalCase 拼接） | `SupplyChainSupplier` |
| 图关系类型 | `{Ontology}{LinkType 首字母大写}` | `SupplyChainSupplies` |

`_snake` = PascalCase/camelCase 转 snake_case（保词界）：`FlightStatusLog` → `flight_status_log`（不是 `flightstatuslog`）。

## 三、api_name 推导规则（当用户未显式提供时）

平台 service 层会按优先级推导 api_name：

1. **display_name 满足 `^[A-Za-z][A-Za-z0-9 _-]{0,99}$`**（ASCII 字母开头）→ 从 display_name 分词推导
2. **backing_column 满足同 pattern** → 从 backing_column 推导
3. **兜底** → `propertyN` / `ObjectTypeN` / `linkTypeN`（N = 已有同名兜底数）

**关键**：**中文 display_name 不满足推导源 pattern**（首字符非 ASCII 字母），会回退到 backing_column；若也没有 backing_column，则兜底成 `property0`。

**因此**：产出 OntologyPackage 时，**所有中文 display_name 的实体必须显式提供 api_name**，不能依赖推导。英文 display_name 可省略 api_name 让平台推导。

### 推导示例

| display_name | backing_column | 推导结果 |
|--------------|----------------|----------|
| `Order Date` | - | `orderDate`（Property）/ `OrderDate`（ObjectType） |
| `supplier_id` | - | `supplierId`（Property） |
| `订单日期` | `order_date` | `orderDate`（从 backing_column） |
| `订单日期` | - | `property0`（兜底，**应避免**） |
| `Supplier` | - | `Supplier`（ObjectType） |

## 四、命名最佳实践

### 4.1 ObjectType 命名
- 用**业务名词单数**：`Supplier`（供应商）、`PurchaseOrder`（采购订单）、`Employee`（员工）
- 禁缩写：`ProOrd`（❌）、`PurchaseOrder`（✅）
- 禁拼音：`GongYingShang`（❌）、`Supplier`（✅）
- 中间实体命名：`主体+客体+Rel` 或 `主体+客体+Allocation`，如 `EmployeePostRel`（员工岗位关系）

### 4.2 Property 命名
- 描述性命名，项目内一致：`supplierId`、`isActive`、`createdAt`
- 避免匈牙利前缀：`strName`（❌）、`name`（✅）
- 避免与对象名重复：`Supplier.supplierName`（❌）、`Supplier.name`（✅）

### 4.3 LinkType 命名
- 小驼峰**动词/动名词短语**：`contains`、`belongsTo`、`supplies`、`managedBy`
- 正反向语义**互逆且明确**：`Order --contains--> OrderItem` ↔ `OrderItem --belongsTo--> Order`
- **禁用模糊动词**：`has`、`rel`、`link`、`related`

### 4.4 ActionType 命名
- 动词+名词：`submitOrder`、`approveRequest`、`cancelShipment`、`createSupplier`
- 状态变更类用状态动词：`activateAccount`、`suspendService`

### 4.5 Dataset / DataSource 命名
- snake_case，反映来源/用途：`erp_supplier_master`、`kafka_order_events`、`purchase_order`（托管表）
- 托管表 Dataset api_name 建议从 ObjectType 推导：`PurchaseOrder` → `purchase_order`（保词界）

### 4.6 ObjectTypeGroup 命名
- 用**业务域/分类名词**，PascalCase：`CoreEntities`（核心实体）、`Receivables`（应收对象组）、`HR`（人力资源）
- 反映语义分组意图，不反映技术实现：`EmployeeGroup`（✅ 面向 HR 场景）而不是 `PostgresTables`（❌ 暴露实现细节）
- **无任何权限语义**，纯分类辅助浏览搜索；Group 命名**不要**暗示权限（如 `AdminOnlyGroup` ❌）

## 五、命名红线

1. ❌ PascalCase/camelCase/snake_case 混用（ObjectType 用了 camelCase、Dataset 用了 PascalCase 等）
2. ❌ 数字开头（所有 pattern 都禁止）
3. ❌ 含连字符 `-` 或点 `.`（仅物理 S3 路径用 `/`，标识符禁 `-`/`.`）
4. ❌ 中文 display_name 不给 api_name（会兜底成 `property0`）
5. ❌ 业务 api_name 泄漏进物理命名（物理命名由平台自动转换，产物不手写物理名）
6. ❌ 用 `.lower()` 全小写丢词界（`FlightStatusLog` → `flightstatuslog` ❌；应 `flight_status_log` ✅）
