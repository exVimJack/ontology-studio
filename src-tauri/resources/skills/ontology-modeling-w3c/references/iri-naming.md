# IRI 命名规范

> IRI（Internationalized Resource Identifier）是 W3C 语义网的标识符基础。本文档定义产出 Turtle 时命名实体的权威规则——IRI 命名空间设计、局部名 pattern、前缀声明、语言标签。

## 一、前缀声明（prefix）

Turtle 文件开头必须声明前缀。

### 标准前缀（必声明）

```turtle
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
```

### 主产物前缀（核心五类必声明）

```turtle
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .      # 第 3 类术语
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .           # 第 4 类规则
@prefix swrlb: <http://www.w3.org/2003/11/swrlb#> .         # 第 4 类内置谓词
```

### 进阶前缀（进阶可选时声明）

```turtle
@prefix process: <http://www.daml.org/services/owl-s/1.2/Process.owl#> .  # 第 5 类流程
@prefix odrl: <http://www.w3.org/ns/odrl/2/> .              # 第 6 类权限
```

### 业务前缀（默认 `:`）

```turtle
@prefix : <https://example.org/ontology/procurement#> .
```

### 扩展前缀（按需）

| 前缀 | 命名空间 | 用途 |
|------|----------|------|
| `dc:` | `http://purl.org/dc/elements/1.1/` | Dublin Core 元数据 |
| `dcterms:` | `http://purl.org/dc/terms/>` | Dublin Core Terms |
| `prov:` | `http://www.w3.org/ns/prov#` | PROV 溯源 |
| `schema:` | `https://schema.org/>` | Schema.org |

## 二、命名空间设计

| 场景 | 推荐命名空间 | 示例 |
|------|-------------|------|
| 对外发布 Linked Data | `https://<域名>/ontology/<本体名>#` | `https://example.com/ontology/procurement#` |
| 内部项目本体 | `https://<项目标识>/ontology/<本体名>#` | `https://onto-studio.local/ontology/procurement#` |
| 实验性本地本体 | `urn:onto-studio:<本体名>:` | `urn:onto-studio:procurement:` |

**规则**：
- 命名空间以 `#` 结尾（片段式，局部名追加在 `#` 后）或 `/` 结尾（斜杠式）。**同一本体统一一种**，推荐 `#`。
- 命名空间要**稳定且可解析**——未来发布时 HTTP GET 该 IRI 应返回 RDF 描述。
- **禁止** `http://example.org/` 用于正式产物（W3C 保留测试域名，工具会忽略）；仅示例文档用。

## 三、局部名 Pattern

| 实体 | Pattern | 风格 | 示例 |
|------|---------|------|------|
| `owl:Class` | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase | `Supplier`、`PurchaseOrder` |
| `owl:DatatypeProperty` | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase | `supplierId`、`orderDate` |
| `owl:ObjectProperty` | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase | `supplies`、`contains` |
| `owl:Ontology` | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase | `ProcurementOntology` |
| `skos:Concept` | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase | `Baitangying`、`HemipteraPest` |
| `swrl:Imp` 规则 | `^[a-z][a-zA-Z0-9]{0,99}$` | camelCase | `rule1`、`downyMildewWarning` |
| 枚举值实例 | `^[A-Z][a-zA-Z0-9]{0,99}$` | PascalCase | `DRAFT`、`APPROVED` |

**关键区分**：类/枚举值/SKOS 概念用 PascalCase；属性/规则用 camelCase。禁用下划线/连字符/点。

## 四、局部名推导

材料给中文 display_name，翻译英文再推导 IRI 局部名：

| 中文 | 英文翻译 | IRI 局部名 | 类型 |
|------|---------|-----------|------|
| 供应商 | Supplier | `:Supplier` | 类 |
| 采购订单 | PurchaseOrder | `:PurchaseOrder` | 类 |
| 供应商编号 | supplierId | `:supplierId` | DatatypeProperty |
| 下单时间 | orderDate | `:orderDate` | DatatypeProperty |
| 包含 | contains | `:contains` | ObjectProperty |
| 白糖罂 | Baitangying | `:Baitangying` | skos:Concept |

### 推导红线

- ❌ 中文直接作局部名（`:供应商`）——破坏跨工具兼容性，必须翻译英文
- ❌ 缩写（`:ProOrd`）——禁缩写，用完整词
- ❌ 拼音（`:GongYingShang`）——禁拼音
- ❌ 数字开头（`:1stOrder`）——Turtle PN_LOCAL 禁止

## 五、语言标签

```turtle
:Supplier rdfs:label "供应商"@zh ;
          rdfs:label "Supplier"@en ;
          rdfs:comment "采购供应商"@zh .
```

| 场景 | 写法 |
|------|------|
| 中文标签 | `"中文"@zh` |
| 英文标签 | `"English"@en` |
| 无语言标签 | `"raw string"` |

**本项目约定**：
- 中文 display_name/说明**必须**带 `@zh`——裸中文字符串无法区分语言
- 英文标签推荐带 `@en`
- 同一实体可有多语言标签

### 禁止

- ❌ 裸中文字符串（`"供应商"` 不带 `@zh`）
- ❌ 非标准语言标签（`@cn`/`@chinese`/`@zh-CN`）——标准是 `@zh`，如需地区用 `@zh-Hans`/`@zh-Hant`

## 六、命名最佳实践

### 类命名
- 业务名词单数：`:Supplier`（供应商）、`:PurchaseOrder`（采购订单）
- 禁缩写：`:PurchaseOrder`（✅）、`:ProOrd`（❌）
- 禁拼音：`:Supplier`（✅）、`:GongYingShang`（❌）
- 关联类：`主体+客体+Relation`（`:EmployeePostRelation`）

### 属性命名
- 描述性命名：`:supplierId`、`:isActive`、`:createdAt`
- 避免匈牙利前缀：`:name`（✅）、`:strName`（❌）
- 避免与对象名重复：`:Supplier.name`（✅）、`:Supplier.supplierName`（❌）

### ObjectProperty 命名
- 小驼峰动词短语：`:contains`、`:belongsTo`、`:supplies`
- 正反向语义互逆：`:Order :contains :OrderItem` ↔ `:OrderItem :belongsTo :Order`
- **禁用模糊动词**：`:has`、`:rel`、`:link`、`:related`

## 七、命名红线

1. ❌ PascalCase/camelCase 混用
2. ❌ 数字开头的局部名
3. ❌ 含下划线/连字符/点
4. ❌ 中文/拼音直接作局部名
5. ❌ 裸中文字符串无 `@zh`
6. ❌ 非标准语言标签
7. ❌ `http://example.org/` 用于正式产物
8. ❌ 命名空间 `/` 和 `#` 混用
9. ❌ 同一命名空间内 IRI 重复定义
