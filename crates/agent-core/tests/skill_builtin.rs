//! 内置 skill 兼容性自测（AGENTS.md 工作守则 6）。
//!
//! 验证内置 skill（src-tauri/resources/skills/）能被 SkillDirectory::load
//! 成功加载、name 匹配目录名、description 非空且 ≤1024 字符、body 非空、
//! references 可读。
//!
//! 内置 skill 随应用分发，必须在发版前保证格式合规。
//! 新增/移除内置 skill 时，同步更新下方 `EXPECTED` 列表。

use agent_core::skill::SkillManager;
use agent_skills::SkillDirectory;
use std::path::PathBuf;

/// 当前实际分发的内置 skill 集合（目录名 = skill name）。
/// 与 `src-tauri/resources/skills/` 下的子目录一一对应。
const EXPECTED: &[&str] = &["ontology-modeling"];

/// 定位内置 skill 目录（与 src-tauri/src/skill.rs 三层兜底一致）。
fn builtin_skills_dir() -> PathBuf {
    // 测试在 crates/agent-core 运行，CARGO_MANIFEST_DIR = crates/agent-core
    // 内置 skill 在 src-tauri/resources/skills/，相对路径 ../../src-tauri/resources/skills
    let manifest = env!("CARGO_MANIFEST_DIR");
    let candidates = [
        PathBuf::from(manifest).join("../../src-tauri/resources/skills"),
        // 也可能是从 workspace 根运行
        PathBuf::from(manifest).join("../src-tauri/resources/skills"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.canonicalize().unwrap_or_else(|_| c.clone());
        }
    }
    // 找不到则跳过测试（CI 环境可能未含 src-tauri 资源）
    PathBuf::from(manifest).join("../../src-tauri/resources/skills")
}

fn builtin_dir_exists() -> bool {
    builtin_skills_dir().exists()
}

#[test]
fn builtin_skills_load_successfully() {
    if !builtin_dir_exists() {
        eprintln!("[skip] builtin skills dir not found, skipping");
        return;
    }
    let dir = builtin_skills_dir();
    for name in EXPECTED {
        let skill_dir = dir.join(name);
        assert!(
            skill_dir.exists(),
            "内置 skill 目录缺失: {}",
            skill_dir.display()
        );
        let loaded = SkillDirectory::load(&skill_dir)
            .unwrap_or_else(|e| panic!("加载内置 skill {} 失败: {e}", name));
        // name 匹配目录名（SkillDirectory::load 强校验）
        assert_eq!(loaded.skill().name().as_str(), *name);
        // description 非空且 ≤1024 字符
        let desc = loaded.skill().description().as_str();
        assert!(!desc.is_empty(), "{} description 为空", name);
        assert!(
            desc.chars().count() <= 1024,
            "{} description 超 1024 字符（{}）",
            name,
            desc.chars().count()
        );
        // body 非空
        assert!(!loaded.skill().body().trim().is_empty(), "{} body 为空", name);
    }
}

#[test]
fn builtin_ontology_modeling_has_references() {
    if !builtin_dir_exists() {
        eprintln!("[skip] builtin skills dir not found, skipping");
        return;
    }
    let dir = builtin_skills_dir();
    let skill_dir = dir.join("ontology-modeling");
    let loaded = SkillDirectory::load(&skill_dir).expect("load ontology-modeling skill");
    assert!(
        loaded.has_references(),
        "ontology-modeling skill 应有 references/ 目录"
    );
    let refs = loaded.references().expect("list references");
    // 四份契约文档必须齐全
    for expected_ref in &[
        "gaia-schema-contract.md",
        "material-to-ontology.md",
        "naming-conventions.md",
        "ontology-package-format.md",
    ] {
        assert!(
            refs.iter().any(|p| p.file_name().is_some_and(|n| n == *expected_ref)),
            "references/ 应含 {expected_ref}"
        );
        let content = loaded
            .read_reference(expected_ref)
            .unwrap_or_else(|e| panic!("read {expected_ref}: {e}"));
        assert!(!content.is_empty(), "{expected_ref} 内容为空");
    }
}

#[test]
fn builtin_skills_discoverable_via_manager() {
    if !builtin_dir_exists() {
        eprintln!("[skip] builtin skills dir not found, skipping");
        return;
    }
    let dir = builtin_skills_dir();
    let mem = std::sync::Arc::new(memory::Memory::open_in_memory().unwrap());
    let mgr = SkillManager::new(
        mem,
        dir,
        PathBuf::from("/nonexistent/user"),
        vec![],
    );
    let all = mgr.discover_all();
    assert_eq!(all.len(), EXPECTED.len(), "应发现 {} 个内置 skill", EXPECTED.len());
    let names: Vec<_> = all.iter().map(|r| r.name.clone()).collect();
    for expected in EXPECTED {
        assert!(
            names.iter().any(|n| n == expected),
            "未发现内置 skill: {}",
            expected
        );
    }
    // 全部标记为 Builtin
    assert!(all.iter().all(|r| matches!(r.source, agent_core::skill::SkillSource::Builtin)));
}
