# Turtle 产物格式（.ttl 文件结构 + 完整示例 + 导入说明）

> 本文档定义 W3C 语义网建模产物的文件格式契约：Turtle 文件结构、完整示例、附属文件格式、导入工具语义。

## 一、产物文件结构

一个 W3C 本体产物由多个文件组成：

| 文件 | 内容 | 必产 | 是否落库 |
|------|------|:---:|----------|
| `<本体名>.ttl` | Turtle 本体定义（7+1 语义规范的核心五类） + SPARQL 查询模板（第 7 类 CRUD，用 `sh:select` 序列化） | ✅ | ✅ 经 `import_ontology_ttl` |
| `<本体名>.open_questions.json` | AI意图/评估标准/业务操作/tentative项 | ❌（无tentative/操作时可省） | ❌ 不落库 |

> **为什么 SPARQL 查询模板写进 .ttl**：W3C SHACL 1.2 标准（`https://www.w3.org/TR/shacl-sparql/`）定义了 `sh:select` 谓词，把 SPARQL SELECT 查询作为 `xsd:string` 字面量序列化进 RDF——SPARQL 查询是 RDF 资源，与本体同文件、可被 IRI 引用、可被任何 RDF 工具消费。不再独立 `.sparql` 文件。
>
> `open_questions` 是 SKILL 扩展约定，自创谓词违反"禁自创谓词"红线，故用独立 JSON 承载，不写进 Turtle。

## 二、Turtle 主文件结构（.ttl）

### 固定骨架

```turtle
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix swrlb: <http://www.w3.org/2003/11/swrlb#> .
@prefix : <https://example.org/ontology/<本体英文名>#> .

# ── 本体元数据 ──
:<本体英文名>Ontology a owl:Ontology ;
    rdfs:label "<本体中文名>"@zh ;
    rdfs:comment "<说明> [confirmed]"@zh ;
    owl:versionInfo "<语义化版本号>" .

# ── 第1类：业务对象描述（RDF）── owl:Class / DatatypeProperty / ObjectProperty
# ── 第2类：业务分层约束（OWL）── rdfs:subClassOf / owl:Restriction / 属性特征
# ── 第3类：业务术语（SKOS）── skos:ConceptScheme / prefLabel / altLabel / exactMatch
# ── 第4类：业务控制规则（SWRL）── swrl:Imp / swrl:body / swrl:head / swrlb:
# ── 第5类：流程活动（OWL-S，进阶可选）── process:AtomicProcess / IOPE
# ── 第6类：权限管控（ODRL，进阶可选）── odrl:Policy / Permission / Prohibition
```

### 结构顺序约定

1. **前缀声明区**（最前）：`rdf`/`rdfs`/`owl`/`xsd` 必声明，主产物加 `skos`/`swrl`/`swrlb`，进阶加 `process`/`odrl`，业务 `:` 随后。
2. **本体元数据**：`owl:Ontology` + `rdfs:label`/`comment`/`versionInfo`/`imports`。
3. **第 1 类 区**：`owl:Class` 定义，按业务域分组，注释行分隔。
4. **第 2 类 区**：`rdfs:subClassOf` 层级 + `owl:Restriction` 约束 + 属性特征。
5. **第 3 类 区**：`skos:ConceptScheme` + 概念。
6. **第 4 类 区**：`swrl:Imp` 规则。
7. **第 5/6 类 区**（进阶）：OWL-S Process / ODRL Policy。
8. **DatatypeProperty/ObjectProperty 区**：所有属性（按 domain 分组）。

## 三、SPARQL 查询模板（写进 .ttl，用 `sh:select`）

SPARQL 查询模板用 W3C SHACL 1.2 标准（`https://www.w3.org/TR/shacl-sparql/`）的 `sh:select` 谓词序列化进 Turtle——每个查询是一个 RDF 资源（带 IRI），与本体定义同文件。

### 固定骨架

