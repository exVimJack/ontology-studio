//! 数据层：import/export/属性手术编辑/SPARQL 查询/changelog（从 ttl/mod.rs 纯移动拆分——逻辑零改动）。

use super::*;

impl TtlStore {
    /// 导入 Turtle 到存储。best-effort：校验失败仍尝试落库（但建议先 validate）。
    ///
    /// **调用前建议先 validate_ttl 确认无 errors**。
    /// `overwrite=true` 时 UPSERT（同 IRI 覆盖）；`overwrite=false` 时已存在则 skip。
    pub fn import_ttl(&self, ttl_content: &str, overwrite: bool) -> StoreResult<TtlImportResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // 0. 双 # IRI 合法化（导入兼容层）：仅产生溯源 warning；解析路径内部临时合法化，**落库存原文**（导出无损复原）
        let (_, legalize_warns) = legalize_double_hash_iris(ttl_content);
        warnings.extend(legalize_warns);

        // 1+2. 单次解析提取本体 IRI + 版本 + 三元组数（避免 extract_ontology_meta
        //      与 parse_ttl_to_memory 各解析一次；解析失败时返回错误结果）
        let (ontology_iri, version, triple_count) = extract_meta_and_count(ttl_content);
        let ontology_iri = match ontology_iri {
            Some(iri) => iri,
            None => {
                return Ok(TtlImportResult {
                    ontology_iri: String::new(),
                    triple_count: 0,
                    errors: vec!["未找到 owl:Ontology 声明，无法确定本体 IRI".to_string()],
                    warnings,
                });
            }
        };
        if triple_count == 0 && !ttl_content.trim().is_empty() {
            // 解析失败（extract_meta_and_count 返回 0 但无法区分空 TTL 与解析失败）。
            // 此处保持旧行为：显式 parse 一次确认失败则报错。
            if let Err(e) = parse_ttl_to_memory(ttl_content) {
                return Ok(TtlImportResult {
                    ontology_iri,
                    triple_count: 0,
                    errors: vec![format!("Turtle 解析失败: {e}")],
                    warnings,
                });
            }
        }

        // 3. 落库（SQLite UPSERT）
        let conn = self.conn.lock().unwrap();
        if !overwrite {
            // 检查是否已存在
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM ontology_ttl WHERE ontology_iri = ?",
                    rusqlite::params![&ontology_iri],
                    |_| Ok(()),
                )
                .is_ok();
            if exists {
                return Ok(TtlImportResult {
                    ontology_iri,
                    triple_count,
                    errors,
                    warnings: {
                        let mut w = warnings;
                        w.push("本体已存在且 overwrite=false，已跳过".to_string());
                        w
                    },
                });
            }
        }
        match conn.execute(
            "INSERT INTO ontology_ttl (ontology_iri, ttl_content, version, triple_count, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(ontology_iri) DO UPDATE SET
                ttl_content = excluded.ttl_content,
                version = excluded.version,
                triple_count = excluded.triple_count,
                updated_at = excluded.updated_at",
            rusqlite::params![
                &ontology_iri,
                ttl_content,
                version.as_deref().unwrap_or(""),
                triple_count as i64,
                now_ms(),
            ],
        ) {
            Ok(_) => {
                // ── 元素级证据落库（docs/EVIDENCE-CHAIN.md L4，standard='w3c'）──
                // 从 TTL 原文注解解析后写入 element_evidence（TTL 原文也存注解，双保险；
                // 整本体替换语义：先清该本体旧证据行）。
                // ⚠️ 必须用原文 tokenize 版（ttl_edit::evidence_extraction_from_text）
                // 而非 oxigraph 版：oxigraph 序列化带尖括号 + 双 # 被合法化成点号，
                // element_id 与前端 list_properties 的原文形态对不上，证据行永远匹配不到。
                let (elements, notes_by_element) =
                    crate::ttl_edit::evidence_extraction_from_text(ttl_content);
                if let Err(e) = crate::evidence::delete_element_evidence(
                    &conn, "w3c", &ontology_iri,
                ) {
                    warnings.push(format!("清理旧证据失败: {e}"));
                }
                let mut items = Vec::new();
                for (element, etype) in &elements {
                    for note in notes_by_element.get(element).into_iter().flatten() {
                        if let Some((source_id, role, quote)) =
                            crate::evidence::parse_evidence_note(note)
                        {
                            items.push((
                                etype.clone(),
                                element.clone(),
                                crate::evidence::Evidence {
                                    source_id,
                                    quote,
                                    span: None,
                                    role: if role == "inferred" {
                                        crate::evidence::EvidenceRole::Inferred
                                    } else {
                                        crate::evidence::EvidenceRole::Direct
                                    },
                                },
                            ));
                        }
                    }
                }
                if let Err(e) = crate::evidence::persist_element_evidence(
                    &conn, "w3c", &ontology_iri, items, now_ms(),
                ) {
                    warnings.push(format!("证据落库失败: {e}"));
                }
            }
            Err(e) => {
                errors.push(format!("SQLite 写入失败: {e}"));
            }
        }
        drop(conn);

        self.notify_change();

        Ok(TtlImportResult {
            ontology_iri,
            triple_count,
            errors,
            warnings,
        })
    }
}


