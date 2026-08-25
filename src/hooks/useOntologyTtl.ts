// Hooks 层：useOntologyTtl.ts（W3C Turtle 本体，对齐 skill ontology-modeling-w3c）
// 对齐 useOntology.ts 模式：queryKey 常量 + useQuery/useMutation。
// 覆盖 list / export / validate / import / delete / query_sparql / charter / changelog。

import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "@/lib/ipc/commands";
import type {
  TtlOntologySummary,
  TtlValidation,
  TtlImportResult,
  TtlCharter,
  TtlChangelog,
} from "@/lib/domain";

const QK_LIST = ["ontology-ttl"] as const;
const QK_CONTENT = (iri: string) => ["ontology-ttl-content", iri] as const;
const QK_CHANGELOG = (iri: string) => ["ontology-ttl-changelog", iri] as const;
const QK_CHARTER = (iri: string) => ["ontology-ttl-charter", iri] as const;

/** 已存 W3C 本体列表（左栏）。 */
export function useOntologyTtls() {
  return useQuery({
    queryKey: QK_LIST,
    queryFn: () => ipc.listOntologyTtl(),
  });
}

/** 导出本体 Turtle 文本（中栏源码展示 / 增量更新起点）。 */
export function useOntologyTtlContent(iri: string | null) {
  return useQuery<string>({
    queryKey: QK_CONTENT(iri ?? ""),
    queryFn: () => ipc.exportOntologyTtl(iri!),
    enabled: !!iri,
  });
}

/** 校验 Turtle（dry-run）。 */
export function useValidateOntologyTtl() {
  return useMutation({
    mutationFn: (ttl: string): Promise<TtlValidation> =>
      ipc.validateOntologyTtl(ttl),
  });
}

/** 导入 Turtle。 */
export function useImportOntologyTtl() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      ttl,
      overwrite,
    }: {
      ttl: string;
      overwrite: boolean;
    }): Promise<TtlImportResult> => ipc.importOntologyTtl(ttl, overwrite),
    onSuccess: (_data) => {
      // import 成功后列表必变；缓存失效让详情刷新
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-content"] });
    },
  });
}

/** 删除本体（幂等，级联清 charter/changelog）。 */
export function useDeleteOntologyTtl() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (iri: string): Promise<boolean> => ipc.deleteOntologyTtl(iri),
    onSuccess: (_deleted, iri) => {
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.removeQueries({ queryKey: QK_CONTENT(iri) });
      qc.removeQueries({ queryKey: QK_CHARTER(iri) });
      qc.removeQueries({ queryKey: QK_CHANGELOG(iri) });
    },
  });
}

/** SPARQL 查询（第 7 类 CRUD）。返回 SPARQL Results JSON 字符串。 */
export function useQueryOntologySparql() {
  return useMutation({
    mutationFn: ({
      ontologyIri,
      sparql,
    }: {
      ontologyIri: string;
      sparql: string;
    }): Promise<string> => ipc.queryOntologySparql(ontologyIri, sparql),
  });
}

/** 一次性查询某本体的所有中文 label，构建 iri→中文label 映射。
 *  这是正确的 label 解析方式：一次查询、全局复用、不在业务查询里拼 OPTIONAL
 *  （避免 oxigraph 并列 OPTIONAL 对 unbound 变量全图扫描导致笛卡尔积爆炸）。 */
export function useTtlLabelMap(iri: string | null) {
  return useQuery<Map<string, string>>({
    queryKey: ["ontology-ttl-labels", iri],
    enabled: !!iri,
    queryFn: async () => {
      const sparql =
        'SELECT ?s ?label WHERE { ?s rdfs:label ?label . FILTER(LANGMATCHES(LANG(?label), "zh")) }';
      const json = await ipc.queryOntologySparql(iri!, sparql);
      const m = new Map<string, string>();
      try {
        const parsed = JSON.parse(json);
        for (const b of parsed.results?.bindings ?? []) {
          const s = b.s?.value;
          const label = b.label?.value;
          if (s && label) m.set(s, label);
        }
      } catch {
        /* 返回空 Map */
      }
      return m;
    },
    staleTime: Infinity,
    retry: false,
    refetchOnWindowFocus: false,
  });
}

/** SPARQL 只读查询（带缓存）。
 *  展示场景（图/树/详情/规则）全是 SELECT，走 useQuery：
 *  相同 (iri, sparql) 自动去重缓存，StrictMode 双跑只发一次 IPC，避免并发解析同本体卡死。
 *  返回解析后的行数组（每行是变量名->值的映射，取 value 字段）。 */
export function useSparqlQueryRead(iri: string, sparql: string) {
  return useQuery<Record<string, string>[]>({
    queryKey: ["ontology-ttl-sparql", iri, sparql],
    queryFn: async () => {
      const json = await ipc.queryOntologySparql(iri, sparql);
      try {
        const parsed = JSON.parse(json);
        const bindings = parsed.results?.bindings ?? [];
        return bindings.map((b: Record<string, { value: string }>) => {
          const row: Record<string, string> = {};
          for (const [k, v] of Object.entries(b)) {
            row[k] = v.value;
          }
          return row;
        });
      } catch {
        // JSON 解析失败（后端返回非标准 Results JSON）→ 返回空，由 error 通道处理
        return [];
      }
    },
    // 展示查询：只读 SELECT，关闭重试与自动 refetch，staleTime=Infinity（与改动前 mutation 行为一致：
    //  仅在组件挂载/依赖变化时发一次，不自动刷新），避免 StrictMode + retry 叠加产生请求风暴。
    //  权衡：本体编辑后不会自动刷新视图，需手动点刷新；展示场景可接受。
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnMount: false,
    staleTime: Infinity,
    gcTime: 0,
  });
}

/** 读取本体设计宪章（不变点）。 */
export function useOntologyTtlCharter(iri: string | null) {
  return useQuery<TtlCharter>({
    queryKey: QK_CHARTER(iri ?? ""),
    queryFn: () => ipc.getOntologyTtlCharter(iri!),
    enabled: !!iri,
  });
}

/** 写入/更新本体设计宪章。 */
export function useSetOntologyTtlCharter() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      charter,
    }: {
      iri: string;
      charter: TtlCharter;
    }): Promise<null> => ipc.setOntologyTtlCharter(iri, charter),
    onSuccess: (_data, variables) => {
      qc.invalidateQueries({ queryKey: QK_CHARTER(variables.iri) });
    },
  });
}

/** 列出变更历史（revision 倒序）。 */
export function useOntologyTtlChangelog(iri: string | null) {
  return useQuery<TtlChangelog[]>({
    queryKey: QK_CHANGELOG(iri ?? ""),
    queryFn: () => ipc.listOntologyTtlChangelog(iri!),
    enabled: !!iri,
  });
}

/**
 * 监听 ontology-changed 事件，失效全部 TTL 本体查询。
 * 覆盖会话内 agent 工具导入（不经过 IPC 命令层）的场景。
 */
export function useOntologyTtlChangedListener() {
  const qc = useQueryClient();
  useEffect(() => {
    const unlisten = listen("ontology-changed", () => {
      // 事件不带 iri：列表 + 所有详情 + changelog + charter 全部失效
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-content"] });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-changelog"] });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-charter"] });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [qc]);
}

export type { TtlOntologySummary };
