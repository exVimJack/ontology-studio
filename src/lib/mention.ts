// Lib 层：mention.ts
// `@token` 解析工具（文件挂载 + skill 激活）。
//
// 语义（见 ARCHITECTURE.md 决策 17 + 决策 20）:
//   用户在消息中输入 `@fileName` / `@skillName` 选中资源 → 文本原位保留 token（位置语义）。
//   发送时解析文本里所有 `@<name>`，匹配资源得 path 列表。
//   后端在 user message 尾部追加 `<conversation-scope>` 注脚（id+name），
//   模型按需 read_document（文件全文 / skill body，统一路径）。
//
// 两种 path 形态：
//   - 文件：path 来自 documents 表（ingest 后入库），如 `/曾国藩专题/书信.md`
//   - skill：虚拟 path `skill://<name>`（SkillManager.ensure_skill_documented 入库）
//
// 匹配规则：name 取 `@` 后到下一个空白/标点边界为止。重名时文件优先（skill 是虚拟资源，
// 通常不与文件同名）。匹配优先级在 `resolveMentionedPaths` 内由遍历顺序决定。

import type { MountedDocDto, SkillDto } from "@/lib/domain"

/** skill 虚拟 path 前缀（与后端 SkillManager.ensure_skill_documented 的去重键一致）。 */
export const SKILL_PATH_PREFIX = "skill://"

/** 构造 skill 的虚拟 path：`skill://<name>`。 */
export function skillPath(name: string): string {
  return `${SKILL_PATH_PREFIX}${name}`
}

/** 判断 path 是否为 skill 虚拟 path。 */
export function isSkillPath(path: string): boolean {
  return path.startsWith(SKILL_PATH_PREFIX)
}

/** 从 skill 列表建立 name→path 映射（仅含可 @ 激活的 skill）。
 *  全局禁用 + project（二期未启用）的 skill 不提供。 */
function buildSkillNameToPath(skills: SkillDto[]): Map<string, string> {
  const m = new Map<string, string>()
  for (const s of skills) {
    if (s.globally_disabled) continue
    if (s.source === "project") continue // 二期未启用
    m.set(s.name, skillPath(s.name))
  }
  return m
}

/**
 * 从文本中解析 `@token`，匹配已挂载文档 + 可用 skill，返回 path 列表（去重，保持出现顺序）。
 *
 * @param text 用户输入的消息文本
 * @param mounted 本会话已挂载的文档列表（文件 path 来源）
 * @param skills 本会话可用的 skill 列表（skill 虚拟 path 来源，来自 useSkillsConversation）
 * @returns 文本中实际出现的、且匹配到的资源 path 列表（文件 path 或 `skill://<name>`）
 */
export function resolveMentionedPaths(
  text: string,
  mounted: MountedDocDto[],
  skills: SkillDto[] = [],
): string[] {
  if (mounted.length === 0 && skills.length === 0) return []

  // 文件 name → path 映射。重名时后者覆盖（罕见，documents 表 path 唯一但 name 可重名）。
  const fileNameToPath = new Map<string, string>()
  for (const d of mounted) {
    fileNameToPath.set(d.name, d.path)
  }
  const skillNameToPath = buildSkillNameToPath(skills)

  // 正则匹配 `@` 后跟非空白非@的字符序列（文件名/skill 名可含 . - _ 中文等）。
  // 边界：遇到空白、@、行首即止。email 里的 @ 不匹配（前需有非空白字符，但 email 整体
  // 含 @，这里宽松匹配后由 nameToPath 查表过滤——查不到的自然排除）。
  const re = /@([^\s@]+)/g
  const paths: string[] = []
  const seen = new Set<string>()
  let m: RegExpExecArray | null
  while ((m = re.exec(text)) !== null) {
    const name = m[1]!
    // 文件优先（skill 是虚拟资源，通常不与文件同名）
    const path = fileNameToPath.get(name) ?? skillNameToPath.get(name)
    if (path && !seen.has(path)) {
      seen.add(path)
      paths.push(path)
    }
  }
  return paths
}

/**
 * 从文本中解析出实际出现的 skill name 列表（去重，保持出现顺序）。
 * 用于 Composer 发送时判定哪些 skill 被 `@` 引用，需即时激活（写 conversation_skills）。
 * 仅返回在 skills 列表中匹配到的 name（全局禁用/project 不在此列，见 buildSkillNameToPath）。
 */
export function resolveMentionedSkillNames(text: string, skills: SkillDto[]): string[] {
  if (skills.length === 0) return []
  const skillNameToPath = buildSkillNameToPath(skills)
  const re = /([^\s@]+)/g
  const names: string[] = []
  const seen = new Set<string>()
  let m: RegExpExecArray | null
  while ((m = re.exec(text)) !== null) {
    const name = m[1]!
    if (skillNameToPath.has(name) && !seen.has(name)) {
      seen.add(name)
      names.push(name)
    }
  }
  return names
}