impl TtlStore {
    /// 增量追加导入（append 模式）：delta 只含**新增**元素时，把文本拼接进库内
    /// 原文（无损，保留既有排版与注释），不触碰存量三元组。
    ///
    /// 安全边界：
    /// - 本体必须已存在（追加是对已有本体的操作）
    /// - delta 中任一 subject 与库内已有定义冲突 → 拒绝（修改类变更走
    ///   export 全文 → 修改 → import(overwrite=true) 全量回导，那才是替换语义）
    /// - 证据闸门按**合并后全文**校验（strict）：存量 grandfathered 豁免，
    ///   delta 新元素必须有证据注解
    /// - 缩水守卫不适用（不替换，存量三元组只增不减）
    pub fn append_ttl(&self, ontology_iri: &str, delta: &str) -> StoreResult<TtlImportResult> {
        let mut warnings = Vec::new();
        // 0. 双 # 合法化告警（同 import_ttl）
        let (_, legalize_warns) = legalize_double_hash_iris(delta);
        warnings.extend(legalize_warns);

        if !self.exists(ontology_iri)? {
            return Ok(TtlImportResult {
                ontology_iri: ontology_iri.to_string(),
                triple_count: 0,
                errors: vec![format!(
                    "本体 {ontology_iri} 不存在，无法追加——新建请走用户宪章向导 + import"
                )],
                warnings,
            });
        }

        let stored = self.export_ttl(ontology_iri)?;
        let stored_store = parse_ttl_to_memory(&stored)?;
        let delta_store = parse_ttl_to_memory(delta).map_err(|e| {
            StoreError::Other(anyhow::anyhow!("Turtle 解析失败: {e}"))
        })?;

        // 1. subject 冲突检查：delta 中已存在于库内的 subject → 拒绝（修改走全量回导）
        let existing_subjects: std::collections::HashSet<String> = stored_store
            .iter()
            .filter_map(|q| q.ok().map(|q| q.subject.to_string()))
            .collect();
        let mut conflicts: Vec<String> = Vec::new();
        for q in delta_store.iter() {
            let Ok(q) = q else { continue };
            let s = q.subject.to_string();
            if existing_subjects.contains(&s) && !conflicts.contains(&s) {
                conflicts.push(s);
            }
        }
        if !conflicts.is_empty() {
            return Ok(TtlImportResult {
                ontology_iri: ontology_iri.to_string(),
                triple_count: 0,
                errors: vec![format!(
                    "追加被拒绝：delta 中以下 subject 在库内已有定义（追加模式只允许新增元素，\
                     不允许重定义）：\n{}\n如需修改已有元素，请走 export 全文 → 修改 → \
                     import(overwrite=true) 全量回导",
                    conflicts
                        .iter()
                        .map(|s| format!("- {s}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )],
                warnings,
            });
        }

        // 2. 合并全文 = 库内原文 + delta（纯文本拼接，无损保留排版注释）
        let merged = format!("{}\n\n{}", stored.trim_end(), delta.trim());

        // 3. 证据闸门按合并后全文校验（strict）：存量豁免、新增需证据
        let (gate_errors, gate_warnings) = self.validate_ttl_evidence(&merged, true)?;
        if !gate_errors.is_empty() {
            return Ok(TtlImportResult {
                ontology_iri: ontology_iri.to_string(),
                triple_count: 0,
                errors: gate_errors,
                warnings: [warnings, gate_warnings].concat(),
            });
        }
        warnings.extend(gate_warnings);

        let triple_count = merged_store_count(&merged);

        // 4. 落库：文本拼接写回 + 合并后证据落库（只增不改：清旧证据会丢存量 matched_by，
        //    故对 delta 元素单独 upsert）
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE ontology_ttl SET ttl_content = ?2, triple_count = ?3, updated_at = ?4
             WHERE ontology_iri = ?1",
            rusqlite::params![ontology_iri, merged, triple_count as i64, now_ms()],
        )
        .map_err(StoreError::Sqlite)?;
        // delta 新元素的证据行（不影响存量 element_evidence）。
        // ⚠️ 必须用原文 tokenize 版而非 oxigraph 版：见 import_ttl 落库段注释——
        // oxigraph 序列化带尖括号 + 双 # 合法化成点号，element_id 与前端对不上。
        let (elements, notes_by_element) = crate::ttl_edit::evidence_extraction_from_text(delta);
        let mut items = Vec::new();
        for (element, etype) in &elements {
            for note in notes_by_element.get(element).into_iter().flatten() {
                if let Some((source_id, role, quote)) =
                    crate::evidence::parse_evidence_note(note)
                {
                    items.push((
                        etype.clone(),
                        element.clone(),
                        crate::evidence::Evidence {
                            source_id,
                            quote,
                            span: None,
                            role: if role == "inferred" {
                                crate::evidence::EvidenceRole::Inferred
                            } else {
                                crate::evidence::EvidenceRole::Direct
                            },
                        },
                    ));
                }
            }
        }
        if !items.is_empty() {
            if let Err(e) = crate::evidence::persist_element_evidence(
                &conn, "w3c", ontology_iri, items, now_ms(),
            ) {
                warnings.push(format!("证据落库失败: {e}"));
            }
        }
        drop(conn);
        self.notify_change();

        Ok(TtlImportResult {
            ontology_iri: ontology_iri.to_string(),
            triple_count,
            errors: Vec::new(),
            warnings,
        })
    }

    // ── 导出（序列化）────────────────────────────────────────────
}


impl TtlStore {
    /// 导出指定本体 IRI 的 Turtle 文本（对齐 Palantir `export(ontology_api_name)`）。
    /// 直接从 SQLite 读 .ttl 文本，不碰 oxigraph。
    /// 统计 TTL 文本的 triple 数（导入缩水防护用）。
    pub fn count_ttl_triples(&self, ttl_content: &str) -> StoreResult<u64> {
        let temp = parse_ttl_to_memory(ttl_content)?;
        Ok(temp.len().map(|n| n as u64).unwrap_or(0))
    }
}


impl TtlStore {
    pub fn export_ttl(&self, ontology_iri: &str) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        let ttl: String = conn.query_row(
            "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        )?;
        Ok(ttl)
    }

    // ── 属性/关系结构化编辑（决策：TTL 手动编辑 P0）─────────────
    //
    // 链路：export 原文 → ttl_edit 文本手术（注释/排版保真）→ validate_ttl
    // （errors 非空**拒绝落库**，比 import_ttl 的 best-effort 更严——手动编辑
    // 必须把损坏挡在库外）→ import_ttl(overwrite=true) → 调用方提交 changelog。
}


impl TtlStore {
    /// 列出本体的全部属性（DatatypeProperty / ObjectProperty）结构化视图。
    /// 散块聚合；domain/range/label/comment 为全 IRI / 纯文本。
    pub fn list_ttl_properties(&self, ontology_iri: &str) -> StoreResult<Vec<crate::ttl_edit::TtlPropertyEdit>> {
        let ttl = self.export_ttl(ontology_iri)?;
        Ok(crate::ttl_edit::list_properties(&ttl))
    }
}


impl TtlStore {
    /// 列出本体已声明的全部类 IRI（属性编辑表单的 domain/range 下拉选项）。
    pub fn list_ttl_class_iris(&self, ontology_iri: &str) -> StoreResult<Vec<String>> {
        let ttl = self.export_ttl(ontology_iri)?;
        Ok(crate::ttl_edit::list_class_iris(&ttl))
    }
}


impl TtlStore {
    /// 新增属性表单的默认 IRI 前缀（与既有属性 namespace 一致，导出形态统一）。
    pub fn ttl_default_property_namespace(
        &self,
        ontology_iri: &str,
    ) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        Ok(crate::ttl_edit::default_property_namespace(&ttl, ontology_iri))
    }
}


impl TtlStore {
    /// 编辑已有属性。文本手术 → validate（errors 拒绝）→ overwrite 落库。
    /// 返回手术后的完整 Turtle（前端可直接刷新预览）。
    pub fn edit_ttl_property(
        &self,
        ontology_iri: &str,
        edit: &crate::ttl_edit::TtlPropertyEdit,
    ) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let edited = crate::ttl_edit::edit_property(&ttl, edit)?;
        self.surgery_import(ontology_iri, &edited)
    }
}


impl TtlStore {
    /// 新增属性（插入在最后一个属性块之后，同类聚集）。
    pub fn add_ttl_property(
        &self,
        ontology_iri: &str,
        edit: &crate::ttl_edit::TtlPropertyEdit,
    ) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let added = crate::ttl_edit::add_property(&ttl, edit)?;
        self.surgery_import(ontology_iri, &added)
    }
}


