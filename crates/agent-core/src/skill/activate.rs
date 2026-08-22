//! 会话激活 + disable 三层判断 + active_skill_doc_paths。
//!
//! 三层 disable 语义（SKILL-SYSTEM.md §0 速览）：
//!   层次 1（作者声明）：frontmatter disable-model-invocation: true
//!     → 不进自动 preamble，只能 @skillName 显式调（只读属性）
//!   层次 2（全局偏好）：disabled_skills 表
//!     → 跨所有会话不进 preamble
//!   层次 3（会话级）  ：conversation_skills 表的 enabled 列
//!     → Builtin/External 默认进，Imported 默认不进，会话级 enabled 可覆盖
//!
//! `build_preamble_section`（进 preamble 的 skill）与 `active_skill_doc_paths`
//! （进 doc_paths_set 的 skill，供 read_document）共用判断逻辑，但有细微差别：
//!   - disable-model-invocation 的 skill 不进 preamble，但 @激活后也要能读
//!   - 全局禁用（层次 2）的 skill 既不进 preamble 也不可读

use std::collections::HashSet;

use super::{SkillError, SkillManager, SkillRecord, SkillSource};

impl SkillManager {
    /// 解析本会话应进 preamble 的 skill 列表（已入库 documents）。
    ///
    /// 返回的 SkillRecord 已确保 doc_id 存在（preamble 的 location 提示模型
    /// 调 read_document，故必须先入库）。
    pub(super) fn resolve_preamble_skills(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SkillRecord>, SkillError> {
        let all = self.discover_all();
        let disabled: HashSet<String> = self
            .memory
            .list_disabled_skills()?
            .into_iter()
            .collect();
        let conv_skills = self.memory.list_conversation_skills(conversation_id)?;

        let mut out = Vec::new();
        for mut record in all {
            // 层次 1：作者声明 disable-model-invocation
            if record.disable_model_invocation {
                continue;
            }
            // 层次 2：全局禁用
            if disabled.contains(&record.name) {
                continue;
            }
            // 层次 3：会话级
            let conv_entry = conv_skills.iter().find(|cs| cs.skill_name == record.name);
            match record.source {
                SkillSource::Builtin | SkillSource::ExternalReadOnly => {
                    // 默认进，除非会话级显式 enabled == false
                    if let Some(cs) = conv_entry {
                        if !cs.enabled {
                            continue;
                        }
                    }
                }
                SkillSource::Imported => {
                    // 默认不进，除非会话级显式 enabled == true
                    if !conv_entry.map(|cs| cs.enabled).unwrap_or(false) {
                        continue;
                    }
                }
                SkillSource::Project => {
                    // 二期，暂不进 preamble
                    continue;
                }
            }
            // 入库 documents（确保 doc_id 存在）
            if record.doc_id.is_none() {
                self.ensure_skill_documented(&mut record)?;
            }
            out.push(record);
        }
        Ok(out)
    }

    /// 返回本会话应加入 doc_paths_set 的 skill doc path（`skill://<name>`）。
    ///
    /// 包括：进 preamble 的 skill + @skillName 显式激活的 skill
    /// （disable-model-invocation 的 skill 不进 preamble，但 @激活后也要能读）。
    /// 全局禁用（层次 2）的 skill 不可读。
    pub fn active_skill_doc_paths(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<String>, SkillError> {
        let all = self.discover_all();
        let disabled: HashSet<String> = self
            .memory
            .list_disabled_skills()?
            .into_iter()
            .collect();
        let conv_skills = self.memory.list_conversation_skills(conversation_id)?;

        let mut paths = Vec::new();
        for mut record in all {
            // 全局禁用的不提供（层次 2 优先，高于一切）
            if disabled.contains(&record.name) {
                continue;
            }

            let conv_entry = conv_skills.iter().find(|cs| cs.skill_name == record.name);
            let should_include = if record.disable_model_invocation {
                // 层次 1：只能 @显式激活（会话级 enabled == true）
                conv_entry.map(|cs| cs.enabled).unwrap_or(false)
            } else {
                match record.source {
                    SkillSource::Builtin | SkillSource::ExternalReadOnly => {
                        // 默认 true（进 preamble 即可读）
                        conv_entry.map(|cs| cs.enabled).unwrap_or(true)
                    }
                    SkillSource::Imported => {
                        // 默认 false（不进 preamble 也不可读，除非会话级激活）
                        conv_entry.map(|cs| cs.enabled).unwrap_or(false)
                    }
                    SkillSource::Project => false,
                }
            };

            if should_include {
                if record.doc_id.is_none() {
                    self.ensure_skill_documented(&mut record)?;
                }
                paths.push(format!("skill://{}", record.name));
                // 资源 doc path 随父 skill 一起进 doc_paths_set，
                // 供 read_document / search_documents 触达完整 skill 内容。
                if let Some(res_paths) = &record.resource_doc_paths {
                    paths.extend(res_paths.iter().cloned());
                }
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn write_skill_full(
        dir: &Path,
        name: &str,
        description: &str,
        body: &str,
        dmi: bool,
    ) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let fm = if dmi {
            format!(
                "---\nname: {name}\ndescription: {description}\ndisable-model-invocation: true\n---\n{body}"
            )
        } else {
            format!("---\nname: {name}\ndescription: {description}\n---\n{body}")
        };
        std::fs::write(skill_dir.join("SKILL.md"), fm).unwrap();
    }

    fn temp_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("onto-skill-act-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_manager(builtin: std::path::PathBuf, user: std::path::PathBuf) -> SkillManager {
        let mem = std::sync::Arc::new(memory::Memory::open_in_memory().unwrap());
        SkillManager::new(mem, builtin, user, vec![])
    }

    /// 建一个会话并返回 id（conversation_skills 表有外键到 conversations）。
    fn make_conv(mgr: &SkillManager) -> String {
        mgr.memory.create_conversation(None).unwrap().id
    }

    #[test]
    fn builtin_in_preamble_by_default() {
        let root = temp_root();
        write_skill_full(&root.join("builtin"), "b1", "desc", "# B1\n", false);
        let mgr = make_manager(root.join("builtin"), PathBuf::from("/none"));

        let preamble = mgr.build_preamble_section("conv-1").unwrap();
        assert!(preamble.contains("<name>b1</name>"));
        assert!(preamble.contains("<available_skills>"));
    }

    #[test]
    fn imported_not_in_preamble_unless_activated() {
        let root = temp_root();
        write_skill_full(&root.join("user"), "imp1", "desc", "# I1\n", false);
        let mgr = make_manager(PathBuf::from("/none"), root.join("user"));
        let conv = make_conv(&mgr);

        // 默认不进 preamble
        let preamble = mgr.build_preamble_section(&conv).unwrap();
        assert!(preamble.is_empty(), "imported 默认不进 preamble");

        // 会话级激活后进 preamble
        mgr.memory
            .set_conversation_skill_enabled(&conv, "imp1", "imported", true)
            .unwrap();
        let preamble = mgr.build_preamble_section(&conv).unwrap();
        assert!(preamble.contains("<name>imp1</name>"));
    }

    #[test]
    fn globally_disabled_excluded() {
        let root = temp_root();
        write_skill_full(&root.join("builtin"), "g1", "desc", "# G1\n", false);
        let mgr = make_manager(root.join("builtin"), PathBuf::from("/none"));

        mgr.memory.set_skill_globally_disabled("g1", true).unwrap();
        let preamble = mgr.build_preamble_section("conv-1").unwrap();
        assert!(preamble.is_empty(), "全局禁用不进 preamble");
    }

    #[test]
    fn dmi_skill_not_in_preamble_but_readable_when_activated() {
        let root = temp_root();
        write_skill_full(&root.join("builtin"), "d1", "desc", "# D1\n", true);
        let mgr = make_manager(root.join("builtin"), PathBuf::from("/none"));
        let conv = make_conv(&mgr);

        // 不进 preamble（层次 1）
        let preamble = mgr.build_preamble_section(&conv).unwrap();
        assert!(preamble.is_empty(), "disable-model-invocation 不进 preamble");

        // 默认也不可读（未激活）
        let paths = mgr.active_skill_doc_paths(&conv).unwrap();
        assert!(paths.is_empty(), "dmi skill 默认不可读");

        // 会话级 @激活后可读
        mgr.memory
            .set_conversation_skill_enabled(&conv, "d1", "builtin", true)
            .unwrap();
        let paths = mgr.active_skill_doc_paths(&conv).unwrap();
        assert!(paths.contains(&"skill://d1".to_string()));
    }

    #[test]
    fn conversation_disable_overrides_builtin_default() {
        let root = temp_root();
        write_skill_full(&root.join("builtin"), "c1", "desc", "# C1\n", false);
        let mgr = make_manager(root.join("builtin"), PathBuf::from("/none"));
        let conv = make_conv(&mgr);

        // 默认进
        assert!(!mgr.build_preamble_section(&conv).unwrap().is_empty());

        // 会话级禁用
        mgr.memory
            .set_conversation_skill_enabled(&conv, "c1", "builtin", false)
            .unwrap();
        assert!(mgr.build_preamble_section(&conv).unwrap().is_empty());

        // 该 skill 不可读
        let paths = mgr.active_skill_doc_paths(&conv).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn active_skill_doc_paths_includes_preamble_and_activated() {
        let root = temp_root();
        write_skill_full(&root.join("builtin"), "p1", "desc", "# P1\n", false);
        write_skill_full(&root.join("user"), "u1", "desc", "# U1\n", false);
        write_skill_full(&root.join("builtin"), "d1", "desc", "# D1\n", true);
        let mgr = make_manager(root.join("builtin"), root.join("user"));
        let conv = make_conv(&mgr);

        // 激活 imported u1 和 dmi d1
        mgr.memory
            .set_conversation_skill_enabled(&conv, "u1", "imported", true)
            .unwrap();
        mgr.memory
            .set_conversation_skill_enabled(&conv, "d1", "builtin", true)
            .unwrap();

        let paths = mgr.active_skill_doc_paths(&conv).unwrap();
        // p1（builtin 默认进）、u1（会话激活）、d1（dmi 会话激活）都应可读
        assert!(paths.contains(&"skill://p1".to_string()));
        assert!(paths.contains(&"skill://u1".to_string()));
        assert!(paths.contains(&"skill://d1".to_string()));
    }

    #[test]
    fn active_skill_doc_paths_includes_all_resource_subdirs_with_parent() {
        // references/assets/scripts 三类资源 doc path 都应随父 skill 一起进 doc_paths_set（A1 方案核心）
        let root = temp_root();
        let builtin = root.join("builtin");
        let skill_dir = builtin.join("resort");
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::create_dir_all(skill_dir.join("assets")).unwrap();
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: resort\ndescription: x\n---\n# R\nSee resources.\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("references").join("a.md"), "# A\n").unwrap();
        std::fs::write(skill_dir.join("assets").join("tpl.md"), "# TPL\n").unwrap();
        std::fs::write(skill_dir.join("scripts").join("run.sh"), "#!/bin/bash\n").unwrap();
        let mgr = make_manager(builtin, PathBuf::from("/none"));
        let conv = make_conv(&mgr);

        let paths = mgr.active_skill_doc_paths(&conv).unwrap();
        // body + 三类子目录各一份都在激活集
        assert!(paths.contains(&"skill://resort".to_string()));
        assert!(paths.contains(&"skill://resort/references/a.md".to_string()));
        assert!(paths.contains(&"skill://resort/assets/tpl.md".to_string()));
        assert!(paths.contains(&"skill://resort/scripts/run.sh".to_string()));
    }

    #[test]
    fn disabled_skill_excludes_all_its_resources() {
        // 全局禁用的 skill，其三类资源都不进 doc_paths_set（层次 2 优先）
        let root = temp_root();
        let builtin = root.join("builtin");
        let skill_dir = builtin.join("disabledres");
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: disabledres\ndescription: x\n---\n# D\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("references").join("x.md"), "# X\n").unwrap();
        std::fs::write(skill_dir.join("scripts").join("s.sh"), "#!/bin/bash\n").unwrap();
        let mgr = make_manager(builtin, PathBuf::from("/none"));

        mgr.memory.set_skill_globally_disabled("disabledres", true).unwrap();
        let conv = make_conv(&mgr);
        let paths = mgr.active_skill_doc_paths(&conv).unwrap();
        assert!(!paths.contains(&"skill://disabledres".to_string()));
        assert!(!paths.contains(&"skill://disabledres/references/x.md".to_string()));
        assert!(!paths.contains(&"skill://disabledres/scripts/s.sh".to_string()));
    }
}
