// UI 层：元素映射绑定层的共享组件（决策 25）。
//
// 两套标准（Palantir / W3C）共用同一套视觉语言与交互：
//   - 状态点：已核对 / 未核对 / 未映射（三态，来自后端 verified 判定）
//   - 来源字段：物理位置定位串（catalog.schema.table.column）
//   - 候选建议：按需触发，勾选后成为映射（建议本身不落库）
//   - 覆盖率卡与缺口清单：类 / 属性 / 关系分别计数，不互相推导
//
// 组件只通过 hooks 取数（不直接 invoke），变更后由 hook 统一 invalidate。

import { useMemo, useState } from "react"
import {
  AlertTriangle,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  Loader2,
  Plus,
  Sparkles,
  Trash2,
  X,
} from "lucide-react"
import { message } from "@tauri-apps/plugin-dialog"
import {
  useElementMappings,
  useDeleteElementMappings,
  useExportElementMapping,
  useMappingCoverage,
  useMappingElements,
  useMappingIntegrity,
  usePurgeOrphanMappings,
  useSaveElementMappings,
  useSuggestElementMappings,
  type MappingStandard,
} from "@/hooks/useElementMapping"
import type {
  ElementMapping,
  ElementMappingInput,
  MappingCoverage,
  MappingElementRef,
  MappingEndpoint,
  MappingIntegrityReport,
  MappingSuggestion,
} from "@/lib/domain"
import {
  coveragePercent,
  coverageSummary,
  elementDisplayName,
  elementTypeLabel,
  endpointLocator,
  exportBlockedReason,
  gapCount,
  integrityGroups,
  isIncompleteMapping,
  mappingExportFileName,
  mappingLocator,
  mappingStateOf,
  MAPPING_STATE_META,
  suggestionPercent,
  unmappedElements,
  unverifiedElements,
  type MappingState,
} from "@/lib/mapping-display"
import { pickSavePath } from "@/lib/save-file"

/** 元素缓存键（element_type + element_id 唯一确定一个元素）。 */
function elementKey(elementType: string, elementId: string): string {
  return `${elementType}\u0000${elementId}`
}

