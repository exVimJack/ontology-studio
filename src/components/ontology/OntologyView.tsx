// UI 层：OntologyView.tsx（三期：本体建模工作台）
// 本体列表 + 定义详情（表格展示 ObjectType/LinkType/ActionType）+ 导入预演面板。
// 覆盖层模式（同 FederationView），Sidebar 底部入口打开。
//
// 布局：左栏本体列表 + 中栏定义详情 + 右栏导入预演。
// ER 图（react-flow）留后续——本期用表格展示，零新依赖。
//
// 工作流：选本体 → 查看定义 → [导入 JSON] → 预演 → 应用。

import { Fragment, useState } from "react";
import { confirm, message } from "@tauri-apps/plugin-dialog";
import {
    Boxes,
    Network,
    Plus,
    X,
    Loader2,
    RefreshCw,
    Table2,
    Link2,
    Zap,
    Database,
    Plug,
    ArrowLeft,
    Upload,
    Download,
    CheckCircle2,
    XCircle,
    AlertTriangle,
    ArrowRight,
    FileUp,
    FileText,
    ClipboardPaste,
    Trash2,
    Pencil,
    Save,
    X as XIcon,
} from "lucide-react";
import { useUiStore } from "@/stores/ui-store";
import { useIsMobile } from "@/hooks/useIsMobile";
import {
    useOntologies,
    useOntologyPayload,
    usePreviewImport,
    useImportOntology,
    useDeleteOntology,
    useOntologyChangedListener,
    useOntologyChangelog,
    useOntologyCharter,
    useSetOntologyCharter,
    useOntologyDatasets,
    useOntologyDataSources,
} from "@/hooks/useOntology";
import { OntologyTtlView } from "@/components/ontology/OntologyTtlView";
import { saveTextFile } from "@/lib/save-file";
import type {
    OntologyPayload,
    ObjectTypeDef,
    LinkDef,
    DatasetDef,
    DataSourceDef,
    ImportPreview,
    ImportPreviewItem,
    OntologyChangelog,
    OntologyCharter,
} from "@/lib/domain";

// ── 预演项状态图标/颜色 ──
const PREVIEW_STATUS_META: Record<
    ImportPreviewItem["status"],
    { icon: typeof CheckCircle2; color: string; label: string }
> = {
    create: { icon: Plus, color: "text-emerald-500", label: "新建" },
    skip: { icon: ArrowRight, color: "text-fg-subtle", label: "跳过" },
    overwrite: { icon: RefreshCw, color: "text-amber-500", label: "覆盖" },
    fail: { icon: XCircle, color: "text-red-500", label: "失败" },
};

// ── 顶层不再有数据集/数据源独立分段（决策 10 修订：按本体隔离，作为详情内 Tab） ──

export function OntologyView() {
    const setOntologyOpen = useUiStore((s) => s.setOntologyOpen);
    const isMobile = useIsMobile();
    const [ontologyTab, setOntologyTab] = useState<"palantir" | "w3c">(
        "palantir",
    );
    const { data: ontologies = [], isLoading, refetch } = useOntologies();
    // 会话内 agent 工具导入后自动刷新（事件由 store 层回调发出）
    useOntologyChangedListener();
    const deleteMut = useDeleteOntology();
    const [selectedApiName, setSelectedApiName] = useState<string | null>(null);
    const [showImport, setShowImport] = useState(false);

    // 删除本体（硬删，级联清子表）——二次确认 + 隐藏后端 detail
    async function handleDelete(apiName: string, displayName: string) {
        const ok = await confirm(
            `确定删除本体「${displayName}」？\n\n该操作不可撤销：对象类型、属性、关系、动作、分组等定义将被全部清除。\n仅被该本体声明的数据集与数据源会一并删除；仍被其他本体声明或引用的资产会保留。`,
            { kind: "warning" },
        );
        if (!ok) return;
        deleteMut.mutate(apiName, {
            onError: async () =>
                message("删除失败，请重试。", { kind: "error" }),
            onSuccess: async (deleted) => {
                if (!deleted)
                    await message("未找到该本体，可能已被删除。", {
                        kind: "info",
                    });
                if (selectedApiName === apiName) setSelectedApiName(null);
            },
        });
    }

    const selected =
        ontologies.find((o) => o.api_name === selectedApiName) ?? null;
    const mobileShowDetail = isMobile && selected !== null;

    return (
        <div className="fixed inset-0 z-40 flex flex-col bg-bg max-md:pt-[env(safe-area-inset-top)] max-md:pb-[env(safe-area-inset-bottom)]">
            {/* 顶栏 */}
            <div className="flex items-center justify-between border-b border-border px-4 py-2">
                <div className="flex items-center gap-1">
                    <button
                        onClick={() => setOntologyTab("palantir")}
                        className={`flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs transition ${
                            ontologyTab === "palantir"
                                ? "bg-bg-hover font-medium text-accent"
                                : "text-fg-subtle hover:bg-bg-hover/50"
                        }`}
                        title="Palantir标准本体（ObjectType/LinkType/ActionType）"
                    >
                        <Boxes size={14} />
                        Palantir标准
                    </button>
                    <button
                        onClick={() => setOntologyTab("w3c")}
                        className={`flex items-center gap-1.5 rounded-md px-2.5 py-1 text-xs transition ${
                            ontologyTab === "w3c"
                                ? "bg-bg-hover font-medium text-accent"
                                : "text-fg-subtle hover:bg-bg-hover/50"
                        }`}
                        title="W3C 标准本体（RDF/OWL/SWRL，Turtle）"
                    >
                        <Network size={14} />
                        W3C 标准
                    </button>
                </div>
                <div className="flex items-center gap-1">
                    {ontologyTab === "palantir" && (
                        <>
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
                                title="导入本体 JSON"
                            >
                                <Upload size={12} /> 导入
                            </button>
                        </>
                    )}
                    <button
                        onClick={() => setOntologyOpen(false)}
                        className="rounded-md p-1.5 text-fg-subtle hover:bg-bg-hover"
                        title="关闭"
                    >
                        <X size={16} />
                    </button>
                </div>
            </div>

            {ontologyTab === "w3c" ? (
                <OntologyTtlView />
            ) : (
                <div className="flex min-h-0 flex-1">
                    {/* 左栏：本体列表 */}
                    <div
                        className={`flex flex-col border-r border-border ${
                            isMobile
                                ? mobileShowDetail
                                    ? "hidden"
                                    : "w-full"
                                : "w-64"
                        }`}
                    >
                        {isMobile && mobileShowDetail ? null : (
                            <>
                                <div className="px-3 py-2">
                                    <span className="text-xs font-medium uppercase tracking-wide text-fg-subtle">
                                        本体
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
                                            暂无本体
                                            <br />
                                            <button
                                                onClick={() =>
                                                    setShowImport(true)
                                                }
                                                className="mt-2 text-accent hover:underline"
                                            >
                                                导入第一个本体
                                            </button>
                                        </div>
                                    ) : (
                                        ontologies.map((o) => (
                                            <OntologyListItem
                                                key={o.api_name}
                                                apiName={o.api_name}
                                                displayName={o.display_name}
                                                description={o.description}
                                                active={
                                                    o.api_name ===
                                                    selectedApiName
                                                }
                                                deleting={
                                                    deleteMut.isPending &&
                                                    deleteMut.variables ===
                                                        o.api_name
                                                }
                                                onClick={() =>
                                                    setSelectedApiName(
                                                        o.api_name,
                                                    )
                                                }
                                                onDelete={() =>
                                                    handleDelete(
                                                        o.api_name,
                                                        o.display_name,
                                                    )
                                                }
                                            />
                                        ))
                                    )}
                                </div>
                            </>
                        )}
                    </div>

                    {/* 中栏：定义详情 */}
                    <div
                        className={`flex min-w-0 flex-1 flex-col ${
                            isMobile && !mobileShowDetail ? "hidden" : "flex"
                        }`}
                    >
                        {isMobile && mobileShowDetail && (
                            <button
                                onClick={() => setSelectedApiName(null)}
                                className="flex items-center gap-1.5 border-b border-border px-3 py-2 text-xs text-fg-muted hover:bg-bg-hover"
                            >
                                <ArrowLeft size={14} />
                                返回本体列表
                            </button>
                        )}
                        {selected ? (
                            <OntologyDetail
                                apiName={selected.api_name}
                                deleting={
                                    deleteMut.isPending &&
                                    deleteMut.variables === selected.api_name
                                }
                                onDelete={() =>
                                    handleDelete(
                                        selected.api_name,
                                        selected.display_name,
                                    )
                                }
                            />
                        ) : !isMobile ? (
                            <div className="flex h-full items-center justify-center text-sm text-fg-subtle">
                                选择左侧本体查看定义
                            </div>
                        ) : null}
                    </div>
                </div>
            )}

            {/* 导入弹层 */}
            {showImport && (
                <ImportDialog
                    onClose={() => setShowImport(false)}
                    onImported={(apiName) => {
                        setSelectedApiName(apiName);
                        setShowImport(false);
                    }}
                />
            )}
        </div>
    );
}

