// 元素映射绑定层的展示层纯函数（决策 25）。
//
// 只做「数据 → 视觉/文案」的确定性转换，不发请求、不读 store：
//   - 三态视觉：已核对 / 未核对 / 未映射（与后端 verified 语义一一对应）
//   - 覆盖率文案与缺口过滤
//   - 物理位置定位串与导出文件名
//
// 三态命名与后端保持一致：verified（已核对）、unverified（未核对）、unmapped（未映射）。
// 未映射不是「错误」，只是提示——所有函数都不得把未映射表述为故障。

import type {
  ElementMapping,
  MappingCoverage,
  MappingCoverageBucket,
  MappingElementRef,
  MappingEndpoint,
  MappingIntegrityItem,
  MappingIntegrityReport,
  MappingSuggestion,
} from "@/lib/domain"

/** 映射状态三态。 */
export type MappingState = "verified" | "unverified" | "unmapped"

/** 三态视觉元数据（色值走语义色 token，不写死具体色号）。 */
export interface MappingStateMeta {
  label: string
  /** Tailwind 类：文字 + 背景 + 边框，用于状态点/badge。 */
  badgeCls: string
  /** 状态点圆点类。 */
  dotCls: string
  /** 一句话解释（悬浮提示用，面向用户，不暴露实现）。 */
  hint: string
}

export const MAPPING_STATE_META: Record<MappingState, MappingStateMeta> = {
  verified: {
    label: "已核对",
    badgeCls:
      "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/30",
    dotCls: "bg-emerald-500",
    hint: "来源字段已与数据源结构核对一致",
  },
  unverified: {
    label: "未核对",
    badgeCls:
      "bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/30",
    dotCls: "bg-amber-500",
    hint: "尚未与数据源结构核对",
  },
  unmapped: {
    label: "未映射",
    badgeCls: "bg-muted text-muted-foreground border-border",
    dotCls: "bg-muted-foreground/40",
    hint: "尚未指定来源字段",
  },
}

/** 元素级的映射状态（多条映射时取最强的一个：任一已核对即已核对）。 */
export function mappingStateOf(rows: ElementMapping[]): MappingState {
  if (rows.length === 0) return "unmapped"
  return rows.some((r) => r.verified) ? "verified" : "unverified"
}

/** 物理位置定位串：catalog.schema.table.column（缺省段自动跳过）。 */
export function endpointLocator(ep: MappingEndpoint | undefined | null): string {
  if (!ep) return ""
  const parts = [ep.catalog, ep.schema, ep.table, ep.column]
    .map((v) => (v ?? "").trim())
    .filter((v) => v.length > 0)
  return parts.join(".")
}

/** 映射行的人读描述：来源表.列（关系带 → 目标表.列）。 */
export function mappingLocator(m: ElementMapping): string {
  const from = endpointLocator(m.source)
  const to = endpointLocator(m.target)
  if (!to) return from
  return `${from} → ${to}`
}

/** 是否为「关系单端映射」（有来源无目标，导出时标为不完整）。 */
export function isIncompleteMapping(m: ElementMapping): boolean {
  const hasSource = endpointLocator(m.source).length > 0
  const hasTarget = endpointLocator(m.target).length > 0
  return hasSource !== hasTarget
}

/** 单桶覆盖率百分比（total 为 0 时返回 0，不做除零）。 */
export function coveragePercent(bucket: MappingCoverageBucket): number {
  const total = bucket.total ?? 0
  if (total <= 0) return 0
  return Math.round(((bucket.mapped ?? 0) / total) * 100)
}

/** 覆盖率一句话文案：类 / 属性 / 关系分别计数，不合并。 */
export function coverageSummary(coverage: MappingCoverage | undefined): string {
  if (!coverage) return "暂无映射统计"
  const c = coverage.classes ?? { mapped: 0, total: 0 }
  const p = coverage.properties ?? { mapped: 0, total: 0 }
  const r = coverage.relations ?? { mapped: 0, total: 0 }
  return `类 ${c.mapped}/${c.total} · 属性 ${p.mapped}/${p.total} · 关系 ${r.mapped}/${r.total}`
}

/** 覆盖率是否有缺口（任一类未映射满即视为有缺口）。 */
export function hasCoverageGap(coverage: MappingCoverage | undefined): boolean {
  if (!coverage) return false
  const buckets: Array<MappingCoverageBucket | undefined> = [
    coverage.classes,
    coverage.properties,
    coverage.relations,
  ]
  return buckets.some((b) => b && (b.mapped ?? 0) < (b.total ?? 0))
}

/** 缺口元素：本体元素里没有映射的那些（未映射）。 */
export function unmappedElements(
  elements: MappingElementRef[],
  mappings: ElementMapping[],
): MappingElementRef[] {
  const mapped = new Set(
    mappings.map((m) => `${m.element_type}\u0000${m.element_id}`),
  )
  return elements.filter(
    (e) => !mapped.has(`${e.element_type}\u0000${e.element_id}`),
  )
}

