# 29 句话清单（7+1 语义规范 × 标准化描述模板）

> 本文档是本体建模方法论的核心骨架——把《本体驱动的AI数据管理》的「7+1 语义规范」细化为 29 句标准化自然语言描述模板。每句对应一个 W3C 标准和一个 Turtle 产出形态。业务专家用这 29 句话讲清楚业务，AI 翻译成 W3C 本体。

## 7 类 × 29 句总表

### 第 1 类：业务对象描述语义（RDF）—— 系统中存在什么实体

回答"业务存在什么实体、它们有什么特征、如何关联"。对应 W3C **RDF** 标准（主-谓-宾三元组，事实关联网络）。

| 句号 | 标准描述 | 白话解释 | Turtle 产出 |
|------|---------|---------|------------|
| 1.1 | 定义数据属性 | 对象有什么特征 | `owl:DatatypeProperty` + `rdfs:range xsd:<type>` |
| 1.2 | 定义对象关系 | 对象间怎么关联 | `owl:ObjectProperty` + `rdfs:range :<Class>` |

**Turtle 示例**：
```turtle
:hasVariety a owl:DatatypeProperty ;          # 1.1 数据属性
    rdfs:domain :Litchi ;
    rdfs:range xsd:string ;
    rdfs:label "品种"@zh .
:hasAge a owl:DatatypeProperty ;
    rdfs:domain :Litchi ;
    rdfs:range xsd:integer ;
    rdfs:label "树龄"@zh .
:occursIn a owl:ObjectProperty ;              # 1.2 对象关系
    rdfs:domain :Pest ;
    rdfs:range :Orchard ;
    rdfs:label "发生于"@zh .
```

### 第 2 类：业务分层约束语义（OWL）—— 对象如何归属和约束

回答"对象归哪类、属性如何推导组合、属性只能给谁用、值怎么填才对"。对应 W3C **OWL** 标准（类层级、属性约束、唯一性规则）。

| 句号 | 标准描述 | 白话解释 | Turtle 产出 |
|------|---------|---------|------------|
| 2.1 | 给对象分层 | 对象归哪类 | `rdfs:subClassOf` 类层级 |
| 2.2 | 定属性关系规则 | 属性间怎么推导组合 | `rdfs:subPropertyOf` / `owl:TransitiveProperty` / `owl:SymmetricProperty` |
| 2.3 | 定属性类绑定规则 | 属性只能给谁用 | `rdfs:domain` 约束主体类 |
| 2.4 | 管理对象实例 | 个体怎么标识 | `owl:NamedIndividual` / `owl:InverseFunctionalProperty`（唯一标识） |
| 2.5 | 定数据属性值合规规则 | 属性怎么填才对 | `owl:Restriction` + `owl:allValuesFrom` / `owl:cardinality` / XSD 值域 |
| 2.6 | 加语义注释 | 语义背景和来源 | `rdfs:comment` / `rdfs:isDefinedBy` / `prov:wasDerivedFrom` |
| 2.7 | 语义管理 | 遵循的规范和公理 | `owl:Ontology` + `owl:imports` / `owl:equivalentClass`（跨本体对齐） |

**Turtle 示例**：
```turtle
:LitchiPlanthopper a owl:Class ;
    rdfs:subClassOf :HemipteraPest .   # 2.1 分层（子类必须显式声明 a owl:Class）
:ancestorOf a owl:TransitiveProperty .                # 2.2 传递属性推导
:hasFruitLoad rdfs:domain :Litchi .                   # 2.3 属性绑定类
:pesticideRegNo a owl:InverseFunctionalProperty ;      # 2.4 唯一标识
    rdfs:range xsd:string .
:Pesticide a owl:Class ;
    rdfs:subClassOf [                          # 2.5 值合规规则
    a owl:Restriction ;
    owl:onProperty :concentration ;
    owl:allValuesFrom [
        a rdfs:Datatype ;
        owl:onDatatype xsd:integer ;
        owl:withRestrictions ( [ xsd:minInclusive 500] [ xsd:maxInclusive 1000 ] )
    ] ] .
:Litchi rdfs:comment "荔枝属无患子科 [confirmed]"@zh ;  # 2.6 语义注释
    rdfs:isDefinedBy :ProcurementOntology .
:ProcurementOntology a owl:Ontology ;                  # 2.7 语义管理
    owl:imports <http://example.org/biology.ttl> .
```

### 第 3 类：业务术语（SKOS）—— 统一语言，建立共识

回答"同名异物怎么办、同物异名怎么办、跨域怎么对齐"。对应 W3C **SKOS** 标准（概念体系、首选/替代标签、映射）。

