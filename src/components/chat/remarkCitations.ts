// Citation 渲染：remark 插件，把正文里的 `[n]` 标记转成可交互角标。
//
// 模型回答时（后端 prompt 指示"用 [n] 标注引用"），正文会出现 [1][2] 等。
// 本插件扫描 text 节点，把 [数字] 替换为 <Citation> 组件（上标小数字，hover 显示来源）。
//
// 实现方式：自定义 remark 插件遍历 mdast，把 text 节点里的 [n] 拆成
// text + html(sup) 序列。react-markdown 的 rehype 阶段会渲染 html 节点。
// 但为安全（禁 raw HTML），改用自定义 component：把 [n] 包成 <sup data-citation="n">，
// 经由 rehype 保留，再由 markdown components 的 sup 渲染拦截。
//
// 简化：直接在 mdast 层把 [n] 转为 link 类型节点（href="#cite-n"），让 components.a 拦截。

import type { Plugin } from "unified"
import type { Root, Text, PhrasingContent } from "mdast"

const CITE_RE = /\[(\d{1,3})\]/g

/** 把 text 节点按 [n] 拆分为 text + link 序列。 */
function splitCitations(text: string): PhrasingContent[] {
  const out: PhrasingContent[] = []
  let last = 0
  let m: RegExpExecArray | null
  CITE_RE.lastIndex = 0
  while ((m = CITE_RE.exec(text)) !== null) {
    if (m.index > last) {
      out.push({ type: "text", value: text.slice(last, m.index) })
    }
    const n = m[1]
    out.push({
      type: "link",
      url: `#cite-${n}`,
      data: { hProperties: { className: ["citation"], "data-citation": n } },
      children: [{ type: "text", value: `[${n}]` }],
    })
    last = m.index + m[0].length
  }
  if (last < text.length) {
    out.push({ type: "text", value: text.slice(last) })
  }
  return out
}

export const remarkCitations: Plugin<[], Root> = () => {
  return (tree) => {
    const walk = (node: Root | PhrasingContent) => {
      const children = (node as { children?: PhrasingContent[] }).children
      if (!children) return
      const next: PhrasingContent[] = []
      for (const child of children) {
        if (child.type === "text") {
          const txt = child as Text
          if (CITE_RE.test(txt.value)) {
            next.push(...splitCitations(txt.value))
            continue
          }
        }
        walk(child as PhrasingContent)
        next.push(child)
      }
      ;(node as { children: PhrasingContent[] }).children = next
    }
    walk(tree)
  }
}
