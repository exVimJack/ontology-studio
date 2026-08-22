//! 导入操作：本地目录复制 / zip 解压，复用 ingest::security 防 zip 炸弹。
//!
//! 复用基础设施：
//!   - `ingest::security::{check_size, ArchiveBudget}`：预校验压缩包大小 + 累计展开字节
//!   - `agent_skills::SkillDirectory::load`：校验源目录是合法 skill（SKILL.md + name 匹配目录名）
//!   - `memory::Memory::remove_skill_records`：卸载时清 DB 记录
//!
//! ingest 没有现成的 `extract_zip_safe`，只有防护原语——解压循环在此自建。
//! zip crate = `9.0.0-pre2`（ingest 已依赖，agent-core 在 Cargo.toml 加同版本）。

use std::fs;
use std::path::{Path, PathBuf};

use agent_skills::SkillDirectory;

use super::{SkillError, SkillManager};

impl SkillManager {
    /// 导入本地 skill 目录：复制到 `~/.onto-studio/skills/<name>/`。
    ///
    /// 先用 Govcraft 校验合法性（SKILL.md 存在 + name 匹配目录名），再递归复制
    /// （含 scripts/references/assets 子目录）。已存在则报错（用户需先卸载）。
    pub fn import_from_dir(&self, src_dir: &Path) -> Result<String, SkillError> {
        // 1. 用 Govcraft 校验源目录是合法 skill
        let skill_dir = SkillDirectory::load(src_dir)
            .map_err(|e| SkillError::InvalidSkill(e.to_string()))?;
        let name = skill_dir.skill().name().as_str().to_string();

        // 2. 目标路径：user_dir/<name>/
        let dest = self.user_dir.join(&name);
        if dest.exists() {
            return Err(SkillError::AlreadyExists(name));
        }

        // 3. 递归复制
        copy_dir_recursive(src_dir, &dest)?;

        // 导入改变磁盘状态，失效 discover 缓存（下次 discover_all 重扫）
        self.invalidate_cache();

        tracing::info!(skill = %name, dest = %dest.display(), "skill imported from dir");
        Ok(name)
    }

    /// 导入 zip：解压到临时目录 → 校验 → 复制到 user_dir。
    ///
    /// 复用 ingest::security 的 zip 炸弹防护：
    ///   - `check_size`：预校验压缩包大小（MAX_FILE_BYTES = 200MB）
    ///   - `ArchiveBudget`：累计展开字节（MAX_ARCHIVE_EXPANDED_BYTES = 500MB）
    /// 解压逻辑自建（ingest 无现成 extract_zip_safe）。
    pub fn import_from_zip(&self, zip_path: &Path) -> Result<String, SkillError> {
        use ingest::security::{check_size, ArchiveBudget};
        use std::fs::File;
        use zip::ZipArchive;

        // 1. 防炸弹：校验压缩包大小
        check_size(zip_path).map_err(|e| SkillError::ZipBomb(e.to_string()))?;

        // 2. 解压到临时目录，用 ArchiveBudget 累计展开字节防炸弹
        let temp = std::env::temp_dir().join(format!("onto-skill-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).map_err(|e| SkillError::Io(e.to_string()))?;
        let file = File::open(zip_path).map_err(|e| SkillError::Io(e.to_string()))?;
        let mut archive = ZipArchive::new(file).map_err(|e| SkillError::Zip(e.to_string()))?;
        let mut budget = ArchiveBudget::new();
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| SkillError::Zip(e.to_string()))?;
            // mangled_name 防路径穿越（zip slip 攻击）。返回 Result，需处理。
            let mangled = entry
                .mangled_name()
                .map_err(|e| SkillError::Zip(e.to_string()))?;
            let outpath = temp.join(mangled);
            if entry.is_dir() {
                std::fs::create_dir_all(&outpath).map_err(|e| SkillError::Io(e.to_string()))?;
            } else {
                std::fs::create_dir_all(outpath.parent().unwrap_or(&temp))
                    .map_err(|e| SkillError::Io(e.to_string()))?;
                let mut outfile =
                    File::create(&outpath).map_err(|e| SkillError::Io(e.to_string()))?;
                let bytes = std::io::copy(&mut entry, &mut outfile)
                    .map_err(|e| SkillError::Io(e.to_string()))?;
                // account 接受 u64（单条目字节），与 io::copy 返回 u64 一致
                budget.account(bytes).map_err(|e| SkillError::ZipBomb(e.to_string()))?;
            }
        }
        // 释放 archive 句柄（file 也随之释放）
        drop(archive);

        // 3. zip 可能是 "skill-name/SKILL.md" 或直接 "SKILL.md"（flat）。
        //    flat 结构解压到随机名临时目录，Govcraft 校验目录名==skill name 会失败，
        //    需先读出 skill name，把内容移到 temp/<name>/ 子目录再 import_from_dir。
        let skill_root = if temp.join("SKILL.md").exists() {
            // flat：读 frontmatter 拿 name，移到 temp/<name>/
            let content = std::fs::read_to_string(temp.join("SKILL.md"))
                .map_err(|e| SkillError::Io(e.to_string()))?;
            let name = extract_skill_name_from_content(&content)
                .ok_or(SkillError::NoSkillInZip)?;
            let target = temp.join(&name);
            if target.exists() {
                return Err(SkillError::AlreadyExists(name));
            }
            std::fs::create_dir_all(&target).map_err(|e| SkillError::Io(e.to_string()))?;
            // 把 temp 下所有文件/目录移到 target/（除 target 本身）
            for entry in std::fs::read_dir(&temp).map_err(|e| SkillError::Io(e.to_string()))? {
                let entry = entry.map_err(|e| SkillError::Io(e.to_string()))?;
                let p = entry.path();
                if p == target {
                    continue;
                }
                let dest = target.join(entry.file_name());
                std::fs::rename(&p, &dest).map_err(|e| SkillError::Io(e.to_string()))?;
            }
            target
        } else {
            // nested：找含 SKILL.md 的子目录
            find_skill_subdir(&temp).ok_or(SkillError::NoSkillInZip)?
        };

        // 4. 复用 import_from_dir（含校验 + 复制 + invalidate_cache）
        let name = self.import_from_dir(&skill_root)?;

        // 5. 清理临时目录
        let _ = fs::remove_dir_all(&temp);
        tracing::info!(skill = %name, "skill imported from zip");
        Ok(name)
    }