impl TtlStore {
    /// 删除属性（同主语散块全删）。
    pub fn delete_ttl_property(&self, ontology_iri: &str, property_iri: &str) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let deleted = crate::ttl_edit::delete_property(&ttl, property_iri)?;
        self.surgery_import(ontology_iri, &deleted)
    }
}


impl TtlStore {
    /// 手术结果落库：validate（errors 拒绝）→ import overwrite。
    /// 与裸 import 的区别：**校验失败返回 Err 不落库**（手动编辑闸门）。
    fn surgery_import(&self, ontology_iri: &str, ttl: &str) -> StoreResult<String> {
        let v = self.validate_ttl(ttl)?;
        if !v.errors.is_empty() {
            return Err(StoreError::Other(anyhow::anyhow!(
                "编辑后校验失败，已拒绝落库：\n{}",
                v.errors.join("\n")
            )));
        }
        let r = self.import_ttl(ttl, true)?;
        if !r.errors.is_empty() {
            return Err(StoreError::Other(anyhow::anyhow!(
                "落库失败：\n{}",
                r.errors.join("\n")
            )));
        }
        if r.ontology_iri != ontology_iri {
            // 手术不该改变 owl:Ontology IRI（身份标识）；变了说明原文头部被动了
            return Err(StoreError::Other(anyhow::anyhow!(
                "编辑后本体 IRI 变为 '{}'（应为 '{}'），已落库但身份异常——请检查 owl:Ontology 声明",
                r.ontology_iri, ontology_iri
            )));
        }
        Ok(ttl.to_string())
    }
}