// ── 数据集 Tab（本体详情内，按本体隔离，决策 10 修订） ──
function DatasetsTab({ apiName }: { apiName: string }) {
    const {
        data: datasets = [],
        isLoading,
        refetch,
    } = useOntologyDatasets(apiName);
    return (
        <div>
            <div className="mb-3 flex items-center justify-between">
                <div className="flex items-center gap-1.5 text-xs font-medium text-fg-muted">
                    <Database size={12} className="text-accent" />
                    数据集
                    <span className="font-normal text-fg-subtle">
                        （属于本本体，按本体隔离）
                    </span>
                </div>
                <button
                    onClick={() => refetch()}
                    className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover"
                    title="刷新"
                >
                    <RefreshCw size={13} />
                </button>
            </div>
            {isLoading ? (
                <div className="flex justify-center py-12">
                    <Loader2
                        className="animate-spin text-fg-subtle"
                        size={16}
                    />
                </div>
            ) : datasets.length === 0 ? (
                <div className="py-12 text-center text-xs text-fg-subtle">
                    本体「{apiName}」未声明任何数据集。
                </div>
            ) : (
                <div className="grid grid-cols-1 gap-2 md:grid-cols-2 lg:grid-cols-3">
                    {datasets.map((d) => (
                        <DatasetCard key={d.api_name} dataset={d} />
                    ))}
                </div>
            )}
        </div>
    );
}

// ── 数据源 Tab（本体详情内，按本体隔离，决策 10 修订） ──
function DataSourcesTab({ apiName }: { apiName: string }) {
    const {
        data: sources = [],
        isLoading,
        refetch,
    } = useOntologyDataSources(apiName);
    return (
        <div>
            <div className="mb-3 flex items-center justify-between">
                <div className="flex items-center gap-1.5 text-xs font-medium text-fg-muted">
                    <Plug size={12} className="text-accent" />
                    数据源
                    <span className="font-normal text-fg-subtle">
                        （属于本本体，按本体隔离）
                    </span>
                </div>
                <button
                    onClick={() => refetch()}
                    className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover"
                    title="刷新"
                >
                    <RefreshCw size={13} />
                </button>
            </div>
            {isLoading ? (
                <div className="flex justify-center py-12">
                    <Loader2
                        className="animate-spin text-fg-subtle"
                        size={16}
                    />
                </div>
            ) : sources.length === 0 ? (
                <div className="py-12 text-center text-xs text-fg-subtle">
                    本体「{apiName}」未声明任何数据源。
                </div>
            ) : (
                <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                    {sources.map((ds) => (
                        <DataSourceCard key={ds.api_name} source={ds} />
                    ))}
                </div>
            )}
        </div>
    );
}