```turtle
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix : <https://example.org/ontology/<本体名>#> .

# 7.1 查未知本体数据
:ListInstancesByClass
    a sh:SPARQLConstraint ;   # 或自定义子类如 :SparqlQueryTemplate
    rdfs:label "按类查实例"@zh ;
    rdfs:comment "7.1 查未知本体数据：给定类名，列出所有实例"@zh ;
    sh:select """
        PREFIX : <https://example.org/ontology/<本体名>#>
        SELECT ?x WHERE { ?x a :ClassName }
    """ .

# 7.3 查本体数据缺口（完整性约束）
:CheckMissingLabels
    a sh:SPARQLConstraint ;
    rdfs:label “查缺标签的类”@zh ;
    rdfs:comment “7.3 查本体数据缺口：找出没有 rdfs:label 的类”@zh ;
    sh:select """
        SELECT ?c WHERE {
            ?c a owl:Class .
            FILTER NOT EXISTS { ?c rdfs:label ?l }
        }
    """ .

# 7.5 统计分析
:CountClassesByDomain
    a sh:SPARQLConstraint ;
    rdfs:label “按 domain 统计属性数”@zh ;
    sh:select """
        SELECT ?domain (COUNT(?p) AS ?cnt) WHERE {
            ?p rdfs:domain ?domain .
        } GROUP BY ?domain ORDER BY DESC(?cnt)
    """ .
```

### 7 类 CRUD 查询模板与 `sh:select` 的对应

| 7+1 第 7 类子项 | 用途 | `sh:select` 写法 |
|---|---|---|
| 7.1 查未知本体数据 | 给定类名查实例 | `SELECT ?x WHERE { ?x a :Class }` |
| 7.2 高效查本体数据 | 索引优化查询 | 加 `FILTER` 缩小范围 |
| 7.3 查本体数据缺口 | 完整性约束 | `FILTER NOT EXISTS { ... }` |
| 7.4 复用本体模板 | 常用查询模板 | 参数化查询（查询 IRI 可被引用） |
| 7.5 统计分析 | COUNT/SUM/AVG | `GROUP BY` + 聚合函数 |
| 7.6 查复杂本体数据 | 联邦查询 | `SERVICE <endpoint> { ... }` |
| 7.7 更新本体数据 | INSERT/DELETE | 用 SPARQL Update（不 `sh:select`，见下） |

### 7.7 SPARQL Update（INSERT/DELETE）的特殊处理

`sh:select` 只适用于 SELECT 查询。SPARQL Update（7.7 INSERT/DELETE）用 `sh:construct` 或单独的更新语句，**不序列化进 .ttl**——更新操作是运行时动作，不是本体定义的一部分。如需记录更新模板，用 `rdfs:comment` 文本说明，实际执行走 `query_ontology_sparql` 工具。

### 注意事项

- **`sh:prefixes`**：`sh:select` 的查询字符串里可用前缀缩写，但需用 `sh:prefixes` 声明（或依赖本体的 `@prefix`）。`query_ontology_sparql` 工具已自动注入常用 W3C 前缀，查询字符串内可直接用 `owl:Class` 等缩写。
- **三引号字面量**：SPARQL 查询含换行，必须用 Turtle 的 `"""..."""` 三引号字面量。
- **oxigraph 支持**：oxigraph 能 parse `sh:select` 字面量（当普通字符串存），但**不会执行** SHACL 验证——`sh:select` 在本 skill 里是「查询模板存储」，执行走 `query_ontology_sparql` 工具，不是 SHACL 引擎。

## 四、open_questions 附属文件格式