impl TtlStore {
    /// 列出所有已存本体（轻量摘要，对齐 Palantir `list_ontologies`）。
    pub fn list_ontologies(&self) -> StoreResult<Vec<TtlOntologySummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT o.ontology_iri, COALESCE(o.version, ''), o.triple_count, o.updated_at,
                    CASE WHEN c.ontology_iri IS NULL THEN 0
                         WHEN TRIM(COALESCE(c.business_scenario,''))=''
                          AND TRIM(COALESCE(c.business_essence,''))=''
 THEN 0
                         ELSE 1 END
             FROM ontology_ttl o
             LEFT JOIN ontology_ttl_charter c ON c.ontology_iri = o.ontology_iri
             ORDER BY o.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TtlOntologySummary {
                has_charter: r.get::<_, i64>(4)? != 0,
                ontology_iri: r.get(0)?,
                version: r.get(1)?,
                triple_count: r.get::<_, i64>(2)? as u64,
                updated_at: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}


impl TtlStore {
    /// 删除指定本体 IRI（幂等）。
    pub fn delete_ontology(&self, ontology_iri: &str) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let affected = conn.execute(
            "DELETE FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
        )?;
        // 证据索引随本体删除
        let _ = crate::evidence::delete_element_evidence(&conn, "w3c", ontology_iri);
        drop(conn);
        if affected > 0 {
            self.notify_change();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}


impl TtlStore {
    /// 执行 SPARQL 查询（第 7 类 CRUD，对齐 skill 方法论）。
    ///
    /// 把指定本体 IRI 的 .ttl 文本 load 进内存 oxigraph，跑 SPARQL，返回 JSON 结果。
    /// 适合中小本体（< 10 万三元组）；大本体会有秒级 parse 延迟。
    ///
    /// 自动注入常用 W3C 前缀声明（rdf/rdfs/owl/xsd/skos/swrl），用户查询可用缩写。
    pub fn query_sparql(&self, ontology_iri: &str, sparql: &str) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let store = parse_ttl_to_memory(&ttl)?;
        use oxigraph::sparql::{
            results::{QueryResultsFormat, QueryResultsSerializer},
            SparqlEvaluator,
        };
        // 注入常用前缀，让用户查询能用 owl:Class 等缩写
        let prefixed = inject_common_prefixes(sparql);
        let parsed = SparqlEvaluator::new()
            .parse_query(&prefixed)
            .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
        let results = parsed
            .on_store(&store)
            .execute()
            .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
        // 序列化为 SPARQL Results JSON
        let serializer = QueryResultsSerializer::from_format(QueryResultsFormat::Json);
        let mut buf = Vec::new();
        match results {
            oxigraph::sparql::QueryResults::Solutions(sols) => {
                let vars = sols.variables().to_vec();
                let mut writer = serializer
                    .serialize_solutions_to_writer(&mut buf, vars)
                    .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
                for sol in sols {
                    let sol = sol.map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
                    writer
                        .serialize(&sol)
                        .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
                }
                writer
                    .finish()
                    .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
            }
            oxigraph::sparql::QueryResults::Boolean(b) => {
                serializer
                    .serialize_boolean_to_writer(&mut buf, b)
                    .map_err(|e| StoreError::Other(anyhow::anyhow!(e)))?;
            }
            oxigraph::sparql::QueryResults::Graph(_) => {
                return Ok("{}".to_string());
            }
        }
        String::from_utf8(buf).map_err(|e| StoreError::Other(anyhow::anyhow!(e)))
    }