// ── 本体列表项 ──
function OntologyListItem({
    apiName,
    displayName,
    description,
    active,
    deleting,
    onClick,
    onDelete,
}: {
    apiName: string;
    displayName: string;
    description?: string;
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
                    <Boxes size={12} className="shrink-0 text-accent" />
                    <span className="truncate text-xs font-medium">
                        {displayName}
                    </span>
                </div>
                <div className="mt-0.5 truncate text-[10px] text-fg-subtle">
                    {apiName}
                </div>
                {description && (
                    <div className="mt-1 line-clamp-2 text-[10px] text-fg-muted">
                        {description}
                    </div>
                )}
            </button>
            {/* 悬停浮现的删除按钮 */}
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

// ── 本体详情（中栏）：ObjectType / LinkType / ActionType 表格 ──
function OntologyDetail({
    apiName,
    deleting,
    onDelete,
}: {
    apiName: string;
    deleting: boolean;
    onDelete: () => void;
}) {
    const { data: payload, isLoading, error } = useOntologyPayload(apiName);
    const { data: charter, isLoading: charterLoading } =
        useOntologyCharter(apiName);
    const setCharterMut = useSetOntologyCharter();
    const [tab, setTab] = useState<DetailTab>("overview");

    // 导出当前本体到 JSON 文件
    async function handleExport() {
        if (!payload) return;
        try {
            const json = JSON.stringify(payload, null, 2);
            const saved = await saveTextFile(json, `${payload.api_name}.json`, [
                { name: "JSON", extensions: ["json"] },
            ]);
            if (saved) {
                // 可加 toast，暂略
            }
        } catch (e) {
            await message(
                `导出失败：${e instanceof Error ? e.message : String(e)}`,
                { kind: "error" },
            );
        }
    }

    if (isLoading) {
        return (
            <div className="flex h-full items-center justify-center">
                <Loader2 className="animate-spin text-fg-subtle" size={20} />
            </div>
        );
    }
    if (error || !payload) {
        return (
            <div className="flex h-full items-center justify-center text-sm text-red-500">
                加载失败：{error?.message ?? "未知错误"}
            </div>
        );
    }

    // Tab 分页：概览优先，实体明细按需切换
    const linkCount = payload.object_types.reduce(
        (n, ot) => n + ot.links.length,
        0,
    );
    const actionCount = payload.action_types?.length ?? 0;
    const datasetCount = payload.datasets?.length ?? 0;
    const dataSourceCount = payload.data_sources?.length ?? 0;

    // 数据集/数据源 Tab 始终展示（即使为 0，也允许用户看到「该本体无数据集」的空状态）
    const tabs: { key: DetailTab; label: string; count: number }[] = [
        { key: "overview" as const, label: "概览", count: 0 },
        {
            key: "objects" as const,
            label: "对象类型",
            count: payload.object_types.length,
        },
        { key: "links" as const, label: "关系", count: linkCount },
        { key: "actions" as const, label: "动作", count: actionCount },
        { key: "datasets" as const, label: "数据集", count: datasetCount },
        { key: "sources" as const, label: "数据源", count: dataSourceCount },
        { key: "history" as const, label: "历史", count: 0 },
    ].filter(
        (t) =>
            t.key === "overview" ||
            t.key === "history" ||
            t.key === "datasets" ||
            t.key === "sources" ||
            t.count > 0,
    );

    return (
        <div className="flex h-full flex-col overflow-hidden">
            {/* 头部信息 */}
            <div className="shrink-0 border-b border-border px-4 py-3">
                <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                        <h2 className="text-base font-semibold">
                            {payload.display_name}
                        </h2>
                        <div className="mt-0.5 text-xs text-fg-subtle">
                            {payload.api_name}
                        </div>
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                        <button
                            onClick={handleExport}
                            className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover hover:text-accent"
                            title="导出为 JSON 文件"
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
                {payload.description && (
                    <p className="mt-2 text-xs text-fg-muted">
                        {payload.description}
                    </p>
                )}
                {/* 本体设计宪章（不变点）：业务场景 / 本质 / 设计意图 / 补充说明 */}
                <CharterPanel
                    charter={charter}
                    loading={charterLoading}
                    saving={setCharterMut.isPending}
                    onSave={(c) =>
                        setCharterMut.mutate({ apiName, charter: c })
                    }
                />
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
                        {t.label}
                        {t.count > 0 && (
                            <span className="rounded bg-bg-active px-1 text-[10px] text-fg-subtle">
                                {t.count}
                            </span>
                        )}
                    </button>
                ))}
            </div>

            {/* Tab 内容（仅内容区滚动，头部与 Tab 栏固定） */}
            <div className="min-h-0 flex-1 overflow-y-auto p-4">
                {tab === "overview" && (
                    <OverviewTab payload={payload} onJump={setTab} />
                )}

                {tab === "objects" && (
                    <div className="space-y-3">
                        {payload.object_types.map((ot) => (
                            <ObjectTypeCard key={ot.api_name} ot={ot} />
                        ))}
                    </div>
                )}

                {tab === "links" && (
                    <div className="overflow-x-auto">
                        <table className="w-full text-xs">
                            <thead>
                                <tr className="border-b border-border text-left text-fg-subtle">
                                    <th className="py-1.5 pr-3 font-medium">
                                        api_name
                                    </th>
                                    <th className="py-1.5 pr-3 font-medium">
                                        源 → 目标
                                    </th>
                                    <th className="py-1.5 pr-3 font-medium">
                                        基数
                                    </th>
                                    <th className="py-1.5 font-medium">方向</th>
                                </tr>
                            </thead>
                            <tbody>
                                {payload.object_types.flatMap((ot) =>
                                    ot.links.map((link) => (
                                        <LinkRow
                                            key={`${ot.api_name}.${link.api_name}`}
                                            link={link}
                                            sourceName={ot.api_name}
                                        />
                                    )),
                                )}
                            </tbody>
                        </table>
                    </div>
                )}

                {tab === "actions" &&
                    (payload.action_types?.length ?? 0) > 0 && (
                        <div className="space-y-2">
                            {(payload.action_types ?? []).map((at) => (
                                <div
                                    key={at.api_name}
                                    className="rounded-md border border-border px-3 py-2"
                                >
                                    <div className="flex items-center justify-between">
                                        <span className="text-xs font-medium">
                                            {at.display_name}
                                        </span>
                                        <div className="flex items-center gap-2 text-[10px] text-fg-subtle">
                                            <span className="rounded bg-bg-hover px-1.5 py-0.5">
                                                {at.risk_level ?? "medium"}
                                            </span>
                                            <span className="rounded bg-bg-hover px-1.5 py-0.5">
                                                {at.operation_kind ?? "mixed"}
                                            </span>
                                        </div>
                                    </div>
                                    <div className="mt-0.5 text-[10px] text-fg-subtle">
                                        {at.api_name}
                                    </div>
                                    {at.description && (
                                        <p className="mt-1 text-[10px] text-fg-muted">
                                            {at.description}
                                        </p>
                                    )}
                                    <div className="mt-1 text-[10px] text-fg-subtle">
                                        影响对象：
                                        {at.affected_object_type_api_name}
                                        {at.batch_enabled && " · 批量"}
                                    </div>
                                </div>
                            ))}
                        </div>
                    )}

                {tab === "datasets" && (
                    <DatasetsTab apiName={payload.api_name} />
                )}

                {tab === "sources" && (
                    <DataSourcesTab apiName={payload.api_name} />
                )}

                {tab === "history" && <HistoryTab apiName={payload.api_name} />}
            </div>
        </div>
    );
}

// ── 详情 Tab 类型（决策 10 修订：数据集/数据源作为详情内 Tab，按本体隔离） ──
type DetailTab =
    | "overview"
    | "objects"
    | "links"
    | "actions"
    | "datasets"
    | "sources"
    | "history";

// ── 概览 Tab：统计卡 + 紧凑瓦片网格 ──
function OverviewTab({
    payload,
    onJump,
}: {
    payload: OntologyPayload;
    onJump: (tab: DetailTab) => void;
}) {
    const links = payload.object_types.flatMap((ot) => ot.links);
    const linkCount = links.length;
    const actionCount = payload.action_types?.length ?? 0;
    const datasetCount = payload.datasets?.length ?? 0;
    const dataSourceCount = payload.data_sources?.length ?? 0;

    const stats: {
        key: DetailTab;
        label: string;
        count: number;
        icon: React.ReactNode;
    }[] = [
        {
            key: "objects" as const,
            label: "对象类型",
            count: payload.object_types.length,
            icon: <Table2 size={14} />,
        },
        {
            key: "links" as const,
            label: "关系",
            count: linkCount,
            icon: <Link2 size={14} />,
        },
        {
            key: "actions" as const,
            label: "动作",
            count: actionCount,
            icon: <Zap size={14} />,
        },
        {
            key: "datasets" as const,
            label: "数据集",
            count: datasetCount,
            icon: <Database size={14} />,
        },
        {
            key: "sources" as const,
            label: "数据源",
            count: dataSourceCount,
            icon: <Plug size={14} />,
        },
    ].filter((s) => s.key === "datasets" || s.key === "sources" || s.count > 0);

    return (
        <div className="space-y-5">
            {/* 统计卡（点击跳转对应 Tab） */}
            {stats.length > 0 && (
                <div className="grid grid-cols-3 gap-2 sm:grid-cols-5">
                    {stats.map((s) => (
                        <button
                            key={s.key}
                            onClick={() => onJump(s.key)}
                            className="flex flex-col items-start gap-1 rounded-lg border border-border px-3 py-2.5 text-left transition hover:border-accent/40 hover:bg-bg-hover/50"
                        >
                            <div className="flex items-center gap-1.5 text-fg-subtle">
                                {s.icon}
                                <span className="text-[10px]">{s.label}</span>
                            </div>
                            <span className="text-xl font-semibold tabular-nums">
                                {s.count}
                            </span>
                        </button>
                    ))}
                </div>
            )}

            {/* 对象类型瓦片网格 */}
            {payload.object_types.length > 0 && (
                <div>
                    <div className="mb-2 flex items-center gap-1.5 text-xs font-medium text-fg-muted">
                        <Table2 size={12} className="text-accent" />
                        对象类型
                    </div>
                    <div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3">
                        {payload.object_types.map((ot) => (
                            <ObjectTypeTile key={ot.api_name} ot={ot} />
                        ))}
                    </div>
                </div>
            )}
        </div>
    );
}

