import { describe, expect, it } from "vitest"
import type { ElementMapping, TtlPropertyEdit } from "@/lib/domain"
import {
  filterTtlProperties,
  groupByDomain,
  iriTail,
  propertyMappingState,
  propertyMatchesQuery,
  splitTtlProperties,
} from "@/lib/ttl-property-filter"

function prop(
  iri: string,
  kind: "datatype" | "object",
  extra: Partial<TtlPropertyEdit> = {},
): TtlPropertyEdit {
  return {
    iri,
    kind,
    label: extra.label,
    comment: extra.comment,
    domain: extra.domain,
    range: extra.range,
  }
}

function mappingOf(elementId: string, verified: boolean): ElementMapping {
  return {
    id: `em-${elementId}`,
    standard: "w3c",
    ontology_key: "http://ex/ont#",
    element_type: "datatype_property",
    element_id: elementId,
    source: {},
    target: {},
    verified,
    verified_at: verified ? 1 : 0,
    source_kind: "manual",
    source_ref: "",
    note: "",
    created_at: 1,
    updated_at: 1,
  }
}

describe("iriTail", () => {
  it("取 # 或 / 的最后一段，带尖括号也能处理", () => {
    expect(iriTail("http://ex/ont#orderNo")).toBe("orderNo")
    expect(iriTail("http://ex/ont/orderNo")).toBe("orderNo")
    expect(iriTail("<http://ex/ont#orderNo>")).toBe("orderNo")
    // 没有分隔符时原样返回
    expect(iriTail("orderNo")).toBe("orderNo")
  })
})

describe("propertyMatchesQuery", () => {
  const p = prop("http://ex/ont#orderNo", "datatype", {
    label: "订单号",
    comment: "主键列",
    domain: "http://ex/ont#Order",
    range: "xsd:string",
  })

  it("空白查询命中全部", () => {
    expect(propertyMatchesQuery(p, "")).toBe(true)
    expect(propertyMatchesQuery(p, "   ")).toBe(true)
    expect(propertyMatchesQuery(p, undefined)).toBe(true)
  })

  it("命中 label / comment / 短名 / 全 IRI / domain / range", () => {
    expect(propertyMatchesQuery(p, "订单")).toBe(true)
    expect(propertyMatchesQuery(p, "主键")).toBe(true)
    expect(propertyMatchesQuery(p, "orderno")).toBe(true) // 大小写不敏感
    expect(propertyMatchesQuery(p, "http://ex/ont#orderNo")).toBe(true)
    expect(propertyMatchesQuery(p, "Order")).toBe(true)
    expect(propertyMatchesQuery(p, "string")).toBe(true)
  })

  it("不命中时返回 false", () => {
    expect(propertyMatchesQuery(p, "不存在")).toBe(false)
  })
})

describe("propertyMappingState", () => {
  it("无映射行 → unmapped", () => {
    expect(propertyMappingState(prop("a", "datatype"), new Map())).toBe(
      "unmapped",
    )
    expect(propertyMappingState(prop("a", "datatype"), undefined)).toBe(
      "unmapped",
    )
  })

  it("映射存在但未核对 → unverified；已核对 → verified", () => {
    const table = new Map<string, ElementMapping[]>([
      ["datatype_property\u0000a", [mappingOf("a", false)]],
      ["datatype_property\u0000b", [mappingOf("b", true)]],
    ])
    expect(propertyMappingState(prop("a", "datatype"), table)).toBe(
      "unverified",
    )
    expect(propertyMappingState(prop("b", "datatype"), table)).toBe("verified")
  })

  it("关系属性用 object_property 键，不与数据属性串味", () => {
    // 映射挂在 datatype_property 键上，关系属性不应误判为已映射
    const table = new Map<string, ElementMapping[]>([
      ["datatype_property\u0000rel", [mappingOf("rel", true)]],
    ])
    expect(propertyMappingState(prop("rel", "object"), table)).toBe("unmapped")
  })
})

describe("filterTtlProperties / splitTtlProperties", () => {
  const list: TtlPropertyEdit[] = [
    prop("http://ex/ont#orderNo", "datatype", { label: "订单号" }),
    prop("http://ex/ont#amount", "datatype", { label: "金额" }),
    prop("http://ex/ont#hasCustomer", "object", { label: "所属客户" }),
  ]
  const table = new Map<string, ElementMapping[]>([
    ["datatype_property\u0000http://ex/ont#orderNo", [mappingOf("x", true)]],
  ])

  it("默认不过滤，保持入参顺序", () => {
    expect(filterTtlProperties(list).map((p) => p.iri)).toEqual(
      list.map((p) => p.iri),
    )
  })

  it("文本搜索叠加生效", () => {
    const got = filterTtlProperties(list, { query: "金额" })
    expect(got).toHaveLength(1)
    expect(got[0].label).toBe("金额")
  })

  it("只看未映射：已核对与未映射的区分要靠映射表，没传映射表视为全未映射", () => {
    // 只挂了一条已核对映射：orderNo 应被排除，其余保留
    const onlyUnmapped = filterTtlProperties(list, {
      onlyUnmapped: true,
      mappingByElement: table,
    })
    expect(onlyUnmapped.map((p) => p.iri)).toEqual([
      "http://ex/ont#amount",
      "http://ex/ont#hasCustomer",
    ])
  })

  it("只看未映射 + 文本搜索可同时生效", () => {
    const got = filterTtlProperties(list, {
      query: "客",
      onlyUnmapped: true,
      mappingByElement: table,
    })
    expect(got.map((p) => p.iri)).toEqual(["http://ex/ont#hasCustomer"])
  })

  it("按 kind 拆分且两页计数互不影响", () => {
    const { datatypeProps, objectProps } = splitTtlProperties(list)
    expect(datatypeProps).toHaveLength(2)
    expect(objectProps).toHaveLength(1)
    expect(datatypeProps.every((p) => p.kind === "datatype")).toBe(true)
    expect(objectProps.every((p) => p.kind === "object")).toBe(true)
  })
})

describe("groupByDomain", () => {
  it("按 domain 分组，未指定 domain 垫底", () => {
    const groups = groupByDomain(
      [
        prop("a", "datatype", { domain: "" }),
        prop("b", "datatype", { domain: "http://ex/ont#Order" }),
        prop("c", "datatype", { domain: "http://ex/ont#Customer" }),
      ],
      new Map([
        ["http://ex/ont#Order", "订单"],
        ["http://ex/ont#Customer", "客户"],
      ]),
    )
    expect(groups.map(([k]) => k)).toEqual([
      "http://ex/ont#Order",
      "http://ex/ont#Customer",
      "",
    ])
    expect(groups[0][1][0].iri).toBe("b")
    expect(groups[2][1][0].iri).toBe("a")
  })

  it("labelMap 缺失时回退到 IRI 短名，不抛错", () => {
    const groups = groupByDomain(
      [
        prop("b", "datatype", { domain: "http://ex/ont#Order" }),
        prop("c", "datatype", { domain: "http://ex/ont#Customer" }),
      ],
      undefined,
    )
    // 按短名 Customer < Order
    expect(groups.map(([k]) => k)).toEqual([
      "http://ex/ont#Customer",
      "http://ex/ont#Order",
    ])
  })

  it("空列表返回空分组", () => {
    expect(groupByDomain([], undefined)).toEqual([])
  })
})