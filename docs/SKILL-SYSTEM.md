# Skill 系统设计文档

> **状态**：设计完成，待开发。本文档是交接给开发人员的完整实施方案。
> **前置阅读**：`ARCHITECTURE.md`（决策 5/15/17/19）、`AGENTS.md`（五大原则）、`CONVERSATION-SCOPE.md`（会话级激活集范式）
> **关联 ADR**：本文档落地时应在 `ARCHITECTURE.md` 新增「决策 20：Skill 系统」
> **最后更新**：2026-07-31

---

## 0. 速览（给开发人员的 5 分钟版）

### 要做什么

为 onto-studio 增加 **Agent Skill** 支持——一种基于 [agentskills.io](https://agentskills.io) 开放规范的、**以文本为主的扩展机制**，让模型按需加载领域知识（联邦查询向导、本体设计指南、PDF 摄取最佳实践等），而非常驻 system prompt。

### 核心特征（必须守住的两点）

1. **以文本为主**：Skill = SKILL.md（YAML frontmatter + Markdown 正文），不是可执行插件。需要执行能力的 skill 走 MCP server（已有 `McpManager`）。
2. **渐进式披露**：name + description 常驻 preamble（Tier 1，几十 token）；完整 SKILL.md 正文按需由模型调 `read_document` 读取（Tier 2）。不注入全文、不用 embedding 检索。

### 技术选型一句话

引入 `agent-skills = "0.2"`（Govcraft，MIT/Apache 双许可）做 SKILL.md 解析+校验+目录加载；onto-studio 自建 `SkillManager`（约 300 行业务代码）做扫描+入库+激活+preamble 拼接。零 embedding、零新抽象，复用现有 `read_document` + `AgentBuilder::preamble` + `CompactingMemory`。

### 四类 Skill 来源

| 来源 | 存放 | 读写 | 可 disable | 可卸载 |
|---|---|---|---|---|
| **内置**（随应用分发） | Tauri 资源 `resources/skills/` | 只读 | ✅ 全局+会话 | ❌ |
| **导入**（用户安装） | `~/.onto-studio/skills/` | 可读写 | ✅ 全局+会话 | ✅ |
| **跨客户端只读**（白嫖 Claude/pi） | `~/.agents/skills/` 等 | 只读 | ✅ 全局+会话 | ❌ |
| **项目级**（二期） | `<workspace>/.agents/skills/` | 只读 | ✅ | ❌ |

### 三层 disable 语义

```
层次 1（作者声明）：SKILL.md frontmatter 的 disable-model-invocation: true
                  → 不进自动 preamble，只能 @skillName 显式调（只读属性，不可运行时改）
层次 2（全局偏好）：disabled_skills 表（应用级，跨所有会话）
                  → 用户在设置页关掉某 skill，所有会话都不进 preamble
层次 3（会话级）  ：conversation_skills 表的 enabled 列
                  → 单会话开关，@skillName 激活或 Inspector 勾选
```

---

## 1. 背景与动机

### 1.1 为什么需要 Skill

onto-studio 的核心能力（联邦查询、本体建模、多模态摄取）有大量领域知识：SQL 模板、ObjectType 建模规范、PDF 解析能力边界等。目前这些知识要么塞在 system prompt（常驻 context、每轮重发、破坏 prefix cache），要么依赖模型通用知识（不可控）。

**Agent Skill 解决这个问题**：把领域知识封装成独立 SKILL.md，description 常驻 preamble（模型知道"有这个能力"），正文按需读取（模型判断需要时调 `read_document` 取全文）。这正是 onto-studio 决策 17（`@` 挂载的 agentic search）的延伸——**知识也走 agentic search，不自动注入**。

### 1.2 为什么选 agentskills.io 标准

- **开放规范**（Anthropic 发起，非私有）：SKILL.md = YAML frontmatter + Markdown，任何 agent 框架实现"发现+解析+渐进式披露"即可支持
- **生态复用**：Anthropic 官方 skill 仓库（[anthropics/skills](https://github.com/anthropics/skills)）、pi skill 仓库（[badlogic/pi-skills](https://github.com/badlogic/pi-skills)）开箱即用
- **跨客户端互操作**：`.agents/skills/` 是跨客户端事实约定，用户已装的 Claude/pi skill 可直接扫描复用
- **与 onto-studio 决策 17 完全对齐**：规范本身就是 progressive disclosure，无需额外抽象

### 1.3 为什么不用 Rig 的动态上下文机制

Rig 0.41 的 `dynamic_context(n, index)` 和 `retrieved_tools` 都需要 `VectorStoreIndexDyn`（embedding 模型）。onto-studio 原则 2 禁止本地 embedding 模型，决策 17 已明确走 agentic search 路线。Skill 的渐进式披露用 `preamble` + `read_document` 即可实现，不需要 embedding 检索。

---

## 2. 业界调研结论

### 2.1 Skill 目录规范（已核实）

| 客户端 | 全局（用户级） | 项目级 |
|---|---|---|
| Claude Code | `~/.claude/skills/` | `.claude/skills/` |
| pi | `~/.pi/agent/skills/` | `.pi/skills/` |
| Gemini CLI | `~/.gemini/skills/` | `.gemini/skills/` |
| Cursor | `~/.cursor/skills/` | `.cursor/skills/` |
| **跨客户端约定** | — | `.agents/skills/` |

**关键事实**（来自 [agentskills.io 实现指南](https://agentskills.io/client-implementation/adding-skills-support)）：
- 规范本身**不规定目录位置**，只定义 SKILL.md 内容格式
- `.agents/skills/` 是跨客户端互操作的事实约定（扫描它可发现其他合规 CLI 安装的 skill）
- 全局目录是各客户端私有的（`~/.claude/`、`~/.pi/`），**没有统一的跨客户端全局约定**
- **onto-studio 的 `~/.onto-studio/skills/` 对应 Claude 的 `~/.claude/skills/`、pi 的 `~/.pi/agent/skills/`——这是符合规范的，每个客户端都有自己的全局目录**

### 2.2 pi 的 skill 机制（最成熟，作为主要参照）

pi 扫描以下位置（来自 [pi skills 文档](https://pi.dev/docs/latest/skills)）：
- 全局：`~/.pi/agent/skills/` + `~/.agents/skills/`
- 项目级（仅受信项目）：`.pi/skills/` + `.agents/skills/`（cwd 及祖先到 git root）
- Packages：`skills/` 目录或 `package.json` 的 `pi.skills`
- Settings：`skills` 数组（可加 `~/.claude/skills` 等跨客户端路径）
- CLI：`--skill <path>`

pi 的 disable 机制：`settings.json` 的 `skills` 数组加 `-path` 前缀条目（如 `-"~/.pi/agent/skills/foo"`）；`--no-skills` 全关；`enableSkillCommands` 控制 `/skill:` 命令。

pi 的 frontmatter 字段（含 agentskills.io 规范字段）：
```yaml
name: my-skill              # 必填，1-64 字符，小写字母/数字/连字符
description: ...            # 必填，≤1024 字符，说明做什么+何时用
license: MIT                # 可选
compatibility: ...          # 可选，≤500 字符，环境要求
metadata: {...}             # 可选，任意键值
allowed-tools: "tool1 tool2"# 可选，空格分隔（实验性）
disable-model-invocation: true  # 可选，true=不进自动 preamble，只能 /skill:name
```

### 2.3 Claude Code 的 disable 教训（必须吸取）

Claude Code **内置 skill 无法 disable/uninstall**——这是用户痛点（[issue #26838](https://github.com/anthropics/claude-code/issues/26838)、[#39749](https://github.com/anthropics/claude-code/issues/39749) 仍 open）。用户抱怨 `claude-developer-platform` 等内置 skill 自动触发却关不掉。

**onto-studio 必须支持内置 skill 的 disable**（全局+会话级），避免重蹈覆辙。但内置 skill 不可卸载（物理在资源目录，卸载无意义）。

### 2.4 候选库评估（已逐行核实源码）

| 库 | 许可证 | 评估结论 |
|---|---|---|
| **`agent-skills` (Govcraft) 0.2.0** ⭐ | MIT OR Apache-2.0 | **引入**。聚焦"解析+校验+目录加载"，边界干净。依赖轻（serde + serde_yml） |
| `agent-skills-rs` (tumf) v0.3.1 | MIT | **不引入**。只做安装到目录（覆盖 Skill 生命周期 1/4），依赖冗余（clap/directories），serde_yaml 0.9 deprecated |
| `skill-manager` (iamawatermelo) | — | **不引入**。空仓库 |
| `kubiyabot/skill` | — | **不引入**。WASM 沙箱 + FastEmbed embedding 违反原则 1/2 |

**Govcraft `agent-skills` 0.2.0 的公共 API**（已核实 `src/lib.rs`）：
```rust
pub use skill::Skill;              // SKILL.md 解析结果
pub use frontmatter::{Frontmatter, FrontmatterBuilder};
pub use loader::SkillDirectory;    // 目录加载（含 scripts/references/assets 访问）
pub use name::{SkillName, SkillNameError};
pub use description::{SkillDescription, SkillDescriptionError};
pub use compatibility::{Compatibility, CompatibilityError};
pub use metadata::Metadata;
pub use allowed_tools::AllowedTools;
pub use error::{LoadError, ParseError};
```

关键方法（已核实签名）：
- `Skill::parse(content: &str) -> Result<Skill, ParseError>`：解析 SKILL.md 全文
- `SkillDirectory::load(path: impl AsRef<Path>) -> Result<SkillDirectory, LoadError>`：加载目录，**校验 skill name == 目录名**（规范要求）
- `skill.name() -> &SkillName`、`skill.description() -> &SkillDescription`、`skill.body() -> &str`、`skill.frontmatter() -> &Frontmatter`
- `SkillDirectory::read_reference(name) / read_script(name) / read_asset(name)`：访问子目录文件
- `Frontmatter` 字段：name/description/license/compatibility/metadata/allowed_tools

**Govcraft 不提供的能力**（onto-studio 自建）：
- ❌ `to_prompt`（生成 preamble XML）——CLI 里有，库没有，需自建（约 20 行）
- ❌ `disable-model-invocation` 字段解析——`RawFrontmatter` 没这字段，需业务层补
- ❌ 扫描/激活/会话绑定——全部是 onto-studio 业务逻辑

### 2.5 遗留问题 L1（serde_yml 安全公告）

**问题**：`agent-skills 0.2.0` 依赖 `serde_yml = "0.0.12"`，有 **RUSTSEC-2025-0068**（unsound 内存不安全）+ 仓库 archived。

**现状决策**：**暂不 patch，先用 0.2.0 跑通业务**（用户明确要求不搞重方案）。记录此遗留问题，待onto-studio 真正发版前处理。

**缓解措施**（未来 patch 时）：
- onto-studio `Cargo.lock` **已有 `serde_yaml_ng 0.10.0`**（lindera 传递依赖拉入）
- Govcraft 全库对 serde_yml 的使用仅 2 处（`skill.rs:189` 的 `from_str`、`read_properties.rs:118` 的 `to_string`）
- patch 成本：3 行代码 + 2 个 Cargo.toml 依赖名，用 `[patch.crates-io]` 指向 fork
- 同时给 Govcraft 上游提 PR 换 serde_yaml_ng

**跟踪**：落地时写进 `ARCHITECTURE.md` 决策 20 的"已知问题"段 + `PROGRESS.md` 待办。

---

## 3. 架构设计

### 3.1 整体分层

```
┌─────────────────────────────────────────────────────────────────┐
│ agent-skills (Govcraft, 0.2.0) — 协议层（复用）                  │
│   Skill::parse / SkillDirectory::load / Frontmatter 字段访问     │
│   负责：SKILL.md → 强类型对象（含校验）、目录结构访问            │
├─────────────────────────────────────────────────────────────────┤
│ crates/agent-core/src/skill/ — onto-studio 业务层（自建，~300行）│
│   mod.rs        模块入口 + SkillRecord / SkillSource 枚举        │
│   manager.rs    SkillManager: 扫描 / 入库 / preamble 拼接        │
│   activate.rs   会话激活 + disable 三层判断 + @skillName 解析    │
│   prompt.rs     preamble XML 生成（<available_skills> 块）       │
│   builtin.rs    内置 skill 资源目录解析（复用 pdfium 三层兜底）  │
│   import.rs     导入操作（本地目录复制 / zip 解压）              │
├─────────────────────────────────────────────────────────────────┤
│ 复用现有基础设施（需小幅改动）                                   │
│   memory: documents 表 + read_document（渐进式披露 Tier 2）      │
│   memory: 新增 disabled_skills + conversation_skills 两张表     │
│   chat.rs: 首次引入 preamble 机制（rig AgentBuilder::preamble）  │
│   provider.rs: ProviderConfig 加 preamble 字段（首次引入）       │
│   src-tauri: Tauri 资源打包（pdfium 同款）+ setup hook 初始化    │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 Skill 来源与目录

```
┌──────────────────────────────────────────────────────────────────┐
│ 1. 内置 Skill（随应用分发，只读）                                 │
│    源码: src-tauri/resources/skills/<name>/SKILL.md             │
│    打包: tauri.conf.json bundle.resources += "resources/skills"  │
│    运行: resource_dir()/skills/<name>/  (pdfium 同款三层兜底解析)│
│    标记: SkillSource::Builtin                                    │
├──────────────────────────────────────────────────────────────────┤
│ 2. 导入 Skill（用户主动安装，可读写）                             │
│    存放: ~/.onto-studio/skills/<name>/                          │
│    来源: GUI 导入（本地目录复制 / zip 解压 / 二期 GitHub clone） │
│    标记: SkillSource::Imported                                   │
├──────────────────────────────────────────────────────────────────┤
│ 3. 跨客户端只读扫描（白嫖 Claude/pi/agents）                      │
│    扫描: ~/.agents/skills/ + ~/.claude/skills/ + ~/.pi/agent/skills/ │
│    标记: SkillSource::ExternalReadOnly                           │
├──────────────────────────────────────────────────────────────────┤
│ 4. 项目级（二期，需先定义"工作区"概念）                          │
│    扫描: <workspace>/.agents/skills/                            │
│    标记: SkillSource::Project                                    │
└──────────────────────────────────────────────────────────────────┘
```

> **注意**：onto-studio 是 Tauri 桌面客户端，没有 CLI 的"当前工作目录"概念。项目级扫描（来源 4）需要先定义"工作区"（用户打开的文件夹），一期不做。

### 3.3 数据模型（SQLite 新增两张表）

在 `crates/memory/src/lib.rs` 的 `init_schema` 中，`init_documents_schema(conn)?` 之后新增：

```rust
// ── Skill 系统（决策 20）──
// 全局禁用偏好（层次 2）：用户在设置页显式 disable 的 skill，跨所有会话不进 preamble
conn.execute_batch(
    r#"
    CREATE TABLE IF NOT EXISTS disabled_skills (
        skill_name    TEXT PRIMARY KEY,        -- skill name（去重键）
        disabled_at   INTEGER NOT NULL          -- unix ms（memory::Timestamp）
    );

    -- 会话级激活（层次 3）：单会话的 skill enable 状态
    -- imported skill 默认不进 preamble，需显式 enabled=1
    -- builtin/external 默认进 preamble，可显式 enabled=0 排除
    CREATE TABLE IF NOT EXISTS conversation_skills (
        conversation_id  TEXT NOT NULL,
        skill_name       TEXT NOT NULL,
        source           TEXT NOT NULL,         -- builtin | imported | external-readonly
        enabled          BOOLEAN NOT NULL DEFAULT 0,
        activated_at     INTEGER NOT NULL,      -- unix ms（memory::Timestamp）
        PRIMARY KEY (conversation_id, skill_name),
        FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
    );
    CREATE INDEX IF NOT EXISTS idx_conv_skills_conv ON conversation_skills(conversation_id);
    "#,
)?;
```

**设计说明**：
- `disabled_skills` 与 `conversation_skills` **独立**——前者是应用级偏好，后者是会话级状态，preamble 拼接时两者 OR 判断（任一命中即不进 preamble）
- `source` 列冗余存于 `conversation_skills`，便于前端展示来源图标
- `conversation_skills` 用 `ON DELETE CASCADE`，会话删除时自动清理（与 `conversation_documents` 一致）
- 时间戳用 `memory::Timestamp` newtype（已实现 specta::Type 导出为 TS number，绕过 BigInt 禁令）

### 3.4 SkillRecord 结构（业务层核心数据结构）

```rust
// crates/agent-core/src/skill/mod.rs

use agent_skills::{Skill, SkillDirectory};
use std::path::PathBuf;

/// Skill 来源（对应数据模型 source 列）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSource {
    Builtin,           // 随应用分发，只读
    Imported,          // 用户导入，可读写
    ExternalReadOnly,  // 跨客户端扫描，只读
    Project,           // 项目级（二期）
}

/// 一个已发现的 Skill（扫描后的内存表示）
#[derive(Debug, Clone)]
pub struct SkillRecord {
    pub name: String,              // skill name（去重键，与目录名一致）
    pub description: String,       // ≤1024 字符，preamble Tier 1 用
    pub source: SkillSource,
    pub dir_path: PathBuf,         // skill 目录绝对路径
    pub doc_id: Option<String>,    // 入库 documents 表后的 id（None=未入库）
    pub disable_model_invocation: bool,  // frontmatter 层次1（Govcraft 不解析，业务层补）
    pub allowed_tools: Option<Vec<String>>,  // frontmatter allowed-tools（可选）
    pub license: Option<String>,
    pub compatibility: Option<String>,
}

/// preamble Tier 1 片段（进 system prompt 的部分）
#[derive(Debug, Clone)]
pub struct SkillPromptEntry {
    pub name: String,
    pub description: String,
    pub location: String,          // read_document(doc_id) 的提示
}
```

### 3.5 SkillManager（核心管理器）

```rust
// crates/agent-core/src/skill/manager.rs

use std::path::{Path, PathBuf};
use std::sync::Arc;
use agent_skills::SkillDirectory;
use memory::Memory;

pub struct SkillManager {
    memory: Arc<Memory>,
    builtin_dir: PathBuf,          // resource_dir()/skills/
    user_dir: PathBuf,             // ~/.onto-studio/skills/
    external_dirs: Vec<PathBuf>,   // ~/.agents/skills/ 等
}

impl SkillManager {
    /// 构造：传入 memory 句柄 + 各目录路径
    /// 路径由 src-tauri setup hook 解析后注入（见 §4.2）
    pub fn new(memory: Arc<Memory>, builtin_dir: PathBuf, user_dir: PathBuf, external_dirs: Vec<PathBuf>) -> Self {
        Self { memory, builtin_dir, user_dir, external_dirs }
    }

    /// 扫描所有来源，返回去重后的 SkillRecord 列表。
    /// 去重规则：同名时 Builtin > Imported > ExternalReadOnly > Project
    /// （与 Claude Code "built-in 与 custom 冲突"一致，但 onto-studio 内置优先）
    pub fn discover_all(&self) -> Vec<SkillRecord> { /* §5.1 详述 */ }

    /// 把 skill body 入库 documents 表，返回 doc_id。
    /// 幂等：path 去重键 = "skill://<name>"，已存在则 upsert
    pub fn ensure_skill_documented(&self, record: &SkillRecord) -> Result<String, SkillError> { /* §5.2 */ }

    /// 生成本会话的 preamble Tier 1 片段（<available_skills> XML）
    /// 入参：会话 id（用于查 conversation_skills.enabled 状态）
    /// 逻辑：见 §3.6 的 preamble 拼接规则
    pub fn build_preamble_section(&self, conversation_id: &str) -> Result<String, SkillError> { /* §5.3 */ }
}
```

### 3.6 Preamble 拼接规则（核心逻辑）

> **⚠️ 性能注意（实现时必须处理）**：`discover_all()` 每次调用都做磁盘 `read_dir` + Govcraft `SkillDirectory::load` 解析，而 `build_preamble_section`（§5.3）和 `active_skill_doc_paths`（§5.4）**都调 `discover_all()`**——意味着**每次发消息会扫描两遍磁盘**。内置 skill 目录通常只有 3-5 个，扫描成本低，但跨客户端目录可能很多。
>
> **解决方案**：`SkillManager` 内部加 `cached_discovery: std::sync::Mutex<Option<(Instant, Vec<SkillRecord>)>>` 字段，TTL 缓存（如 60 秒）。导入/卸载 skill 时调 `invalidate_cache()` 清空。实现示例：
> ```rust
> fn discover_all_cached(&self) -> Vec<SkillRecord> {
>     let guard = self.cached_discovery.lock().unwrap();
>     if let Some((at, records)) = guard.as_ref() {
>         if at.elapsed() < std::time::Duration::from_secs(60) {
>             return records.clone();
>         }
>     }
>     drop(guard);
>     let fresh = self.discover_all();
>     *self.cached_discovery.lock().unwrap() = Some((std::time::Instant::now(), fresh.clone()));
>     fresh
> }
> ```
> `build_preamble_section` 和 `active_skill_doc_paths` 改调 `discover_all_cached()`。一期可先不做缓存（内置 skill 少），但文档记录此性能点，二期优化。

`build_preamble_section(conversation_id)` 的判断流程（对每个已发现的 skill）：

```
1. 若 frontmatter.disable_model_invocation == true
     → 跳过自动 preamble（层次 1，作者声明）
     → 但仍入库 documents，供 @skillName 显式调 read_document

2. 若 skill_name 在 disabled_skills 表
     → 跳过（层次 2，全局禁用）

3. 按 source 判断默认行为：
     Builtin / ExternalReadOnly:
       默认进 preamble
       除非 conversation_skills 有该会话 + skill_name 且 enabled == 0（层次 3 会话级 disable）
     Imported:
       默认不进 preamble
       除非 conversation_skills 有该会话 + skill_name 且 enabled == 1（层次 3 会话级激活）

4. 所有 skill（无论是否进 preamble）都入库 documents
   → 模型可随时 read_document(doc_id) 读全文（供 @skillName 显式调）
```

生成的 XML 格式（参照 Govcraft CLI 的 `to_prompt` 命令，[agentskills.io 规范](https://agentskills.io/integrate-skills)）：

```xml
<available_skills>
<skill>
<name>onto-studio-federation</name>
<description>联邦查询向导：引导用户注册数据源、建模本体、执行 TextQL 查询。当用户询问跨数据源查询或本体设计时使用。</description>
<location>用 read_document 工具读取（doc_id 在挂载文档列表中查找 name=onto-studio-federation）</location>
</skill>
<skill>
<name>pdf-ingest-best-practice</name>
<description>PDF 摄取最佳实践：说明 onto-studio 的 PDF 解析能力边界（中文 CID CMap、扫描件、表格）与推荐工作流。</description>
<location>同上</location>
</skill>
</available_skills>
```

> **location 字段说明**：agentskills.io 规范的 location 通常是文件路径，但 onto-studio 的 skill body 入库 documents 表，模型通过 `read_document(id)` 读取。location 写成提示性文字，引导模型先 `list_documents` 找到 name 匹配的 doc 再 `read_document`。这与决策 17 的 `@` 挂载注脚机制一致。

### 3.7 与 chat.rs 的集成

**现状**：`chat.rs` 的 `stream_with_memory`（第 286 行起）已有动态注入 doc_tools + fed_tools + MCP 工具的逻辑，但**目前没有使用 preamble**——builder 从未调 `.preamble()`。Skill 系统是 onto-studio **首次引入 preamble 机制**，需两处改动：

1. `crates/agent-core/src/provider.rs` 的 `ProviderConfig` 新增 `preamble: Option<String>` 字段（系统人设，如"你是 onto-studio 助手..."，用户在设置页配置）
2. `crates/agent-core/src/chat.rs` 的 **`ChatService`**（chat.rs:146，文档旧版误作 `ChatHandle`）新增 `skill_manager: Option<Arc<SkillManager>>` 字段，并加 `set_skill_manager(&mut self, sm: Arc<SkillManager>)` 方法（仿照现有 `set_federation`/`set_memory` 模式，chat.rs:233）
3. `src-tauri/src/state.rs` 的 `AppState` 新增 `skill_manager: Arc<SkillManager>` 字段（与 `memory`/`tool_handle`/`federation` 并列，state.rs:21），setup hook 构造后写入；ChatService 在 setup 中通过 `set_skill_manager` 注入

**集成代码**（`stream_with_memory` 中，builder 构建处。**注意：OpenAi 和 Anthropic 两个分支重复构建 builder，preamble 注入逻辑需在两处都加，或提取为公共方法**）：

```rust
// chat.rs stream_with_memory 中，构建 builder 后、build() 前。
// ⚠️ 此段在 DynClient::OpenAi 和 DynClient::Anthropic 两个分支都要加
//    （现有代码两个分支结构一致，建议提取 build_agent_builder 辅助函数避免重复）
let mut builder = c.agent(&self.config.model);

// ① 注入 preamble（系统人设 + Skill Tier 1）
//    onto-studio 首次引入 preamble：系统人设来自 ProviderConfig，
//    skill 段来自 SkillManager（若有）
let base_preamble = self.config.preamble.as_deref().unwrap_or("");
let preamble = if let Some(sm) = &self.skill_manager {
    match sm.build_preamble_section(&conv_id) {
        Ok(section) if !section.is_empty() && !base_preamble.is_empty() => {
            format!("{}\n\n{}", base_preamble, section)
        }
        Ok(section) if !section.is_empty() => section,
        _ => base_preamble.to_string(),
    }
} else {
    base_preamble.to_string()
};
if !preamble.is_empty() {
    builder = builder.preamble(&preamble);  // rig AgentBuilder::preamble(&str)，已核实存在
}

// ② 注入 memory / reasoning_params（已有逻辑，保持不变）
if let Some(m) = &memory {
    builder = builder.memory(m.clone());
}
if let Some(p) = reasoning_params {
    builder = builder.additional_params(p);
}

// ③ Skill doc_paths 合并到 doc_paths_set（在构建 builder 之前，与 doc_tools 注入同步）
//    当前 doc_paths_set 是 Arc<HashSet>（不可变），需改为可变 Vec 或重建
//    见下方说明
```

**doc_paths_set 合并的注意点**：现有 `doc_paths_set` 是 `Arc<HashSet<String>>`（第 321 行），用于过滤 `document_tools`。Skill 的 `active_skill_doc_paths()` 返回 `Vec<String>`（`skill://<name>` 格式），需在构建 `doc_paths_set` 后、调用 `document_tools()` 前合并：

```rust
// 在现有 doc_paths_set 构建后（chat.rs 约 318 行），插入 skill doc_paths：
let mut doc_paths: Vec<String> = /* 现有 resolve_active_doc_paths 结果 */;
if let Some(sm) = &self.skill_manager {
    if let Ok(skill_paths) = sm.active_skill_doc_paths(&conv_id) {
        doc_paths.extend(skill_paths);
    }
}
let doc_paths_set = Arc::new(doc_paths.into_iter().collect::<std::collections::HashSet<_>>());
```

**关键点**：
- Skill preamble 拼在系统人设后面，**不破坏 prefix cache**（系统人设前缀不变，skill 段是后缀追加）
- Skill body 入库 documents 后，path（`skill://<name>`）加入 `doc_paths_set`，`read_document` 工具即可读取——**与文件检索统一路径**（决策 17 的核心精神）
- `AgentBuilder::preamble(&str)` 在 rig-agent 0.41 的 `builder.rs:205` 已核实存在
- **preamble 是 onto-studio 首次引入**：之前不用系统人设，Skill 系统倒逼加上。若一期暂不加用户可配置的系统人设，`ProviderConfig.preamble` 可先硬编码默认值或留 None

### 3.8 `@skillName` 显式激活

**现状澄清**：onto-studio 的 `@` 挂载解析**在前端**完成（`src/lib/mention.ts` 的 `resolveMentionedPaths`），后端 `send_message` 命令收到的 `mounted_paths: Vec<String>` 是已解析的文档 path 列表（`src-tauri/src/commands/chat.rs:89`）。前端 `Composer.tsx:122` 调 `resolveMentionedPaths(content, mountedDocs)` 从消息文本提取 `@name` token，查 `useMountedDocuments` 返回的 name→path 映射得到 path。

**Skill 的 `@skillName` 需扩展这条链路**（不能直接复用，因为 skill 的 path 是虚拟的 `skill://<name>`，不在 `documents` 表的常规文件里）：

1. **前端 `useMountedDocuments` 扩展**：除常规摄入文档外，额外查询 `list_skills` 命令返回的 skill 列表，把 skill name → `skill://<name>` 加入映射。这样 `resolveMentionedPaths` 的正则 `/@([^\s@]+)/g` 能匹配到 `@skillName`
2. **前端 `MentionMenu.tsx`**（Composer 触发的自动补全菜单）：把 skill name 加入补全候选（与文件名混排，可用图标区分来源）
3. **后端 `send_message`**：收到 `mounted_paths` 后，识别其中 `skill://` 前缀的条目 → 在 `conversation_skills` 插入 `(conversation_id, skill_name, source, enabled=1)` → skill 的 doc_id 加入本会话 `doc_paths_set`
4. **`<mounted-documents>` 注脚**：现有逻辑（`commands/chat.rs:275`）对每个 mounted_ref 追加 `<document id="..." name="..."/>`。skill 的注脚格式相同（skill 已入库 documents 表，有 id+name），模型按注脚调 `read_document(id)` 读全文

> **关键**：skill 的 `@` 激活**复用的是后端注脚机制 + 前端正则解析**，但前端的 name→path 映射源需扩充（加 skill 列表），后端的 path 识别需加 `skill://` 前缀分支。这不是“零改动复用”，是“同模式扩展”。一期若赶进度，可先不做 `@skillName` 自动补全，只靠 Inspector 面板勾选激活（§7.3 第二个入口）。

---

## 4. 落地实施

### 4.1 依赖与配置

**`crates/agent-core/Cargo.toml`** 新增：
```toml
[dependencies]
agent-skills = "0.2"   # Govcraft，MIT/Apache-2.0。遗留 L1：serde_yml RUSTSEC
```

**`src-tauri/tauri.conf.json`** 的 `bundle.resources` 新增：
```json
"resources": [
  "resources/pdfium",
  "resources/skills"
]
```

**遗留 L1 跟踪**（`PROGRESS.md` 待办 + `ARCHITECTURE.md` 决策 20 已知问题）：
```
- [ ] L1: agent-skills 0.2 依赖 serde_yml 0.0.12（RUSTSEC-2025-0068）
      发版前 patch 为 serde_yaml_ng 0.10（Cargo.lock 已有，3 行改动）
      或等 Govcraft 上游合并 PR
```

### 4.2 目录路径解析（src-tauri setup hook）

**现状**：`AppState::new(db_path)`（state.rs:52）只接受 db_path，构造时 `chat: None`、`federation: None`。skill_manager 无法在 `AppState::new` 里构造（需先有 memory）。参考现有 federation 的初始化模式——`AppState::new` 先返回基础状态，setup hook 后续异步赋值。

在 `src-tauri/src/lib.rs` 的 setup hook 中，`AppState::new` 完成后、`restore_provider` 之前新增：

```rust
// src-tauri/src/lib.rs setup hook（在 AppState::new 之后）
// 现有代码：let state = AppState::new(db_path)?; app.manage(state);
// 改为：先构造 state，再注入 skill_manager，最后 manage

let mut state = AppState::new(db_path)?;

// Skill 系统初始化（决策 20）：
// 1. 解析各 skill 目录路径
let builtin_skills_dir = skill::builtin_dir(app.handle());  // resource_dir/skills/
let user_skills_dir = skill::user_dir();                     // ~/.onto-studio/skills/
let external_skill_dirs = skill::external_dirs();             // ~/.agents/skills/ 等

// 2. 确保用户导入目录存在
if let Err(e) = std::fs::create_dir_all(&user_skills_dir) {
    tracing::warn!(error = %e, dir = %user_skills_dir.display(), "create user skills dir failed");
}

// 3. 构造 SkillManager 并注入 AppState（AppState 需新增 skill_manager 字段）
let skill_manager = std::sync::Arc::new(
    agent_core::skill::SkillManager::new(
        state.memory.clone(),  // AppState 已有 Arc<Memory>
        builtin_skills_dir,
        user_skills_dir,
        external_skill_dirs,
    )
);
state.skill_manager = skill_manager.clone();

// 4. 注入 ChatService（若已恢复 provider 构造了 chat）
//    注意：tokio::sync::RwLock 的 write() 是 async，需 .await
//    若 provider 在 setup 后才恢复，此处 chat 可能还是 None——
//    应在 restore_provider 构造 ChatService 后同步调 set_skill_manager
if let Some(chat) = state.chat.write().await.as_mut() {
    chat.set_skill_manager(skill_manager);
}

app.manage(state);  // 最后才 manage
```

> **注意 provider 恢复时机**：现有 `restore_provider`（lib.rs setup 后半段）构造 ChatService 时需同步调 `chat.set_skill_manager(skill_manager.clone())`。若 skill_manager 在 restore_provider 之后才构造，则需在 restore_provider 内拿 AppState 的 skill_manager 字段注入。建议顺序：① AppState::new → ② 构造 skill_manager 赋值给 state → ③ restore_provider 构造 ChatService 时从 state 取 skill_manager 注入。

**路径解析辅助函数**（`src-tauri/src/skill.rs`，复用 pdfium.rs 的三层兜底模式）：

```rust
// src-tauri/src/skill.rs
use tauri::{Manager, AppHandle};
use tauri::path::BaseDirectory;
use std::path::PathBuf;

/// 内置 skill 目录：resource_dir/skills/（dev = src-tauri/resources/skills/）
/// 复用 pdfium.rs 的三层兜底（Resource → resource_dir/resources → CARGO_MANIFEST_DIR）
pub fn builtin_dir(app: &AppHandle) -> PathBuf {
    app.path().resolve("skills", BaseDirectory::Resource)
        .ok().filter(|p| p.exists())
        .or_else(|| app.path().resource_dir().ok()
            .map(|d| d.join("resources").join("skills"))
            .filter(|p| p.exists()))
        .or_else(|| {
            let manifest = env!("CARGO_MANIFEST_DIR");
            Some(PathBuf::from(manifest).join("resources").join("skills"))
                .filter(|p| p.exists())
        })
        .expect("builtin skills dir not found")
}

/// 用户导入目录：~/.onto-studio/skills/
/// 对应 ~/.claude/skills/、~/.pi/agent/skills/（业界点目录规范）
pub fn user_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
        .join(".onto-studio").join("skills")
}

/// 跨客户端只读扫描目录
pub fn external_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    vec![
        home.join(".agents").join("skills"),        // 跨客户端约定
        home.join(".claude").join("skills"),         // Claude Code
        home.join(".pi").join("agent").join("skills"), // pi
    ]
}
```

> **`dirs` crate**：需在 `src-tauri/Cargo.toml` 加 `dirs = "6"`（MIT/Apache-2.0，纯 Rust 无原生依赖）。或用 `std::env::var("HOME")` 手写避免新依赖（Windows 需特殊处理 `USERPROFILE`，建议用 `dirs`）。

### 4.3 内置 Skill 编写

**目录结构**（`src-tauri/resources/skills/`）：
```
src-tauri/resources/skills/
├── onto-studio-federation/
│   ├── SKILL.md
│   └── references/
│       └── sql-templates.md       # TextQL 生成的 SQL 模板示例
├── onto-studio-ontology/
│   └── SKILL.md
└── onto-studio-ingest/
    └── SKILL.md
```

**SKILL.md 模板**（`onto-studio-federation/SKILL.md`）：
```markdown
---
name: onto-studio-federation
description: 联邦查询向导。引导用户注册数据源（MySQL/PG/CSV/Excel）、设计本体（ObjectType/LinkType/ActionType）、执行 TextQL 自然语言转 SQL 查询。当用户询问跨数据源查询、数据源注册、本体建模或 TextQL 时使用。
license: MIT
---

# 联邦查询向导

## 何时使用本 Skill

- 用户想查询 MySQL/PostgreSQL/CSV/Excel 数据源
- 用户提到"本体""ObjectType""LinkType""数据建模"
- 用户用自然语言描述查询意图，需转 SQL

## 工作流

1. **确认数据源**：用 list_data_sources 工具查看已注册数据源
   - 若用户的数据源未注册，引导其在前端"数据源管理"页注册
2. **理解本体**：用 federation_query 工具查询已有 ObjectType/LinkType
3. **构造查询**：将用户自然语言意图转为 SQL（遵循 references/sql-templates.md 的模式）
4. **执行**：用 federation_query 执行 SQL，返回结果

## 注意事项

- onto-studio 用 DataFusion 联邦查询，支持 MySQL/PG/CSV/Excel
- TextQL 是 NL→SQL 编译器，见 references/sql-templates.md 的生成模式
- 本体存储复用 memory SQLite 的 ontology 表族（决策见 ARCHITECTURE.md 三期）
```

> **内置 skill 的 name 必须等于目录名**（Govcraft `SkillDirectory::load` 强校验，agentskills.io 规范要求）。

### 4.4 文件清单（新建/修改）

| 文件 | 操作 | 内容 |
|---|---|---|
| `crates/agent-core/Cargo.toml` | 修改 | 加 `agent-skills = "0.2"`、`ingest = { path = "../ingest" }`、`zip = "9.0.0-pre2"` |
| `crates/agent-core/src/skill/mod.rs` | 新建 | 模块入口 + SkillRecord/SkillSource/SkillError |
| `crates/agent-core/src/skill/manager.rs` | 新建 | SkillManager（discover/ensure_documented/build_preamble） |
| `crates/agent-core/src/skill/activate.rs` | 新建 | 会话激活 + disable 三层判断 + active_skill_doc_paths |
| `crates/agent-core/src/skill/prompt.rs` | 新建 | preamble XML 生成（手写极简 XML 转义，零依赖） |
| `crates/agent-core/src/skill/builtin.rs` | 新建 | 内置 skill frontmatter 扩展（补 disable-model-invocation 解析） |
| `crates/agent-core/src/skill/import.rs` | 新建 | 导入操作（本地目录复制 / zip 解压，复用 ingest::security） |
| `crates/agent-core/src/lib.rs` | 修改 | 加 `pub mod skill;` |
| `crates/agent-core/src/chat.rs` | 修改 | ChatService 加 `skill_manager: Option<Arc<SkillManager>>` 字段 + `set_skill_manager` 方法；stream_with_memory 注入 preamble + doc_paths（**OpenAi/Anthropic 两分支都要改**） |
| `crates/agent-core/src/provider.rs` | 修改 | ProviderConfig 加 `preamble: Option<String>` 字段（首次引入系统人设） |
| `crates/memory/src/lib.rs` | 修改 | init_schema 加 `init_skill_schema(conn)?` 调用 |
| `crates/memory/src/skill_repo.rs` | 新建 | disabled_skills + conversation_skills 两表的 CRUD（参照 documents.rs 风格） |
| `src-tauri/src/state.rs` | 修改 | AppState 加 `skill_manager: Arc<SkillManager>` 字段 |
| `src-tauri/src/skill.rs` | 新建 | 目录路径解析（builtin_dir/user_dir/external_dirs） |
| `src-tauri/src/lib.rs` | 修改 | setup hook 加 skill 初始化 + `collect_commands!` 注册新命令 |
| `src-tauri/src/commands/mod.rs` | 修改 | 加 `pub mod skill;` |
| `src-tauri/src/commands/skill.rs` | 新建 | Tauri 命令（list/import/activate/disable/uninstall） |
| `src-tauri/Cargo.toml` | 修改 | 加 `dirs = "6"` |
| `src-tauri/tauri.conf.json` | 修改 | bundle.resources += "resources/skills" |
| `src-tauri/resources/skills/*/SKILL.md` | 新建 | 3 个内置 skill（federation/ontology/ingest） |

---

## 5. 关键实现细节

### 5.1 discover_all（扫描去重）

```rust
// crates/agent-core/src/skill/manager.rs

impl SkillManager {
    pub fn discover_all(&self) -> Vec<SkillRecord> {
        let mut records: Vec<SkillRecord> = Vec::new();

        // 1. 内置（resource_dir/skills/）
        self.scan_dir(&self.builtin_dir, SkillSource::Builtin, &mut records);
        // 2. 用户导入（~/.onto-studio/skills/）
        self.scan_dir(&self.user_dir, SkillSource::Imported, &mut records);
        // 3. 跨客户端只读
        for dir in &self.external_dirs {
            self.scan_dir(dir, SkillSource::ExternalReadOnly, &mut records);
        }

        // 去重：同名时 Builtin > Imported > ExternalReadOnly
        // （用 HashMap<name, SkillRecord>，后扫到的低优先级 source 跳过已存在的 name）
        let mut deduped: std::collections::HashMap<String, SkillRecord> = std::collections::HashMap::new();
        for r in records {
            let priority = match r.source {
                SkillSource::Builtin => 3,
                SkillSource::Imported => 2,
                SkillSource::ExternalReadOnly => 1,
                SkillSource::Project => 0,
            };
            match deduped.get(&r.name) {
                Some(existing) if priority_of(existing.source) >= priority => {}
                _ => { deduped.insert(r.name.clone(), r); }
            }
        }
        deduped.into_values().collect()
    }

    fn scan_dir(&self, dir: &Path, source: SkillSource, out: &mut Vec<SkillRecord>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };  // 目录不存在则跳过
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            match SkillDirectory::load(&path) {
                Ok(skill_dir) => {
                    let skill = skill_dir.skill();
                    // 补 parse disable-model-invocation（Govcraft 不解析，业务层补）
                    let dmi = parse_disable_model_invocation(&path);
                    out.push(SkillRecord {
                        name: skill.name().as_str().to_string(),
                        description: skill.description().as_str().to_string(),
                        source,
                        dir_path: path,
                        doc_id: None,  // 延迟到 ensure_skill_documented
                        disable_model_invocation: dmi,
                        allowed_tools: skill.frontmatter().allowed_tools()
                            .map(|at| at.iter().map(|s| s.to_string()).collect()),
                        license: skill.frontmatter().license().map(String::from),
                        compatibility: skill.frontmatter().compatibility()
                            .map(|c| c.as_str().to_string()),
                    });
                }
                Err(e) => {
                    tracing::warn!(dir = %path.display(), error = ?e, "skill load failed, skipping");
                }
            }
        }
    }
}
```

### 5.2 ensure_skill_documented（入库 documents）

```rust
impl SkillManager {
    /// 把 skill body 入库 documents 表，path 去重键 = "skill://<name>"
    /// 幂等：已存在则 upsert（skill 内容更新时覆盖）
    pub fn ensure_skill_documented(&self, record: &mut SkillRecord) -> Result<String, SkillError> {
        let skill_dir = SkillDirectory::load(&record.dir_path)
            .map_err(|e| SkillError::Load(e.to_string()))?;
        let body = skill_dir.skill().body();  // SKILL.md 的 Markdown 正文

        // 构造 DocumentRow（实际签名：upsert_document(&self, row: DocumentRow)）
        // path 去重键 = "skill://<name>"，name = skill name，format = "skill-md"
        let row = memory::documents::DocumentRow {
            id: uuid::Uuid::new_v4().to_string(),      // 新 id（upsert 按 path 去重，id 会复用旧的）
            path: format!("skill://{}", record.name),   // 去重键
            name: record.name.clone(),                  // 展示名 = skill name
            format: "skill-md".to_string(),             // format 标记（前端可识别）
            text: body.to_string(),
            char_count: body.len() as u32,
            created_at: memory::Timestamp::now().as_i64(), // unix ms（Timestamp newtype，as_i64 返回 i64）
            folder_path: None,                          // skill 不进文件夹
            source_conv_id: None,                       // 非会话上传来源
        };
        let doc_id = self.memory.upsert_document(row)?;

        record.doc_id = Some(doc_id.clone());
        Ok(doc_id)
    }
}
```

> **已核实**：`upsert_document(&self, row: DocumentRow) -> MemoryResult<String>`（`crates/memory/src/documents.rs:126`），接受 `DocumentRow` 结构体（字段见 `documents.rs:22-34`）。upsert 按 `path` 去重（`ON CONFLICT(path) DO UPDATE`），同 path 会复用旧 id。skill body 入库后会自动建 FTS5 索引（异步，`index_document` spawn_blocking），这样 `search_documents` 也能搜到 skill 内容——这是 bonus，不影响主流程。

### 5.3 build_preamble_section（preamble 拼接）

```rust
// crates/agent-core/src/skill/prompt.rs

use crate::skill::{SkillManager, SkillRecord, SkillSource};
// XML 转义手写极简版（避免引入新依赖，符合 onto-studio 轻量化原则）
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl SkillManager {
    pub fn build_preamble_section(&self, conversation_id: &str) -> Result<String, SkillError> {
        let all = self.discover_all();
        let disabled: std::collections::HashSet<String> = self.memory.list_disabled_skills()?;
        let conv_skills = self.memory.list_conversation_skills(conversation_id)?;

        let mut entries: Vec<(String, String)> = Vec::new();  // (name, description)
        for mut record in all {
            // 层次 1：作者声明 disable-model-invocation
            if record.disable_model_invocation { continue; }
            // 层次 2：全局禁用
            if disabled.contains(&record.name) { continue; }
            // 层次 3：会话级
            let conv_entry = conv_skills.iter().find(|cs| cs.skill_name == record.name);
            match record.source {
                SkillSource::Builtin | SkillSource::ExternalReadOnly => {
                    // 默认进，除非会话级显式 enabled == 0
                    if let Some(cs) = conv_entry {
                        if !cs.enabled { continue; }
                    }
                }
                SkillSource::Imported => {
                    // 默认不进，除非会话级显式 enabled == 1
                    if !conv_entry.map(|cs| cs.enabled).unwrap_or(false) { continue; }
                }
                SkillSource::Project => { /* 二期 */ continue; }
            }
            // 入库 documents（确保 doc_id 存在，供 read_document）
            if record.doc_id.is_none() {
                self.ensure_skill_documented(&mut record)?;
            }
            entries.push((record.name.clone(), record.description.clone()));
        }

        if entries.is_empty() {
            return Ok(String::new());  // 无可用 skill，不追加 preamble
        }

        // 生成 <available_skills> XML
        let mut xml = String::from("<available_skills>\n");
        for (name, desc) in &entries {
            xml.push_str(&format!(
                "<skill>\n<name>{}</name>\n<description>{}</description>\n<location>用 read_document 工具读取（先 list_documents 找 name={}]）</location>\n</skill>\n",
                escape_xml(name), escape_xml(desc), escape_xml(name)
            ));
        }
        xml.push_str("</available_skills>");
        Ok(xml)
    }
}
```

### 5.4 active_skill_doc_paths（供 read_document 读取）

```rust
// crates/agent-core/src/skill/activate.rs

impl SkillManager {
    /// 返回本会话应加入 doc_paths_set 的 skill doc path（"skill://<name>"）
    /// 包括：进 preamble 的 skill + @skillName 显式激活的 skill
    /// （disable-model-invocation 的 skill 不进 preamble，但 @激活后也要能读）
    pub fn active_skill_doc_paths(&self, conversation_id: &str) -> Result<Vec<String>, SkillError> {
        let all = self.discover_all();
        let disabled = self.memory.list_disabled_skills()?;
        let conv_skills = self.memory.list_conversation_skills(conversation_id)?;

        let mut paths = Vec::new();
        for mut record in all {
            // 全局禁用的不提供（层次 2 优先）
            if disabled.contains(&record.name) { continue; }

            let conv_entry = conv_skills.iter().find(|cs| cs.skill_name == record.name);
            let should_include = if record.disable_model_invocation {
                // 层次 1：只能 @显式激活（会话级 enabled == 1）
                conv_entry.map(|cs| cs.enabled).unwrap_or(false)
            } else {
                match record.source {
                    SkillSource::Builtin | SkillSource::ExternalReadOnly => {
                        conv_entry.map(|cs| cs.enabled).unwrap_or(true)  // 默认 true
                    }
                    SkillSource::Imported => {
                        conv_entry.map(|cs| cs.enabled).unwrap_or(false)  // 默认 false
                    }
                    SkillSource::Project => false,
                }
            };

            if should_include {
                if record.doc_id.is_none() {
                    self.ensure_skill_documented(&mut record)?;
                }
                paths.push(format!("skill://{}", record.name));
            }
        }
        Ok(paths)
    }
}
```

### 5.5 disable-model-invocation 解析（补 Govcraft 缺失）

Govcraft 的 `RawFrontmatter`（`skill.rs` 内部）不解析 `disable-model-invocation` 字段。onto-studio 业务层补：

```rust
// crates/agent-core/src/skill/builtin.rs

use std::path::Path;
use std::fs;

/// 解析 SKILL.md frontmatter 的 disable-model-invocation 字段
/// Govcraft 0.2 不解析此字段，业务层补（极简正则，避免引入 YAML 依赖）
pub fn parse_disable_model_invocation(skill_dir: &Path) -> bool {
    let skill_md = skill_dir.join("SKILL.md");
    let Ok(content) = fs::read_to_string(&skill_md) else { return false; };
    // 只解析 frontmatter 段（第一个 --- 到第二个 ---）
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))
        else { return false; };
    let Some(end) = rest.find("\n---").or_else(|| rest.find("\r\n---"))
        else { return false; };
    let frontmatter = &rest[..end];
    // 极简匹配：disable-model-invocation: true
    frontmatter.lines().any(|line| {
        let line = line.trim();
        line == "disable-model-invocation: true" || line == "disable-model-invocation: TRUE"
    })
}
```

> **为什么不复用 Govcraft 的 YAML 解析**：Govcraft 的 `RawFrontmatter` 是私有的，无法扩展。手写极简解析（只查一个布尔字段）约 15 行，零新依赖，符合 onto-studio 轻量化原则。若未来 Govcraft 上游加了此字段，可移除这层。

### 5.6 导入操作（本地目录 / zip）

```rust
// crates/agent-core/src/skill/import.rs

use std::path::PathBuf;
use std::fs;
use agent_skills::SkillDirectory;

impl SkillManager {
    /// 导入本地 skill 目录：复制到 ~/.onto-studio/skills/<name>/
    /// 先用 Govcraft 校验合法性（SKILL.md 存在 + name 匹配目录名）
    pub fn import_from_dir(&self, src_dir: &Path) -> Result<String, SkillError> {
        // 1. 用 Govcraft 校验源目录是合法 skill
        let skill_dir = SkillDirectory::load(src_dir)
            .map_err(|e| SkillError::InvalidSkill(e.to_string()))?;
        let name = skill_dir.skill().name().as_str().to_string();

        // 2. 目标路径：~/.onto-studio/skills/<name>/
        let dest = self.user_dir.join(&name);
        if dest.exists() {
            return Err(SkillError::AlreadyExists(name));
        }

        // 3. 递归复制（含 scripts/references/assets 子目录）
        copy_dir_recursive(src_dir, &dest)?;

        Ok(name)
    }

    /// 导入 zip：解压到临时目录 → 校验 → 复制到 user_dir
    /// 复用 ingest::security 的 zip 炸弹防护（check_size + ArchiveBudget）
    /// 解压逻辑自建（ingest 无现成 extract_zip_safe，只有防护原语）
    pub fn import_from_zip(&self, zip_path: &Path) -> Result<String, SkillError> {
        use ingest::security::{check_size, ArchiveBudget};
        use zip::ZipArchive;
        use std::fs::File;

        // 1. 防炸弹：校验压缩包大小（check_size 在 ingest::security）
        check_size(zip_path).map_err(|e| SkillError::ZipBomb(e.to_string()))?;

        // 2. 解压到临时目录，用 ArchiveBudget 累计展开字节防炸弹
        let temp = std::env::temp_dir().join(format!("onto-skill-import-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).map_err(|e| SkillError::Io(e.to_string()))?;
        let file = File::open(zip_path).map_err(|e| SkillError::Io(e.to_string()))?;
        let mut archive = ZipArchive::new(file).map_err(|e| SkillError::Zip(e.to_string()))?;
        let mut budget = ArchiveBudget::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| SkillError::Zip(e.to_string()))?;
            let outpath = temp.join(entry.mangled_name());
            if entry.is_dir() {
                std::fs::create_dir_all(&outpath).map_err(|e| SkillError::Io(e.to_string()))?;
            } else {
                std::fs::create_dir_all(outpath.parent().unwrap_or(&temp))
                    .map_err(|e| SkillError::Io(e.to_string()))?;
                let mut outfile = File::create(&outpath).map_err(|e| SkillError::Io(e.to_string()))?;
                let bytes = std::io::copy(&mut entry, &mut outfile).map_err(|e| SkillError::Io(e.to_string()))?;
                budget.account(bytes).map_err(|e| SkillError::ZipBomb(e.to_string()))?;
            }
        }

        // 3. zip 可能是 "skill-name/SKILL.md" 或直接 "SKILL.md"
        let skill_root = if temp.join("SKILL.md").exists() {
            temp.clone()
        } else {
            find_skill_subdir(&temp).ok_or(SkillError::NoSkillInZip)?
        };

        // 4. 复用 import_from_dir
        let name = self.import_from_dir(&skill_root)?;

        // 5. 清理临时目录
        let _ = fs::remove_dir_all(&temp);
        Ok(name)
    }

    /// 卸载导入的 skill：删除 ~/.onto-studio/skills/<name>/
    /// 同时清理 disabled_skills + conversation_skills 中的记录
    /// 内置/external-readonly 不可卸载
    pub fn uninstall(&self, skill_name: &str) -> Result<(), SkillError> {
        let dir = self.user_dir.join(skill_name);
        if !dir.exists() {
            return Err(SkillError::NotImported(skill_name.to_string()));
        }
        fs::remove_dir_all(&dir).map_err(|e| SkillError::Io(e.to_string()))?;
        // 清理 DB 记录
        self.memory.remove_skill_records(skill_name)?;
        Ok(())
    }
}

/// 递归复制目录（std 没有提供，手写）
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
```

> **zip 安全解压**：复用 `crates/ingest/src/security.rs` 的 `check_size(path)`（预校验压缩包大小）+ `ArchiveBudget::new().account(bytes)`（累计展开字节防炸弹）。**ingest 没有现成的 `extract_zip_safe` 函数**，只有防护原语——解压循环需在 `import.rs` 自建（如上代码所示）。zip crate 是 `zip = "9.0.0-pre2"`（ingest 已依赖，`agent-core` 需在 Cargo.toml 加 `zip = "9.0.0-pre2"` + `ingest = { path = "../ingest" }`）。

---

## 6. Tauri 命令层（IPC 契约）

### 6.1 命令清单

在 `src-tauri/src/commands/skill.rs` 新增（全部加 `#[derive(specta::Type)]`）：

```rust
// src-tauri/src/commands/skill.rs
use agent_core::skill::{SkillRecord, SkillSource};
use serde::{Deserialize, Serialize};
use specta::specta_type;  // 见 BigInt 公约

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SkillDto {
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub disable_model_invocation: bool,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    /// 是否已在本会话激活（会话级）
    pub conversation_enabled: Option<bool>,
    /// 是否全局禁用
    pub globally_disabled: bool,
}

#[tauri::command]
#[specta::specta]
pub async fn list_skills(
    state: tauri::State<'_, AppState>,
    conversation_id: Option<String>,
) -> Result<Vec<SkillDto>, String> {
    // 通过 state.skill_manager 扫描 + 合并激活状态
}

#[tauri::command]
#[specta::specta]
pub async fn import_skill_from_dir(
    state: tauri::State<'_, AppState>,
    src_path: String,
) -> Result<String, String> {
    state.skill_manager.import_from_dir(std::path::Path::new(&src_path)).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn import_skill_from_zip(
    state: tauri::State<'_, AppState>,
    zip_path: String,
) -> Result<String, String> {
    state.skill_manager.import_from_zip(std::path::Path::new(&zip_path)).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn uninstall_skill(
    state: tauri::State<'_, AppState>,
    skill_name: String,
) -> Result<(), String> {
    state.skill_manager.uninstall(&skill_name).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_skill_conversation_enabled(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    skill_name: String,
    enabled: bool,
) -> Result<(), String> {
    state.skill_manager.set_conversation_enabled(&conversation_id, &skill_name, enabled).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_skill_globally_disabled(
    state: tauri::State<'_, AppState>,
    skill_name: String,
    disabled: bool,
) -> Result<(), String> {
    state.skill_manager.set_globally_disabled(&skill_name, disabled).map_err(|e| e.to_string())
}
```

**BigInt 公约**：以上 DTO 无裸 64 位整数字段，无需 `#[specta(type = Number)]` 注解。`SkillSource` 枚举用 `#[serde(rename_all = "kebab-case")]` 导出为 TS 字符串字面量联合类型。

### 6.2 命令注册与绑定生成

**命令注册机制**（已核实 `src-tauri/src/lib.rs:40`）：onto-studio 用 **`tauri_specta::Builder` + `collect_commands!` 宏**，不是 `tauri::generate_handler!`。新命令需加到 `lib.rs` 的 `collect_commands![...]` 列表末尾：

```rust
// src-tauri/src/lib.rs（现有 builder 构造处，约第 40 行）
let builder = tauri_specta::Builder::<tauri::Wry>::new().commands(collect_commands![
    // ... 现有命令（create_conversation, send_message, ...）...
    execute_federation_query,
    explain_federation_query,
    // ↓ 新增 skill 命令（本批）
    list_skills,
    import_skill_from_dir,
    import_skill_from_zip,
    uninstall_skill,
    set_skill_conversation_enabled,
    set_skill_globally_disabled,
]);
```

同时需在 `lib.rs` 顶部的 `use commands::{...}` 导入块（约第 27 行）加入 `skill::{list_skills, import_skill_from_dir, ...}`，并在 `src-tauri/src/commands/mod.rs` 注册 `pub mod skill;`。

**绑定生成**：运行 `ONTO_GEN_BINDINGS=1 cargo run` 重新生成 TS 绑定到 `src/ipc/`。⚠️ 注意 grep warning（AGENTS.md 工作守则 7）。

**AppState 注入**：skill 命令通过 `tauri::State<'_, AppState>` 取 `skill_manager`（而非文档旧版写的 `State<'_, Arc<SkillManager>>`——AppState 是统一的 state 容器，`app.manage(AppState::new(...))` 已注册，skill_manager 是其字段）。

---

## 7. 前端（src/）指引

### 7.1 技术约束（遵守 AGENTS.md）

- **状态三层分离**：skill 列表用 TanStack Query（服务端状态）；激活开关用 Zustand（全局 UI）；导入对话框用 useState（组件本地）
- **四层依赖**：`components/skills/` 不得直接 `invoke`，走 `ipc/skills.ts`（自动生成的绑定）
- **持久化**：skill 数据走 Rust SQLite；UI 偏好（如展开状态）用 `@tauri-store/zustand`，禁用 localStorage

### 7.2 建议组件结构

```
src/
├── ipc/
│   └── skills.ts                 # tauri-specta 自动生成
├── state/
│   └── useSkills.ts              # TanStack Query: useSkillsQuery / useImportSkill / useToggleSkill
├── components/
│   └── skills/
│       ├── SkillList.tsx         # 列表（含来源图标、disable 开关）
│       ├── SkillCard.tsx         # 单个 skill 卡片（name/desc/source/开关）
│       ├── SkillImportDialog.tsx # 导入对话框（选目录/选 zip）
│       ├── SkillDetailDrawer.tsx # 详情抽屉（SKILL.md 预览、frontmatter、allowed-tools）
│       └── SkillInspectorPanel.tsx # 会话内 Inspector 的 skill 面板
└── ...
```

### 7.3 三个入口的 UI

| 入口 | 组件 | 操作 |
|---|---|---|
| 设置页 > Skills | `SkillList` + `SkillCard` | 全局 enable/disable、导入、卸载、查看详情 |
| 会话内 > Inspector > Skills | `SkillInspectorPanel` | 会话级 enable/disable、`@skillName` 提示 |
| 会话输入框 `@skillName` | 复用现有 `@` 挂载的自动补全 | skill name 加入 `@` 补全候选 |

---

## 8. 测试策略

### 8.1 crates 层单元测试（平台无关，cargo test）

| 测试 | 位置 | 验证点 |
|---|---|---|
| frontmatter 解析 | `crates/agent-core/src/skill/builtin.rs` | `parse_disable_model_invocation` 正确识别 true/false/缺失 |
| 扫描去重 | `crates/agent-core/src/skill/manager.rs` | 同名 skill 按优先级 Builtin>Imported>External 去重 |
| preamble 生成 | `crates/agent-core/src/skill/prompt.rs` | 三层 disable 逻辑正确、XML 格式合规、空列表返回空串 |
| 导入/卸载 | `crates/agent-core/src/skill/import.rs` | 目录复制、zip 解压、卸载清理、重复导入报错 |
| 入库 documents | `crates/agent-core/src/skill/manager.rs` | `skill://<name>` 去重键、upsert 幂等、read_document 可读 |

测试用临时目录 + `Memory::open_in_memory()`，参照现有 `crates/agent-core/tests/` 风格。

### 8.2 src-tauri 层（cargo check --manifest-path src-tauri/Cargo.toml）

> ⚠️ **编译检查必须覆盖 src-tauri**（AGENTS.md 工作守则 7）：`cargo check --workspace` 只检查 `crates/*`，src-tauri 需单独检查。`ONTO_GEN_BINDINGS=1 cargo run` 的 warning 易被截断，需显式 grep warning。

### 8.3 内置 skill 兼容性自测

参照 AGENTS.md 工作守则 6，3 个内置 skill 需写兼容性自测（`crates/agent-core/tests/skill_builtin.rs`）：
- 每个 SKILL.md 能被 `SkillDirectory::load` 成功加载
- name 匹配目录名
- description 非空且 ≤1024 字符
- references/ 子目录的文件可 `read_reference` 读取

---

## 9. 分期路线

### 一期（本次实施）
- [ ] 引入 `agent-skills = "0.2"`（遗留 L1 记录）
- [ ] `crates/memory`：新增 `disabled_skills` + `conversation_skills` 两表 + `skill_repo.rs`
- [ ] `crates/agent-core/src/skill/`：mod/manager/activate/prompt/builtin/import 六模块
- [ ] `chat.rs` 集成：preamble 注入 + doc_paths 合并
- [ ] `src-tauri`：setup hook 初始化 + 目录解析 + Tauri 命令层
- [ ] 3 个内置 skill（federation/ontology/ingest）
- [ ] 前端：SkillList + SkillCard + 导入对话框 + Inspector 面板
- [ ] 测试：crates 单测 + src-tauri 编译检查 + 内置 skill 兼容性自测

### 二期
- [ ] GitHub skill 仓库安装（`git clone`，需评估 `gix` vs 系统 git，原则 1）
- [ ] 项目级 skill 扫描（需先定义 onto-studio 的"工作区"概念）
- [ ] Skill 编辑器（GUI 编辑 SKILL.md，imported source 可编辑）
- [ ] L1 修复：patch serde_yml → serde_yaml_ng

### 三期（可选）
- [ ] Skill 市场（浏览 anthropics/skills、badlogic/pi-skills 一键安装）
- [ ] Skill 版本管理（导入 skill 的升级/回退）
- [ ] 与 MCP server 的 skill 联动（可执行 skill 作为 MCP 接入）

---

## 10. 风险与注意事项

### 10.1 已知风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| **L1: serde_yml RUSTSEC** | `cargo audit` 告警 | 发版前 patch serde_yaml_ng（Cargo.lock 已有，3 行改动） |
| **Govcast API 变动** | 0.2.0 是唯一发布版，0.3+ 可能改 API | Cargo.lock 锁定 0.2.0；升级前重跑兼容性自测 |
| **skill body 入库 FTS5 索引耗时** | 大 skill 的 jieba 分词慢 | 复用 documents 的异步索引（`index_document` spawn_blocking），不阻塞扫描 |
| **`discover_all()` 重复扫描** | `build_preamble` + `active_doc_paths` 各调一次，每发消息扫描两遍磁盘 | §3.6 加 TTL 缓存（60s），导入/卸载时 invalidate；一期可先不做 |
| **`@skillName` 需扩展前端链路** | skill path 是虚拟 `skill://<name>`，不在常规 documents 表，现有 `resolveMentionedPaths` 查不到 | §3.8：扩展 `useMountedDocuments` 加 skill 映射 + 后端识别 `skill://` 前缀；一期可暂不做，靠 Inspector 勾选 |
| **跨客户端 skill 不可写** | 用户想"编辑"external-readonly skill | UI 禁用编辑按钮；引导用户"导入到 onto-studio"（复制到 user_dir） |

### 10.2 必须遵守的约束

1. **业务逻辑在 `crates/`**：SkillManager 全部在 `crates/agent-core`，`src-tauri` 只做薄封装（AGENTS.md 原则 4）
2. **不引入 embedding**：preamble 拼接是确定逻辑（扫描+查表），不涉及向量检索（原则 2）
3. **许可证核实**：`agent-skills` MIT/Apache-2.0 ✅、`dirs` MIT/Apache-2.0 ✅；未来引入 `gix` 需核实（二期）
4. **IPC 类型安全**：所有 Tauri 命令加 `#[derive(specta::Type)]`，禁手写 TS 类型（AGENTS.md 前端约束）
5. **编译检查覆盖 src-tauri**：用 `cargo check --manifest-path src-tauri/Cargo.toml`，不只 `cargo check --workspace`（AGENTS.md 工作守则 7）

### 10.3 开发顺序建议

1. 先建 `crates/memory` 两表 + `skill_repo.rs`（可独立 cargo test）
2. 再建 `crates/agent-core/src/skill/` 六模块（依赖 memory + agent-skills，可独立 cargo test）
3. 再改 `chat.rs` 集成（依赖 skill 模块）
4. 再建 `src-tauri` 命令层 + setup hook
5. 最后写 3 个内置 skill + 前端

每步完成后 `cargo check` 验证，避免到最后才发现编译错误。

---

## 附录 A：agentskills.io 规范字段速查

| 字段 | 必填 | 约束 | Govcraft 解析 | onto-studio 处理 |
|---|---|---|---|---|
| `name` | ✅ | 1-64 字符，小写字母/数字/连字符，无前后导/连续连字符 | ✅ SkillName 强类型校验 | 去重键、目录名校验 |
| `description` | ✅ | ≤1024 字符 | ✅ SkillDescription 校验 | preamble Tier 1 |
| `license` | ❌ | 字符串 | ✅ Option<String> | 展示用 |
| `compatibility` | ❌ | ≤500 字符 | ✅ Option<Compatibility> | 展示用 |
| `metadata` | ❌ | 任意键值 | ✅ Option<Metadata> | 透传（一期不用） |
| `allowed-tools` | ❌ | 空格分隔 | ✅ Option<AllowedTools> | 展示用（一期不强制） |
| `disable-model-invocation` | ❌ | bool | ❌ Govcraft 不解析 | **业务层补**（§5.5） |

## 附录 B：参考链接

- [agentskills.io 规范](https://agentskills.io/specification)
- [agentskills.io 实现指南](https://agentskills.io/client-implementation/adding-skills-support)
- [pi skills 文档](https://pi.dev/docs/latest/skills)
- [Claude Code skills 文档](https://code.claude.com/docs/en/skills)
- [Govcraft agent-skills 源码](https://github.com/Govcraft/agent-skills)
- [Anthropic 官方 skill 仓库](https://github.com/anthropics/skills)
- [pi skill 仓库](https://github.com/badlogic/pi-skills)
- onto-studio `ARCHITECTURE.md` 决策 17（`@` 挂载 agentic search）
- onto-studio `CONVERSATION-SCOPE.md`（会话级激活集范式）
