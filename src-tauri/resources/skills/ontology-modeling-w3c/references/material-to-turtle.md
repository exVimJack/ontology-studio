# 从业务材料到 29 句话的方法论

> 把 Word/PDF/PPT/Excel/TXT 文档、业务描述、结构化数据源 schema 抽象成符合「29 句话」范式的标准化描述集，再翻译成 W3C Turtle 的实战方法论。对齐《本体驱动的AI数据管理》第 6.1 节预处理流程。

## 一、预处理六步法总览

```
① 显性知识AI提炼 → ② 隐性知识专家补充 → ③ 审核准入（四标准）→
④ 协同评审 → ⑤ 29句话产出（带置信度）→ ⑥ tentative进open_questions
```

## 二、按材料类型分流的抽取策略

### 2.1 非结构化文档（Word / PDF / TXT）

**特征**：自然语言段落，业务方案/需求文档/操作手册/规章制度。

**抽取策略**：
1. **名词扫描** → `owl:Class` 候选 + PascalCase IRI（第 1 类）。业务对象的数据属性（1.1）和关系（1.2）。
2. **动词扫描** → 业务操作（创建/审批/取消）→ **记入 `open_questions.business_operations`**，不入本体层（红线）。如需流程语义走 OWL-S 进阶层（第 5 类）。
3. **规则扫描**："当...时执行..."、"如果...那么..." → SWRL 规则候选（第 4 类 4.1）。
4. **状态/枚举扫描** → `owl:oneOf` 枚举类 或 `xsd:string` + comment 列举值（第 2.5 类）。
5. **术语扫描**：同词不同义、同物异名 → SKOS 概念拆分/合并（第 3 类）。

### 2.2 半结构化文档（Excel / CSV）

**特征**：数据字典、字段说明表、清单。

**抽取策略**：
1. **sheet/区块标题** → `owl:Class` 候选。列 → `owl:DatatypeProperty`（第 1.1 类）。
2. **列名→`rdfs:label`（@zh），列说明→`rdfs:comment`，数据类型→XSD**（第 2.5 类值合规）：
   - "金额/价格"列 → `xsd:decimal`
   - "时间/日期"列 → `xsd:dateTime`/`xsd:date`
   - "是/否"列 → `xsd:boolean`
   - "状态"列 → `owl:oneOf` 枚举类 或 `xsd:string` + comment
3. **主键列**：不设主键约束（W3C 无主键概念），作普通 `owl:DatatypeProperty`。如需唯一性用 `owl:InverseFunctionalProperty`（第 2.4 类实例标识）。
4. **关联列**（引用另一张表）→ `owl:ObjectProperty`（第 1.2 类），而非字面量属性。

### 2.3 演示文档（PPT）

**特征**：业务架构图、流程图、列表要点。

**抽取策略**：
1. **架构图节点** → `owl:Class` 候选；**连线** → `owl:ObjectProperty` 候选（第 1 类）。
2. **流程图步骤** → OWL-S `process:AtomicProcess`（进阶第 5 类），或记入 `open_questions`。
3. **列表要点** → 类的 `owl:DatatypeProperty` 或枚举值。
4. PPT 信息密度低，常需配合 Word/Excel 补全属性细节。

### 2.4 业务描述（自然语言段落/口述）

**抽取策略**：同 2.1，但更依赖语义理解：
1. 解析"主谓宾"：主语/宾语 → `owl:Class`，谓语 → `owl:ObjectProperty`（第 1 类）。
2. 解析"每个X有Y个Z"：基数——**默认不声明**（开放世界），仅材料明确要求"必须/至多"时用 `owl:minCardinality` 等（第 2.5 类）。
3. 解析"当...时执行..."：SWRL 规则（第 4 类 4.1）。
4. 解析"敏感/机密/隐私"：`rdfs:comment` 标注 `【敏感】`（第 2.6 类语义注释）。

### 2.5 结构化数据源 schema（DDL / 表结构 / API 报文 / JSON Schema）

**抽取策略**：
1. **物理表 → `owl:Class` 候选**（业务化重命名：`t_sup_master` → `:Supplier`）。
2. **物理列 → `owl:DatatypeProperty`**，XSD 类型映射见下表（第 1.1 类）。
3. **物理主键 → 普通属性**，如需唯一性用 `owl:InverseFunctionalProperty`（第 2.4 类）。
4. **物理外键 → `owl:ObjectProperty`**（第 1.2 类）。
5. **视图/中间表 → 不建类**（除非有独立业务身份）。