/** 未核对元素：有映射但全部未核对的那些。 */
export function unverifiedElements(
  elements: MappingElementRef[],
  mappings: ElementMapping[],
): MappingElementRef[] {
  const byElement = new Map<string, ElementMapping[]>()
  for (const m of mappings) {
    const key = `${m.element_type}\u0000${m.element_id}`
    const list = byElement.get(key)
    if (list) list.push(m)
    else byElement.set(key, [m])
  }
  return elements.filter((e) => {
    const rows = byElement.get(`${e.element_type}\u0000${e.element_id}`)
    return rows !== undefined && mappingStateOf(rows) === "unverified"
  })
}

/** 缺口清单总数（未映射 + 未核对，不含孤儿——孤儿是另一类问题，单独呈现）。 */
export function gapCount(
  elements: MappingElementRef[],
  mappings: ElementMapping[],
): number {
  return (
    unmappedElements(elements, mappings).length +
    unverifiedElements(elements, mappings).length
  )
}

/** 完整性报告的分组文案（孤儿/失效/未核对三类，互不合并）。 */
export interface IntegrityGroup {
  key: "orphans" | "stale" | "unverified"
  label: string
  hint: string
  items: MappingIntegrityItem[]
}

export function integrityGroups(
  report: MappingIntegrityReport | undefined,
): IntegrityGroup[] {
  if (!report) return []
  return [
    {
      key: "orphans",
      label: "已失效元素",
      hint: "对应元素在本体中已不存在，可选择清理",
      items: report.orphans ?? [],
    },
    {
      key: "stale",
      label: "结构已变化",
      hint: "来源字段在数据源里已变化，建议重新核对",
      items: report.stale ?? [],
    },
    {
      key: "unverified",
      label: "待核对",
      hint: "尚未与数据源结构核对",
      items: report.unverified ?? [],
    },
  ].filter((g) => g.items.length > 0) as IntegrityGroup[]
}

/** 建议置信度的展示百分比（0–1 → 0–100；缺失/非法值按 0）。 */
export function suggestionPercent(score: number | null | undefined): number {
  const v = typeof score === "number" && Number.isFinite(score) ? score : 0
  return Math.max(0, Math.min(100, Math.round(v * 100)))
}

/** 建议按其元素分组（属性卡上按元素展示候选）。 */
export function groupSuggestions(
  suggestions: MappingSuggestion[],
): Map<string, MappingSuggestion[]> {
  const out = new Map<string, MappingSuggestion[]>()
  for (const s of suggestions) {
    const key = `${s.element_type}\u0000${s.element_id}`
    const list = out.get(key)
    if (list) list.push(s)
    else out.set(key, [s])
  }
  return out
}

/**
 * 导出文件名：<本体标识>-元素映射.xlsx。
 *
 * 导出只有一种范围（全部元素），文件名不再带范围后缀。
 * W3C 的本体标识是 IRI，整串做文件名既长又难读，故先取 IRI 末段
 * （`http://ex/onto#Supply` → `Supply`）；其余非法文件名字符统一替换为下划线。
 *
 * **必须**与 Rust 侧 `mapping_export::export_file_name` 保持同一规则
 * （两处各需立刻拿到文件名，无法共享代码，靠测试锁一致性）。
 */
export function mappingExportFileName(ontologyKey: string): string {
  const raw = (ontologyKey ?? "").trim()
  let candidate = raw
  if (raw.includes("://")) {
    const noScheme = raw.replace(/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//, "")
    const segs = noScheme.split(/[#/]/).filter((seg) => seg.length > 0)
    candidate = segs.length > 0 ? segs[segs.length - 1] : noScheme
  }
  const safe = candidate
    .replace(/[\\/:*?"<>|\s]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 80)
  return `${safe || "ontology"}-元素映射.xlsx`
}

/**
 * 导出前置检查：本体里没有任何元素时给提示（而不是生成只有表头的空文件）。
 *
 * 有条目但尚未映射**不**阻止导出——未映射的字段在 CSV 里留空即可。
 */
export function exportBlockedReason(
  coverage: MappingCoverage | undefined,
): string | null {
  if (!coverage) return null
  const totalElements =
    (coverage.classes?.total ?? 0) +
    (coverage.properties?.total ?? 0) +
    (coverage.relations?.total ?? 0)
  if (totalElements > 0) return null
  return "本体里还没有可导出的元素"
}

/** 元素类型的中文标签（列表/卡片/导出提示共用一套口径）。 */
export const ELEMENT_TYPE_LABEL: Record<string, string> = {
  object_type: "对象类型",
  class: "类",
  property: "属性",
  datatype_property: "数据属性",
  link: "关系",
  object_property: "对象属性",
}

export function elementTypeLabel(t: string): string {
  return ELEMENT_TYPE_LABEL[t] ?? t
}

/** 元素显示名（display_name 优先，回退到标识的末段）。 */
export function elementDisplayName(e: MappingElementRef): string {
  if (e.display_name && e.display_name.trim().length > 0) return e.display_name
  const id = e.element_id ?? ""
  const tail = id.split(/[#/]/).filter((s) => s.length > 0).pop()
  return tail ?? id
}