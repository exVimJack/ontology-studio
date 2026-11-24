// Hooks 层：useOntologyTtl.ts（W3C Turtle 本体，对齐 skill ontology-modeling-w3c）
// 对齐 useOntology.ts 模式：queryKey 常量 + useQuery/useMutation。
// 覆盖 list / export / validate / import / delete / query_sparql / charter / changelog。

import { useEffect, useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { ipc } from "@/lib/ipc/commands";
import type { ElementEvidenceRow } from "./useOntology";

export type { ElementEvidenceRow };
import type {
  TtlChangelog,
  TtlCharter,
  TtlImportResult,
  TtlOntologySummary,
  TtlClassEdit,
  TtlClassMeta,
  TtlPropertyEdit,
  TtlValidation,
  TtlVersionDiff,
  TtlVersionSummary,
  WorkspaceDiffRevokeItem,
} from "@/lib/domain";
import {
  extractTtlCharterFromContent,
  hasCharterContent,
} from "@/lib/ttl-charter";

const QK_LIST = ["ontology-ttl"] as const;
const QK_CONTENT = (iri: string) => ["ontology-ttl-content", iri] as const;
const QK_CHANGELOG = (iri: string) => ["ontology-ttl-changelog", iri] as const;
const QK_CHARTER = (iri: string) => ["ontology-ttl-charter", iri] as const;
const QK_PROPERTIES = (iri: string) =>
  ["ontology-ttl-properties", iri] as const;
const QK_CLASS_METAS = (iri: string) =>
  ["ontology-ttl-class-metas", iri] as const;
const QK_CLASS_IRIS = (iri: string) =>
  ["ontology-ttl-class-iris", iri] as const;
const QK_ACTIVE_REV = (iri: string) => ["ontology-ttl-active-rev", iri] as const;
const QK_WORKSPACE_DIRTY = (iri: string) =>
  ["ontology-ttl-workspace-dirty", iri] as const;

/** 已存 W3C 本体列表（左栏）。 */
export function useOntologyTtls() {
  return useQuery({
    queryKey: QK_LIST,
    queryFn: () => ipc.listOntologyTtl(),
  });
}

/** 生效快照 Turtle（发布版本，导出用；不含未发布修改）。 */
export function useExportOntologyTtl() {
  return useMutation({
    mutationFn: (iri: string): Promise<string> => ipc.exportOntologyTtl(iri),
  });
}

/** 工作区草稿 Turtle（中栏源码 / 概念图谱等编辑态）。 */
export function useOntologyTtlContent(iri: string | null) {
  return useQuery<string>({
    queryKey: QK_CONTENT(iri ?? ""),
    queryFn: () => ipc.exportOntologyTtlWorkspace(iri!),
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
      // import 成功后列表必变；缓存失效让详情刷新；写路径置脏
      qc.invalidateQueries({ queryKey: QK_LIST });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-content"] });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-workspace-dirty"] });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-active-rev"] });
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
/** 列出 W3C TTL 本体的元素级证据（docs/EVIDENCE-CHAIN.md L4，三态展示）。
 *  element_type: class | datatype_property | object_property；element_id 为完整 IRI。 */
export function useTtlElementEvidence(ontologyIri: string | null) {
  return useQuery<ElementEvidenceRow[]>({
    queryKey: ["ontology-ttl-element-evidence", ontologyIri ?? ""],
    queryFn: async () => {
      const json = await ipc.listElementEvidence("w3c", ontologyIri!);
      return JSON.parse(json) as ElementEvidenceRow[];
    },
    enabled: !!ontologyIri,
  });
}

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

/** 读取本体设计宪章（不变点）。
 *  表内无记录时回退解析 TTL 头部内嵌的 W3C 词汇宪章
 *  （dcterms:description/skos:definition/rdfs:comment/skos:scopeNote），
 *  保证 .ttl 内已有宪章的本体直接可见；用户保存后以表为准。
 *  回退查询复用 QK_CONTENT 缓存（详情页已加载，零额外 IPC）。 */
export function useOntologyTtlCharter(iri: string | null) {
  const storedQ = useQuery<TtlCharter>({
    queryKey: QK_CHARTER(iri ?? ""),
    queryFn: () => ipc.getOntologyTtlCharter(iri!),
    enabled: !!iri,
  });
  const stored = storedQ.data;
  const storedEmpty = !!iri && stored != null && !hasCharterContent(stored);
  const contentQ = useQuery<string>({
    queryKey: QK_CONTENT(iri ?? ""),
    queryFn: () => ipc.exportOntologyTtl(iri!),
    enabled: storedEmpty,
  });
  const parsed = useMemo(
    () => (contentQ.data ? extractTtlCharterFromContent(contentQ.data) : null),
    [contentQ.data],
  );
  const data = useMemo<TtlCharter | undefined>(() => {
    if (parsed) {
      return {
        business_scenario: "",
        business_essence: "",
        updated_by: "agent",
        updated_at: 0,
        ...parsed,
      };
    }
    return stored;
  }, [parsed, stored]);
  return {
    data,
    isLoading: storedQ.isLoading || (storedEmpty && contentQ.isLoading),
    isError: storedQ.isError || (storedEmpty && contentQ.isError),
  };
}

/** 写入/更新本体设计宪章。 */
/** 宪章先行（决策 24）：创建空骨架 W3C 本体 + 设计宪章（仅宪章向导调用）。 */
export function useCreateOntologyTtlWithCharter() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      displayName,
      charter,
    }: {
      iri: string;
      displayName: string;
      charter: TtlCharter;
    }): Promise<null> =>
      ipc.createOntologyTtlWithCharter(iri, displayName, charter),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: QK_LIST });
    },
  });
}

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
      qc.invalidateQueries({ queryKey: ["ontology-ttl-workspace-dirty"] });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-sparql"] });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-version-diff"] });
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [qc]);
}

