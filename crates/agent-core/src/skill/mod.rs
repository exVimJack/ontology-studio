//! Skill 系统（决策 20）。
//!
//! 基于 agentskills.io 开放规范的、以文本为主的扩展机制。Skill = SKILL.md
//! （YAML frontmatter + Markdown 正文），不是可执行插件——需要执行能力的 skill
//! 走 MCP server。渐进式披露：name + description 常驻 preamble（Tier 1，几十
//! token），完整正文按需由模型调 `read_document` 读取（Tier 2）。
//!
//! 复用现有基础设施：
//!   - memory::documents 表 + read_document（渐进式披露 Tier 2）
//!   - memory::skill_repo 两张表（全局禁用 + 会话级激活）
//!   - chat.rs 的 AgentBuilder::preamble（首次引入系统人设 + skill Tier 1）
//!   - 详见 docs/SKILL-SYSTEM.md
//!
//! 模块结构：
//!   - manager.rs   SkillManager：扫描 / 入库 / preamble 拼接 / 导入 / 卸载
//!   - activate.rs  会话激活 + disable 三层判断 + active_skill_doc_paths
//!   - prompt.rs    preamble XML 生成（<available_skills> 块）
//!   - builtin.rs   内置 skill frontmatter 扩展（补 disable-model-invocation）
//!   - import.rs    导入操作（本地目录复制 / zip 解压，复用 ingest::security）

pub mod activate;
pub mod builtin;
pub mod import;
pub mod manager;
pub mod prompt;

pub use manager::SkillManager;

use std::path::{Path, PathBuf};

/// Skill 来源（对应 conversation_skills.source 列）。
///
/// kebab-case 序列化，specta 导出为 TS 字符串字面量联合类型。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, specta::Type,
)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSource {
    /// 随应用分发，只读（resource_dir/skills/）
    Builtin,
    /// 用户导入，可读写（~/.onto-studio/skills/）
    Imported,
    /// 跨客户端扫描，只读（~/.agents/skills/ 等）
    ExternalReadOnly,
    /// 项目级（二期，需先定义"工作区"概念）
    Project,
}

impl SkillSource {
    /// 转为 conversation_skills.source 列存储的字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Imported => "imported",
            Self::ExternalReadOnly => "external-readonly",
            Self::Project => "project",
        }
    }

    /// 去重优先级（高优先级覆盖同名低优先级）。
    fn priority(self) -> u8 {
        match self {
            Self::Builtin => 3,
            Self::Imported => 2,
            Self::ExternalReadOnly => 1,
            Self::Project => 0,
        }
    }
}

/// 一个已发现的 Skill（扫描后的内存表示）。
#[derive(Debug, Clone)]
pub struct SkillRecord {
    /// skill name（去重键，与目录名一致）
    pub name: String,
    /// ≤1024 字符，preamble Tier 1 用
    pub description: String,
    pub source: SkillSource,
    /// skill 目录绝对路径
    pub dir_path: PathBuf,
    /// 入库 documents 表后的 id（None = 未入库）
    pub doc_id: Option<String>,
    /// references/assets/scripts 三个规范子目录下所有入库文件的 doc path 列表
    /// （形如 `skill://<name>/<dir>/<file>`，None = 未入库）。
    /// 随 body 一起入库（ensure_skill_documented），随父 skill 一起进
    /// active_skill_doc_paths / 一起卸载清理。
    pub resource_doc_paths: Option<Vec<String>>,
    /// frontmatter 层次 1（Govcraft 不解析，业务层补）
    pub disable_model_invocation: bool,
    /// frontmatter allowed-tools（可选，空格分隔工具列表）
    pub allowed_tools: Option<Vec<String>>,
    /// frontmatter license（可选，展示用）
    pub license: Option<String>,
    /// frontmatter compatibility（可选，展示用）
    pub compatibility: Option<String>,
}