    // ════════════════════════════════════════════════════════════════
    // 本体设计宪章（不变点）+ 变更日志（变化点）
    // ════════════════════════════════════════════════════════════════
}


impl TtlStore {
    /// 本体 IRI 是否已存在（宪章先行闸门用）。
    pub fn exists(&self, ontology_iri: &str) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT 1 FROM ontology_ttl WHERE ontology_iri = ?1",
                rusqlite::params![ontology_iri],
                |_| Ok(()),
            )
            .is_ok())
    }
}


impl TtlStore {
    /// 提交一条本体变更日志（git commit message 式）。
    ///
    /// **每次 import_ttl 或 delete_ontology 之后应调用**——把本次改动的「为什么」和
    /// 「整体设计」落成可回溯记录。体积限制：title+body ≤500 字符，整条（含 summary）≤1K 字符。
    /// revision 每本体从 1 递增。返回分配的 revision 序号。
    /// changelog 写入不视为本体本身变更，不触发 notify_change（避免循环刷新）。
    pub fn commit_change(
        &self,
        ontology_iri: &str,
        title: &str,
        body: &str,
        change_summary: &str,
        conversation_id: Option<&str>,
        author: &str,
    ) -> StoreResult<u32> {
        const MAX_TITLE_BODY_CHARS: usize = 500;
        const MAX_TOTAL_CHARS: usize = 1000;
        let title_body = title.chars().count() + body.chars().count();
        if title_body > MAX_TITLE_BODY_CHARS {
            return Err(StoreError::InvalidApiName {
                entity_kind: "ttl_changelog",
                api_name: format!("title+body={title_body}"),
                pattern: "title+body <= 500 chars",
            });
        }
        let total = title_body + change_summary.chars().count();
        if total > MAX_TOTAL_CHARS {
            return Err(StoreError::InvalidApiName {
                entity_kind: "ttl_changelog",
                api_name: format!("total={total}"),
                pattern: "total <= 1000 chars",
            });
        }
        let conn = self.conn.lock().unwrap();
        // 校验本体存在
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM ontology_ttl WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
                |_| Ok(()),
            )
            .is_ok();
        if !exists {
            return Err(StoreError::NotFound(format!(
                "本体 '{ontology_iri}' 未找到，无法提交 changelog"
            )));
        }
        let next_rev: i64 = conn.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM ontology_ttl_changelog WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        )?;
        let now = now_ms();
        conn.execute(
            "INSERT INTO ontology_ttl_changelog (id, ontology_iri, revision, title, body, change_summary, conversation_id, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                new_id(),
                ontology_iri,
                next_rev,
                title,
                body,
                change_summary,
                conversation_id,
                author,
                now,
            ],
        )?;
        Ok(next_rev as u32)
    }
}