export type { TtlOntologySummary, TtlPropertyEdit };

// ════════════════════════════════════════════════════════════════
// W3C Turtle 属性结构化编辑（手动编辑 P0：属性/关系，不动类）
// 手术命令返回手术后的完整 TTL；成功后失效 content/properties/sparql 展示缓存。
// ════════════════════════════════════════════════════════════════

/** 本体全部属性（散块已聚合），属性编辑 Tab 列表数据源。 */
export function useOntologyTtlProperties(iri: string | null) {
  return useQuery<TtlPropertyEdit[]>({
    queryKey: QK_PROPERTIES(iri ?? ""),
    queryFn: () => ipc.listOntologyTtlProperties(iri!),
    enabled: !!iri,
  });
}

/** 本体全部类 IRI（编辑表单 domain/range 下拉用）。 */
export function useOntologyTtlClassIris(iri: string | null) {
  return useQuery<string[]>({
    queryKey: QK_CLASS_IRIS(iri ?? ""),
    queryFn: () => ipc.listOntologyTtlClassIris(iri!),
    enabled: !!iri,
    staleTime: Infinity,
  });
}

/** 新增属性表单的默认 IRI 前缀（与既有属性 namespace 一致）。 */
export function useOntologyTtlDefaultPropertyNs(iri: string | null) {
  return useQuery<string>({
    queryKey: ["ontology-ttl-default-prop-ns", iri],
    queryFn: () => ipc.getOntologyTtlDefaultPropertyNs(iri!),
    enabled: !!iri,
    staleTime: Infinity,
  });
}

/** 类/属性手术成功后的缓存失效（编辑/新增/删除共用）。 */
function invalidateAfterSurgery(
  qc: ReturnType<typeof useQueryClient>,
  iri: string,
) {
  qc.invalidateQueries({ queryKey: QK_CONTENT(iri) });
  qc.invalidateQueries({ queryKey: QK_WORKSPACE_DIRTY(iri) });
  qc.invalidateQueries({ queryKey: QK_ACTIVE_REV(iri) });
  qc.invalidateQueries({ queryKey: QK_PROPERTIES(iri) });
  qc.invalidateQueries({ queryKey: QK_CLASS_IRIS(iri) });
  qc.invalidateQueries({ queryKey: QK_CLASS_METAS(iri) });
  // 展示缓存（图/树/详情/label 映射/SPARQL 只读）staleTime=Infinity，需强制失效
  qc.invalidateQueries({ queryKey: ["ontology-ttl-sparql", iri] });
  qc.invalidateQueries({ queryKey: ["ontology-ttl-labels", iri] });
  // 相对生效 diff：图上着色依赖此查询；漏失效会导致改完类/属性后 overlay 仍是旧状态
  // （prefix 匹配该本体下所有 rev 对，与 publish/activate 一致）
  qc.invalidateQueries({ queryKey: ["ontology-ttl-version-diff", iri] });
}

