/**
 * ontology-graph — 本体概念图谱纯逻辑模块（无 React/G6 依赖，便于单元测试）
 *
 * 职责：
 *  1. rowsToGraphData：SPARQL 查询行 → ConceptNode/ConceptEdge（GraphPanel 数据处理）
 *  2. computeDepth：沿 subClassOf 计算类节点深度（用于分层染色）
 *  3. depthStyle / nodeStyle / edgeStyle：主题感知的节点/边样式工厂
 *  4. buildGraphData：组装 G6 GraphData（含数据类型可见性过滤 + 深度染色）
 *  5. filterDatatypeVisibility：全局 ☐属性 ∨ 按类双击展开集合
 *  6. buildClassDeleteConfirmMessage：删除类确认框文案（属性/关系/继承摘要）
 *
 * 运行测试：npm test（vitest，见 ontology-graph.test.ts）
 */
import type { GraphData } from "@antv/g6";

// ── 类型 ──

/** 相对生效版的差异状态（概念图谱 overlay） */
export type DiffStatus = "added" | "modified" | "removed";

export interface ConceptNode {
    id: string;
    label: string;
    iri: string;
    kind: "class" | "datatype";
    /** 语义标签覆盖：tooltip/详情面板优先显示（缺省按 kind 映射 RDF 术语） */
    kindText?: string;
    /** 相对生效版：新增 / 修改 / 删除（幽灵） */
    diffStatus?: DiffStatus;
}

export interface ConceptEdge {
    id: string;
    source: string;
    target: string;
    label?: string;
    kind: "hierarchy" | "object" | "datatype";
    /** 谓词 IRI（object/datatype 边 = 属性定义 IRI；hierarchy 边为空） */
    predIri?: string;
    /** 语义标签覆盖：tooltip 优先显示（缺省按 kind 映射 RDF 术语） */
    kindText?: string;
    /** 相对生效版：新增 / 修改 / 删除（幽灵） */
    diffStatus?: DiffStatus;
}

export type GraphRow = Record<string, string>;

// ── 主题色板 ──

// 浅色节点底 + 细灰蓝关系线：给「相对生效」绿/黄/红差异留对比度；
// 避免原深蓝实心底 + 粗翠绿边把新增绿淹没。
const THEME: Record<string, Record<string, Record<string, string>>> = {
    light: {
        bg: { canvas: "#ffffff", surface: "#f8fafc" },
        class: { fill: "#ffffff", stroke: "#94a3b8", labelFill: "#334155" },
        datatype: { fill: "#fffbeb", stroke: "#d97706", labelFill: "#78350f" },
        hierarchy: { stroke: "#cbd5e1", labelFill: "#334155" },
        // 关系边标签与类节点标签同色，避免边上字发灰发淡
        object: { stroke: "#64748b", labelFill: "#334155" },
        datatypeEdge: { stroke: "#d97706", labelFill: "#92400e" },
        // 层级仅用极浅冷灰区分，不再用实心靛蓝
        root: { fill: "#f1f5f9", stroke: "#64748b", labelFill: "#0f172a" },
        tier1: { fill: "#f8fafc", stroke: "#94a3b8", labelFill: "#1e293b" },
        tier2: { fill: "#ffffff", stroke: "#94a3b8", labelFill: "#334155" },
        tier3: { fill: "#ffffff", stroke: "#cbd5e1", labelFill: "#475569" },
        tier4: { fill: "#ffffff", stroke: "#e2e8f0", labelFill: "#64748b" },
    },
    dark: {
        bg: { canvas: "#0f172a", surface: "#1e293b" },
        class: { fill: "#1e293b", stroke: "#64748b", labelFill: "#e2e8f0" },
        datatype: { fill: "#422006", stroke: "#fbbf24", labelFill: "#fde68a" },
        hierarchy: { stroke: "#475569", labelFill: "#e2e8f0" },
        object: { stroke: "#94a3b8", labelFill: "#e2e8f0" },
        datatypeEdge: { stroke: "#fbbf24", labelFill: "#fde68a" },
        root: { fill: "#334155", stroke: "#94a3b8", labelFill: "#f8fafc" },
        tier1: { fill: "#1e293b", stroke: "#64748b", labelFill: "#e2e8f0" },
        tier2: { fill: "#1e293b", stroke: "#475569", labelFill: "#cbd5e1" },
        tier3: { fill: "#0f172a", stroke: "#475569", labelFill: "#94a3b8" },
        tier4: { fill: "#0f172a", stroke: "#334155", labelFill: "#94a3b8" },
    },
};

