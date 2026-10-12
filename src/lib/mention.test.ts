// 单元测试：mention.ts（@token 解析纯逻辑）
// 覆盖决策 17 修订：路径 token（`@文件夹/文件名`）精确匹配 + 纯 name 兜底 + skill 解析。
// 运行：npm test（vitest）。

import { describe, expect, it } from "vitest"
import type { DocumentSummaryDto, SkillDto } from "@/lib/domain"
import { docMentionToken, resolveMentionedPaths, resolveMentionedSkillNames, skillPath } from "./mention"

let seq = 0

/** 构造文档摘要（默认 md 格式、根目录）。 */
function doc(name: string, folderPath: string | null = null, format = "md"): DocumentSummaryDto {
  seq += 1
  return {
    id: `doc_${seq}`,
    path: `/os/path/${seq}/${name}`,
    name,
    format,
    char_count: 100,
    created_at: 1_700_000_000_000,
    folder_path: folderPath,
  }
}

/** 构造 skill（默认 builtin、启用）。 */
function skill(name: string, overrides: Partial<SkillDto> = {}): SkillDto {
  return {
    name,
    description: "",
    source: "builtin",
    disable_model_invocation: false,
    license: null,
    compatibility: null,
    allowed_tools: null,
    conversation_enabled: null,
    globally_disabled: false,
    ...overrides,
  }
}

// ── docMentionToken：token 单一来源 ──

describe("docMentionToken", () => {
  it("非根目录：folder_path 去前导 / + 文件名", () => {
    expect(docMentionToken("报告.pdf", "/知识库/试驾")).toBe("知识库/试驾/报告.pdf")
  })

  it("多级目录：完整子路径拼接", () => {
    expect(docMentionToken("a.md", "/a/b/c")).toBe("a/b/c/a.md")
  })

  it("folder_path 带尾随 / 时去尾随", () => {
    expect(docMentionToken("a.md", "/知识库/")).toBe("知识库/a.md")
  })

  it("根目录 /：回退纯文件名", () => {
    expect(docMentionToken("a.md", "/")).toBe("a.md")
  })

  it("null / undefined / 空白 folder_path：回退纯文件名", () => {
    expect(docMentionToken("a.md", null)).toBe("a.md")
    expect(docMentionToken("a.md", undefined)).toBe("a.md")
    expect(docMentionToken("a.md", "  ")).toBe("a.md")
  })
})

// ── resolveMentionedPaths：路径 token 优先 + name 兜底 + skill ──

describe("resolveMentionedPaths", () => {
  const docs = [
    doc("报告.pdf", "/知识库/试驾"),
    doc("报告.pdf", "/知识库/复购"), // 重名：不同文件夹
    doc("随笔.md", null), // 根目录散文件
  ]

  it("路径 token 精确匹配文件夹内文件", () => {
    const out = resolveMentionedPaths("请分析 @知识库/试驾/报告.pdf", docs)
    expect(out).toEqual([docs[0]!.path])
  })

  it("重名文件靠路径区分：两个路径 token 各命中对应文件", () => {
    const text = "对比 @知识库/试驾/报告.pdf 和 @知识库/复购/报告.pdf"
    const out = resolveMentionedPaths(text, docs)
    expect(out).toEqual([docs[0]!.path, docs[1]!.path])
  })

  it("根目录散文件：纯 name token 命中", () => {
    const out = resolveMentionedPaths("看看 @随笔.md", docs)
    expect(out).toEqual([docs[2]!.path])
  })

  it("旧消息纯 name token 兜底匹配文件夹内文件（重名取后者）", () => {
    const out = resolveMentionedPaths("看看 @报告.pdf", docs)
    expect(out).toEqual([docs[1]!.path])
  })

  it("skill-md 文档被过滤：不作为文档匹配，同名 skill 走 skill 解析", () => {
    const skillDoc = doc("my-skill", "/Skills/my-skill", "skill-md")
    const out = resolveMentionedPaths("用 @my-skill 处理", [skillDoc], [skill("my-skill")])
    expect(out).toEqual([skillPath("my-skill")])
  })

  it("skill name 命中返回 skill:// 虚拟 path", () => {
    const out = resolveMentionedPaths("用 @建模助手 处理", [], [skill("建模助手")])
    expect(out).toEqual([skillPath("建模助手")])
  })

  it("全局禁用 / project 来源的 skill 不参与匹配", () => {
    const out = resolveMentionedPaths(
      "@禁用skill @projectskill",
      [],
      [
        skill("禁用skill", { globally_disabled: true }),
        skill("projectskill", { source: "project" }),
      ],
    )
    expect(out).toEqual([])
  })

  it("匹配优先级：路径 token > 纯 name > skill name", () => {
    // 同名三形态并存时，路径 token 精确匹配优先
    const d = doc("x.md", "/kb")
    const out = resolveMentionedPaths("@kb/x.md", [d], [skill("x.md")])
    expect(out).toEqual([d.path])
    // 纯 name 优先于 skill name
    const out2 = resolveMentionedPaths("@x.md", [d], [skill("x.md")])
    expect(out2).toEqual([d.path])
  })

  it("去重且保持出现顺序：同文件出现两次只返回一次", () => {
    const out = resolveMentionedPaths("@随笔.md 再看 @随笔.md", docs)
    expect(out).toEqual([docs[2]!.path])
  })

  it("email 等 @token 查表不到则排除，不误报", () => {
    const out = resolveMentionedPaths("联系 foo@example.com 看看 @随笔.md", docs)
    expect(out).toEqual([docs[2]!.path])
  })

  it("无任何匹配返回空数组", () => {
    expect(resolveMentionedPaths("普通文本不含引用", docs)).toEqual([])
  })

  it("allDocs 与 skills 均为空直接返回空", () => {
    expect(resolveMentionedPaths("@随便什么", [], [])).toEqual([])
  })
})

// ── resolveMentionedSkillNames：行为不变（回归保护）──

describe("resolveMentionedSkillNames", () => {
  it("解析出现且可用的 skill name，去重保序", () => {
    const out = resolveMentionedSkillNames("@a 然后 @b 再 @a", [
      skill("a"),
      skill("b"),
      skill("c"),
    ])
    expect(out).toEqual(["a", "b"])
  })

  it("全局禁用 / project 来源不返回", () => {
    const out = resolveMentionedSkillNames("@a @b", [
      skill("a", { globally_disabled: true }),
      skill("b", { source: "project" }),
    ])
    expect(out).toEqual([])
  })

  it("skills 为空直接返回空", () => {
    expect(resolveMentionedSkillNames("@a", [])).toEqual([])
  })
})
