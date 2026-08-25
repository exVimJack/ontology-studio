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
//   - 核心概念图谱 → 力导向图(@xyflow/react)，节点=类，边=ObjectProperty
//   - 类层级树 → 缩进树，rdfs:subClassOf
//   - 规则逻辑树 → SWRL swrl:Imp 的"如果-那么"
//   - SPARQL 控制台 → 第 7 类 CRUD
//   - 宪章(+1) / 历史 → 复用 Palantir 的 CharterPanel/HistoryTab 设计

import { useMemo, useState } from "react";
import { confirm, message, open } from "@tauri-apps/plugin-dialog";
import {
    Network,
    Plus,
    X,
    Loader2,
    RefreshCw,
    Table2,
    GitBranch,
    Terminal,
    FileCode2,
    Upload,
    Download,
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
} from "lucide-react";
import {
    ReactFlow,
    Background,
    Controls,
    Handle,
    Position as FlowPosition,
    type Node,
    type Edge,
    type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import dagre from "@dagrejs/dagre";
import { useIsMobile } from "@/hooks/useIsMobile";
import { saveTextFile } from "@/lib/save-file";
import {
    useOntologyTtls,
    useOntologyTtlContent,
    useValidateOntologyTtl,
    useImportOntologyTtl,
    useDeleteOntologyTtl,
    useQueryOntologySparql,
    useSparqlQueryRead,
    useTtlLabelMap,
    useOntologyTtlCharter,
    useSetOntologyTtlCharter,
    useOntologyTtlChangelog,
    useOntologyTtlChangedListener,
} from "@/hooks/useOntologyTtl";
import type {
    TtlOntologySummary,
    TtlValidation,
    TtlCharter,
    TtlChangelog,
} from "@/lib/domain";

// ── SPARQL 查询模板（预置，用户可改）──
const SPARQL_TEMPLATES: { label: string; sparql: string }[] = [
    {
        label: "所有类",
        sparql: "SELECT ?c WHERE { ?c a owl:Class . } ORDER BY ?c",
    },
    {
        label: "类层级",
        sparql: "SELECT ?sub ?sup WHERE { ?sub rdfs:subClassOf ?sup . } ORDER BY ?sup ?sub",
    },
    {
        label: "对象属性",
        sparql: "SELECT ?p ?domain ?range WHERE { ?p a owl:ObjectProperty . OPTIONAL { ?p rdfs:domain ?domain } OPTIONAL { ?p rdfs:range ?range } }",
    },
    {
        label: "数据属性",
        sparql: "SELECT ?p ?domain ?range WHERE { ?p a owl:DatatypeProperty . OPTIONAL { ?p rdfs:domain ?domain } OPTIONAL { ?p rdfs:range ?range } }",
    },
    {
        label: "SWRL 规则",
        sparql: "SELECT ?rule WHERE { ?rule a swrl:Imp . }",
    },
];

export function OntologyTtlView() {
    const isMobile = useIsMobile();
    const { data: ontologies = [], isLoading, refetch } = useOntologyTtls();
    useOntologyTtlChangedListener();
    const deleteMut = useDeleteOntologyTtl();
    const [selectedIri, setSelectedIri] = useState<string | null>(null);
    const [showImport, setShowImport] = useState(false);

    async function handleDelete(iri: string) {
        const ok = await confirm(
            `确定删除本体「${shortIri(iri)}」？\n\n该操作不可撤销：Turtle 文本、设计宪章、变更历史将被全部清除。`,
            { kind: "warning" },
        );
        if (!ok) return;
        deleteMut.mutate(iri, {
            onError: async () =>
                message("删除失败，请重试。", { kind: "error" }),
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

    return (
        <div className="flex h-full flex-col">
            {/* 顶栏（无独立关闭/标题——外层 OntologyView 已有 Tab 栏）*/}
            <div className="flex items-center justify-between border-b border-border px-4 py-2">
                <div className="flex items-center gap-2">
                    <Network size={16} className="text-accent" />
                    <span className="text-xs text-fg-subtle">
                        W3C Turtle · ontology_ttl 表
                    </span>
                </div>
                <div className="flex items-center gap-1">
                    <button
                        onClick={() => refetch()}
                        className="rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover"
                        title="刷新"
                    >
                        <RefreshCw size={14} />
                    </button>
                    <button
                        onClick={() => setShowImport(true)}
                        className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-accent hover:bg-bg-hover"
                        title="导入 Turtle 文件"
                    >
                        <Upload size={12} /> 导入
                    </button>
                </div>
            </div>

            <div className="flex min-h-0 flex-1">
                {/* 左栏：本体列表 */}
                <div
                    className={`flex flex-col border-r border-border ${
                        isMobile
                            ? mobileShowDetail
                                ? "hidden"
                                : "w-full"
                            : "w-72"
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
                                            onClick={() => setShowImport(true)}
                                            className="mt-2 text-accent hover:underline"
                                        >
                                            导入第一个 .ttl
                                        </button>
                                    </div>
                                ) : (
                                    ontologies.map((o) => (
                                        <TtlListItem
                                            key={o.ontology_iri}
                                            summary={o}
                                            active={
                                                o.ontology_iri === selectedIri
                                            }
                                            deleting={
                                                deleteMut.isPending &&
                                                deleteMut.variables ===
                                                    o.ontology_iri
                                            }
                                            onClick={() =>
                                                setSelectedIri(o.ontology_iri)
                                            }
                                            onDelete={() =>
                                                handleDelete(o.ontology_iri)
                                            }
                                        />
                                    ))
                                )}
                            </div>
                        </>
                    )}
                </div>

                {/* 中栏：详情 */}
                <div
                    className={`flex min-w-0 flex-1 flex-col ${
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
    );
}

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
                    <span className="truncate">
                        {summary.version || "无版本"}
                    </span>
                    <span>· {summary.triple_count} 三元组</span>
                </div>
            </button>
            <button
                onClick={(e) => {
                    e.stopPropagation();
                    onDelete();
                }}
                disabled={deleting}
                className="absolute right-1 top-1 rounded p-1 text-fg-subtle opacity-0 transition hover:bg-red-500/10 hover:text-red-500 group-hover:opacity-100 disabled:opacity-50"
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
    | "source"
    | "sparql"
    | "charter"
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
    const [tab, setTab] = useState<DetailTab>("overview");

    async function handleExport() {
        if (!content) return;
        try {
            const saved = await saveTextFile(content, `${shortIri(iri)}.ttl`, [
                { name: "Turtle", extensions: ["ttl"] },
            ]);
            if (saved) {
                // 导出成功，暂略 toast
            }
        } catch (e) {
            await message(
                `导出失败：${e instanceof Error ? e.message : String(e)}`,
                { kind: "error" },
            );
        }
    }

    const tabs: { key: DetailTab; label: string; icon: typeof Table2 }[] = [
        { key: "overview", label: "概览", icon: Table2 },
        { key: "graph", label: "概念图谱", icon: Network },
        { key: "hierarchy", label: "类层级", icon: GitBranch },
        { key: "rules", label: "规则", icon: Workflow },
        { key: "source", label: "Turtle", icon: FileCode2 },
        { key: "sparql", label: "SPARQL", icon: Terminal },
        { key: "history", label: "历史", icon: GitBranch },
    ];

    return (
        <div className="flex h-full flex-col overflow-hidden">
            {/* 头部 */}
            <div className="shrink-0 border-b border-border px-4 py-3">
                <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                        <h2 className="text-base font-semibold">
                            {shortIri(iri)}
                        </h2>
                        <div className="mt-0.5 flex items-center gap-3 text-xs text-fg-subtle">
                            <span title={iri}>{iri}</span>
                        </div>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                        <button
                            onClick={handleExport}
                            className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover hover:text-accent"
                            title="导出为 .ttl 文件"
                        >
                            <Download size={12} />
                            导出
                        </button>
                        <button
                            onClick={onDelete}
                            disabled={deleting}
                            className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-red-500/10 hover:text-red-500 disabled:opacity-50"
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
                        <span>
                            更新于 {new Date(updatedAt).toLocaleString()}
                        </span>
                    )}
                </div>
                {/* 本体设计宪章（不变点）：业务场景 / 本质 / 设计意图 / 补充说明。
                    放头部（对齐 Palantir OntologyView）——冷启动基线，用户进页即可见 */}
                <CharterPanel iri={iri} />
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

            {/* Tab 内容 */}
            <div className="min-h-0 flex-1 overflow-hidden">
                {tab === "overview" && (
                    <OverviewPanel
                        iri={iri}
                        tripleCount={tripleCount}
                        content={content}
                        onJump={setTab}
                    />
                )}
                {tab === "graph" && <GraphPanel iri={iri} />}
                {tab === "hierarchy" && <HierarchyPanel iri={iri} />}
                {tab === "rules" && <RulesPanel iri={iri} />}
                {tab === "source" && <SourcePanel content={content} />}
                {tab === "sparql" && <SparqlPanel iri={iri} />}
                {tab === "history" && <HistoryPanel iri={iri} />}
            </div>
        </div>
    );
}

// ── 概览面板 ──
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
            <div className="mt-4 rounded-md border border-dashed border-border p-3 text-xs text-fg-subtle">
                <p className="font-medium text-fg-muted">本体 IRI</p>
                <p className="mt-1 break-all">{iri}</p>
            </div>
        </div>
    );
}

// ── 核心概念图谱（力导向图，@xyflow/react）──
// 节点：owl:Class（所有类）。
// 边：rdfs:subClassOf（层级，实线）+ ObjectProperty 的 domain→range（关系，虚线带标签）。
// 即使本体无 ObjectProperty，也能基于 subClassOf 画出类层级关系图。
// ── 核心概念图谱（力导向图，@xyflow/react + dagre 布局，VOWL 风格自定义节点）──
// 节点：owl:Class（圆形暖色底，对齐 VOWL 规范）。
// 边：rdfs:subClassOf（实线箭头）+ ObjectProperty 的 domain→range（虚线带标签）。
// 布局：dagre 层级布局（自动避免节点重叠 + 边交叉最小化）。
function GraphPanel({ iri }: { iri: string }) {
    // 查询所有类 + subClassOf + ObjectProperty domain→range
    const sparql = useMemo(
        () =>
            "SELECT ?s ?p ?o ?type WHERE { " +
            '{ ?s a owl:Class . BIND("class" AS ?type) } UNION ' +
            '{ ?s rdfs:subClassOf ?o . BIND("subClassOf" AS ?p) BIND("hierarchy" AS ?type) } UNION ' +
            '{ ?p a owl:ObjectProperty . ?p rdfs:domain ?s . ?p rdfs:range ?o . BIND("objectProperty" AS ?type) } ' +
            "FILTER(isIRI(?s)) FILTER(!BOUND(?o) || isIRI(?o)) }",
        [],
    );
    const { data, isPending, error } = useSparqlQuery(iri, sparql);
    const { data: labelMap } = useTtlLabelMap(iri);

    const nodeTypes = useMemo(() => ({ owlClass: OwlClassNode }), []);

    const { nodes, edges } = useMemo(() => {
        if (!data) return { nodes: [] as Node[], edges: [] as Edge[] };
        const nodeSet = new Map<string, { label: string; iri: string }>();
        const edgeList: Edge[] = [];
        // ensureNode：用 IRI local name 做 Map key（去重），显示名查 labelMap
        const ensureNode = (term: string) => {
            const key = termToShort(term);
            if (!key || key === term) return;
            if (!nodeSet.has(key)) {
                nodeSet.set(key, { label: labelOf(labelMap, term), iri: term });
            }
        };
        for (const row of data) {
            const s = row["s"] ?? "";
            const p = row["p"] ?? "";
            const o = row["o"] ?? "";
            const type = row["type"] ?? "";
            if (!s) continue;
            if (type === "class") {
                ensureNode(s);
            } else if (type === "hierarchy" && o) {
                ensureNode(s);
                ensureNode(o);
                const sub = termToShort(s);
                const sup = termToShort(o);
                if (sub !== sup) {
                    edgeList.push({
                        id: `h-${sub}-${sup}`,
                        source: sub,
                        target: sup,
                        label: undefined,
                        type: "smoothstep",
                        animated: false,
                        style: { stroke: "#94a3b8", strokeWidth: 1.5 },
                        markerEnd: {
                            type: "arrowclosed" as const,
                            color: "#94a3b8",
                        },
                    });
                }
            } else if (type === "objectProperty" && p && o) {
                ensureNode(s);
                ensureNode(o);
                const pKey = termToShort(p);
                const dom = termToShort(s);
                const rng = termToShort(o);
                if (dom !== rng) {
                    edgeList.push({
                        id: `o-${pKey}-${dom}-${rng}`,
                        source: dom,
                        target: rng,
                        label: labelOf(labelMap, p),
                        labelStyle: { fontSize: 10, fill: "#10b981" },
                        labelBgStyle: { fill: "rgba(255,255,255,0.85)" },
                        type: "smoothstep",
                        animated: false,
                        style: {
                            stroke: "#10b981",
                            strokeWidth: 1.5,
                            strokeDasharray: "5 3",
                        },
                        markerEnd: {
                            type: "arrowclosed" as const,
                            color: "#10b981",
                        },
                    });
                }
            }
        }
        // dagre 自动布局
        const g = new dagre.graphlib.Graph();
        g.setDefaultEdgeLabel(() => ({}));
        g.setGraph({
            rankdir: "TB",
            nodesep: 60,
            ranksep: 80,
            marginx: 40,
            marginy: 40,
        });
        const NODE_W = 140;
        const NODE_H = 44;
        for (const [label] of nodeSet) {
            g.setNode(label, { width: NODE_W, height: NODE_H });
        }
        for (const e of edgeList) {
            g.setEdge(e.source, e.target);
        }
        dagre.layout(g);
        const laidNodes: Node[] = Array.from(nodeSet.entries()).map(
            ([label, info]) => {
                const pos = g.node(label);
                return {
                    id: label,
                    type: "owlClass",
                    data: { label: info.label, iri: info.iri },
                    position: pos
                        ? { x: pos.x - NODE_W / 2, y: pos.y - NODE_H / 2 }
                        : { x: 0, y: 0 },
                    style: { width: NODE_W, height: NODE_H },
                };
            },
        );
        return { nodes: laidNodes, edges: edgeList };
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
                <AlertTriangle size={20} className="text-amber-500" />
                <p className="text-sm text-red-500">图谱加载失败</p>
                <p className="text-[11px] text-fg-subtle">{error.message}</p>
                <p className="mt-1 text-[10px] text-fg-subtle">
                    提示：可在「SPARQL」页手动执行 owl:Class 查询排查。
                </p>
            </div>
        );
    }
    if (nodes.length === 0) {
        return (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
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
        <div className="flex h-full w-full flex-col">
            <div className="shrink-0 border-b border-border bg-bg-hover/30 px-3 py-1.5 text-[10px] text-fg-subtle">
                <span className="inline-flex items-center gap-1">
                    <span className="inline-block h-0.5 w-4 bg-slate-400" />
                    实线 = subClassOf（类层级）
                </span>
                <span className="ml-3 inline-flex items-center gap-1">
                    <span
                        className="inline-block h-0.5 w-4 bg-emerald-500"
                        style={{ borderTop: "2px dashed #10b981", height: 0 }}
                    />
                    虚线 = ObjectProperty（对象属性 domain→range）
                </span>
                <span className="ml-3 text-fg-subtle/60">
                    · 拖拽移动节点，滚轮缩放
                </span>
            </div>
            <div className="min-h-0 flex-1">
                <ReactFlow
                    nodes={nodes}
                    edges={edges}
                    nodeTypes={nodeTypes}
                    fitView
                    nodesDraggable
                    edgesFocusable
                    proOptions={{ hideAttribution: true }}
                    defaultEdgeOptions={{ markerEnd: { type: "arrowclosed" } }}
                >
                    <Background color="#e2e8f0" gap={20} />
                    <Controls showInteractive={false} />
                </ReactFlow>
            </div>
        </div>
    );
}

/** VOWL 风格类节点：圆形/胶囊形，暖色底（#fecc57）。
 *  对齐 WebVOWL 视觉规范——owl:Class 用圆形节点 + 黄色填充。 */
function OwlClassNode({ data }: NodeProps) {
    const label = (data as { label?: string }).label ?? "";
    const iri = (data as { iri?: string }).iri ?? "";
    return (
        <div
            className="flex items-center justify-center rounded-full border-2 px-3"
            style={{
                background: "#fecc57",
                borderColor: "#e0a800",
                width: "auto",
                minWidth: 80,
                height: 36,
                boxShadow: "0 1px 3px rgba(0,0,0,0.1)",
            }}
            title={iri}
        >
            <Handle
                type="target"
                position={FlowPosition.Top}
                style={{ opacity: 0 }}
            />
            <span
                className="truncate text-[11px] font-semibold text-slate-800"
                style={{ maxWidth: 120 }}
            >
                {label}
            </span>
            <Handle
                type="source"
                position={FlowPosition.Bottom}
                style={{ opacity: 0 }}
            />
            <Handle
                type="target"
                position={FlowPosition.Left}
                style={{ opacity: 0 }}
            />
            <Handle
                type="source"
                position={FlowPosition.Right}
                style={{ opacity: 0 }}
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
            ...[...allClasses].filter(
                (c) => !parents.has(c) && !allChildren.has(c),
            ),
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
                <AlertTriangle size={20} className="text-amber-500" />
                <p className="text-sm text-red-500">类层级加载失败</p>
                <p className="text-[11px] text-fg-subtle">{error.message}</p>
            </div>
        );
    }
    if (tree.length === 0) {
        return (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-center">
                <GitBranch size={20} className="text-fg-subtle" />
                <p className="text-sm text-fg-subtle">
                    本体无 owl:Class 定义。
                </p>
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
                        selected === node.label
                            ? "text-accent"
                            : "text-accent/70"
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
                    {labelOf(labelMap, classIri)}
                </h3>
            </div>
            <DetailSection title="父类（subClassOf）" items={supers} />
            <DetailSection title="子类" items={subs} />
            <DetailSection
                title="关联属性（domain/range）"
                items={props.map((p) =>
                    p.role === "domain"
                        ? `${p.p} → ${p.other}`
                        : `${p.p} ← ${p.other}`,
                )}
            />
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
                        <li
                            key={i}
                            className="font-mono text-[11px] text-fg-muted"
                        >
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
                <AlertTriangle size={20} className="text-amber-500" />
                <p className="text-sm text-red-500">规则加载失败</p>
                <p className="text-[11px] text-fg-subtle">
                    {rulesQ.error.message}
                </p>
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
                <span className="text-xs font-semibold">
                    {labelOf(labelMap, rule)}
                </span>
                {isPending && (
                    <Loader2
                        size={10}
                        className="animate-spin text-fg-subtle"
                    />
                )}
            </div>
            {hasError ? (
                <p className="mt-2 text-[11px] text-red-500">
                    atom 解析失败：{(bodyQ.error || headQ.error)?.message}
                </p>
            ) : !isPending && !hasStructured ? (
                <div className="mt-2">
                    <p className="mb-1.5 text-[11px] text-fg-subtle">
                        规则未用标准 SWRL List 结构，原始三元组如下：
                    </p>
                    <ul className="space-y-0.5 font-mono text-[10px] text-fg-muted">
                        {fallbackRows.length === 0 ? (
                            <li className="text-fg-subtle">
                                (无可展示的三元组)
                            </li>
                        ) : (
                            fallbackRows.map((r, i) => (
                                <li key={i}>
                                    {labelOf(labelMap, r["p"] ?? "")}:{" "}
                                    {r["o"] ?? ""}
                                </li>
                            ))
                        )}
                    </ul>
                </div>
            ) : (
                <div className="mt-2 flex items-start gap-2 text-[11px]">
                    <div className="flex-1 rounded border border-amber-500/20 bg-amber-500/5 p-2">
                        <div className="mb-1 flex items-center gap-1 font-medium text-amber-600 dark:text-amber-400">
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
                    <ArrowRight
                        size={14}
                        className="mt-4 shrink-0 text-accent"
                    />
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
function SourcePanel({ content }: { content?: string }) {
    if (!content) {
        return (
            <div className="flex h-full items-center justify-center">
                <Loader2 className="animate-spin text-fg-subtle" size={20} />
            </div>
        );
    }
    const lines = content.split("\n");
    return (
        <div className="h-full overflow-auto bg-bg-base/50">
            <div className="flex min-h-full font-mono text-[11px] leading-relaxed">
                {/* 行号列 */}
                <div className="select-none border-r border-border bg-bg-hover/20 px-2 py-3 text-right text-fg-subtle">
                    {lines.map((_, i) => (
                        <div key={i}>{i + 1}</div>
                    ))}
                </div>
                {/* 代码列 */}
                <pre className="flex-1 overflow-x-auto whitespace-pre-wrap break-all px-3 py-3 text-fg-muted">
                    {content}
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
                    className="w-full resize-y rounded border border-border bg-bg-base px-2 py-1 font-mono text-[11px] focus:border-accent focus:outline-none"
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
                {err && (
                    <div className="text-xs text-red-500">查询错误：{err}</div>
                )}
                {result && result.rows.length > 0 && (
                    <table className="w-full text-[11px]">
                        <thead>
                            <tr className="border-b border-border text-left text-fg-subtle">
                                {result.vars.map((v) => (
                                    <th
                                        key={v}
                                        className="py-1 pr-3 font-medium"
                                    >
                                        {v}
                                    </th>
                                ))}
                            </tr>
                        </thead>
                        <tbody>
                            {result.rows.map((row, i) => (
                                <tr
                                    key={i}
                                    className="border-b border-border/50"
                                >
                                    {result.vars.map((v) => (
                                        <td
                                            key={v}
                                            className="py-1 pr-3 text-fg-muted"
                                        >
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
    const hasContent =
        c.business_scenario ||
        c.business_essence ||
        c.design_intent ||
        c.invariants;
    return (
        <div className="h-full overflow-y-auto p-4">
            {editing ? (
                <div className="space-y-2 rounded-md border border-border bg-bg-hover/30 p-3">
                    <CharterFieldEditor
                        label="业务场景"
                        hint="服务于什么业务目标、谁用、解决什么问题"
                        value={draft.business_scenario}
                        onChange={(v) =>
                            setDraft({ ...draft, business_scenario: v })
                        }
                    />
                    <CharterFieldEditor
                        label="业务本质"
                        hint="核心业务对象/状态/关系/动态行为的一句话本质概括"
                        value={draft.business_essence}
                        onChange={(v) =>
                            setDraft({ ...draft, business_essence: v })
                        }
                    />
                    <CharterFieldEditor
                        label="设计意图"
                        hint="为什么这样建模、够用且可扩展的取舍、可扩展方向"
                        value={draft.design_intent}
                        onChange={(v) =>
                            setDraft({ ...draft, design_intent: v })
                        }
                    />
                    <CharterFieldEditor
                        label="补充说明"
                        hint="不可违反的业务约束、边界条件等（自由文本）"
                        value={draft.invariants}
                        onChange={(v) => setDraft({ ...draft, invariants: v })}
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
                <div className="rounded-md border border-border bg-bg-hover/20 p-3">
                    <div className="mb-2 flex items-center justify-between">
                        <span className="text-xs font-medium text-fg-subtle">
                            设计宪章（不变点）
                        </span>
                        <button
                            onClick={startEdit}
                            className="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-accent"
                            title="仅用户明确要求调整时编辑"
                        >
                            <Pencil size={10} />
                            {hasContent ? "编辑" : "补充"}
                        </button>
                    </div>
                    {hasContent ? (
                        <div className="space-y-2">
                            <CharterFieldRead
                                label="业务场景"
                                value={c.business_scenario}
                            />
                            <CharterFieldRead
                                label="业务本质"
                                value={c.business_essence}
                            />
                            <CharterFieldRead
                                label="设计意图"
                                value={c.design_intent}
                            />
                            <CharterFieldRead
                                label="补充说明"
                                value={c.invariants}
                            />
                        </div>
                    ) : (
                        <p className="text-[11px] text-fg-subtle">
                            未定义设计宪章——冷启动建模时应补充，作为 AI
                            理解业务的基线。
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
    design_intent: "",
    invariants: "",
    updated_by: "user",
    updated_at: 0,
};

function CharterFieldRead({ label, value }: { label: string; value?: string }) {
    if (!value) return null;
    return (
        <div>
            <div className="text-[10px] font-medium text-fg-subtle">
                {label}
            </div>
            <p className="whitespace-pre-wrap text-[11px] text-fg-muted">
                {value}
            </p>
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
                className="mt-0.5 w-full resize-y rounded border border-border bg-bg-base px-1.5 py-1 text-[11px] focus:border-accent focus:outline-none"
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
                    <span className="truncate text-xs font-medium">
                        {log.title}
                    </span>
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
                    <span className="rounded bg-amber-500/10 px-1.5 py-0.5 font-medium text-amber-600 dark:text-amber-400">
                        ~ {summary.modified.length} modified
                    </span>
                )}
                {summary.deleted && summary.deleted.length > 0 && (
                    <span className="rounded bg-red-500/10 px-1.5 py-0.5 font-medium text-red-600 dark:text-red-400">
                        − {summary.deleted.length} deleted
                    </span>
                )}
            </div>
            <div className="mt-2 flex items-center gap-2 text-[10px] text-fg-subtle">
                <span className="rounded bg-bg-hover px-1 py-0.5">
                    {log.author}
                </span>
                {log.conversation_id && (
                    <span title="来源会话">
                        会话 {log.conversation_id.slice(0, 8)}
                    </span>
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
            await message(
                `导入失败：${e instanceof Error ? e.message : String(e)}`,
                { kind: "error" },
            );
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
                        <span className="text-sm font-semibold">
                            导入 W3C Turtle 本体
                        </span>
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
                        className="min-h-[200px] flex-1 resize-y rounded border border-border bg-bg-base px-2 py-1 font-mono text-[11px] focus:border-accent focus:outline-none"
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
                            className="flex items-center gap-1 rounded-md border border-amber-500/40 px-3 py-1 text-xs text-amber-600 hover:bg-amber-500/10 disabled:opacity-50 dark:text-amber-400"
                        >
                            导入（覆盖）
                        </button>
                    </div>
                    {validation && (
                        <div className="mt-3 space-y-1.5 text-xs">
                            <div className="text-fg-subtle">
                                {validation.triple_count} 三元组
                            </div>
                            {validation.errors.length > 0 && (
                                <div className="rounded border border-red-500/30 bg-red-500/5 p-2">
                                    <div className="mb-1 flex items-center gap-1 font-medium text-red-600 dark:text-red-400">
                                        <XCircle size={12} />
                                        {validation.errors.length}{" "}
                                        个错误（禁止导入）
                                    </div>
                                    <ul className="space-y-0.5 text-[10px] text-red-600 dark:text-red-400">
                                        {validation.errors.map((e, i) => (
                                            <li key={i}>• {e}</li>
                                        ))}
                                    </ul>
                                </div>
                            )}
                            {validation.warnings.length > 0 && (
                                <div className="rounded border border-amber-500/30 bg-amber-500/5 p-2">
                                    <div className="mb-1 flex items-center gap-1 font-medium text-amber-600 dark:text-amber-400">
                                        <AlertTriangle size={12} />
                                        {validation.warnings.length} 个警告
                                    </div>
                                    <ul className="space-y-0.5 text-[10px] text-amber-600 dark:text-amber-400">
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
