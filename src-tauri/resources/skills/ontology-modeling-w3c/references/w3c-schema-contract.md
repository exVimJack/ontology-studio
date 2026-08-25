# W3C Schema 契约（7+1 语义规范词汇表）

> 本文档是 Turtle 产物的**谓词级真相源**。覆盖 7 类语义规范对应的全部 W3C 词汇表（RDF/RDFS/OWL/SKOS/SWRL/OWL-S/ODRL/SPARQL）+ XSD 数据类型映射 + Turtle 语法契约。产出 Turtle 时逐谓词核对，**不得自创谓词、不得误用语义**。

## 一、XSD 数据类型映射（第 1 类数据属性 + 第 2 类值合规）

### 业务场景 → XSD 类型

| 业务场景 | 正确 XSD 类型 | 禁止 | 备注 |
|----------|-------------|------|------|
| 金额/单价/费用 | `xsd:decimal` | `xsd:double`/`xsd:float` | 精确十进制，禁浮点 |
| 业务时间（含时分秒） | `xsd:dateTime` | `xsd:string` | `YYYY-MM-DDThh:mm:ss` |
| 时间戳（带时区） | `xsd:dateTimeStamp` | `xsd:string` | 强制时区 |
| 日期（无时间） | `xsd:date` | `xsd:string` | `YYYY-MM-DD` |
| 时间（无日期） | `xsd:time` | `xsd:string` | `hh:mm:ss` |
| 布尔状态 | `xsd:boolean` | `0/1`/`xsd:string` | `true`/`false` |
| 业务编号/标识 | `xsd:string` | 自增数值 | 语义网无主键概念 |
| 数量/序号（<2³¹） | `xsd:integer` | `xsd:string` | 32-bit |
| 大整数（>2³¹） | `xsd:long` | `xsd:integer` 别名 | 64-bit |
| 整数（16-bit） | `xsd:short` | - | ±32767 |
| 整数（8-bit） | `xsd:byte` | - | ±127 |
| 浮点（科学计算） | `xsd:double` | `xsd:decimal`（金额禁） | IEEE 754 64-bit |
| 单精度浮点 | `xsd:float` | `xsd:decimal`（金额禁） | IEEE 754 32-bit |
| URI/URL | `xsd:anyURI` | `xsd:string` | |
| 二进制数据 | `xsd:base64Binary` | `xsd:string` | base64 |
| 结构化 JSON | 建结构类 | `xsd:string` 存 JSON 原文 | 保语义网结构化 |

### 值域约束（第 2.5 句）

```turtle
# 浓度范围 500-1000
:Pesticide rdfs:subClassOf [
    a owl:Restriction ;
    owl:onProperty :concentration ;
    owl:allValuesFrom [
        a rdfs:Datatype ;
        owl:onDatatype xsd:integer ;
        owl:withRestrictions (
            [ xsd:minInclusive 500 ]
            [ xsd:maxInclusive 1000 ]
        )
    ]
] .
```

## 二、OWL 2 词汇表（第 1、2 类）

### 2.1 类与属性（第 1 类 + 2.1/2.3）

| 谓词 | 语义 | 用法示例 |
|------|------|---------|
| `rdf:type`（简写 `a`） | 实例是某类成员 | `:Supplier a owl:Class` |
| `owl:Class` | 类声明 | `:Supplier a owl:Class` |
| `owl:DatatypeProperty` | 数据属性（值域字面量） | `:supplierId a owl:DatatypeProperty` |
| `owl:ObjectProperty` | 对象属性（值域类实例） | `:supplies a owl:ObjectProperty` |
| `rdfs:domain` | 主体类 | `:supplierId rdfs:domain :Supplier` |
| `rdfs:range` | 客体类/类型 | `:supplierId rdfs:range xsd:string` |
| `rdfs:subClassOf` | 子类（2.1 分层） | `:Aircraft rdfs:subClassOf :Vehicle` |
| `rdfs:subPropertyOf` | 子属性 | `:hasHomeAddr rdfs:subPropertyOf :hasAddr` |

### 2.2 属性逻辑特征（第 2.2 句）