```json
{
  "ontology_iri": "https://example.org/ontology/procurement#ProcurementOntology",
  "version": "1.0.0",
  "ai_intent": {
    "business_goal": "为果农提供精准荔枝病虫害防治指导",
    "ai_tasks": ["识别病虫害", "推荐用药", "生成防治方案"],
    "success_criteria": "防治方案准确率>90%，用药安全合规"
  },
  "evaluation_criteria": {
    "decision_basis": "依据本体事理规则推理",
    "check_dimensions": ["决策符合业务规则", "输出匹配业务质量要求"],
    "output_samples": []
  },
  "related_shili": ["荔枝病虫害防治事理模型"],
  "business_operations": [
    {
      "name": "inspect",
      "description": "巡园检查",
      "confidence": "confirmed",
      "note": "流程操作，如不走OWL-S进阶则记入此处不入本体"
    }
  ],
  "tentative_items": [
    {
      "entity_iri": ":transactionPrice",
      "description": "成交价是否含税",
      "options": ["含税", "不含税", "可配置"],
      "default_assumption": "不含税",
      "note": "需与采购部门确认定价口径"
    }
  ],
  "open_questions": [
    "订单明细成交价是否含税？(tentative: 不含税)",
    "是否需要跨本体复用 schema.org 的 Organization 类？(tentative: 是)"
  ],
  "notes": [
    "业务操作不入W3C本体层（描述逻辑不表达命令式操作），建议应用层实现",
    "SWRL规则执行需推理机，oxigraph存储层暂不内置推理，复杂规则可走LLM解读执行"
  ]
}
```

### 字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|:---:|------|
| `ontology_iri` | string | ✅ | 本体完整 IRI |
| `version` | string | ✅ | 与 Turtle 的 `owl:versionInfo` 一致 |
| `ai_intent` | object | ✅ | +1 类：业务目标/AI任务/成功标准 |
| `evaluation_criteria` | object | ✅ | +1 类：决策依据/检查维度/输出样例 |
| `related_shili` | array | ✅ | +1 类：关联事理模型 |
| `business_operations` | array | ❌ | 业务操作记录（不入本体层） |
| `tentative_items` | array | ❌ | tentative 决策点 |
| `open_questions` | array | ❌ | 人工确认问题 |
| `notes` | array | ❌ | 建模决策说明 |

## 五、导入工具语义

### validate_ontology_ttl

```
validate_ontology_ttl(ttl_content: string) -> ValidationResult
```

返回 `{ errors: string[], warnings: string[] }`。

**校验项**（errors 非空则禁止 import）：
- Turtle 语法合法性
- 类必须 `a owl:Class`（非 `rdfs:Class`）
- 属性必须 `a owl:DatatypeProperty` 或 `owl:ObjectProperty`（无混用）
- `rdfs:range` 必须是合法 xsd 类型或合法类 IRI
- IRI 唯一性（同命名空间内重复定义）
- `rdfs:subClassOf` 无循环
- SWRL 规则用 `swrl:Imp`/`swrlb:` 标准词汇
- SKOS 同名异物未用 `altLabel` 合并

**警告项**（warnings 非空可 import）：
- 敏感属性未标注 `【敏感】`
- 中文标签未带 `@zh`
- 置信度标记缺失
- 滥用 `owl:cardinality`

### import_ontology_ttl

```
import_ontology_ttl(ttl_content: string, overwrite?: string[]) -> ImportResult
```

- `ttl_content`：Turtle 全文。
- `overwrite`：需覆写的 IRI 局部名列表（增量更新用）。未列入的同名实体默认 skip。
- 返回 `ImportResult`：per-entity 状态 + errors + warnings。
- **best-effort 部分失败**：单实体失败不影响其他。

## 六、增量更新语义

### 冷启动
```
validate_ontology_ttl(ttl) → import_ontology_ttl(ttl)
```

### 增量更新
```
export_ontology_ttl(iri) → 改Turtle → validate → import(ttl, overwrite=[要覆写的IRI局部名])
```

- `overwrite` 列出的实体 → 先删后建。
- 未列入的同名实体 → skip（保护已有成果）。
- 新增实体 → 直接 insert。

## 七、命名约定

| 项 | 约定 | 示例 |
|----|------|------|
| Turtle 文件名 | `<本体英文名>.ttl`（snake_case） | `procurement.ttl` |
| Turtle 文件名 | `<本体英文名>.ttl` | `procurement.ttl` |
| 附属文件名 | `<本体英文名>.open_questions.json` | `procurement.open_questions.json` |
| 命名空间 IRI | `https://<域名>/ontology/<本体英文名>#` | `https://example.org/ontology/procurement#` |
| 本体 IRI | `<命名空间><本体英文名>Ontology` | `...#ProcurementOntology` |
