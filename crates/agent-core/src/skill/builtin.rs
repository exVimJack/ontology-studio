//! 内置 skill frontmatter 扩展：补 Govcraft 不解析的 `disable-model-invocation`。
//!
//! Govcraft 0.2 的 `RawFrontmatter` 是私有的，无法扩展字段。手写极简解析
//! （只查一个布尔字段），约 15 行，零新依赖（符合 onto-studio 轻量化原则）。
//! 若未来 Govcraft 上游加了此字段，可移除这层。

use std::fs;
use std::path::Path;

/// 解析 SKILL.md frontmatter 的 `disable-model-invocation` 字段。
///
/// 层次 1（作者声明）：true 表示不进自动 preamble，只能 @skillName 显式调
/// read_document。Govcraft 0.2 不解析此字段，业务层补。
///
/// 极简实现：只解析 frontmatter 段（第一个 `---` 到第二个 `---`），
/// 逐行匹配 `disable-model-invocation: true`（容忍大小写与首尾空白）。
pub fn parse_disable_model_invocation(skill_dir: &Path) -> bool {
    let skill_md = skill_dir.join("SKILL.md");
    let Ok(content) = fs::read_to_string(&skill_md) else {
        return false;
    };
    let Some(frontmatter) = extract_frontmatter(&content) else {
        return false;
    };
    frontmatter.lines().any(|line| {
        let line = line.trim();
        // 仅匹配 `disable-model-invocation: true`（容忍大小写）
        let Some((k, v)) = line.split_once(':') else {
            return false;
        };
        k.trim() == "disable-model-invocation" && v.trim().eq_ignore_ascii_case("true")
    })
}

/// 提取 frontmatter 段（不含首尾 `---` 分隔符）。无 frontmatter 返回 None。
fn extract_frontmatter(content: &str) -> Option<&str> {
    // 容忍 \r\n / \n 两种行尾
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    // 找第二个 `---`（行首）
    let end = rest
        .find("\n---")
        .or_else(|| rest.find("\r\n---"))?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_skill(dir: &std::path::Path, frontmatter: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let mut f = std::fs::File::create(dir.join("SKILL.md")).unwrap();
        write!(f, "---\n{frontmatter}---\n{body}").unwrap();
    }

    #[test]
    fn parse_dmi_true() {
        let tmp = tempfile_dir();
        write_skill(
            &tmp,
            "name: test-skill\ndescription: x\ndisable-model-invocation: true\n",
            "# body\n",
        );
        assert!(parse_disable_model_invocation(&tmp));
    }

    #[test]
    fn parse_dmi_false_explicit() {
        let tmp = tempfile_dir();
        write_skill(
            &tmp,
            "name: test-skill\ndescription: x\ndisable-model-invocation: false\n",
            "# body\n",
        );
        assert!(!parse_disable_model_invocation(&tmp));
    }

    #[test]
    fn parse_dmi_absent_defaults_false() {
        let tmp = tempfile_dir();
        write_skill(&tmp, "name: test-skill\ndescription: x\n", "# body\n");
        assert!(!parse_disable_model_invocation(&tmp));
    }

    #[test]
    fn parse_dmi_case_insensitive() {
        let tmp = tempfile_dir();
        write_skill(
            &tmp,
            "name: test-skill\ndescription: x\ndisable-model-invocation: TRUE\n",
            "# body\n",
        );
        assert!(parse_disable_model_invocation(&tmp));
    }

    #[test]
    fn parse_dmi_with_surrounding_whitespace() {
        let tmp = tempfile_dir();
        write_skill(
            &tmp,
            "name: test-skill\ndescription: x\n  disable-model-invocation:  true  \n",
            "# body\n",
        );
        assert!(parse_disable_model_invocation(&tmp));
    }

    #[test]
    fn parse_dmi_missing_file() {
        let tmp = tempfile_dir();
        assert!(!parse_disable_model_invocation(&tmp));
    }

    #[test]
    fn parse_dmi_no_frontmatter() {
        let tmp = tempfile_dir();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("SKILL.md"), "# just markdown, no frontmatter\n").unwrap();
        assert!(!parse_disable_model_invocation(&tmp));
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "onto-skill-test-{}",
            uuid::Uuid::new_v4()
        ));
        // 测试结束清理
        let dir_clone = dir.clone();
        // 用 Drop 保证清理太重；测试用例少，靠测试进程退出即可。但为卫生起见仍尝试清理。
        std::fs::create_dir_all(&dir_clone).unwrap();
        dir
    }
}