### SQL → XSD 映射

| SQL 类型 | XSD 类型 | 备注 |
|----------|---------|------|
| `VARCHAR`/`CHAR`/`TEXT`/`CLOB` | `xsd:string` | |
| `INT`/`INTEGER` | `xsd:integer` | |
| `BIGINT` | `xsd:long` | 64-bit |
| `SMALLINT` | `xsd:short` | 16-bit |
| `TINYINT` | `xsd:byte` | 8-bit |
| `DECIMAL`/`NUMERIC` | `xsd:decimal` | 金额必须 |
| `FLOAT` | `xsd:float` | 32-bit |
| `DOUBLE`/`REAL` | `xsd:double` | 金额禁，改 decimal |
| `BOOLEAN`/`BIT` | `xsd:boolean` | |
| `DATE` | `xsd:date` | |
| `TIMESTAMP`/`DATETIME` | `xsd:dateTime` | |
| `BYTEA`/`BLOB` | `xsd:base64Binary` | |
| `JSON`/`JSONB` | 建结构类 | 不用 `xsd:string` 存 |
| `UUID` | `xsd:string` | |

## 三、显性知识 AI 提炼（第 1 步）

AI 对多源显性知识深度解构与关联分析，把 7+1 语义规范核心要素与各类知识载体精准匹配，自动抽取标准化业务语义描述。

**输入**：结构化数据 + 半结构化文档 + 非结构化资料。
**输出**：7 类 × 29 句话的初步描述（带置信度）。
**原则**：构建范围由 AI 业务意图目标决定，遵循"够用且可扩展"原则。

## 四、隐性知识专家补充（第 2 步）

大量关键业务知识（专家经验、决策逻辑、上下文认知）未在文档沉淀。业务专家依据 29 句话范式，从三维度萃取：

### 维度一：说清"有哪些对象"（对应第 1、2、3 类）
- 按 SKOS 统一业务领域术语，避免同词不同义（第 3 类）
- 按 RDF 定义核心资源对象的属性及相互关系（第 1 类）
- 按 OWL 划分"公共层-领域层-实例层"抽象层级（第 2.1 类）

### 维度二：说清"任务怎么做"（对应第 5、7 类）
- 按 OWL-S 复用已定义的对象语义，拆解业务的原子动作，建立动作与对象的关联（第 5 类）
- 按 SPARQL 明确本体数据操作逻辑，让 AI 可自主挖掘线索、生成查询、验证本体（第 7 类）

### 维度三：说清"有哪些规则"（对应第 4、2、6 类）
- 按 SWRL 将隐含的业务控制规则显性化，"如果...那么..."描述（第 4 类）
- 按 OWL 界定各层级语义边界与关联逻辑，类继承/定义域/值域约束（第 2 类）
- 按 ODRL 明确权限范围，界定角色对资源的可操作/不可操作/必须执行（第 6 类）

## 五、审核准入（第 3 步，四标准）

显性+隐性知识完成对齐后，须审核确保满足 AI 业务本体建模要求。

### 建模准入标准（四标准）

| 标准 | 对应 | 解决问题 |
|------|------|---------|
| **描述清晰** | 对应语义规范 | 术语全局唯一，对齐 29 句话模板，解决描述模糊/语义偏差 |
| **逻辑正确** | 对应逻辑合规 | 对象/规则/动作分类清晰，严格遵循本体分层逻辑，验证业务规则无闭环冲突 |
| **内容完备** | 对应内容达标 | 串联"AI意图→事实-事理-行动"完整链条，解决内容缺失/支撑不足 |
| **极简表达** | 对应Agent意图满足 | 业务上下文精简精准，避免冗余，满足AI Agent意图与推理需求即可 |

### 协同评审路径（第 4 步）

- **初审**（建模工程师驱动）：依据 Checklist 对业务描述做语义规范性检查，确保术语一致且符合 7+1 规范。
- **复核**（业务专家驱动）：组织业务专家召开复核会议，针对逻辑断点或语义偏差深度对齐，验证业务规则闭环与内容完备性。

## 六、29 句话产出（第 5 步）

