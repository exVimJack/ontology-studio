// Hooks 层：useElementMapping.ts（决策 25 元素映射绑定层）
//
// TanStack Query 封装映射的读取与变更。约束（§12.2 四层单向依赖）：
//   - 本文件只依赖 ipc + domain，不 import 组件或 store
//   - 变更成功后 invalidate 对应本体的映射相关 query（含覆盖率与完整性报告）
//
// 缓存键以 (standard, ontologyKey) 为根，切换本体时各本体互不污染。

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ipc } from "@/lib/ipc/commands"
import type {
  ElementMapping,
  ElementMappingItem,
  MappingCoverage,
  MappingElementRef,
  MappingIntegrityReport,
  MappingSuggestion,
  MappingWriteResult,
} from "@/lib/domain"

/** 映射标准：palantir（ontology_key = 本体 api_name）或 w3c（= 本体 IRI）。 */
export type MappingStandard = "palantir" | "w3c"

/** 某本体的映射 query 根键（invalidate 用前缀匹配，覆盖所有子键）。 */
export const mappingQueryRoot = (
  standard: MappingStandard,
  ontologyKey: string,
) => ["element-mapping", standard, ontologyKey] as const

const QK_MAPPINGS = (standard: MappingStandard, ontologyKey: string) =>
  [...mappingQueryRoot(standard, ontologyKey), "list"] as const
const QK_ELEMENTS = (standard: MappingStandard, ontologyKey: string) =>
  [...mappingQueryRoot(standard, ontologyKey), "elements"] as const
const QK_COVERAGE = (standard: MappingStandard, ontologyKey: string) =>
  [...mappingQueryRoot(standard, ontologyKey), "coverage"] as const
const QK_INTEGRITY = (standard: MappingStandard, ontologyKey: string) =>
  [...mappingQueryRoot(standard, ontologyKey), "integrity"] as const

/** 供其他模块拼 key 用（避免各处硬编码字符串）。 */
export const mappingQueryKeys = {
  mappings: QK_MAPPINGS,
  elements: QK_ELEMENTS,
  coverage: QK_COVERAGE,
  integrity: QK_INTEGRITY,
}

/** 某本体的全部映射行。 */
export function useElementMappings(
  standard: MappingStandard,
  ontologyKey: string | null,
  enabled = true,
) {
  return useQuery<ElementMapping[]>({
    queryKey: QK_MAPPINGS(standard, ontologyKey ?? ""),
    queryFn: () => ipc.listElementMappings(standard, ontologyKey!),
    enabled: enabled && !!ontologyKey,
  })
}

/** 某元素的映射行（属性卡/关系卡按需取，不必拉全量）。 */
export function useElementMappingsFor(
  standard: MappingStandard,
  ontologyKey: string | null,
  elementType: string | null,
  elementId: string | null,
) {
  return useQuery<ElementMapping[]>({
    queryKey: [
      ...mappingQueryRoot(standard, ontologyKey ?? ""),
      "element",
      elementType ?? "",
      elementId ?? "",
    ] as const,
    queryFn: () =>
      ipc.listElementMappingsFor(
        standard,
        ontologyKey!,
        elementType!,
        elementId!,
      ),
    enabled: !!ontologyKey && !!elementType && !!elementId,
  })
}

/** 本体元素全集（缺口清单与候选建议的输入）。 */
export function useMappingElements(
  standard: MappingStandard,
  ontologyKey: string | null,
  enabled = true,
) {
  return useQuery<MappingElementRef[]>({
    queryKey: QK_ELEMENTS(standard, ontologyKey ?? ""),
    queryFn: () => ipc.listMappingElements(standard, ontologyKey!),
    enabled: enabled && !!ontologyKey,
  })
}

/** 覆盖率（概览卡）。 */
export function useMappingCoverage(
  standard: MappingStandard,
  ontologyKey: string | null,
  enabled = true,
) {
  return useQuery<MappingCoverage>({
    queryKey: QK_COVERAGE(standard, ontologyKey ?? ""),
    queryFn: () => ipc.mappingCoverage(standard, ontologyKey!),
    enabled: enabled && !!ontologyKey,
  })
}