/// Skill 系统统一错误。
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill load failed: {0}")]
    Load(String),
    #[error("invalid skill: {0}")]
    InvalidSkill(String),
    #[error("skill already exists: {0}")]
    AlreadyExists(String),
    #[error("skill not imported: {0}")]
    NotImported(String),
    #[error("zip bomb protection: {0}")]
    ZipBomb(String),
    #[error("zip: {0}")]
    Zip(String),
    #[error("io: {0}")]
    Io(String),
    #[error("no SKILL.md found in zip")]
    NoSkillInZip,
    #[error("memory: {0}")]
    Memory(#[from] memory::MemoryError),
}

impl SkillError {}

/// 读取并解析某目录下的 `SKILL.md`，**先把 CRLF 归一为 LF 再解析**。
///
/// 动机是实现事实而非偏好：`agent-skills 0.2.0` 的 frontmatter 切分器
/// （`find_closing_delimiter`）用 `content.lines()` 迭代（`lines()` 会剥掉 `\r`），
/// 却按 `line.len() + 1` 累计字节偏移——CRLF 每行少算 1 字节。累计漂移后
/// `&after_opening[..pos]` / `[pos + 3..]` 会落进多字节字符中间（panic）或提前切到
/// frontmatter 内部（正文起始带残片）。所以**所有**解析入口必须先归一化。
///
/// 归一化只作用于内存中的解析输入，MUST NOT 回写用户磁盘上的源文件。
///
/// 分步计时日志（metadata → read+parse）留在这里：当初为定位 Windows 上
/// `SkillDirectory::load` 内部 fs 调用卡死而加，重构时不得削弱。
pub(super) fn read_skill_md(dir: &Path) -> Result<agent_skills::Skill, SkillError> {
    let skill_md = dir.join("SKILL.md");
    // symlink_metadata 而非 skill_md.is_file()：后者调 std::fs::metadata，在 Windows
    // 上对 reparse point / junction / 网络符号链接会阻塞解析（扫描未知目录时会卡死）。
    let meta_t = std::time::Instant::now();
    let md = std::fs::symlink_metadata(&skill_md);
    let is_file = md.as_ref().map(|m| m.is_file()).unwrap_or(false);
    tracing::debug!(
        dir = %dir.display(),
        skill_md_exists = md.is_ok(),
        is_file,
        elapsed_ms = meta_t.elapsed().as_millis() as u64,
        "skill_md: metadata"
    );
    if !is_file {
        return Err(SkillError::InvalidSkill(format!(
            "{} 下没有 SKILL.md",
            dir.display()
        )));
    }
    let read_t = std::time::Instant::now();
    let raw = std::fs::read_to_string(&skill_md)
        .map_err(|e| SkillError::Load(format!("读取 SKILL.md 失败：{e}")))?;
    let content = raw.replace("\r\n", "\n");
    let skill = agent_skills::Skill::parse(&content)
        .map_err(|e| SkillError::InvalidSkill(format!("SKILL.md 解析失败：{e}")))?;
    tracing::debug!(
        dir = %dir.display(),
        bytes = content.len(),
        elapsed_ms = read_t.elapsed().as_millis() as u64,
        "skill_md: read+parse ok"
    );
    Ok(skill)
}