/** 状态点 + 文字（列表紧凑版）。 */
export function MappingStateBadge({
  state,
  count,
}: {
  state: MappingState
  count?: number
}) {
  const meta = MAPPING_STATE_META[state]
  return (
    <span
      className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] ${meta.badgeCls}`}
      title={meta.hint}
    >
      <span className={`h-1.5 w-1.5 rounded-full ${meta.dotCls}`} />
      {meta.label}
      {count !== undefined && count > 0 && (
        <span className="tabular-nums opacity-70">{count}</span>
      )}
    </span>
  )
}

/** 来源字段单元格（列表列）：有映射显示定位串，无映射显示占位。 */
export function SourceFieldCell({
  rows,
  onCopy,
}: {
  rows: ElementMapping[]
  onCopy?: (text: string) => void
}) {
  if (rows.length === 0) {
    return <span className="text-[10px] text-fg-subtle">未映射</span>
  }
  const locators = rows.map((r) => mappingLocator(r)).filter((s) => s.length > 0)
  const first = locators[0] ?? ""
  const more = locators.length - 1
  return (
    <span className="inline-flex items-center gap-1">
      <span
        className="max-w-[180px] truncate font-mono text-[10px] text-fg-muted"
        title={locators.join("\n")}
      >
        {first}
      </span>
      {more > 0 && (
        <span className="text-[10px] text-fg-subtle">+{more}</span>
      )}
      {isIncompleteMapping(rows[0]) && (
        <span title="关系只绑定了一端">
          <AlertTriangle size={10} className="text-warning" />
        </span>
      )}
      {onCopy && first && (
        <button
          onClick={() => onCopy(locators.join("\n"))}
          className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover hover:text-accent"
          title="复制来源字段"
        >
          <Copy size={10} />
        </button>
      )}
    </span>
  )
}

/**
 * 映射编辑器（单个元素）：展示当前映射 + 新增/编辑/删除 + 候选建议。
 *
 * 「来源字段」与概念定义（label/comment/domain/range）分区呈现：
 * 编辑映射不改变概念定义本身。
 */
export function ElementMappingEditor({
  standard,
  ontologyKey,
  elementType,
  elementId,
  displayName,
  dataType,
  sameTableHint,
  readOnly = false,
}: {
  standard: MappingStandard
  ontologyKey: string
  elementType: string
  elementId: string
  displayName?: string
  dataType?: string
  sameTableHint?: string
  readOnly?: boolean
}) {
  const { data: allRows = [], isLoading } = useElementMappings(
    standard,
    ontologyKey,
  )
  const rows = useMemo(
    () =>
      allRows.filter(
        (r) =>
          r.element_type === elementType && r.element_id === elementId,
      ),
    [allRows, elementType, elementId],
  )
  const save = useSaveElementMappings()
  const remove = useDeleteElementMappings()
  const suggest = useSuggestElementMappings()
  const [draft, setDraft] = useState<MappingEndpoint | null>(null)
  const [target, setTarget] = useState<MappingEndpoint | null>(null)
  const [note, setNote] = useState("")
  const [suggestions, setSuggestions] = useState<MappingSuggestion[] | null>(
    null,
  )
  const busy = save.isPending || remove.isPending

  const state = mappingStateOf(rows)

  function resetDraft() {
    setDraft(null)
    setTarget(null)
    setNote("")
  }

  /** 落库：按元素整组替换（保留既有行 + 更新）。 */
  async function persist(next: ElementMappingInput[]) {
    try {
      const res = await save.mutateAsync({
        standard,
        ontologyKey,
        items: [{ element_type: elementType, element_id: elementId, mappings: next }],
      })
      const warn = res.warnings ?? []
      if (warn.length > 0) {
        await message(warn.join("\n"), { kind: "warning" })
      }
      resetDraft()
      setSuggestions(null)
    } catch (e) {
      await message(`保存失败：${e instanceof Error ? e.message : String(e)}`, {
        kind: "error",
      })
    }
  }

  function toInput(m: ElementMapping): ElementMappingInput {
    return {
      source: m.source ?? {},
      target: m.target ?? {},
      source_kind: m.source_kind || "manual",
      source_ref: m.source_ref ?? "",
      note: m.note ?? "",
    }
  }

  async function addMapping() {
    const src = draft
    if (!src || !(src.table ?? "").trim()) {
      await message("请先填写来源表", { kind: "warning" })
      return
    }
    const hasTarget = (target?.table ?? "").trim().length > 0
    const input: ElementMappingInput = {
      source: {
        catalog: src.catalog ?? "",
        schema: src.schema ?? "",
        table: (src.table ?? "").trim(),
        column: (src.column ?? "").trim(),
      },
      target: hasTarget
        ? {
            catalog: target?.catalog ?? "",
            schema: target?.schema ?? "",
            table: (target?.table ?? "").trim(),
            column: (target?.column ?? "").trim(),
          }
        : {},
      source_kind: "manual",
      note: note.trim(),
    }
    await persist([...rows.map(toInput), input])
  }

  async function deleteRow(row: ElementMapping) {
    const next = rows.filter((r) => r.id !== row.id).map(toInput)
    await persist(next)
  }

  async function runSuggest() {
    try {
      const out = await suggest.mutateAsync({
        standard,
        ontologyKey,
        elements: [
          {
            element_type: elementType,
            element_id: elementId,
            display_name: displayName ?? "",
            data_type: dataType ?? "",
            same_table_hint: sameTableHint ?? "",
          },
        ],
      })
      setSuggestions(out)
      if (out.length === 0) {
        await message(
          "没有找到匹配的候选。可先注册数据源，或直接填写来源表和字段。",
          { kind: "info" },
        )
      }
    } catch (e) {
      await message(`获取候选失败：${e instanceof Error ? e.message : String(e)}`, {
        kind: "error",
      })
    }
  }

  async function adoptSuggestion(s: MappingSuggestion) {
    const input: ElementMappingInput = {
      source: s.candidate,
      source_kind: "manual",
      note: "",
    }
    await persist([...rows.map(toInput), input])
  }

  const inputCls =
    "w-full rounded-md border border-border bg-bg-base px-2 py-1 text-xs"

  return (
    <div className="space-y-2 rounded-lg border border-border bg-bg-base p-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-1.5 text-xs font-medium text-fg-muted">
          来源字段
          <MappingStateBadge state={state} count={rows.length} />
        </div>
        {!readOnly && (
          <div className="flex items-center gap-1">
            <button
              onClick={runSuggest}
              disabled={suggest.isPending || busy}
              className="flex items-center gap-1 rounded-md border border-border px-2 py-0.5 text-[10px] text-fg-muted hover:bg-bg-hover disabled:opacity-50"
              title="按名称与类型匹配数据源字段"
            >
              {suggest.isPending ? (
                <Loader2 size={10} className="animate-spin" />
              ) : (
                <Sparkles size={10} />
              )}
              匹配候选
            </button>
          </div>
        )}
      </div>

      {isLoading ? (
        <div className="flex items-center gap-1 text-[10px] text-fg-subtle">
          <Loader2 size={10} className="animate-spin" /> 读取中…
        </div>
      ) : rows.length === 0 ? (
        <p className="text-[10px] text-fg-subtle">
          尚未指定来源字段。填写后即可把该元素映射到数据源的列。
        </p>
      ) : (
        <div className="space-y-1">
          {rows.map((r) => (
            <div
              key={r.id}
              className="flex items-center justify-between gap-2 rounded bg-bg-hover/40 px-2 py-1"
            >
              <div className="min-w-0">
                <div className="truncate font-mono text-[11px] text-fg-muted">
                  {mappingLocator(r)}
                </div>
                {r.note && (
                  <div className="truncate text-[10px] text-fg-subtle">
                    {r.note}
                  </div>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <MappingStateBadge
                  state={r.verified ? "verified" : "unverified"}
                />
                {!readOnly && (
                  <button
                    onClick={() => deleteRow(r)}
                    disabled={busy}
                    className="rounded p-0.5 text-fg-subtle hover:bg-danger-bg hover:text-danger disabled:opacity-50"
                    title="移除这条映射"
                  >
                    <Trash2 size={11} />
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}

      {/* 候选建议 */}
      {suggestions && suggestions.length > 0 && !readOnly && (
        <div className="space-y-1 rounded-md border border-accent/30 bg-accent/5 p-2">
          <div className="flex items-center justify-between">
            <span className="text-[10px] font-medium text-accent">
              候选（点选后加入映射）
            </span>
            <button
              onClick={() => setSuggestions(null)}
              className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover"
              title="收起"
            >
              <X size={10} />
            </button>
          </div>
          {suggestions.map((s, i) => (
            <button
              key={`${s.candidate.table}.${s.candidate.column}.${i}`}
              onClick={() => adoptSuggestion(s)}
              disabled={busy}
              className="flex w-full items-center justify-between gap-2 rounded px-1.5 py-1 text-left hover:bg-accent/10 disabled:opacity-50"
            >
              <span className="min-w-0">
                <span className="block truncate font-mono text-[10px] text-fg-muted">
                  {endpointLocator(s.candidate)}
                </span>
                {(s.reasons ?? []).length > 0 && (
                  <span className="block truncate text-[9px] text-fg-subtle">
                    {(s.reasons ?? []).join(" · ")}
                  </span>
                )}
              </span>
              <span className="shrink-0 tabular-nums text-[10px] text-accent">
                {suggestionPercent(s.score)}%
              </span>
            </button>
          ))}
        </div>
      )}

      {/* 手动新增 */}
      {!readOnly && (
        draft ? (
          <div className="space-y-1.5 border-t border-border pt-2">
            <div className="grid grid-cols-2 gap-1.5">
              <input
                value={draft.table ?? ""}
                onChange={(e) => setDraft({ ...draft, table: e.target.value })}
                placeholder="来源表"
                className={inputCls}
              />
              <input
                value={draft.column ?? ""}
                onChange={(e) => setDraft({ ...draft, column: e.target.value })}
                placeholder="来源字段（留空=整表）"
                className={inputCls}
              />
              <input
                value={draft.catalog ?? ""}
                onChange={(e) =>
                  setDraft({ ...draft, catalog: e.target.value })
                }
                placeholder="数据源（可选）"
                className={inputCls}
              />
              <input
                value={draft.schema ?? ""}
                onChange={(e) => setDraft({ ...draft, schema: e.target.value })}
                placeholder="schema（可选）"
                className={inputCls}
              />
            </div>
            {elementType === "object_property" ||
            elementType === "link" ? (
              <div className="grid grid-cols-2 gap-1.5">
                <input
                  value={target?.table ?? ""}
                  onChange={(e) =>
                    setTarget({ ...(target ?? {}), table: e.target.value })
                  }
                  placeholder="目标表（关系另一端）"
                  className={inputCls}
                />
                <input
                  value={target?.column ?? ""}
                  onChange={(e) =>
                    setTarget({ ...(target ?? {}), column: e.target.value })
                  }
                  placeholder="目标字段"
                  className={inputCls}
                />
              </div>
            ) : null}
            <input
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder="备注（可选）"
              className={inputCls}
            />
            <div className="flex justify-end gap-1.5">
              <button
                onClick={resetDraft}
                disabled={busy}
                className="rounded-md px-2 py-0.5 text-[10px] text-fg-muted hover:bg-bg-hover"
              >
                取消
              </button>
              <button
                onClick={addMapping}
                disabled={busy}
                className="flex items-center gap-1 rounded-md bg-accent px-2 py-0.5 text-[10px] font-medium text-white disabled:opacity-50"
              >
                {busy ? (
                  <Loader2 size={10} className="animate-spin" />
                ) : (
                  <Check size={10} />
                )}
                保存
              </button>
            </div>
          </div>
        ) : (
          <button
            onClick={() => setDraft({ table: "", column: "" })}
            disabled={busy}
            className="flex items-center gap-1 rounded-md border border-dashed border-border px-2 py-1 text-[10px] text-fg-subtle hover:border-accent/40 hover:text-accent disabled:opacity-50"
          >
            <Plus size={10} />
            添加来源字段
          </button>
        )
      )}
    </div>
  )
}

/** 映射列表的通用 hook：元素键 → 映射行。 */
export function useMappingRowsByElement(
  standard: MappingStandard,
  ontologyKey: string | null,
) {
  const q = useElementMappings(standard, ontologyKey)
  const map = useMemo(() => {
    const m = new Map<string, ElementMapping[]>()
    for (const r of q.data ?? []) {
      const key = elementKey(r.element_type, r.element_id)
      const list = m.get(key)
      if (list) list.push(r)
      else m.set(key, [r])
    }
    return m
  }, [q.data])
  return { rows: q.data ?? [], byElement: map, isLoading: q.isLoading }
}

/** 覆盖率卡：类/属性/关系三桶分别呈现，点击下钻缺口清单。 */
export function MappingCoverageCard({
  coverage,
  onDrilldown,
  title = "来源字段覆盖率",
}: {
  coverage: MappingCoverage | undefined
  onDrilldown?: () => void
  title?: string
}) {
  if (!coverage) return null
  const buckets = [
    { key: "classes", label: "类", bucket: coverage.classes },
    { key: "properties", label: "属性", bucket: coverage.properties },
    { key: "relations", label: "关系", bucket: coverage.relations },
  ]
  return (
    <div className="rounded-lg border border-border p-3">
      <div className="mb-2 flex items-center justify-between">
        <span className="text-xs font-medium text-fg-muted">{title}</span>
        {onDrilldown && (
          <button
            onClick={onDrilldown}
            className="text-[10px] text-accent hover:underline"
          >
            查看缺口
          </button>
        )}
      </div>
      <div className="grid grid-cols-3 gap-2">
        {buckets.map(({ key, label, bucket }) => {
          const mapped = bucket?.mapped ?? 0
          const total = bucket?.total ?? 0
          const pct = coveragePercent(bucket ?? { mapped: 0, total: 0 })
          const full = total > 0 && mapped >= total
          return (
            <div key={key} className="rounded-md bg-bg-hover/40 px-2 py-1.5">
              <div className="flex items-baseline justify-between">
                <span className="text-[10px] text-fg-subtle">{label}</span>
                <span className="tabular-nums text-[10px] text-fg-subtle">
                  {pct}%
                </span>
              </div>
              <div className="text-sm font-semibold tabular-nums">
                {mapped}
                <span className="text-[10px] font-normal text-fg-subtle">
                  /{total}
                </span>
              </div>
              <div className="mt-1 h-1 overflow-hidden rounded bg-border">
                <div
                  className={`h-full rounded ${full ? "bg-success" : "bg-accent"}`}
                  style={{ width: `${pct}%` }}
                />
              </div>
            </div>
          )
        })}
      </div>
      <p className="mt-2 text-[10px] text-fg-subtle">
        {coverageSummary(coverage)}
      </p>
    </div>
  )
}

/** 完整性报告问题清单（孤儿/结构变化/待核对，均不自动处理）。 */
export function MappingIntegrityList({
  report,
  onPurgeOrphans,
}: {
  report: MappingIntegrityReport | undefined
  onPurgeOrphans?: () => void
}) {
  const groups = integrityGroups(report)
  if (groups.length === 0) return null
  return (
    <div className="space-y-2 rounded-lg border border-warning/30 bg-warning-bg/30 p-3">
      <div className="flex items-center gap-1.5 text-xs font-medium text-warning-text">
        <AlertTriangle size={12} />
        来源字段有待处理项
      </div>
      {groups.map((g) => (
        <div key={g.key}>
          <div className="mb-1 flex items-center justify-between">
            <span className="text-[10px] font-medium text-fg-muted">
              {g.label}（{g.items.length}）
            </span>
            {g.key === "orphans" && onPurgeOrphans && (
              <button
                onClick={onPurgeOrphans}
                className="text-[10px] text-danger hover:underline"
              >
                清理
              </button>
            )}
          </div>
          <div className="space-y-0.5">
            {g.items.slice(0, 20).map((item) => (
              <div
                key={item.mapping_id}
                className="truncate text-[10px] text-fg-subtle"
                title={item.detail}
              >
                {elementTypeLabel(item.element_type)}·{item.element_id}
                {item.locator ? `（${item.locator}）` : ""}
              </div>
            ))}
            {g.items.length > 20 && (
              <div className="text-[10px] text-fg-subtle">
                还有 {g.items.length - 20} 项…
              </div>
            )}
          </div>
        </div>
      ))}
      <p className="text-[10px] text-fg-subtle/80">
        这些项不会被自动处理，确认后再做清理或重新绑定。
      </p>
    </div>
  )
}

/** 缺口清单：未映射 + 未核对，可逐项补映射。 */
export function MappingGapPanel({
  standard,
  ontologyKey,
  showIntegrity = true,
}: {
  standard: MappingStandard
  ontologyKey: string
  showIntegrity?: boolean
}) {
  const { rows, byElement, isLoading } = useMappingRowsByElement(
    standard,
    ontologyKey,
  )
  const { data: elements = [], isLoading: loadingElements } =
    useMappingElements(standard, ontologyKey)
  const coverage = useMappingCoverage(standard, ontologyKey)
  const integrity = useMappingIntegrity(standard, ontologyKey)
  const purge = usePurgeOrphanMappings()
  const [editing, setEditing] = useState<string | null>(null)
  const [filter, setFilter] = useState("")

  const gaps = useMemo(() => {
    const unmapped = unmappedElements(elements, rows).map((e) => ({
      element: e,
      kind: "unmapped" as const,
    }))
    const unverified = unverifiedElements(elements, rows).map((e) => ({
      element: e,
      kind: "unverified" as const,
    }))
    const all = [...unmapped, ...unverified]
    const q = filter.trim().toLowerCase()
    if (!q) return all
    return all.filter(
      (g) =>
        g.element.element_id.toLowerCase().includes(q) ||
        (g.element.display_name ?? "").toLowerCase().includes(q),
    )
  }, [elements, rows, filter])

  async function handlePurge() {
    try {
      const n = await purge.mutateAsync({ standard, ontologyKey })
      await message(n > 0 ? `已清理 ${n} 条` : "没有需要清理的项", {
        kind: "info",
      })
    } catch (e) {
      await message(`清理失败：${e instanceof Error ? e.message : String(e)}`, {
        kind: "error",
      })
    }
  }

  if (isLoading || loadingElements) {
    return (
      <div className="flex items-center gap-1.5 p-4 text-xs text-fg-subtle">
        <Loader2 size={12} className="animate-spin" /> 读取来源字段状态…
      </div>
    )
  }

  return (
    <div className="space-y-3 p-4">
      <MappingCoverageCard coverage={coverage.data} />
      {showIntegrity && (
        <MappingIntegrityList
          report={integrity.data}
          onPurgeOrphans={handlePurge}
        />
      )}
      <div className="rounded-lg border border-border">
        <div className="flex items-center gap-2 border-b border-border px-3 py-2">
          <span className="text-xs font-medium text-fg-muted">
            缺口（{gapCount(elements, rows)}）
          </span>
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="搜索元素"
            className="ml-auto w-40 rounded-md border border-border bg-bg-base px-2 py-1 text-[11px]"
          />
        </div>
        {gaps.length === 0 ? (
          <p className="px-3 py-4 text-center text-xs text-fg-subtle">
            没有缺口，全部元素都已绑定来源字段
          </p>
        ) : (
          <div className="divide-y divide-border">
            {gaps.map(({ element, kind }) => {
              const key = elementKey(element.element_type, element.element_id)
              const open = editing === key
              const rowsForElement = byElement.get(key) ?? []
              return (
                <div key={key} className="px-3 py-2">
                  <button
                    onClick={() => setEditing(open ? null : key)}
                    className="flex w-full items-center gap-2 text-left"
                  >
                    {open ? (
                      <ChevronDown size={12} className="shrink-0" />
                    ) : (
                      <ChevronRight size={12} className="shrink-0" />
                    )}
                    <span className="shrink-0 rounded bg-bg-hover px-1 text-[10px] text-fg-subtle">
                      {elementTypeLabel(element.element_type)}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-xs">
                      {elementDisplayName(element)}
                    </span>
                    <MappingStateBadge
                      state={kind}
                      count={rowsForElement.length}
                    />
                  </button>
                  {open && (
                    <div className="mt-2 pl-5">
                      <ElementMappingEditor
                        standard={standard}
                        ontologyKey={ontologyKey}
                        elementType={element.element_type}
                        elementId={element.element_id}
                        displayName={element.display_name}
                        dataType={element.data_type}
                        sameTableHint={element.same_table_hint}
                      />
                    </div>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}

/** 导出按钮（单一动作：导出全部元素及其来源字段，未映射的字段留空）。 */
export function MappingExportButton({
  standard,
  ontologyKey,
  coverage,
}: {
  standard: MappingStandard
  ontologyKey: string
  coverage: MappingCoverage | undefined
}) {
  const exportMut = useExportElementMapping()
  const [busy, setBusy] = useState(false)

  async function doExport() {
    const blocked = exportBlockedReason(coverage)
    if (blocked) {
      await message(blocked, { kind: "info" })
      return
    }
    // 先让用户选保存位置（取消则什么也不做），再由后端生成并写入。
    // 工作簿是二进制，经 IPC 回传会整体序列化成 number 数组，大本体开销明显。
    const path = await pickSavePath(mappingExportFileName(ontologyKey), [
      { name: "Excel 工作簿", extensions: ["xlsx"] },
    ])
    if (!path) return
    setBusy(true)
    try {
      await exportMut.mutateAsync({
        standard,
        ontologyKey,
        scope: "all",
        targetPath: path,
      })
      await message(`已导出`, { kind: "info" })
    } catch (e) {
      await message(`导出失败：${e instanceof Error ? e.message : String(e)}`, {
        kind: "error",
      })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex items-center gap-1.5">
      <button
        onClick={doExport}
        disabled={busy}
        className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-[11px] text-fg-muted hover:bg-bg-hover disabled:opacity-50"
        title="导出全部类、属性与关系及其来源字段；未映射的字段留空"
      >
        {busy ? (
          <Loader2 size={11} className="animate-spin" />
        ) : (
          <Download size={11} />
        )}
        导出映射
      </button>
    </div>
  )
}

/** 元素清单为空时的提示文案（供页面复用，避免各处各写一套）。 */
export const MAPPING_EMPTY_HINT =
  "本体里还没有可映射的元素，先完成类与属性的定义"

export type { MappingElementRef }