// Hooks 层：useActiveScope.ts
// 会话激活集（CONVERSATION-SCOPE.md §2.2）：读取/设置当前会话可见的文件夹 + 数据源。
// 激活集的 documents 部分（@触发的单文件）由 useMountedDocuments 管，本 hook 只管 folders/sources。
//
// 数据流：conversations.active_folders / active_sources 两列（JSON），后端 stream_with_memory
// 时读取并据此过滤 agent 工具（document_tools/federation_tools）。

import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { ipc } from "@/lib/ipc/commands";

const QK = (id: string) => ["active-scope", id] as const;

/** 读取会话激活集（folders + documents + sources）。 */
export function useActiveScope(conversationId: string | null) {
  return useQuery({
    queryKey: conversationId ? QK(conversationId) : ["active-scope", "none"],
    queryFn: () => ipc.getActiveScope(conversationId!),
    enabled: !!conversationId,
  });
}

/** 设置会话激活的文件夹列表（整体覆盖）。空数组 = 清空。 */
export function useSetActiveFolders() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      conversationId,
      folders,
    }: {
      conversationId: string;
      folders: string[];
    }) => ipc.setActiveFolders(conversationId, folders),
    onSuccess: (_d, { conversationId }) => {
      qc.invalidateQueries({ queryKey: QK(conversationId) });
    },
  });
}

/** 设置会话激活的数据源列表（整体覆盖）。空数组 = 清空。 */
export function useSetActiveSources() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      conversationId,
      sources,
    }: {
      conversationId: string;
      sources: string[];
    }) => ipc.setActiveSources(conversationId, sources),
    onSuccess: (_d, { conversationId }) => {
      qc.invalidateQueries({ queryKey: QK(conversationId) });
    },
  });
}

/**
 * 切换某文件夹的激活状态。调用方需传当前 scope（从 useActiveScope 拿）。
 * 已激活 → 移除；未激活 → 追加。
 */
export function useToggleActiveFolder() {
  const setFolders = useSetActiveFolders();
  return useMutation({
    mutationFn: async ({
      conversationId,
      folder,
      currentFolders,
    }: {
      conversationId: string;
      folder: string;
      currentFolders: string[];
    }) => {
      const next = currentFolders.includes(folder)
        ? currentFolders.filter((f) => f !== folder)
        : [...currentFolders, folder];
      await setFolders.mutateAsync({ conversationId, folders: next });
    },
  });
}

/** 切换某数据源的激活状态。 */
export function useToggleActiveSource() {
  const setSources = useSetActiveSources();
  return useMutation({
    mutationFn: async ({
      conversationId,
      source,
      currentSources,
    }: {
      conversationId: string;
      source: string;
      currentSources: string[];
    }) => {
      const next = currentSources.includes(source)
        ? currentSources.filter((s) => s !== source)
        : [...currentSources, source];
      await setSources.mutateAsync({ conversationId, sources: next });
    },
  });
}

/** 设置会话激活的本体列表（整体覆盖）。空数组 = 清空。 */
export function useSetActiveOntologies() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      conversationId,
      ontologies,
    }: {
      conversationId: string;
      ontologies: string[];
    }) => ipc.setActiveOntologies(conversationId, ontologies),
    onSuccess: (_d, { conversationId }) => {
      qc.invalidateQueries({ queryKey: QK(conversationId) });
    },
  });
}
