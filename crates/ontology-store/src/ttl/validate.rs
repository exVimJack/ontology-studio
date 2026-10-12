//! 校验层：validate_ttl（L0/L1）+ 证据校验 + validation_gate（从 ttl/mod.rs 纯移动拆分——逻辑零改动）。

use super::*;

impl TtlStore {
    /// 校验 Turtle 内容：语法合法性 + 7+1 语义规范一致性。
    ///
    /// **errors 非空则禁止 import**：
    ///   - Turtle 语法错（解析失败）
    ///   - 类非 `a owl:Class`（用了 rdfs:Class）
    ///   - 属性混用（同一 IRI 既是 DatatypeProperty 又是 ObjectProperty）
    ///   - SWRL 规则未用 swrl:Imp 标准词汇
    ///   - IRI 命名规范违反（类 PascalCase / 属性 camelCase）
    ///   - rdfs:subClassOf 循环继承
    ///
    /// **warnings 非空可继续 import**：
    ///   - 中文标签未带 `@zh`
    pub fn validate_ttl(&self, ttl_content: &str) -> StoreResult<TtlValidation> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // ── 0. 双 # IRI 合法化（导入兼容层，语法解析前）──
        let (_, legalize_warns) = legalize_double_hash_iris(ttl_content);
        warnings.extend(legalize_warns);

        // ── 1. 语法解析（oxigraph 兜底）──
        let temp = match parse_ttl_to_memory(&ttl_content) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("Turtle 语法错误: {e}"));
                return Ok(TtlValidation {
                    triple_count: 0,
                    errors,
                    warnings,
                    issues: Vec::new(),
                    review: None,
                });
            }
        };
        let triple_count = temp.len().map(|n| n as u64).unwrap_or(0);

        // ── 2. 7+1 语义规范检查（Rust 侧真相源，SPARQL 在 temp 上跑）──
        check_class_uses_owl_class(&temp, &mut errors);
        check_property_not_mixed(&temp, &mut errors);
        check_iri_naming(&temp, &mut errors);
        check_chinese_labels_have_lang(&temp, &mut warnings);
        check_swrl_uses_standard_vocab(&temp, &mut errors);
        check_subclass_no_cycle(&temp, &mut errors);
        check_subclass_subject_is_class(&temp, &mut errors);
        check_subproperty_subject_is_property(&temp, &mut errors);
        check_subproperty_no_cycle(&temp, &mut errors);
        check_dangling_property_refs(&temp, &mut errors);

        // ── 3. L2 逻辑一致性 + L3 方法论（结构化 issues，向后兼容并入 errors/warnings）──
        let l23_issues = crate::ttl_checks_l23::run_l23_checks(&temp);
        let (l23_errors, l23_warnings, issues) =
            crate::ttl_checks_l23::split_issues(l23_issues);
        errors.extend(l23_errors);
        warnings.extend(l23_warnings);

        Ok(TtlValidation {
            triple_count,
            errors,
            warnings,
            issues,
            review: None,
        })
    }

    // ── 导入（落库）──────────────────────────────────────────────
}


impl TtlStore {
    /// 登记证据源（与 OntologyStore 同库文件，委托 evidence::register_source）。
    /// 幂等：同 ontology+kind+content_hash 返回既有 source_id。
    pub fn register_evidence_source(
        &self,
        kind: &str,
        title: &str,
        content: &str,
        ontology_api_name: &str,
        origin_ref: &str,
    ) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        crate::evidence::register_source(
            &conn,
            kind,
            title,
            content,
            ontology_api_name,
            origin_ref,
            now_ms(),
        )
    }
}