impl TtlStore {
    /// 列出本体的变更日志（按 revision 倒序，最新在前）。
    pub fn list_changelog(&self, ontology_iri: &str) -> StoreResult<Vec<TtlChangelog>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT revision, title, body, change_summary, conversation_id, author, created_at \
             FROM ontology_ttl_changelog WHERE ontology_iri=?1 ORDER BY revision DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![ontology_iri], |r| {
            Ok(TtlChangelog {
                revision: r.get::<_, i64>(0)? as u32,
                title: r.get(1)?,
                body: r.get(2)?,
                change_summary: r.get(3)?,
                conversation_id: r.get(4)?,
                author: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}


/// 单次解析同时取本体 IRI + 版本 + 三元组数（import 内避免二次 parse_ttl_to_memory）。
/// 与 `extract_ontology_meta` 同一解析逻辑，但顺带统计 triple_count，供 import_ttl
/// 复用——gate 已内联重跑 validate，此处不再重复解析。
pub(crate) fn extract_meta_and_count(
    ttl: &str,
) -> (Option<String>, Option<String>, u64) {
    let Ok(temp) = parse_ttl_to_memory(ttl) else {
        return (None, None, 0);
    };
    let triple_count = temp.len().map(|n| n as u64).unwrap_or(0);
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
    (iri, version, triple_count)
}

// ════════════════════════════════════════════════════════════════
// 内部：7+1 语义规范校验子项（纯函数，操作内存 oxigraph Store）
// ════════════════════════════════════════════════════════════════

/// 给 SPARQL 查询注入常用 W3C 前缀声明（若用户查询用了缩写但没声明 PREFIX）。
/// 仅当查询不含 `PREFIX` 关键字时注入（避免重复声明报错）。
pub(crate) fn inject_common_prefixes(sparql: &str) -> String {
    if sparql.contains("PREFIX ") || sparql.contains("PREFIX	") {
        return sparql.to_string();
    }
    format!(
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n         PREFIX owl: <http://www.w3.org/2002/07/owl#>\n         PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n         PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n         PREFIX swrl: <http://www.w3.org/2003/11/swrl#>\n         PREFIX rdfg: <http://www.w3.org/2009/pointg#>\n         {sparql}"
    )
}
