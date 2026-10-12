// Hooks 层：useOntology.ts（三期：本体建模）
// 本体列表 / 导出详情 / 预演导入 / 执行导入的 TanStack Query 封装。
// 对齐 useFederation.ts 模式：queryKey 常量 + useQuery/useMutation。

import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "@/lib/ipc/commands";
import type {
  OntologyPayload,
  ImportPreview,
  ImportResult,
  OntologyChangelog,
  OntologyCharter,
  DatasetDef,
  DataSourceDef,
} from "@/lib/domain";

const QK_LIST = ["ontologies"] as const;
const QK_PAYLOAD = (apiName: string) => ["ontology-payload", apiName] as const;
const QK_CHANGELOG = (apiName: string) =>
  ["ontology-changelog", apiName] as const;
const QK_CHARTER = (apiName: string) => ["ontology-charter", apiName] as const;
const QK_ELEMENT_EVIDENCE = (apiName: string) =>
  ["ontology-element-evidence", apiName] as const;

/** 元素级证据行（docs/EVIDENCE-CHAIN.md L4，后端 element_evidence 表）。 */
export interface ElementEvidenceRow {
  element_type: string;
  element_id: string;
  source_id: string;
  source_title: string;
  source_kind: string;
  quote: string;
  role: "direct" | "inferred";
  /** "exact" | "normalized" | ""（空 = 引用未通过校验：源缺失或 quote 不符） */
  matched_by: string;
  span: [number, number] | null;
  created_at: number;
}

/** 证据三态（docs/EVIDENCE-CHAIN.md §6）：direct 🟢 / inferred 🟡 / ungrounded 🔴。 */
export type EvidenceStatus = "direct" | "inferred" | "manual" | "ungrounded";

/** 三态展示元数据（Palantir 与 W3C TTL 视图共用）。 */
export const EVIDENCE_STATUS_META: Record<
  EvidenceStatus,
  { icon: string; label: string; title: string }
> = {
  direct: { icon: "🟢", label: "有据", title: "有出处引用（direct）" },
  inferred: { icon: "🟡", label: "推断", title: "仅推断依据（inferred）" },
  manual: {
    icon: "⚪",
    label: "手认",
    title: "手工确认：用户显式声明该元素为人工设计，无需源引用",
  },
  ungrounded: {
    icon: "🔴",
    label: "无据",
    title: "无出处引用或引用校验未通过（ungrounded）",
  },
};

/** 由一组证据行判定三态（invalid 视为 ungrounded：🔴）。 */
export function evidenceStatus(rows: ElementEvidenceRow[]): EvidenceStatus {
  if (rows.length === 0) return "ungrounded";
  if (rows.some((r) => r.matched_by === "")) return "ungrounded";
  if (rows.every((r) => r.matched_by === "manual")) return "manual";
  if (rows.every((r) => r.matched_by === "manual" || r.role === "inferred"))
    return "inferred";
  return "direct";
}

/** TTL（W3C）视图的证据状态判定：无证据行的元素视为 ⚪ 手工确认。
 *  语义：用户亲手导入/编辑 TTL = 为定义背书（显式人工确认），非「未经输入佐证」；
 *  🔴 仅保留给「有证据行但引用校验失败」（源被删/quote 失配，曾经有据现在失效）。 */
export function evidenceStatusTtl(rows: ElementEvidenceRow[]): EvidenceStatus {
  if (rows.length === 0) return "manual";
  return evidenceStatus(rows);
}

/** 列出 Palantir 本体的元素级证据（详情面板三态展示）。 */
export function useElementEvidence(apiName: string | null) {
  return useQuery<ElementEvidenceRow[]>({
    queryKey: QK_ELEMENT_EVIDENCE(apiName ?? ""),
    queryFn: async () => {
      const json = await ipc.listElementEvidence("palantir", apiName!);
      return JSON.parse(json) as ElementEvidenceRow[];
    },
    enabled: !!apiName,
  });
}

const QK_DATASETS = ["ontology-datasets"] as const;
const QK_DATA_SOURCES = ["ontology-data-sources"] as const;

/** 已存储本体列表（左栏）。 */
export function useOntologies() {
  return useQuery({
    queryKey: QK_LIST,
    queryFn: () => ipc.listOntologies(),
  });
}

/** 导出本体详情（中栏 ER/表格展示）。 */
export function useOntologyPayload(apiName: string | null) {
  return useQuery<OntologyPayload>({
    queryKey: QK_PAYLOAD(apiName ?? ""),
    queryFn: async () => {
      const json = await ipc.exportOntology(apiName!);
      try {
        return JSON.parse(json) as OntologyPayload;
      } catch (e) {
        throw new Error(`解析本体 payload 失败: ${e}`);
      }
    },
    enabled: !!apiName,
  });
}

/** 列出指定本体下的全部数据集（决策 10 修订：按本体隔离）。 */
export function useOntologyDatasets(ontologyApiName: string | null) {
  return useQuery<DatasetDef[]>({
    queryKey: [...QK_DATASETS, ontologyApiName],
    queryFn: async () => {
      const json = await ipc.listOntologyDatasets(ontologyApiName!);
      try {
        return JSON.parse(json) as DatasetDef[];
      } catch (e) {
        throw new Error(`解析数据集列表失败: ${e}`);
      }
    },
    enabled: !!ontologyApiName,
  });
}

