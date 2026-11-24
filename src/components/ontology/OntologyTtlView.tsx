// UI 层：OntologyTtlView.tsx（W3C Turtle 本体工作台）
// 对齐书 6.4.2「本体模型的图形化与可视化展示能力」+ 表 6-9 的 7+1 语义维度。
//
// 三栏布局（同 OntologyView，主键改为 ontology_iri）：
//   左栏：本体列表（IRI / 版本 / 三元组数）
//   中栏：多视图 Tab —— 概览 / 核心概念图谱(力导向) / 类层级树 /
//         规则逻辑树 / Turtle 源码 / SPARQL 控制台
//   右栏：导入预演（validate 结果）+ 导出
//
// 视图映射表 6-9：
//   - 核心概念图谱 → G6 v5 图（@antv/g6），节点=类/数据属性，边=ObjectProperty
//   - 类层级树 → 缩进树，rdfs:subClassOf
//   - 规则逻辑树 → SWRL swrl:Imp 的"如果-那么"
//   - SPARQL 控制台 → 第 7 类 CRUD
//   - 宪章(+1) / 历史 → 复用 Palantir 的 CharterPanel/HistoryTab 设计

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { confirm, message, open } from "@tauri-apps/plugin-dialog";
import {
  Tags,
  Network,
  Plus,
  X,
  Loader2,
  Table2,
  GitBranch,
  GitCommitHorizontal,
  Terminal,
  FileCode2,
  Download,
  Link2,
  CheckCircle2,
  XCircle,
  AlertTriangle,
  ArrowRight,
  FileUp,
  Trash2,
  Pencil,
  Save,
  X as XIcon,
  Boxes,
  Workflow,
  Search,
  ChevronDown,
  ChevronRight,
  Sparkles,
  Check,
  Database,
  Copy,
} from "lucide-react";
import { ConceptGraph } from "@/components/ontology/ConceptGraph";
import {
  ElementMappingEditor,
  MappingCoverageCard,
  MappingExportButton,
  MappingGapPanel,
  MappingStateBadge,
  SourceFieldCell,
  useMappingRowsByElement,
} from "@/components/ontology/ElementMappingPanel";
import { endpointLocator, mappingStateOf } from "@/lib/mapping-display";
import {
  groupByDomain,
  splitTtlProperties,
} from "@/lib/ttl-property-filter";
import { useMappingCoverage } from "@/hooks/useElementMapping";
import { CreateOntologyWizard } from "@/components/ontology/CreateOntologyWizard";
import { OntologyVersionsPanel, DiffView } from "@/components/ontology/OntologyVersionsPanel";
import { PublishOntologyVersionDialog } from "@/components/ontology/PublishOntologyVersionDialog";
import { rowsToGraphData } from "@/lib/ontology-graph";
import {
  applyEffectiveDiffToGraph,
  countEffectiveDiff,
  diffHasEntries,
  filterDiffRelatedTo,
  type GraphDiffFocus,
} from "@/lib/ontology-effective-diff";
import {
  collectClassStrongDeps,
  describeClassCascadeOutcome,
  isLeafRestoreBlockedByDeletedClass,
  summarizeClassStrongDeps,
  toWorkspaceRevokeItem,
} from "@/lib/ontology-diff-display";
import { useIsMobile } from "@/hooks/useIsMobile";
import { saveTextFile } from "@/lib/save-file";
import {
  useOntologyTtls,
  useOntologyTtlContent,
  useOntologyActiveRevision,
  useOntologyWorkspaceDirty,
  useOntologyVersionDiff,
  useValidateOntologyTtl,
  useExportOntologyTtl,
  useImportOntologyTtl,
  useDeleteOntologyTtl,
  useQueryOntologySparql,
  useSparqlQueryRead,
  useTtlLabelMap,
  useOntologyTtlCharter,
  useSetOntologyTtlCharter,
  useOntologyTtlChangelog,
  useOntologyTtlChangedListener,
  useOntologyTtlProperties,
  useOntologyTtlClassIris,
  useOntologyTtlDefaultPropertyNs,
  useEditOntologyTtlProperty,
  useAddOntologyTtlProperty,
  useDeleteOntologyTtlProperty,
  useRevokeOntologyWorkspaceDiffItem,
  useRevokeOntologyWorkspaceClassCascade,
  useTtlElementEvidence,
} from "@/hooks/useOntologyTtl";
import {
  EVIDENCE_STATUS_META,
  evidenceStatusTtl,
  type EvidenceStatus,
  type ElementEvidenceRow,
} from "@/hooks/useOntology";
import { useProvider } from "@/hooks/useProvider";
import {
  suggestCharter,
  CHARTER_SUGGEST_MODES,
  type CharterSuggestMode,
  type CharterSuggestion,
} from "@/lib/charter-suggest";
import type {
  ElementMapping,
  TtlOntologySummary,
  TtlValidation,
  TtlCharter,
  TtlChangelog,
  TtlPropertyEdit,
} from "@/lib/domain";

// ── SPARQL 查询模板（预置，用户可改）──
const SPARQL_TEMPLATES: { label: string; sparql: string }[] = [
  {
    label: "所有类",
    sparql: "SELECT ?c WHERE { ?c a owl:Class . } ORDER BY ?c",
  },
  {
    label: "类层级",
    sparql:
      "SELECT ?sub ?sup WHERE { ?sub rdfs:subClassOf ?sup . } ORDER BY ?sup ?sub",
  },
  {
    label: "对象属性",
    sparql:
      "SELECT ?p ?domain ?range WHERE { ?p a owl:ObjectProperty . OPTIONAL { ?p rdfs:domain ?domain } OPTIONAL { ?p rdfs:range ?range } }",
  },
  {
    label: "数据属性",
    sparql:
      "SELECT ?p ?domain ?range WHERE { ?p a owl:DatatypeProperty . OPTIONAL { ?p rdfs:domain ?domain } OPTIONAL { ?p rdfs:range ?range } }",
  },
  {
    label: "SWRL 规则",
    sparql: "SELECT ?rule WHERE { ?rule a swrl:Imp . }",
  },
];

export interface OntologyTtlViewHandle {
  refetch: () => void;
  openImport: () => void;
  openCreate: () => void;
}

export const OntologyTtlView = forwardRef<OntologyTtlViewHandle>(
  function OntologyTtlView(_props, ref) {
    const isMobile = useIsMobile();
    const { data: ontologies = [], isLoading, refetch } = useOntologyTtls();
    useOntologyTtlChangedListener();
    const deleteMut = useDeleteOntologyTtl();
    const [selectedIri, setSelectedIri] = useState<string | null>(null);
    const [showImport, setShowImport] = useState(false);
    const [showCreate, setShowCreate] = useState(false);

    // 切换到 W3C Tab 时自动选中第一个本体
    useEffect(() => {
      if (ontologies.length > 0 && selectedIri === null) {
        setSelectedIri(ontologies[0].ontology_iri);
      }
    }, [ontologies, selectedIri]);

    async function handleDelete(iri: string) {
      const ok = await confirm(
        `确定删除本体「${shortIri(iri)}」？\n\n该操作不可撤销：Turtle 文本、设计宪章、变更历史将被全部清除。`,
        { kind: "warning" },
      );
      if (!ok) return;
      deleteMut.mutate(iri, {
        onError: async () => message("删除失败，请重试。", { kind: "error" }),
        onSuccess: async (deleted) => {
          if (!deleted)
            await message("未找到该本体，可能已被删除。", {
              kind: "info",
            });
          if (selectedIri === iri) setSelectedIri(null);
        },
      });
    }

    const selected =
      ontologies.find((o) => o.ontology_iri === selectedIri) ?? null;
    const mobileShowDetail = isMobile && selected !== null;

    useImperativeHandle(ref, () => ({
      refetch,
      openImport: () => setShowImport(true),
      openCreate: () => setShowCreate(true),
    }));

    return (
      <>
        <CreateOntologyWizard
          variant="ttl"
          open={showCreate}
          onClose={() => setShowCreate(false)}
          onCreated={(iri) => {
            refetch();
            setSelectedIri(iri);
          }}
        />
        <div className="flex h-full min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1">
            {/* 左栏：本体列表 */}
            <div
              className={`flex min-h-0 flex-col border-r border-border ${
                isMobile ? (mobileShowDetail ? "hidden" : "w-full") : "w-72"
              }`}
            >
              {isMobile && mobileShowDetail ? null : (
                <>
                  <div className="px-3 py-2">
                    <span className="text-xs font-medium uppercase tracking-wide text-fg-subtle">
                      本体（W3C）
                    </span>
                  </div>
                  <div className="flex-1 overflow-y-auto px-1">
                    {isLoading ? (
                      <div className="flex justify-center py-8">
                        <Loader2
                          className="animate-spin text-fg-subtle"
                          size={16}
                        />
                      </div>
                    ) : ontologies.length === 0 ? (
                      <div className="px-3 py-8 text-center text-xs text-fg-subtle">
                        暂无 W3C 本体
                        <br />
                        <button
                          onClick={() => setShowCreate(true)}
                          className="mt-2 text-accent hover:underline"
                        >
                          新建本体（先完成设计宪章）
                        </button>
                        <br />
                        <button
                          onClick={() => setShowImport(true)}
                          className="mt-1 text-accent hover:underline"
                        >
                          导入已有 .ttl
                        </button>
                      </div>
                    ) : (
                      ontologies.map((o) => (
                        <TtlListItem
                          key={o.ontology_iri}
                          summary={o}
                          active={o.ontology_iri === selectedIri}
                          deleting={
                            deleteMut.isPending &&
                            deleteMut.variables === o.ontology_iri
                          }
                          onClick={() => setSelectedIri(o.ontology_iri)}
                          onDelete={() => handleDelete(o.ontology_iri)}
                        />
                      ))
                    )}
                  </div>
                </>
              )}
            </div>

            {/* 中栏：详情 */}
            <div
              className={`flex min-h-0 min-w-0 flex-1 flex-col ${
                isMobile && !mobileShowDetail ? "hidden" : "flex"
              }`}
            >
              {selected ? (
                <TtlDetail
                  iri={selected.ontology_iri}
                  version={selected.version}
                  tripleCount={selected.triple_count}
                  updatedAt={selected.updated_at}
                  deleting={
                    deleteMut.isPending &&
                    deleteMut.variables === selected.ontology_iri
                  }
                  onDelete={() => handleDelete(selected.ontology_iri)}
                />
              ) : !isMobile ? (
                <div className="flex h-full items-center justify-center text-sm text-fg-subtle">
                  选择左侧本体查看定义
                </div>
              ) : null}
            </div>
          </div>

          {showImport && (
            <TtlImportDialog
              onClose={() => setShowImport(false)}
              onImported={(iri) => {
                setSelectedIri(iri);
                setShowImport(false);
              }}
            />
          )}
        </div>
      </>
    );
  },
);

// ── 本体列表项 ──
function TtlListItem({
  summary,
  active,
  deleting,
  onClick,
  onDelete,
}: {
  summary: TtlOntologySummary;
  active: boolean;
  deleting: boolean;
  onClick: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="group relative mx-1">
      <button
        onClick={onClick}
        className={`block w-full rounded-md px-2 py-2 text-left ${
          active ? "bg-bg-hover" : "hover:bg-bg-hover/50"
        }`}
      >
        <div className="flex items-center gap-1.5">
          <Network size={12} className="shrink-0 text-accent" />
          <span className="truncate text-xs font-medium">
            {shortIri(summary.ontology_iri)}
          </span>
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-[10px] text-fg-subtle">
          <span className="truncate">{summary.version || "无版本"}</span>
          <span>· {summary.triple_count} 三元组</span>
        </div>
      </button>
      <button
        onClick={(e) => {
          e.stopPropagation();
          onDelete();
        }}
        disabled={deleting}
        className="absolute right-1 top-1 rounded p-1 text-fg-subtle opacity-0 transition hover:bg-danger-bg hover:text-danger group-hover:opacity-100 disabled:opacity-50"
        title="删除本体"
      >
        {deleting ? (
          <Loader2 size={12} className="animate-spin" />
        ) : (
          <Trash2 size={12} />
        )}
      </button>
    </div>
  );
}

// ── 详情（中栏）──
type DetailTab =
  | "overview"
  | "graph"
  | "hierarchy"
  | "rules"
  | "properties"
  | "mapping"
  | "source"
  | "sparql"
  | "charter"
  | "version"
  | "history";