// ── IRI 工具 ──

/** 从 SPARQL 查询返回的 term 提取短名（可能带 <尖括号>）。 */
export function termToShort(term: string): string {
    if (!term) return "";
    const s =
        term.startsWith("<") && term.endsWith(">") ? term.slice(1, -1) : term;
    const parts = s.split(/[#/]/);
    const local = parts.length > 1 ? parts[parts.length - 1] : "";
    return local || s;
}

/**
 * 元素 kind → 对外 RDF 术语。节点/边共用概念图谱 tooltip：
 * 只按 class/非 class 二分会把 object 关系边标成 owl:DatatypeProperty。
 */
export function rdfTermForKind(kind?: string): string {
    switch (kind) {
        case "class":
            return "owl:Class";
        case "object":
            return "owl:ObjectProperty";
        case "hierarchy":
            return "rdfs:subClassOf";
        case "datatype":
            return "owl:DatatypeProperty";
        default:
            return "";
    }
}


/** 从 labelMap 查中文显示名，查不到回退到 IRI 短名。 */
export function labelOf(
    labelMap: Map<string, string> | undefined,
    iri: string,
): string {
    const s = iri.startsWith("<") && iri.endsWith(">") ? iri.slice(1, -1) : iri;
    return labelMap?.get(s) ?? termToShort(iri);
}

// ── SPARQL 行 → 节点/边 ──

/**
 * 将 SPARQL 查询行转换为 ConceptNode/ConceptEdge。
 * 行格式（GraphPanel 的 4 路 UNION 查询）：
 *  - type="class"：节点
 *  - type="hierarchy"：subClassOf 层级边
 *  - type="objectProperty"：ObjectProperty domain→range 边（带属性名）
 *  - type="datatypeProperty"：DatatypeProperty domain 节点 + 琥珀边
 */
export function rowsToGraphData(
    rows: GraphRow[],
    labelMap?: Map<string, string>,
): { nodes: ConceptNode[]; edges: ConceptEdge[] } {
    const nodeMap = new Map<string, ConceptNode>();
    const edgeList: ConceptEdge[] = [];
    // ensureNode：用 IRI local name 做 Map key（去重），显示名查 labelMap
    const ensureNode = (term: string, kind: "class" | "datatype" = "class") => {
        const key = termToShort(term);
        if (!key || key === term) return;
        if (!nodeMap.has(key)) {
            nodeMap.set(key, {
                id: key,
                label: labelOf(labelMap, term),
                iri: term,
                kind,
            });
        }
    };
    for (const row of rows) {
        const s = row["s"] ?? "";
        const p = row["p"] ?? "";
        const o = row["o"] ?? "";
        const type = row["type"] ?? "";
        if (!s) continue;
        if (type === "class") {
            ensureNode(s, "class");
        } else if (type === "hierarchy" && o) {
            ensureNode(s, "class");
            ensureNode(o, "class");
            const sub = termToShort(s);
            const sup = termToShort(o);
            if (sub !== sup) {
                edgeList.push({
                    id: `h-${sub}-${sup}`,
                    source: sub,
                    target: sup,
                    kind: "hierarchy",
                });
            }
        } else if (type === "objectProperty" && p && o) {
            ensureNode(s, "class");
            ensureNode(o, "class");
            const pKey = termToShort(p);
            const dom = termToShort(s);
            const rng = termToShort(o);
            // 允许自环（domain=range，如「上级来源」）；删除态幽灵边本就可画，
            // 若此处过滤则恢复后关系从图谱消失。
            if (dom && rng && pKey) {
                edgeList.push({
                    id: `o-${pKey}-${dom}-${rng}`,
                    source: dom,
                    target: rng,
                    label: labelOf(labelMap, p),
                    kind: "object",
                    predIri: p,
                });
            }
        } else if (type === "datatypeProperty" && p) {
            ensureNode(s, "class");
            ensureNode(p, "datatype");
            const pKey = termToShort(p);
            const dom = termToShort(s);
            if (pKey && dom && pKey !== dom) {
                let label = labelOf(labelMap, p);
                // 如果有 rdfs:range（如 xsd:string），追加到标签
                if (o) {
                    const rangeShort = termToShort(o);
                    if (rangeShort && rangeShort !== o) {
                        label += ` (${rangeShort})`;
                    }
                }
                edgeList.push({
                    id: `d-${pKey}-${dom}`,
                    source: dom,
                    target: pKey,
                    label,
                    kind: "datatype",
                    predIri: p,
                });
            }
        }
    }
    return { nodes: Array.from(nodeMap.values()), edges: edgeList };
}

// ── 层级深度（tier）：沿 rdfs:subClassOf 向上计数，根类深度 0 ──

export function computeDepth(
    nodes: ConceptNode[],
    edges: ConceptEdge[],
): Map<string, number> {
    const parents = new Map<string, string>();
    for (const e of edges) {
        if (e.kind === "hierarchy") parents.set(e.source, e.target);
    }
    const depth = new Map<string, number>();
    const getDepth = (id: string): number => {
        const cached = depth.get(id);
        if (cached !== undefined) return cached;
        const p = parents.get(id);
        const d = p ? getDepth(p) + 1 : 0;
        depth.set(id, d);
        return d;
    };
    for (const n of nodes) if (n.kind === "class") getDepth(n.id);
    return depth;
}

// ── 样式工厂（主题感知）──

export function depthStyle(theme: string, d: number): Record<string, unknown> {
    const t = THEME[theme] ?? THEME.light;
    if (d <= 0) {
        return { fill: t.root.fill, stroke: t.root.stroke, labelFill: t.root.labelFill };
    }
    const key = `tier${Math.min(d, 4)}`;
    const s = t[key] ?? t.tier4;
    return { fill: s.fill, stroke: s.stroke, labelFill: s.labelFill };
}

/** 画布背景色（导出图片时用作背景）。 */
export function canvasBackground(theme: string): string {
    return (THEME[theme] ?? THEME.light).bg.canvas;
}

export function nodeStyle(
    theme: string,
    kind: string,
): Record<string, unknown> {
    const t = THEME[theme] ?? THEME.light;
    const s = kind === "class" ? t.class : t.datatype;
    return kind === "class"
        ? {
              size: 52,
              fill: s.fill,
              stroke: s.stroke,
              lineWidth: 1.5,
              lineDash: [],
              opacity: 1,
              labelFill: s.labelFill,
              labelFontSize: 11,
              labelFontWeight: 600,
              labelPlacement: "center",
          }
        : {
              size: [88, 32],
              fill: s.fill,
              stroke: s.stroke,
              lineWidth: 1,
              lineDash: [],
              opacity: 1,
              radius: 6,
              labelFill: s.labelFill,
              labelFontSize: 10,
              labelFontWeight: 500,
              labelPlacement: "center",
              labelMaxWidth: "100%",
          };
}

export function edgeStyle(
    theme: string,
    kind: string,
): Record<string, unknown> {
    const t = THEME[theme] ?? THEME.light;
    const base: Record<string, Record<string, string>> = {
        hierarchy: t.hierarchy,
        object: t.object,
        datatype: t.datatypeEdge,
    };
    const s = base[kind] ?? t.object;
    const isHierarchy = kind === "hierarchy";
    return {
        stroke: s.stroke,
        // 默认关系线保持细线；差异态边再加粗（见 applyDiffEdgeStyle）
        lineWidth: isHierarchy ? 1 : 1.25,
        // object 边默认实心，但必须**显式写出** lineDash：缺键会让上一帧的虚线残留（G6 只更新给到的属性）
        lineDash: isHierarchy ? [5, 4] : kind === "datatype" ? [4, 3] : [],
        opacity: 1,
        endArrow: true,
        endArrowSize: 6,
        endArrowFill: s.stroke,
        labelFill: s.labelFill,
        labelFontSize: isHierarchy ? 9 : kind === "datatype" ? 9 : 10,
        labelFontWeight: 500,
        labelBackground: true,
        labelBackgroundFill: t.bg.surface,
        labelBackgroundRadius: 4,
        labelOffsetY: 6,
    };
}

/** 松散 IRI 相等：整串相等，或以末段 local name 相等（与 ConceptGraph 清单对齐）。 */
export function iriLooseEq(a: string, b: string): boolean {
    if (!a || !b) return false;
    if (a === b) return true;
    const sa = a.split(/[#/]/).pop() ?? "";
    const sb = b.split(/[#/]/).pop() ?? "";
    return !!sa && sa === sb;
}

// ── 删除类确认摘要（卡片删除用） ──

/** 删除类确认框的输入摘要（组件按 propsQ / 图数据计算，纯函数负责成文）。 */
export interface ClassDeleteSummary {
    /** 待删类 IRI（只用于取名，不进文案） */
    classIri: string;
    /** 将被删除的数据属性（定义域仅该类） */
    datatypeProps: ReadonlyArray<{ iri: string; label?: string }>;
    /** 将被删除的关系（domain 或 range 触及该类） */
    objectProps: ReadonlyArray<{ iri: string; label?: string }>;
    /** 将被解除继承的父类显示名 */
    parentLabels: ReadonlyArray<string>;
    /** 将被解除继承的子类显示名 */
    childLabels: ReadonlyArray<string>;
}

const shortIri = (iri: string) => iri.split(/[#/]/).pop() || iri;

/** 列表裁剪：最多 8 项，余下省略号（避免确认框被长清单撑爆）。 */
function clipNames(xs: ReadonlyArray<string>): string {
    return `${xs.slice(0, 8).join("、")}${xs.length > 8 ? "…" : ""}`;
}

/**
 * 删除类确认框文案：业务语言摘要将一并处理的属性/关系/继承。
 * 不出现引用方 IRI 列表，也不描述后端实现细节。
 */
export function buildClassDeleteConfirmMessage(s: ClassDeleteSummary): string {
    const names = (ps: ClassDeleteSummary["datatypeProps"]) =>
        ps.map((p) => p.label || shortIri(p.iri));
    // 继承分「父类/子类」两组，避免每个名字都重复前缀
    const inheritParts: string[] = [];
    if (s.parentLabels.length) inheritParts.push(`父类 ${s.parentLabels.join("、")}`);
    if (s.childLabels.length) inheritParts.push(`子类 ${s.childLabels.join("、")}`);
    const lines = [`确定删除「${shortIri(s.classIri)}」及其相关定义吗？`];
    if (s.datatypeProps.length) lines.push(`将删除属性：${clipNames(names(s.datatypeProps))}`);
    if (s.objectProps.length) lines.push(`将删除关系：${clipNames(names(s.objectProps))}`);
    if (inheritParts.length) lines.push(`将解除继承：${inheritParts.join("；")}`);
    if (!s.datatypeProps.length && !s.objectProps.length && !inheritParts.length) {
        lines.push("该对象暂无关联的属性、关系或继承。");
    }
    lines.push("共享属性若同时属于其他类，将仅去掉对本类的定义。");
    if (s.objectProps.length) {
        lines.push("关系上的基数/限定约束将随之移除。");
    }
    return lines.join("\n");
}

/**
 * 数据属性可见性：全局开 → 全显；否则仅 domain∈expandedAttrClassIris 的 datatype 节点。
 * 关系（object）边不参与此过滤；无 domain 的孤儿属性仅全局开时可见。
 */
export function filterDatatypeVisibility(
    nodes: ConceptNode[],
    edges: ConceptEdge[],
    showDatatypes: boolean,
    expandedAttrClassIris: ReadonlySet<string> = new Set(),
): { nodes: ConceptNode[]; edges: ConceptEdge[] } {
    if (showDatatypes) return { nodes, edges };

    const classIriById = new Map(
        nodes.filter((n) => n.kind === "class").map((n) => [n.id, n.iri]),
    );
    const domainIriByDtId = new Map<string, string>();
    for (const e of edges) {
        if (e.kind !== "datatype") continue;
        const classIri = classIriById.get(e.source);
        if (classIri) domainIriByDtId.set(e.target, classIri);
    }

    const expandedHas = (iri: string) => {
        for (const x of expandedAttrClassIris) {
            if (iriLooseEq(x, iri)) return true;
        }
        return false;
    };

    const visibleNodes = nodes.filter((n) => {
        if (n.kind !== "datatype") return true;
        const domain = domainIriByDtId.get(n.id);
        if (!domain) return false;
        return expandedHas(domain);
    });
    const nodeIds = new Set(visibleNodes.map((n) => n.id));
    const visibleEdges = edges.filter(
        (e) => nodeIds.has(e.source) && nodeIds.has(e.target),
    );
    return { nodes: visibleNodes, edges: visibleEdges };
}

/** 组装 G6 GraphData：数据类型可见性过滤 + 类节点深度染色 + 边标签兜底。 */
export function buildGraphData(
    nodes: ConceptNode[],
    edges: ConceptEdge[],
    theme: string,
    showDatatypes: boolean,
    expandedAttrClassIris: ReadonlySet<string> = new Set(),
): GraphData {
    const { nodes: visibleNodes, edges: visibleEdges } = filterDatatypeVisibility(
        nodes,
        edges,
        showDatatypes,
        expandedAttrClassIris,
    );
    const depthMap = computeDepth(visibleNodes, visibleEdges);
    const gNodes = visibleNodes.map((n) => {
        const style = nodeStyle(theme, n.kind);
        // 有差异态时跳过深度蓝填充——否则只改描边几乎看不出「新增」
        if (n.kind === "class" && !n.diffStatus) {
            const d = depthMap.get(n.id);
            if (d !== undefined) {
                Object.assign(style, depthStyle(theme, d));
            }
        }
        applyDiffNodeStyle(style, n.diffStatus);
        return {
            id: n.id,
            data: {
                id: n.id,
                label: n.label,
                iri: n.iri,
                kind: n.kind,
                kindText: n.kindText,
                diffStatus: n.diffStatus,
            },
            style: {
                ...style,
                labelText: n.label, // G6 v5 显示标签需要 labelText
            },
        };
    });
    const gEdges = visibleEdges.map((e) => {
        const base = edgeStyle(theme, e.kind);
        applyDiffEdgeStyle(base, e.diffStatus);
        const isLoop = e.source === e.target;
        return {
            id: e.id,
            source: e.source,
            target: e.target,
            data: {
                kind: e.kind,
                label: e.label,
                kindText: e.kindText,
                diffStatus: e.diffStatus,
            },
            style: {
                ...base,
                // G6 v5：source===target 时自动走 loop 路径；给个默认方位避免贴节点
                ...(isLoop
                    ? { loopPlacement: "top" as const, loopDist: 36 }
                    : {}),
                labelText: e.label ?? (e.kind === "hierarchy" ? "subClassOf" : ""),
            },
        };
    });
    return { nodes: gNodes, edges: gEdges };
}

/**
 * 图上统一变更色（粉底虚线）：新增 / 修改 / 删除**完全同形**，不保留任何视觉差异。
 * 类型与「是否仍在草稿里」由变更清单与撤销回执承担；removed 的降透明度肉眼几乎不可辨，
 * 留着只会让「撤销后仍粉虚线」被读成「撤销没生效」，故一并取消。
 */
function applyDiffNodeStyle(
    style: Record<string, unknown>,
    status?: DiffStatus,
): void {
    if (!status) return;
    style.fill = "#fecaca";
    style.stroke = "#b91c1c";
    style.labelFill = "#7f1d1d";
    style.lineWidth = 1.5;
    style.lineDash = [5, 4];
    style.opacity = 1;
}

function applyDiffEdgeStyle(
    style: Record<string, unknown>,
    status?: DiffStatus,
): void {
    if (!status) return;
    style.stroke = "#dc2626";
    style.endArrowFill = "#dc2626";
    // 与变更态类节点标签同色（此前边标签仍用灰蓝，幽灵/新增关系字发淡）
    style.labelFill = "#7f1d1d";
    style.lineWidth = 1.5;
    style.lineDash = [5, 4];
    style.opacity = 1;
}