/** 列出指定本体下的全部数据源（决策 10 修订：按本体隔离）。 */
export function useOntologyDataSources(ontologyApiName: string | null) {
  return useQuery<DataSourceDef[]>({
    queryKey: [...QK_DATA_SOURCES, ontologyApiName],
    queryFn: async () => {
      const json = await ipc.listOntologyDataSources(ontologyApiName!);
      try {
        return JSON.parse(json) as DataSourceDef[];
      } catch (e) {
        throw new Error(`解析数据源列表失败: ${e}`);
      }
    },
    enabled: !!ontologyApiName,
  });
}

/** 预演导入（dry-run）。 */
export function usePreviewImport() {
  return useMutation({
    mutationFn: async ({
      payload,
      overwrite,
      overwriteDataSources,
    }: {
      payload: OntologyPayload;
      overwrite: string[];
      overwriteDataSources: string[];
    }): Promise<ImportPreview> => {
      const json = await ipc.previewOntologyImport(
        payload,
        overwrite,
        overwriteDataSources,
      );
      try {
        return JSON.parse(json) as ImportPreview;
      } catch (e) {
        throw new Error(`解析预演结果失败: ${e}`);
      }
    },
  });
}

/** 删除本体（硬删，级联清子表）。成功后刷新列表 + 清除详情缓存。 */
export function useDeleteOntology() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (apiName: string): Promise<boolean> =>
      ipc.deleteOntology(apiName),
    onSuccess: (_deleted, apiName) => {
      // 删除后列表必变；详情缓存主动清除（避免残留选中已删本体）
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.removeQueries({ queryKey: QK_PAYLOAD(apiName) });
    },
  });
}
export function useImportOntology() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({
      payload,
      overwrite,
      overwriteDataSources,
    }: {
      payload: OntologyPayload;
      overwrite: string[];
      overwriteDataSources: string[];
    }): Promise<ImportResult> => {
      const json = await ipc.importOntology(
        payload,
        overwrite,
        overwriteDataSources,
      );
      try {
        return JSON.parse(json) as ImportResult;
      } catch (e) {
        throw new Error(`解析导入结果失败: ${e}`);
      }
    },
    onSuccess: (_data, variables) => {
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.invalidateQueries({
        queryKey: QK_PAYLOAD(variables.payload.api_name),
      });
    },
  });
}

/** 列出本体变更历史（git commit log 式，详情页「历史」Tab 用）。 */
export function useOntologyChangelog(apiName: string | null) {
  return useQuery<OntologyChangelog[]>({
    queryKey: QK_CHANGELOG(apiName ?? ""),
    queryFn: () => ipc.listOntologyChangelog(apiName!),
    enabled: !!apiName,
  });
}

/** 读取本体设计宪章（不变点，详情页头部常驻展示）。 */
export function useOntologyCharter(apiName: string | null) {
  return useQuery<OntologyCharter>({
    queryKey: QK_CHARTER(apiName ?? ""),
    queryFn: () => ipc.getOntologyCharter(apiName!),
    enabled: !!apiName,
  });
}

/** 写入/更新本体设计宪章（只有用户明确要求调整时才调用）。 */
/** 宪章先行（决策 24）：创建空本体 + 设计宪章（仅宪章向导调用）。 */
export function useCreateOntologyWithCharter() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      apiName,
      displayName,
      description,
      charter,
    }: {
      apiName: string;
      displayName: string;
      description: string;
      charter: OntologyCharter;
    }): Promise<null> =>
      ipc.createOntologyWithCharter(apiName, displayName, description, charter),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_LIST });
    },
  });
}

export function useSetOntologyCharter() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      apiName,
      charter,
    }: {
      apiName: string;
      charter: OntologyCharter;
    }): Promise<null> => ipc.setOntologyCharter(apiName, charter),
    onSuccess: (_data, variables) => {
      // charter 不影响实体定义，只失效 charter 缓存（不触发 ontology-changed 事件）
      qc.invalidateQueries({ queryKey: QK_CHARTER(variables.apiName) });
    },
  });
}

/**
 * 监听后端 ontology-changed 事件，失效全部本体查询。
 * 覆盖会话内 agent 工具导入（不经过 IPC 命令层）的场景。
 * 在 OntologyView 挂载时调用。
 */
export function useOntologyChangedListener() {
  const qc = useQueryClient();
  useEffect(() => {
    const unlisten = listen("ontology-changed", () => {
      // 事件不带 api_name：列表 + 所有详情 payload + changelog 全部失效。
      // charter 也一并失效——charter 本身是「不变点」（import/delete 不覆盖它），
      // 但会话内 agent 工具 set_ontology_charter 会直接写库且不经过 IPC 命令层，
      // 走 notify_change 发同一事件；这里失效 charter 缓存让面板刷新（charter
      // 未变时只是无害的重取）。
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.invalidateQueries({ queryKey: ["ontology-payload"] });
      qc.invalidateQueries({ queryKey: ["ontology-changelog"] });
      qc.invalidateQueries({ queryKey: ["ontology-charter"] });
      qc.invalidateQueries({ queryKey: ["ontology-datasets"] });
      qc.invalidateQueries({ queryKey: ["ontology-data-sources"] });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [qc]);
}
