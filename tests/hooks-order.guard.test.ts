// 回归守卫：整个 src/ 不得出现 React Hooks 顺序违规（React #310）。
//
// 背景：本体列表切换本体时偶发「Rendered fewer hooks than expected. This may be
// caused by an accidental early return statement.」——根因是两处组件把 hook 写在了
// 加载早退之后：
//   1. OntologyTtlView.tsx 的 SourcePanel：`if (!content) return ...` 之后才 useEffect
//      （切本体时 content 先变 undefined，同实例前后两次渲染 hook 数量不一致）
//   2. ModelSettingsPanel.tsx 的 ProviderForm：`cond ? useA() : useB()` 条件调用 hook
//
// 为什么需要这条测试：项目此前只启用了 oxlint 的 `import` 插件，react-hooks 规则
// 未开启，这类 bug 静态检查完全发现不了，只能靠运行时崩。这里把规则钉进测试，
// 任何人再引入同类写法会直接把 CI 打红。
//
// 注意 oxlint 诊断里的 `code` 是 `插件(规则)` 形式（如 `react-hooks(rules-of-hooks)`），
// 不是 ESLint 的 `插件/规则`。写成后者会让过滤永远命中不到、测试变成永久假绿
//——所以下面加了「扫描器自检」用例，用一份已知违规的临时文件验证真的能检出。
//
// 本文件放在 tests/ 而非 src/：它需要用 node 内置模块（child_process/path/fs），
// 而 src 走浏览器 tsconfig（未装 @types/node，`npm run build` 的 tsc 会报错）。

import { describe, expect, it } from "vitest"
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const HOOKS_RULE = "react-hooks/rules-of-hooks"
const HOOKS_RULE_CODE = "react-hooks(rules-of-hooks)"

interface OxlintDiagnostic {
  message: string
  code: string
  filename: string
}

/** 跑 oxlint 并解析 JSON；有诊断时退出码非 0，但 stdout 仍是合法 JSON。 */
function runOxlint(target: string): OxlintDiagnostic[] {
  const bin = path.join(projectRoot, "node_modules", "oxlint", "bin", "oxlint")
  let stdout = ""
  try {
    stdout = execFileSync(
      process.execPath,
      [bin, "--react-plugin", "-D", HOOKS_RULE, "-f", "json", target],
      { cwd: projectRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
    )
  } catch (e) {
    stdout = (e as { stdout?: string }).stdout ?? ""
  }
  if (!stdout.trim()) return []
  const parsed = JSON.parse(stdout) as { diagnostics?: OxlintDiagnostic[] }
  return parsed.diagnostics ?? []
}

describe("React Hooks 顺序", () => {
  it("src/ 下无 rules-of-hooks 违规（无早退后调 hook / 条件调用 hook）", () => {
    const diagnostics = runOxlint("src").filter((d) => d.code === HOOKS_RULE_CODE)
    const report = diagnostics
      .map((d) => `${path.relative(projectRoot, d.filename)}: ${d.message}`)
      .join("\n")
    expect(report).toBe("")
  }, 120_000)

  it("扫描器自检：已知违规文件必须被检出（防止规则名写错导致永久假绿）", () => {
    // 临时目录必须放在项目内：oxlint 对项目外路径直接跳过、不产出诊断，
    // 放 os.tmpdir() 会让这条自检永远为绿，反而掩盖问题。
    const dir = fs.mkdtempSync(path.join(projectRoot, ".tmp-hooks-guard-"))
    try {
      fs.writeFileSync(
        path.join(dir, "Bad.tsx"),
        [
          "import { useState } from 'react'",
          "export function Bad({ ready }: { ready: boolean }) {",
          "  if (!ready) return null",
          "  const [n] = useState(0)",
          "  return <div>{n}</div>",
          "}",
          "",
        ].join("\n"),
        "utf8",
      )
      const found = runOxlint(dir).some((d) => d.code === HOOKS_RULE_CODE)
      expect(found).toBe(true)
    } finally {
      fs.rmSync(dir, { recursive: true, force: true })
    }
  }, 120_000)
})