| 句号 | 标准描述 | 白话解释 | Turtle 产出 |
|------|---------|---------|------------|
| 3.1 | 统一内部术语 | 避免同物异名 | `skos:prefLabel`（首选）+ `skos:altLabel`（替代） |
| 3.2 | 统一分类逻辑 | 避免理解偏差 | `skos:ConceptScheme` + `skos:broader`/`skos:narrower` 层级 |
| 3.3 | 统一对外口径 | 避免跨域混淆 | `skos:exactMatch`/`skos:closeMatch`/`skos:related` 跨本体映射 |

**Turtle 示例**：
```turtle
:Baitangying a skos:Concept ;                          # 3.1 同物异名
    skos:prefLabel "白糖罂"@zh ;
    skos:altLabel "蜂糖罂"@zh .
:PestCategory a skos:ConceptScheme ;                   # 3.2 分类逻辑
    rdfs:label "病虫害分类"@zh .
:HemipteraPest skos:broader :Pest ;                    # 宽义层级
    skos:inScheme :PestCategory .
:LitchiDownyMildew skos:exactMatch :PeronophythoraLitchi .  # 3.3 跨域对齐
```

> **关键**：同名异物（如"霜霉病"在荔枝和葡萄指不同病害）必须拆为两个 `skos:Concept`，**不能用 `skos:altLabel` 合并**（那是同物异名才用）。

### 第 4 类：业务控制规则语义（SWRL）—— 什么条件触发什么动作

回答"什么条件做什么事"。对应 W3C **SWRL** 标准（如果...那么...的 Horn 规则）。

| 句号 | 标准描述 | 白话解释 | Turtle 产出 |
|------|---------|---------|------------|
| 4.1 | 定业务触发规则 | 什么条件做什么事 | `swrl:Imp`（body 前件 → head 后件）+ `swrl:Variable` + `swrlb:` 内置谓词 |

**Turtle 示例**（荔枝霜疫霉病预警规则）：
```turtle
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix swrlb: <http://www.w3.org/2003/11/swrlb> .

:rule1 a swrl:Imp ;
    rdfs:label "霜疫霉病预防预警"@zh ;
    rdfs:comment "气温连续三天超25℃且湿度>80%触发预警 [confirmed]"@zh ;
    swrl:body (
        [ a swrl:Variable ; swrl:argument :t ]
        [ a swrl:AtomList ; rdf:first [:temperature :t] ; rdf:rest rdf:nil ]
    ) .
# 实际写法用 AtomList 结构，更完整示例见 w3c-schema-contract.md §4
```

> SWRL 规则的完整 Turtle 语法（`swrl:body`/`swrl:head` 的 AtomList 结构、`swrlb:greaterThan` 等内置谓词清单）见 [w3c-schema-contract.md §4](w3c-schema-contract.md)。

### 第 5 类：流程活动和业务操作语义（OWL-S + BPMN 2.0）—— 业务怎么做【进阶可选】

回答"最小步骤是什么、步骤输出什么、什么时候做什么有什么变化、步骤怎么串成流程"。对应 W3C **OWL-S** 标准（Process + IOPE：Input/Output/Precondition/Effect）+ BPMN 2.0。

| 句号 | 标准描述 | 白话解释 | Turtle 产出 |
|------|---------|---------|------------|
| 5.1 | 拆原子操作 | 最小步骤是什么 | `process:AtomicProcess` |
| 5.2 | 明确操作输入输出 | 步骤要输出什么 | `process:hasInput` / `process:hasOutput` |
| 5.3 | 明确操作前提效果 | 什么时候做有什么变化 | `process:hasPrecondition` / `process:hasResult` |
| 5.4 | 组合简单服务 | 步骤怎么串成流程 | `process:Sequence` / `process:ControlConstruct` |
| 5.5 | 组合复杂服务 | 复杂步骤怎么组合 | `process:CompositeProcess` |

> **进阶可选**——仅当材料明确要求流程自动化语义时才产出。OWL-S 词汇用 `process:` 前缀（`http://www.daml.org/services/owl-s/1.2/Process.owl#`）。见 [w3c-schema-contract.md §5](w3c-schema-contract.md)。

### 第 6 类：权限管控语义（ODRL）—— 谁能做什么【进阶可选】

回答"管控什么、谁给谁拿权限、权限对应什么动作、能做什么不能做什么"。对应 W3C **ODRL** 标准（策略/权限/禁止/义务）。

| 句号 | 标准描述 | 白话解释 | Turtle 产出 |
|------|---------|---------|------------|
| 6.1 | 明确管控对象 | 管控什么 | `odrl:target`（资产） |
| 6.2 | 明确权限相关方 | 谁给谁拿权限 | `odrl:assigner` / `odrl:assignee` |
| 6.3 | 明确权限规则 | 权限对应什么动作 | `odrl:Permission` / `odrl:Prohibition` / `odrl:Duty` |
| 6.4 | 明确权限操作 | 能做什么不能做什么 | `odrl:action`（如 `odrl:read`/`odrl:write`/`odrl:execute`） |