/** 完整性报告（孤儿/失效/未核对）。 */
export function useMappingIntegrity(
  standard: MappingStandard,
  ontologyKey: string | null,
  enabled = true,
) {
  return useQuery<MappingIntegrityReport>({
    queryKey: QK_INTEGRITY(standard, ontologyKey ?? ""),
    queryFn: () => ipc.mappingIntegrity(standard, ontologyKey!),
    enabled: enabled && !!ontologyKey,
  })
}

/** 变更后统一失效该本体的映射相关缓存。 */
function useInvalidateMapping() {
  const qc = useQueryClient()
  return (standard: MappingStandard, ontologyKey: string) =>
    qc.invalidateQueries({
      queryKey: mappingQueryRoot(standard, ontologyKey),
    })
}

/** 保存映射（按元素整组替换）。 */
export function useSaveElementMappings() {
  const invalidate = useInvalidateMapping()
  return useMutation<
    MappingWriteResult,
    unknown,
    {
      standard: MappingStandard
      ontologyKey: string
      items: ElementMappingItem[]
    }
  >({
    mutationFn: (vars) =>
      ipc.saveElementMappings(vars.standard, vars.ontologyKey, vars.items),
    onSuccess: (_r, vars) => invalidate(vars.standard, vars.ontologyKey),
  })
}

/** 删除某元素的全部映射。 */
export function useDeleteElementMappings() {
  const invalidate = useInvalidateMapping()
  return useMutation<
    number,
    unknown,
    {
      standard: MappingStandard
      ontologyKey: string
      elementType: string
      elementId: string
    }
  >({
    mutationFn: (vars) =>
      ipc.deleteElementMappings(
        vars.standard,
        vars.ontologyKey,
        vars.elementType,
        vars.elementId,
      ),
    onSuccess: (_r, vars) => invalidate(vars.standard, vars.ontologyKey),
  })
}

/** 清理孤儿映射（调用方负责先做用户确认）。 */
export function usePurgeOrphanMappings() {
  const invalidate = useInvalidateMapping()
  return useMutation<
    number,
    unknown,
    { standard: MappingStandard; ontologyKey: string }
  >({
    mutationFn: (vars) => ipc.purgeOrphanMappings(vars.standard, vars.ontologyKey),
    onSuccess: (_r, vars) => invalidate(vars.standard, vars.ontologyKey),
  })
}

/**
 * 候选建议（按需触发，不用 useQuery——建议是「用户点了才查」的动作，
 * 不是页面进入即拉的常态数据；避免每次打开属性卡都跑一遍匹配）。
 */
export function useSuggestElementMappings() {
  return useMutation<
    MappingSuggestion[],
    unknown,
    {
      standard: MappingStandard
      ontologyKey: string
      elements?: MappingElementRef[]
    }
  >({
    mutationFn: (vars) =>
      ipc.suggestElementMappings(
        vars.standard,
        vars.ontologyKey,
        vars.elements,
      ),
  })
}

/** 历史来源字段回填（升级/导入后调用；幂等）。 */
export function useBackfillElementMappings() {
  const invalidate = useInvalidateMapping()
  return useMutation<
    number,
    unknown,
    { ontologyKey: string; standard?: MappingStandard }
  >({
    mutationFn: (vars) => ipc.backfillElementMappings(vars.ontologyKey),
    onSuccess: (_r, vars) =>
      invalidate(vars.standard ?? "palantir", vars.ontologyKey),
  })
}

/** 导出映射为多工作表工作簿并落盘（返回实际写入路径）。 */
export function useExportElementMapping() {
  return useMutation<
    string,
    unknown,
    {
      standard: MappingStandard
      ontologyKey: string
      scope: "all" | "gap"
      targetPath: string
    }
  >({
    mutationFn: (vars) =>
      ipc.exportElementMapping(
        vars.standard,
        vars.ontologyKey,
        vars.scope,
        vars.targetPath,
      ),
  })
}

export type { ElementMappingItem }