    /// 卸载导入的 skill：删除 `~/.onto-studio/skills/<name>/`。
    ///
    /// 同时清理 disabled_skills + conversation_skills 中的记录，
    /// 以及 documents 表里的 skill body（`skill://<name>`）+ 所有 references
    /// （`skill://<name>/references/*`）。内置/external-readonly 不可卸载
    /// （调用方应拒绝，这里只在 user_dir 不存在时报错）。
    pub fn uninstall(&self, skill_name: &str) -> Result<(), SkillError> {
        let dir = self.user_dir.join(skill_name);
        if !dir.exists() {
            return Err(SkillError::NotImported(skill_name.to_string()));
        }
        // 删除前先 discover 拿到 resource_doc_paths（避免删目录后扫不到）。
        // discover_all 走缓存；此处要拿最新的磁盘状态，先 invalidate。
        self.invalidate_cache();
        let res_paths: Vec<String> = self
            .discover_all()
            .into_iter()
            .find(|r| r.name == skill_name)
            .and_then(|mut r| {
                // ensure 入库（拿 resource_doc_paths）；失败忽略，下面仍会删 body。
                let _ = self.ensure_skill_documented(&mut r);
                r.resource_doc_paths
            })
            .unwrap_or_default();

        fs::remove_dir_all(&dir).map_err(|e| SkillError::Io(e.to_string()))?;
        // 清理 DB 记录（disabled_skills + conversation_skills）
        self.memory.remove_skill_records(skill_name)?;
        // 清理 documents 表里的 skill body（path = skill://<name>）
        let _ = self
            .memory
            .delete_document_by_path(&format!("skill://{skill_name}"));
        // 清理资源文档（逐个精确删，delete_document_by_path 不支持前缀）
        for rp in &res_paths {
            let _ = self.memory.delete_document_by_path(rp);
        }
        // 卸载改变磁盘状态，失效 discover 缓存
        self.invalidate_cache();
        tracing::info!(skill = %skill_name, resources_removed = res_paths.len(), "skill uninstalled");
        Ok(())
    }
}

/// 递归复制目录（std 没有提供，手写）。
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), SkillError> {
    fs::create_dir_all(dest).map_err(|e| SkillError::Io(e.to_string()))?;
    for entry in fs::read_dir(src).map_err(|e| SkillError::Io(e.to_string()))? {
        let entry = entry.map_err(|e| SkillError::Io(e.to_string()))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| SkillError::Io(e.to_string()))?;
        }
    }
    Ok(())
}