// ── 历史 Tab：git commit log 式变更记录 ──
function HistoryTab({ apiName }: { apiName: string }) {
    const { data: logs = [], isLoading } = useOntologyChangelog(apiName);

    if (isLoading) {
        return (
            <div className="flex items-center gap-2 text-xs text-fg-subtle">
                <Loader2 size={14} className="animate-spin" />
                加载历史…
            </div>
        );
    }
    if (logs.length === 0) {
        return (
            <div className="text-xs text-fg-subtle">
                暂无变更记录。会话内修改本体后，设计说明会记录在这里。
            </div>
        );
    }
    return (
        <div className="space-y-3">
            {logs.map((log) => (
                <ChangelogEntry key={log.revision} log={log} />
            ))}
        </div>
    );
}

// ── 单条变更记录卡 ──
// ── 本体设计宪章（不变点）面板 ──
// 头部常驻展示业务场景/本质/设计意图/补充说明，支持编辑切换。
// charter 不随历史变化——编辑入口仅为「用户明确要求调整不变点」时使用。
function CharterPanel({
    charter,
    loading,
    saving,
    onSave,
}: {
    charter: OntologyCharter | undefined;
    loading: boolean;
    saving: boolean;
    onSave: (c: OntologyCharter) => void;
}) {
    const [editing, setEditing] = useState(false);
    // 编辑态本地草稿——进入编辑时用现有 charter 初始化（无 charter 时空串）
    const [draft, setDraft] = useState<OntologyCharter>(emptyCharter);

    function startEdit() {
        setDraft(charter ?? emptyCharter);
        setEditing(true);
    }
    function cancelEdit() {
        setEditing(false);
    }
    function submit() {
        onSave(draft);
        setEditing(false);
    }

    if (loading) {
        return (
            <div className="mt-2 flex items-center gap-1 text-[10px] text-fg-subtle">
                <Loader2 size={10} className="animate-spin" />
                加载宪章…
            </div>
        );
    }

    const c = charter ?? emptyCharter;
    const hasContent =
        c.business_scenario ||
        c.business_essence ||
        c.design_intent ||
        c.invariants;

    // ── 编辑态 ──
    if (editing) {
        return (
            <div className="mt-2 space-y-2 rounded-md border border-border bg-bg-hover/30 p-2.5">
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
                    onChange={(v) => setDraft({ ...draft, design_intent: v })}
                />
                <CharterFieldEditor
                    label="补充说明"
                    hint="不可违反的业务约束、边界条件等（自由文本）"
                    value={draft.invariants}
                    onChange={(v) => setDraft({ ...draft, invariants: v })}
                />
                <div className="flex items-center justify-between pt-1">
                    <span className="text-[10px] text-fg-subtle">
                        ⚠ 只有用户明确要求调整不变点时才编辑——
                        常规增量更新不应改动宪章
                    </span>
                    <div className="flex items-center gap-1">
                        <button
                            onClick={cancelEdit}
                            disabled={saving}
                            className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-bg-hover disabled:opacity-50"
                        >
                            <XIcon size={12} />
                            取消
                        </button>
                        <button
                            onClick={submit}
                            disabled={saving}
                            className="flex items-center gap-1 rounded-md bg-accent px-2 py-1 text-xs text-white hover:bg-accent/90 disabled:opacity-50"
                        >
                            {saving ? (
                                <Loader2 size={12} className="animate-spin" />
                            ) : (
                                <Save size={12} />
                            )}
                            保存
                        </button>
                    </div>
                </div>
            </div>
        );
    }

    // ── 只读态：无内容时提示 + 引导编辑按钮；有内容时展示四段 ──
    if (!hasContent) {
        return (
            <div className="mt-2 flex items-center justify-between rounded-md border border-dashed border-border px-2.5 py-1.5">
                <span className="text-[10px] text-fg-subtle">
                    未定义设计宪章（业务场景/本质/意图）——
                    冷启动建模时应补充，作为 AI 理解业务的基线
                </span>
                <button
                    onClick={startEdit}
                    className="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-accent"
                >
                    <Pencil size={10} />
                    补充
                </button>
            </div>
        );
    }
    return (
        <div className="mt-2 rounded-md border border-border bg-bg-hover/20 p-2.5">
            <div className="flex items-center justify-between">
                <span className="text-[10px] font-medium text-fg-subtle">
                    设计宪章（不变点）
                </span>
                <button
                    onClick={startEdit}
                    className="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-accent"
                    title="仅用户明确要求调整时编辑"
                >
                    <Pencil size={10} />
                    编辑
                </button>
            </div>
            <div className="mt-1.5 space-y-1.5">
                <CharterFieldRead
                    label="业务场景"
                    value={c.business_scenario}
                />
                <CharterFieldRead label="业务本质" value={c.business_essence} />
                <CharterFieldRead label="设计意图" value={c.design_intent} />
                <CharterFieldRead label="补充说明" value={c.invariants} />
            </div>
        </div>
    );
}

const emptyCharter: OntologyCharter = {
    business_scenario: "",
    business_essence: "",
    design_intent: "",
    invariants: "",
    updated_by: "user",
    updated_at: 0,
};

