// Lib 层：mention.ts
// `@token` 解析工具（文件挂载 + skill 激活）。
//
// 语义（见 ARCHITECTURE.md 决策 17 + 决策 20）:
//   用户在消息中输入 `@token` 选中资源 → 文本原位保留 token（位置语义）。
//   发送时解析文本里所有 `@<token>`，匹配资源得 path 列表。
//   后端在 user message 尾部追加 `<conversation-scope>` 注脚（id+name+path），
//   模型按需 read_document（文件全文 / skill body，统一路径）。
//
// 两种 path 形态：
//   - 文件：path 来自 documents 表（ingest 后入库），如 `/曾国藩专题/书信.md`
//   - skill：虚拟 path `skill://<name>`（SkillManager.ensure_skill_documented 入库）
//
// token 形态（决策 17 修订：token 携带知识库路径）：
//   - 文件夹内文件：`@文件夹/子目录/文件名`（folder_path 去前导 `/` + 文件名）
//   - 根目录散文件：`@文件名`
//   - skill：`@skillName`
//   复制指令到新会话重放时，路径 token 仍能唯一定位知识库文件（重名靠路径区分）；
//   纯 name token 兜底匹配（兼容旧消息），后端对命中的库内文件幂等补挂载。
//
// 匹配规则：token 取 `@` 后到下一个空白/行首为止（`/` `.` 不截断，路径整体捕获）。
// 匹配优先级：路径 token（`文件夹/文件名`）→ 纯文件名 → skill 名。重名文件靠路径区分；
// 纯 name 重名时后者覆盖（罕见，documents 表 path 唯一但 name 可重名）。
// email 里的 @ 宽松匹配后由查表过滤——查不到的自然排除。

import type { DocumentSummaryDto, SkillDto } from "@/lib/domain"

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

/**
 * 文件在 @ 菜单/消息中的 token（不含 `@` 前缀）。
 * 文件夹内：`文件夹/子目录/文件名`（folder_path 去前导与尾随 `/`）；根目录：`文件名`。
 * 菜单插入（MentionMenu.candidateToken）与消息解析（resolveMentionedPaths）共用本函数，
 * 保证 token 格式单一来源。
 */
export function docMentionToken(
  name: string,
  folderPath: string | null | undefined,
): string {
  const fp = folderPath?.trim()
  if (fp && fp !== "/") {
    return `${fp.replace(/^\/+/, "").replace(/\/+$/, "")}/${name}`
  }
  return name
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
 * 从文本中解析 `@token`，匹配知识库文档 + 可用 skill，返回 path 列表（去重，保持出现顺序）。
 *
 * 文档基于**全库**解析（不再限于本会话已挂载列表）：路径 token（`文件夹/文件名`）
 * 精确匹配优先，纯 name token 兜底（兼容旧消息与根目录文件）。这样把指令复制到
 * 新会话重放时文件仍能定位；后端对命中的库内文件幂等补挂载（INSERT OR IGNORE）。
 *
 * @param text 用户输入的消息文本
 * @param allDocs 知识库全量文档列表（useAllDocuments，含 folder_path；skill-md 文档被过滤）
 * @param skills 本会话可用的 skill 列表（skill 虚拟 path 来源，来自 useSkillsConversation）
 * @returns 文本中实际出现的、且匹配到的资源 path 列表（文件 path 或 `skill://<name>`）
 */
export function resolveMentionedPaths(
  text: string,
  allDocs: DocumentSummaryDto[],
  skills: SkillDto[] = [],
): string[] {
  if (allDocs.length === 0 && skills.length === 0) return []

  // KB 路径 token → path（`文件夹/子目录/文件名` 精确匹配，重名文件靠路径区分）
  const kbTokenToPath = new Map<string, string>()
  // 纯文件名 → path（兜底：旧消息 `@文件名` token；重名时后者覆盖）
  const nameToPath = new Map<string, string>()
  for (const d of allDocs) {
    if (d.format === "skill-md") continue // skill 文档走 buildSkillNameToPath，避免 name 冲突
    kbTokenToPath.set(docMentionToken(d.name, d.folder_path), d.path)
    nameToPath.set(d.name, d.path)
  }
  const skillNameToPath = buildSkillNameToPath(skills)

  // 正则匹配 `@` 后跟非空白非@的字符序列（路径/skill 名可含 / . - _ 中文等）。
  // 边界：遇到空白、@、行首即止。email 里的 @ 宽松匹配后由查表过滤——查不到的自然排除。
  const re = /@([^\s@]+)/g
  const paths: string[] = []
  const seen = new Set<string>()
  let m: RegExpExecArray | null
  while ((m = re.exec(text)) !== null) {
    const token = m[1]!
    // 路径 token 精确匹配优先 → 纯文件名兜底（旧消息/根目录）→ skill 名
    const path =
      kbTokenToPath.get(token) ?? nameToPath.get(token) ?? skillNameToPath.get(token)
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