| 谓词 | 语义 | 用法 |
|------|------|------|
| `owl:FunctionalProperty` | 函数属性（多对一唯一性） | 特定防治期每病害仅一最优药 |
| `owl:InverseFunctionalProperty` | 反函数（值唯一标识主体，2.4 实例标识） | 农药登记证号唯一对应农药 |
| `owl:TransitiveProperty` | 传递（层级链完整性） | 病害子类传递 |
| `owl:SymmetricProperty` | 对称 | 双向等价关系 |
| `owl:AsymmetricProperty` | 反对称 | (进阶) |
| `owl:IrreflexiveProperty` | 反自反 | (进阶) |
| `owl:inverseOf` | 逆属性（双向闭环） | "防治"与"被防治"互逆 |
| `owl:equivalentProperty` | 等价属性 | 跨本体映射 |

> **`rdfs:domain`/`rdfs:range` 是推理规则不是约束**——开放世界下，声明 domain 意味着"任何有此属性的主体会被推断为该类实例"，不是"只有该类能用此属性"。

### 2.3 基数与约束（第 2.5 句，可选/进阶）

| 谓词 | 语义 | 用法 |
|------|------|------|
| `owl:Restriction` | 约束（空白节点） | `[ a owl:Restriction ; ... ]` |
| `owl:onProperty` | 约束的属性 | `owl:onProperty :contains` |
| `owl:minCardinality` | 最小基数 | `"1"^^xsd:nonNegativeInteger` |
| `owl:maxCardinality` | 最大基数 | `"1"^^xsd:nonNegativeInteger` |
| `owl:cardinality` | 精确基数 | `"1"^^xsd:nonNegativeInteger` |
| `owl:allValuesFrom` | 全部值来自 | 值域约束 |
| `owl:someValuesFrom` | 至少一值来自 | 存在量词约束 |
| `owl:hasValue` | 必须有特定值 | 固定值约束 |

> **基数使用原则**：默认不声明（开放世界默认无约束）。仅当材料明确要求"必须/至多/恰好"且需推理验证时才用。`owl:Restriction` 是**唯一允许空白节点**的场景。

### 2.4 跨本体复用（第 2.7 句 语义管理）

| 谓词 | 语义 | 用法 |
|------|------|------|
| `owl:Ontology` | 本体本身 | `:MyOnt a owl:Ontology` |
| `owl:versionInfo` | 版本 | `owl:versionInfo "1.0.0"` |
| `owl:imports` | 引入外部本体 | `owl:imports <http://...other.ttl>` |
| `owl:equivalentClass` | 等价类 | `:Supplier owl:equivalentClass schema:Vendor` |
| `owl:disjointWith` | 不相交 | `:Cat owl:disjointWith :Dog` |
| `owl:oneOf` | 枚举 | `:Status owl:oneOf (:DRAFT :APPROVED)` |
| `rdfs:label` | 标签（带 @zh） | `rdfs:label "供应商"@zh` |
| `rdfs:comment` | 说明（含敏感标注+置信度） | `rdfs:comment "【敏感】税号 [confirmed]"@zh` |
| `rdfs:seeAlso` | 相关资源 | `rdfs:seeAlso <https://wiki...>` |
| `rdfs:isDefinedBy` | 定义来源 | `rdfs:isDefinedBy :MyOnt` |

## 三、SKOS 词汇表（第 3 类 业务术语）

| 谓词 | 语义 | 第几句 | 用法 |
|------|------|--------|------|
| `skos:Concept` | 概念 | - | `:Baitangying a skos:Concept` |
| `skos:ConceptScheme` | 概念体系 | 3.2 | `:PestCategory a skos:ConceptScheme` |
| `skos:prefLabel` | 首选标签 | 3.1 | `skos:prefLabel "白糖罂"@zh` |
| `skos:altLabel` | 替代标签（同物异名，3.1） | 3.1 | `skos:altLabel "蜂糖罂"@zh` |
| `skos:hiddenLabel` | 隐藏标签（搜索用） | 3.1 | 拼音/错别字 |
| `skos:definition` | 定义 | 3.1 | `skos:definition "...详细说明..."@zh` |
| `skos:broader` | 宽义（上位概念，3.2） | 3.2 | `:HemipteraPest skos:broader :Pest` |
| `skos:narrower` | 窄义（下位概念，3.2） | 3.2 | `:Pest skos:narrower :HemipteraPest` |
| `skos:inScheme` | 归属体系 | 3.2 | `:HemipteraPest skos:inScheme :PestCategory` |
| `skos:exactMatch` | 精确等价（跨域，3.3） | 3.3 | `:A skos:exactMatch :B` |
| `skos:closeMatch` | 近似等价（跨域，3.3） | 3.3 | `:A skos:closeMatch :B` |
| `skos:related` | 相关（非等价，3.3） | 3.3 | `:Typhoon skos:related :DisasterWeather` |