/// 从 SKILL.md 内容提取 frontmatter 的 name 字段（极简解析，避免重复 Govcraft 内部逻辑）。
/// 用于 zip flat 导入时确定目标子目录名。
fn extract_skill_name_from_content(content: &str) -> Option<String> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    let frontmatter = &rest[..end];
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == "name" {
                let name = v.trim();
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

/// 在解压后的临时目录中查找含 SKILL.md 的子目录。
///
/// zip 通常是 `skill-name/SKILL.md` 结构。遍历一级子目录找含 SKILL.md 的。
fn find_skill_subdir(temp: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(temp).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").exists() {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_skill(dir: &Path, name: &str, description: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let mut f = fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        write!(
            f,
            "---\nname: {name}\ndescription: {description}\n---\n# {name}\n"
        )
        .unwrap();
    }

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onto-skill-imp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_manager(user_dir: &Path) -> SkillManager {
        let mem = std::sync::Arc::new(memory::Memory::open_in_memory().unwrap());
        SkillManager::new(mem, PathBuf::from("/none"), user_dir.to_path_buf(), vec![])
    }

    #[test]
    fn import_from_dir_copies() {
        let root = temp_root();
        let src = root.join("src");
        let user = root.join("user");
        write_skill(&src, "imp-dir-skill", "desc");
        let mgr = make_manager(&user);

        let name = mgr.import_from_dir(&src.join("imp-dir-skill")).unwrap();
        assert_eq!(name, "imp-dir-skill");
        // 复制到 user_dir
        assert!(user.join("imp-dir-skill").join("SKILL.md").exists());
    }

    #[test]
    fn import_from_dir_rejects_existing() {
        let root = temp_root();
        let src = root.join("src");
        let user = root.join("user");
        write_skill(&src, "dup-skill", "desc");
        // 先放一个同名到 user_dir
        write_skill(&user, "dup-skill", "existing");
        let mgr = make_manager(&user);

        let err = mgr.import_from_dir(&src.join("dup-skill")).unwrap_err();
        assert!(matches!(err, SkillError::AlreadyExists(_)));
    }

    #[test]
    fn import_from_dir_rejects_invalid() {
        let root = temp_root();
        let src = root.join("src");
        let user = root.join("user");
        // 目录无 SKILL.md
        fs::create_dir_all(src.join("empty")).unwrap();
        let mgr = make_manager(&user);

        let err = mgr.import_from_dir(&src.join("empty")).unwrap_err();
        assert!(matches!(err, SkillError::InvalidSkill(_)));
    }

    #[test]
    fn import_from_zip_flat() {
        let root = temp_root();
        let user = root.join("user");
        let zip_path = root.join("flat.zip");

        // 构造 zip：直接含 SKILL.md（无外层目录）
        let tmp_extract = root.join("zip_src_flat");
        fs::create_dir_all(&tmp_extract).unwrap();
        fs::write(
            tmp_extract.join("SKILL.md"),
            "---\nname: zip-flat-skill\ndescription: x\n---\n# body\n",
        )
        .unwrap();
        create_zip(&zip_path, &tmp_extract, true);

        let mgr = make_manager(&user);
        let name = mgr.import_from_zip(&zip_path).unwrap();
        assert_eq!(name, "zip-flat-skill");
        assert!(user.join("zip-flat-skill").join("SKILL.md").exists());
    }

    #[test]
    fn import_from_zip_nested() {
        let root = temp_root();
        let user = root.join("user");
        let zip_path = root.join("nested.zip");

        // 构造 zip：含 zip-nested-skill/SKILL.md
        let tmp_extract = root.join("zip_src_nested");
        let skill_dir = tmp_extract.join("zip-nested-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: zip-nested-skill\ndescription: x\n---\n# body\n",
        )
        .unwrap();
        create_zip(&zip_path, &skill_dir, false);

        let mgr = make_manager(&user);
        let name = mgr.import_from_zip(&zip_path).unwrap();
        assert_eq!(name, "zip-nested-skill");
        assert!(user.join("zip-nested-skill").join("SKILL.md").exists());
    }

    #[test]
    fn uninstall_removes_dir_and_records() {
        let root = temp_root();
        let user = root.join("user");
        let builtin = root.join("builtin");
        write_skill(&user, "rm-skill", "desc");
        write_skill(&builtin, "keep-skill", "desc");
        let mgr = SkillManager::new(
            std::sync::Arc::new(memory::Memory::open_in_memory().unwrap()),
            builtin,
            user.clone(),
            vec![],
        );

        // 入库 + 设全局禁用
        let mut all = mgr.discover_all();
        let rm = all.iter_mut().find(|r| r.name == "rm-skill").unwrap();
        mgr.ensure_skill_documented(rm).unwrap();
        mgr.memory.set_skill_globally_disabled("rm-skill", true).unwrap();

        mgr.uninstall("rm-skill").unwrap();
        assert!(!user.join("rm-skill").exists(), "目录应删除");
        // 全局禁用记录应清除
        assert!(!mgr.memory.is_skill_globally_disabled("rm-skill").unwrap());
        // documents 表的 skill 全文应删除
        assert!(mgr
            .memory
            .document_id_by_path("skill://rm-skill")
            .unwrap()
            .is_none());
        // keep-skill 不受影响
        assert!(user.join("rm-skill").exists() == false);
    }

    #[test]
    fn uninstall_nonexistent_errors() {
        let root = temp_root();
        let user = root.join("user");
        let mgr = make_manager(&user);
        let err = mgr.uninstall("nope").unwrap_err();
        assert!(matches!(err, SkillError::NotImported(_)));
    }

    #[test]
    fn uninstall_cleans_resources_documents() {
        // 卸载 skill 时，三类资源文档都应从 documents 表清理（不只删 body）
        let root = temp_root();
        let src = root.join("src");
        let user = root.join("user");
        let skill_dir = src.join("unres-skill");
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::create_dir_all(skill_dir.join("scripts")).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: unres-skill\ndescription: x\n---\n# B\n",
        )
        .unwrap();
        std::fs::write(skill_dir.join("references").join("c.md"), "# C\n").unwrap();
        std::fs::write(skill_dir.join("references").join("d.md"), "# D\n").unwrap();
        std::fs::write(skill_dir.join("scripts").join("s.sh"), "#!/bin/bash\n").unwrap();
        let mgr = make_manager(&user);

        // 导入 + 入库资源
        mgr.import_from_dir(&skill_dir).unwrap();
        let mut all = mgr.discover_all();
        let mut record = all.pop().unwrap();
        mgr.ensure_skill_documented(&mut record).unwrap();
        let res_paths = record.resource_doc_paths.clone().unwrap();
        assert_eq!(res_paths.len(), 3);
        // 入库后应能查到
        for rp in &res_paths {
            assert!(mgr.memory.document_id_by_path(rp).unwrap().is_some());
        }

        // 卸载
        mgr.uninstall("unres-skill").unwrap();
        // body + 三类资源都应已删除
        assert!(mgr
            .memory
            .document_id_by_path("skill://unres-skill")
            .unwrap()
            .is_none());
        for rp in &res_paths {
            assert!(
                mgr.memory.document_id_by_path(rp).unwrap().is_none(),
                "卸载后资源 {rp} 应已删除"
            );
        }
    }

    /// 构造 zip。flat=true 时直接打包目录内文件（无外层目录），
    /// flat=false 时打包目录本身（外层目录名保留）。
    fn create_zip(zip_path: &Path, src_dir: &Path, flat: bool) {
        let file = fs::File::create(zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();

        if flat {
            // 直接把 src_dir 内文件加到 zip 根
            for entry in fs::read_dir(src_dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_file() {
                    let name = entry.file_name().to_str().unwrap().to_string();
                    zip.start_file(name, opts).unwrap();
                    let mut f = fs::File::open(&path).unwrap();
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
                    std::io::Write::write_all(&mut zip, &buf).unwrap();
                }
            }
        } else {
            // 打包目录本身：遍历递归，路径前缀 = 目录名
            let dir_name = src_dir.file_name().unwrap().to_str().unwrap();
            add_dir_to_zip(&mut zip, src_dir, dir_name, &opts);
        }
        zip.finish().unwrap();
    }

    fn add_dir_to_zip(
        zip: &mut zip::ZipWriter<fs::File>,
        dir: &Path,
        prefix: &str,
        opts: &zip::write::SimpleFileOptions,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = format!("{prefix}/{}", entry.file_name().to_str().unwrap());
            if path.is_dir() {
                zip.add_directory(&name, *opts).unwrap();
                add_dir_to_zip(zip, &path, &name, opts);
            } else {
                zip.start_file(&name, *opts).unwrap();
                let mut f = fs::File::open(&path).unwrap();
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut f, &mut buf).unwrap();
                std::io::Write::write_all(zip, &buf).unwrap();
            }
        }
    }
}
