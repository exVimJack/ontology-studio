//! Skill 系统存储（决策 20）。
//!
//! 两张表：
//!   - `disabled_skills`：全局禁用偏好（层次 2，应用级，跨所有会话）
//!   - `conversation_skills`：会话级激活状态（层次 3）
//!
//! 与 `conversation_documents` 一样用 `ON DELETE CASCADE` 跟随会话清理。
//! 时间戳用 i64 unix ms（与 documents 表一致；IPC 层若需暴露走 memory::Timestamp）。

use rusqlite::{params, OptionalExtension};

use crate::MemoryResult;

/// 全局禁用的 skill 行（层次 2）。
#[derive(Debug, Clone)]
pub struct DisabledSkillRow {
    pub skill_name: String,
    pub disabled_at: i64, // unix ms
}

/// 会话级 skill 激活行（层次 3）。
///
/// `enabled` 语义随 source 而异（见 SKILL-SYSTEM.md §3.6）：
///   - Builtin / ExternalReadOnly：默认进 preamble，enabled=false 表示显式排除
///   - Imported：默认不进 preamble，enabled=true 表示显式激活
#[derive(Debug, Clone)]
pub struct ConversationSkillRow {
    pub conversation_id: String,
    pub skill_name: String,
    /// "builtin" | "imported" | "external-readonly" | "project"
    pub source: String,
    pub enabled: bool,
    pub activated_at: i64,
}

impl super::Memory {
    /// 初始化 Skill 系统两张表。在 init_schema 中调用，幂等。
    pub(crate) fn init_skill_schema(conn: &rusqlite::Connection) -> MemoryResult<()> {
        conn.execute_batch(
            r#"
            -- 全局禁用偏好（层次 2）：用户在设置页显式 disable 的 skill，
            -- 跨所有会话不进 preamble。skill_name 作去重键。
            CREATE TABLE IF NOT EXISTS disabled_skills (
                skill_name    TEXT PRIMARY KEY,
                disabled_at   INTEGER NOT NULL
            );

            -- 会话级激活（层次 3）：单会话的 skill enable 状态。
            -- imported skill 默认不进 preamble，需显式 enabled=1；
            -- builtin/external 默认进 preamble，可显式 enabled=0 排除。
            -- source 列冗余存，便于前端展示来源图标。
            CREATE TABLE IF NOT EXISTS conversation_skills (
                conversation_id  TEXT NOT NULL,
                skill_name       TEXT NOT NULL,
                source           TEXT NOT NULL,
                enabled          INTEGER NOT NULL DEFAULT 0,
                activated_at     INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, skill_name),
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_conv_skills_conv ON conversation_skills(conversation_id);
            "#,
        )?;
        Ok(())
    }

    // ───────── 全局禁用（层次 2） ─────────

    /// 列出全部全局禁用的 skill name。
    pub fn list_disabled_skills(&self) -> MemoryResult<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT skill_name FROM disabled_skills")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 设置某 skill 全局禁用（enabled=true 禁用，false 解除）。
    /// 幂等：已存在则更新时间；解除时删除行。
    pub fn set_skill_globally_disabled(
        &self,
        skill_name: &str,
        disabled: bool,
    ) -> MemoryResult<()> {
        let conn = self.lock();
        if disabled {
            conn.execute(
                "INSERT INTO disabled_skills(skill_name, disabled_at) VALUES (?, ?)
                 ON CONFLICT(skill_name) DO UPDATE SET disabled_at = excluded.disabled_at",
                params![skill_name, crate::now_ms()],
            )?;
        } else {
            conn.execute(
                "DELETE FROM disabled_skills WHERE skill_name = ?",
                params![skill_name],
            )?;
        }
        Ok(())
    }

    /// 判断某 skill 是否全局禁用。
    pub fn is_skill_globally_disabled(&self, skill_name: &str) -> MemoryResult<bool> {
        let conn = self.lock();
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM disabled_skills WHERE skill_name = ?",
                params![skill_name],
                |r| r.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    // ───────── 会话级激活（层次 3） ─────────

    /// 列出某会话的全部 skill 激活记录。
    pub fn list_conversation_skills(
        &self,
        conversation_id: &str,
    ) -> MemoryResult<Vec<ConversationSkillRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT conversation_id, skill_name, source, enabled, activated_at
             FROM conversation_skills
             WHERE conversation_id = ?
             ORDER BY activated_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |r| {
            Ok(ConversationSkillRow {
                conversation_id: r.get(0)?,
                skill_name: r.get(1)?,
                source: r.get(2)?,
                enabled: r.get::<_, i64>(3)? != 0,
                activated_at: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 设置某会话某 skill 的 enabled 状态（upsert）。
    /// source 由调用方传入（与 SkillRecord.source 一致）。
    pub fn set_conversation_skill_enabled(
        &self,
        conversation_id: &str,
        skill_name: &str,
        source: &str,
        enabled: bool,
    ) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO conversation_skills(conversation_id, skill_name, source, enabled, activated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(conversation_id, skill_name) DO UPDATE SET
                source = excluded.source,
                enabled = excluded.enabled,
                activated_at = excluded.activated_at",
            params![
                conversation_id,
                skill_name,
                source,
                enabled as i64,
                crate::now_ms()
            ],
        )?;
        Ok(())
    }

    /// 删除某会话某 skill 的激活记录（恢复默认行为）。
    pub fn remove_conversation_skill(
        &self,
        conversation_id: &str,
        skill_name: &str,
    ) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM conversation_skills WHERE conversation_id = ? AND skill_name = ?",
            params![conversation_id, skill_name],
        )?;
        Ok(())
    }

    /// 清除某 skill 的全部会话记录（卸载导入 skill 时调用）。
    /// 同时清 disabled_skills 行（卸载后偏好无意义）。
    pub fn remove_skill_records(&self, skill_name: &str) -> MemoryResult<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM conversation_skills WHERE skill_name = ?",
            params![skill_name],
        )?;
        conn.execute(
            "DELETE FROM disabled_skills WHERE skill_name = ?",
            params![skill_name],
        )?;
        Ok(())
    }
}

// DisabledSkillRow 当前仅用于类型导出（list_disabled_skills 返回 Vec<String>），
// 保留结构体供将来扩展（如记录 disabled_at）。
#[allow(dead_code)]
const _: fn() = || {
    let _: DisabledSkillRow;
};