/// 加载并校验一个 skill 目录，校验语义与上游 `SkillDirectory::load` 逐条对齐
/// （上游 `agent-skills-0.2.0/src/loader.rs` 共 5 条：目录存在且是目录 → SKILL.md
/// 存在 → 可读 → 可解析 → frontmatter `name` == 目录名；无其它隐含校验）。
///
/// 项目内**所有**需要校验的 SKILL.md 读取路径都走这里，不再调用上游那个在 CRLF
/// 上会 panic 的 API——目的不只是绕开缺陷，更是消除「发现路径修了、导入路径没修」
/// 这类两处实现分叉。
pub(super) fn load_skill_at(dir: &Path) -> Result<agent_skills::Skill, SkillError> {
    let skill = read_skill_md(dir)?;
    let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if skill.name().as_str() != dir_name {
        return Err(SkillError::InvalidSkill(format!(
            "SKILL.md 的 name「{}」与目录名「{dir_name}」不一致",
            skill.name().as_str()
        )));
    }
    Ok(skill)
}
/// agentskills.io 规范定义的 skill 资源子目录（除 SKILL.md 外的可选目录）。
///
/// 规范目录结构：
/// ```text
/// skill-name/
/// ├── SKILL.md      # 必需：元数据 + 指令
/// ├── scripts/      # 可选：可执行代码
/// ├── references/   # 可选：文档
/// └── assets/       # 可选：模板、资源
/// ```
///
/// onto-studio 的处理：三个目录下的文本文件都入库为 doc
/// （`skill://<name>/<dir>/<file>`），让模型经 read_document / search_documents
/// 触达完整 skill 内容。二进制文件（图片等）跳过（本期不做 MEDIA_REFERENCE）。
///
/// - `references` / `assets`：资源文档，模型直接读
/// - `scripts`：可执行脚本。onto-studio 无执行 skill 脚本的能力（skill 是文本扩展，
///   可执行走 MCP），但模型仍需读到脚本内容以理解可用命令、指导用户，故同样入库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum SkillSubdir {
    References,
    Assets,
    Scripts,
}

impl SkillSubdir {
    /// 目录名（与磁盘上的子目录名一致）。
    pub(super) fn dir_name(self) -> &'static str {
        match self {
            Self::References => "references",
            Self::Assets => "assets",
            Self::Scripts => "scripts",
        }
    }

    /// 遍历规范定义的全部子目录。
    pub(super) fn all() -> &'static [SkillSubdir] {
        &[Self::References, Self::Assets, Self::Scripts]
    }
}

/// 纳入入库的文本资源扩展名（agentskills.io 规范识别的资源扩展名超集）。
/// 二进制（图片 / dll / 压缩包等）跳过，本期不做 MEDIA_REFERENCE。
const RESOURCE_TEXT_EXTS: &[&str] = &[
    "md", "txt", "json", "yaml", "yml", "toml", "csv", "xml", "sh", "py", "js", "ts",
    "rs",
];

/// 构造单个资源文件的 doc path：`skill://<name>/<dir>/<filename>`。
/// filename 保留原文件名（含扩展名），便于模型从 SKILL.md body 里的相对路径
/// 引用（如 `references/gaia-schema-contract.md`、`scripts/run-check.sh`）
/// 对应到 doc path。
pub(super) fn resource_doc_path(skill_name: &str, subdir: SkillSubdir, filename: &str) -> String {
    format!("skill://{skill_name}/{}/{filename}", subdir.dir_name())
}

/// 判断文件是否为可入库的文本资源（按扩展名）。
pub(super) fn is_text_resource(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    RESOURCE_TEXT_EXTS.iter().any(|&ok| ok == ext)
}

/// 扫描 skill 目录下某个规范子目录，返回所有可入库的文本文件路径
/// （按文件名排序，保证 discover 顺序稳定）。目录不存在或无匹配文件返回空。
///
/// 直接用 read_dir 扫规范子目录名（references/assets/scripts），不依赖上游
/// `agent_skills::SkillDirectory` 的枚举 API——它只能经 `SkillDirectory::load` 构造，
/// 而该构造在 CRLF 输入上会 panic（见 `read_skill_md`）。独立扫描更鲁棒，且不要求
/// 目录先通过校验。
pub(super) fn scan_subdir_files(
    skill_dir: &Path,
    subdir: SkillSubdir,
) -> Vec<PathBuf> {
    let subdir_path = skill_dir.join(subdir.dir_name());
    let Ok(entries) = std::fs::read_dir(&subdir_path) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_text_resource(p))
        .collect();
    files.sort();
    files
}
