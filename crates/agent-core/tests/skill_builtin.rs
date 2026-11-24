//! 内置 skill 兼容性自测（AGENTS.md 工作守则 6）。
//!
//! 验证内置 skill（src-tauri/resources/skills/）能被**项目自己的加载路径**成功
//! 发现并入库：name 匹配目录名、description 非空且 ≤1024 字符、body 非空且开头
//! 无 frontmatter 残片、references 四类契约文档齐全且可按 path 读到。
//!
//! 这里刻意不调 `agent_skills::SkillDirectory::load`：上游那个构造在 CRLF 输入上
//! 会 panic（`find_closing_delimiter` 每行少算 1 字节），而生产代码已经不走它了
//! ——测一个用不到的 API 只会把上游缺陷伪装成我们的红灯。测 `SkillManager` 这条
//! 真实链路才能同时守住「内置技能合规」与「Windows CRLF 检出下仍然可用」两件事。
//!
//! 内置 skill 随应用分发，必须在发版前保证格式合规。
//! 新增/移除内置 skill 时，同步更新下方 `EXPECTED` 列表。

use agent_core::skill::{SkillManager, SkillSource};
use std::path::PathBuf;
use std::sync::Arc;

/// 当前实际分发的内置 skill 集合（目录名 = skill name）。
/// 与 `src-tauri/resources/skills/` 下的子目录一一对应。
const EXPECTED: &[&str] = &["ontology-modeling", "ontology-modeling-w3c"];

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

/// 用内置目录构造一个管理器（user/external 指向不存在的路径，只扫内置）。
fn manager_for_builtin() -> SkillManager {
    let mem = Arc::new(memory::Memory::open_in_memory().unwrap());
    SkillManager::new(
        mem,
        builtin_skills_dir(),
        PathBuf::from("/nonexistent/user"),
        vec![],
    )
}

#[test]
fn builtin_skills_load_successfully() {
    if !builtin_dir_exists() {
        eprintln!("[skip] builtin skills dir not found, skipping");
        return;
    }
    let mgr = manager_for_builtin();
    let mut all = mgr.discover_all();
    for name in EXPECTED {
        let rec = all
            .iter_mut()
            .find(|r| r.name == *name)
            .unwrap_or_else(|| panic!("内置 skill {name} 未被发现（SKILL.md 解析失败或 name 与目录名不一致）"));
        assert!(
            matches!(rec.source, SkillSource::Builtin),
            "{name} 来源应为 Builtin"
        );
        // description 非空且 ≤1024 字符
        assert!(!rec.description.is_empty(), "{name} description 为空");
        assert!(
            rec.description.chars().count() <= 1024,
            "{} description 超 1024 字符（{}）",
            name,
            rec.description.chars().count()
        );
        // body 经真实入库路径可读到，且开头不是 frontmatter 残片
        // （CRLF 缺陷的「不 panic 但错位」那一档正是从 body 头部露出 `---`）
        let id = mgr
            .ensure_skill_documented(rec)
            .unwrap_or_else(|e| panic!("{name} 入库失败: {e}"));
        let read = mgr
            .memory()
            .read_document(&id, None, None)
            .unwrap()
            .unwrap_or_else(|| panic!("{name} 入库后读不到"));
        let (_, _, _, body, _) = read;
        assert!(!body.trim().is_empty(), "{name} body 为空");
        assert!(
            !body.trim_start().starts_with('-'),
            "{name} body 开头疑似 frontmatter 残片: {:?}",
            &body.chars().take(40).collect::<String>()
        );
    }
}

#[test]
fn builtin_ontology_modeling_has_references() {
    if !builtin_dir_exists() {
        eprintln!("[skip] builtin skills dir not found, skipping");
        return;
    }
    let mgr = manager_for_builtin();
    let mut all = mgr.discover_all();
    let rec = all
        .iter_mut()
        .find(|r| r.name == "ontology-modeling")
        .expect("内置 ontology-modeling skill 应被发现");
    mgr.ensure_skill_documented(rec)
        .expect("入库应成功");

    let paths = rec.resource_doc_paths.clone().unwrap_or_default();
    // 四份契约文档必须齐全，且能按 doc path 读到非空内容（模型侧就靠这条路取资源）
    for expected_ref in &[
        "gaia-schema-contract.md",
        "material-to-ontology.md",
        "naming-conventions.md",
        "ontology-package-format.md",
    ] {
        let want = format!("skill://ontology-modeling/references/{expected_ref}");
        assert!(
            paths.iter().any(|p| p == &want),
            "references 未入库: {want}，实际={paths:?}"
        );
        let read = mgr
            .memory()
            .read_document_by_path(&want)
            .unwrap()
            .unwrap_or_else(|| panic!("{expected_ref} 应按 path 可查"));
        assert!(!read.3.is_empty(), "{expected_ref} 内容为空");
    }
}

#[test]
fn builtin_skills_discoverable_via_manager() {
    if !builtin_dir_exists() {
        eprintln!("[skip] builtin skills dir not found, skipping");
        return;
    }
    let mgr = manager_for_builtin();
    let all = mgr.discover_all();
    assert_eq!(
        all.len(),
        EXPECTED.len(),
        "应发现 {} 个内置 skill",
        EXPECTED.len()
    );
    let names: Vec<_> = all.iter().map(|r| r.name.clone()).collect();
    for expected in EXPECTED {
        assert!(
            names.iter().any(|n| n == expected),
            "未发现内置 skill: {}",
            expected
        );
    }
    // 全部标记为 Builtin
    assert!(all.iter().all(|r| matches!(r.source, SkillSource::Builtin)));
}