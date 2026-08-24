//! SkillManager：扫描 / 入库 / preamble 拼接 / 导入 / 卸载。
//!
//! 核心管理器（约 300 行业务代码，平台无关）。由 src-tauri setup hook 构造
//! 后注入 AppState（路径解析在 src-tauri/src/skill.rs，复用 pdfium 三层兜底）。
//!
//! 性能（SKILL-SYSTEM.md §3.6 已落地）：`discover_all()` 做磁盘 read_dir +
//! Govcraft SkillDirectory::load 解析。`build_preamble_section` 和
//! `active_skill_doc_paths` 都会调，每次发消息扫描两遍磁盘。现已加 60s TTL
//! 缓存（`discover_cache`），导入/卸载时 `invalidate_cache()` 失效。
//! 全局禁用/会话级激活是 DB 查询（非磁盘扫描），不影响缓存。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_skills::SkillDirectory;

use super::builtin::parse_disable_model_invocation;
use super::{
    resource_doc_path, scan_subdir_files, SkillError, SkillRecord, SkillSource, SkillSubdir,
};

/// 缓存有效期（秒）。SKILL-SYSTEM §3.6 建议 60s。
const DISCOVER_CACHE_TTL_SECS: u64 = 60;

/// Skill 管理器。
pub struct SkillManager {
    pub(super) memory: Arc<memory::Memory>,
    pub(super) builtin_dir: PathBuf,
    pub(super) user_dir: PathBuf,
    pub(super) external_dirs: Vec<PathBuf>,
    /// discover_all 结果缓存：(写入时刻, 已去重的 SkillRecord 列表)。
    /// None = 缓存空（首次或已 invalidate）。Mutex 临界区极短（仅取/存 Vec）。
    discover_cache: Mutex<Option<(Instant, Vec<SkillRecord>)>>,
}