### 关键区分

- **同物异名**（白糖罂=蜂糖罂）→ 一个 `skos:Concept` + `prefLabel` + `altLabel`（3.1）
- **同名异物**（荔枝霜霉病 ≠ 葡萄霜霉病）→ **两个独立 `skos:Concept`**，各自 `prefLabel`，用 `skos:related` 或不关联（3.1 红线）
- **跨本体对齐** → `skos:exactMatch`（完全等价）或 `skos:closeMatch`（近似）（3.3）

## 四、SWRL 词汇表（第 4 类 业务控制规则）

### 4.1 核心结构

SWRL 规则用 `swrl:Imp`（规则）+ `swrl:body`（前件，IF）+ `swrl:head`（后件，THEN）。前件/后件是 `swrl:AtomList`（原子列表）。

| 谓词 | 语义 | 用法 |
|------|------|------|
| `swrl:Imp` | 规则 | `:rule1 a swrl:Imp` |
| `swrl:body` | 前件（IF） | `swrl:body ( atom1 atom2 ... )` |
| `swrl:head` | 后件（THEN） | `swrl:head ( atom )` |
| `swrl:Variable` | 变量 | `[ a swrl:Variable ; swrl:argumentName "x"^^xsd:string ]` |

### 4.2 swrlb 内置谓词（Built-ins）

| 谓词 | 语义 | 示例 |
|------|------|------|
| `swrlb:equal` | 等于 | `swrlb:equal(?x, ?y)` |
| `swrlb:notEqual` | 不等 | |
| `swrlb:lessThan` | 小于 | |
| `swrlb:lessThanOrEqual` | 小于等于 | |
| `swrlb:greaterThan` | 大于 | `swrlb:greaterThan(?temp, 25)` |
| `swrlb:greaterThanOrEqual` | 大于等于 | |
| `swrlb:add`/`subtract`/`multiply`/`divide` | 算术 | |
| `swrlb:stringEqualIgnoreCase` | 字符串忽略大小写相等 | |
| `swrlb:contains` | 字符串包含 | |
| `swrlb:matches` | 正则匹配 | |

> 完整 swrlb 谓词清单见 W3C SWRL Submission（https://www.w3.org/Submission/SWRL/ 8.3 节）。

### 4.3 完整 SWRL 规则示例

规则：气温连续三天超 25℃ 且湿度 > 80% → 触发霜疫霉病预防预警

```turtle
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix swrlb: <http://www.w3.org/2003/11/swrlb#> .

:rule1 a swrl:Imp ;
    rdfs:label "霜疫霉病预防预警规则"@zh ;
    rdfs:comment "气温>25℃且湿度>80%触发预警 [confirmed]"@zh ;
    swrl:body (
        # ?l 是荔枝个体，?t 是温度，?h 是湿度
        [ a swrl:Variable ; swrl:argumentName "l"^^xsd:string ]
        [ a swrl:Variable ; swrl:argumentName "t"^^xsd:string ]
        [ a swrl:Variable ; swrl:argumentName "h"^^xsd:string ]
        # 原子：?l 有温度 ?t
        [ rdf:type swrl:AtomList ;
          rdf:first [ a swrl:IndividualPropertyAtom ;
                      swrl:propertyPredicate :hasTemperature ;
                      swrl:argument1 "l"^^xsd:string ;
                      swrl:argument2 "t"^^xsd:string ] ;
          rdf:rest rdf:nil ]
    ) .
```

> SWRL 的 RDF 序列化语法较繁琐（AtomList 结构），实际产出时大模型 A 可用更紧凑的等价写法（如 Turtle 的 collection 语法），关键是 `swrl:Imp`/`swrl:body`/`swrl:head`/`swrl:Variable`/`swrlb:` 谓词正确。验证时以能被标准 SWRL 解析器（如 Protégé）接受为准。

## 五、OWL-S 词汇表（第 5 类 流程操作）【进阶可选】

> 仅当材料明确要求流程自动化语义时才产出。前缀 `process:` = `http://www.daml.org/services/owl-s/1.2/Process.owl#`。

