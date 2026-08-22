//! Skill 目录路径解析（决策 20）。
//!
//! 复用 pdfium.rs 的三层兜底模式（Resource → resource_dir/resources → CARGO_MANIFEST_DIR）
//! 解析内置 skill 目录。用户导入目录与跨客户端扫描目录用 `dirs` crate 解析 home。

use std::path::PathBuf;
use tauri::{AppHandle, Manager, Wry};
use tauri::path::BaseDirectory;

/// 内置 skill 目录：`resource_dir/skills/`（dev = `src-tauri/resources/skills/`）。
///
/// 复用 pdfium.rs 的三层兜底（Resource → resource_dir/resources → CARGO_MANIFEST_DIR）。
/// 三层都找不到时返回 None（不 panic，skill 系统降级为空列表，不阻断启动）。
pub fn builtin_dir(app: &AppHandle<Wry>) -> Option<PathBuf> {
    app.path()
        .resolve("skills", BaseDirectory::Resource)
        .ok()
        .filter(|p| p.exists())
        // 兜底 1：dev 下 Tauri 会把 bundle.resources 拷到 <exe-dir>/resources/。
        .or_else(|| {
            app.path()
                .resource_dir()
                .ok()
                .map(|d| d.join("resources").join("skills"))
                .filter(|p| p.exists())
        })
        // 兜底 2：源码树（CARGO_MANIFEST_DIR = src-tauri/，resources/ 在其下）。
        .or_else(|| {
            let manifest = env!("CARGO_MANIFEST_DIR");
            let p = std::path::Path::new(manifest).join("resources").join("skills");
            if p.exists() {
                Some(p)
            } else {
                None
            }
        })
}

/// 用户导入目录：`~/.onto-studio/skills/`。
///
/// 对应 `~/.claude/skills/`、`~/.pi/agent/skills/`（业界点目录规范，
/// 每个客户端有自己的全局目录，符合 agentskills.io 实现指南）。
pub fn user_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".onto-studio")
        .join("skills")
}

/// 跨客户端只读扫描目录。
///
/// 扫描这些目录可发现其他合规 CLI（Claude/pi/agents 约定）安装的 skill，
/// 实现跨客户端互操作。全部只读。
pub fn external_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    vec![
        home.join(".agents").join("skills"),         // 跨客户端约定
        home.join(".claude").join("skills"),         // Claude Code
        home.join(".pi").join("agent").join("skills"), // pi
    ]
}
