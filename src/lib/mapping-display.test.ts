// 元素映射绑定层的展示层单测（决策 25）。
// 覆盖三态判定、定位串、覆盖率折算、缺口过滤、导出前置检查、文件名安全化。

import { describe, expect, it } from "vitest"
import type {
  ElementMapping,
  MappingCoverage,
  MappingElementRef,
  MappingIntegrityReport,
} from "@/lib/domain"
import {
  coveragePercent,
  coverageSummary,
  elementDisplayName,
  elementTypeLabel,
  endpointLocator,
  exportBlockedReason,
  gapCount,
  groupSuggestions,
  hasCoverageGap,
  integrityGroups,
  isIncompleteMapping,
  mappingExportFileName,
  mappingLocator,
  mappingStateOf,
  MAPPING_STATE_META,
  suggestionPercent,
  unmappedElements,
  unverifiedElements,
} from "@/lib/mapping-display"

function mapping(over: Partial<ElementMapping> = {}): ElementMapping {
  return {
    id: "id-1",
    standard: "w3c",
    ontology_key: "http://ex/ont",
    element_type: "datatype_property",
    element_id: "http://ex/ont#name",
    source: { table: "t_user", column: "name" },
    target: {},
    verified: true,
    verified_at: 1,
    source_kind: "manual",
    source_ref: "",
    note: "",
    created_at: 1,
    updated_at: 1,
    ...over,
  }
}

function element(over: Partial<MappingElementRef> = {}): MappingElementRef {
  return {
    element_type: "datatype_property",
    element_id: "http://ex/ont#name",
    display_name: "",
    data_type: "STRING",
    same_table_hint: "",
    ...over,
  }
}

describe("三态判定", () => {
  it("无映射时为未映射", () => {
    expect(mappingStateOf([])).toBe("unmapped")
    expect(MAPPING_STATE_META.unmapped.label).toBe("未映射")
  })

  it("全部未核对时为未核对", () => {
    expect(
      mappingStateOf([mapping({ verified: false }), mapping({ verified: false })]),
    ).toBe("unverified")
  })

  it("任一已核对即视为已核对", () => {
    expect(
      mappingStateOf([mapping({ verified: false }), mapping({ verified: true })]),
    ).toBe("verified")
  })
})

describe("物理位置定位串", () => {
  it("跳过缺省段", () => {
    expect(endpointLocator({ table: "t", column: "c" })).toBe("t.c")
    expect(endpointLocator({ catalog: "db", table: "t" })).toBe("db.t")
    expect(endpointLocator({})).toBe("")
    expect(endpointLocator(null)).toBe("")
  })

  it("关系带箭头", () => {
    expect(
      mappingLocator(
        mapping({
          element_type: "object_property",
          source: { table: "t_order", column: "user_id" },
          target: { table: "t_user", column: "id" },
        }),
      ),
    ).toBe("t_order.user_id → t_user.id")
  })

  it("单端映射识别为不完整", () => {
    expect(
      isIncompleteMapping(
        mapping({ source: { table: "t", column: "c" }, target: {} }),
      ),
    ).toBe(true)
    expect(
      isIncompleteMapping(
        mapping({
          source: { table: "a", column: "x" },
          target: { table: "b", column: "y" },
        }),
      ),
    ).toBe(false)
  })
})

describe("覆盖率", () => {
  const cov: MappingCoverage = {
    standard: "w3c",
    ontology_key: "k",
    classes: { mapped: 1, total: 4 },
    properties: { mapped: 3, total: 3 },
    relations: { mapped: 0, total: 2 },
  }

  it("百分比四舍五入且不除零", () => {
    expect(coveragePercent({ mapped: 1, total: 3 })).toBe(33)
    expect(coveragePercent({ mapped: 0, total: 0 })).toBe(0)
  })

  it("文案分别计数不合并", () => {
    expect(coverageSummary(cov)).toBe("类 1/4 · 属性 3/3 · 关系 0/2")
    expect(coverageSummary(undefined)).toBe("暂无映射统计")
  })

  it("有缺口判定", () => {
    expect(hasCoverageGap(cov)).toBe(true)
    expect(
      hasCoverageGap({
        standard: "w3c",
        ontology_key: "k",
        classes: { mapped: 1, total: 1 },
        properties: { mapped: 2, total: 2 },
        relations: { mapped: 0, total: 0 },
      }),
    ).toBe(false)
  })
})

