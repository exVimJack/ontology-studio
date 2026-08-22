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
/// 复用 `agent_skills::SkillDirectory` 的枚举 API（scripts()/references()/assets()），
/// 避免手写 read_dir 与规范目录名脱节。
pub(super) fn scan_subdir_files(
    skill_dir: &agent_skills::SkillDirectory,
    subdir: SkillSubdir,
) -> Vec<PathBuf> {
    let mut files = match subdir {
        SkillSubdir::References => skill_dir.references().unwrap_or_default(),
        SkillSubdir::Assets => skill_dir.assets().unwrap_or_default(),
        SkillSubdir::Scripts => skill_dir.scripts().unwrap_or_default(),
    };
    files.retain(|p| p.is_file() && is_text_resource(p));
    files.sort();
    files
}
