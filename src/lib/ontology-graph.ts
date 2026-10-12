/**
 * ontology-graph — 本体概念图谱纯逻辑模块（无 React/G6 依赖，便于单元测试）
 *
 * 职责：
 *  1. rowsToGraphData：SPARQL 查询行 → ConceptNode/ConceptEdge（GraphPanel 数据处理）
 *  2. computeDepth：沿 subClassOf 计算类节点深度（用于分层染色）
 *  3. depthStyle / nodeStyle / edgeStyle：主题感知的节点/边样式工厂
 *  4. buildGraphData：组装 G6 GraphData（含 showDatatypes 过滤 + 深度染色）
 *
 * 运行测试：npm test（vitest，见 ontology-graph.test.ts）
 */
import type { GraphData } from "@antv/g6";

// ── 类型 ──

export interface ConceptNode {
    id: string;
    label: string;
    iri: string;
    kind: "class" | "datatype";
    /** 语义标签覆盖：tooltip/详情面板优先显示（缺省按 kind 映射 RDF 术语） */
    kindText?: string;
}

export interface ConceptEdge {
    id: string;
    source: string;
    target: string;
    label?: string;
    kind: "hierarchy" | "object" | "datatype";
    /** 语义标签覆盖：tooltip 优先显示（缺省按 kind 映射 RDF 术语） */
    kindText?: string;
}

export type GraphRow = Record<string, string>;

// ── 主题色板 ──

const THEME: Record<string, Record<string, Record<string, string>>> = {
    light: {
        bg: { canvas: "#ffffff", surface: "#f8fafc" },
        class: { fill: "#dbeafe", stroke: "#3b82f6", labelFill: "#1e3a8a" },
        datatype: { fill: "#fef3c7", stroke: "#f59e0b", labelFill: "#78350f" },
        hierarchy: { stroke: "#94a3b8", labelFill: "#64748b" },
        object: { stroke: "#10b981", labelFill: "#065f46" },
        datatypeEdge: { stroke: "#f59e0b", labelFill: "#b45309" },
        root: { fill: "#4f46e5", stroke: "#4338ca", labelFill: "#ffffff" },
        tier1: { fill: "#2563eb", stroke: "#1d4ed8", labelFill: "#ffffff" },
        tier2: { fill: "#60a5fa", stroke: "#3b82f6", labelFill: "#1e3a8a" },
        tier3: { fill: "#93c5fd", stroke: "#60a5fa", labelFill: "#1e3a8a" },
        tier4: { fill: "#dbeafe", stroke: "#93c5fd", labelFill: "#1e3a8a" },
    },
    dark: {
        bg: { canvas: "#0f172a", surface: "#1e293b" },
        class: { fill: "#1e3a5f", stroke: "#60a5fa", labelFill: "#bfdbfe" },
        datatype: { fill: "#5c3d0e", stroke: "#fbbf24", labelFill: "#fde68a" },
        hierarchy: { stroke: "#64748b", labelFill: "#94a3b8" },
        object: { stroke: "#34d399", labelFill: "#a7f3d0" },
        datatypeEdge: { stroke: "#fbbf24", labelFill: "#fde68a" },
        root: { fill: "#6366f1", stroke: "#818cf8", labelFill: "#e0e7ff" },
        tier1: { fill: "#3b82f6", stroke: "#60a5fa", labelFill: "#bfdbfe" },
        tier2: { fill: "#2563eb", stroke: "#3b82f6", labelFill: "#bfdbfe" },
        tier3: { fill: "#1d4ed8", stroke: "#2563eb", labelFill: "#bfdbfe" },
        tier4: { fill: "#1e3a5f", stroke: "#1d4ed8", labelFill: "#bfdbfe" },
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
            if (dom !== rng) {
                edgeList.push({
                    id: `o-${pKey}-${dom}-${rng}`,
                    source: dom,
                    target: rng,
                    label: labelOf(labelMap, p),
                    kind: "object",
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
              lineWidth: 2,
              labelFill: s.labelFill,
              labelFontSize: 11,
              labelFontWeight: 600,
              labelPlacement: "center",
          }
        : {
              size: [88, 32],
              fill: s.fill,
              stroke: s.stroke,
              lineWidth: 1.5,
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
        lineWidth: isHierarchy ? 1.5 : 2,
        lineDash: isHierarchy ? [6, 4] : kind === "datatype" ? [4, 3] : undefined,
        endArrow: true,
        endArrowFill: s.stroke,
        labelFill: s.labelFill,
        labelFontSize: isHierarchy ? 10 : kind === "datatype" ? 9 : 10,
        labelFontWeight: 500,
        labelBackground: true,
        labelBackgroundFill: t.bg.surface,
        labelBackgroundRadius: 4,
        labelOffsetY: 6,
    };
}

/** 组装 G6 GraphData：showDatatypes 过滤 + 类节点深度染色 + 边标签兜底。 */
export function buildGraphData(
    nodes: ConceptNode[],
    edges: ConceptEdge[],
    theme: string,
    showDatatypes: boolean,
): GraphData {
    const visibleNodes = showDatatypes
        ? nodes
        : nodes.filter((n) => n.kind !== "datatype");
    const nodeIds = new Set(visibleNodes.map((n) => n.id));
    const visibleEdges = edges.filter(
        (e) => nodeIds.has(e.source) && nodeIds.has(e.target),
    );
    const depthMap = computeDepth(visibleNodes, visibleEdges);
    const gNodes = visibleNodes.map((n) => {
        const style = nodeStyle(theme, n.kind);
        // 类节点：按层级深度染色（根深叶浅）
        if (n.kind === "class") {
            const d = depthMap.get(n.id);
            if (d !== undefined) {
                Object.assign(style, depthStyle(theme, d));
            }
        }
        return {
            id: n.id,
            data: { id: n.id, label: n.label, iri: n.iri, kind: n.kind, kindText: n.kindText },
            style: {
                ...style,
                labelText: n.label, // G6 v5 显示标签需要 labelText
            },
        };
    });
    const gEdges = visibleEdges.map((e) => {
        const base = edgeStyle(theme, e.kind);
        return {
            id: e.id,
            source: e.source,
            target: e.target,
            data: { kind: e.kind, label: e.label, kindText: e.kindText },
            style: {
                ...base,
                labelText: e.label ?? (e.kind === "hierarchy" ? "subClassOf" : ""),
            },
        };
    });
    return { nodes: gNodes, edges: gEdges };
}