每条 29 句话产出格式：
- **句号**（如 2.1 给对象分层）
- **标准描述**（"给对象分层"）
- **白话解释**（"对象归哪类"）
- **业务实例**（如"荔枝蝽属于半翅目害虫"）
- **W3C 映射**（`rdfs:subClassOf`）
- **置信度**（confirmed/high/tentative）

完整 29 句清单见 [29-sentences.md](29-sentences.md)。

## 七、tentative 项处理（第 6 步）

- `tentative` 项**不写入 Turtle 主文件的语义部分**，但可在 `rdfs:comment` 文本末尾保留标注。
- 所有 tentative 项汇总进附属 `open_questions.json` 的 `tentative_items` 字段。
- 业务操作（创建/审批/取消等）汇总进 `business_operations` 字段，**不入本体层**。

## 八、完整示例：荔枝病虫害防治场景

**材料片段**：
> 荔枝树有品种、树龄、挂果量等属性。荔枝蝽属于半翅目害虫，霜疫霉病属于真菌性病害。白糖罂和蜂糖罂指同一品种。气温连续三天超 25℃ 且湿度 > 80% 触发霜疫霉病预防预警。防治流程分巡园、识别、配药、施药四步。普通果农仅能查看防治方案，认证植保员方可修改用药清单。需要查询当前季节易发病害。

**29 句话抽取**（按 7 类）：

| 类 | 句号 | 描述 | 置信度 | Turtle 映射 |
|----|------|------|--------|-------------|
| 1 | 1.1 | 定义数据属性：品种、树龄、挂果量 | confirmed | `:hasVariety`/`:hasAge`/`:hasFruitLoad` DatatypeProperty |
| 1 | 1.2 | 定义对象关系：病害发生于果园 | confirmed | `:occursIn` ObjectProperty |
| 2 | 2.1 | 给对象分层：荔枝蝽 subClassOf 半翅目害虫 | confirmed | `rdfs:subClassOf` |
| 2 | 2.1 | 霜疫霉病 subClassOf 真菌性病害 | confirmed | `rdfs:subClassOf` |
| 2 | 2.5 | 浓度合规 500-1000 | confirmed | `owl:Restriction` + 值域 |
| 2 | 2.6 | 加语义注释 | confirmed | `rdfs:comment` |
| 3 | 3.1 | 同物异名：白糖罂=蜂糖罂 | confirmed | `skos:prefLabel`+`altLabel` |
| 3 | 3.2 | 分类逻辑：病虫害分类体系 | confirmed | `skos:ConceptScheme`+`broader` |
| 4 | 4.1 | 业务触发规则：气温>25℃且湿度>80%→预警 | confirmed | `swrl:Imp` + `swrlb:greaterThan` |
| 5 | 5.1-5.3 | 拆原子操作+输入输出+前提效果（巡园/识别/配药/施药） | high | `process:AtomicProcess`（进阶） |
| 6 | 6.1-6.4 | 权限管控：果农查看，植保员修改 | confirmed | `odrl:Permission`/`Prohibition`（进阶） |
| 7 | 7.1 | 查询当前季节易发病害 | confirmed | SPARQL SELECT 模板 |
| +1 | - | AI意图：为果农提供精准防治指导 | confirmed | `open_questions.ai_intent` |

**业务操作**（不入本体层）：巡园/识别/配药/施药 → `open_questions.business_operations`（如不走 OWL-S 进阶）。

## 九、抽取检查清单

- [ ] 第 1 类（1.1 数据属性 / 1.2 对象关系）是否覆盖所有业务对象？
- [ ] 第 2 类（2.1-2.7 分层/推导/绑定/实例/合规/注释/管理）是否完整？
- [ ] 第 3 类（3.1-3.3 术语/分类/对外口径）是否处理同名异物与同物异名？
- [ ] 第 4 类（4.1 触发规则）是否覆盖所有关键决策点？
- [ ] 第 5 类（5.1-5.5，进阶）是否拆原子操作+IOPE+流程组合？
- [ ] 第 6 类（6.1-6.4，进阶）是否覆盖管控对象/相关方/规则/操作？
- [ ] 第 7 类（7.1-7.7）SPARQL 模板是否覆盖核心业务查询？
- [ ] +1 类（AI意图/评估标准/关联事理）是否明确？
- [ ] 业务操作是否记入 `open_questions`（不入本体）？
- [ ] 每条是否标注置信度？tentative 项是否进 `open_questions`？
- [ ] 四标准（描述清晰/逻辑正确/内容完备/极简表达）是否满足？