> **进阶可选**——仅当材料明确要求权限管控语义时才产出。见 [w3c-schema-contract.md §6](w3c-schema-contract.md)。

### 第 7 类：CRUD 数据操作语义（SPARQL）—— 数据全生命周期操作

回答"信息怎么查、怎么查更快、数据漏了什么补、常用查询怎么存、本体怎么统计、跨数据源联合查询、数据怎么改怎么删"。对应 W3C **SPARQL** 标准（查询/更新/联邦查询）。

| 句号 | 标准描述 | 白话解释 | 产出形态 |
|------|---------|---------|---------|
| 7.1 | 查未知本体数据 | 信息怎么查 | SPARQL SELECT 查询模板 |
| 7.2 | 高效查本体数据 | 怎么查更快 | SPARQL 查询模板 + 索引建议（注释） |
| 7.3 | 查本体数据缺口 | 数据漏了什么补 | SPARQL ASK / SELECT 查完整性约束 |
| 7.4 | 复用本体模板 | 常用查询怎么存 | SPARQL 查询模板库（附属 .sparql 文件） |
| 7.5 | 统计分析本体数据 | 本体怎么统计 | SPARQL AGGREGATE（COUNT/SUM/AVG/GROUP BY） |
| 7.6 | 查复杂本体数据 | 跨数据源联合查询 | SPARQL Federated Query（SERVICE 子句） |
| 7.7 | 更新本体数据 | 数据怎么改怎么删 | SPARQL UPDATE（INSERT/DELETE） |

> SPARQL 模板作为附属 `.sparql` 文件交付，**不进 Turtle 主文件**（SPARQL 不是 RDF 序列化）。见 [ttl-package-format.md §3](ttl-package-format.md)。

**SPARQL 模板示例**：
```sparql
# 7.1 查未知本体数据：查询当前季节易发病害
SELECT ?disease WHERE {
    ?disease rdf:type :Disease ;
             :occursInSeason ?season .
    FILTER(?season = :Summer)
}

# 7.6 查复杂本体数据：跨数据源联合查询（植保+气象）
SELECT ?orchard ?disease ?temp WHERE {
    ?orchard :hasDisease ?disease .
    SERVICE <http://weather.example.org/sparql> {
        ?orchard :hasTemperature ?temp .
        FILTER(?temp > 25)
    }
}

# 7.7 更新本体数据：新增病害记录
INSERT {
    :orchard1 :hasDisease :DownyMildew .
} WHERE {}
```

### +1 类：目标与评估（非 W3C，AI 对齐层）—— 不进 Turtle

**核心**：确保本体与 AI 业务意图对齐，避免"模型与 AI 脱节"。

| 子项 | 内容 | 产出形态 |
|------|------|---------|
| 业务目标定义 | AI 需完成的任务及业务效果（AI 业务意图） | 附属 `open_questions.json` 的 `ai_intent` 字段 |
| 评估质控标准 | 决策判定依据、结果检查维度、输出样例 | 附属 `open_questions.json` 的 `evaluation_criteria` 字段 |
| 关联事理模型 | 让 AI 明确"做什么、做到什么标准、依据什么做" | 附属 `open_questions.json` 的 `related_shili` 字段 |

> +1 类是 SKILL 扩展约定，**不写入 Turtle**——用附属 JSON 文件承载，保持 Turtle 纯净（标准 RDF 工具可消费，不被 AI 对齐元信息污染）。

## 29 句话抽取检查清单

抽取 29 句话时，逐类核对：

- [ ] 第 1 类：是否定义了所有业务对象的数据属性（1.1）和对象关系（1.2）？
- [ ] 第 2 类：是否给对象分层（2.1）？属性推导规则（2.2）？属性绑定类（2.3）？实例标识（2.4）？值合规（2.5）？语义注释（2.6）？语义管理（2.7）？
- [ ] 第 3 类：内部术语统一（3.1）？分类逻辑（3.2）？对外口径（3.3）？
- [ ] 第 4 类：业务触发规则（4.1）是否覆盖所有关键决策点？
- [ ] 第 5 类（进阶）：原子操作（5.1）？输入输出（5.2）？前提效果（5.3）？流程组合（5.4-5.5）？
- [ ] 第 6 类（进阶）：管控对象（6.1）？相关方（6.2）？权限规则（6.3）？权限操作（6.4）？
- [ ] 第 7 类：查（7.1-7.6）？更新（7.7）的 SPARQL 模板是否覆盖核心业务查询？
- [ ] +1 类：AI 业务意图、评估标准、关联事理模型是否明确？
- [ ] 每条是否标注置信度（confirmed/high/tentative）？
- [ ] tentative 项是否进附属 `open_questions`？