| 谓词 | 语义 | 第几句 |
|------|------|--------|
| `process:AtomicProcess` | 原子操作 | 5.1 |
| `process:CompositeProcess` | 复合操作 | 5.5 |
| `process:hasInput` | 输入 | 5.2 |
| `process:hasOutput` | 输出 | 5.2 |
| `process:hasPrecondition` | 前提 | 5.3 |
| `process:hasResult` | 效果（结果） | 5.3 |
| `process:Sequence` | 顺序组合 | 5.4 |
| `process:ControlConstruct` | 控制结构 | 5.4 |
| `process:components` | 组件列表 | 5.4 |

```turtle
@prefix process: <http://www.daml.org/services/owl-s/1.2/Process.owl#> .

:InspectProcess a process:AtomicProcess ;            # 5.1 原子操作
    rdfs:label "巡园检查"@zh ;
    process:hasInput :inputOrchard ;                 # 5.2 输入
    process:hasOutput :outputPestStatus ;            # 5.2 输出
    process:hasPrecondition :orchardExists ;         # 5.3 前提
    process:hasResult [ process:hasEffect :pestIdentified ] .  # 5.3 效果
```

## 六、ODRL 词汇表（第 6 类 权限管控）【进阶可选】

> 仅当材料明确要求权限管控语义时才产出。前缀 `odrl:` = `http://www.w3.org/ns/odrl/2/`。

| 谓词 | 语义 | 第几句 |
|------|------|--------|
| `odrl:Policy` | 策略（基类） | - |
| `odrl:Permission` | 权限（允许） | 6.3 |
| `odrl:Prohibition` | 禁止 | 6.3 |
| `odrl:Duty` | 义务（必须） | 6.3 |
| `odrl:target` | 管控对象（资产，6.1） | 6.1 |
| `odrl:assigner` | 权限授予方（6.2） | 6.2 |
| `odrl:assignee` | 权限接收方（6.2） | 6.2 |
| `odrl:action` | 操作动作（6.4） | 6.4 |

### 常用 action 值

`odrl:read` / `odrl:write` / `odrl:execute` / `odrl:modify` / `odrl:delete` / `odrl:share` / `odrl:sell`

```turtle
@prefix odrl: <http://www.w3.org/ns/odrl/2/> .

:editMedicinePolicy a odrl:Policy ;
    rdfs:label "用药清单编辑权限"@zh ;
    odrl:permission [
        odrl:target :MedicineList ;                  # 6.1 管控对象
        odrl:assigner :PlantProtectionStation ;       # 6.2 授予方
        odrl:assignee :CertifiedProtector ;          # 6.2 接收方
        odrl:action odrl:modify                       # 6.4 操作
    ] ;
    odrl:prohibition [
        odrl:target :MedicineList ;
        odrl:assignee :OrdinaryFarmer ;
        odrl:action odrl:modify                       # 6.4 禁止
    ] .
```

## 七、字段大小写速查

| 实体 | IRI 局部名 pattern | 示例 |
|------|-------------------|------|
| 本体（`owl:Ontology`） | PascalCase | `:ProcurementOntology` |
| 类（`owl:Class`） | PascalCase `^[A-Z]...` | `:PurchaseOrder` |
| DatatypeProperty | camelCase `^[a-z]...` | `:orderDate` |
| ObjectProperty | camelCase `^[a-z]...` | `:supplies` |
| 枚举值实例 | PascalCase `^[A-Z]...` | `:DRAFT`、`:APPROVED` |
| SWRL 规则 | camelCase `^[a-z]...` | `:rule1`、`:downyMildewWarning` |
| SKOS 概念 | PascalCase | `:Baitangying` |
| 标准谓词 | 固定小写 | `rdfs:label`、`owl:Class` |

## 八、禁止谓词清单（红线）

1. ❌ `rdfs:Class` 替代 `owl:Class`
2. ❌ 自创谓词表达标准语义（如 `:myType` 替代 `rdf:type`）
3. ❌ `owl:DatatypeProperty` 与 `owl:ObjectProperty` 混用
4. ❌ 用空白节点表达类/属性定义（仅 `owl:Restriction` 内部允许）
5. ❌ 数据源绑定信息写入本体
6. ❌ 平台私有字段（如 `:primaryKey` 自创字段替代 `owl:InverseFunctionalProperty`）
7. ❌ 同名异物用 `skos:altLabel` 合并（应拆为两个 `skos:Concept`）
8. ❌ SWRL 规则不用 `swrl:Imp`/`swrlb:` 标准词汇
