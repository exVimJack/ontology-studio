//! preamble XML 生成（<available_skills> 块）。
//!
//! 手写极简 XML 转义，零依赖（符合 onto-studio 轻量化原则）。
//! 格式参照 Govcraft CLI 的 `to_prompt` 命令与 agentskills.io 规范。
//!
//! 注意：此模块只负责 XML 文本生成，disable 三层判断逻辑在 activate.rs
//! （build_preamble_section 复用 resolve_active 的判断结果）。

use super::{SkillManager, SkillRecord};

/// XML 转义：& < >（极简版，skill name/description 不会出现引号属性场景）。
pub(super) fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 生成 <available_skills> XML 块。
///
/// 入参是已通过 disable 三层判断、且 doc_id 已入库的 skill 列表。
/// 空列表返回空串（调用方据此不追加 preamble）。
///
/// `<location>` 文案说明模型如何读到 skill 完整内容：
///   - SKILL.md body 已入库（doc path = `skill://<name>`）
///   - references/assets/scripts 三个规范子目录下的文本资源均已入库
///     （doc path = `skill://<name>/<dir>/<file>`）
///   - 用 `search_documents` 跨 body+资源搜关键词（如 "主键 pattern"、"storage_type 取值"）
///   - 用 `list_documents` 拿到所有文档 id（含资源），再 `read_document(id)` 精读
///   - SKILL.md body 里写的 `references/<file>.md` / `scripts/<file>.sh` / `assets/<file>`
///     相对路径 = 上述 doc path 的 <dir>/<file> 部分
///
/// 这条文案是过去一轮 Agent 断链的修复点：原文案只说"用 read_document 读”，
/// 但 list_documents 刻意排除 skill-md，模型连 id 都拿不到，只能在知识库里瞎找。
pub(super) fn format_available_skills(skills: &[&SkillRecord]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut xml = String::from("<available_skills>\n");
    for s in skills {
        xml.push_str("<skill>\n");
        xml.push_str(&format!(
            "<name>{}</name>\n",
            escape_xml(&s.name)
        ));
        xml.push_str(&format!(
            "<description>{}</description>\n",
            escape_xml(&s.description)
        ));
        // location 提示模型如何读到 skill 完整内容（body + 三类资源子目录）。
        // 资源数量动态拼入，让模型知道该 skill 带了多少份契约/脚本/模板文档。
        let res_count = s.resource_doc_paths.as_ref().map(|v| v.len()).unwrap_or(0);
        let location = if res_count == 0 {
            format!(
                "本 skill 的 SKILL.md 全文已入库，doc path = skill://{name}。\
                 用 search_documents 搜关键词、或 list_documents 拿到 id 后 read_document(id) 精读。",
                name = escape_xml(&s.name)
            )
        } else {
            format!(
                "本 skill 的 SKILL.md 全文 + {res} 份资源文件均已入库（references/assets/scripts 规范子目录下）。\
                 body doc path = skill://{name}；资源 doc path = skill://{name}/<子目录>/<文件名>\
                 （<子目录> ∈ {{references, assets, scripts}}，对应 SKILL.md body 里写的相对路径）。\
                 读取方式：① search_documents 跨 body+资源搜关键词（如“主键 pattern”“storage_type 取值”）；\
                 ② list_documents 拿到所有文档 id（含资源）后 read_document(id) 精读。\
                 产出前务必先读 references 里的 schema 契约文档，勿凭描述猜字段名。",
                res = res_count,
                name = escape_xml(&s.name)
            )
        };
        xml.push_str(&format!("<location>{}</location>\n", location));
        xml.push_str("</skill>\n");
    }
    xml.push_str("</available_skills>");
    xml
}

impl SkillManager {
    /// 生成本会话的 preamble Tier 1 片段（<available_skills> XML）。
    ///
    /// 判断流程（对每个已发现的 skill）：
    ///   1. frontmatter.disable_model_invocation == true → 跳过（层次 1）
    ///   2. skill_name 在 disabled_skills 表 → 跳过（层次 2）
    ///   3. 按 source 判断默认 + 会话级 enabled 覆盖（层次 3）
    ///   4. 所有进 preamble 的 skill 都入库 documents（确保 doc_id 存在）
    ///
    /// 空结果返回空串（调用方据此不追加 preamble，不破坏 prefix cache）。
    pub fn build_preamble_section(&self, conversation_id: &str) -> Result<String, super::SkillError> {
        let active = self.resolve_preamble_skills(conversation_id)?;
        let refs: Vec<&SkillRecord> = active.iter().collect();
        Ok(format_available_skills(&refs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{SkillRecord, SkillSource};
    use std::path::PathBuf;

    fn record(name: &str, refs: Option<Vec<String>>) -> SkillRecord {
        SkillRecord {
            name: name.to_string(),
            description: format!("desc for {name}"),
            source: SkillSource::Builtin,
            dir_path: PathBuf::from("/tmp"),
            doc_id: Some("id-1".to_string()),
            resource_doc_paths: refs,
            disable_model_invocation: false,
            allowed_tools: None,
            license: None,
            compatibility: None,
        }
    }

    #[test]
    fn empty_skills_returns_empty() {
        assert_eq!(format_available_skills(&[]), "");
    }

    #[test]
    fn no_resources_location_mentions_only_body() {
        let r = record("bare", None);
        let xml = format_available_skills(&[&r]);
        assert!(xml.contains("<name>bare</name>"));
        assert!(xml.contains("skill://bare"));
        assert!(!xml.contains("资源文件"));
    }

    #[test]
    fn with_resources_location_lists_count_and_paths() {
        let r = record(
            "ontology-modeling",
            Some(vec![
                "skill://ontology-modeling/references/gaia-schema-contract.md".to_string(),
                "skill://ontology-modeling/references/ontology-package-format.md".to_string(),
                "skill://ontology-modeling/scripts/validate.sh".to_string(),
            ]),
        );
        let xml = format_available_skills(&[&r]);
        assert!(xml.contains("3 份资源文件"), "应拼入资源数量");
        assert!(
            xml.contains("references/assets/scripts"),
            "应说明三类规范子目录"
        );
        assert!(
            xml.contains("产出前务必先读 references 里的 schema 契约文档"),
            "应明确提示先读契约文档"
        );
    }

    #[test]
    fn xml_escapes_special_chars_in_name() {
        let mut r = record("a<b>&c", None);
        r.description = "d<e>&f".to_string();
        let xml = format_available_skills(&[&r]);
        assert!(xml.contains("&lt;"));
        assert!(xml.contains("&amp;"));
        assert!(!xml.contains("<b>") && !xml.contains("<e>"));
    }
}