describe("缺口清单", () => {
  const elements = [
    element(),
    element({ element_id: "http://ex/ont#age" }),
    element({ element_id: "http://ex/ont#email" }),
  ]
  const mappings = [
    mapping(),
    mapping({
      id: "id-2",
      element_id: "http://ex/ont#age",
      verified: false,
    }),
  ]

  it("未映射只含完全没有映射的元素", () => {
    const out = unmappedElements(elements, mappings)
    expect(out.map((e) => e.element_id)).toEqual(["http://ex/ont#email"])
  })

  it("未核对只含有映射但全未核对的元素", () => {
    const out = unverifiedElements(elements, mappings)
    expect(out.map((e) => e.element_id)).toEqual(["http://ex/ont#age"])
  })

  it("缺口总数是不重复的两类之和", () => {
    expect(gapCount(elements, mappings)).toBe(2)
  })

  it("同 id 不同 element_type 不互相命中", () => {
    const e = [element({ element_type: "class" })]
    expect(unmappedElements(e, mappings)).toHaveLength(1)
  })
})

describe("完整性报告分组", () => {
  it("空报告无分组", () => {
    expect(integrityGroups(undefined)).toEqual([])
    expect(
      integrityGroups({
        standard: "w3c",
        ontology_key: "k",
        orphans: [],
        stale: [],
        unverified: [],
      }),
    ).toEqual([])
  })

  it("按三类分别成组且顺序固定", () => {
    const groups = integrityGroups({
      standard: "w3c",
      ontology_key: "k",
      orphans: [
        {
          mapping_id: "m1",
          element_type: "class",
          element_id: "x",
          locator: "t",
          detail: "元素已不存在",
        },
      ],
      stale: [],
      unverified: [
        {
          mapping_id: "m2",
          element_type: "property",
          element_id: "y",
          locator: "t.c",
          detail: "未核对",
        },
      ],
    } as MappingIntegrityReport)
    expect(groups.map((g) => g.key)).toEqual(["orphans", "unverified"])
    expect(groups[0].label).toBe("已失效元素")
  })
})

describe("候选建议", () => {
  it("置信度折算并夹紧在 0–100", () => {
    expect(suggestionPercent(0.9)).toBe(90)
    expect(suggestionPercent(-1)).toBe(0)
    expect(suggestionPercent(3)).toBe(100)
    expect(suggestionPercent(Number.NaN)).toBe(0)
  })

  it("按元素分组", () => {
    const g = groupSuggestions([
      {
        element_type: "datatype_property",
        element_id: "a",
        candidate: { table: "t", column: "c" },
        score: 0.5,
        reasons: [],
      },
      {
        element_type: "datatype_property",
        element_id: "a",
        candidate: { table: "t", column: "d" },
        score: 0.3,
        reasons: [],
      },
    ])
    expect(g.size).toBe(1)
    expect(g.get("datatype_property\u0000a")).toHaveLength(2)
  })
})

describe("导出", () => {
  it("文件名安全化，扩展名为 xlsx，不带范围后缀", () => {
    expect(mappingExportFileName("http://ex/ont")).toBe("ont-元素映射.xlsx")
    expect(mappingExportFileName("http://ex/onto#SupplyChain")).toBe(
      "SupplyChain-元素映射.xlsx",
    )
    expect(mappingExportFileName("supply_chain")).toBe(
      "supply_chain-元素映射.xlsx",
    )
    expect(mappingExportFileName("我的 本体")).toBe("我的_本体-元素映射.xlsx")
    expect(mappingExportFileName("")).toBe("ontology-元素映射.xlsx")
  })

  it("只在没有元素时阻止导出，未映射不阻止", () => {
    const empty: MappingCoverage = {
      standard: "w3c",
      ontology_key: "k",
      classes: { mapped: 0, total: 2 },
      properties: { mapped: 0, total: 2 },
      relations: { mapped: 0, total: 0 },
    }
    // 有元素但一个都没映射：仍然可以导出（对应字段留空）
    expect(exportBlockedReason(empty)).toBeNull()
    expect(
      exportBlockedReason({
        ...empty,
        classes: { mapped: 0, total: 0 },
        properties: { mapped: 0, total: 0 },
        relations: { mapped: 0, total: 0 },
      }),
    ).toBe("本体里还没有可导出的元素")
    expect(exportBlockedReason(undefined)).toBeNull()
  })
})

describe("元素标签", () => {
  it("类型中文化，未知类型原样返回", () => {
    expect(elementTypeLabel("class")).toBe("类")
    expect(elementTypeLabel("object_property")).toBe("对象属性")
    expect(elementTypeLabel("weird")).toBe("weird")
  })

  it("显示名优先，缺失时取标识末段", () => {
    expect(elementDisplayName(element({ display_name: "名称" }))).toBe("名称")
    expect(elementDisplayName(element({ display_name: "  " }))).toBe("name")
    expect(
      elementDisplayName(element({ element_id: "http://ex/ont#Person" })),
    ).toBe("Person")
  })
})