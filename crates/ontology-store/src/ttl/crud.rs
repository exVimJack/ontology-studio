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
            "INSERT INTO ontology_ttl (ontology_iri, ttl_content, version, triple_count, updated_at,
                                      active_revision, workspace_dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, 1)
             ON CONFLICT(ontology_iri) DO UPDATE SET
                ttl_content = excluded.ttl_content,
                version = excluded.version,
                triple_count = excluded.triple_count,
                updated_at = excluded.updated_at,
                workspace_dirty = 1",
            rusqlite::params![
                &ontology_iri,
                ttl_content,
                version.as_deref().unwrap_or(""),
                triple_count as i64,
                now_ms(),
            ],
        ) {
            Ok(_) => {
                // 内容变更 → 置脏；保留 active_revision（消费面仍读生效快照）
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
        // 内容变更 → 置脏；保留 active_revision
        Self::mark_workspace_dirty_conn(&conn, ontology_iri)?;
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
    /// 导出工作区草稿 TTL（编辑态 / Agent full 模式）。
    pub fn export_ttl(&self, ontology_iri: &str) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        let ttl: String = conn.query_row(
            "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        )?;
        Ok(ttl)
    }

    /// 导出生效发布快照 TTL（消费面：文件导出 / Agent summary / 联邦）。
    ///
    /// - `active_revision` 为空 → [`StoreError::Unpublished`]
    /// - 脏工作区仍返回生效快照（不含未发布修改）
    pub fn export_effective_ttl(&self, ontology_iri: &str) -> StoreResult<String> {
        let conn = self.conn.lock().unwrap();
        let active: Option<i64> = match conn.query_row(
            "SELECT active_revision FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(StoreError::NotFound(format!("本体 '{ontology_iri}' 未找到")));
            }
            Err(e) => return Err(e.into()),
        };
        let Some(rev) = active else {
            return Err(StoreError::Unpublished(ontology_iri.to_string()));
        };
        let ttl: String = match conn.query_row(
            "SELECT ttl_content FROM ontology_ttl_versions \
             WHERE ontology_iri=?1 AND revision=?2",
            rusqlite::params![ontology_iri, rev],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(StoreError::NotFound(format!(
                    "生效版本（第 {rev} 次发布）快照缺失"
                )));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(ttl)
    }

    /// 工作区是否相对生效快照有未发布修改。
    pub fn get_workspace_dirty(&self, ontology_iri: &str) -> StoreResult<bool> {
        let conn = self.conn.lock().unwrap();
        let dirty: i64 = match conn.query_row(
            "SELECT workspace_dirty FROM ontology_ttl WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(StoreError::NotFound(format!("本体 '{ontology_iri}' 未找到")));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(dirty != 0)
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
    /// 编辑类声明块（label/comment/subClassOf 受管重写；其余谓词原样保留）。
    pub fn edit_ttl_class(
        &self,
        ontology_iri: &str,
        edit: &crate::ttl_edit::TtlClassEdit,
    ) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let edited = crate::ttl_edit::edit_class(&ttl, edit)?;
        self.surgery_import(ontology_iri, &edited)
    }

    /// 删除类（卡片硬删）：级联处理独占/共享数据属性、触及该类的关系、继承边后删类。
    /// 不再因属性/关系引用拒绝；残留无法自动清理的引用（如实例）返回人话错误。
    pub fn delete_ttl_class(&self, ontology_iri: &str, class_iri: &str) -> StoreResult<String> {
        let ttl = self.export_ttl(ontology_iri)?;
        let deleted = crate::ttl_edit::delete_class_cascade(&ttl, class_iri)?;
        self.surgery_import(ontology_iri, &deleted)
    }

    /// 类元数据列表（label/comment，概念图谱编辑表单回显用）。
    pub fn list_ttl_class_metas(&self, ontology_iri: &str) -> StoreResult<Vec<crate::ttl_edit::TtlClassMeta>> {
        let ttl = self.export_ttl(ontology_iri)?;
        Ok(crate::ttl_edit::list_class_metas(&ttl))
    }

    /// 逐条撤销：相对当前生效快照，将工作区中该 diff 条目还原。
    /// 无生效版时拒绝。成功后若工作区已与生效版无结构化差异则清 dirty。
    pub fn revoke_workspace_diff_item(
        &self,
        ontology_iri: &str,
        item: &crate::ttl::WorkspaceDiffRevokeItem,
    ) -> StoreResult<String> {
        let effective = self.export_effective_ttl(ontology_iri).map_err(|e| {
            StoreError::Other(anyhow::anyhow!(
                "无法读取生效版（需先发布）：{e}"
            ))
        })?;
        let workspace = self.export_ttl(ontology_iri)?;
        let next = apply_revoke_patch(&workspace, &effective, item)?;
        let out = self.surgery_import(ontology_iri, &next)?;
        self.clear_dirty_if_matches_effective(ontology_iri)?;
        Ok(out)
    }

    /// 类行级联撤销：服务端按当前 workspace vs 生效版 diff 重算强依赖并原子写回。
    /// `op`: `add` = 撤销新增类；`remove` = 恢复已删除类。
    ///
    /// 恢复方向额外受「对端存在性门禁」约束，见 [`build_class_cascade_steps`]。
    pub fn revoke_workspace_class_cascade(
        &self,
        ontology_iri: &str,
        class_iri: &str,
        op: &str,
    ) -> StoreResult<String> {
        let effective = self.export_effective_ttl(ontology_iri).map_err(|e| {
            StoreError::Other(anyhow::anyhow!(
                "无法读取生效版（需先发布）：{e}"
            ))
        })?;
        let mut workspace = self.export_ttl(ontology_iri)?;

        let Some(active) = self.get_active_revision(ontology_iri)? else {
            return Err(StoreError::Other(anyhow::anyhow!(
                "无生效版本，无法级联撤销"
            )));
        };
        let diff = self.diff_versions(ontology_iri, active, 0)?;

        let class_in_added = diff
            .added_classes
            .iter()
            .any(|i| iri_loose_eq(i, class_iri));
        let class_in_removed = diff
            .removed_classes
            .iter()
            .any(|i| iri_loose_eq(i, class_iri));

        match op {
            "add" if !class_in_added => {
                return Err(StoreError::Other(anyhow::anyhow!(
                    "工作区中无新增类 '{class_iri}'，无法撤销"
                )));
            }
            "remove" if !class_in_removed => {
                return Err(StoreError::Other(anyhow::anyhow!(
                    "工作区中无已删除类 '{class_iri}'，无法恢复"
                )));
            }
            "add" | "remove" => {}
            other => {
                return Err(StoreError::Other(anyhow::anyhow!(
                    "不支持的类级联操作：{other}"
                )));
            }
        }

        let class_iri_canonical = if op == "add" {
            diff.added_classes
                .iter()
                .find(|i| iri_loose_eq(i, class_iri))
                .cloned()
                .unwrap_or_else(|| class_iri.to_string())
        } else {
            diff.removed_classes
                .iter()
                .find(|i| iri_loose_eq(i, class_iri))
                .cloned()
                .unwrap_or_else(|| class_iri.to_string())
        };

        let plan =
            build_class_cascade_steps(&diff, &class_iri_canonical, op, &workspace, &effective);

        // 跳过项落日志：便于线上核对「为什么这条关系没随类恢复」，与前端回执同源同口径
        if !plan.skipped.is_empty() {
            tracing::debug!(
                "类级联撤销跳过对端缺失的构件: ontology={ontology_iri} class={class_iri} op={op} skipped={:?} steps={:?}",
                plan.skipped,
                plan.steps.len(),
            );
        }

        for step in &plan.steps {
            // 级联撤新增类时跳过单条「仍被引用」检查：构件已先删
            workspace = apply_revoke_patch_cascade(&workspace, &effective, step)?;
        }

        // 恢复方向补一步：类声明块是从生效版原样迁回的，块内指向「仍不存在的类」的
        // rdfs:subClassOf 父项同样属悬空公理，按同一门禁摘除；父类后续恢复时随其级联带回。
        if op == "remove" {
            workspace = strip_subclass_of_missing_parents(&workspace, &class_iri_canonical)?;
        }

        let out = self.surgery_import(ontology_iri, &workspace)?;
        self.clear_dirty_if_matches_effective(ontology_iri)?;
        Ok(out)
    }

}

/// 类级联撤销的执行计划：待应用步骤 + 因对端存在性被跳过的条目。
pub(crate) struct ClassCascadePlan {
    pub steps: Vec<WorkspaceDiffRevokeItem>,
    /// 恢复方向未纳入步骤的条目（属性/关系 IRI，或继承边的子类 IRI）。
    pub skipped: Vec<String>,
}

/// 按当前 diff 组装类级联步骤。
///
/// `op == "add"`（撤销新增类）：先清构件 / 回滚 ~，最后删类，语义保持既有规定。
///
/// `op == "remove"`（恢复已删除类）：先恢复类，再恢复构件 / 回滚 ~，并施加
/// **对端存在性门禁**——构件恢复后不得指向工作区中不存在的类：
/// 1. 端点在生效版里是已声明类（`a owl:Class`）才算「类端点」，xsd 值域等自然豁免；
/// 2. 该类端点在「恢复 C 之后的工作区类集合」里不存在 → 跳过该构件，保持其删除差异，
///    待用户恢复对端类时随该类的级联一并带回。
///
/// 自环（domain 与 range 同为 C）随 C 正常恢复。先恢复 C 还是先恢复对端类，终态一致。
pub(crate) fn build_class_cascade_steps(
    diff: &TtlVersionDiff,
    class_iri_canonical: &str,
    op: &str,
    workspace: &str,
    effective: &str,
) -> ClassCascadePlan {
    let mut steps: Vec<WorkspaceDiffRevokeItem> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    if op == "add" {
        // 先清构件 / 回滚 ~，最后删类
        for p in &diff.added_properties {
            if prop_is_strong_dep(p, class_iri_canonical) {
                steps.push(WorkspaceDiffRevokeItem::Property {
                    op: "add".into(),
                    iri: p.iri.clone(),
                });
            }
        }
        for p in &diff.modified_properties {
            if domain_range_diff_touches_c(&p.before, &p.after, class_iri_canonical) {
                steps.push(WorkspaceDiffRevokeItem::Property {
                    op: "modify".into(),
                    iri: p.iri.clone(),
                });
            }
        }
        for e in &diff.added_subclass_of {
            if iri_loose_eq(&e.child, class_iri_canonical)
                || iri_loose_eq(&e.parent, class_iri_canonical)
            {
                steps.push(WorkspaceDiffRevokeItem::Subclass {
                    op: "add".into(),
                    child: e.child.clone(),
                    parent: e.parent.clone(),
                });
            }
        }
        steps.push(WorkspaceDiffRevokeItem::Class {
            op: "add".into(),
            iri: class_iri_canonical.to_string(),
        });
        return ClassCascadePlan { steps, skipped };
    }

    // 恢复方向：先恢复类，再恢复构件 / 回滚 ~
    steps.push(WorkspaceDiffRevokeItem::Class {
        op: "remove".into(),
        iri: class_iri_canonical.to_string(),
    });

    let effective_class_locals = local_name_set(&crate::ttl_edit::list_class_iris(effective));
    let mut present_class_locals = local_name_set(&crate::ttl_edit::list_class_iris(workspace));
    present_class_locals.insert(local_name(class_iri_canonical));
    // 端点是「生效版里声明过的类」且恢复 C 后工作区仍没有它 → 缺端点，构件不进级联
    let endpoint_missing = |iri: &str| -> bool {
        let l = local_name(iri);
        !l.is_empty() && effective_class_locals.contains(&l) && !present_class_locals.contains(&l)
    };

    for p in &diff.removed_properties {
        if !prop_is_strong_dep(p, class_iri_canonical) {
            continue;
        }
        if endpoint_missing(&p.domain) || endpoint_missing(&p.range) {
            skipped.push(p.iri.clone());
            continue;
        }
        steps.push(WorkspaceDiffRevokeItem::Property {
            op: "remove".into(),
            iri: p.iri.clone(),
        });
    }
    for p in &diff.modified_properties {
        if !domain_range_diff_touches_c(&p.before, &p.after, class_iri_canonical) {
            continue;
        }
        // 回滚目标是生效版定义（before），按回滚后的端点判存在性
        if endpoint_missing(&p.before.domain) || endpoint_missing(&p.before.range) {
            skipped.push(p.iri.clone());
            continue;
        }
        steps.push(WorkspaceDiffRevokeItem::Property {
            op: "modify".into(),
            iri: p.iri.clone(),
        });
    }
    for e in &diff.removed_subclass_of {
        if !iri_loose_eq(&e.child, class_iri_canonical)
            && !iri_loose_eq(&e.parent, class_iri_canonical)
        {
            continue;
        }
        if endpoint_missing(&e.child) || endpoint_missing(&e.parent) {
            skipped.push(e.child.clone());
            continue;
        }
        steps.push(WorkspaceDiffRevokeItem::Subclass {
            op: "remove".into(),
            child: e.child.clone(),
            parent: e.parent.clone(),
        });
    }

    ClassCascadePlan { steps, skipped }
}

/// IRI 末段 local name（与 [`iri_loose_eq`] 同一口径）。
fn local_name(iri: &str) -> String {
    iri.rsplit(['#', '/']).next().unwrap_or("").to_string()
}

/// 一组 IRI 的 local name 集合。
fn local_name_set(iris: &[String]) -> std::collections::HashSet<String> {
    iris.iter().map(|i| local_name(i)).collect()
}

/// 摘掉某类指向「工作区中不存在的类」的 `rdfs:subClassOf` 父项。
///
/// 类声明块由 [`crate::ttl_edit::extract_subject_raw_blocks`] 从生效版原样迁回，块内的父类
/// 引用不受级联门禁约束，因此对端父类仍为删除态时会带回一条悬空继承边。此处按同一口径
/// （端点是已声明类且工作区里没有它）摘除该父项，label/comment 与其余谓词原样保留。
fn strip_subclass_of_missing_parents(ttl: &str, class_iri: &str) -> StoreResult<String> {
    let present = local_name_set(&crate::ttl_edit::list_class_iris(ttl));
    let self_local = local_name(class_iri);
    let parents: Vec<String> = crate::ttl_edit::list_subclass_of_edges(ttl)
        .into_iter()
        .filter(|(c, _)| iri_loose_eq(c, class_iri))
        .map(|(_, p)| p)
        .collect();
    let kept: Vec<String> = parents
        .iter()
        .filter(|p| {
            let l = local_name(p);
            !l.is_empty() && (l == self_local || present.contains(&l))
        })
        .cloned()
        .collect();
    if kept.len() == parents.len() {
        return Ok(ttl.to_string());
    }
    let (label, comment) = crate::ttl_edit::list_class_metas(ttl)
        .into_iter()
        .find(|c| iri_loose_eq(&c.iri, class_iri))
        .map(|m| (m.label, m.comment))
        .unwrap_or_default();
    let edit = crate::ttl_edit::TtlClassEdit {
        iri: class_iri.to_string(),
        label,
        comment,
        parents: kept,
    };
    crate::ttl_edit::edit_class(ttl, &edit)
}

fn iri_loose_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let sa = a.rsplit(['#', '/']).next().unwrap_or("");
    let sb = b.rsplit(['#', '/']).next().unwrap_or("");
    !sa.is_empty() && sa == sb
}

/// 强依赖：domain 或 range 触及类即纳入（不依赖 kind）。
/// 否则 `kind` 空/异常时，仅值域落在 C 的关系会漏级联，图上两端仍虚线着色。
fn prop_is_strong_dep(p: &crate::ttl_edit::TtlPropertyEdit, class_iri: &str) -> bool {
    iri_loose_eq(&p.domain, class_iri) || iri_loose_eq(&p.range, class_iri)
}

fn domain_range_diff_touches_c(
    before: &crate::ttl_edit::TtlPropertyEdit,
    after: &crate::ttl_edit::TtlPropertyEdit,
    class_iri: &str,
) -> bool {
    if before.domain != after.domain
        && (iri_loose_eq(&before.domain, class_iri) || iri_loose_eq(&after.domain, class_iri))
    {
        return true;
    }
    if before.range != after.range
        && (iri_loose_eq(&before.range, class_iri) || iri_loose_eq(&after.range, class_iri))
    {
        return true;
    }
    false
}

fn apply_revoke_patch(
    workspace: &str,
    effective: &str,
    item: &crate::ttl::WorkspaceDiffRevokeItem,
) -> StoreResult<String> {
    apply_revoke_patch_inner(workspace, effective, item, false)
}

/// 级联路径：撤新增类时跳过「仍被引用」门禁（构件已先撤）。
fn apply_revoke_patch_cascade(
    workspace: &str,
    effective: &str,
    item: &crate::ttl::WorkspaceDiffRevokeItem,
) -> StoreResult<String> {
    apply_revoke_patch_inner(workspace, effective, item, true)
}

fn apply_revoke_patch_inner(
    workspace: &str,
    effective: &str,
    item: &crate::ttl::WorkspaceDiffRevokeItem,
    cascade_skip_class_refs: bool,
) -> StoreResult<String> {
    use crate::ttl::WorkspaceDiffRevokeItem;
    use crate::ttl_edit::{self, TtlClassEdit};

    match item {
        WorkspaceDiffRevokeItem::Property { op, iri } => match op.as_str() {
            "add" => ttl_edit::delete_property(workspace, iri),
            "modify" | "remove" => {
                let props = ttl_edit::list_properties(effective);
                let Some(p) = props.into_iter().find(|p| p.iri == *iri) else {
                    return Err(StoreError::NotFound(format!(
                        "生效版中无属性 '{iri}'，无法撤销"
                    )));
                };
                if op == "remove" {
                    ttl_edit::add_property(workspace, &p)
                } else {
                    ttl_edit::edit_property(workspace, &p)
                }
            }
            other => Err(StoreError::Other(anyhow::anyhow!(
                "不支持的属性撤销操作：{other}"
            ))),
        },
        WorkspaceDiffRevokeItem::Class { op, iri } => match op.as_str() {
            "add" => {
                if !cascade_skip_class_refs {
                    let refs = ttl_edit::find_class_references(workspace, iri);
                    if !refs.is_empty() {
                        return Err(StoreError::Other(anyhow::anyhow!(
                            "类 '{}' 仍被引用，无法撤销新增：\n{}",
                            iri,
                            refs.join("\n")
                        )));
                    }
                }
                ttl_edit::delete_class(workspace, iri)
            }
            "modify" => {
                let metas = ttl_edit::list_class_metas(effective);
                let Some(m) = metas.into_iter().find(|c| c.iri == *iri) else {
                    return Err(StoreError::NotFound(format!(
                        "生效版中无类 '{iri}'，无法撤销"
                    )));
                };
                let parents: Vec<String> = ttl_edit::list_subclass_of_edges(effective)
                    .into_iter()
                    .filter(|(c, _)| c == iri)
                    .map(|(_, p)| p)
                    .collect();
                let edit = TtlClassEdit {
                    iri: iri.clone(),
                    label: m.label,
                    comment: m.comment,
                    parents,
                };
                ttl_edit::edit_class(workspace, &edit)
            }
            "remove" => {
                let blocks = ttl_edit::extract_subject_raw_blocks(effective, iri)?;
                Ok(ttl_edit::append_raw_blocks(workspace, &blocks))
            }
            other => Err(StoreError::Other(anyhow::anyhow!(
                "不支持的类撤销操作：{other}"
            ))),
        },
        WorkspaceDiffRevokeItem::Subclass {
            op,
            child,
            parent,
        } => {
            let metas = ttl_edit::list_class_metas(workspace);
            let meta = metas.iter().find(|c| c.iri == *child);
            let (label, comment) = meta
                .map(|m| (m.label.clone(), m.comment.clone()))
                .unwrap_or_default();
            let mut parents: Vec<String> = ttl_edit::list_subclass_of_edges(workspace)
                .into_iter()
                .filter(|(c, _)| c == child)
                .map(|(_, p)| p)
                .collect();
            match op.as_str() {
                "add" => {
                    parents.retain(|p| p != parent);
                }
                "remove" => {
                    if !parents.iter().any(|p| p == parent) {
                        parents.push(parent.clone());
                    }
                }
                other => {
                    return Err(StoreError::Other(anyhow::anyhow!(
                        "不支持的继承撤销操作：{other}"
                    )));
                }
            }
            let edit = TtlClassEdit {
                iri: child.clone(),
                label,
                comment,
                parents,
            };
            ttl_edit::edit_class(workspace, &edit)
        }
        WorkspaceDiffRevokeItem::Meta { .. } => Err(StoreError::Other(anyhow::anyhow!(
            "本体头部元信息撤销暂未支持，请整包回退版本或手动改源码"
        ))),
    }
}

impl TtlStore {
    /// 工作区与生效版无结构化差异时清 dirty。
    fn clear_dirty_if_matches_effective(&self, ontology_iri: &str) -> StoreResult<()> {
        let Some(active) = self.get_active_revision(ontology_iri)? else {
            return Ok(());
        };
        let diff = self.diff_versions(ontology_iri, active, 0)?;
        let empty = diff.added_classes.is_empty()
            && diff.removed_classes.is_empty()
            && diff.modified_classes.is_empty()
            && diff.added_properties.is_empty()
            && diff.removed_properties.is_empty()
            && diff.modified_properties.is_empty()
            && diff.added_subclass_of.is_empty()
            && diff.removed_subclass_of.is_empty()
            && !diff.ontology_meta_changed;
        if empty {
            let conn = self.conn.lock().unwrap();
            conn.execute(
                "UPDATE ontology_ttl SET workspace_dirty = 0 WHERE ontology_iri=?1",
                rusqlite::params![ontology_iri],
            )?;
        }
        Ok(())
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
                         ELSE 1 END,
                    o.active_revision, o.workspace_dirty
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
                active_revision: r.get(5)?,
                workspace_dirty: r.get::<_, i64>(6)? != 0,
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
        // 版本快照随本体删除（显式删：FK 级联依赖 PRAGMA foreign_keys，此处兜底）
        conn.execute(
            "DELETE FROM ontology_ttl_versions WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
        )?;
        // 证据索引随本体删除
        let _ = crate::evidence::delete_element_evidence(&conn, "w3c", ontology_iri);
        // 校验对抗产物随本体删除——validation_issues.id 是全局主键而规则 issue
        // id 只在本体内唯一，孤儿行会让后续本体 persist 同名 id 撞 UNIQUE
        conn.execute(
            "DELETE FROM validation_issues WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
        )?;
        conn.execute(
            "DELETE FROM validation_reviews WHERE ontology_iri = ?",
            rusqlite::params![ontology_iri],
        )?;
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
// 映射绑定层（决策 25）：W3C 侧入口
//
// 与 Palantir 侧（`OntologyStore` 上的同名方法）共用 `mapping.rs` 的读写实现，
// 差异只有两级标识的取值：
//   - `ontology_key` = 本体 IRI（不是 api_name）
//   - `element_id`   = 元素 IRI 原文形态（可能含双 `#`，与 `element_evidence` 同一坐标系）
// ════════════════════════════════════════════════════════════════

impl TtlStore {
    /// 保存映射（按元素整组替换语义）。W3C 路径**只写新表**，不写 .ttl。
    pub fn save_element_mappings(
        &self,
        ontology_iri: &str,
        items: impl IntoIterator<Item = crate::mapping::ElementMappingItem>,
        snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
    ) -> StoreResult<crate::mapping::MappingWriteOutcome> {
        let conn = self.conn.lock().unwrap();
        let outcome = crate::mapping::upsert_element_mappings(
            &conn,
            crate::mapping::STANDARD_W3C,
            ontology_iri,
            items,
            snapshot,
            now_ms(),
        )?;
        if outcome.written > 0 || outcome.removed > 0 {
            // 映射不是设计资产：只通知前端刷新映射视图，不动 workspace_dirty、
            // 不写 changelog、不产生版本快照（design D8）。
            self.notify_change();
        }
        Ok(outcome)
    }

    /// 列出某本体（IRI）的全部映射。
    pub fn list_element_mappings(
        &self,
        ontology_iri: &str,
    ) -> StoreResult<Vec<crate::mapping::ElementMapping>> {
        let conn = self.conn.lock().unwrap();
        crate::mapping::list_element_mappings(&conn, crate::mapping::STANDARD_W3C, ontology_iri)
    }

    /// 某元素的映射列表（属性卡/关系卡渲染用）。
    pub fn list_element_mappings_for(
        &self,
        ontology_iri: &str,
        element_type: &str,
        element_id: &str,
    ) -> StoreResult<Vec<crate::mapping::ElementMapping>> {
        let conn = self.conn.lock().unwrap();
        crate::mapping::list_element_mappings_for(
            &conn,
            crate::mapping::STANDARD_W3C,
            ontology_iri,
            element_type,
            element_id,
        )
    }

    /// 删除某元素的全部映射（返回删除行数）。
    pub fn delete_element_mappings_for_element(
        &self,
        ontology_iri: &str,
        element_type: &str,
        element_id: &str,
    ) -> StoreResult<usize> {
        let conn = self.conn.lock().unwrap();
        let n = crate::mapping::delete_element_mappings_for_element(
            &conn,
            crate::mapping::STANDARD_W3C,
            ontology_iri,
            element_type,
            element_id,
        )?;
        if n > 0 {
            self.notify_change();
        }
        Ok(n)
    }

    /// 映射覆盖率（类 / 属性 / 关系分别统计）。
    pub fn mapping_coverage(
        &self,
        ontology_iri: &str,
    ) -> StoreResult<crate::mapping::MappingCoverage> {
        let conn = self.conn.lock().unwrap();
        let elements = w3c_elements(&conn, ontology_iri)?;
        let mappings = crate::mapping::list_element_mappings(
            &conn,
            crate::mapping::STANDARD_W3C,
            ontology_iri,
        )?;
        Ok(crate::mapping::compute_coverage(
            crate::mapping::STANDARD_W3C,
            ontology_iri,
            &elements,
            &mappings,
        ))
    }

    /// 映射完整性报告（孤儿 / 失效 / 未核对）。
    pub fn mapping_integrity(
        &self,
        ontology_iri: &str,
        snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
    ) -> StoreResult<crate::mapping::MappingIntegrityReport> {
        let conn = self.conn.lock().unwrap();
        let elements = w3c_elements(&conn, ontology_iri)?;
        let mappings = crate::mapping::list_element_mappings(
            &conn,
            crate::mapping::STANDARD_W3C,
            ontology_iri,
        )?;
        Ok(crate::mapping::compute_integrity(
            crate::mapping::STANDARD_W3C,
            ontology_iri,
            &elements,
            &mappings,
            snapshot,
        ))
    }

    /// 清理孤儿映射（**仅在用户确认后调用**）。
    pub fn purge_orphan_mappings(
        &self,
        ontology_iri: &str,
        snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
    ) -> StoreResult<usize> {
        let report = self.mapping_integrity(ontology_iri, snapshot)?;
        let ids = crate::mapping::orphan_mapping_ids(&report);
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().unwrap();
        let n = crate::mapping::delete_element_mappings_by_ids(&conn, &ids)?;
        self.notify_change();
        Ok(n)
    }

    /// 候选建议（只读，不落库）。同表线索由调用方（或本函数）补：类已映射的表。
    pub fn suggest_element_mappings(
        &self,
        ontology_iri: &str,
        elements: &[crate::mapping::MappingElementRef],
        snapshot: &crate::mapping_verify::PhysicalSchemaSnapshot,
    ) -> StoreResult<Vec<crate::mapping::MappingSuggestion>> {
        let known = self.list_element_mappings(ontology_iri)?;
        Ok(crate::mapping_suggest::suggest(elements, snapshot, &known))
    }

    /// 列出本体的元素全集（含建议所需元数据）。
    pub fn list_mapping_elements(
        &self,
        ontology_iri: &str,
    ) -> StoreResult<Vec<crate::mapping::MappingElementRef>> {
        let conn = self.conn.lock().unwrap();
        w3c_element_refs(&conn, ontology_iri)
    }

    /// 装配映射导出数据集（xlsx 渲染与 Agent 文本预览共用）。
    ///
    /// W3C 侧实体在其 payload 概念里没有类似 `ontologies.display_name` 的单行
    /// 元数据表在此处可查，概览页的本体名回退为 IRI 末段（业务可读）。
    pub fn mapping_dataset(
        &self,
        ontology_iri: &str,
        scope: crate::mapping::MappingExportScope,
        snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
        exported_at: &str,
    ) -> StoreResult<crate::mapping_export::ExportDataset> {
        let conn = self.conn.lock().unwrap();
        let elements = w3c_element_refs(&conn, ontology_iri)?;
        let mappings = crate::mapping::list_element_mappings(
            &conn,
            crate::mapping::STANDARD_W3C,
            ontology_iri,
        )?;
        let display = w3c_ontology_display_name(&conn, ontology_iri);
        Ok(crate::mapping_export::build_dataset(
            crate::mapping_export::DatasetInput {
                standard: crate::mapping::STANDARD_W3C,
                ontology_key: ontology_iri,
                ontology_name: &display,
                scope,
                elements: &elements,
                mappings: &mappings,
                snapshot,
                // W3C 无「属性级主键」概念（Palantir payload 独有），传空表。
                primary_keys: &std::collections::HashMap::new(),
                exported_at,
            },
        ))
    }

    /// 导出映射为 xlsx 工作簿字节（多工作表：概览 + 类 + 属性 + 关系）。
    pub fn mapping_workbook(
        &self,
        ontology_iri: &str,
        scope: crate::mapping::MappingExportScope,
        snapshot: Option<&crate::mapping_verify::PhysicalSchemaSnapshot>,
        exported_at: &str,
    ) -> StoreResult<Vec<u8>> {
        let ds = self.mapping_dataset(ontology_iri, scope, snapshot, exported_at)?;
        crate::mapping_export::render_workbook(&ds)
    }
}

/// W3C 侧元素全集（class / datatype_property / object_property），从 .ttl 原文提取。
///
/// `element_id` 用原文 token 形态（双 `#` 保留、无尖括号）——与 `element_evidence`
/// 的坐标系一致，前端 `r.element_id === iri` 才能精确命中。
fn w3c_elements(
    conn: &Connection,
    ontology_iri: &str,
) -> StoreResult<Vec<(String, String)>> {
    Ok(w3c_element_refs(conn, ontology_iri)?
        .into_iter()
        .map(|e| (e.element_type, e.element_id))
        .collect())
}

/// W3C 本体显示名（概览页用）。
///
/// 取名优先级：
/// 1. `ontology_ttl.display_name` 列——用户在「新建本体」向导填的名称，
///    独立落列，不会被 agent 全量导入改写（这是用户问的「创建本体时的名称」）；
/// 2. `owl:Ontology` 头部 `rdfs:label`——手工导入的 .ttl 可能自己带了名字；
/// 3. IRI 末段（末段为空时用整串，避免概览页留白）。
fn w3c_ontology_display_name(conn: &Connection, ontology_iri: &str) -> String {
    let row = conn.query_row(
        "SELECT display_name, ttl_content FROM ontology_ttl WHERE ontology_iri=?1",
        rusqlite::params![ontology_iri],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    );
    let (stored_name, ttl) = row.unwrap_or_default();

    if !stored_name.trim().is_empty() {
        return stored_name;
    }
    let label = crate::ttl::versions::ontology_label(&ttl);
    if !label.trim().is_empty() {
        return label;
    }
    // IRI 以 `#`/`/` 收尾时末段为空（命名空间形式），此时用整串，避免概览页留白
    let short = local_name(ontology_iri);
    if short.is_empty() {
        ontology_iri.to_string()
    } else {
        short
    }
}

/// W3C 侧元素全集 + 建议所需元数据 + 导出所需语义上下文。
///
/// - `display_name` = `rdfs:label`（空则取 IRI 末段）
/// - `data_type` = `rdfs:range` 映射成的 DataType（仅数据属性）
/// - 语义上下文：属性取 `rdfs:domain` 作为所属类；对象属性取 `domain`/`range`
///   作为源类/目标类（见 `design.md` D6）。
///
/// **端点属性与基数留空**：Turtle 通常不声明「用哪个数据属性做外键」，也不声明
/// 基数（需 `owl:cardinality` 约束，本项目建模规范未要求）。此处诚实留空，
/// 不按命名猜测——规格要求 `Undeclared endpoint property stays empty`。
fn w3c_element_refs(
    conn: &Connection,
    ontology_iri: &str,
) -> StoreResult<Vec<crate::mapping::MappingElementRef>> {
    use crate::mapping::{
        ETYPE_CLASS, ETYPE_DATATYPE_PROPERTY, ETYPE_OBJECT_PROPERTY,
    };
    let ttl: String = conn
        .query_row(
            "SELECT ttl_content FROM ontology_ttl WHERE ontology_iri=?1",
            rusqlite::params![ontology_iri],
            |r| r.get(0),
        )
        .map_err(|_| StoreError::NotFound(format!("ontology '{ontology_iri}'")))?;

    // 类 IRI / 末段 → (IRI, 显示名)。domain/range 引用类时用它回填对象名。
    let class_metas = crate::ttl_edit::list_class_metas(&ttl);
    let mut class_names: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    for meta in &class_metas {
        let name = if meta.label.is_empty() {
            local_name(&meta.iri)
        } else {
            meta.label.clone()
        };
        class_names.insert(meta.iri.clone(), (meta.iri.clone(), name.clone()));
        class_names
            .entry(local_name(&meta.iri))
            .or_insert((meta.iri.clone(), name));
    }
    let lookup = |iri: &str| -> (String, String) {
        if iri.is_empty() {
            return (String::new(), String::new());
        }
        class_names
            .get(iri)
            .or_else(|| class_names.get(&local_name(iri)))
            .cloned()
            .unwrap_or_else(|| (iri.to_string(), local_name(iri)))
    };

    let mut out = Vec::new();
    for meta in &class_metas {
        out.push(crate::mapping::MappingElementRef {
            element_type: ETYPE_CLASS.to_string(),
            element_id: meta.iri.clone(),
            display_name: if meta.label.is_empty() {
                local_name(&meta.iri)
            } else {
                meta.label.clone()
            },
            data_type: String::new(),
            same_table_hint: String::new(),
            description: meta.comment.clone(),
            ..Default::default()
        });
    }
    for p in crate::ttl_edit::list_properties(&ttl) {
        // rdfs:range 是 xsd 类型（datatype）或类 IRI（object）；建议器只认前者
        let data_type = map_range_to_data_type(&p.range);
        let is_object = p.kind == "object";
        let (owner_id, owner_name) = lookup(&p.domain);
        let (target_id, target_name) = if is_object {
            lookup(&p.range)
        } else {
            (String::new(), String::new())
        };
        out.push(crate::mapping::MappingElementRef {
            element_type: if is_object {
                ETYPE_OBJECT_PROPERTY.to_string()
            } else {
                ETYPE_DATATYPE_PROPERTY.to_string()
            },
            element_id: p.iri.clone(),
            display_name: if p.label.is_empty() {
                local_name(&p.iri)
            } else {
                p.label.clone()
            },
            data_type,
            same_table_hint: String::new(),
            owner_id,
            owner_name,
            target_id,
            target_name,
            // 端点属性与基数在 Turtle 语义里未声明，保持空（不猜）。
            endpoint_property: String::new(),
            target_property: String::new(),
            cardinality: String::new(),
            description: p.comment.clone(),
        });
    }
    Ok(out)
}

/// xsd range → DataType 枚举（`xsd:string` → `STRING`）；未知返回空串。
fn map_range_to_data_type(range: &str) -> String {
    let local = range.rsplit('#').next().unwrap_or(range);
    match local {
        "string" | "normalizedString" | "token" => "STRING",
        "int" | "integer" | "nonNegativeInteger" | "positiveInteger" | "negativeInteger"
        | "short" | "byte" => "INTEGER",
        "long" => "LONG",
        "decimal" => "DECIMAL",
        "float" => "FLOAT",
        "double" => "DOUBLE",
        "boolean" => "BOOLEAN",
        "date" => "DATE",
        "dateTime" | "time" => "TIMESTAMP",
        _ => return String::new(),
    }
    .to_string()
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