/** 编辑既有属性（iri 为身份不可改，其余字段原位替换其主语块）。 */
export function useEditOntologyTtlProperty() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      edit,
    }: {
      iri: string;
      edit: TtlPropertyEdit;
    }): Promise<string> => ipc.editOntologyTtlProperty(iri, edit),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

/** 新增属性（追加在最后一个属性块之后）。 */
export function useAddOntologyTtlProperty() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      edit,
    }: {
      iri: string;
      edit: TtlPropertyEdit;
    }): Promise<string> => ipc.addOntologyTtlProperty(iri, edit),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

/** 删除属性（被 owl:onProperty / subPropertyOf 引用时后端拒绝）。 */
export function useDeleteOntologyTtlProperty() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      propertyIri,
    }: {
      iri: string;
      propertyIri: string;
    }): Promise<string> => ipc.deleteOntologyTtlProperty(iri, propertyIri),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

/** 变更清单逐条撤销（相对生效快照逆向补丁）。 */
export function useRevokeOntologyWorkspaceDiffItem() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      item,
    }: {
      iri: string;
      item: WorkspaceDiffRevokeItem;
    }): Promise<string> => ipc.revokeOntologyWorkspaceDiffItem(iri, item),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

/** 类行级联撤销（服务端重算强依赖，原子写回）。 */
export function useRevokeOntologyWorkspaceClassCascade() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      classIri,
      op,
    }: {
      iri: string;
      classIri: string;
      op: "add" | "remove";
    }): Promise<string> =>
      ipc.revokeOntologyWorkspaceClassCascade(iri, classIri, op),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

/** 类元数据列表（label/comment，概念图谱编辑表单回显用）。 */
export function useOntologyTtlClassMetas(iri: string | null) {
  return useQuery<TtlClassMeta[]>({
    queryKey: QK_CLASS_METAS(iri ?? ""),
    queryFn: () => ipc.listOntologyTtlClassMetas(iri!),
    enabled: !!iri,
  });
}

/** 编辑类声明块（label/comment/subClassOf 受管重写）。 */
export function useEditOntologyTtlClass() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      edit,
    }: {
      iri: string;
      edit: TtlClassEdit;
    }): Promise<string> => ipc.editOntologyTtlClass(iri, edit),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

/** 删除类（卡片硬删：独占属性删、多定义域摘除、触及关系整删、继承清理后删类）。 */
export function useDeleteOntologyTtlClass() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      classIri,
    }: {
      iri: string;
      classIri: string;
    }): Promise<string> => ipc.deleteOntologyTtlClass(iri, classIri),
    onSuccess: (_ttl, v) => invalidateAfterSurgery(qc, v.iri),
  });
}

// ── 版本管理（显式发布才留快照；回退切指针不新建版本）──

const QK_VERSIONS = (iri: string) => ["ontology-ttl-versions", iri] as const;
const QK_VERSION_DIFF = (iri: string, a: number, b: number) =>
  ["ontology-ttl-version-diff", iri, a, b] as const;

/** 版本历史（revision 降序；active 对应条目前端标"已启用"）。 */
export function useOntologyVersions(iri: string | null) {
  return useQuery<TtlVersionSummary[]>({
    queryKey: QK_VERSIONS(iri ?? ""),
    queryFn: () => ipc.listOntologyVersions(iri!),
    enabled: !!iri,
  });
}

/** 当前生效发布版本（null = 从未发布）。 */
export function useOntologyActiveRevision(iri: string | null) {
  return useQuery<number | null>({
    queryKey: QK_ACTIVE_REV(iri ?? ""),
    queryFn: () => ipc.getOntologyActiveRevision(iri!),
    enabled: !!iri,
  });
}

