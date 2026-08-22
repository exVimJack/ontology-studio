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