impl SkillManager {
    /// 构造：传入 memory 句柄 + 各目录路径（由 src-tauri setup hook 解析后注入）。
    pub fn new(
        memory: Arc<memory::Memory>,
        builtin_dir: PathBuf,
        user_dir: PathBuf,
        external_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            memory,
            builtin_dir,
            user_dir,
            external_dirs,
            discover_cache: Mutex::new(None),
        }
    }

    /// 共享的 memory 句柄（供命令层访问 skill_repo CRUD）。
    pub fn memory(&self) -> &Arc<memory::Memory> {
        &self.memory
    }

    /// 扫描所有来源，返回去重后的 SkillRecord 列表。
    ///
    /// 去重规则：同名时 Builtin > Imported > ExternalReadOnly > Project
    /// （与 Claude Code "built-in 与 custom 冲突"一致，但 onto-studio 内置优先）。
    ///
    /// **性能**：走 60s TTL 缓存（`discover_cache`）。导入/卸载后调
    /// `invalidate_cache()` 立即失效。`build_preamble_section` 和
    /// `active_skill_doc_paths` 复用同一缓存，单次发消息不再扫描两遍磁盘。
    /// 缓存返回的是 clone（SkillRecord 含 String/PathBuf，量小可接受）。
    pub fn discover_all(&self) -> Vec<SkillRecord> {
        // 先查缓存（命中且未过期直接返回 clone）
        if let Ok(cache) = self.discover_cache.lock() {
            if let Some((written_at, records)) = cache.as_ref() {
                if written_at.elapsed().as_secs() < DISCOVER_CACHE_TTL_SECS {
                    return records.clone();
                }
            }
        }
        // miss 或过期：扫描磁盘
        let records = self.discover_all_uncached();
        // 填缓存（失败仅 warn，不影响返回）
        if let Ok(mut cache) = self.discover_cache.lock() {
            *cache = Some((Instant::now(), records.clone()));
        }
        records
    }

    /// 失效 discover 缓存。导入/卸载后调，确保下次 discover_all 重扫磁盘。
    /// 全局禁用/会话级激活是 DB 查询（非磁盘扫描），不需调此方法。
    pub fn invalidate_cache(&self) {
        if let Ok(mut cache) = self.discover_cache.lock() {
            *cache = None;
        }
    }

    /// 无缓存的扫描实现（discover_all 内部调，或测试直接调验证磁盘状态）。
    fn discover_all_uncached(&self) -> Vec<SkillRecord> {
        let total_start = std::time::Instant::now();
        let mut records: Vec<SkillRecord> = Vec::new();

        // 1. 内置（resource_dir/skills/）— 最高优先级，必须扫到
        self.scan_dir_timed(&self.builtin_dir, SkillSource::Builtin, &mut records);
        // 2. 用户导入（~/.onto-studio/skills/）— 重要
        self.scan_dir_timed(&self.user_dir, SkillSource::Imported, &mut records);

        // 3. 跨客户端只读（~/.pi/, ~/.claude/, ~/.agents/）— 互操作 bonus，
        //    绝不能因为某个 external 目录扫描卡死/出错而影响上面 builtin/user 的结果。
        //    每个目录独立容错：出错只 warn 跳过，不影响其它。
        for dir in &self.external_dirs {
            let ext_t = std::time::Instant::now();
            let before = records.len();
            self.scan_dir_timed(dir, SkillSource::ExternalReadOnly, &mut records);
            let elapsed = ext_t.elapsed();
            if elapsed.as_secs() >= 2 {
                tracing::warn!(
                    dir = %dir.display(),
                    elapsed_ms = elapsed.as_millis() as u64,
                    added = records.len() - before,
                    "external skills dir scan slow (>=2s), but continuing (fail-soft)"
                );
            }
        }

        // 去重：同名取高优先级 source
        let mut deduped: std::collections::HashMap<String, SkillRecord> =
            std::collections::HashMap::new();
        for r in records {
            match deduped.get(&r.name) {
                Some(existing) if existing.source.priority() >= r.source.priority() => {
                    // 已有更高优先级，跳过
                }
                _ => {
                    deduped.insert(r.name.clone(), r);
                }
            }
        }
        let out: Vec<_> = deduped.into_values().collect();
        tracing::info!(
            total_ms = total_start.elapsed().as_millis() as u64,
            found = out.len(),
            names = ?out.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
            "discover_all_uncached done"
        );
        out
    }

    /// scan_dir 带耗时日志（定位「某目录扫描极慢」问题，如 Windows 上
    /// `~/.pi/agent/skills` 含巨型 .venv 子目录）。
    fn scan_dir_timed(&self, dir: &Path, source: SkillSource, out: &mut Vec<SkillRecord>) {
        let t = std::time::Instant::now();
        let before = out.len();
        self.scan_dir(dir, source, out);
        let added = out.len() - before;
        let exists = dir.exists();
        tracing::debug!(
            dir = %dir.display(),
            source = ?source,
            exists,
            added,
            elapsed_ms = t.elapsed().as_millis() as u64,
            "scan_dir"
        );
        if t.elapsed().as_millis() > 500 {
            tracing::warn!(
                dir = %dir.display(),
                elapsed_ms = t.elapsed().as_millis() as u64,
                "scan_dir slow (>500ms), this may cause skills window loading"
            );
        }
    }

    /// 扫描单个目录下的所有 skill 子目录，追加到 out。
    /// 目录不存在则静默跳过（external_dirs 可能未配置）。
    fn scan_dir(&self, dir: &Path, source: SkillSource, out: &mut Vec<SkillRecord>) {
        let t = std::time::Instant::now();
        let Ok(entries) = std::fs::read_dir(dir) else {
            tracing::info!(dir = %dir.display(), source = ?source, "scan_dir: read_dir failed (dir not exist)");
            return; // 目录不存在则跳过
        };
        tracing::info!(dir = %dir.display(), source = ?source, "scan_dir: read_dir ok, iterating entries");
        for entry in entries.flatten() {
            let path = entry.path();
            // 用 symlink_metadata 而非 path.is_dir()：后者调 std::fs::metadata，
            // 在 Windows 上对 reparse point / junction / 网络符号链接会阻塞解析，
            // 可能卡死整个 scan_dir（进而卡死 list_skills）。symlink_metadata
            // 只读链接自身信息（不跟随），file_type().is_dir() 判断符号链接
            // 指向的目标类型（不实际解析），安全且快速。
            let is_dir = std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_dir())
                .unwrap_or(false);
            if !is_dir {
                continue;
            }
            let load_t = std::time::Instant::now();
            tracing::info!(subdir = %path.display(), "scan_dir: before SkillDirectory::load");
            match SkillDirectory::load(&path) {
                Ok(skill_dir) => {
                    let skill = skill_dir.skill();
                    tracing::info!(
                        subdir = %path.display(),
                        name = %skill.name().as_str(),
                        elapsed_ms = load_t.elapsed().as_millis() as u64,
                        "scan_dir: load ok"
                    );
                    let dmi = parse_disable_model_invocation(&path);
                    let allowed_tools = skill
                        .frontmatter()
                        .allowed_tools()
                        .map(|at| at.as_slice().to_vec());
                    let license = skill.frontmatter().license().map(String::from);
                    let compatibility = skill
                        .frontmatter()
                        .compatibility()
                        .map(|c| c.as_str().to_string());
                    out.push(SkillRecord {
                        name: skill.name().as_str().to_string(),
                        description: skill.description().as_str().to_string(),
                        source,
                        dir_path: path,
                        doc_id: None,
                        resource_doc_paths: None,
                        disable_model_invocation: dmi,
                        allowed_tools,
                        license,
                        compatibility,
                    });
                }
                Err(e) => {
                    tracing::info!(
                        subdir = %path.display(),
                        error = ?e,
                        elapsed_ms = load_t.elapsed().as_millis() as u64,
                        "scan_dir: load failed, skipping"
                    );
                }
            }
        }
        tracing::info!(
            dir = %dir.display(),
            elapsed_ms = t.elapsed().as_millis() as u64,
            "scan_dir: done"
        );
    }

    /// 把 skill body + references/assets/scripts 三个规范子目录下所有文本文件
    /// 入库 documents 表，返回 body 的 doc_id（资源文件的 doc path 收集进
    /// record.resource_doc_paths）。
    ///
    /// 幂等：body 去重键 = `skill://<name>`；资源去重键 =
    /// `skill://<name>/<dir>/<filename>`。已存在则 upsert（内容更新时覆盖）。
    /// 入库后自动建 FTS5 索引（异步，index_document spawn_blocking），search_documents
    /// 也能搜到 skill body + 全部资源内容——这是 bonus，不影响主流程。
    ///
    /// 资源入库动机：SKILL.md body 常以相对路径引用 `references/<file>.md`、
    /// `scripts/run.sh`、`assets/template.md`，但模型没有“按磁盘路径读文件”的工具——
    /// 必须把这些资源也入库为 doc，才能让 read_document / search_documents 触达
    /// 完整 skill 内容（否则模型只能在知识库里瞎找，找不到就放弃，是过去一轮
    /// Agent 断链的根因）。规范定义的三类子目录：
    ///   - references/：文档（契约、方法论）
    ///   - assets/：模板、资源
    ///   - scripts/：可执行脚本（onto-studio 无执行能力，但模型需读内容以理解命令）
    pub fn ensure_skill_documented(
        &self,
        record: &mut SkillRecord,
    ) -> Result<String, SkillError> {
        // 已入库则直接返回缓存 id（同一进程内多次调用）
        if let Some(id) = &record.doc_id {
            return Ok(id.clone());
        }
        let skill_dir =
            SkillDirectory::load(&record.dir_path).map_err(|e| SkillError::Load(e.to_string()))?;
        let body = skill_dir.skill().body();

        // skill body 放在 /Skills/<name>/ 下，避免平铺到知识库根目录干扰用户浏览
        let folder = format!("/Skills/{}", record.name);
        let row = memory::documents::DocumentRow {
            id: memory::new_document_id(),
            path: format!("skill://{}", record.name), // 去重键
            name: record.name.clone(),                // 展示名 = skill name
            format: "skill-md".to_string(),           // format 标记
            text: body.to_string(),
            char_count: body.chars().count() as u32,
            created_at: memory::now_ms(),
            folder_path: Some(folder),
            source_conv_id: None,     // 非会话上传来源
        };
        let doc_id = self.memory.upsert_document(row)?;

        // 资源入库：遍历 references/assets/scripts 三个规范子目录，
        // 扫描每个目录下的文本文件，逐个 upsert。
        // 失败仅 warn（单个资源入库失败不应阻断整个 skill 加载）。
        let mut res_paths: Vec<String> = Vec::new();
        for subdir in SkillSubdir::all() {
            for res_file in scan_subdir_files(&skill_dir, *subdir) {
                let filename = match res_file.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let text = match std::fs::read_to_string(&res_file) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::warn!(
                            skill = %record.name,
                            dir = subdir.dir_name(),
                            file = %res_file.display(),
                            error = %e,
                            "resource read failed, skipping"
                        );
                        continue;
                    }
                };
                let res_path = resource_doc_path(&record.name, *subdir, &filename);
                // 资源放到 /Skills/<name>/<subdir>/ 下（如 /Skills/ontology-modeling/references/）
                let res_folder = format!("/Skills/{}/{}", record.name, subdir.dir_name());
                let res_row = memory::documents::DocumentRow {
                    id: memory::new_document_id(),
                    path: res_path.clone(),
                    name: filename.clone(),
                    // 三类资源统一用 skill-resource 标记（与 skill-md 区分，
                    // 供 list_documents / 前端区分策略）。
                    format: "skill-resource".to_string(),
                    text: text.clone(),
                    char_count: text.chars().count() as u32,
                    created_at: memory::now_ms(),
                    folder_path: Some(res_folder),
                    source_conv_id: None,
                };
                match self.memory.upsert_document(res_row) {
                    Ok(res_id) => {
                        // 异步建 FTS5 索引（与 body 同路径）
                        let mem = self.memory.clone();
                        let db_path = self.memory.db_path().map(|p| p.to_path_buf());
                        if db_path.is_some() {
                            std::thread::spawn(move || {
                                if let Err(e) = mem.index_document(&res_id) {
                                    tracing::warn!(error = %e, "resource FTS5 index failed (non-blocking)");
                                }
                            });
                        } else {
                            let mem = self.memory.clone();
                            if let Err(e) = mem.index_document(&res_id) {
                                tracing::warn!(error = %e, "resource FTS5 index failed (in-memory)");
                            }
                        }
                        res_paths.push(res_path);
                    }
                    Err(e) => {
                        tracing::warn!(
                            skill = %record.name,
                            dir = subdir.dir_name(),
                            file = %filename,
                            error = %e,
                            "resource upsert failed, skipping"
                        );
                    }
                }
            }
        }
        if !res_paths.is_empty() {
            tracing::info!(
                skill = %record.name,
                resources = res_paths.len(),
                "skill resources indexed"
            );
        }

        // 异步建 body 的 FTS5 索引（不阻塞；内存库退化为同步）
        let mem = self.memory.clone();
        let id_clone = doc_id.clone();
        let db_path = self.memory.db_path().map(|p| p.to_path_buf());
        if db_path.is_some() {
            // 文件库：后台索引
            std::thread::spawn(move || {
                if let Err(e) = mem.index_document(&id_clone) {
                    tracing::warn!(error = %e, "skill FTS5 index failed (non-blocking)");
                }
            });
        } else {
            // 内存库：同步索引（测试场景可接受）
            if let Err(e) = self.memory.index_document(&doc_id) {
                tracing::warn!(error = %e, "skill FTS5 index failed (in-memory)");
            }
        }

        record.resource_doc_paths = Some(res_paths);
        record.doc_id = Some(doc_id.clone());
        Ok(doc_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str, description: &str, body: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
        )
        .unwrap();
    }

    /// 写一个带资源子目录的 skill。res 参数：(子目录名, 文件名, 内容) 三元组数组，
    /// 子目录名 ∈ {references, assets, scripts}（agentskills.io 规范）。
    fn write_skill_with_resources(
        dir: &Path,
        name: &str,
        description: &str,
        res: &[(&str, &str, &str)],
    ) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\nSee resources.\n"),
        )
        .unwrap();
        for (subdir, fname, content) in res {
            std::fs::create_dir_all(skill_dir.join(subdir)).unwrap();
            std::fs::write(skill_dir.join(subdir).join(fname), content).unwrap();
        }
    }

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onto-skill-mgr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_manager(builtin: PathBuf, user: PathBuf) -> SkillManager {
        let mem = std::sync::Arc::new(memory::Memory::open_in_memory().unwrap());
        SkillManager::new(mem, builtin, user, vec![])
    }

    #[test]
    fn discover_all_scans_builtin_and_user() {
        let root = temp_root();
        let builtin = root.join("builtin");
        let user = root.join("user");
        write_skill(&builtin, "a-skill", "A desc.", "# A\n");
        write_skill(&user, "b-skill", "B desc.", "# B\n");
        let mgr = make_manager(builtin, user);

        let all = mgr.discover_all();
        let names: Vec<_> = all.iter().map(|r| r.name.clone()).collect();
        assert!(names.contains(&"a-skill".to_string()));
        assert!(names.contains(&"b-skill".to_string()));
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn discover_all_dedup_builtin_over_user() {
        let root = temp_root();
        let builtin = root.join("builtin");
        let user = root.join("user");
        write_skill(&builtin, "dup-skill", "builtin version", "# B\n");
        write_skill(&user, "dup-skill", "user version", "# U\n");
        let mgr = make_manager(builtin, user);

        let all = mgr.discover_all();
        assert_eq!(all.len(), 1, "同名应去重");
        let r = &all[0];
        assert_eq!(r.source, SkillSource::Builtin, "内置优先");
        assert_eq!(r.description, "builtin version");
    }

    #[test]
    fn discover_all_missing_dirs_ok() {
        let mgr = make_manager(
            PathBuf::from("/nonexistent/builtin"),
            PathBuf::from("/nonexistent/user"),
        );
        let all = mgr.discover_all();
        assert!(all.is_empty());
    }

    #[test]
    fn ensure_skill_documented_upserts_and_caches_id() {
        let root = temp_root();
        let builtin = root.join("builtin");
        write_skill(&builtin, "doc-skill", "desc.", "# Body content here\n");
        let mgr = make_manager(builtin.clone(), PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        assert!(record.doc_id.is_none());

        let id1 = mgr.ensure_skill_documented(&mut record).unwrap();
        assert!(record.doc_id.is_some());
        // 第二次调用应复用缓存 id（不再 upsert）
        let id2 = mgr.ensure_skill_documented(&mut record).unwrap();
        assert_eq!(id1, id2);

        // skill:// path 应已入库
        let path = format!("skill://{}", record.name);
        let found = mgr.memory.document_id_by_path(&path).unwrap();
        assert_eq!(found, Some(id1));
    }

    #[test]
    fn ensure_skill_documented_body_stored() {
        let root = temp_root();
        let builtin = root.join("builtin");
        write_skill(&builtin, "body-skill", "desc.", "# My Skill\n\nInstructions here.\n");
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        let id = mgr.ensure_skill_documented(&mut record).unwrap();

        // read_document 应能取到正文（内存库同步索引）
        let read = mgr.memory.read_document(&id, None, None).unwrap();
        let (_, _name, _format, text, _count) = read.unwrap();
        assert!(text.contains("# My Skill"));
        assert!(text.contains("Instructions here."));
    }

    #[test]
    fn disable_model_invocation_parsed() {
        let root = temp_root();
        let builtin = root.join("builtin");
        // 写一个带 disable-model-invocation 的 skill
        let skill_dir = builtin.join("dmi-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: dmi-skill\ndescription: x\ndisable-model-invocation: true\n---\n# body\n",
        )
        .unwrap();
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let all = mgr.discover_all();
        let r = all.iter().find(|r| r.name == "dmi-skill").unwrap();
        assert!(r.disable_model_invocation);
    }

    #[test]
    fn discover_cache_hits_within_ttl() {
        let root = temp_root();
        let builtin = root.join("builtin");
        let user = root.join("user");
        write_skill(&builtin, "cache-skill", "v1", "# B\n");
        let mgr = make_manager(builtin.clone(), user.clone());

        // 首次扫描填缓存
        let all1 = mgr.discover_all();
        assert_eq!(all1.len(), 1);
        assert_eq!(all1[0].description, "v1");

        // 篡改磁盘（模拟外部变更）：若缓存生效，description 应仍是 v1
        std::fs::remove_dir_all(builtin.join("cache-skill")).unwrap();
        write_skill(&builtin, "cache-skill", "v2-tampered", "# B\n");
        let all2 = mgr.discover_all();
        assert_eq!(all2.len(), 1, "缓存命中应返回旧结果");
        assert_eq!(all2[0].description, "v1", "缓存未过期，应返回缓存值而非重扫");
    }

    #[test]
    fn invalidate_cache_forces_rescan() {
        let root = temp_root();
        let builtin = root.join("builtin");
        let user = root.join("user");
        write_skill(&builtin, "inv-skill", "v1", "# B\n");
        let mgr = make_manager(builtin.clone(), user.clone());

        // 首次填缓存
        let _ = mgr.discover_all();

        // 篡改 + invalidate
        std::fs::remove_dir_all(builtin.join("inv-skill")).unwrap();
        write_skill(&builtin, "inv-skill", "v2-refreshed", "# B\n");
        mgr.invalidate_cache();

        let all = mgr.discover_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].description, "v2-refreshed", "invalidate 后应重扫磁盘");
    }

    #[test]
    fn import_and_uninstall_invalidate_cache() {
        let root = temp_root();
        let src = root.join("src");
        let user = root.join("user");
        write_skill(&src, "imp-cache-skill", "desc", "# B\n");
        // user_dir 初始为空
        std::fs::create_dir_all(&user).unwrap();
        let mgr = make_manager(PathBuf::from("/nonexistent"), user.clone());

        // 首次扫描：user 为空，0 个
        assert!(mgr.discover_all().is_empty());

        // 导入后无需手动 invalidate（import_from_dir 内部已调）
        mgr.import_from_dir(&src.join("imp-cache-skill")).unwrap();
        let all = mgr.discover_all();
        assert_eq!(all.len(), 1, "导入后缓存应已失效，重扫看到新 skill");
        assert_eq!(all[0].name, "imp-cache-skill");

        // 卸载后同理
        mgr.uninstall("imp-cache-skill").unwrap();
        assert!(mgr.discover_all().is_empty(), "卸载后缓存应已失效，重扫为空");
    }

    #[test]
    fn ensure_skill_documented_indexes_references() {
        let root = temp_root();
        let builtin = root.join("builtin");
        write_skill_with_resources(
            &builtin,
            "ref-skill",
            "desc",
            &[
                ("references", "contract.md", "# Contract\n主键 pattern: ^[A-Z].*$\n"),
                ("references", "naming.md", "# Naming\nPascalCase / camelCase / snake_case\n"),
            ],
        );
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        assert!(record.doc_id.is_none());
        assert!(record.resource_doc_paths.is_none());

        let body_id = mgr.ensure_skill_documented(&mut record).unwrap();
        assert!(record.doc_id.is_some());
        // 两份 references 都应入库
        let res_paths = record.resource_doc_paths.as_ref().unwrap();
        assert_eq!(res_paths.len(), 2, "两份 references 都应入库");
        assert!(res_paths.contains(&"skill://ref-skill/references/contract.md".to_string()));
        assert!(res_paths.contains(&"skill://ref-skill/references/naming.md".to_string()));

        // body 入库
        let body_path = format!("skill://{}", record.name);
        assert_eq!(
            mgr.memory.document_id_by_path(&body_path).unwrap(),
            Some(body_id)
        );
        // references 入库（path 精确查询）
        for rp in res_paths {
            assert!(
                mgr.memory.document_id_by_path(rp).unwrap().is_some(),
                "reference {rp} 应入库"
            );
        }
    }

    #[test]
    fn ensure_skill_documented_indexes_all_three_subdirs() {
        // 验证 agentskills.io 规范的三类子目录（references/assets/scripts）都入库
        let root = temp_root();
        let builtin = root.join("builtin");
        write_skill_with_resources(
            &builtin,
            "full-skill",
            "desc",
            &[
                ("references", "schema.md", "# Schema\nstorage_type ∈ {MANAGED, VIRTUAL}\n"),
                ("assets", "template.md", "# Template\n占位符 {{name}}\n"),
                ("scripts", "run-check.sh", "#!/bin/bash\necho check\n"),
            ],
        );
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        mgr.ensure_skill_documented(&mut record).unwrap();

        let res_paths = record.resource_doc_paths.as_ref().unwrap();
        assert_eq!(res_paths.len(), 3, "三个子目录各一份，共 3 份资源");
        assert!(res_paths.contains(&"skill://full-skill/references/schema.md".to_string()));
        assert!(res_paths.contains(&"skill://full-skill/assets/template.md".to_string()));
        assert!(res_paths.contains(&"skill://full-skill/scripts/run-check.sh".to_string()));

        // scripts 内容应可读（onto-studio 无执行能力，但模型可读内容理解命令）
        let script_path = "skill://full-skill/scripts/run-check.sh";
        let script_id = mgr.memory.document_id_by_path(script_path).unwrap().unwrap();
        let read = mgr.memory.read_document(&script_id, None, None).unwrap();
        let (_path, _name, _format, text, _count) = read.unwrap();
        assert!(text.contains("#!/bin/bash"));
    }

    #[test]
    fn ensure_skill_documented_references_readable_via_read_document() {
        let root = temp_root();
        let builtin = root.join("builtin");
        write_skill_with_resources(
            &builtin,
            "readable-skill",
            "desc",
            &[("references", "schema.md", "# Schema\nstorage_type ∈ {MANAGED, VIRTUAL}\n")],
        );
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        mgr.ensure_skill_documented(&mut record).unwrap();

        let ref_path = "skill://readable-skill/references/schema.md";
        let ref_id = mgr.memory.document_id_by_path(ref_path).unwrap().unwrap();
        let read = mgr.memory.read_document(&ref_id, None, None).unwrap();
        let (_path, name, _format, text, _count) = read.unwrap();
        assert_eq!(name, "schema.md");
        assert!(text.contains("storage_type ∈ {MANAGED, VIRTUAL}"));
    }

    #[test]
    fn ensure_skill_documented_ignores_binary_resources() {
        let root = temp_root();
        let builtin = root.join("builtin");
        let skill_dir = builtin.join("bin-skill");
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: bin-skill\ndescription: x\n---\n# B\n",
        )
        .unwrap();
        // 二进制图片 + 文本 md 混放（references 目录）
        std::fs::write(skill_dir.join("references").join("diag.png"), b"\x89PNG\r\n").unwrap();
        std::fs::write(skill_dir.join("references").join("ok.md"), "# OK\n").unwrap();
        // assets 目录也放一个二进制 + 一个文本
        std::fs::create_dir_all(skill_dir.join("assets")).unwrap();
        std::fs::write(skill_dir.join("assets").join("logo.jpg"), b"\xFF\xD8\xFF\xE0").unwrap();
        std::fs::write(skill_dir.join("assets").join("tpl.md"), "# TPL\n").unwrap();
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        mgr.ensure_skill_documented(&mut record).unwrap();

        let res_paths = record.resource_doc_paths.as_ref().unwrap();
        assert_eq!(res_paths.len(), 2, "二进制文件应跳过，只入库文本资源");
        assert!(res_paths.contains(&"skill://bin-skill/references/ok.md".to_string()));
        assert!(res_paths.contains(&"skill://bin-skill/assets/tpl.md".to_string()));
        assert!(!res_paths.iter().any(|p| p.contains("diag.png")), "png 应跳过");
        assert!(!res_paths.iter().any(|p| p.contains("logo.jpg")), "jpg 应跳过");
    }

    #[test]
    fn ensure_skill_documented_no_resources_dirs_ok() {
        let root = temp_root();
        let builtin = root.join("builtin");
        write_skill(&builtin, "bare-skill", "desc", "# Bare\n");
        let mgr = make_manager(builtin, PathBuf::from("/nonexistent"));

        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        mgr.ensure_skill_documented(&mut record).unwrap();
        // 无任何资源子目录 → 空列表（不是 None）
        assert_eq!(
            record.resource_doc_paths.as_ref().unwrap().len(),
            0,
            "无资源子目录应返回空 Vec"
        );
        // body 仍正常入库
        assert!(record.doc_id.is_some());
    }
}