function CharterFieldRead({
    label,
    value,
}: {
    label: string;
    value: string | undefined;
}) {
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
    value: string | undefined;
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

function ChangelogEntry({ log }: { log: OntologyChangelog }) {
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
    const date = new Date(log.created_at);
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
                    {date.toLocaleString()}
                </span>
            </div>
            {log.body && (
                <p className="mt-1.5 whitespace-pre-wrap text-[11px] text-fg-muted">
                    {log.body}
                </p>
            )}
            {/* 实体级 +/−/~ 摘要徽标 */}
            {(summary.created?.length ||
                summary.deleted?.length ||
                summary.modified?.length) && (
                <div className="mt-2 flex flex-wrap items-center gap-1.5 text-[10px]">
                    {summary.created && summary.created.length > 0 && (
                        <SummaryBadge kind="created" items={summary.created} />
                    )}
                    {summary.modified && summary.modified.length > 0 && (
                        <SummaryBadge
                            kind="modified"
                            items={summary.modified}
                        />
                    )}
                    {summary.deleted && summary.deleted.length > 0 && (
                        <SummaryBadge kind="deleted" items={summary.deleted} />
                    )}
                </div>
            )}
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

// ── 变更摘要徽标 ──
function SummaryBadge({
    kind,
    items,
}: {
    kind: "created" | "deleted" | "modified";
    items: string[];
}) {
    const meta = {
        created: {
            label: "+",
            color: "text-emerald-600 dark:text-emerald-400",
            bg: "bg-emerald-500/10",
        },
        modified: {
            label: "~",
            color: "text-amber-600 dark:text-amber-400",
            bg: "bg-amber-500/10",
        },
        deleted: {
            label: "−",
            color: "text-red-600 dark:text-red-400",
            bg: "bg-red-500/10",
        },
    }[kind];
    return (
        <span
            className={`rounded ${meta.bg} ${meta.color} px-1.5 py-0.5 font-medium`}
            title={`${kind}: ${items.join(", ")}`}
        >
            {meta.label} {items.length} {kind}
        </span>
    );
}

// ── 对象类型紧凑瓦片（概览用，不含属性表） ──
function ObjectTypeTile({ ot }: { ot: ObjectTypeDef }) {
    return (
        <div className="rounded-md border border-border px-3 py-2">
            <div className="flex items-center justify-between gap-2">
                <span className="truncate text-xs font-medium">
                    {ot.display_name}
                </span>
                <span className="shrink-0 rounded bg-bg-hover px-1.5 py-0.5 text-[10px] text-fg-subtle">
                    {ot.storage_type}
                </span>
            </div>
            <div className="mt-0.5 truncate text-[10px] text-fg-subtle">
                {ot.api_name}
            </div>
            <div className="mt-1.5 flex items-center gap-3 text-[10px] text-fg-subtle">
                <span>{ot.properties.length} 属性</span>
                {ot.links.length > 0 && <span>{ot.links.length} 关系</span>}
            </div>
        </div>
    );
}

// ── ObjectType 卡片（属性表 + 内嵌链接） ──
function ObjectTypeCard({ ot }: { ot: ObjectTypeDef }) {
    return (
        <div className="rounded-md border border-border">
            {/* 头部 */}
            <div className="border-b border-border px-3 py-2">
                <div className="flex items-center justify-between">
                    <span className="text-xs font-semibold">
                        {ot.display_name}
                    </span>
                    <span className="rounded bg-bg-hover px-1.5 py-0.5 text-[10px] text-fg-subtle">
                        {ot.storage_type}
                    </span>
                </div>
                <div className="mt-0.5 text-[10px] text-fg-subtle">
                    {ot.api_name}
                </div>
                {ot.description && (
                    <p className="mt-1 text-[10px] text-fg-muted">
                        {ot.description}
                    </p>
                )}
            </div>

            {/* 属性表 */}
            {ot.properties.length > 0 && (
                <div className="overflow-x-auto px-3 py-2">
                    <table className="w-full text-[11px]">
                        <thead>
                            <tr className="border-b border-border text-left text-fg-subtle">
                                <th className="py-1 pr-2 font-medium">属性</th>
                                <th className="py-1 pr-2 font-medium">类型</th>
                                <th className="py-1 pr-2 font-medium">标识</th>
                                <th className="py-1 font-medium">搜索</th>
                            </tr>
                        </thead>
                        <tbody>
                            {ot.properties.map((p) => (
                                <tr
                                    key={p.api_name}
                                    className="border-b border-border/50 last:border-0"
                                >
                                    <td className="py-1 pr-2">
                                        <div className="font-medium">
                                            {p.display_name}
                                        </div>
                                        <div className="text-[9px] text-fg-subtle">
                                            {p.api_name}
                                        </div>
                                    </td>
                                    <td className="py-1 pr-2 text-fg-muted">
                                        {p.data_type}
                                    </td>
                                    <td className="py-1 pr-2">
                                        {p.is_primary_key && (
                                            <span
                                                className="text-amber-500"
                                                title="主键"
                                            >
                                                PK
                                            </span>
                                        )}
                                        {p.is_title_property && (
                                            <span
                                                className="ml-1 text-sky-500"
                                                title="标题属性"
                                            >
                                                T
                                            </span>
                                        )}
                                    </td>
                                    <td className="py-1">
                                        {p.searchable ? (
                                            <CheckCircle2
                                                size={10}
                                                className="text-emerald-500"
                                            />
                                        ) : (
                                            <span className="text-fg-subtle">
                                                —
                                            </span>
                                        )}
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            )}

            {/* 内嵌关系（简表） */}
            {ot.links.length > 0 && (
                <div className="border-t border-border px-3 py-2">
                    <div className="mb-1 text-[10px] font-medium text-fg-subtle">
                        关系
                    </div>
                    <div className="space-y-0.5">
                        {ot.links.map((link) => (
                            <LinkInlineRow key={link.api_name} link={link} />
                        ))}
                    </div>
                </div>
            )}
        </div>
    );
}

// ── 关系内嵌行（ObjectType 卡片内） ──
function LinkInlineRow({ link }: { link: LinkDef }) {
    return (
        <div className="flex items-center gap-1.5 text-[10px]">
            <Link2 size={9} className="text-fg-subtle" />
            <span className="font-medium">{link.api_name}</span>
            <ArrowRight size={9} className="text-fg-subtle" />
            <span className="text-fg-muted">
                {link.target_object_type_api_name}
            </span>
            <span className="ml-auto rounded bg-bg-hover px-1 text-fg-subtle">
                {link.cardinality}
            </span>
        </div>
    );
}

// ── 关系表格行（LinkType 扁平列表） ──
function LinkRow({ link, sourceName }: { link: LinkDef; sourceName: string }) {
    return (
        <tr className="border-b border-border/50 last:border-0">
            <td className="py-1.5 pr-3">
                <div className="font-medium">{link.api_name}</div>
                {link.display_name !== link.api_name && (
                    <div className="text-[9px] text-fg-subtle">
                        {link.display_name}
                    </div>
                )}
            </td>
            <td className="py-1.5 pr-3 text-fg-muted">
                {sourceName} → {link.target_object_type_api_name}
            </td>
            <td className="py-1.5 pr-3 text-fg-muted">{link.cardinality}</td>
            <td className="py-1.5 text-fg-muted">
                {link.foreign_key_property_api_name ?? "—"}
            </td>
        </tr>
    );
}

// ── Dataset 卡片（含数据源绑定 + 物理位置） ──
function DatasetCard({ dataset }: { dataset: DatasetDef }) {
    const isView = dataset.is_view;
    const kind = dataset.kind ?? "MANAGED";
    return (
        <div className="rounded-md border border-border px-3 py-2">
            {/* 头部：名称 + kind/view 徽标 */}
            <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                    <span className="text-xs font-medium">
                        {dataset.display_name || dataset.api_name}
                    </span>
                    {dataset.display_name && (
                        <span className="ml-2 text-[10px] text-fg-subtle">
                            {dataset.api_name}
                        </span>
                    )}
                </div>
                <div className="flex shrink-0 items-center gap-1.5 text-[10px] text-fg-subtle">
                    <span className="rounded bg-bg-hover px-1.5 py-0.5">
                        {kind}
                    </span>
                    {isView && (
                        <span className="rounded bg-sky-500/10 px-1.5 py-0.5 text-sky-600 dark:text-sky-400">
                            view
                        </span>
                    )}
                </div>
            </div>

            {/* 绑定关系 + 物理位置 */}
            {(dataset.data_source_api_name ||
                dataset.source_dataset_api_name ||
                dataset.storage_location) && (
                <div className="mt-1.5 space-y-0.5 text-[10px] text-fg-subtle">
                    {dataset.data_source_api_name && (
                        <div className="flex items-center gap-1">
                            <Plug size={9} className="shrink-0" />
                            <span>数据源：</span>
                            <span className="font-medium text-fg-muted">
                                {dataset.data_source_api_name}
                            </span>
                        </div>
                    )}
                    {isView && dataset.source_dataset_api_name && (
                        <div className="flex items-center gap-1">
                            <ArrowRight size={9} className="shrink-0" />
                            <span>派生自：</span>
                            <span className="font-medium text-fg-muted">
                                {dataset.source_dataset_api_name}
                            </span>
                        </div>
                    )}
                    {dataset.storage_location && (
                        <div className="flex items-center gap-1">
                            <Database size={9} className="shrink-0" />
                            <span
                                className="truncate"
                                title={dataset.storage_location}
                            >
                                {dataset.storage_location}
                            </span>
                        </div>
                    )}
                </div>
            )}
        </div>
    );
}

// ── DataSource 卡片（本体声明依赖的外部数据源） ──
// 注意：这里的 DataSource 是本体定义层声明的逻辑数据源（随 payload 导入导出，
// 对齐 Gaia DataSourceCreate），与联邦查询里已注册的物理连接实例是两套概念。
function DataSourceCard({ source }: { source: DataSourceDef }) {
    const hasCredential = !!source.credential_id;
    // connector_config 形如 {"host":"...","database":"..."}，
    // 本地单用户应用：连接配置直接展示（敏感键打码）
    const config =
        source.connector_config &&
        typeof source.connector_config === "object" &&
        !Array.isArray(source.connector_config)
            ? Object.entries(source.connector_config as Record<string, unknown>)
            : [];
    const configRows = config.map(([k, v]) => ({
        key: k,
        value: String(v),
    }));
    return (
        <div className="rounded-md border border-border px-3 py-2">
            {/* 头部：名称 + connector_type 徽标 */}
            <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                    <span className="text-xs font-medium">
                        {source.display_name}
                    </span>
                    <span className="ml-2 text-[10px] text-fg-subtle">
                        {source.api_name}
                    </span>
                </div>
                <div className="flex shrink-0 items-center gap-1.5 text-[10px]">
                    <span className="rounded bg-bg-hover px-1.5 py-0.5 font-medium text-fg-subtle">
                        {source.connector_type}
                    </span>
                    {hasCredential && (
                        <span
                            className="rounded bg-emerald-500/10 px-1.5 py-0.5 text-emerald-600 dark:text-emerald-400"
                            title="已绑定凭据"
                        >
                            凭据
                        </span>
                    )}
                </div>
            </div>

            {source.description && (
                <p className="mt-1 text-[10px] text-fg-muted">
                    {source.description}
                </p>
            )}

            {/* 连接配置（key-value 全量展示） */}
            {configRows.length > 0 && (
                <div className="mt-1.5 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[10px]">
                    {configRows.map(({ key, value }) => (
                        <Fragment key={key}>
                            <span className="text-fg-subtle">{key}</span>
                            <span
                                className="truncate font-medium text-fg-muted"
                                title={value}
                            >
                                {value}
                            </span>
                        </Fragment>
                    ))}
                </div>
            )}
        </div>
    );
}

// ── 导入弹层：选文件/粘贴 JSON → 预演 → 应用 ──
function ImportDialog({
    onClose,
    onImported,
}: {
    onClose: () => void;
    onImported: (apiName: string) => void;
}) {
    const [jsonText, setJsonText] = useState("");
    const [filePath, setFilePath] = useState<string | null>(null);
    const [inputMode, setInputMode] = useState<"file" | "paste">("file");
    const [overwrite, setOverwrite] = useState<string[]>([]);
    const [overwriteDataSources, setOverwriteDataSources] = useState<string[]>(
        [],
    );
    const [parsed, setParsed] = useState<OntologyPayload | null>(null);
    const [parseErr, setParseErr] = useState<string | null>(null);
    const [preview, setPreview] = useState<ImportPreview | null>(null);
    const [timings, setTimings] = useState<{
        preview?: string;
        import?: string;
    }>({});

    const previewMut = usePreviewImport();
    const importMut = useImportOntology();

    // 选文件 → 读内容 → 自动解析
    async function handlePickFile() {
        setParseErr(null);
        setParsed(null);
        setPreview(null);
        try {
            const { open } = await import("@tauri-apps/plugin-dialog");
            const picked = await open({
                multiple: false,
                title: "选择本体 JSON 文件",
                filters: [
                    { name: "JSON", extensions: ["json"] },
                    { name: "所有文件", extensions: ["*"] },
                ],
            });
            if (typeof picked !== "string") return; // 用户取消
            setFilePath(picked);
            // 用 readFile（二进制）+ TextDecoder，复用 Composer 已验证的权限路径
            // （readTextFile 需额外 scope，readFile + fs:scope ** 已生效）
            const { readFile } = await import("@tauri-apps/plugin-fs");
            const bytes = await readFile(picked);
            const text = new TextDecoder("utf-8").decode(bytes);
            setJsonText(text);
            parsePayload(text, picked);
        } catch (e) {
            setParseErr(
                `读取文件失败: ${e instanceof Error ? e.message : String(e)}`,
            );
        }
    }

    // 解析 JSON 文本 → OntologyPayload
    function parsePayload(text: string, source?: string) {
        setPreview(null);
        setParsed(null);
        setParseErr(null);
        try {
            const obj = JSON.parse(text) as OntologyPayload;
            if (!obj.api_name || !obj.object_types) {
                setParseErr("缺少必填字段 api_name 或 object_types");
                return;
            }
            setParsed(obj);
        } catch (e) {
            setParseErr(
                `JSON 解析失败: ${e instanceof Error ? e.message : String(e)}` +
                    (source ? `（${source}）` : ""),
            );
        }
    }

    // 手动粘贴时重新解析
    function handleParsePaste() {
        parsePayload(jsonText, "粘贴内容");
    }

    // 切换某 DataSource 的覆写勾选（skip → overwrite）
    function toggleDsOverwrite(apiName: string) {
        setOverwriteDataSources((prev) =>
            prev.includes(apiName)
                ? prev.filter((n) => n !== apiName)
                : [...prev, apiName],
        );
        // 勾选变更后预演结果已过期，清掉避免展示陈旧状态
        setPreview(null);
    }

    // 预演导入
    async function handlePreview() {
        if (!parsed) return;
        setPreview(null);
        const t0 = performance.now();
        try {
            const result = await previewMut.mutateAsync({
                payload: parsed,
                overwrite,
                overwriteDataSources,
            });
            const t1 = performance.now();
            setTimings((t) => ({ ...t, preview: `${(t1 - t0).toFixed(0)}ms` }));
            setPreview(result);
        } catch {
            // mutation isError 会展示
        }
    }

    // 应用导入
    async function handleImport() {
        if (!parsed) return;
        const t0 = performance.now();
        try {
            await importMut.mutateAsync({
                payload: parsed,
                overwrite,
                overwriteDataSources,
            });
            const t1 = performance.now();
            setTimings((t) => ({ ...t, import: `${(t1 - t0).toFixed(0)}ms` }));
            onImported(parsed.api_name);
        } catch {
            // mutation isError 会展示
        }
    }

    const hasBlockingErrors = preview && preview.errors.length > 0;
    const canImport = parsed && !hasBlockingErrors && !importMut.isPending;

    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
            <div className="flex max-h-[90vh] w-full max-w-4xl flex-col rounded-lg border border-border bg-bg shadow-xl">
                {/* [诊断] 耗时横幅 */}
                <div className="flex items-center justify-center gap-6 border-b border-red-500/40 bg-red-500/10 px-4 py-2 text-base font-bold text-red-600">
                    <span>[诊断耗时]</span>
                    <span>preview: {timings.preview ?? "—"}</span>
                    <span>import: {timings.import ?? "—"}</span>
                </div>
                {/* 头部 */}
                <div className="flex items-center justify-between border-b border-border px-4 py-2">
                    <h2 className="text-sm font-semibold">导入本体</h2>
                    <button
                        onClick={onClose}
                        className="rounded-md p-1 text-fg-subtle hover:bg-bg-hover"
                    >
                        <X size={16} />
                    </button>
                </div>

                <div className="flex min-h-0 flex-1 overflow-y-auto">
                    {/* 左：文件选择 / JSON 输入 */}
                    <div className="flex w-1/2 flex-col border-r border-border">
                        {/* 模式切换 */}
                        <div className="flex border-b border-border">
                            <button
                                onClick={() => setInputMode("file")}
                                className={`flex items-center gap-1.5 px-3 py-2 text-xs font-medium ${
                                    inputMode === "file"
                                        ? "border-b-2 border-accent text-accent"
                                        : "text-fg-subtle hover:bg-bg-hover"
                                }`}
                            >
                                <FileUp size={12} />
                                从文件导入
                            </button>
                            <button
                                onClick={() => setInputMode("paste")}
                                className={`flex items-center gap-1.5 px-3 py-2 text-xs font-medium ${
                                    inputMode === "paste"
                                        ? "border-b-2 border-accent text-accent"
                                        : "text-fg-subtle hover:bg-bg-hover"
                                }`}
                            >
                                <ClipboardPaste size={12} />
                                粘贴 JSON
                            </button>
                        </div>

                        {inputMode === "file" ? (
                            /* 文件选择模式 */
                            <div className="flex flex-1 flex-col">
                                <div className="px-3 py-3">
                                    <button
                                        onClick={handlePickFile}
                                        className="flex w-full items-center justify-center gap-2 rounded-md border border-dashed border-border px-3 py-6 text-xs text-fg-subtle hover:border-accent hover:text-accent"
                                    >
                                        <FileUp size={20} />
                                        <span>选择本体 JSON 文件</span>
                                    </button>
                                </div>
                                {filePath && (
                                    <div className="flex items-center gap-1.5 border-t border-border px-3 py-2 text-[11px]">
                                        <FileText
                                            size={11}
                                            className="shrink-0 text-fg-subtle"
                                        />
                                        <span
                                            className="truncate text-fg-muted"
                                            title={filePath}
                                        >
                                            {filePath.split(/[\\/]/).pop()}
                                        </span>
                                        <button
                                            onClick={handlePickFile}
                                            className="ml-auto text-accent hover:underline"
                                        >
                                            重新选择
                                        </button>
                                    </div>
                                )}
                                {/* 文件内容预览（只读） */}
                                {jsonText && (
                                    <pre className="flex-1 overflow-auto bg-bg p-3 font-mono text-[10px] text-fg-muted">
                                        {jsonText.slice(0, 2000)}
                                        {jsonText.length > 2000 &&
                                            "\n…（截断预览）"}
                                    </pre>
                                )}
                            </div>
                        ) : (
                            /* 粘贴模式 */
                            <>
                                <textarea
                                    value={jsonText}
                                    onChange={(e) =>
                                        setJsonText(e.target.value)
                                    }
                                    placeholder='{"api_name":"...","display_name":"...","object_types":[...]}'
                                    className="flex-1 resize-none bg-bg p-3 font-mono text-[11px] text-fg focus:outline-none"
                                    spellCheck={false}
                                />
                                <div className="flex items-center gap-2 border-t border-border px-3 py-2">
                                    <button
                                        onClick={handleParsePaste}
                                        disabled={!jsonText.trim()}
                                        className="rounded-md bg-bg-hover px-3 py-1 text-xs font-medium hover:bg-bg-hover/70 disabled:opacity-40"
                                    >
                                        解析
                                    </button>
                                </div>
                            </>
                        )}

                        {/* 解析状态 */}
                        <div className="flex items-center gap-2 border-t border-border px-3 py-2">
                            {parseErr && (
                                <span className="flex items-center gap-1 text-[11px] text-red-500">
                                    <XCircle size={12} /> {parseErr}
                                </span>
                            )}
                            {parsed && (
                                <span className="flex items-center gap-1 text-[11px] text-emerald-500">
                                    <CheckCircle2 size={12} /> 已解析：
                                    {parsed.display_name}
                                    <span className="ml-1 text-fg-subtle">
                                        （{parsed.object_types.length}{" "}
                                        对象类型）
                                    </span>
                                </span>
                            )}
                        </div>
                    </div>

                    {/* 右：预演结果 */}
                    <div className="flex w-1/2 flex-col">
                        <div className="flex items-center justify-between border-b border-border px-3 py-2">
                            <label className="text-xs font-medium text-fg-subtle">
                                预演结果
                            </label>
                            <button
                                onClick={handlePreview}
                                disabled={!parsed || previewMut.isPending}
                                className="flex items-center gap-1 rounded-md bg-bg-hover px-3 py-1 text-xs font-medium hover:bg-bg-hover/70 disabled:opacity-40"
                            >
                                {previewMut.isPending ? (
                                    <Loader2
                                        size={12}
                                        className="animate-spin"
                                    />
                                ) : (
                                    <RefreshCw size={12} />
                                )}
                                预演
                            </button>
                        </div>

                        <div className="flex-1 overflow-y-auto p-3">
                            {previewMut.isError && (
                                <div className="mb-2 flex items-start gap-1.5 rounded-md border border-red-500/30 bg-red-500/5 p-2 text-[11px] text-red-500">
                                    <XCircle
                                        size={12}
                                        className="mt-0.5 shrink-0"
                                    />
                                    <span>
                                        预演失败：
                                        {previewMut.error instanceof Error
                                            ? previewMut.error.message
                                            : String(previewMut.error)}
                                    </span>
                                </div>
                            )}

                            {importMut.isError && (
                                <div className="mb-2 flex items-start gap-1.5 rounded-md border border-red-500/30 bg-red-500/5 p-2 text-[11px] text-red-500">
                                    <XCircle
                                        size={12}
                                        className="mt-0.5 shrink-0"
                                    />
                                    <span>
                                        导入失败：
                                        {importMut.error instanceof Error
                                            ? importMut.error.message
                                            : String(importMut.error)}
                                    </span>
                                </div>
                            )}

                            {preview ? (
                                <PreviewResult
                                    preview={preview}
                                    overwriteDataSources={overwriteDataSources}
                                    onToggleDsOverwrite={toggleDsOverwrite}
                                />
                            ) : (
                                <div className="flex h-full items-center justify-center text-center text-xs text-fg-subtle">
                                    {parsed
                                        ? "点击「预演」查看将发生的变更"
                                        : "先在左侧粘贴 JSON 并解析"}
                                </div>
                            )}
                        </div>

                        {/* 底部操作栏 */}
                        <div className="flex items-center justify-between border-t border-border px-3 py-2">
                            <div className="flex items-center gap-2 text-[11px] text-fg-subtle">
                                <span>覆盖：</span>
                                <input
                                    type="text"
                                    value={overwrite.join(", ")}
                                    onChange={(e) =>
                                        setOverwrite(
                                            e.target.value
                                                .split(",")
                                                .map((s) => s.trim())
                                                .filter(Boolean),
                                        )
                                    }
                                    placeholder="ObjectType api_name（逗号分隔）"
                                    className="w-48 rounded border border-border bg-bg px-2 py-0.5 text-[11px] focus:outline-none focus:border-accent"
                                />
                            </div>
                            <button
                                onClick={handleImport}
                                disabled={!canImport || importMut.isPending}
                                className="flex items-center gap-1.5 rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-white hover:bg-accent/90 disabled:opacity-40"
                            >
                                {importMut.isPending ? (
                                    <Loader2
                                        size={12}
                                        className="animate-spin"
                                    />
                                ) : (
                                    <Upload size={12} />
                                )}
                                应用导入
                            </button>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}

// ── 预演结果展示 ──
function PreviewResult({
    preview,
    overwriteDataSources,
    onToggleDsOverwrite,
}: {
    preview: ImportPreview;
    overwriteDataSources: string[];
    onToggleDsOverwrite: (apiName: string) => void;
}) {
    const sections: { label: string; items: ImportPreviewItem[] }[] = [
        { label: "对象类型", items: preview.object_types },
        { label: "关系", items: preview.links },
        { label: "动作", items: preview.actions },
        { label: "数据集", items: preview.datasets },
        { label: "数据源", items: preview.data_sources },
        { label: "分组", items: preview.object_type_groups },
    ];

    return (
        <div className="space-y-3">
            {/* 阻断错误 */}
            {preview.errors.length > 0 && (
                <div className="rounded-md border border-red-500/30 bg-red-500/5 p-2">
                    <div className="mb-1 flex items-center gap-1 text-[11px] font-medium text-red-500">
                        <XCircle size={12} /> 阻断错误（需修复后才能导入）
                    </div>
                    <ul className="ml-4 list-disc text-[11px] text-red-500">
                        {preview.errors.map((e, i) => (
                            <li key={i}>{e}</li>
                        ))}
                    </ul>
                </div>
            )}

            {/* 非阻断警告 */}
            {preview.warnings.length > 0 && (
                <div className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2">
                    <div className="mb-1 flex items-center gap-1 text-[11px] font-medium text-amber-500">
                        <AlertTriangle size={12} /> 警告
                    </div>
                    <ul className="ml-4 list-disc text-[11px] text-amber-600 dark:text-amber-400">
                        {preview.warnings.map((w, i) => (
                            <li key={i}>{w}</li>
                        ))}
                    </ul>
                </div>
            )}

            {/* 本体状态 */}
            <div className="flex items-center gap-2 text-[11px]">
                <span className="text-fg-subtle">本体状态：</span>
                <span
                    className={`rounded px-1.5 py-0.5 font-medium ${
                        preview.ontology_status === "create"
                            ? "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                            : "bg-fg-subtle/10 text-fg-subtle"
                    }`}
                >
                    {preview.ontology_status === "create" ? "新建" : "已存在"}
                </span>
            </div>

            {/* 各实体预演项 */}
            {sections.map(
                (sec) =>
                    sec.items.length > 0 && (
                        <div key={sec.label}>
                            <div className="mb-1 text-[10px] font-medium uppercase tracking-wide text-fg-subtle">
                                {sec.label}
                            </div>
                            <div className="space-y-0.5">
                                {sec.items.map((item) => {
                                    const meta =
                                        PREVIEW_STATUS_META[item.status];
                                    const Icon = meta.icon;
                                    const isDsSkip =
                                        sec.label === "数据源" &&
                                        item.status === "skip";
                                    const dsChecked =
                                        overwriteDataSources.includes(
                                            item.api_name,
                                        );
                                    return (
                                        <div
                                            key={item.api_name}
                                            className="flex items-center gap-1.5 text-[11px]"
                                        >
                                            <Icon
                                                size={11}
                                                className={`shrink-0 ${meta.color}`}
                                            />
                                            <span className="font-medium">
                                                {item.api_name}
                                            </span>
                                            <span
                                                className={`ml-auto ${meta.color}`}
                                            >
                                                {meta.label}
                                            </span>
                                            {isDsSkip && (
                                                <label
                                                    className="ml-2 flex shrink-0 cursor-pointer items-center gap-1 text-[10px] text-fg-subtle hover:text-accent"
                                                    title="勾选后用 payload 中的 connector_config 覆写已有记录（可用于更新真实密码）"
                                                >
                                                    <input
                                                        type="checkbox"
                                                        checked={dsChecked}
                                                        onChange={() =>
                                                            onToggleDsOverwrite(
                                                                item.api_name,
                                                            )
                                                        }
                                                        className="h-3 w-3 accent-amber-500"
                                                    />
                                                    覆盖
                                                </label>
                                            )}
                                            {item.reason && (
                                                <span className="ml-2 truncate text-fg-subtle">
                                                    {item.reason}
                                                </span>
                                            )}
                                        </div>
                                    );
                                })}
                            </div>
                        </div>
                    ),
            )}
        </div>
    );
}