impl TtlStore {
    /// 证据链 TTL 校验（docs/EVIDENCE-CHAIN.md 5.4，W3C 路径，决策24后续）。
    ///
    /// LLM 在 TTL 中为每个建模元素（owl:Class / owl:DatatypeProperty / owl:ObjectProperty）
    /// 写证据注解：
    ///
    /// ```ttl
    /// <#OrderStatus> a owl:Class ;
    ///     prov:hadPrimarySource <urn:evidence:src-xxx> ;          # 推荐的 W3C PROV 词汇（round-trip 友好）
    ///     skos:editorialNote "evidence: src-xxx | direct | 「订单创建后处于待支付状态」" .
    /// ```
    ///
    /// 校验以 editorialNote 为准（唯一可靠载体）：格式 `evidence: {source_id} | {role} | 「quote」`，
    /// inferred 多段在「」内用 ` ⟪…⟫ ` 拼接，逐段匹配。
    ///
    /// 源表 evidence_sources 与本 store 共用同一 SQLite 文件（lib.rs 装配层同路径打开）。
    /// 返回 (errors, warnings)；strict=true 时无注解的建模元素报 S1 阻断。
    pub fn validate_ttl_evidence(
        &self,
        ttl_content: &str,
        strict: bool,
    ) -> StoreResult<(Vec<String>, Vec<String>)> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let temp = match parse_ttl_to_memory(ttl_content) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("Turtle 语法错误: {e}"));
                return Ok((errors, warnings));
            }
        };
        let (elements, notes_by_element) = ttl_evidence_extraction(&temp);
        // grandfathering：库内已存在的元素（存量本体，导入于证据链上线前）不适用 S1——
        // 降级为 warning。否则存量本体一旦被 agent 触碰就会被全量拒绝，
        // 且 overwrite=整本体替换语义下 agent 无法回导原文，直接导致数据丢失（事故 2026-09-17）。
        // 新增元素仍强制 S1；已存在元素若带了注解，E1/E2 照常校验。
        let (existing_elements, stored_defs): (
            std::collections::HashSet<String>,
            std::collections::HashMap<String, std::collections::HashSet<String>>,
        ) = {
            let iri = extract_ontology_meta(ttl_content, &mut Vec::new()).0;
            match iri {
                Some(iri) if self.exists(&iri).unwrap_or(false) => {
                    match self.export_ttl(&iri) {
                        Ok(stored) => match parse_ttl_to_memory(&stored) {
                            Ok(st) => {
                                let (els, _) = ttl_evidence_extraction(&st);
                                let defs = ttl_element_definitions(&st);
                                (els.into_iter().collect(), defs)
                            }
                            Err(_) => Default::default(),
                        },
                        Err(_) => Default::default(),
                    }
                }
                _ => Default::default(),
            }
        };
        let incoming_defs = ttl_element_definitions(&temp);
        // 3. 加载证据源（同库 evidence_sources 表；旧库无表时视为空集，注解会报 E1）
        let sources = {
            let conn = self.conn.lock().unwrap();
            crate::evidence::load_sources_map(&conn)?
        };
        // 4. 逐元素校验（grandfathered 元素不逐条告警，聚合成一条口径说明——
        //    遗留本体可达数百条，逐条告警会淹没输出并误导 agent 以为会被拦截）
        let mut grandfathered_missing: Vec<String> = Vec::new();
        for element in &elements {
            let notes = notes_by_element.get(element);
            let grandfathered = existing_elements.contains(element)
                && stored_defs.get(element) == incoming_defs.get(element);
            let definition_changed = existing_elements.contains(element)
                && stored_defs.get(element) != incoming_defs.get(element);
            let Some(notes) = notes else {
                if strict && !grandfathered {
                    errors.push(format!(
                        "元素 '{element}' 无任何证据注解（strict 模式阻断）——请为每个类/属性添加 \
                         skos:editorialNote \"evidence: {{source_id}} | {{direct|inferred}} | 「原文逐字摘录」\"；\
                         确属抽象设计时请向用户说明并请求确认"
                    ));
                } else {
                    grandfathered_missing.push(element.clone());
                }
                if definition_changed {
                    errors.push(format!(
                        "元素 '{element}' 的定义相对库内存量版本发生了修改，但未提供证据注解——\n\
                         修改后的内容必须有输入依据：补 skos:editorialNote \"evidence: {{source_id}} | {{role}} | 「原文」\"，\n\
                         或确属人工设计时用豁免注解 \"evidence: none | direct | 「用户确认理由」\""
                    ));
                    continue;
                }
                continue;
            };
            let mut all_inferred = true;
            for note in notes {
                let Some((source_id, role, quote)) =
                    crate::evidence::parse_evidence_note(note)
                else {
                    errors.push(format!(
                        "元素 '{element}' 的证据注解格式非法：\"{note}\"\n\
                         期望格式：evidence: {{source_id}} | {{direct|inferred}} | 「quote」"
                    ));
                    continue;
                };
                if role == "inferred" {
                    // 保持 all_inferred 判定语义：direct 存在则非全推断
                } else {
                    all_inferred = false;
                }
                // 豁免注解：evidence: none | direct | 「理由」——用户显式声明
                // 该元素为人工确认设计、无需源引用（保留人工自主权，gate 放行，
                // 落库 matched_by='manual'，UI 显示 ⚪ 手认）
                if source_id == "none" {
                    all_inferred = false;
                    continue;
                }
                let Some(src) = sources.get(&source_id) else {
                    errors.push(format!(
                        "元素 '{element}' 的证据注解引用了未登记的源 {source_id}（E1）——请先在会话中登记源"
                    ));
                    continue;
                };
                for seg in crate::evidence::quote_segments(&quote) {
                    if crate::evidence::match_quote(&src.content, &seg, None)
                        == crate::evidence::MatchLevel::None
                    {
                        let ctx = crate::evidence::quote_context(&src.content, &seg, 200)
                            .unwrap_or_else(|| src.content.chars().take(200).collect());
                        errors.push(format!(
                            "元素 '{element}' 的证据引用原文不存在于源「{}」，疑似臆想（E2）：\"{}\"\n\
                             源「{}」中的相关原文片段：{}",
                            src.title,
                            seg.chars().take(50).collect::<String>(),
                            src.title,
                            ctx
                        ));
                    }
                }
            }
            if all_inferred && !strict {
                warnings.push(format!(
                    "元素 '{element}' unconfirmed：证据均为组合推断（inferred），需人工确认"
                ));
            }
        }
        // grandfathered 聚合口径：一条 warning 说明存量豁免规模，替代逐条告警
        if !grandfathered_missing.is_empty() {
            warnings.push(format!(
                "grandfathered：{} 个库内存量元素（定义未变）无证据注解，自动豁免不阻断——                 本次提交的新增/定义变更元素仍需证据；建议后续为存量补录",
                grandfathered_missing.len()
            ));
        }
        Ok((errors, warnings))
    }
}


