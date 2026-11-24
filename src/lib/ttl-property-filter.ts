// 属性列表的过滤/分组纯函数（W3C 属性页签）。
//
// 抽到纯函数层的理由（不只是一次性重构）：
//   1. 组件里原先用闭包直接捕获 `onlyUnmapped` / `mappingByElement`，
//      而这些 const 的声明位置在闭包使用点之后 → 渲染期立即执行时命中 TDZ
//      （"Cannot access 'onlyUnmapped' before initialization"）。
//      改成"参数进、结果出"后，依赖由调用点显式传入，不再有声明顺序陷阱。
//   2. 过滤与分组是可单测的确定性变换，按项目约定必须下沉到 `src/lib/`。
//
// 语义保持不变：文本搜索（label / comment / IRI 短名 / 全 IRI / domain / range，
// 大小写不敏感）与「只看未映射」叠加生效；「未映射」沿用映射三态口径。

import type { ElementMapping, TtlPropertyEdit } from "@/lib/domain"
import { mappingStateOf } from "@/lib/mapping-display"

/** 属性过滤条件。 */
export interface TtlPropertyFilter {
  /** 文本搜索串（大小写不敏感；空白串表示不过滤）。 */
  query?: string
  /** 只看尚未指定来源字段的属性。 */
  onlyUnmapped?: boolean
  /** 元素键 → 映射行（来自 useMappingRowsByElement）。 */
  mappingByElement?: Map<string, ElementMapping[]>
}

/** IRI 短名：去尖括号后取 `#` 或 `/` 的最后一段（与界面展示口径一致）。 */
export function iriTail(iri: string): string {
  const s = iri.startsWith("<") && iri.endsWith(">") ? iri.slice(1, -1) : iri
  const parts = s.split(/[#/]/)
  const local = parts.length > 1 ? parts[parts.length - 1] : ""
  return local || s
}

/** 属性在映射表里的元素键（datatype_property / object_property）。 */
export function propertyElementKey(p: TtlPropertyEdit): string {
  const kind = p.kind === "object" ? "object_property" : "datatype_property"
  return `${kind}\u0000${p.iri}`
}

/** 属性当前的映射状态（无映射行 → unmapped）。 */
export function propertyMappingState(
  p: TtlPropertyEdit,
  mappingByElement: Map<string, ElementMapping[]> | undefined,
): ReturnType<typeof mappingStateOf> {
  return mappingStateOf(mappingByElement?.get(propertyElementKey(p)) ?? [])
}

/** 是否命中文本搜索（空白串视作全部命中）。 */
export function propertyMatchesQuery(
  p: TtlPropertyEdit,
  query: string | undefined,
): boolean {
  const q = (query ?? "").trim().toLowerCase()
  if (!q) return true
  const haystack = [
    p.label ?? "",
    p.comment ?? "",
    iriTail(p.iri),
    p.iri,
    p.domain ?? "",
    p.range ?? "",
  ]
    .join("\n")
    .toLowerCase()
  return haystack.includes(q)
}

/** 属性是否通过全部过滤条件。 */
export function propertyPasses(
  p: TtlPropertyEdit,
  filter: TtlPropertyFilter,
): boolean {
  if (!propertyMatchesQuery(p, filter.query)) return false
  if (!filter.onlyUnmapped) return true
  return propertyMappingState(p, filter.mappingByElement) === "unmapped"
}

/** 过滤属性列表（保持入参顺序，不做排序）。 */
export function filterTtlProperties(
  properties: TtlPropertyEdit[],
  filter: TtlPropertyFilter = {},
): TtlPropertyEdit[] {
  return properties.filter((p) => propertyPasses(p, filter))
}

/** 过滤并按 kind 拆成「关系（object）」与「数据属性（datatype）」。 */
export function splitTtlProperties(
  properties: TtlPropertyEdit[],
  filter: TtlPropertyFilter = {},
): {
  datatypeProps: TtlPropertyEdit[]
  objectProps: TtlPropertyEdit[]
} {
  const kept = filterTtlProperties(properties, filter)
  return {
    datatypeProps: kept.filter((p) => p.kind === "datatype"),
    objectProps: kept.filter((p) => p.kind === "object"),
  }
}

/** 按 domain 分组（中文 label 优先，未指定 domain 的组排最后）。 */
export function groupByDomain(
  properties: TtlPropertyEdit[],
  labelMap: Map<string, string> | undefined,
): Array<[string, TtlPropertyEdit[]]> {
  const groups = new Map<string, TtlPropertyEdit[]>()
  for (const p of properties) {
    const key = p.domain ?? ""
    const arr = groups.get(key)
    if (arr) arr.push(p)
    else groups.set(key, [p])
  }
  return [...groups.entries()].sort((a, b) => {
    if (!a[0]) return 1 // 未指定垫底
    if (!b[0]) return -1
    const na = labelMap?.get(a[0]) ?? iriTail(a[0])
    const nb = labelMap?.get(b[0]) ?? iriTail(b[0])
    return na.localeCompare(nb, "zh")
  })
}