/** 工作区相对生效快照是否有未发布修改。 */
export function useOntologyWorkspaceDirty(iri: string | null) {
  return useQuery<boolean>({
    queryKey: QK_WORKSPACE_DIRTY(iri ?? ""),
    queryFn: () => ipc.getOntologyWorkspaceDirty(iri!),
    enabled: !!iri,
  });
}

/** 两版本结构化 diff（rev=0 = 当前工作区内容）。 */
export function useOntologyVersionDiff(
  iri: string | null,
  revA: number | null,
  revB: number | null,
) {
  return useQuery<TtlVersionDiff>({
    queryKey: QK_VERSION_DIFF(iri ?? "", revA ?? 0, revB ?? 0),
    queryFn: () => ipc.diffOntologyVersions(iri!, revA!, revB!),
    enabled: !!iri && revA != null && revB != null,
    staleTime: 30_000,
  });
}

/** 发布当前内容为新版本（版本=里程碑）。 */
export function usePublishOntologyVersion() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      label,
      description,
    }: {
      iri: string;
      label: string;
      description: string;
    }): Promise<number> => ipc.publishOntologyVersion(iri, label, description, "user"),
    onSuccess: (_rev, v) => {
      qc.invalidateQueries({ queryKey: QK_VERSIONS(v.iri) });
      qc.invalidateQueries({ queryKey: QK_ACTIVE_REV(v.iri) });
      qc.invalidateQueries({ queryKey: QK_WORKSPACE_DIRTY(v.iri) });
      qc.invalidateQueries({ queryKey: QK_CHANGELOG(v.iri) });
      qc.invalidateQueries({ queryKey: QK_CONTENT(v.iri) });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-version-diff", v.iri] });
      qc.invalidateQueries({ queryKey: QK_LIST });
    },
  });
}

/** 回退到指定版本（切指针覆盖当前内容，不新建版本）。 */
export function useRevertOntologyVersion() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      revision,
    }: {
      iri: string;
      revision: number;
    }): Promise<null> => ipc.revertOntologyVersion(iri, revision, "user"),
    onSuccess: (_r, v) => {
      qc.invalidateQueries({ queryKey: QK_VERSIONS(v.iri) });
      qc.invalidateQueries({ queryKey: QK_ACTIVE_REV(v.iri) });
      qc.invalidateQueries({ queryKey: QK_WORKSPACE_DIRTY(v.iri) });
      qc.invalidateQueries({ queryKey: QK_CHANGELOG(v.iri) });
      qc.invalidateQueries({ queryKey: QK_CONTENT(v.iri) });
      qc.invalidateQueries({ queryKey: QK_PROPERTIES(v.iri) });
      qc.invalidateQueries({ queryKey: QK_CLASS_IRIS(v.iri) });
      qc.invalidateQueries({ queryKey: ["ontology-ttl-version-diff", v.iri] });
      qc.invalidateQueries({ queryKey: QK_LIST });
    },
  });
}

/** 删除单个版本快照（当前启用版本后端拒绝）。 */
export function useDeleteOntologyVersion() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      iri,
      revision,
    }: {
      iri: string;
      revision: number;
    }): Promise<null> => ipc.deleteOntologyVersion(iri, revision),
    onSuccess: (_r, v) => {
      qc.invalidateQueries({ queryKey: QK_VERSIONS(v.iri) });
      qc.invalidateQueries({ queryKey: QK_ACTIVE_REV(v.iri) });
      qc.invalidateQueries({ queryKey: QK_CHANGELOG(v.iri) });
    },
  });
}

/** 版本上限（默认 20）。 */
export function useVersionLimit() {
  return useQuery<number>({
    queryKey: ["ontology-ttl-version-limit"],
    queryFn: () => ipc.getVersionLimit(),
  });
}

/** 设置版本上限（0 回退默认）。 */
export function useSetVersionLimit() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (limit: number): Promise<number> => ipc.setVersionLimit(limit),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["ontology-ttl-version-limit"] });
    },
  });
}