impl TtlStore {
    /// 校验闸门（VALIDATION-DESIGN.md v0.3.1，import 前调用）。
    ///
    /// 除现有「宪章先行」「证据」闸门外，检查该本体是否存在未闭环的对抗状态：
    /// - **未闭环 error issue**：`validation_issues` 中 severity=error 且
    ///   status ∈ {open, disputed}（dispute 未 overruled）→ 拒绝
    /// - **未 resolved review**：`validation_reviews` 中 status=in_progress → 拒绝
    /// - **未经校验（旁路堵漏）**：TTL 内容与最近一次 validate 不一致（改完没重跑）
    ///   → 内联重跑 validate 确认（实现选择：直接对传入 ttl 跑 validate_ttl，
    ///   有 error 即拒绝——无需签名比对存储，天然防绕过）
    ///
    /// 返回 (errors, warnings)：errors 非空则拒绝导入。空 errors 通过。
    pub fn validation_gate(&self, ttl_content: &str) -> StoreResult<(Vec<String>, Vec<String>)> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // 1. 内联重跑 validate（旁路堵漏：改完没重跑 validate 的 TTL 会被拦）
        let v = self.validate_ttl(ttl_content)?;
        for e in &v.errors {
            errors.push(e.clone());
        }
        for w in &v.warnings {
            warnings.push(w.clone());
        }
        if !errors.is_empty() {
            return Ok((errors, warnings));
        }

        // 2. 提取本体 IRI，查对抗状态表
        let (ontology_iri, _) = extract_ontology_meta(ttl_content, &mut warnings);
        let Some(iri) = ontology_iri else {
            errors.push("未找到 owl:Ontology 声明，无法确定本体 IRI".to_string());
            return Ok((errors, warnings));
        };

        let conn = self.conn.lock().unwrap();

