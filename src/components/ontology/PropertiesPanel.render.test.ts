// 属性页签的渲染级回归测试（决策 25 的来源字段筛选）。
//
// 为什么需要这一层：本次线上崩溃是「组件体内闭包在 const 声明之前就被求值」
// 造成的渲染期 TDZ（`Cannot access 'onlyUnmapped' before initialization`），
// 纯函数单测和 `tsc` 都抓不到——只有真正渲染一次才会暴露。
//
// 做法：不引入 jsdom（项目当前只跑 node 环境），而是在 node 里装一个最小的
// DOM/QueryClient 垫片，用 react-dom/server 把组件渲染成字符串。
// 断言重点是「有属性时能渲染出内容且不抛错」——空列表恰好不会触发旧 bug 的
// 执行路径（filter 回调不执行），所以用例必须喂入非空属性列表。

import { describe, expect, it, vi, beforeAll, afterAll } from "vitest"

// ── 最小 DOM 垫片（react-dom/server 与组件内 useEffect 相关调用需要） ──
const globals = globalThis as unknown as Record<string, unknown>

beforeAll(() => {
  if (!globals.window) {
    globals.window = {
      matchMedia: () => ({
        matches: false,
        addEventListener: () => {},
        removeEventListener: () => {},
      }),
      addEventListener: () => {},
      removeEventListener: () => {},
      localStorage: {
        getItem: () => null,
        setItem: () => {},
        removeItem: () => {},
      },
    }
  }
  if (!globals.navigator) {
    globals.navigator = { clipboard: { writeText: async () => {} } }
  }
})

afterAll(() => {
  delete globals.window
  delete globals.navigator
})

// 组件用到的 IPC：全部返回空数据，让面板走「有本体、无映射」的分支
vi.mock("@/lib/ipc/commands", () => ({
  ipc: {
    listElementMappings: vi.fn(async () => []),
    mappingCoverage: vi.fn(async () => null),
    mappingIntegrity: vi.fn(async () => null),
    listMappingElements: vi.fn(async () => []),
    saveElementMappings: vi.fn(async () => ({ written: 0, deleted: 0 })),
    deleteElementMappings: vi.fn(async () => 0),
    purgeOrphanMappings: vi.fn(async () => 0),
    suggestElementMappings: vi.fn(async () => []),
    exportElementMapping: vi.fn(async () => "/tmp/out.xlsx"),
  },
}))

vi.mock("@/components/ontology/ConceptGraph", () => ({
  ConceptGraph: () => null,
}))

vi.mock("@/hooks/useOntologyTtl", async () => {
  const props = [
    { iri: "http://ex/ont#orderNo", kind: "datatype", label: "订单号", domain: "http://ex/ont#Order", range: "http://www.w3.org/2001/XMLSchema#string" },
    { iri: "http://ex/ont#amount", kind: "datatype", label: "金额", domain: "http://ex/ont#Order", range: "http://www.w3.org/2001/XMLSchema#decimal" },
    { iri: "http://ex/ont#hasCustomer", kind: "object", label: "所属客户", domain: "http://ex/ont#Order", range: "http://ex/ont#Customer" },
  ]
  return {
    useOntologyTtlProperties: () => ({
      data: props,
      isLoading: false,
    }),
    useTtlElementEvidence: () => ({ data: [] }),
    useOntologyTtlClassIris: () => ({ data: [] }),
    useTtlLabelMap: () => ({ data: new Map() }),
    useOntologyTtlDefaultPropertyNs: () => ({ data: null }),
    useEditOntologyTtlProperty: () => ({ isPending: false, mutateAsync: vi.fn() }),
    useAddOntologyTtlProperty: () => ({ isPending: false, mutateAsync: vi.fn() }),
    useDeleteOntologyTtlProperty: () => ({ isPending: false, mutateAsync: vi.fn() }),
  }
})

describe("PropertiesPanel 渲染", () => {
  it("有属性时能渲染，不因筛选逻辑抛错", async () => {
    const { renderToString } = await import("react-dom/server")
    const { QueryClient, QueryClientProvider } = await import(
      "@tanstack/react-query"
    )
    const { createElement } = await import("react")
    const { PropertiesPanel } = await import(
      "@/components/ontology/OntologyTtlView"
    )

    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })

    const html = renderToString(
      createElement(
        QueryClientProvider,
        { client },
        createElement(PropertiesPanel, { iri: "http://ex/ont#" }),
      ),
    )

    // 列表态应渲染出属性行（含中文 label），而不是错误页/空白
    // 默认子页签是「关系」，所以首屏显示关系属性
    expect(html).toContain("所属客户")
    expect(html).toContain("未映射")
    // 计数区反映过滤后的分类计数（关系 1 / 数据属性 2）
    expect(html).toContain("共 3 属性")
    // 两个子页签都在，且带过滤后计数
    expect(html).toContain("关系")
    expect(html).toContain("数据属性")
    // 来源字段列存在
    expect(html).toContain("来源字段")
  })
})