function TtlDetail({
  iri,
  version,
  tripleCount,
  updatedAt,
  deleting,
  onDelete,
}: {
  iri: string;
  version: string;
  tripleCount: number;
  updatedAt: number;
  deleting: boolean;
  onDelete: () => void;
}) {
  const { data: content } = useOntologyTtlContent(iri);
  const { data: activeRev } = useOntologyActiveRevision(iri);
  const { data: workspaceDirty } = useOntologyWorkspaceDirty(iri);
  const exportTtl = useExportOntologyTtl();
  const canExport = activeRev != null;
  const [tab, setTab] = useState<DetailTab>("overview");
  // 图谱→Turtle 联动：要聚焦的源码行号（1-based）
  const [sourceFocusLine, setSourceFocusLine] = useState<number | null>(null);
  // 已访问过的 Tab：首次渲染后保持挂载（CSS 隐藏），避免 G6 等重实例反复销毁/重建
  const [visited, setVisited] = useState<Set<DetailTab>>(
    () => new Set(["overview"]),
  );
  useEffect(() => {
    setVisited((prev) => {
      if (prev.has(tab)) return prev;
      const next = new Set(prev);
      next.add(tab);
      return next;
    });
  }, [tab]);

  async function handleExport() {
    if (!canExport) {
      await message("请先发布版本后再导出", { kind: "warning" });
      return;
    }
    try {
      // 消费面：导出生效快照（不含未发布修改）
const ttl = await exportTtl.mutateAsync(iri);
      const saved = await saveTextFile(ttl, `${shortIri(iri)}.ttl`, [
        { name: "Turtle", extensions: ["ttl"] },
      ]);
      if (saved) {
        // 导出成功，暂略 toast
      }
    } catch (e) {
      console.error("export ontology ttl", e);
      await message("请先发布版本后再导出", { kind: "error" });
    }
  }

  // 图谱→Turtle 联动：找到 IRI 在源码中的定义行，切到源码页并聚焦
  function handleJumpToSource(termIri: string) {
    let lineNo: number | null = null;
    if (content) {
      const lines = content.split("\n");
      // Turtle 定义行格式：<IRI> a owl:Class ; 或 <IRI> rdfs:label ...
      const target = `<${termIri}>`;
      for (let i = 0; i < lines.length; i++) {
        const t = lines[i].trim();
        if (t.startsWith(target)) {
          lineNo = i + 1;
          break;
        }
      }
    }
    setSourceFocusLine(lineNo);
    setTab("source");
    setVisited((prev) => {
      if (prev.has("source")) return prev;
      const next = new Set(prev);
      next.add("source");
      return next;
    });
  }

  const tabs: { key: DetailTab; label: string; icon: typeof Table2 }[] = [
    { key: "overview", label: "概览", icon: Table2 },
    { key: "graph", label: "概念图谱", icon: Network },
    { key: "hierarchy", label: "类层级", icon: GitBranch },
    { key: "rules", label: "规则", icon: Workflow },
    { key: "properties", label: "属性", icon: Tags },
    { key: "mapping", label: "来源字段", icon: Database },
    { key: "source", label: "Turtle", icon: FileCode2 },
    { key: "sparql", label: "SPARQL", icon: Terminal },
    { key: "version", label: "版本", icon: GitCommitHorizontal },
    { key: "history", label: "历史", icon: GitBranch },
  ];

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col overflow-hidden">
      {/* 头部 */}
      <div className="shrink-0 border-b border-border px-4 py-3">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <h2 className="text-base font-semibold">{shortIri(iri)}</h2>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <button
              onClick={handleExport}
              disabled={!canExport}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
              title={
                !canExport
                  ? "请先发布版本后再导出"
                  : workspaceDirty
                    ? "导出生效已发布版本（不含未发布修改）"
                    : "导出为 .ttl 文件"
              }
            >
              <Download size={12} />
              导出
            </button>
            <button
              onClick={onDelete}
              disabled={deleting}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-danger-bg hover:text-danger disabled:opacity-50"
              title="删除本体"
            >
              {deleting ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Trash2 size={12} />
              )}
              删除
            </button>
          </div>
        </div>
        <div className="mt-1.5 flex flex-wrap items-center gap-3 text-[10px] text-fg-subtle">
          {version && (
            <span className="rounded bg-bg-hover px-1.5 py-0.5">
              v{version}
            </span>
          )}
          <span className="rounded bg-bg-hover px-1.5 py-0.5">
            {tripleCount} 三元组
          </span>
          {updatedAt > 0 && (
            <span>更新于 {new Date(updatedAt).toLocaleString()}</span>
          )}
        </div>
        {/* 本体设计宪章（不变点）：业务场景 / 业务目标（设计意图/补充说明不对业务呈现）。
                    已移入「概览」Tab 展示，让头部区更高可容纳更多 Tab 内容 */}
      </div>

      {/* Tab 栏 */}
      <div className="flex shrink-0 gap-1 overflow-x-auto border-b border-border px-3 py-1.5">
        {tabs.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            className={`flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-xs transition ${
              tab === t.key
                ? "bg-bg-hover font-medium text-accent"
                : "text-fg-subtle hover:bg-bg-hover/50"
            }`}
          >
            <t.icon size={12} />
            {t.label}
          </button>
        ))}
      </div>

      {/* Tab 内容：已访问的 Tab 保持挂载（CSS 隐藏），切回无需重建。
          外层 flex-col，活动页用 flex-1（勿只靠 h-full，父高未定时会塌缩） */}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {visited.has("overview") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "overview" ? "" : "hidden"}`}
          >
            <OverviewPanel
              iri={iri}
              tripleCount={tripleCount}
              content={content}
              onJump={setTab}
            />
          </div>
        )}
        {visited.has("graph") && (
          <div
            className={`flex min-h-0 flex-1 flex-col ${tab === "graph" ? "" : "hidden"}`}
          >
            <GraphPanel iri={iri} onJumpToSource={handleJumpToSource} />
          </div>
        )}
        {visited.has("hierarchy") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "hierarchy" ? "" : "hidden"}`}
          >
            <HierarchyPanel iri={iri} />
          </div>
        )}
        {visited.has("rules") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "rules" ? "" : "hidden"}`}
          >
            <RulesPanel iri={iri} />
          </div>
        )}
        {visited.has("properties") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "properties" ? "" : "hidden"}`}
          >
            <PropertiesPanel iri={iri} />
          </div>
        )}
        {visited.has("mapping") && (
          <div
            className={`min-h-0 flex-1 overflow-auto ${tab === "mapping" ? "" : "hidden"}`}
          >
            <MappingTab iri={iri} />
          </div>
        )}
        {visited.has("source") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "source" ? "" : "hidden"}`}
          >
            <SourcePanel content={content} focusLine={sourceFocusLine} />
          </div>
        )}
        {visited.has("sparql") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "sparql" ? "" : "hidden"}`}
          >
            <SparqlPanel iri={iri} />
          </div>
        )}
        {visited.has("version") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "version" ? "" : "hidden"}`}
          >
            <OntologyVersionsPanel ontologyIri={iri} />
          </div>
        )}
        {visited.has("history") && (
          <div
            className={`min-h-0 flex-1 overflow-hidden ${tab === "history" ? "" : "hidden"}`}
          >
            <HistoryPanel iri={iri} />
          </div>
        )}
      </div>
    </div>
  );
}

// ── 概览面板 ──
// ── 来源字段页（决策 25：映射绑定层）──
// 概念定义与物理来源分离：本页只处理「本体元素 ↔ 数据源表字段」的绑定，
// 不修改任何 TTL 概念三元组（映射保存在独立的绑定层里）。
function MappingTab({ iri }: { iri: string }) {
  const { data: coverage } = useMappingCoverage("w3c", iri);
  return (
    <div className="space-y-3 p-4">
      <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2">
        <div>
          <div className="text-xs font-medium text-fg-muted">来源字段绑定</div>
          <p className="mt-0.5 text-[10px] text-fg-subtle">
            指定类与属性来自数据源的哪些表和字段；不改变概念本身
          </p>
        </div>
        <MappingExportButton
          standard="w3c"
          ontologyKey={iri}
          coverage={coverage}
        />
      </div>
      <MappingGapPanel standard="w3c" ontologyKey={iri} />
    </div>
  );
}

function OverviewPanel({
  iri,
  tripleCount,
  content,
  onJump,
}: {
  iri: string;
  tripleCount: number;
  content?: string;
  onJump: (tab: DetailTab) => void;
}) {
  const coverage = useMappingCoverage("w3c", iri);
  const stats = [
    {
      key: "graph" as DetailTab,
      label: "三元组",
      count: tripleCount,
      icon: Table2,
    },
    {
      key: "source" as DetailTab,
      label: "Turtle 行数",
      count: content?.split("\n").length ?? 0,
      icon: FileCode2,
    },
  ];
  return (
    <div className="h-full overflow-y-auto p-4">
      {iri && (
        <div className="mb-4 rounded-lg border border-border bg-bg-hover/30 px-3 py-2.5">
          <div className="flex items-center gap-1.5 text-fg-subtle">
            <Link2 size={14} />
            <span className="text-[10px]">IRI</span>
          </div>
          <p className="mt-1 break-all text-xs leading-relaxed text-fg-muted">
            {iri}
          </p>
        </div>
      )}
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
        {stats.map((s) => (
          <button
            key={s.key}
            onClick={() => onJump(s.key)}
            className="flex flex-col items-start gap-1 rounded-lg border border-border px-3 py-2.5 text-left transition hover:border-accent/40 hover:bg-bg-hover/50"
          >
            <div className="flex items-center gap-1.5 text-fg-subtle">
              <s.icon size={14} />
              <span className="text-[10px]">{s.label}</span>
            </div>
            <span className="text-xl font-semibold tabular-nums">
              {s.count}
            </span>
          </button>
        ))}
      </div>
      {/* 来源字段覆盖率（点击下钻到来源字段页） */}
      <div className="mt-3">
        <MappingCoverageCard
          coverage={coverage.data}
          onDrilldown={() => onJump("mapping")}
        />
      </div>
      {/* 设计宪章：移入概览页，冷启动基线 */}
      <div className="mt-4">
        <CharterPanel iri={iri} />
      </div>
    </div>
  );
}

// ── 核心概念图谱（G6 v5 力导向/层级/径向多布局）──
// 节点：owl:Class（圆）+ owl:DatatypeProperty（琥珀色方，domain 类挂属性）。
// 边：rdfs:subClassOf（实线箭头）+ ObjectProperty（虚线带标签）+ DatatypeProperty（琥珀虚线）。
// 脏且有生效版时默认叠加「相对生效」着色与幽灵删除。
function GraphPanel({
  iri,
  onJumpToSource,
}: {
  iri: string;
  onJumpToSource: (termIri: string) => void;
}) {
  const sparql = useMemo(
    () =>
      "SELECT ?s ?p ?o ?type WHERE { " +
      '{ ?s a owl:Class . BIND("class" AS ?type) } UNION ' +
      '{ ?s rdfs:subClassOf ?o . BIND("subClassOf" AS ?p) BIND("hierarchy" AS ?type) } UNION ' +
      '{ ?p a owl:ObjectProperty . ?p rdfs:domain ?s . ?p rdfs:range ?o . BIND("objectProperty" AS ?type) } UNION ' +
      '{ ?p a owl:DatatypeProperty . ?p rdfs:domain ?s . OPTIONAL { ?p rdfs:range ?o } . BIND("datatypeProperty" AS ?type) } ' +
      "FILTER(isIRI(?s)) FILTER(!BOUND(?o) || isIRI(?o)) }",
    [],
  );
  const { data, isPending, error } = useSparqlQuery(iri, sparql);
  const { data: labelMap } = useTtlLabelMap(iri);
  const { data: activeRev } = useOntologyActiveRevision(iri);
  const { data: dirty } = useOntologyWorkspaceDirty(iri);
  const showDiff = !!dirty && activeRev != null;
  const diffQ = useOntologyVersionDiff(
    showDiff ? iri : null,
    showDiff ? activeRev : null,
    showDiff ? 0 : null,
  );
  const [listOpen, setListOpen] = useState(false);
  const [listFilter, setListFilter] = useState<"all" | "added" | "modified" | "removed">("all");
  const [listFocus, setListFocus] = useState<GraphDiffFocus | null>(null);
  const [publishOpen, setPublishOpen] = useState(false);
  const anchorClassLabel =
    listFocus?.kind === "class"
      ? labelMap?.get(listFocus.iri) ||
        listFocus.iri.split(/[#/]/).pop() ||
        listFocus.iri
      : null;

  const base = useMemo(
    () => rowsToGraphData(data ?? [], labelMap),
    [data, labelMap],
  );

  const { nodes, edges } = useMemo(() => {
    if (!showDiff || !diffQ.data) return base;
    // 删除项以幽灵保留在图上；清单侧也会列出，不再提供「隐藏幽灵」开关
    return applyEffectiveDiffToGraph(base.nodes, base.edges, diffQ.data);
  }, [base, showDiff, diffQ.data]);

  const counts = countEffectiveDiff(showDiff ? diffQ.data : null);

  // 有选中且与所选无关时整段隐藏（含「变更清单」标题），避免「rev N vs 0：无差异」
  const focusedDiff =
    showDiff && diffQ.data
      ? filterDiffRelatedTo(diffQ.data, listFocus)
      : null;
  const showChangeList =
    listOpen &&
    showDiff &&
    !!diffQ.data &&
    (!listFocus || diffHasEntries(focusedDiff));

  if (isPending) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-3">
        <div className="graph-spinner" />
        <p className="text-[11px] text-fg-subtle/70">正在加载图谱…</p>
      </div>
    );
  }
  if (error) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 p-4 text-center">
        <AlertTriangle size={20} className="text-warning" />
        <p className="text-sm text-danger">图谱加载失败</p>
        <p className="text-[11px] text-fg-subtle">{error.message}</p>
        <p className="mt-1 text-[10px] text-fg-subtle">
          提示：可在「SPARQL」页手动执行 owl:Class 查询排查。
        </p>
      </div>
    );
  }
  if (nodes.length === 0 && !showDiff) {
    return (
      <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 text-center">
        <Network size={20} className="text-fg-subtle" />
        <p className="text-sm text-fg-subtle">
          本体无 owl:Class 定义，无可绘制的关系图。
        </p>
        <p className="text-[10px] text-fg-subtle">
          概念图谱需要本体声明 owl:Class。
        </p>
      </div>
    );
  }

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      {showDiff && (
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-1.5 text-[11px] text-fg-subtle">
          <span className="font-medium text-fg">相对生效</span>
          <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-emerald-700 dark:text-emerald-300">
            +{counts.added}
          </span>
          <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-amber-800 dark:text-amber-200">
            ~{counts.modified}
          </span>
          <span className="rounded bg-red-500/15 px-1.5 py-0.5 text-red-700 dark:text-red-300">
            −{counts.removed}
          </span>
          {(counts.addedProps > 0 || counts.removedProps > 0) && (
            <span
              className="text-[10px] text-fg-subtle"
              title="属性增删计入摘要 +/−；定义域类会标变更色。清单内仍用红绿黄区分类型。"
            >
              含属性 {counts.addedProps + counts.removedProps}{" "}
              项（计入 +/−；定义域标变更色）
            </span>
          )}
          <span className="text-[10px] text-fg-subtle">粉虚线=有变更</span>
          <button
            type="button"
            className="rounded px-2 py-0.5 hover:bg-bg-hover hover:text-accent"
            onClick={() => setListOpen((v) => !v)}
          >
            {listOpen ? "收起清单" : "变更清单"}
          </button>
          <button
            type="button"
            className="ml-auto rounded bg-accent px-2.5 py-0.5 text-white hover:opacity-90"
            onClick={() => setPublishOpen(true)}
          >
            去发布
          </button>
        </div>
      )}
      <div className="relative flex min-h-0 flex-1 flex-col">
        <ConceptGraph
          nodes={nodes}
          edges={edges}
          storageKey={iri}
          onJumpToSource={onJumpToSource}
          ontologyIri={iri}
          className="min-h-0 flex-1"
          onSelectionChange={setListFocus}
          sidebarBottom={
            showChangeList && diffQ.data ? (
              <div className="flex min-h-0 flex-1 flex-col">
                <div className="flex shrink-0 items-center gap-1 border-b border-border px-2 py-1.5 text-[11px]">
                  <span className="font-medium">变更清单</span>
                  {listFocus?.kind === "class" ? (
                    <span
                      className="ml-auto truncate text-[10px] text-accent"
                      title={listFocus.iri}
                    >
                      当前类：{anchorClassLabel}
                    </span>
                  ) : listFocus ? (
                    <span className="ml-auto text-[10px] text-accent">
                      与所选相关
                    </span>
                  ) : (
                    <select
                      className="ml-auto rounded border border-border bg-bg px-1 py-0.5 text-[10px]"
                      value={listFilter}
                      onChange={(e) =>
                        setListFilter(e.target.value as typeof listFilter)
                      }
                    >
                      <option value="all">全部</option>
                      <option value="added">仅新增</option>
                      <option value="modified">仅修改</option>
                      <option value="removed">仅删除</option>
                    </select>
                  )}
                  <button
                    type="button"
                    className="rounded p-0.5 hover:bg-bg-hover"
                    onClick={() => setListOpen(false)}
                    title="关闭"
                  >
                    <X size={12} />
                  </button>
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto p-2 text-[11px]">
                  <GraphDiffList
                    diff={diffQ.data}
                    filter={listFocus ? "all" : listFilter}
                    focus={listFocus}
                    labelMap={labelMap}
                    ontologyIri={iri}
                  />
                </div>
              </div>
            ) : null
          }
        />
      </div>
      <PublishOntologyVersionDialog
        ontologyIri={iri}
        open={publishOpen}
        onClose={() => setPublishOpen(false)}
        onPublished={() => setListOpen(false)}
      />
    </div>
  );
}

function GraphDiffList({
  diff,
  filter,
  focus,
  labelMap,
  ontologyIri,
}: {
  diff: import("@/lib/domain").TtlVersionDiff;
  filter: "all" | "added" | "modified" | "removed";
  focus?: GraphDiffFocus | null;
  labelMap?: Map<string, string> | null;
  ontologyIri?: string;
}) {
  const revoke = useRevokeOntologyWorkspaceDiffItem();
  const cascade = useRevokeOntologyWorkspaceClassCascade();
  const [revokeErr, setRevokeErr] = useState("");
  const [revokeNote, setRevokeNote] = useState("");
  const related = filterDiffRelatedTo(diff, focus ?? null);
  const showA = filter === "all" || filter === "added";
  const showM = filter === "all" || filter === "modified";
  const showR = filter === "all" || filter === "removed";
  const filtered = {
    ...related,
    added_classes: showA ? related.added_classes : [],
    added_properties: showA ? related.added_properties : [],
    added_subclass_of: showA ? (related.added_subclass_of ?? []) : [],
    modified_classes: showM ? related.modified_classes : [],
    modified_properties: showM ? related.modified_properties : [],
    ontology_meta_changed: showM ? related.ontology_meta_changed : false,
    removed_classes: showR ? related.removed_classes : [],
    removed_properties: showR ? related.removed_properties : [],
    removed_subclass_of: showR ? (related.removed_subclass_of ?? []) : [],
  };
  const revokePending = revoke.isPending || cascade.isPending;
  return (
    <div>
      {revokeErr && (
        <p className="mb-2 text-[10px] text-red-600">{revokeErr}</p>
      )}
      {revokeNote && (
        <p className="mb-2 text-[10px] text-emerald-700 dark:text-emerald-400">
          {revokeNote}
        </p>
      )}
      <DiffView
        diff={filtered}
        showTitle={false}
        labelMap={labelMap}
        revokePending={revokePending}
        getRevokeBlockReason={
          ontologyIri
            ? // 禁止序必须对照完整 diff：focus 过滤后可能丢掉 removed_classes
              (target) => isLeafRestoreBlockedByDeletedClass(diff, target)
            : undefined
        }
        onRevoke={
          ontologyIri
            ? (target) => {
                void (async () => {
                  setRevokeErr("");
                  setRevokeNote("");
                  const blocked = isLeafRestoreBlockedByDeletedClass(
                    diff,
                    target,
                  );
                  if (blocked) {
                    setRevokeErr(blocked);
                    return;
                  }
                  const onError = (e: unknown) =>
                    setRevokeErr(
                      (e as Error).message || "撤销失败，请重试",
                    );

                  if (
                    target.kind === "class" &&
                    (target.op === "add" || target.op === "remove")
                  ) {
                    const deps = collectClassStrongDeps(diff, target.iri);
                    const lines = summarizeClassStrongDeps(deps);
                    const short =
                      labelMap?.get(target.iri)?.trim() ||
                      termToShort(target.iri);
                    const action =
                      target.op === "add" ? "撤销新增类" : "恢复已删除类";
                    const body =
                      lines.length > 0
                        ? `${action}「${short}」将同时处理：\n${lines.map((l) => `· ${l}`).join("\n")}\n\n确定继续？`
                        : `${action}「${short}」？`;
                    const ok = await confirm(body, { kind: "warning" });
                    if (!ok) return;
                    cascade.mutate(
                      {
                        iri: ontologyIri,
                        classIri: target.iri,
                        op: target.op,
                      },
                      {
                        onError,
                        onSuccess: () =>
                          setRevokeNote(
                            describeClassCascadeOutcome(deps, short),
                          ),
                      },
                    );
                    return;
                  }

                  revoke.mutate(
                    {
                      iri: ontologyIri,
                      item: toWorkspaceRevokeItem(target),
                    },
                    { onError },
                  );
                })();
              }
            : undefined
        }
      />
    </div>
  );
}

// ── 类层级树 ──
// 查所有 owl:Class + subClassOf，构建树。选中类后右栏显示详情（父/子/属性）。
function HierarchyPanel({ iri }: { iri: string }) {
  const sparql =
    "SELECT ?cls ?sup WHERE { " +
    "{ ?cls a owl:Class . FILTER(isIRI(?cls)) OPTIONAL { ?cls rdfs:subClassOf ?sup . FILTER(isIRI(?sup)) } } " +
    "}";
  const { data, isPending, error } = useSparqlQuery(iri, sparql);
  const { data: labelMap } = useTtlLabelMap(iri);
  const [selected, setSelected] = useState<string | null>(null);

  const tree = useMemo(() => {
    if (!data) return [] as TreeNode[];
    const parents = new Map<string, string[]>();
    const allClasses = new Set<string>();
    for (const row of data) {
      const cls = row["cls"] ?? "";
      const sup = row["sup"] ?? "";
      if (!cls) continue;
      allClasses.add(cls);
      if (sup) {
        if (cls === sup) continue;
        if (!parents.has(sup)) parents.set(sup, []);
        parents.get(sup)!.push(cls);
      }
    }
    const allChildren = new Set<string>();
    for (const kids of parents.values()) {
      kids.forEach((k) => allChildren.add(k));
    }
    // 根 = 出现在 subClassOf 但未作为子的，或无父的类
    const roots = [
      ...[...parents.keys()].filter((k) => !allChildren.has(k)),
      ...[...allClasses].filter((c) => !parents.has(c) && !allChildren.has(c)),
    ];
    const build = (label: string, seen = new Set<string>()): TreeNode => {
      if (seen.has(label))
        return {
          label,
          display: labelOf(labelMap, label),
          children: [],
        };
      seen.add(label);
      return {
        label,
        display: labelOf(labelMap, label),
        children: (parents.get(label) ?? [])
          .map((c) => build(c, new Set(seen)))
          .sort((a, b) => a.display.localeCompare(b.display, "zh")),
      };
    };
    return roots
      .map((r) => build(r))
      .sort((a, b) => a.display.localeCompare(b.display, "zh"));
  }, [data, labelMap]);

  if (isPending) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="animate-spin text-fg-subtle" size={20} />
      </div>
    );
  }
  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-4 text-center">
        <AlertTriangle size={20} className="text-warning" />
        <p className="text-sm text-danger">类层级加载失败</p>
        <p className="text-[11px] text-fg-subtle">{error.message}</p>
      </div>
    );
  }
  if (tree.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
        <GitBranch size={20} className="text-fg-subtle" />
        <p className="text-sm text-fg-subtle">本体无 owl:Class 定义。</p>
      </div>
    );
  }
  return (
    <div className="flex h-full">
      {/* 左：树 */}
      <div className="flex-1 overflow-y-auto border-r border-border p-3">
        <div className="space-y-0.5">
          {tree.map((n) => (
            <TreeView
              key={n.label}
              node={n}
              depth={0}
              selected={selected}
              onSelect={setSelected}
            />
          ))}
        </div>
      </div>
      {/* 右：选中类详情 */}
      <div className="w-64 shrink-0 overflow-y-auto p-3">
        {selected ? (
          <ClassDetailPanel iri={iri} classIri={selected} />
        ) : (
          <p className="mt-4 text-center text-[11px] text-fg-subtle">
            点击左侧类名查看详情
            <br />
            （父类 / 子类 / 对象属性）
          </p>
        )}
      </div>
    </div>
  );
}

interface TreeNode {
  label: string; // 完整 IRI（内部 key + 选中态）
  display: string; // 显示名（中文 label 或 IRI 短名）
  children: TreeNode[];
}
function TreeView({
  node,
  depth,
  selected,
  onSelect,
}: {
  node: TreeNode;
  depth: number;
  selected: string | null;
  onSelect: (label: string) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  const hasChildren = node.children.length > 0;
  return (
    <div>
      <div
        className="flex cursor-pointer items-center gap-1 rounded px-1 py-0.5 hover:bg-bg-hover/50"
        style={{ paddingLeft: depth * 16 }}
        onClick={() => onSelect(node.label)}
      >
        {hasChildren ? (
          <button
            onClick={(e) => {
              e.stopPropagation();
              setExpanded((e) => !e);
            }}
            className="text-fg-subtle hover:text-accent"
          >
            {expanded ? "▾" : "▸"}
          </button>
        ) : (
          <span className="w-3" />
        )}
        <Boxes
          size={11}
          className={`shrink-0 ${
            selected === node.label ? "text-accent" : "text-accent/70"
          }`}
        />
        <span
          className={`text-xs ${
            selected === node.label
              ? "font-semibold text-accent"
              : "font-medium"
          }`}
          title={node.label}
        >
          {node.display}
        </span>
      </div>
      {expanded &&
        hasChildren &&
        node.children.map((c) => (
          <TreeView
            key={c.label}
            node={c}
            depth={depth + 1}
            selected={selected}
            onSelect={onSelect}
          />
        ))}
    </div>
  );
}

/** 选中类的详情面板：父类、子类、以其为 domain/range 的属性。
 *  classLabel 是短名（termToShort 结果），用 CONTAINS(STR()) 匹配完整 IRI。 */
function ClassDetailPanel({
  iri,
  classIri,
}: {
  iri: string;
  classIri: string;
}) {
  // classIri 是完整 IRI，用精确 = 匹配（比 CONTAINS 快且准）
  const superQ = useSparqlQuery(
    iri,
    `SELECT ?sup WHERE { <${classIri}> rdfs:subClassOf ?sup . FILTER(isIRI(?sup)) }`,
  );
  const subQ = useSparqlQuery(
    iri,
    `SELECT ?sub WHERE { ?sub rdfs:subClassOf <${classIri}> . FILTER(isIRI(?sub)) }`,
  );
  const propQ = useSparqlQuery(
    iri,
    `SELECT ?p ?role ?other WHERE { { ?p rdfs:domain <${classIri}> . ?p rdfs:range ?other . BIND("domain" AS ?role) } UNION { ?p rdfs:range <${classIri}> . ?p rdfs:domain ?other . BIND("range" AS ?role) } FILTER(isIRI(?p)) FILTER(isIRI(?other)) }`,
  );
  const { data: labelMap } = useTtlLabelMap(iri);
  // 该类的证据（三态展示）
  const { data: allEvRows = [] } = useTtlElementEvidence(iri);
  const classEvRows = allEvRows.filter(
    (r) => r.element_id === classIri && r.element_type === "class",
  );
  const classStatus = evidenceStatusTtl(classEvRows);

  const supers = (superQ.data ?? [])
    .map((r) => r["sup"] ?? "")
    .filter(Boolean)
    .map((x) => labelOf(labelMap, x));
  const subs = (subQ.data ?? [])
    .map((r) => r["sub"] ?? "")
    .filter(Boolean)
    .map((x) => labelOf(labelMap, x));
  const props = (propQ.data ?? [])
    .map((r) => ({
      p: labelOf(labelMap, r["p"] ?? ""),
      role: r["role"] ?? "",
      other: labelOf(labelMap, r["other"] ?? ""),
    }))
    .filter((p) => p.p);

  return (
    <div className="space-y-3">
      <div>
        <h3 className="break-all text-xs font-semibold">
          <span title={EVIDENCE_STATUS_META[classStatus].title}>
            {EVIDENCE_STATUS_META[classStatus].icon}
          </span>{" "}
          {labelOf(labelMap, classIri)}
        </h3>
      </div>
      {classEvRows.length > 0 && (
        <div>
          <div className="mb-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
            依据（{classEvRows.length}）
          </div>
          <div className="space-y-1">
            {classEvRows.map((r, i) => (
              <div
                key={`${r.source_id}.${i}`}
                className="rounded bg-bg-hover px-2 py-1 text-[10px]"
                title={
                  r.matched_by === ""
                    ? `引用校验未通过：源「${r.source_title}」中未找到原文`
                    : `匹配等级：${r.matched_by === "exact" ? "原文精确匹配" : "规范化匹配"}`
                }
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-medium text-fg-muted">
                    {r.matched_by === "manual"
                      ? "⚪"
                      : r.role === "inferred"
                        ? "🟡"
                        : "🟢"}
                  </span>
                  <span className="truncate text-fg-subtle">
                    {r.source_title}
                  </span>
                </div>
                <div className="mt-0.5 truncate text-fg-subtle">
                  「{r.quote}」
                </div>
                {r.matched_by === "" && (
                  <div className="mt-0.5 text-danger">
                    ⚠ 引用在源中未找到（可能被改写）
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
      <ClassSourceSection iri={iri} classIri={classIri} />
      <DetailSection title="父类（subClassOf）" items={supers} />
      <DetailSection title="子类" items={subs} />
      <DetailSection
        title="关联属性（domain/range）"
        items={props.map((p) =>
          p.role === "domain" ? `${p.p} → ${p.other}` : `${p.p} ← ${p.other}`,
        )}
      />
    </div>
  );
}

/** 类卡上的来源表分区：未映射时不出现来源行（不占位、不提示缺失）。 */
function ClassSourceSection({
  iri,
  classIri,
}: {
  iri: string;
  classIri: string;
}) {
  const { byElement } = useMappingRowsByElement("w3c", iri);
  const rows = byElement.get(`class\u0000${classIri}`) ?? [];
  if (rows.length === 0) return null;
  return (
    <div>
      <div className="mb-1 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
        来源表
        <MappingStateBadge state={mappingStateOf(rows)} count={rows.length} />
      </div>
      <div className="space-y-1">
        {rows.map((r) => {
          // 类映射只到表级（column 通常为空），定位串天然省略末尾段
          const locator = endpointLocator(r.source)
          return (
          <div
            key={r.id}
            className="flex items-center justify-between gap-1 rounded bg-bg-hover px-2 py-1"
          >
            <span className="truncate font-mono text-[10px] text-fg-muted">
              {locator || "（未指定）"}
            </span>
            <button
              onClick={() => void navigator.clipboard?.writeText(locator)}
              className="shrink-0 rounded p-0.5 text-fg-subtle hover:bg-bg-base hover:text-accent"
              title="复制来源表"
            >
              <Copy size={10} />
            </button>
          </div>
          )
        })}
      </div>
    </div>
  );
}

function DetailSection({ title, items }: { title: string; items: string[] }) {
  return (
    <div>
      <div className="mb-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
        {title}
        <span className="ml-1 text-fg-subtle/60">({items.length})</span>
      </div>
      {items.length === 0 ? (
        <p className="text-[11px] text-fg-subtle/60">无</p>
      ) : (
        <ul className="space-y-0.5">
          {items.map((item, i) => (
            <li key={i} className="font-mono text-[11px] text-fg-muted">
              {item}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ── 规则逻辑树（SWRL swrl:Imp → 如果[前件]→那么[后件]）──
// SWRL atom 以 RDF List 存储：rule swrl:body/rdf:first [swrl:classPredicate C; swrl:argument1 ?x]。
// 这里查每个规则的 body/head atom，渲染为可读的「如果 ?x rdf:type C... → 那么 ?x rdf:type D...」。
function RulesPanel({ iri }: { iri: string }) {
  // 先查所有规则
  const rulesQ = useSparqlQuery(
    iri,
    "SELECT ?rule WHERE { ?rule a swrl:Imp . }",
  );
  // 对每条规则，查其 body（前件）+ head（后件）的 atom 列表展开项
  const ruleIris = (rulesQ.data ?? [])
    .map((r) => r["rule"] ?? "")
    .filter((r) => r.length > 0);

  if (rulesQ.isPending) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="animate-spin text-fg-subtle" size={20} />
      </div>
    );
  }
  if (rulesQ.error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-4 text-center">
        <AlertTriangle size={20} className="text-warning" />
        <p className="text-sm text-danger">规则加载失败</p>
        <p className="text-[11px] text-fg-subtle">{rulesQ.error.message}</p>
      </div>
    );
  }
  if (ruleIris.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
        <Workflow size={20} className="text-fg-subtle" />
        <p className="text-sm text-fg-subtle">
          本体无 SWRL 规则（第 4 类业务控制规则）。
        </p>
        <p className="text-[10px] text-fg-subtle">
          SWRL 规则需以 swrl:Imp 声明。
        </p>
      </div>
    );
  }
  return (
    <div className="h-full overflow-y-auto p-4">
      <div className="space-y-3">
        {ruleIris.map((rule, i) => (
          <SwrlRuleCard key={i} iri={iri} rule={rule} />
        ))}
      </div>
    </div>
  );
}

/** 单条 SWRL 规则卡：查 body/head atom 列表并渲染可读的「如果...那么...」。
 *  策略：body、head 各一个简单查询（避免 UNION 语法脆弱性）；
 *  查到 atom 就结构化展示；查不到则回退显示规则原始三元组，保证「至少能看懂」。 */
function SwrlRuleCard({ iri, rule }: { iri: string; rule: string }) {
  // rule 是裸 IRI 字符串（SPARQL Results JSON 的 value 字段不带尖括号），
  // 拼进 SPARQL 时必须包成 <...> 才是合法的 IRI term，否则会被解析器当成非法 token。
  const ruleTerm = `<${rule}>`;
  const { data: labelMap } = useTtlLabelMap(iri);
  // body atom（前件）：展开 RDF List，取 classPredicate + argument1/2
  const bodyQ = useSparqlQuery(
    iri,
    `SELECT ?atom ?cp ?a1 ?a2 ?pp ?pv WHERE { ${ruleTerm} swrl:body ?body . ?body rdf:rest*/rdf:first ?atom . { ?atom swrl:classPredicate ?cp . OPTIONAL { ?atom swrl:argument1 ?a1 } OPTIONAL { ?atom swrl:argument2 ?a2 } } UNION { ?atom swrl:propertyPredicate ?pp . ?atom swrl:argument1 ?a1 . OPTIONAL { ?atom swrl:argument2 ?pv } } }`,
  );
  // head atom（后件）
  const headQ = useSparqlQuery(
    iri,
    `SELECT ?atom ?cp ?a1 ?a2 ?pp ?pv WHERE { ${ruleTerm} swrl:head ?head . ?head rdf:rest*/rdf:first ?atom . { ?atom swrl:classPredicate ?cp . OPTIONAL { ?atom swrl:argument1 ?a1 } OPTIONAL { ?atom swrl:argument2 ?a2 } } UNION { ?atom swrl:propertyPredicate ?pp . ?atom swrl:argument1 ?a1 . OPTIONAL { ?atom swrl:argument2 ?pv } } }`,
  );
  // 兜底：查规则的所有属性（当 atom 列表为空或查询失败时展示原始结构）
  const fallbackQ = useSparqlQuery(
    iri,
    `SELECT ?p ?o WHERE { ${ruleTerm} ?p ?o . }`,
  );

  const isPending = bodyQ.isPending || headQ.isPending;
  const hasError = bodyQ.error || headQ.error;
  const bodyAtoms = bodyQ.data ?? [];
  const headAtoms = headQ.data ?? [];
  const hasStructured = bodyAtoms.length > 0 || headAtoms.length > 0;

  const renderAtom = (row: Record<string, string>) => {
    const cp = row["cp"] ? labelOf(labelMap, row["cp"]) : "";
    const pp = row["pp"] ? labelOf(labelMap, row["pp"]) : "";
    const a1 = row["a1"] ? formatArg(row["a1"]) : "";
    const a2 = row["a2"] ? formatArg(row["a2"]) : "";
    const pv = row["pv"] ? formatArg(row["pv"]) : "";
    if (cp) {
      if (a2) return `${a1} 是 ${cp}(${a2})`;
      if (a1) return `${a1} 是 ${cp}`;
      return `是 ${cp}`;
    }
    if (pp) {
      if (pv) return `${a1} 的 ${pp} = ${pv}`;
      if (a1) return `${a1} 的 ${pp}`;
      return pp;
    }
    return "(不可解析的 atom)";
  };

  const fallbackRows = (fallbackQ.data ?? []).filter(
    (r) =>
      !(
        termToShort(r["p"] ?? "").startsWith("type") ||
        termToShort(r["p"] ?? "") === "type"
      ),
  );

  return (
    <div className="rounded-md border border-border p-3">
      <div className="flex items-center gap-2">
        <Workflow size={12} className="text-accent" />
        <span className="text-xs font-semibold">{labelOf(labelMap, rule)}</span>
        {isPending && (
          <Loader2 size={10} className="animate-spin text-fg-subtle" />
        )}
      </div>
      {hasError ? (
        <p className="mt-2 text-[11px] text-danger">
          atom 解析失败：{(bodyQ.error || headQ.error)?.message}
        </p>
      ) : !isPending && !hasStructured ? (
        <div className="mt-2">
          <p className="mb-1.5 text-[11px] text-fg-subtle">
            规则未用标准 SWRL List 结构，原始三元组如下：
          </p>
          <ul className="space-y-0.5 font-mono text-[10px] text-fg-muted">
            {fallbackRows.length === 0 ? (
              <li className="text-fg-subtle">(无可展示的三元组)</li>
            ) : (
              fallbackRows.map((r, i) => (
                <li key={i}>
                  {labelOf(labelMap, r["p"] ?? "")}: {r["o"] ?? ""}
                </li>
              ))
            )}
          </ul>
        </div>
      ) : (
        <div className="mt-2 flex items-start gap-2 text-[11px]">
          <div className="flex-1 rounded border border-warning-border bg-warning-bg p-2">
            <div className="mb-1 flex items-center gap-1 font-medium text-warning-text">
              <ArrowRight size={10} className="rotate-180" />
              如果（前件 body）
            </div>
            {isPending ? (
              <p className="text-fg-subtle">解析中...</p>
            ) : bodyAtoms.length > 0 ? (
              <ul className="space-y-0.5 font-mono">
                {bodyAtoms.map((a, i) => (
                  <li key={i}>• {renderAtom(a)}</li>
                ))}
              </ul>
            ) : (
              <p className="text-fg-subtle">无前件</p>
            )}
          </div>
          <ArrowRight size={14} className="mt-4 shrink-0 text-accent" />
          <div className="flex-1 rounded border border-emerald-500/20 bg-emerald-500/5 p-2">
            <div className="mb-1 flex items-center gap-1 font-medium text-emerald-600 dark:text-emerald-400">
              <ArrowRight size={10} />
              那么（后件 head）
            </div>
            {isPending ? (
              <p className="text-fg-subtle">解析中...</p>
            ) : headAtoms.length > 0 ? (
              <ul className="space-y-0.5 font-mono">
                {headAtoms.map((a, i) => (
                  <li key={i}>• {renderAtom(a)}</li>
                ))}
              </ul>
            ) : (
              <p className="text-fg-subtle">无后件</p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/** 格式化 SWRL argument 变量/常量。 */
function formatArg(arg: string): string {
  if (!arg) return "";
  if (arg.startsWith("http://") || arg.startsWith("https://")) {
    return `«${termToShort(arg)}»`;
  }
  return arg; // 变量名，如 ?x、?x1
}

// ── Turtle 源码 ──
// ── 属性编辑面板（手动编辑 P0：属性/关系，不动类）──
// 数据源 useOntologyTtlProperties（后端文本手术聚合散块，非 SPARQL）。
// 编辑/新增/删除走 surgery 命令：校验比 import 严格（errors 非空即拒），
// 保注释/排版/prefix（原位替换主语块，不重序列化全文）。

/** xsd 常用数据类型（datatype 属性 range 下拉）。 */
const XSD_TYPES = [
  "http://www.w3.org/2001/XMLSchema#string",
  "http://www.w3.org/2001/XMLSchema#integer",
  "http://www.w3.org/2001/XMLSchema#decimal",
  "http://www.w3.org/2001/XMLSchema#float",
  "http://www.w3.org/2001/XMLSchema#double",
  "http://www.w3.org/2001/XMLSchema#boolean",
  "http://www.w3.org/2001/XMLSchema#date",
  "http://www.w3.org/2001/XMLSchema#dateTime",
  "http://www.w3.org/2001/XMLSchema#long",
];

/** 表单草稿：TtlPropertyEdit 的可编辑投影（iri 编辑态只读）。 */
type PropertyDraft = {
  iri: string;
  kind: string;
  label: string;
  comment: string;
  domain: string;
  range: string;
};

function draftOf(p: TtlPropertyEdit): PropertyDraft {
  return {
    iri: p.iri,
    kind: p.kind,
    label: p.label ?? "",
    comment: p.comment ?? "",
    domain: p.domain ?? "",
    range: p.range ?? "",
  };
}

function draftToEdit(d: PropertyDraft): TtlPropertyEdit {
  return {
    iri: d.iri,
    kind: d.kind,
    label: d.label,
    comment: d.comment,
    domain: d.domain,
    range: d.range,
  };
}

export function PropertiesPanel({ iri }: { iri: string }) {
  const { data: properties, isLoading } = useOntologyTtlProperties(iri);
  // 元素级证据（W3C 侧三态展示，docs/EVIDENCE-CHAIN.md §6）
  const { data: evidenceRows = [] } = useTtlElementEvidence(iri);
  const { data: classIris } = useOntologyTtlClassIris(iri);
  const { data: labelMap } = useTtlLabelMap(iri);
  // 新增属性的默认 IRI 前缀（与既有属性 namespace 一致，而非本体 IRI）
  const { data: defaultNs } = useOntologyTtlDefaultPropertyNs(iri);
  const editMut = useEditOntologyTtlProperty();
  const addMut = useAddOntologyTtlProperty();
  const deleteMut = useDeleteOntologyTtlProperty();

  // 编辑中的属性 IRI（null = 列表态）；新增态用特殊标记
  const [editingIri, setEditingIri] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [draft, setDraft] = useState<PropertyDraft | null>(null);
  const [formErr, setFormErr] = useState<string | null>(null);
  // 搜索过滤（名称/IRI 短名/说明，大小写不敏感）——大本体 300+ 属性靠滚动不现实
  const [filter, setFilter] = useState("");
  const busy = editMut.isPending || addMut.isPending || deleteMut.isPending;

  // 来源字段（决策 25）：映射与概念分离，此处只读用于列表展示与筛选。
  // 过滤逻辑在 `src/lib/ttl-property-filter.ts`（纯函数 + 单测）：
  // 依赖通过参数显式传入，不在组件体内用闭包捕获后声明的 state，
  // 从而避免渲染期立即执行命中 TDZ（"Cannot access ... before initialization"）。
  const { byElement: mappingByElement } = useMappingRowsByElement("w3c", iri);
  const [onlyUnmapped, setOnlyUnmapped] = useState(false);

  const { datatypeProps, objectProps } = useMemo(
    () =>
      splitTtlProperties(properties ?? [], {
        query: filter,
        onlyUnmapped,
        mappingByElement,
      }),
    [properties, filter, onlyUnmapped, mappingByElement],
  );

  // 子页签：关系 / 数据属性（搜索跨两页生效，页签显示过滤后计数）。
  // 注意：全部 hooks 必须在任何早退 return 之前（Rules of Hooks——
  // loading/draft 态提前 return 时 hooks 数量不能变，否则 React #310）
  const [subTab, setSubTab] = useState<"object" | "datatype">("object");
  // domain 折叠分组（数据属性页签用）：key = domain IRI（"" = 未指定）
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  // 按 domain 分组（保留类下拉顺序无关，按组名排序；无 domain 单列「未指定」）
  const domainGroups = useMemo(
    () => groupByDomain(datatypeProps, labelMap),
    [datatypeProps, labelMap],
  );

  function startEdit(p: TtlPropertyEdit) {
    setAdding(false);
    setEditingIri(p.iri);
    setDraft(draftOf(p));
    setFormErr(null);
  }

  function startAdd() {
    // 新属性 IRI：优先取既有属性的 namespace（后端投票得出，与全文
    // 形态统一——如 ns: 短形对应的绝对 IRI）；兜底本体 IRI 的 # 前缀
    const base =
      defaultNs ??
      (iri.includes("#") ? iri.slice(0, iri.lastIndexOf("#") + 1) : `${iri}#`);
    setEditingIri(null);
    setAdding(true);
    setDraft({
      iri: `${base}newProperty`,
      kind: "datatype",
      label: "",
      comment: "",
      domain: "",
      range: XSD_TYPES[0],
    });
    setFormErr(null);
  }

  function cancelForm() {
    setEditingIri(null);
    setAdding(false);
    setDraft(null);
    setFormErr(null);
  }

  async function saveForm() {
    if (!draft) return;
    // 前端预检：label 必填（后端 rdfs:label 语义要求）
    if (!draft.label.trim()) {
      setFormErr("label 不能为空");
      return;
    }
    // IRI 尾段 camelCase 预检（后端校验为准，这里只做即时反馈）
    const local = shortIri(draft.iri);
    if (!/^[a-z][A-Za-z0-9]*$/.test(local)) {
      setFormErr(`属性名「${local}」不符合 camelCase 规范（小写字母开头）`);
      return;
    }
    // object 关系必须指定 range（指向类）
    if (draft.kind === "object" && !draft.range) {
      setFormErr("关系（object）必须指定 range（目标类）");
      return;
    }
    setFormErr(null);
    try {
      if (adding) {
        await addMut.mutateAsync({ iri, edit: draftToEdit(draft) });
      } else {
        await editMut.mutateAsync({ iri, edit: draftToEdit(draft) });
      }
      cancelForm();
    } catch (e) {
      setFormErr(e instanceof Error ? e.message : String(e));
    }
  }

  async function handleDelete(p: TtlPropertyEdit) {
    const confirmed = await confirm(
      `确定删除属性「${p.label || shortIri(p.iri)}」？\n\n将移除其全部 TTL 定义块。若被 owl:onProperty / subPropertyOf 引用会被校验拒绝。`,
      { kind: "warning" },
    );
    if (!confirmed) return;
    try {
      await deleteMut.mutateAsync({ iri, propertyIri: p.iri });
      if (editingIri === p.iri) cancelForm();
    } catch (e) {
      await message(`删除失败：${e instanceof Error ? e.message : String(e)}`, {
        kind: "error",
      });
    }
  }

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="animate-spin text-fg-subtle" size={20} />
      </div>
    );
  }

  // ── 编辑表单态 ──
  if (draft) {
    return (
      <div className="h-full overflow-auto p-4">
        <div className="mx-auto max-w-2xl">
          <div className="mb-4 flex items-center justify-between">
            <h3 className="text-sm font-semibold">
              {adding ? "新增属性" : "编辑属性"}
            </h3>
            <button
              onClick={cancelForm}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover"
            >
              <X size={12} />
              取消
            </button>
          </div>

          <div className="space-y-3 rounded-lg border border-border bg-bg-base p-4">
            {/* IRI */}
            <div>
              <label className="mb-1 block text-xs font-medium text-fg-muted">
                IRI{adding ? "" : "（身份，不可修改）"}
              </label>
              <input
                value={draft.iri}
                readOnly={!adding}
                onChange={(e) => setDraft({ ...draft, iri: e.target.value })}
                className={`w-full rounded-md border border-border bg-bg-base px-2 py-1.5 font-mono text-xs ${
                  adding ? "" : "text-fg-subtle"
                }`}
              />
              {!adding && (
                <p className="mt-1 text-[10px] text-fg-subtle">
                  修改 IRI 等于换一个属性：先新增再删除旧的
                </p>
              )}
            </div>

            {/* 类型切换 */}
            <div>
              <label className="mb-1 block text-xs font-medium text-fg-muted">
                类型
              </label>
              <div className="flex gap-2">
                <button
                  onClick={() =>
                    setDraft({
                      ...draft,
                      kind: "datatype",
                      range:
                        draft.kind === "object" ? XSD_TYPES[0] : draft.range,
                    })
                  }
                  disabled={busy}
                  className={`rounded-md border px-3 py-1.5 text-xs ${
                    draft.kind === "datatype"
                      ? "border-accent bg-accent/10 text-accent"
                      : "border-border text-fg-muted hover:bg-bg-hover"
                  }`}
                >
                  数据属性
                  <span className="ml-1 text-[10px] text-fg-subtle">
                    DatatypeProperty
                  </span>
                </button>
                <button
                  onClick={() =>
                    setDraft({
                      ...draft,
                      kind: "object",
                      range: "",
                    })
                  }
                  disabled={busy}
                  className={`rounded-md border px-3 py-1.5 text-xs ${
                    draft.kind === "object"
                      ? "border-accent bg-accent/10 text-accent"
                      : "border-border text-fg-muted hover:bg-bg-hover"
                  }`}
                >
                  关系
                  <span className="ml-1 text-[10px] text-fg-subtle">
                    ObjectProperty
                  </span>
                </button>
              </div>
            </div>

            {/* label / comment */}
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="mb-1 block text-xs font-medium text-fg-muted">
                  名称（rdfs:label）
                </label>
                <input
                  value={draft.label}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      label: e.target.value,
                    })
                  }
                  className="w-full rounded-md border border-border bg-bg-base px-2 py-1.5 text-xs"
                  placeholder="中文名"
                />
              </div>
              <div>
                <label className="mb-1 block text-xs font-medium text-fg-muted">
                  说明（rdfs:comment）
                </label>
                <input
                  value={draft.comment}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      comment: e.target.value,
                    })
                  }
                  className="w-full rounded-md border border-border bg-bg-base px-2 py-1.5 text-xs"
                  placeholder="可选"
                />
              </div>
            </div>

            {/* domain：类下拉 */}
            <div>
              <label className="mb-1 block text-xs font-medium text-fg-muted">
                所属类（rdfs:domain）
              </label>
              <select
                value={draft.domain}
                onChange={(e) =>
                  setDraft({
                    ...draft,
                    domain: e.target.value,
                  })
                }
                className="w-full rounded-md border border-border bg-bg-base px-2 py-1.5 text-xs"
              >
                <option value="">（未指定）</option>
                {(classIris ?? []).map((c) => (
                  <option key={c} value={c}>
                    {labelMap?.get(c) ?? shortIri(c)}
                  </option>
                ))}
              </select>
            </div>

            {/* range：datatype → xsd 下拉；object → 类下拉 */}
            <div>
              <label className="mb-1 block text-xs font-medium text-fg-muted">
                取值类型（rdfs:range）
                {draft.kind === "object" ? "：关系指向的类" : "：xsd 数据类型"}
              </label>
              {draft.kind === "datatype" ? (
                <select
                  value={XSD_TYPES.includes(draft.range) ? draft.range : ""}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      range: e.target.value,
                    })
                  }
                  className="w-full rounded-md border border-border bg-bg-base px-2 py-1.5 text-xs"
                >
                  <option value="">（未指定）</option>
                  {XSD_TYPES.map((t) => (
                    <option key={t} value={t}>
                      {shortIri(t)}
                    </option>
                  ))}
                  {/* 已有但不在常用列表的 xsd 类型（如受限类型） */}
                  {draft.range &&
                    draft.range.startsWith(
                      "http://www.w3.org/2001/XMLSchema#",
                    ) &&
                    !XSD_TYPES.includes(draft.range) && (
                      <option value={draft.range}>
                        {shortIri(draft.range)}
                      </option>
                    )}
                </select>
              ) : (
                <select
                  value={draft.range}
                  onChange={(e) =>
                    setDraft({
                      ...draft,
                      range: e.target.value,
                    })
                  }
                  className="w-full rounded-md border border-border bg-bg-base px-2 py-1.5 text-xs"
                >
                  <option value="">（必选：目标类）</option>
                  {(classIris ?? []).map((c) => (
                    <option key={c} value={c}>
                      {labelMap?.get(c) ?? shortIri(c)}
                    </option>
                  ))}
                </select>
              )}
            </div>

            {/* 来源字段（与概念字段分区：保存映射不改变概念定义） */}
            {!adding && (
              <div className="border-t border-border pt-3">
                <div className="mb-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                  数据来源
                </div>
                <ElementMappingEditor
                  standard="w3c"
                  ontologyKey={iri}
                  elementType={
                    draft.kind === "object"
                      ? "object_property"
                      : "datatype_property"
                  }
                  elementId={draft.iri}
                  displayName={draft.label}
                  dataType={draft.kind === "object" ? "" : draft.range}
                  sameTableHint={draft.domain}
                />
                <p className="mt-1 text-[10px] text-fg-subtle">
                  来源字段单独保存，不写进 Turtle 概念定义
                </p>
              </div>
            )}

            {/* 错误 + 保存 */}
            {formErr && (
              <div className="rounded-md border border-danger/30 bg-danger-bg/50 px-3 py-2 text-xs text-danger">
                {formErr}
              </div>
            )}
            <div className="flex justify-end gap-2 pt-1">
              <button
                onClick={cancelForm}
                disabled={busy}
                className="rounded-md px-3 py-1.5 text-xs text-fg-muted hover:bg-bg-hover"
              >
                取消
              </button>
              <button
                onClick={saveForm}
                disabled={busy}
                className="flex items-center gap-1 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-white disabled:opacity-50"
              >
                {busy ? (
                  <Loader2 size={12} className="animate-spin" />
                ) : (
                  <Save size={12} />
                )}
                {adding ? "新增" : "保存"}
              </button>
            </div>
            <p className="text-[10px] text-fg-subtle">
              保存 = 文本手术（原位替换该属性的主语块），Turtle
              源码中的注释与排版保持不变；结果校验失败会整体拒绝。
            </p>
          </div>
        </div>
      </div>
    );
  }

  // ── 列表态 ──
  const total = properties?.length ?? 0;
  const shown = datatypeProps.length + objectProps.length;

  const activeList = subTab === "object" ? objectProps : datatypeProps;

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-4 py-2">
        <div className="relative flex-1 max-w-md">
          <Search
            size={12}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-fg-subtle"
          />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="搜索属性（名称 / IRI / 说明 / domain→range）"
            className="w-full rounded-md border border-border bg-bg-base py-1.5 pl-7 pr-2 text-xs"
          />
        </div>
        <button
          onClick={() => setOnlyUnmapped((v) => !v)}
          className={`shrink-0 rounded-md border px-2 py-1 text-[11px] transition ${
            onlyUnmapped
              ? "border-accent bg-accent/10 text-accent"
              : "border-border text-fg-subtle hover:bg-bg-hover"
          }`}
          title="只显示尚未指定来源字段的属性"
        >
          只看未映射
        </button>
        <div className="shrink-0 text-xs text-fg-subtle">
          {filter.trim() ? `${shown}/${total} 匹配` : `共 ${total} 属性`}
        </div>
        <button
          onClick={startAdd}
          className="ml-auto flex shrink-0 items-center gap-1 rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-white hover:bg-accent/90"
        >
          <Plus size={12} />
          新增属性
        </button>
      </div>

      {/* 子页签：关系 / 数据属性（各自过滤后计数） */}
      <div className="flex shrink-0 gap-1 border-b border-border px-3 py-1.5">
        {(
          [
            { key: "object", label: "关系", count: objectProps.length },
            {
              key: "datatype",
              label: "数据属性",
              count: datatypeProps.length,
            },
          ] as const
        ).map((t) => (
          <button
            key={t.key}
            onClick={() => setSubTab(t.key)}
            className={`flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-xs transition ${
              subTab === t.key
                ? "bg-bg-hover font-medium text-accent"
                : "text-fg-subtle hover:bg-bg-hover/50"
            }`}
          >
            {t.label}
            <span className="rounded bg-bg-hover/60 px-1 text-[10px] text-fg-subtle">
              {t.count}
            </span>
          </button>
        ))}
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-4">
        {activeList.length === 0 ? (
          <div className="rounded-md border border-dashed border-border px-3 py-2 text-xs text-fg-subtle">
            {filter.trim() ? "无匹配属性" : "暂无"}
          </div>
        ) : subTab === "object" ? (
          /* 关系页：平铺表格（80 行量级，不需要分组） */
          <PropertyTable
            list={objectProps}
            labelMap={labelMap}
            evidenceRows={evidenceRows}
            mappingByElement={mappingByElement}
            busy={busy}
            onEdit={startEdit}
            onDelete={handleDelete}
          />
        ) : (
          /* 数据属性页：按 domain 折叠分组 */
          <div className="space-y-2">
            {domainGroups.map(([domain, list]) => {
              const open = !collapsed.has(domain);
              const groupName = domain
                ? (labelMap?.get(domain) ?? shortIri(domain))
                : "未指定 domain";
              return (
                <div
                  key={domain}
                  className="overflow-hidden rounded-lg border border-border"
                >
                  <button
                    onClick={() => {
                      setCollapsed((prev) => {
                        const next = new Set(prev);
                        if (next.has(domain)) next.delete(domain);
                        else next.add(domain);
                        return next;
                      });
                    }}
                    className="flex w-full items-center gap-1.5 bg-bg-hover/30 px-3 py-1.5 text-left text-xs font-semibold text-fg-muted hover:bg-bg-hover/50"
                  >
                    {open ? (
                      <ChevronDown size={12} />
                    ) : (
                      <ChevronRight size={12} />
                    )}
                    {groupName}
                    <span className="rounded bg-bg-hover/60 px-1 text-[10px] font-normal text-fg-subtle">
                      {list.length}
                    </span>
                  </button>
                  {open && (
                    <PropertyTable
                      list={list}
                      labelMap={labelMap}
                      evidenceRows={evidenceRows}
                      mappingByElement={mappingByElement}
                      busy={busy}
                      onEdit={startEdit}
                      onDelete={handleDelete}
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

/** 属性表格（无外框，嵌入分组容器或独立使用）。 */
function PropertyTable({
  list,
  labelMap,
  evidenceRows,
  mappingByElement,
  busy,
  onEdit,
  onDelete,
}: {
  list: TtlPropertyEdit[];
  labelMap?: Map<string, string>;
  /** W3C 元素级证据行（element_id=属性 IRI） */
  evidenceRows: ElementEvidenceRow[];
  /** 元素键 → 映射行（来源字段列展示用） */
  mappingByElement: Map<string, ElementMapping[]>;
  busy: boolean;
  onEdit: (p: TtlPropertyEdit) => void;
  onDelete: (p: TtlPropertyEdit) => void;
}) {
  /** 属性对应的映射行（datatype/object 两种类型前缀都查一遍）。 */
  const rowsOf = (p: TtlPropertyEdit): ElementMapping[] =>
    mappingByElement.get(`datatype_property\u0000${p.iri}`) ??
    mappingByElement.get(`object_property\u0000${p.iri}`) ??
    [];
  const statusOf = (iri: string): EvidenceStatus => {
    const rows = evidenceRows.filter(
      (r) =>
        r.element_id === iri &&
        (r.element_type === "datatype_property" ||
          r.element_type === "object_property"),
    );
    return evidenceStatusTtl(rows);
  };
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-xs">
        <thead className="bg-bg-hover/30 text-fg-subtle">
          <tr>
            <th className="px-3 py-1.5 text-left font-medium">名称</th>
            <th className="px-3 py-1.5 text-left font-medium">IRI</th>
            <th className="px-3 py-1.5 text-left font-medium">
              domain → range
            </th>
            <th className="px-3 py-1.5 text-left font-medium">来源字段</th>
            <th className="px-3 py-1.5 text-left font-medium">依据</th>
            <th className="px-3 py-1.5 text-right font-medium">操作</th>
          </tr>
        </thead>
        <tbody>
          {list.map((p) => (
            <tr
              key={p.iri}
              className="border-t border-border hover:bg-bg-hover/20"
            >
              <td className="px-3 py-1.5">
                <div className="font-medium">{p.label || shortIri(p.iri)}</div>
                {p.comment && (
                  <div className="text-[10px] text-fg-subtle">{p.comment}</div>
                )}
              </td>
              <td className="px-3 py-1.5 font-mono text-[10px] text-fg-subtle">
                {shortIri(p.iri)}
              </td>
              <td className="px-3 py-1.5 text-fg-muted">
                {p.domain || p.range ? (
                  <span className="flex items-center gap-1">
                    <span>
                      {p.domain
                        ? (labelMap?.get(p.domain) ?? shortIri(p.domain))
                        : "（任意）"}
                    </span>
                    <ArrowRight size={10} className="text-fg-subtle" />
                    <span>
                      {p.range
                        ? (labelMap?.get(p.range) ?? shortIri(p.range))
                        : "（任意）"}
                    </span>
                  </span>
                ) : (
                  <span className="text-fg-subtle">（未指定）</span>
                )}
              </td>
              <td className="px-3 py-1.5">
                {(() => {
                  const rows = rowsOf(p);
                  const st = mappingStateOf(rows);
                  return (
                    <span className="flex items-center gap-1.5">
                      <MappingStateBadge state={st} count={rows.length} />
                      <SourceFieldCell
                        rows={rows}
                        onCopy={(text) => {
                          void navigator.clipboard?.writeText(text);
                        }}
                      />
                    </span>
                  );
                })()}
              </td>
              <td className="px-3 py-1.5">
                {(() => {
                  const st = statusOf(p.iri);
                  const rows = evidenceRows.filter(
                    (r) => r.element_id === p.iri,
                  );
                  const meta = EVIDENCE_STATUS_META[st];
                  return (
                    <span
                      title={
                        rows.some((r) => r.matched_by === "")
                          ? `引用校验未通过（源中未找到原文）｜${meta.title}`
                          : meta.title
                      }
                    >
                      {meta.icon}
                      {rows.some((r) => r.matched_by === "") && (
                        <span className="ml-0.5 text-[10px] text-danger">
                          ⚠
                        </span>
                      )}
                    </span>
                  );
                })()}
              </td>
              <td className="px-3 py-1.5 text-right">
                <button
                  onClick={() => onEdit(p)}
                  disabled={busy}
                  className="mr-1 rounded-md p-1 text-fg-subtle hover:bg-bg-hover hover:text-accent disabled:opacity-50"
                  title="编辑"
                >
                  <Pencil size={12} />
                </button>
                <button
                  onClick={() => onDelete(p)}
                  disabled={busy}
                  className="rounded-md p-1 text-fg-subtle hover:bg-danger-bg hover:text-danger disabled:opacity-50"
                  title="删除"
                >
                  <Trash2 size={12} />
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SourcePanel({
  content,
  focusLine,
}: {
  content?: string;
  focusLine?: number | null;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const codeRef = useRef<HTMLPreElement>(null);
  // 联动聚焦：滚动到目标行并短暂高亮。
  // Rules of Hooks：切换本体时 content 会先变 undefined（新 IRI 的 query 还没回），
  // 若把本 effect 放在加载早退之后，同一实例的前后两次渲染 hook 数量不同，
  // React 会抛 #310「Rendered fewer hooks than expected」。故所有 hook 必须在早退之前。
  useEffect(() => {
    if (!focusLine || !scrollRef.current) return;
    const lineEl = scrollRef.current.querySelector(
      `[data-line="${focusLine}"]`,
    );
    lineEl?.scrollIntoView({ block: "center" });
  }, [focusLine, content]);
  if (!content) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="animate-spin text-fg-subtle" size={20} />
      </div>
    );
  }
  const lines = content.split("\n");
  return (
    <div ref={scrollRef} className="h-full overflow-auto bg-bg-base/50">
      <div className="flex min-h-full font-mono text-[11px] leading-relaxed">
        {/* 行号列 */}
        <div className="select-none border-r border-border bg-bg-hover/20 px-2 py-3 text-right text-fg-subtle">
          {lines.map((_, i) => (
            <div key={i}>{i + 1}</div>
          ))}
        </div>
        {/* 代码列 */}
        <pre
          ref={codeRef}
          className="flex-1 overflow-x-auto whitespace-pre-wrap break-all px-3 py-3 text-fg-muted"
        >
          {lines.map((line, i) => (
            <div
              key={i}
              data-line={i + 1}
              className={
                focusLine && focusLine === i + 1
                  ? "bg-amber-500/20 text-amber-900 dark:text-amber-200"
                  : ""
              }
            >
              {line}
            </div>
          ))}
        </pre>
      </div>
    </div>
  );
}

// ── SPARQL 控制台（第 7 类 CRUD）──
function SparqlPanel({ iri }: { iri: string }) {
  const [query, setQuery] = useState(SPARQL_TEMPLATES[0].sparql);
  const sparqlMut = useQueryOntologySparql();
  const [result, setResult] = useState<{
    vars: string[];
    rows: Record<string, string>[];
  } | null>(null);
  const [resultRaw, setResultRaw] = useState<string>("");
  const [err, setErr] = useState<string | null>(null);

  async function run() {
    setErr(null);
    setResult(null);
    try {
      const json = await sparqlMut.mutateAsync({
        ontologyIri: iri,
        sparql: query,
      });
      setResultRaw(json);
      const parsed = JSON.parse(json);
      if (parsed.head && parsed.results) {
        setResult({
          vars: parsed.head.vars ?? [],
          rows: (parsed.results.bindings ?? []).map(
            (b: Record<string, { value: string }>) => {
              const row: Record<string, string> = {};
              for (const [k, v] of Object.entries(b)) {
                row[k] = v.value;
              }
              return row;
            },
          ),
        });
      }
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="flex h-full flex-col">
      <div className="border-b border-border p-3">
        <div className="mb-2 flex flex-wrap gap-1">
          {SPARQL_TEMPLATES.map((t) => (
            <button
              key={t.label}
              onClick={() => setQuery(t.sparql)}
              className="rounded bg-bg-hover px-2 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-active hover:text-accent"
            >
              {t.label}
            </button>
          ))}
        </div>
        <textarea
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          rows={4}
          className="w-full resize-y rounded border border-border bg-bg-base px-2 py-1 font-mono text-[11px] focus:outline-none"
        />
        <div className="mt-2 flex items-center gap-2">
          <button
            onClick={run}
            disabled={sparqlMut.isPending}
            className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-white hover:bg-accent/90 disabled:opacity-50"
          >
            {sparqlMut.isPending ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Terminal size={12} />
            )}
            执行
          </button>
          {resultRaw && (
            <span className="text-[10px] text-fg-subtle">
              {result?.rows.length ?? 0} 行结果
            </span>
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-3">
        {err && <div className="text-xs text-danger">查询错误：{err}</div>}
        {result && result.rows.length > 0 && (
          <table className="w-full text-[11px]">
            <thead>
              <tr className="border-b border-border text-left text-fg-subtle">
                {result.vars.map((v) => (
                  <th key={v} className="py-1 pr-3 font-medium">
                    {v}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {result.rows.map((row, i) => (
                <tr key={i} className="border-b border-border/50">
                  {result.vars.map((v) => (
                    <td key={v} className="py-1 pr-3 text-fg-muted">
                      {row[v] ?? ""}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {result && result.rows.length === 0 && (
          <div className="text-xs text-fg-subtle">查询无结果行。</div>
        )}
      </div>
    </div>
  );
}

// ── 宪章面板（+1 目标与评估）──
// ── 宪章 AI 建议（P0 纯前端）──
// 红线：AI 永不直接写宪章；建议必须用户逐字段采纳；AI 不虚构业务事实（缺失→提问）。

/** 本体结构上下文：类/属性的中文 label + comment（无 OPTIONAL，避免 oxigraph 笛卡尔积）。 */
const CHARTER_ENTITY_CONTEXT_SPARQL =
  'SELECT ?label ?comment WHERE { ?s rdfs:label ?label ; rdfs:comment ?comment . FILTER(LANGMATCHES(LANG(?label), "zh")) } LIMIT 150';

type CharterFieldKey = "business_scenario" | "business_essence";

const CHARTER_FIELDS: { key: CharterFieldKey; label: string }[] = [
  { key: "business_scenario", label: "业务场景" },
  { key: "business_essence", label: "业务目标" },
];

function CharterAiSuggest({
  iri,
  draft,
  onApply,
}: {
  iri: string;
  draft: TtlCharter;
  onApply: (patch: Partial<TtlCharter>) => void;
}) {
  const { data: provider } = useProvider();
  const { data: rows } = useSparqlQueryRead(iri, CHARTER_ENTITY_CONTEXT_SPARQL);
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<CharterSuggestMode>(() =>
    draft.business_scenario || draft.business_essence ? "complete" : "draft",
  );
  const [suggestion, setSuggestion] = useState<CharterSuggestion | null>(null);
  const [dismissed, setDismissed] = useState<Set<CharterFieldKey>>(new Set());
  const [adopted, setAdopted] = useState<Set<CharterFieldKey>>(new Set());
  const [loading, setLoading] = useState(false);
  // 业务背景：手工导入的空宪章本体缺参考材料，AI 起草前用户在此补充（仅本次编辑会话内有效）
  const [background, setBackground] = useState("");

  const entityContext = useMemo(
    () => (rows ?? []).map((r) => `- ${r.label}：${r.comment}`).join("\n"),
    [rows],
  );

  async function request() {
    if (!provider || loading) return;
    setLoading(true);
    setSuggestion(null);
    setDismissed(new Set());
    setAdopted(new Set());
    try {
      setSuggestion(
        await suggestCharter(provider, {
          mode,
          draft,
          entityContext: background.trim()
            ? `${entityContext}\n\n<业务背景-用户补充>\n${background.trim()}\n</业务背景-用户补充>`
            : entityContext,
        }),
      );
    } catch (e) {
      void message(e instanceof Error ? e.message : String(e), {
        kind: "error",
      });
    } finally {
      setLoading(false);
    }
  }

  function adopt(key: CharterFieldKey) {
    const v = suggestion?.[key];
    if (typeof v !== "string") return;
    onApply({ [key]: v });
    setAdopted(new Set([...adopted, key]));
  }

  function dismiss(key: CharterFieldKey) {
    setDismissed(new Set([...dismissed, key]));
  }

  /** 一键忽略全部建议（不采纳时的快捷处理） */
  function dismissAll() {
    if (!suggestion) return;
    setDismissed(
      new Set(
        CHARTER_FIELDS.filter((f) => typeof suggestion[f.key] === "string").map(
          (f) => f.key,
        ),
      ),
    );
  }

  function adoptAll() {
    if (!suggestion) return;
    const patch: Partial<TtlCharter> = {};
    for (const f of CHARTER_FIELDS) {
      const v = suggestion[f.key];
      if (typeof v === "string" && !dismissed.has(f.key)) {
        patch[f.key] = v;
      }
    }
    if (Object.keys(patch).length === 0) return;
    onApply(patch);
    setAdopted(
      new Set([...adopted, ...(Object.keys(patch) as CharterFieldKey[])]),
    );
  }

  const adoptable = suggestion
    ? CHARTER_FIELDS.filter(
        (f) =>
          typeof suggestion[f.key] === "string" &&
          !dismissed.has(f.key) &&
          !adopted.has(f.key),
      )
    : [];

  return (
    <div className="rounded-md border border-border bg-bg-base/60 p-2">
      <div className="flex items-center justify-between">
        <button
          onClick={() => setOpen(!open)}
          className={`flex items-center gap-1.5 rounded-md px-2 py-1 text-[11px] font-medium ${
            open
              ? "bg-accent text-white"
              : "bg-accent/10 text-accent hover:bg-accent/20"
          }`}
        >
          <Sparkles size={12} />
          AI 建议
        </button>
        {open && (
          <span className="text-[10px] text-fg-subtle">
            仅供参考——采纳后仍需你确认保存
          </span>
        )}
      </div>
      {open && (
        <div className="mt-2 space-y-2">
          <label className="block">
            <span className="text-[10px] font-medium text-fg-subtle">
              业务背景（可选）——粘贴文档要点或描述业务，供 AI 起草参考
            </span>
            <textarea
              value={background}
              onChange={(e) => setBackground(e.target.value)}
              rows={2}
              placeholder="例如：面向汽车经销商的零售业务，覆盖试驾、成交、交付……"
              className="mt-0.5 w-full resize-y rounded-md border border-border bg-bg-base px-2 py-1 text-xs outline-none"
            />
          </label>
          <div className="flex flex-wrap items-center gap-1">
            {CHARTER_SUGGEST_MODES.map((m) => (
              <button
                key={m.value}
                title={m.hint}
                onClick={() => setMode(m.value)}
                className={`rounded px-1.5 py-0.5 text-[10px] ${
                  mode === m.value
                    ? "bg-accent/20 text-accent"
                    : "text-fg-subtle hover:bg-bg-hover"
                }`}
              >
                {m.label}
              </button>
            ))}
            <button
              onClick={request}
              disabled={loading || !provider}
              className="ml-auto flex items-center gap-1 rounded-md bg-accent px-2 py-0.5 text-[10px] text-white hover:bg-accent/90 disabled:opacity-50"
            >
              {loading ? (
                <Loader2 size={10} className="animate-spin" />
              ) : (
                <Sparkles size={10} />
              )}
              生成建议
            </button>
          </div>
          {!provider && (
            <p className="text-[10px] text-fg-subtle">
              未配置模型 Provider——请先在「设置」中配置后使用。
            </p>
          )}
          {suggestion && (
            <div className="space-y-1.5">
              {CHARTER_FIELDS.map((f) => {
                const v = suggestion[f.key];
                if (typeof v !== "string" || dismissed.has(f.key)) return null;
                const isAdopted = adopted.has(f.key);
                return (
                  <div
                    key={f.key}
                    className="rounded border border-border p-1.5"
                  >
                    <div className="flex items-center justify-between">
                      <span className="text-[10px] font-medium text-fg-subtle">
                        {f.label}
                      </span>
                      <div className="flex gap-1">
                        <button
                          onClick={() => dismiss(f.key)}
                          disabled={isAdopted}
                          className="flex items-center gap-0.5 rounded px-1 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover disabled:opacity-50"
                        >
                          <XIcon size={9} />
                          忽略
                        </button>
                        <button
                          onClick={() => adopt(f.key)}
                          disabled={isAdopted}
                          className={`flex items-center gap-0.5 rounded px-1 py-0.5 text-[10px] disabled:opacity-70 ${
                            isAdopted
                              ? "text-success"
                              : "text-accent hover:bg-bg-hover"
                          }`}
                        >
                          <Check size={9} />
                          {isAdopted ? "已采纳" : "采纳"}
                        </button>
                      </div>
                    </div>
                    <p className="mt-1 whitespace-pre-wrap text-[11px] text-fg-muted">
                      {v}
                    </p>
                  </div>
                );
              })}
              {suggestion.questions.length > 0 && (
                <div className="rounded border border-border bg-bg-hover/30 p-1.5">
                  <div className="text-[10px] font-medium text-fg-subtle">
                    需要你补充（AI 不臆造业务事实）
                  </div>
                  <ul className="mt-0.5 list-disc pl-4 text-[11px] text-fg-muted">
                    {suggestion.questions.map((q, i) => (
                      <li key={i}>{q}</li>
                    ))}
                  </ul>
                </div>
              )}
              {adoptable.length > 0 && (
                <div className="flex items-center justify-end gap-2">
                  <button
                    onClick={dismissAll}
                    className="flex items-center gap-1 rounded-md px-2 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover"
                  >
                    <XIcon size={10} />
                    忽略全部
                  </button>
                  <button
                    onClick={adoptAll}
                    className="flex items-center gap-1 rounded-md bg-accent/10 px-2 py-0.5 text-[10px] font-medium text-accent hover:bg-accent/20"
                  >
                    <Check size={10} />
                    采纳全部（{adoptable.length}）
                  </button>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function CharterPanel({ iri }: { iri: string }) {
  const { data: charter, isLoading } = useOntologyTtlCharter(iri);
  const setMut = useSetOntologyTtlCharter();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<TtlCharter>(emptyCharter);

  function startEdit() {
    setDraft(charter ?? emptyCharter);
    setEditing(true);
  }
  function submit() {
    setMut.mutate(
      { iri, charter: draft },
      { onSuccess: () => setEditing(false) },
    );
  }

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="animate-spin text-fg-subtle" size={20} />
      </div>
    );
  }
  const c = charter ?? emptyCharter;
  const hasContent = c.business_scenario || c.business_essence;
  return (
    <div className="h-full overflow-y-auto">
      {editing ? (
        <div className="space-y-2 rounded-md border border-border bg-bg-hover/30 p-3">
          <CharterAiSuggest
            iri={iri}
            draft={draft}
            onApply={(patch) => setDraft((d) => ({ ...d, ...patch }))}
          />
          <CharterFieldEditor
            label="业务场景"
            hint="定义本体建模的使用范围以及对象"
            value={draft.business_scenario}
            onChange={(v) => setDraft({ ...draft, business_scenario: v })}
          />
          <CharterFieldEditor
            label="业务目标"
            hint="定义本体建模的使用目标"
            value={draft.business_essence}
            onChange={(v) => setDraft({ ...draft, business_essence: v })}
          />
          <div className="flex items-center justify-end gap-1 pt-1">
            <button
              onClick={() => setEditing(false)}
              disabled={setMut.isPending}
              className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover disabled:opacity-50"
            >
              <XIcon size={12} />
              取消
            </button>
            <button
              onClick={submit}
              disabled={setMut.isPending}
              className="flex items-center gap-1 rounded-md bg-accent px-2 py-1 text-xs text-white hover:bg-accent/90 disabled:opacity-50"
            >
              {setMut.isPending ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Save size={12} />
              )}
              保存
            </button>
          </div>
        </div>
      ) : (
        <div className="rounded-md border border-border bg-bg-base/60 p-3">
          <div className="mb-2 flex items-center justify-between">
            <span className="flex items-center gap-1 text-sm font-semibold text-fg">
              <Sparkles size={13} />
              设计宪章（不变点）
            </span>
            <button
              onClick={startEdit}
              className="flex items-center gap-1 rounded-md bg-accent/10 px-2 py-1 text-[11px] font-medium text-accent hover:bg-accent/20"
              title="仅用户明确要求调整时编辑"
            >
              <Pencil size={11} />
              {hasContent ? "编辑" : "补充"}
            </button>
          </div>
          {hasContent ? (
            <div className="space-y-2">
              <CharterFieldRead label="业务场景" value={c.business_scenario} />
              <CharterFieldRead label="业务目标" value={c.business_essence} />
            </div>
          ) : (
            <p className="text-[11px] text-fg-subtle">
              未定义设计宪章——冷启动建模时应补充，作为 AI 理解业务的基线。
            </p>
          )}
        </div>
      )}
    </div>
  );
}

const emptyCharter: TtlCharter = {
  business_scenario: "",
  business_essence: "",
  updated_by: "user",
  updated_at: 0,
};

function CharterFieldRead({ label, value }: { label: string; value?: string }) {
  if (!value) return null;
  return (
    <div>
      <div className="text-[10px] font-medium text-fg-subtle">{label}</div>
      <p className="whitespace-pre-wrap text-[11px] text-fg-muted">{value}</p>
    </div>
  );
}

function CharterFieldEditor({
  label,
  hint,
  value,
  onChange,
}: {
  label: string;
  hint: string;
  value?: string;
  onChange: (v: string) => void;
}) {
  return (
    <div>
      <div className="flex items-baseline justify-between">
        <label className="text-[10px] font-medium text-fg-subtle">
          {label}
        </label>
        <span className="text-[10px] text-fg-subtle">{hint}</span>
      </div>
      <textarea
        value={value ?? ""}
        onChange={(e) => onChange(e.target.value)}
        rows={2}
        className="mt-0.5 w-full resize-y rounded border border-border bg-bg-base px-1.5 py-1 text-[11px] focus:outline-none"
      />
    </div>
  );
}

// ── 历史面板 ──
function HistoryPanel({ iri }: { iri: string }) {
  const { data: logs = [], isLoading } = useOntologyTtlChangelog(iri);
  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="animate-spin text-fg-subtle" size={20} />
      </div>
    );
  }
  if (logs.length === 0) {
    return (
      <div className="flex h-full items-center justify-center text-xs text-fg-subtle">
        暂无变更记录。
      </div>
    );
  }
  return (
    <div className="h-full overflow-y-auto p-4">
      <div className="space-y-3">
        {logs.map((log) => (
          <ChangelogEntry key={log.revision} log={log} />
        ))}
      </div>
    </div>
  );
}

function ChangelogEntry({ log }: { log: TtlChangelog }) {
  const summary: {
    created?: string[];
    deleted?: string[];
    modified?: string[];
  } = (() => {
    try {
      return JSON.parse(log.change_summary);
    } catch {
      return {};
    }
  })();
  return (
    <div className="rounded-md border border-border px-3 py-2.5">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="shrink-0 rounded bg-bg-hover px-1.5 py-0.5 font-mono text-[10px] text-fg-subtle">
            #{log.revision}
          </span>
          <span className="truncate text-xs font-medium">{log.title}</span>
        </div>
        <span className="shrink-0 text-[10px] text-fg-subtle">
          {new Date(log.created_at).toLocaleString()}
        </span>
      </div>
      {log.body && (
        <p className="mt-1.5 whitespace-pre-wrap text-[11px] text-fg-muted">
          {log.body}
        </p>
      )}
      <div className="mt-2 flex flex-wrap items-center gap-1.5 text-[10px]">
        {summary.created && summary.created.length > 0 && (
          <span className="rounded bg-emerald-500/10 px-1.5 py-0.5 font-medium text-emerald-600 dark:text-emerald-400">
            + {summary.created.length} created
          </span>
        )}
        {summary.modified && summary.modified.length > 0 && (
          <span className="rounded bg-warning-bg px-1.5 py-0.5 font-medium text-warning-text">
            ~ {summary.modified.length} modified
          </span>
        )}
        {summary.deleted && summary.deleted.length > 0 && (
          <span className="rounded bg-danger-bg px-1.5 py-0.5 font-medium text-danger-text">
            − {summary.deleted.length} deleted
          </span>
        )}
      </div>
      <div className="mt-2 flex items-center gap-2 text-[10px] text-fg-subtle">
        <span className="rounded bg-bg-hover px-1 py-0.5">{log.author}</span>
        {log.conversation_id && (
          <span title="来源会话">会话 {log.conversation_id.slice(0, 8)}</span>
        )}
      </div>
    </div>
  );
}

// ── 导入弹层（粘贴 / 选择 .ttl 文件 → validate → import）──
function TtlImportDialog({
  onClose,
  onImported,
}: {
  onClose: () => void;
  onImported: (iri: string) => void;
}) {
  const [text, setText] = useState("");
  const [validation, setValidation] = useState<TtlValidation | null>(null);
  const validateMut = useValidateOntologyTtl();
  const importMut = useImportOntologyTtl();

  async function handleValidate() {
    if (!text.trim()) return;
    try {
      const r = await validateMut.mutateAsync(text);
      setValidation(r);
    } catch (e) {
      setValidation({
        triple_count: 0,
        errors: [e instanceof Error ? e.message : String(e)],
        warnings: [],
      });
    }
  }

  async function handleImport(overwrite: boolean) {
    if (!validation || validation.errors.length > 0) return;
    try {
      const r = await importMut.mutateAsync({ ttl: text, overwrite });
      if (r.errors.length > 0) {
        await message(`导入完成但有错误：\n${r.errors.join("\n")}`, {
          kind: "warning",
        });
        return;
      }
      onImported(r.ontology_iri);
    } catch (e) {
      await message(`导入失败：${e instanceof Error ? e.message : String(e)}`, {
        kind: "error",
      });
    }
  }

  async function handleOpenFile() {
    const selected = await open({
      multiple: false,
      filters: [{ name: "Turtle", extensions: ["ttl", "turtle"] }],
    });
    if (selected && typeof selected === "string") {
      try {
        const { readTextFile } = await import("@tauri-apps/plugin-fs");
        const content = await readTextFile(selected);
        setText(content);
        setValidation(null);
      } catch (e) {
        await message(
          `读取文件失败：${e instanceof Error ? e.message : String(e)}`,
          { kind: "error" },
        );
      }
    }
  }

  const canImport = validation && validation.errors.length === 0;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div className="flex max-h-[80vh] w-full max-w-2xl flex-col rounded-lg border border-border bg-bg-elevated shadow-xl">
        <div className="flex items-center justify-between border-b border-border px-4 py-2">
          <div className="flex items-center gap-2">
            <FileUp size={14} className="text-accent" />
            <span className="text-sm font-semibold">导入 W3C Turtle 本体</span>
          </div>
          <button
            onClick={onClose}
            className="rounded p-1 text-fg-subtle hover:bg-bg-hover"
          >
            <X size={16} />
          </button>
        </div>
        <div className="flex min-h-0 flex-1 flex-col p-4">
          <div className="mb-2 flex items-center gap-2">
            <button
              onClick={handleOpenFile}
              className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover"
            >
              <FileUp size={12} />
              选择 .ttl 文件
            </button>
            <span className="text-[10px] text-fg-subtle">
              或直接粘贴 Turtle 文本
            </span>
          </div>
          <textarea
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setValidation(null);
            }}
            rows={10}
            placeholder="@prefix owl: <http://www.w3.org/2002/07/owl#> .&#10;..."
            className="min-h-[160px] flex-1 resize-y overflow-y-auto rounded border border-border bg-bg-base px-2 py-1 font-mono text-[11px] focus:outline-none"
          />
          <div className="mt-2 flex items-center gap-2">
            <button
              onClick={handleValidate}
              disabled={!text.trim() || validateMut.isPending}
              className="flex items-center gap-1 rounded-md border border-border px-3 py-1 text-xs hover:bg-bg-hover disabled:opacity-50"
            >
              {validateMut.isPending ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <CheckCircle2 size={12} />
              )}
              校验
            </button>
            <button
              onClick={() => handleImport(false)}
              disabled={!canImport || importMut.isPending}
              className="flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-white hover:bg-accent/90 disabled:opacity-50"
            >
              {importMut.isPending ? (
                <Loader2 size={12} className="animate-spin" />
              ) : (
                <Plus size={12} />
              )}
              导入（新增）
            </button>
            <button
              onClick={() => handleImport(true)}
              disabled={!canImport || importMut.isPending}
              className="flex items-center gap-1 rounded-md border border-warning-border px-3 py-1 text-xs text-amber-600 hover:bg-warning-bg disabled:opacity-50 dark:text-amber-400"
            >
              导入（覆盖）
            </button>
          </div>
          {/* 校验结果可能很长（如证据闸门逐元素 S1 错误）：限高内滚动，防止撑破对话框 */}
          {validation && (
            <div className="mt-3 max-h-[38vh] shrink-0 space-y-1.5 overflow-y-auto text-xs">
              <div className="text-fg-subtle">
                {validation.triple_count} 三元组
              </div>
              {validation.errors.length > 0 && (
                <div className="rounded border border-danger-border bg-danger-bg p-2">
                  <div className="mb-1 flex items-center gap-1 font-medium text-danger-text">
                    <XCircle size={12} />
                    {validation.errors.length} 个错误（禁止导入）
                  </div>
                  <ul className="space-y-0.5 text-[10px] text-danger-text">
                    {validation.errors.map((e, i) => (
                      <li key={i}>• {e}</li>
                    ))}
                  </ul>
                </div>
              )}
              {validation.warnings.length > 0 && (
                <div className="rounded border border-warning-border bg-warning-bg p-2">
                  <div className="mb-1 flex items-center gap-1 font-medium text-warning-text">
                    <AlertTriangle size={12} />
                    {validation.warnings.length} 个警告
                  </div>
                  <ul className="space-y-0.5 text-[10px] text-warning-text">
                    {validation.warnings.map((w, i) => (
                      <li key={i}>• {w}</li>
                    ))}
                  </ul>
                </div>
              )}
              {validation.errors.length === 0 && (
                <div className="flex items-center gap-1 text-emerald-600 dark:text-emerald-400">
                  <CheckCircle2 size={12} />
                  校验通过，可导入
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ════════════════════════════════════════════════════════════════
// 内部：SPARQL 查询 hook + IRI 工具
// ════════════════════════════════════════════════════════════════

/** SPARQL 查询（只读，复用 useSparqlQueryRead——带 TanStack Query 缓存去重）。
 *  展示场景全是 SELECT，走 useQuery 而非 mutation：相同 (iri, sparql) 自动去重，
 *  StrictMode 双跑只发一次 IPC，避免 11 条规则 × 3 查询 × 2 双跑 = 66 个并发解析同本体卡死。 */
function useSparqlQuery(iri: string, sparql: string) {
  const q = useSparqlQueryRead(iri, sparql);
  return {
    data: q.data ?? null,
    error: q.error,
    isPending: q.isPending,
  };
}

/** 从完整 IRI 提取短名（# 或 / 后最后一段）。 */
function shortIri(iri: string): string {
  const s = iri.startsWith("<") && iri.endsWith(">") ? iri.slice(1, -1) : iri;
  const parts = s.split(/[#/]/);
  const local = parts.length > 1 ? parts[parts.length - 1] : "";
  return local || s;
}

/** 从 SPARQL 查询返回的 term 提取短名。
 *  term 可能是 <iri>（带尖括号）或裸 IRI，也可能不可解析。 */
/** 从 labelMap 查中文显示名，查不到回退到 IRI 短名。 */
function labelOf(
  labelMap: Map<string, string> | undefined,
  iri: string,
): string {
  const s = iri.startsWith("<") && iri.endsWith(">") ? iri.slice(1, -1) : iri;
  return labelMap?.get(s) ?? termToShort(iri);
}

function termToShort(term: string): string {
  if (!term) return "";
  const s =
    term.startsWith("<") && term.endsWith(">") ? term.slice(1, -1) : term;
  const parts = s.split(/[#/]/);
  const local = parts.length > 1 ? parts[parts.length - 1] : "";
  return local || s;
}