        // 2a. 未闭环 error issue（open/disputed 且 dispute 未 overruled）
        let pending: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, status FROM validation_issues
                     WHERE ontology_iri = ?1 AND severity = 'error'
                       AND status IN ('open', 'disputed')",
                )
                .map_err(StoreError::from)?;
            let rows = stmt
                .query_map(rusqlite::params![&iri], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(StoreError::from)?;
            rows.filter_map(Result::ok).collect()
        };
        for (id, status) in &pending {
            errors.push(format!(
                "存在未闭环 error issue {id}（status={status}）——请先完成对抗/裁决再导入"
            ));
        }

        // 2b. 未 resolved review（L4.3 多轮评审进行中）
        let unresolved: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM validation_reviews
                     WHERE ontology_iri = ?1 AND status = 'in_progress'",
                )
                .map_err(StoreError::from)?;
            let rows = stmt
                .query_map(rusqlite::params![&iri], |r| r.get::<_, String>(0))
                .map_err(StoreError::from)?;
            rows.filter_map(Result::ok).collect()
        };
        for id in &unresolved {
            errors.push(format!(
                "存在未完成的语义评审 {id}（in_progress）——请先完成多轮对抗或人工裁决再导入"
            ));
        }

        Ok((errors, warnings))
    }
}


/// 解析 Turtle 文本到内存 oxigraph Store（复用）。
pub fn parse_ttl_to_memory(ttl: &str) -> StoreResult<oxigraph::store::Store> {
    use oxigraph::io::{RdfFormat, RdfParser};
    // 库内存原文（可能含非法双 # IRI）——只在喂给 oxigraph 的最后一刻临时合法化
    let (legalized, _) = legalize_double_hash_iris(ttl);
    let temp = oxigraph::store::Store::new().map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
    let parser = RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri("urn:onto-studio:validate:")
        .unwrap_or_else(|_| RdfParser::from_format(RdfFormat::Turtle));
    temp.load_from_reader(parser, legalized.as_bytes())
        .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
    Ok(temp)
}

// ════════════════════════════════════════════════════════════════
// 内部：双 # IRI 合法化（导入兼容）
// ════════════════════════════════════════════════════════════════

/// 把 Turtle 文本里 fragment 含多个 `#` 的非法 IRI 合法化（导入兼容层）。
///
/// 背景：外部导出工具（如 Palantir 风格的 `{ns}#{Class}#{property}` 扁平化写法）
/// 会产生 `<https://x.cn/ontology/O#Lead#leadId>` 这类 IRI——RFC 3986/3987 中
/// `#` 是 fragment 分隔符，fragment 内不允许再出现 `#`，oxigraph 解析层直接拒绝。
///
/// 策略：第一个 `#` 是命名空间分隔符（保留），后续 `#` 替换为 `.`（合法 fragment
/// 字符），即 `O#Lead#leadId` → `O#Lead.leadId`。类名段必须保留——同名属性挂不同类
/// （如 `Lead#province` / `Dealership#province`）去掉类名会撞 IRI。
///
/// 只重写真正的 IRI token（`<...>` 内、无空白/引号），字符串字面量和注释里的
/// `<x#y#z>` 不动。返回（合法化后文本, 每处替换的 warning）。
pub(crate) fn legalize_double_hash_iris(ttl: &str) -> (String, Vec<String>) {
    let bytes = ttl.as_bytes();
    let mut out = String::with_capacity(ttl.len() + 64);
    let mut warnings = Vec::new();
    // 状态机：Normal / LineComment / ShortString / LongString
    #[derive(PartialEq)]
    enum St {
        Normal,
        LineComment,
        ShortString,
        LongString,
    }
    let mut st = St::Normal;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match st {
            St::Normal => match b {
                b'#' => {
                    st = St::LineComment;
                    out.push('#');
                    i += 1;
                }
                b'"' if bytes[i..].starts_with(b"\"\"\"") => {
                    st = St::LongString;
                    out.push_str("\"\"\"");
                    i += 3;
                }
                b'"' => {
                    st = St::ShortString;
                    out.push('"');
                    i += 1;
                }
                b'<' => {
                    // 扫描 IRI token：到 '>' 为止，中间不允许空白/引号/控制字符
                    let mut j = i + 1;
                    while j < bytes.len() {
                        let c = bytes[j];
                        if c == b'>'
                            || c.is_ascii_whitespace()
                            || c == b'"'
                            || c == b'<'
                            || c == b'\''
                            || c == b'{'
                            || c == b'}'
                        {
                            break;
                        }
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'>' {
                        // 完整 `<...>` token，检查 fragment 是否含多个 #
                        let iri = &ttl[i + 1..j];
                        match legalize_one_iri(iri) {
                            Some(fixed) => {
                                warnings.push(format!(
                                    "IRI '<{iri}>' fragment 含多个 '#'（非法 IRI，RFC 3987），已自动合法化为 '<{fixed}>'（双 # 导入兼容）"
                                ));
                                out.push('<');
                                out.push_str(&fixed);
                                out.push('>');
                            }
                            None => out.push_str(&ttl[i..=j]),
                        }
                        i = j + 1;
                    } else {
                        // 不是完整 IRI token（如裸 `<`），原样输出
                        out.push('<');
                        i += 1;
                    }
                }
                _ => {
                    // 原样拷贝到下一个 ASCII 状态字符（多字节 UTF-8 直接过）
                    let start = i;
                    while i < bytes.len() {
                        let c = bytes[i];
                        if matches!(c, b'#' | b'"' | b'<') {
                            break;
                        }
                        i += 1;
                    }
                    out.push_str(&ttl[start..i]);
                }
            },
            St::LineComment => {
                // 拷贝完整 UTF-8 字符（注释可含中文）
                let ch = ttl[i..].chars().next().unwrap();
                out.push(ch);
                if ch == '\n' {
                    st = St::Normal;
                }
                i += ch.len_utf8();
            }
            St::ShortString => {
                if b == b'\\' && i + 1 < bytes.len() {
                    // 转义：`\\x` 两字节原样拷贝（转义后必为 ASCII）
                    out.push(b as char);
                    out.push(bytes[i + 1] as char);
                    i += 2;
                } else {
                    if b == b'"' {
                        st = St::Normal;
                    }
                    // 拷贝完整 UTF-8 字符（字符串内容可含中文）
                    let ch = ttl[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
            St::LongString => {
                if bytes[i..].starts_with(b"\"\"\"") {
                    out.push_str("\"\"\"");
                    st = St::Normal;
                    i += 3;
                } else {
                    let ch = ttl[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }
    (out, warnings)
}

/// 单个 IRI 合法化：fragment 内首个 `#` 之后的 `#` 替换为 `.`。无多个 `#` 返回 None。
pub(crate) fn legalize_one_iri(iri: &str) -> Option<String> {
    let first = iri.find('#')?;
    let rest = &iri[first + 1..];
    if !rest.contains('#') {
        return None;
    }
    Some(format!("{}{}", &iri[..first + 1], rest.replace('#', ".")))
}

/// 从 Turtle 提取本体 IRI 和版本（找 `?o a owl:Ontology` 的 `?o` + `owl:versionInfo`）。
pub fn extract_ontology_meta(
    ttl: &str,
    _warnings: &mut Vec<String>,
) -> (Option<String>, Option<String>) {
    let Ok(temp) = parse_ttl_to_memory(ttl) else {
        return (None, None);
    };
    let iri = run_sparql_solutions(
        &temp,
        "SELECT ?o WHERE { ?o <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Ontology> . } LIMIT 1",
    )
    .into_iter()
    .next()
    .and_then(|row| row.get("o").map(|v| strip_iri_brackets(&v.to_string()).to_string()));
    let version = iri.as_ref().and_then(|iri| {
        run_sparql_solutions(
            &temp,
            &format!("SELECT ?v WHERE {{ <{iri}> <http://www.w3.org/2002/07/owl#versionInfo> ?v . }} LIMIT 1"),
        )
        .into_iter()
        .next()
        .and_then(|row| row.get("v").map(|v| strip_term(&v.to_string()).to_string()))
    });
    (iri, version)
}

/// 第 2 类：类必须 `a owl:Class`（非 `rdfs:Class`）。
pub(crate) fn check_class_uses_owl_class(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    // 用全 IRI 避免 SPARQL 前缀声明问题
    let q = "SELECT ?c WHERE {\n\
        ?c <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2000/01/rdf-schema#Class> .\n\
        FILTER NOT EXISTS { ?c <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> }\n\
        FILTER(isIRI(?c))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(v) = row.get("c") {
            errors.push(format!(
                "类 '{v}' 声明为 rdfs:Class，应为 owl:Class（第 2 类约束）"
            ));
        }
    }
}

/// 第 1 类：DatatypeProperty 与 ObjectProperty 不可混用。
pub(crate) fn check_property_not_mixed(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?p WHERE {\n\
        ?p <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> .\n\
        ?p <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> .\n\
        FILTER(isIRI(?p))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(v) = row.get("p") {
            errors.push(format!(
                "属性 '{v}' 同时声明为 DatatypeProperty 和 ObjectProperty（第 1 类约束，禁混用）"
            ));
        }
    }
}

/// IRI 局部名规范：类 PascalCase，属性 camelCase。
pub(crate) fn check_iri_naming(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?s ?t WHERE {\n\
        { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> . BIND(\"Class\" AS ?t) }\n\
        UNION { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> . BIND(\"Prop\" AS ?t) }\n\
        UNION { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> . BIND(\"Prop\" AS ?t) }\n\
        FILTER(isIRI(?s))\n    }";
    for row in run_sparql_solutions(store, q) {
        let Some(s) = row.get("s") else { continue };
        let Some(t) = row.get("t") else { continue };
        let iri = strip_iri_brackets(s).to_string();
        let local = extract_local_name(&iri);
        if local.is_empty() {
            continue;
        }
        let kind = strip_term(t).to_string();
        if kind == "Class" && !is_pascal_case(&local) {
            errors.push(format!(
                "类 IRI 局部名 '{local}' 不符合 PascalCase 规范（应为如 Supplier）"
            ));
        } else if kind == "Prop" && !is_valid_property_local_name(&local) {
            errors.push(format!(
                "属性 IRI 局部名 '{local}' 不符合 camelCase 规范（应为如 supplierId；双 # 导入兼容产生的 'Class.prop' 形式也接受）"
            ));
        }
    }
}

/// 第 3 类：中文标签必须带 `@zh` 语言标签。
pub(crate) fn check_chinese_labels_have_lang(store: &oxigraph::store::Store, warnings: &mut Vec<String>) {
    // 找 rdfs:label / rdfs:comment 的中文字面量但无语言标签
    let q = "SELECT ?s WHERE {\n\
        { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l . }\n\
        UNION { ?s <http://www.w3.org/2000/01/rdf-schema#comment> ?l . }\n\
        FILTER(isLiteral(?l) && lang(?l) = \"\")\n\
        FILTER(regex(str(?l), \"[\\\\u4e00-\\\\u9fff]\"))\n\
    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(s) = row.get("s") {
            warnings.push(format!(
                "实体 '{s}' 的中文标签/说明未带 @zh 语言标签（第 3 类约束）"
            ));
        }
    }
}

/// 第 4 类：SWRL 规则必须用 swrl:Imp 标准词汇。
pub(crate) fn check_swrl_uses_standard_vocab(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?r WHERE {\n\
        { ?r <http://www.w3.org/2003/11/swrl#body> ?b . }\n\
        UNION { ?r <http://www.w3.org/2003/11/swrl#head> ?h . }\n\
        FILTER(isIRI(?r))\n\
        FILTER NOT EXISTS { ?r <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2003/11/swrl#Imp> }\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(r) = row.get("r") {
            errors.push(format!(
                "规则 '{r}' 用了 swrl:body/head 但未声明 a swrl:Imp（第 4 类约束）"
            ));
        }
    }
}

/// 第 2 类：rdfs:subClassOf 不可循环继承。
pub(crate) fn check_subclass_no_cycle(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?c WHERE {\n\
        ?c <http://www.w3.org/2000/01/rdf-schema#subClassOf>+ ?c .\n\
        FILTER(isIRI(?c))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(c) = row.get("c") {
            errors.push(format!(
                "类 '{c}' 存在 rdfs:subClassOf 循环继承（第 2 类约束）"
            ));
        }
    }
}

/// 第 2 类（补充）：rdfs:subClassOf 的主语必须是 owl:Class。
///
/// oxigraph 无推理，子类若未显式声明 `a owl:Class`，`?c a owl:Class` 查询会漏掉它，
/// 破坏分层语义。该校验防止 29 句话模板里的 `:X rdfs:subClassOf :Y` 缺声明。
pub(crate) fn check_subclass_subject_is_class(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?sub WHERE {\n\
        ?sub <http://www.w3.org/2000/01/rdf-schema#subClassOf> ?sup .\n\
        FILTER(isIRI(?sub))\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2000/01/rdf-schema#Class> }\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(sub) = row.get("sub") {
            let iri = strip_iri_brackets(sub);
            errors.push(format!(
                "子类 '{iri}' 用了 rdfs:subClassOf 但未声明 a owl:Class（第 2 类约束，子类必须显式声明为类——oxigraph 无推理，未声明会从类查询中遗漏）"
            ));
        }
    }
}

/// 第 1 类（补充）：rdfs:subPropertyOf 的主语必须是 rdf:Property 或其子类。
///
/// 同理 subClassOf——subPropertyOf 的主语必须是属性，否则分层语义断裂。
pub(crate) fn check_subproperty_subject_is_property(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?sub WHERE {\n\
        ?sub <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> ?sup .\n\
        FILTER(isIRI(?sub))\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> }\n\
        FILTER NOT EXISTS { ?sub <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#AnnotationProperty> }\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(sub) = row.get("sub") {
            let iri = strip_iri_brackets(sub);
            errors.push(format!(
                "子属性 '{iri}' 用了 rdfs:subPropertyOf 但未声明为 rdf:Property/owl:DatatypeProperty/owl:ObjectProperty（第 1 类约束）"
            ));
        }
    }
}

/// 第 1 类（补充）：rdfs:subPropertyOf 禁循环继承。
///
/// 与 subClassOf 的 cycle 检查对称——属性继承成环同样是语义损坏，此前漏检。
pub(crate) fn check_subproperty_no_cycle(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    let q = "SELECT ?p WHERE {\n\
        ?p <http://www.w3.org/2000/01/rdf-schema#subPropertyOf>+ ?p .\n\
        FILTER(isIRI(?p))\n    }";
    for row in run_sparql_solutions(store, q) {
        if let Some(p) = row.get("p") {
            errors.push(format!(
                "属性 '{p}' 存在 rdfs:subPropertyOf 循环继承（第 1 类约束）"
            ));
        }
    }
}

/// 属性编辑安全网（决策：TTL 手动编辑 P0）：悬空属性引用检测。
///
/// 删属性/改 IRI 后，以下引用可能悬空（validate 此前不查，静默落库）：
///   1. `owl:onProperty` 指向的属性必须已声明（owl:Restriction 引用属性）
///   2. `rdfs:subPropertyOf` 的 object（父属性）必须是已声明属性
pub(crate) fn check_dangling_property_refs(store: &oxigraph::store::Store, errors: &mut Vec<String>) {
    // 已声明属性集：rdf:type ∈ {owl:DatatypeProperty, owl:ObjectProperty, rdf:Property, owl:AnnotationProperty}
    let declared = r#"PREFIX owl: <http://www.w3.org/2002/07/owl#>
        PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        SELECT ?p WHERE {
            { ?p a owl:DatatypeProperty } UNION { ?p a owl:ObjectProperty }
            UNION { ?p a rdf:Property } UNION { ?p a owl:AnnotationProperty }
            FILTER(isIRI(?p))
        }"#;
    let declared: std::collections::HashSet<String> = run_sparql_solutions(store, declared)
        .into_iter()
        .filter_map(|row| row.get("p").cloned())
        .collect();
    // owl:onProperty 悬空
    let on_prop = r#"PREFIX owl: <http://www.w3.org/2002/07/owl#>
        SELECT ?r ?p WHERE {
            ?r owl:onProperty ?p .
            FILTER(isIRI(?p))
        }"#;
    for row in run_sparql_solutions(store, on_prop) {
        if let Some(p) = row.get("p") {
            if !declared.contains(p) {
                errors.push(format!(
                    "owl:onProperty 引用的属性 '{p}' 未声明（悬空引用——删除属性前先解除引用）"
                ));
            }
        }
    }
    // subPropertyOf 父属性悬空
    let sub_prop = r#"PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT ?sub ?sup WHERE {
            ?sub rdfs:subPropertyOf ?sup .
            FILTER(isIRI(?sub)) FILTER(isIRI(?sup))
        }"#;
    for row in run_sparql_solutions(store, sub_prop) {
        if let Some(sup) = row.get("sup") {
            if !declared.contains(sup) {
                errors.push(format!(
                    "rdfs:subPropertyOf 的父属性 '{sup}' 未声明（悬空引用）"
                ));
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════
// 内部辅助：SPARQL 执行 + IRI 处理
// ════════════════════════════════════════════════════════════════

/// 从 IRI 字符串提取局部名（# 后或 / 后最后一段）。
pub(crate) fn extract_local_name(iri: &str) -> String {
    let s = strip_iri_brackets(iri);
    s.rsplit(['#', '/']).next().unwrap_or("").to_string()
}

/// TTL 证据提取（建模元素全集 + 证据注解），validate 与 import 持久化共用。
/// 元素定义指纹：element IRI → 其全部 (predicate, object) 串集合。
/// 用于 grandfathering 的「定义是否变化」判定——IRI 存在但定义变了 = 新主张，需证据。
/// 证据注解本身（skos:editorialNote）不参与指纹（有无注解由 notes_by_element 判断）。
pub(crate) fn ttl_element_definitions(
    temp: &oxigraph::store::Store,
) -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    let mut defs: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for quad in temp.iter() {
        let Ok(quad) = quad else { continue };
        let s = quad.subject.to_string();
        let p = quad.predicate.to_string();
        let o = quad.object.to_string();
        if p == "http://www.w3.org/2004/02/skos/core#editorialNote" {
            continue;
        }
        defs.entry(s).or_default().insert(format!("{p} {o}"));
    }
    defs
}

pub(crate) fn ttl_evidence_extraction(
    temp: &oxigraph::store::Store,
) -> (Vec<String>, std::collections::HashMap<String, Vec<String>>) {
    // 1. 建模元素全集（类 + 两类属性）
    let elements_q = "SELECT DISTINCT ?e WHERE {\n\
        { ?e <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> }\n\
        UNION { ?e <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#DatatypeProperty> }\n\
        UNION { ?e <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#ObjectProperty> }\n\
        FILTER(isIRI(?e))\n        }";
    let elements: Vec<String> = run_sparql_solutions(temp, elements_q)
        .into_iter()
        .filter_map(|mut row| row.remove("e"))
        .collect();
    // 2. 证据注解（editorialNote 以 "evidence:" 开头）
    let notes_q = "SELECT ?e ?note WHERE {\n\
        ?e <http://www.w3.org/2004/02/skos/core#editorialNote> ?note .\n\
        FILTER(isIRI(?e) && STRSTARTS(STR(?note), \"evidence:\"))\n        }";
    let mut notes_by_element: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for mut row in run_sparql_solutions(temp, notes_q) {
        if let (Some(e), Some(note)) = (row.remove("e"), row.remove("note")) {
            notes_by_element.entry(e).or_default().push(note);
        }
    }
    (elements, notes_by_element)
}

/// 统计 TTL 文本 triple 数（append_ttl 落库用）。
pub(crate) fn merged_store_count(ttl: &str) -> u64 {
    parse_ttl_to_memory(ttl)
        .ok()
        .and_then(|t| t.len().ok())
        .unwrap_or(0) as u64
}

/// PascalCase：首字母大写，仅字母数字。
pub(crate) fn is_pascal_case(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// camelCase：首字母小写，仅字母数字。
pub(crate) fn is_camel_case(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// 属性局部名：标准 camelCase，或双 # 导入兼容产生的 `PascalCase.camelCase`
/// （类名段.属性名段，两段分别校验）。
pub(crate) fn is_valid_property_local_name(s: &str) -> bool {
    if !s.contains('.') {
        return is_camel_case(s);
    }
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 2 && is_pascal_case(parts[0]) && is_camel_case(parts[1])
}
