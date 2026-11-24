/**
 * ConceptGraph — G6 v5 本体概念图谱组件
 *
 * 替换原有 @xyflow/react 的 GraphPanel 渲染层。
 * 输入：统一节点/边数据模型（与现有 SPARQL 查询结果解耦）
 * 输出：G6 力导向/层级/径向多布局图谱
 *
 * 使用方式：
 *   <ConceptGraph
 *     nodes={[{ id, label, iri, kind }]}
 *     edges={[{ source, target, label, kind }]}
 *   />
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import {
  Search,
  X,
  Copy,
  Maximize,
  Minimize,
  FileCode2,
  Plus,
  Minus,
  Pencil,
  Trash2,
  ArrowLeft,
  Check,
} from "lucide-react";
import { confirm, message } from "@tauri-apps/plugin-dialog";
import { Graph } from "@antv/g6";
import {
  useOntologyTtlClassMetas,
  useOntologyTtlClassIris,
  useOntologyTtlProperties,
  useEditOntologyTtlClass,
  useDeleteOntologyTtlClass,
  useEditOntologyTtlProperty,
  useDeleteOntologyTtlProperty,
} from "@/hooks/useOntologyTtl";
import type { TtlPropertyEdit } from "@/lib/domain";
import type { LayoutOptions } from "@antv/g6";
import {
  buildGraphData,
  canvasBackground,
  computeDepth,
  rdfTermForKind,
  buildClassDeleteConfirmMessage,
  filterDatatypeVisibility,
  iriLooseEq as iriLooseEqLib,
} from "@/lib/ontology-graph";
import type { ConceptNode, ConceptEdge } from "@/lib/ontology-graph";

// ── 主题检测 ──
function subscribeTheme(cb: () => void) {
  const observer = new MutationObserver(cb);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class"],
  });
  return () => observer.disconnect();
}
function getTheme() {
  return document.documentElement.classList.contains("dark") ? "dark" : "light";
}

// 重导出类型供外部使用
// (OntologyTtlView 的 GraphPanel 通过 ConceptGraph 导入 ConceptNode/ConceptEdge)
export type { ConceptNode, ConceptEdge } from "@/lib/ontology-graph";

/** 图选中焦点（供变更清单相关过滤）；与 GraphDiffFocus 对齐。 */
export type ConceptGraphSelection =
  | { kind: "class"; iri: string }
  | { kind: "property"; iri: string }
  | { kind: "hierarchy"; childIri: string; parentIri: string };

interface ConceptGraphProps {
  nodes: ConceptNode[];
  edges: ConceptEdge[];
  /** 持久化键：拖拽后的节点位置按此键存 localStorage（同一本体固定不变） */
  storageKey?: string;
  className?: string;
  /** 图谱→Turtle 联动：点击节点跳转到 Turtle 源码定义行 */
  onJumpToSource?: (iri: string) => void;
  /** 本体 IRI——提供后启用图上编辑（类/属性/关系 的修改与删除） */
  ontologyIri?: string;
  /** 选中变化（点空白/Esc/关详情为 null）；用于变更清单相关过滤 */
  onSelectionChange?: (sel: ConceptGraphSelection | null) => void;
  /** 右栏下段（如变更清单）；详情/编辑在上，清单在下，避免浮层重叠 */
  sidebarBottom?: ReactNode;
}

export function ConceptGraph({
  nodes,
  edges,
  storageKey,
  className,
  onJumpToSource,
  ontologyIri,
  onSelectionChange,
  sidebarBottom,
}: ConceptGraphProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const graphRef = useRef<Graph | null>(null);
  const [layoutType, setLayoutType] = useState<string>(() => {
    if (storageKey) {
      try {
        const saved = localStorage.getItem(
          `concept-graph-layout-${storageKey}`,
        );
        const valid = ["concentric", "force", "dagre", "circular", "grid"];
        if (saved && valid.includes(saved)) return saved;
      } catch {
        /* ignore */
      }
    }
    return "dagre";
  });
  // 全屏状态
  const [isFullscreen, setIsFullscreen] = useState(false);
  // 主题订阅：跟随 html.dark 类切换（Tailwind v4）
  const theme = useSyncExternalStore(subscribeTheme, getTheme);

  // 弹层状态
  const [detail, setDetail] = useState<{
    node: ConceptNode;
    x: number;
    y: number;
  } | null>(null);
  // 边详情（关系/属性定义）
  const [edgeDetail, setEdgeDetail] = useState<ConceptEdge | null>(null);
  /** 属性/关系查看卡（从类卡行进入或点属性节点/关系边） */
  const [propView, setPropView] = useState<{
    iri: string;
    kind: "object" | "datatype";
  } | null>(null);
  /** 类锚：编属性/关系门禁 + 清单宽过滤 */
  const [anchorClassIri, setAnchorClassIri] = useState<string | null>(null);
  const [anchorGateMsg, setAnchorGateMsg] = useState("");
  // 编辑面板：类 / 属性 / 关系（分型，不可互转）
  const [editing, setEditing] = useState<
    | { type: "class"; iri: string }
    | { type: "property"; iri: string; kind: "object" | "datatype" }
    | null
  >(null);

  // 属性开关（默认不勾选，避免大数据属性量时图过密）
  const [showDatatypes, setShowDatatypes] = useState(false);
  /** 双击类展开的数据属性 domain 集合（与 showDatatypes 并集） */
  const [expandedAttrClassIris, setExpandedAttrClassIris] = useState<
    Set<string>
  >(() => new Set());
  // 搜索定位
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);

  const searchResults = useMemo(() => {
    if (!searchQuery.trim()) return [];
    const q = searchQuery.toLowerCase();
    const { nodes: visible } = filterDatatypeVisibility(
      nodes,
      edges,
      showDatatypes,
      expandedAttrClassIris,
    );
    return visible
      .filter(
        (n) =>
          n.label.toLowerCase().includes(q) || n.id.toLowerCase().includes(q),
      )
      .slice(0, 20);
  }, [nodes, edges, searchQuery, showDatatypes, expandedAttrClassIris]);

  const onSelectionChangeRef = useRef(onSelectionChange);
  onSelectionChangeRef.current = onSelectionChange;
  const emitSelection = (sel: ConceptGraphSelection | null) => {
    onSelectionChangeRef.current?.(sel);
  };

  // 搜索定位（“选中高亮”）：清除旧高亮 → focusElement → 高亮目标
  const handleSearchSelect = (id: string) => {
    const graph = graphRef.current;
    if (!graph) return;
    // 清除之前的高亮
    clearHighlight(graph);
    // 定位并高亮
    graph.focusElement(id, false);
    setTimeout(() => {
      applyHighlight(graph, id);
    }, 100);
    setSearchQuery("");
    setSearchOpen(false);
    setDetail(null);
    setEdgeDetail(null);
    const node = nodes.find((n) => n.id === id);
    if (node?.kind === "class") applyClassSelect(node);
    else if (node?.kind === "datatype")
      applyPropertySelect(node.iri, "datatype");
    else {
      setAnchorClassIri(null);
      emitSelection(null);
    }
  };

  const handleSearchOpen = () => {
    setSearchOpen(true);
    setTimeout(() => searchInputRef.current?.focus(), 50);
  };

  // ── 高亮/还原：直接改 data.style + 显式 draw ──
  // 不走 G6 v5 的 setElementState 状态系统：v5.1.1 实测（浏览器像素级验证）
  // setElementState 清空状态后元素树样式已恢复、但 G 渲染器的脏区不生效，
  // canvas 像素停留在 dimmed 帧不再重绘（连 refreshData/element.draw 都无法
  // 恢复，只有布局/视口这类结构性变化能触发重绘）。updateNodeData +
  // element.draw 组合可稳定触发重绘，故高亮与还原统一走 data.style。
  // 样式值与原 node/edge state 定义保持一致：
  //   节点 focused: { opacity 1, lineWidth 3, stroke #f59e0b }  dimmed: { opacity 0.12 }
  //   边   focused: { opacity 1 }                                dimmed: { opacity 0.08 }
  // buildData 经 ref 读取：graph.on 装在 [] 闭包里，直接闭包 buildData 会过期。
  // 先占位，buildData 定义后每帧回写（见下方）。
  const buildDataRef = useRef<() => ReturnType<typeof buildGraphData>>(
    () => ({ nodes: [], edges: [] }),
  );

  const redrawElements = (graph: Graph) => {
    try {
      // Graph.context 是 private，但运行时可访问（v5 官方 setElementState 内部
      // 同样走 context.element.draw）；用断言访问避免 G6 暴露公开 draw API 前
      // 无法显式重绘的问题。
      const el = (
        graph as unknown as {
          context: {
            element: {
              draw: (cfg: { animation: boolean; stage: string }) => unknown;
            };
          };
        }
      ).context.element;
      void el.draw({ animation: false, stage: "state" });
    } catch {
      /* element controller 不可用时忽略 */
    }
  };

  const applyHighlight = (graph: Graph, nodeId: string) => {
    const { edges: curEdges } = dataRef.current;
    const neighborIds = new Set<string>([nodeId]);
    for (const e of curEdges) {
      if (e.source === nodeId) neighborIds.add(e.target);
      if (e.target === nodeId) neighborIds.add(e.source);
    }
    const nodeUpdates = graph
      .getNodeData()
      .filter((n) => n.id)
      .map((n) => ({
        id: n.id as string,
        style: neighborIds.has(n.id as string)
          ? { opacity: 1, lineWidth: 3, stroke: "#f59e0b" }
          : { opacity: 0.12 },
      }));
    const edgeUpdates = graph
      .getEdgeData()
      .filter((e) => e.id)
      .map((e) => ({
        id: e.id as string,
        style:
          neighborIds.has(e.source) && neighborIds.has(e.target)
            ? { opacity: 1 }
            : { opacity: 0.08 },
      }));
    graph.updateNodeData(nodeUpdates);
    graph.updateEdgeData(edgeUpdates);
    redrawElements(graph);
  };

  // 清除全部高亮（详情关闭 / Esc / 画布·边·minimap 点击时调用）。
  // 必须把 stroke/lineWidth 一并还原：updateNodeData 是合并写入，
  // 只设 opacity:1 时橙色描边会残留在数据上。
  const clearHighlight = (graph: Graph | null) => {
    if (!graph) return;
    const baseline = buildDataRef.current();
    const nodeBase = new Map(
      (baseline.nodes ?? []).map((n) => [
        n.id as string,
        n.style as Record<string, unknown> | undefined,
      ]),
    );
    const edgeBase = new Map(
      (baseline.edges ?? []).map((e) => [
        e.id as string,
        e.style as Record<string, unknown> | undefined,
      ]),
    );
    const nodeUpdates = graph
      .getNodeData()
      .filter((n) => n.id)
      .map((n) => {
        const id = n.id as string;
        const base = nodeBase.get(id) ?? {};
        return {
          id,
          style: {
            opacity: 1,
            stroke: base.stroke as string | undefined,
            lineWidth: base.lineWidth as number | undefined,
          },
        };
      });
    const edgeUpdates = graph
      .getEdgeData()
      .filter((e) => e.id)
      .map((e) => {
        const id = e.id as string;
        const base = edgeBase.get(id) ?? {};
        return {
          id,
          style: {
            opacity: 1,
            stroke: base.stroke as string | undefined,
            lineWidth: base.lineWidth as number | undefined,
            endArrowFill: base.endArrowFill as string | undefined,
          },
        };
      });
    graph.updateNodeData(nodeUpdates);
    graph.updateEdgeData(edgeUpdates);
    redrawElements(graph);
  };

  // ── 图上编辑（ontologyIri 提供时启用）：hooks 无条件调用，query 用 enabled 门控 ──
  const classMetasQ = useOntologyTtlClassMetas(ontologyIri ?? null);
  const classIrisQ = useOntologyTtlClassIris(ontologyIri ?? null);
  const propsQ = useOntologyTtlProperties(ontologyIri ?? null);
  const editClass = useEditOntologyTtlClass();
  const deleteClass = useDeleteOntologyTtlClass();
  const editProp = useEditOntologyTtlProperty();
  const deleteProp = useDeleteOntologyTtlProperty();
  const [form, setForm] = useState<{
    label: string;
    comment: string;
    domain: string;
    range: string;
    parents: string[];
  }>({ label: "", comment: "", domain: "", range: "", parents: [] });
  const [formErr, setFormErr] = useState("");
  const [formOk, setFormOk] = useState("");

  const openClassEdit = (iri: string) => {
    const meta = (classMetasQ.data ?? []).find((m) => m.iri === iri);
    const parents = edges
      .filter((e) => e.kind === "hierarchy" && e.source === iri)
      .map((e) => nodes.find((n) => n.id === e.target)?.iri ?? "");
    setEditing({ type: "class", iri });
    setForm({
      label: meta?.label ?? "",
      comment: meta?.comment ?? "",
      domain: "",
      range: "",
      parents: parents.filter(Boolean),
    });
    setFormErr("");
    setFormOk("");
  };

  const iriLooseEq = iriLooseEqLib;

  const propTouchesClass = (prop: TtlPropertyEdit | undefined, classIri: string) => {
    if (!prop) return false;
    return (
      iriLooseEq(prop.domain ?? "", classIri) ||
      iriLooseEq(prop.range ?? "", classIri)
    );
  };

  const openPropertyEdit = (predIri: string, kind: "object" | "datatype") => {
    if (!anchorClassIri) {
      setAnchorGateMsg("请先选择一个类，再编辑属性或关系");
      return;
    }
    const prop = (propsQ.data ?? []).find((x) => x.iri === predIri);
    if (prop && !propTouchesClass(prop, anchorClassIri)) {
      setAnchorGateMsg("请先选择一个类，再编辑属性或关系");
      return;
    }
    setAnchorGateMsg("");
    setEditing({ type: "property", iri: predIri, kind });
    setForm({
      label: prop?.label ?? "",
      comment: prop?.comment ?? "",
      domain: prop?.domain ?? "",
      range: prop?.range ?? "",
      parents: [],
    });
    setFormErr("");
    setFormOk("");
  };

  const saveEdit = () => {
    if (!ontologyIri || !editing) return;
    setFormErr("");
    setFormOk("");
    if (editing.type === "class") {
      editClass.mutate(
        {
          iri: ontologyIri,
          edit: {
            iri: editing.iri,
            label: form.label,
            comment: form.comment,
            parents: form.parents,
          },
        },
        {
          onSuccess: () => setFormOk("已保存"),
          onError: (e) => setFormErr((e as Error).message),
        },
      );
    } else {
      const prop = (propsQ.data ?? []).find((x) => x.iri === editing.iri);
      const next: TtlPropertyEdit = {
        iri: editing.iri,
        kind: editing.kind,
        label: form.label,
        comment: form.comment,
        domain: form.domain,
        range: form.range,
      };
      editProp.mutate(
        { iri: ontologyIri, edit: next },
        {
          onSuccess: () => {
            void prop;
            setFormOk("已保存");
          },
          onError: (e) => setFormErr((e as Error).message),
        },
      );
    }
  };

  /** 取消编辑：回查看卡，保留类锚。 */
  const cancelEdit = () => {
    const was = editing;
    setEditing(null);
    setFormErr("");
    setFormOk("");
    if (!was) return;
    if (was.type === "class") {
      const node =
        nodes.find((n) => n.kind === "class" && iriLooseEq(n.iri, was.iri)) ??
        ({
          id: was.iri,
          label: was.iri.split(/[#/]/).pop() || was.iri,
          iri: was.iri,
          kind: "class" as const,
        } satisfies ConceptNode);
      applyClassSelect(node);
      return;
    }
    setPropView({ iri: was.iri, kind: was.kind });
    setDetail(null);
    setEdgeDetail(null);
  };

  /** 一期摘要：基于 propsQ 单 domain（多定义域共享可能显示为独占，以后端执行为准）。 */
  const confirmDeleteClassMessage = (classIri: string) => {
    const props = propsQ.data ?? [];
    const datatypeProps: { iri: string; label?: string }[] = [];
    const objectProps: { iri: string; label?: string }[] = [];
    for (const p of props) {
      const label = p.label || undefined;
      if (p.kind === "object") {
        if (
          iriLooseEq(p.domain ?? "", classIri) ||
          iriLooseEq(p.range ?? "", classIri)
        ) {
          objectProps.push({ iri: p.iri, label });
        }
      } else if (iriLooseEq(p.domain ?? "", classIri)) {
        datatypeProps.push({ iri: p.iri, label });
      }
    }
    // 继承边：该类作为父/子端点的边会一并解除
    const classNode = nodes.find(
      (n) => n.kind === "class" && iriLooseEq(n.iri, classIri),
    );
    const labelOf = (id: string) => nodes.find((n) => n.id === id)?.label ?? id;
    const parentLabels = edges
      .filter((e) => e.kind === "hierarchy" && e.source === classNode?.id)
      .map((e) => labelOf(e.target));
    const childLabels = edges
      .filter((e) => e.kind === "hierarchy" && e.target === classNode?.id)
      .map((e) => labelOf(e.source));
    return buildClassDeleteConfirmMessage({
      classIri,
      datatypeProps,
      objectProps,
      parentLabels,
      childLabels,
    });
  };

  /**
   * 卡片删类：确认通过后才发起写入。
   *
   * MUST NOT 用 window.confirm —— tauri-plugin-dialog 在 webview 初始化时把它覆盖成
   * 返回 Promise 的异步函数，`!window.confirm(x)` 恒为 false，用户点取消也会执行。
   */
  const deleteClassFromCard = async (classIri: string) => {
    if (!ontologyIri) return;
    const ok = await confirm(confirmDeleteClassMessage(classIri), {
      kind: "warning",
    });
    if (!ok || deleteClass.isPending) return;
    deleteClass.mutate(
      { iri: ontologyIri, classIri },
      {
        onSuccess: () => {
          setEditing(null);
          setDetail(null);
          setPropView(null);
          setEdgeDetail(null);
          setAnchorClassIri(null);
          setAnchorGateMsg("");
          clearHighlight(graphRef.current);
          emitSelection(null);
        },
        onError: (e) => {
          const msg = (e as Error).message || "删除失败，请重试";
          // 禁止向用户堆 IRI 墙
          const friendly = msg.includes("http")
            ? "删除失败：仍有无法自动清理的引用，请先处理相关内容后再试"
            : msg;
          void message(friendly, { kind: "error" });
        },
      },
    );
  };

  /** 卡片删属性/关系：同样先 await 确认，再发起写入。 */
  const deletePropertyFromCard = async (propertyIri: string) => {
    if (!ontologyIri) return;
    const ok = await confirm("确定删除该属性或关系吗？", { kind: "warning" });
    if (!ok || deleteProp.isPending) return;
    deleteProp.mutate(
      { iri: ontologyIri, propertyIri },
      {
        onSuccess: () => {
          setEditing(null);
          setPropView(null);
          setEdgeDetail(null);
          if (anchorClassIri) {
            const node = nodes.find(
              (n) => n.kind === "class" && iriLooseEq(n.iri, anchorClassIri),
            );
            if (node) applyClassSelect(node);
            else {
              setDetail(null);
              setAnchorClassIri(null);
              emitSelection(null);
            }
          } else {
            setDetail(null);
            emitSelection(null);
          }
        },
        onError: (e) => {
          void message((e as Error).message || "删除失败，请重试", {
            kind: "error",
          });
        },
      },
    );
  };

  const closeDetail = () => {
    setDetail(null);
    setPropView(null);
    setEdgeDetail(null);
    setAnchorClassIri(null);
    setAnchorGateMsg("");
    setEditing(null);
    clearHighlight(graphRef.current);
    emitSelection(null);
  };

  /** 清单焦点：有类锚时始终钉类（宽过滤）；否则跟选中元素。 */
  const emitListFocus = (
    sel: ConceptGraphSelection | null,
    nextAnchor: string | null,
  ) => {
    if (nextAnchor) {
      emitSelection({ kind: "class", iri: nextAnchor });
      return;
    }
    emitSelection(sel);
  };

  const applyClassSelect = (node: ConceptNode) => {
    setDetail({ node, x: 0, y: 0 });
    setPropView(null);
    setEdgeDetail(null);
    setEditing(null);
    setAnchorClassIri(node.iri);
    setAnchorGateMsg("");
    emitListFocus({ kind: "class", iri: node.iri }, node.iri);
  };

  /** 属性卡「返回」：回到下钻前类卡（锚优先，否则用 domain）。 */
  const backFromPropView = (domainHint?: string | null) => {
    const classIri = anchorClassIri || domainHint || null;
    if (!classIri) {
      closeDetail();
      return;
    }
    const classNode =
      nodes.find((n) => n.kind === "class" && iriLooseEq(n.iri, classIri)) ??
      ({
        id: classIri,
        label: classIri.split(/[#/]/).pop() || classIri,
        iri: classIri,
        kind: "class" as const,
      } satisfies ConceptNode);
    applyClassSelect(classNode);
  };

  const toggleExpandedAttrClass = (classIri: string) => {
    setExpandedAttrClassIris((prev) => {
      const next = new Set(prev);
      let found: string | null = null;
      for (const x of next) {
        if (iriLooseEq(x, classIri)) {
          found = x;
          break;
        }
      }
      if (found) next.delete(found);
      else next.add(classIri);
      return next;
    });
  };

  const applyPropertySelect = (
    predIri: string,
    kind: "object" | "datatype",
  ) => {
    const prop = (propsQ.data ?? []).find((x) => x.iri === predIri);
    const touches =
      !!anchorClassIri && propTouchesClass(prop, anchorClassIri);
    if (touches && anchorClassIri) {
      setDetail(null);
      setEdgeDetail(null);
      setPropView({ iri: predIri, kind });
      setAnchorGateMsg("");
      emitListFocus({ kind: "property", iri: predIri }, anchorClassIri);
      return;
    }
    // 无关或无锚：切换焦点，清锚
    setDetail(null);
    setEdgeDetail(null);
    setPropView({ iri: predIri, kind });
    setAnchorClassIri(null);
    setAnchorGateMsg("");
    emitListFocus({ kind: "property", iri: predIri }, null);
  };

  // 构建 G6 数据（主题感知），稳定引用避免重复渲染
  const buildData = useCallback(
    () =>
      buildGraphData(
        nodes,
        edges,
        theme,
        showDatatypes,
        expandedAttrClassIris,
      ),
    [nodes, edges, theme, showDatatypes, expandedAttrClassIris],
  );
  buildDataRef.current = buildData;

  // 内容签名：相同节点/边数据跳过 setData+render
  const dataSignature = useMemo(() => {
    let h = 0;
    for (const n of nodes) {
      h = ((h << 5) - h + n.id.length) | 0;
      // diffStatus 必须进签名：异步 diff overlay 到达时 id 不变，
      // 漏掉会跳过 setData，图上永远看不到相对生效着色。
      if (n.diffStatus === "added") h = ((h << 3) - h + 11) | 0;
      else if (n.diffStatus === "modified") h = ((h << 3) - h + 13) | 0;
      else if (n.diffStatus === "removed") h = ((h << 3) - h + 17) | 0;
    }
    for (const e of edges) {
      h = ((h << 5) - h + e.id.length) | 0;
      if (e.diffStatus === "added") h = ((h << 3) - h + 19) | 0;
      else if (e.diffStatus === "modified") h = ((h << 3) - h + 23) | 0;
      else if (e.diffStatus === "removed") h = ((h << 3) - h + 29) | 0;
    }
    // 显示开关/主题/按类展开也参与签名：它们改变 buildData 输出（过滤/配色），
    // 但不改变 nodes/edges——不进签名会被增量更新去重跳过，
    // 画布不同步（例：取消勾选「属性」后属性节点残留）。
    h = ((h << 3) - h + (showDatatypes ? 1 : 0)) | 0;
    h = ((h << 3) - h + (theme === "dark" ? 2 : 0)) | 0;
    for (const iri of [...expandedAttrClassIris].sort()) {
      h = ((h << 5) - h + iri.length) | 0;
      for (let i = 0; i < iri.length; i++) h = ((h << 5) - h + iri.charCodeAt(i)) | 0;
    }
    return h;
  }, [nodes, edges, showDatatypes, theme, expandedAttrClassIris]);

  // dataRef：graph.on 回调（[] 闭包）中读取最新节点/边数据
  const dataRef = useRef({ nodes, edges });
  dataRef.current = { nodes, edges };

  const applyClassSelectRef = useRef(applyClassSelect);
  applyClassSelectRef.current = applyClassSelect;
  const applyPropertySelectRef = useRef(applyPropertySelect);
  applyPropertySelectRef.current = applyPropertySelect;
  const toggleExpandedAttrClassRef = useRef(toggleExpandedAttrClass);
  toggleExpandedAttrClassRef.current = toggleExpandedAttrClass;

  const selectionFromEdge = (e: ConceptEdge): ConceptGraphSelection | null => {
    if (e.kind === "hierarchy") {
      const { nodes: ns } = dataRef.current;
      const child = ns.find((n) => n.id === e.source);
      const parent = ns.find((n) => n.id === e.target);
      if (!child?.iri || !parent?.iri) return null;
      return {
        kind: "hierarchy",
        childIri: child.iri,
        parentIri: parent.iri,
      };
    }
    if (e.predIri) return { kind: "property", iri: e.predIri };
    return null;
  };

  // 主题切换时重建样式并重绘
  useEffect(() => {
    const graph = graphRef.current;
    if (!graph) return;
    graph.setData(buildData());
    graph.render();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [theme]);

  // 创建/销毁 G6 实例
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const graph = new Graph({
      container,
      data: buildData(),
      layout: { type: "dagre", rankdir: "TB", nodesep: 60, ranksep: 80 },
      node: {
        state: {
          focused: { opacity: 1, lineWidth: 3, stroke: "#f59e0b" },
          selected: { opacity: 1, lineWidth: 3, stroke: "#f59e0b" },
          dimmed: { opacity: 0.12 },
        },
      },
      edge: {
        state: {
          focused: { opacity: 1 },
          dimmed: { opacity: 0.08 },
        },
      },
      behaviors: [
        "drag-canvas",
        // 大图下滚轮缩放卡顿：默认 animation 200ms 每帧全量重绘，改同步缩放
        { type: "zoom-canvas", animation: false, sensitivity: 1.5 },
        "drag-element",
        // 注意：hover-activate 与手动 setElementState(focused/dimmed) 冲突，
        // hover 产生的 active 状态会在 clearAllStates 后即刻被 hover-activate 的
        // pointermove 处理恢复，导致点击空白处/ESC 无法清除高亮。手动管理状态时
        // 不要启用 hover-activate。
      ],
      plugins: [
        { type: "minimap", size: [160, 110], position: "left-bottom" },
        {
          type: "contextmenu",
          trigger: "contextmenu",
          getItems: (evt: unknown) => {
            const target = (evt as { target?: { id?: string } }).target;
            if (!target?.id) return [];
            const nodeData = graph.getElementData(target.id);
            const d = nodeData?.data;
            if (!d) return [];
            return [
              { name: "查看详情", value: "detail" },
              { name: "复制 IRI", value: "copyIri" },
              { name: "在新窗口打开 IRI", value: "openIri" },
            ];
          },
          onClick: (value: string, _target: HTMLElement, current: unknown) => {
            const currentNode = current as { id?: string };
            if (!currentNode?.id) return;
            const nodeData = graph.getElementData(currentNode.id) as {
              data?: { iri?: string; label?: string; kind?: string };
            };
            const d = nodeData?.data;
            if (!d) return;
            if (value === "copyIri" && d.iri) {
              navigator.clipboard?.writeText(d.iri).catch(() => {});
            } else if (value === "openIri" && d.iri) {
              window.open(d.iri, "_blank", "noopener");
            } else if (value === "detail" && d.iri) {
              const node = nodes.find((n) => n.iri === d.iri);
              if (node) setDetail({ node, x: 0, y: 0 });
            }
          },
        },
        {
          type: "tooltip",
          getContent: (_evt: unknown, items: unknown[]) => {
            const d = (
              items?.[0] as {
                data?: {
                  label?: string;
                  iri?: string;
                  kind?: string;
                  kindText?: string;
                };
              }
            )?.data;
            if (!d) return "";
            // 节点与边共用该 tooltip：按真实元素类型映射术语
            const kindText = d.kindText ?? rdfTermForKind(d.kind);
            return `<div style="font-size:11px;line-height:1.5">
              <div style="font-weight:600">${d.label ?? ""}</div>
              ${kindText ? `<div style="color:#64748b">${kindText}</div>` : ""}
              <div style="color:#94a3b8;word-break:break-all;max-width:240px">${d.iri ?? ""}</div>
            </div>`;
          },
        },
      ],
      autoFit: "view",
      animation: false,
      background: canvasBackground(theme),
    });

    // ResizeObserver：容器尺寸变化时按实像素通知 G6（避免 sizeOf 在 0 高时落到 480 默认值后卡住）
    let lastSize = 0;
    const resizeObserver = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const w = Math.floor(entry.contentRect.width);
      const h = Math.floor(entry.contentRect.height);
      const area = w * h;
      if (w > 0 && h > 0) {
        graph.resize(w, h);
        // 容器 resize 后也重新定位 minimap（全屏/切回窗口）；
        // setupMinimap 幂等，重复调用安全
        setupMinimap();
        // 从 0 恢复，或宽高变化较大时重新适配（右栏开合）
        if (
          lastSize === 0 ||
          Math.abs(area - lastSize) / Math.max(lastSize, 1) > 0.08
        ) {
          graph.fitView();
        }
      }
      lastSize = area;
    });
    resizeObserver.observe(container);

    // 单击延迟；双击用 click.detail===2（勿再听 node:dblclick）。
    // G6 会对同一次双击各发一次「合成 dblclick」(detail=2) 与「原生 dblclick」，
    // 若两边都 toggle 会展开又立刻收起。
    let nodeClickTimer: ReturnType<typeof setTimeout> | null = null;
    graph.on("node:click", (evt: unknown) => {
      const e = evt as { target?: { id?: string }; detail?: number };
      const nodeId = e.target?.id;
      const detail = e.detail ?? 1;
      if (nodeClickTimer) {
        clearTimeout(nodeClickTimer);
        nodeClickTimer = null;
      }
      if (detail === 2) {
        const node = dataRef.current.nodes.find((n) => n.id === nodeId);
        if (node?.kind === "class") {
          toggleExpandedAttrClassRef.current(node.iri);
        }
        return;
      }
      nodeClickTimer = setTimeout(() => {
        nodeClickTimer = null;
        const { nodes: curNodes } = dataRef.current;
        if (nodeId) {
          const node = curNodes.find((n) => n.id === nodeId);
          if (node) {
            applyHighlight(graph, node.id);
            if (node.kind === "class") applyClassSelectRef.current(node);
            else applyPropertySelectRef.current(node.iri, "datatype");
          }
        } else {
          setDetail(null);
          setPropView(null);
          setEdgeDetail(null);
          setAnchorClassIri(null);
          clearHighlight(graph);
          emitSelection(null);
        }
      }, 280);
    });

    graph.on("canvas:click", () => {
      setDetail(null);
      setPropView(null);
      setEdgeDetail(null);
      setEditing(null);
      setAnchorClassIri(null);
      setAnchorGateMsg("");
      clearHighlight(graph);
      emitSelection(null);
    });

    // 边点击 → 关系/属性查看；继承边仍走边详情
    graph.on("edge:click", (evt: unknown) => {
      const target = (evt as { target?: { id?: string } }).target;
      const edgeId = target?.id;
      const { edges: curEdges } = dataRef.current;
      const e = curEdges.find((x) => x.id === edgeId);
      if (e) {
        clearHighlight(graph);
        if (e.kind === "object" && e.predIri) {
          applyPropertySelectRef.current(e.predIri, "object");
        } else if (e.kind === "datatype" && e.predIri) {
          applyPropertySelectRef.current(e.predIri, "datatype");
        } else {
          setDetail(null);
          setPropView(null);
          setEdgeDetail(e);
          setAnchorClassIri(null);
          emitSelection(selectionFromEdge(e));
        }
      } else {
        setEdgeDetail(null);
        setDetail(null);
        setPropView(null);
        setAnchorClassIri(null);
        clearHighlight(graph);
        emitSelection(null);
      }
    });

    // ── 拖拽位置持久化：拖完落盘，下次打开恢复 ──
    const posKey = storageKey ? `concept-graph-positions-${storageKey}` : null;
    graph.on("node:dragend", () => {
      if (!posKey) return;
      const positions: Record<string, [number, number]> = {};
      for (const n of graph.getNodeData()) {
        const p = graph.getElementPosition(n.id);
        if (Number.isFinite(p[0]) && Number.isFinite(p[1])) {
          positions[n.id] = [p[0], p[1]];
        }
      }
      try {
        localStorage.setItem(posKey, JSON.stringify(positions));
      } catch {
        /* 隐私模式等场景下静默失败 */
      }
    });

    // 首屏渲染后恢复上次拖拽位置
    graph.once("afterrender", () => {
      if (!posKey) return;
      let stored: string | null = null;
      try {
        stored = localStorage.getItem(posKey);
      } catch {
        return;
      }
      if (!stored) return;
      try {
        const positions = JSON.parse(stored) as Record<
          string,
          [number, number]
        >;
        const posMap: Record<string, [number, number]> = {};
        for (const n of graph.getNodeData()) {
          if (positions[n.id]) posMap[n.id] = positions[n.id];
        }
        if (Object.keys(posMap).length > 0) {
          graph.translateElementTo(posMap, false);
        }
      } catch {
        /* 解析失败则忽略 */
      }
    });

    graph.render();
    graphRef.current = graph;
    // 调试暴露：DEV 环境挂 window，便于浏览器 console 检查事件派发与状态（不进生产包）
    if (import.meta.env.DEV) {
      (window as unknown as { __conceptGraph?: Graph }).__conceptGraph = graph;
    }

    // 修正 minimap 位置与交互：graph.render() 是异步的，minimap 容器在
    // afterrender 之后才创建，必须等它出现再处理（同步执行时 container
    // 为 undefined，会被静默跳过——旧版 fixMinimapPos 从未生效就是这个原因）。
    const setupMinimap = () => {
      try {
        const minimap = graph.getPluginInstance("minimap") as unknown as {
          container: HTMLElement;
        };
        if (!minimap?.container) return;
        const el = minimap.container;
        if (el.dataset.interactionFixed === "1") return; // 幂等：只装一次
        el.dataset.interactionFixed = "1";
        // 位置：用 left:0 / bottom:0 替代 G6 算的固定像素，resize 后仍左下角对齐
        el.style.right = "";
        el.style.top = "";
        el.style.left = "0px";
        el.style.bottom = "0px";
        // 视觉可见化：默认浅灰白底与画布背景几乎无色差，用户容易误认为
        // 画布空白区域而点击（表现为"点空白无响应"）。加边框+半透明底。
        el.style.border = "1px solid rgba(148,163,184,0.45)";
        el.style.borderRadius = "6px";
        el.style.background = "rgba(148,163,184,0.12)";
        el.style.overflow = "hidden";
        // 点击事件透传：minimap 容器 pointer-events:auto 且不冒泡进场景图，
        // 主图收不到 canvas:click。小地图上的点击视为点空白：清高亮+关详情
        // （拖动导航不受影响，click 仅在非拖动时触发）。
        el.addEventListener("click", () => {
          setDetail(null);
          setEdgeDetail(null);
          clearHighlight(graphRef.current);
          emitSelection(null);
        });
      } catch {
        /* 兼容无 minimap 或 API 变更 */
      }
    };
    // on 而非 once：setData/重渲染时 G6 可能重建 minimap 容器（dataset 丢失），
    // dataset 幂等保护下重复调用安全
    graph.on("afterrender", setupMinimap);
    // 双保险：afterrender 若已错过（绑定晚于 render 完成），轮询兜底。
    // setupMinimap 幂等，检测到已装好即自动停止。
    {
      let tries = 0;
      const poll = () => {
        try {
          const minimap = graph.getPluginInstance("minimap") as unknown as {
            container: HTMLElement;
          };
          if (minimap?.container?.dataset?.interactionFixed === "1") return; // 已装好
        } catch {
          /* ignore */
        }
        setupMinimap();
        if (++tries < 20) setTimeout(poll, 300);
      };
      setTimeout(poll, 100);
    }

    return () => {
      resizeObserver.disconnect();
      graph.destroy();
      graphRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 数据变化时增量更新（内容签名去重：相同数据跳过 setData）
  const lastSignatureRef = useRef(0);
  useEffect(() => {
    const graph = graphRef.current;
    if (!graph) return;
    if (dataSignature === lastSignatureRef.current) return;
    lastSignatureRef.current = dataSignature;
    graph.setData(buildData());
    graph.render();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dataSignature, buildData, showDatatypes, theme]);

  // 布局切换
  const handleLayoutChange = (type: string) => {
    setLayoutType(type);
    // 持久化布局偏好
    if (storageKey) {
      try {
        localStorage.setItem(`concept-graph-layout-${storageKey}`, type);
      } catch {
        /* ignore */
      }
    }
    const graph = graphRef.current;
    if (!graph) return;
    // 清除持久化位置（用户点了布局切换，需要自动重排）
    if (storageKey) {
      try {
        localStorage.removeItem(`concept-graph-positions-${storageKey}`);
      } catch {
        /* ignore */
      }
    }
    const data = graph.getData();
    data.nodes?.forEach((n: Record<string, unknown>) => {
      (n.style as Record<string, unknown>).x = undefined;
      (n.style as Record<string, unknown>).y = undefined;
    });
    graph.setData(data);
    const layout: LayoutOptions = {
      type,
      preventOverlap: true,
      animation: false,
    };
    if (type === "dagre") {
      layout.rankdir = "TB";
      layout.nodesep = 60;
      layout.ranksep = 80;
    }
    // G6 v5：setLayout 只设 options，render() 才会应用新布局
    graph.setLayout(layout);
    graph.render();
  };

  const handleRelayout = () => handleLayoutChange(layoutType);

  const handleZoomIn = () => graphRef.current?.zoomBy(1.3, false);
  const handleZoomOut = () => graphRef.current?.zoomBy(0.77, false);
  const handleFitView = () => graphRef.current?.fitView();

  const showAside =
    !!sidebarBottom || !!detail || !!edgeDetail || !!editing || !!propView;

  // 全屏：写窗口像素盒 + 强制 G6 resize。
  // Tauri WebView 下 requestFullscreen / 父级内 % 高度都不可靠。
  const handleFullscreen = () => setIsFullscreen((v) => !v);

  const syncGraphSize = useCallback(() => {
    const graph = graphRef.current;
    const el = containerRef.current;
    const root = rootRef.current;
    if (!graph || !el) return;

    // 强制一次 reflow，确保刚写的 fixed 像素盒已参与布局
    if (root) void root.offsetHeight;

    const rect = el.getBoundingClientRect();
    let width = Math.floor(rect.width);
    let height = Math.floor(rect.height);

    // 全屏：若 flex 子项尚未拉满，按窗口像素直接算画布区（扣侧栏 + 画布顶边）
    if (isFullscreen) {
      const asideW = showAside ? 288 : 0; // w-72
      const top = Math.max(0, rect.top);
      const fsW = Math.floor(window.innerWidth - asideW);
      const fsH = Math.floor(window.innerHeight - top);
      if (fsW > 0 && (width < fsW * 0.9 || width <= 0)) width = fsW;
      if (fsH > 0 && (height < fsH * 0.9 || height <= 0)) height = fsH;
      // 同步撑开容器，避免 G6 canvas 小于外层白底
      el.style.width = `${width}px`;
      el.style.height = `${height}px`;
    } else {
      el.style.width = "";
      el.style.height = "";
    }

    if (width <= 0 || height <= 0) return;
    try {
      graph.resize(width, height);
      void graph.fitView();
    } catch {
      /* destroy 竞态忽略 */
    }
  }, [isFullscreen, showAside]);

  // Ctrl+F 搜索；Esc：先退出全屏，再关弹层
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setSearchOpen(true);
        setTimeout(() => searchInputRef.current?.focus(), 50);
      }
      if (e.key === "Escape") {
        if (isFullscreen) {
          setIsFullscreen(false);
          return;
        }
        if (detail) setDetail(null);
        setEdgeDetail(null);
        clearHighlight(graphRef.current);
        emitSelection(null);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [detail, isFullscreen]);
  const handleExportPng = async () => {
    const graph = graphRef.current;
    if (!graph) return;
    const url = await graph.toDataURL({ mode: "overall", type: "image/png" });
    const a = document.createElement("a");
    a.href = url;
    a.download = `ontology-graph-${Date.now()}.png`;
    a.click();
  };

  const handleExportSvg = async () => {
    const graph = graphRef.current;
    if (!graph) return;
    const url = await graph.toDataURL({ mode: "overall", type: "image/png" });
    // 将 PNG 内嵌到 SVG 中（位图矢量外壳）
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100%" height="100%" viewBox="0 0 ${graph.getSize()[0]} ${graph.getSize()[1]}">
  <image width="100%" height="100%" href="${url}"/>
</svg>`;
    const blob = new Blob([svg], { type: "image/svg+xml" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `ontology-graph-${Date.now()}.svg`;
    a.click();
    URL.revokeObjectURL(a.href);
  };

  const nodeCount = nodes.length;
  const edgeCount = edges.length;

  // 图例计数：按深度分层 + 按类型统计
  const depthMap = useMemo(() => computeDepth(nodes, edges), [nodes, edges]);
  const classCounts = useMemo(() => {
    let root = 0;
    let sub = 0;
    let deep = 0;
    for (const n of nodes) {
      if (n.kind !== "class") continue;
      const d = depthMap.get(n.id) ?? 0;
      if (d === 0) root++;
      else if (d <= 2) sub++;
      else deep++;
    }
    return { root, sub, deep };
  }, [nodes, depthMap]);
  const datatypeCount = nodes.filter((n) => n.kind === "datatype").length;
  const hierarchyCount = edges.filter((e) => e.kind === "hierarchy").length;
  const objectCount = edges.filter((e) => e.kind === "object").length;

  // 全屏：直接写窗口像素尺寸（避开 % / vh / 父级 overflow 链），再 resize G6
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    if (!isFullscreen) {
      root.style.position = "";
      root.style.top = "";
      root.style.left = "";
      root.style.right = "";
      root.style.bottom = "";
      root.style.width = "";
      root.style.height = "";
      root.style.zIndex = "";
      root.style.background = "";
      root.style.margin = "";
      syncGraphSize();
      const t1 = window.setTimeout(syncGraphSize, 50);
      const t2 = window.setTimeout(syncGraphSize, 150);
      return () => {
        window.clearTimeout(t1);
        window.clearTimeout(t2);
      };
    }

    const applyFsBox = () => {
      const w = window.innerWidth;
      const h = window.innerHeight;
      root.style.position = "fixed";
      root.style.top = "0";
      root.style.left = "0";
      root.style.right = "auto";
      root.style.bottom = "auto";
      root.style.width = `${w}px`;
      root.style.height = `${h}px`;
      root.style.zIndex = "9999";
      root.style.background = canvasBackground(theme);
      root.style.margin = "0";
      syncGraphSize();
    };
    applyFsBox();
    const t1 = window.setTimeout(applyFsBox, 50);
    const t2 = window.setTimeout(applyFsBox, 200);
    window.addEventListener("resize", applyFsBox);
    return () => {
      window.clearTimeout(t1);
      window.clearTimeout(t2);
      window.removeEventListener("resize", applyFsBox);
    };
  }, [isFullscreen, theme, syncGraphSize]);

  // 右栏开合只触发 resize，不碰全屏定位样式
  useLayoutEffect(() => {
    syncGraphSize();
  }, [showAside, syncGraphSize]);

  return (
    <div
      ref={rootRef}
      className={`flex min-h-0 flex-row ${
        isFullscreen ? "" : `relative h-full w-full ${className ?? ""}`
      }`}
    >
      {/* 全屏根节点已是窗口像素高，子项用 h-full 可拉满；非全屏仍靠 flex stretch */}
      <div
        className={`flex min-h-0 min-w-0 flex-1 flex-col self-stretch ${
          isFullscreen ? "h-full" : ""
        }`}
      >
      {/* 顶栏：图例 + 布局切换 + 操作 */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-bg-hover/30 px-3 py-1.5 text-[10px] text-fg-subtle">
        <span className="inline-flex items-center gap-1">
          <svg
            width="12"
            height="12"
            viewBox="0 0 12 12"
            className="inline-block shrink-0"
          >
            <circle
              cx="6"
              cy="6"
              r="4.5"
              fill="#f1f5f9"
              stroke="#64748b"
              strokeWidth="1"
            />
          </svg>
          根类 {classCounts.root}
        </span>
        <span className="inline-flex items-center gap-1">
          <svg
            width="12"
            height="12"
            viewBox="0 0 12 12"
            className="inline-block shrink-0"
          >
            <circle
              cx="6"
              cy="6"
              r="4.5"
              fill="#f8fafc"
              stroke="#94a3b8"
              strokeWidth="1"
            />
          </svg>
          子类 {classCounts.sub}
        </span>
        <span className="inline-flex items-center gap-1">
          <svg
            width="12"
            height="12"
            viewBox="0 0 12 12"
            className="inline-block shrink-0"
          >
            <circle
              cx="6"
              cy="6"
              r="4.5"
              fill="#ffffff"
              stroke="#cbd5e1"
              strokeWidth="1"
            />
          </svg>
          深层类 {classCounts.deep}
        </span>
        <span className="ml-1 inline-flex items-center gap-1">
          <svg
            width="38"
            height="12"
            viewBox="0 0 38 12"
            className="inline-block shrink-0"
          >
            <line
              x1="1"
              y1="6"
              x2="18"
              y2="6"
              stroke="#d97706"
              strokeWidth="1"
              strokeDasharray="4 3"
            />
            <polygon points="18,3 24,6 18,9" fill="#d97706" />
            <circle
              cx="32"
              cy="6"
              r="4.5"
              fill="#fffbeb"
              stroke="#d97706"
              strokeWidth="1"
            />
          </svg>
          数据属性 {datatypeCount}
        </span>
        <span className="ml-1 inline-flex items-center gap-1">
          <svg
            width="26"
            height="12"
            viewBox="0 0 26 12"
            className="inline-block shrink-0"
          >
            <line
              x1="1"
              y1="6"
              x2="13"
              y2="6"
              stroke="#cbd5e1"
              strokeWidth="1"
              strokeDasharray="5 4"
            />
            <polygon points="13,3.5 18,6 13,8.5" fill="#cbd5e1" />
          </svg>
          subClassOf {hierarchyCount}
        </span>
        <span className="inline-flex items-center gap-1">
          <svg
            width="26"
            height="12"
            viewBox="0 0 26 12"
            className="inline-block shrink-0"
          >
            <line
              x1="1"
              y1="6"
              x2="13"
              y2="6"
              stroke="#64748b"
              strokeWidth="1"
            />
            <polygon points="13,3 19,6 13,9" fill="#64748b" />
          </svg>
          ObjectProperty {objectCount}
        </span>
        <span className="ml-auto inline-flex items-center gap-1 text-fg-subtle/60">
          {nodeCount} 节点 · {edgeCount} 边
        </span>
      </div>

      {/* 工具栏 */}
      <div className="flex shrink-0 items-center gap-1 border-b border-border px-2 py-1">
        <select
          className="form-select h-6 rounded border border-border bg-transparent px-1 text-[11px] outline-none"
          value={layoutType}
          onChange={(e) => handleLayoutChange(e.target.value)}
          aria-label="布局算法"
        >
          <option
            value="concentric"
            title="同深度节点在同一个圆环上，展示深度分布"
          >
            同心圆
          </option>
          <option value="force" title="物理模拟，节点自动聚簇，适合复杂网络">
            力导向
          </option>
          <option value="dagre" title="分层排列，父上子下，适合查看继承树">
            层级树
          </option>
          <option value="circular" title="所有节点均匀分布在圆环上">
            环形
          </option>
          <option value="grid" title="按网格均匀排列，适合扁平结构">
            网格
          </option>
        </select>
        <button
          type="button"
          onClick={handleRelayout}
          className="rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title="重排"
        >
          ↻ 重排
        </button>
        {/* 搜索定位 */}
        <div className="relative">
          {searchOpen ? (
            <div className="flex items-center gap-0.5">
              <input
                ref={searchInputRef}
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    setSearchOpen(false);
                    setSearchQuery("");
                  }
                  if (e.key === "Enter" && searchResults.length > 0) {
                    handleSearchSelect(searchResults[0].id);
                  }
                }}
                placeholder="搜索类/属性…"
                className="h-5 w-28 rounded border border-border bg-transparent px-1 text-[11px] outline-none placeholder:text-fg-subtle/40"
              />
              <button
                type="button"
                onClick={() => {
                  setSearchOpen(false);
                  setSearchQuery("");
                }}
                className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover"
              >
                <X size={10} />
              </button>
              {searchQuery.trim() && searchResults.length > 0 && (
                <div className="absolute left-0 top-full z-20 mt-1 max-h-48 w-52 overflow-y-auto rounded border border-border bg-surface p-1 shadow-lg">
                  {searchResults.map((n) => (
                    <button
                      key={n.id}
                      type="button"
                      onClick={() => handleSearchSelect(n.id)}
                      className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-[11px] text-left hover:bg-bg-hover"
                    >
                      <span
                        className={`inline-block h-1.5 w-1.5 shrink-0 rounded-${
                          n.kind === "class" ? "full" : "sm"
                        } ${n.kind === "class" ? "bg-blue-500" : "bg-amber-500"}`}
                      />
                      <span className="truncate">{n.label}</span>
                      <span className="ml-auto shrink-0 text-fg-subtle/50">
                        {n.kind === "class" ? "类" : "属性"}
                      </span>
                    </button>
                  ))}
                </div>
              )}
              {searchQuery.trim() && searchResults.length === 0 && (
                <div className="absolute left-0 top-full z-20 mt-1 w-52 rounded border border-border bg-surface p-2 text-[11px] text-fg-subtle/60 shadow-lg">
                  无匹配
                </div>
              )}
            </div>
          ) : (
            <button
              type="button"
              onClick={handleSearchOpen}
              className="rounded px-1 py-0.5 text-[11px] transition hover:bg-bg-hover"
              title="搜索节点 (Ctrl+F)"
            >
              <Search size={10} />
            </button>
          )}
        </div>
        <button
          type="button"
          onClick={() => setShowDatatypes((v) => !v)}
          className={`rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover ${
            showDatatypes ? "" : "opacity-50"
          }`}
          title="显示/隐藏 DatatypeProperty 节点"
        >
          {showDatatypes ? "☑ 属性" : "☐ 属性"}
        </button>
        <span className="mx-0.5 h-3 w-px bg-border" />
        <button
          type="button"
          onClick={handleZoomIn}
          className="inline-flex items-center rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title="放大"
        >
          <Plus size={11} />
        </button>
        <button
          type="button"
          onClick={handleZoomOut}
          className="inline-flex items-center rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title="缩小"
        >
          <Minus size={11} />
        </button>
        <button
          type="button"
          onClick={handleFitView}
          className="rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title="自适应"
        >
          ⤢ 自适应
        </button>
        <span className="mx-0.5 h-3 w-px bg-border" />
        <button
          type="button"
          onClick={handleExportPng}
          className="rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title="导出 PNG"
        >
          ⤓ PNG
        </button>
        <button
          type="button"
          onClick={handleExportSvg}
          className="rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title="导出 SVG（位图内嵌）"
        >
          ⤓ SVG
        </button>
        <span className="mx-0.5 h-3 w-px bg-border" />
        <button
          type="button"
          onClick={handleFullscreen}
          className="rounded px-1 py-0.5 text-[11px] transition hover:bg-bg-hover"
          title={isFullscreen ? "退出全屏" : "全屏"}
        >
          {isFullscreen ? <Minimize size={10} /> : <Maximize size={10} />}
        </button>
      </div>

      {/* 画布：全屏时由 syncGraphSize 按窗口像素撑开 container，避免 flex 链塌缩 */}
      <div className="relative min-h-0 flex-1 overflow-hidden">
        <div
          ref={containerRef}
          className={isFullscreen ? "" : "absolute inset-0"}
          style={
            isFullscreen
              ? { position: "absolute", top: 0, left: 0 }
              : undefined
          }
        />
        {nodeCount === 0 && (
          <div className="absolute inset-0 flex items-center justify-center text-xs text-fg-subtle/50">
            该本体无类/属性
          </div>
        )}
      </div>
      </div>

      {showAside && (
      <aside
        className={`flex min-h-0 w-72 shrink-0 flex-col self-stretch border-l border-border bg-bg ${
          isFullscreen ? "h-full" : ""
        }`}
      >
      {/* 上段：实体/边详情或编辑（有内容才渲染，避免空块抢走清单高度） */}
      {(detail || edgeDetail || editing || propView) && (
      <div
        className={`min-h-0 ${
          sidebarBottom
            ? editing
              ? // 编辑表单字段多（label/comment/父类列表），比查看卡需要更多高度；
                // 外层交给面板内部的表单区滚动，这里只做高度约束，避免双层滚动。
                // 必须是 flex 容器，内部 `flex-1` 的表单区才能撑开并独立滚动。
                "flex max-h-[78%] shrink-0 flex-col overflow-hidden border-b border-border"
              : "max-h-[58%] shrink-0 overflow-y-auto border-b border-border"
            : // 无变更清单时右侧只有这一块：不能拉满窗口（否则 flex 撑高后
              // 表单区用 flex-1 吸收剩余空间，页脚被推离内容掉到窗口底部）。
              // max-h-full 兜住 aside 高度：内容短则自然高度（页脚紧跟表单，
              // 不再掉到窗口底部）；内容长才滚动。
              "max-h-full shrink-0 overflow-y-auto"
        }`}
      >
      {detail && !editing && detail.node.kind === "class" &&
        (() => {
          const classIri = detail.node.iri;
          const relatedProps = (propsQ.data ?? []).filter(
            (p) =>
              iriLooseEq(p.domain ?? "", classIri) ||
              iriLooseEq(p.range ?? "", classIri),
          );
          const attrs = relatedProps.filter((p) => p.kind === "datatype");
          const rels = relatedProps.filter((p) => p.kind === "object");
          const parents = edges
            .filter((e) => e.kind === "hierarchy" && e.source === detail.node.id)
            .map((e) => nodes.find((n) => n.id === e.target))
            .filter(Boolean) as ConceptNode[];
          const children = edges
            .filter((e) => e.kind === "hierarchy" && e.target === detail.node.id)
            .map((e) => nodes.find((n) => n.id === e.source))
            .filter(Boolean) as ConceptNode[];
          const labelOfIri = (iri: string) => {
            const n = nodes.find((x) => iriLooseEq(x.iri, iri));
            if (n?.label) return n.label;
            const meta = (classMetasQ.data ?? []).find((m) =>
              iriLooseEq(m.iri, iri),
            );
            return meta?.label || iri.split(/[#/]/).pop() || iri;
          };
          return (
            <div
              className="p-3"
              onClick={(e) => e.stopPropagation()}
            >
              <div className="flex items-center justify-between">
                <h3 className="text-sm font-semibold">{detail.node.label}</h3>
                <button
                  type="button"
                  onClick={closeDetail}
                  className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover"
                >
                  <X size={12} />
                </button>
              </div>
              <div className="mt-1 text-[11px] text-fg-subtle">owl:Class</div>
              <div className="mt-1 break-all text-[10px] text-fg-subtle/60">
                {detail.node.iri}
              </div>
              {anchorGateMsg && (
                <p className="mt-2 text-[10px] text-amber-700">{anchorGateMsg}</p>
              )}

              <div className="mt-2 max-h-40 space-y-2 overflow-y-auto border-t border-border pt-1.5">
                <div>
                  <div className="mb-0.5 text-[11px] font-medium text-fg-subtle">
                    属性
                    <span className="ml-1 text-fg-subtle/60">({attrs.length})</span>
                  </div>
                  {attrs.length === 0 ? (
                    <p className="text-[11px] text-fg-subtle/60">无</p>
                  ) : (
                    attrs.map((p) => (
                      <button
                        key={p.iri}
                        type="button"
                        className="flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-[11.5px] text-fg hover:bg-bg-hover"
                        onClick={() => applyPropertySelect(p.iri, "datatype")}
                      >
                        <span className="truncate">
                          {p.label || p.iri.split(/[#/]/).pop()}
                        </span>
                        {p.range && (
                          <span className="ml-auto shrink-0 truncate text-fg-subtle">
                            → {p.range.split(/[#/]/).pop()}
                          </span>
                        )}
                      </button>
                    ))
                  )}
                </div>
                <div>
                  <div className="mb-0.5 text-[11px] font-medium text-fg-subtle">
                    关系
                    <span className="ml-1 text-fg-subtle/60">({rels.length})</span>
                  </div>
                  {rels.length === 0 ? (
                    <p className="text-[11px] text-fg-subtle/60">无</p>
                  ) : (
                    rels.map((p) => (
                      <button
                        key={p.iri}
                        type="button"
                        className="flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-[11.5px] text-fg hover:bg-bg-hover"
                        onClick={() => applyPropertySelect(p.iri, "object")}
                      >
                        <span className="truncate">
                          {p.label || p.iri.split(/[#/]/).pop()}
                        </span>
                        <span className="ml-auto shrink-0 truncate text-fg-subtle">
                          → {labelOfIri(p.range || "")}
                        </span>
                      </button>
                    ))
                  )}
                </div>
                <div>
                  <div className="mb-0.5 text-[11px] font-medium text-fg-subtle">
                    继承
                    <span className="ml-1 text-fg-subtle/60">
                      ({parents.length + children.length})
                    </span>
                  </div>
                  {parents.length === 0 && children.length === 0 ? (
                    <p className="text-[11px] text-fg-subtle/60">无</p>
                  ) : (
                    <>
                      {parents.map((n) => (
                        <div
                          key={`p-${n.id}`}
                          className="truncate px-1 py-0.5 text-[11.5px] text-fg-subtle"
                        >
                          ⊑ {n.label}
                        </div>
                      ))}
                      {children.map((n) => (
                        <div
                          key={`c-${n.id}`}
                          className="truncate px-1 py-0.5 text-[11.5px] text-fg-subtle"
                        >
                          {n.label} ⊑ …
                        </div>
                      ))}
                    </>
                  )}
                </div>
              </div>

              <div className="mt-2 flex flex-wrap gap-1 border-t border-border pt-1.5 [&>button]:whitespace-nowrap">
                {ontologyIri && (
                  <>
                    <button
                      type="button"
                      onClick={() => openClassEdit(detail.node.iri)}
                      className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                    >
                      <Pencil size={11} /> 编辑
                    </button>
                    <button
                      type="button"
                      onClick={() => deleteClassFromCard(detail.node.iri)}
                      disabled={deleteClass.isPending}
                      className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] text-red-600 transition hover:bg-bg-hover disabled:opacity-40"
                    >
                      <Trash2 size={11} /> 删除
                    </button>
                  </>
                )}
                <button
                  type="button"
                  onClick={() => {
                    navigator.clipboard
                      ?.writeText(detail.node.iri)
                      .catch(() => {});
                  }}
                  className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                >
                  <Copy size={11} /> 复制 IRI
                </button>
                {onJumpToSource && (
                  <button
                    type="button"
                    onClick={() => onJumpToSource(detail.node.iri)}
                    className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                  >
                    <FileCode2 size={11} /> 源码
                  </button>
                )}
              </div>
            </div>
          );
        })()}

      {/* 属性 / 关系查看卡 */}
      {propView && !editing && !detail && (
        <div className="p-3" onClick={(e) => e.stopPropagation()}>
          {(() => {
            const prop = (propsQ.data ?? []).find((x) => x.iri === propView.iri);
            const isRel = propView.kind === "object";
            const title =
              prop?.label || propView.iri.split(/[#/]/).pop() || propView.iri;
            const canBack = !!(anchorClassIri || prop?.domain);
            return (
              <>
                <div className="flex items-center justify-between gap-1">
                  <div className="flex min-w-0 items-center gap-1">
                    {canBack && (
                      <button
                        type="button"
                        onClick={() => backFromPropView(prop?.domain)}
                        className="inline-flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 text-[10px] text-fg-subtle hover:bg-bg-hover hover:text-fg"
                        title="返回所属类"
                      >
                        <ArrowLeft size={12} />
                        返回
                      </button>
                    )}
                    <h3 className="truncate text-sm font-semibold">{title}</h3>
                  </div>
                  <button
                    type="button"
                    onClick={closeDetail}
                    className="shrink-0 rounded p-0.5 text-fg-subtle hover:bg-bg-hover"
                    title="关闭"
                  >
                    <X size={12} />
                  </button>
                </div>
                <div className="mt-1 text-[11px] text-fg-subtle">
                  {isRel ? "关系（ObjectProperty）" : "属性（DatatypeProperty）"}
                </div>
                <div className="mt-1 break-all text-[10px] text-fg-subtle/60">
                  {propView.iri}
                </div>
                {anchorGateMsg && (
                  <p className="mt-2 text-[11px] text-amber-700">{anchorGateMsg}</p>
                )}
                <div className="mt-2 space-y-0.5 text-[11.5px] text-fg-subtle">
                  {prop?.domain && (
                    <div className="truncate">
                      所属类：{prop.domain.split(/[#/]/).pop()}
                    </div>
                  )}
                  {prop?.range && (
                    <div className="truncate">
                      {isRel ? "目标类" : "数据类型"}：
                      {prop.range.split(/[#/]/).pop()}
                    </div>
                  )}
                  {prop?.comment && (
                    <div className="mt-1 text-[11.5px] text-fg">{prop.comment}</div>
                  )}
                </div>
                {ontologyIri && (
                  <div className="mt-2 flex gap-1 border-t border-border pt-1.5">
                    <button
                      type="button"
                      onClick={() =>
                        openPropertyEdit(propView.iri, propView.kind)
                      }
                      className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                    >
                      <Pencil size={11} /> 编辑
                    </button>
                    <button
                      type="button"
                      onClick={() => deletePropertyFromCard(propView.iri)}
                      disabled={deleteProp.isPending}
                      className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] text-red-600 transition hover:bg-bg-hover disabled:opacity-40"
                    >
                      <Trash2 size={11} /> 删除
                    </button>
                    {onJumpToSource && (
                      <button
                        type="button"
                        onClick={() => onJumpToSource(propView.iri)}
                        className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                      >
                        <FileCode2 size={11} /> 源码
                      </button>
                    )}
                  </div>
                )}
              </>
            );
          })()}
        </div>
      )}

      {/* 边详情：关系/属性定义 查看 + 编辑/删除入口 */}
      {edgeDetail && !editing && !detail && (
        <div
          className="p-3"
          onClick={(e) => e.stopPropagation()}
        >
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-semibold">
              {edgeDetail.label || edgeDetail.kindText || edgeDetail.kind}
            </h3>
            <button
              type="button"
              onClick={() => {
                setEdgeDetail(null);
                emitSelection(null);
              }}
              className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover"
            >
              <X size={12} />
            </button>
          </div>
          <div className="mt-1 text-[11px] text-fg-subtle">
            {edgeDetail.kind === "hierarchy"
              ? "父子关系（rdfs:subClassOf）"
              : edgeDetail.kind === "object"
                ? "对象属性定义（domain → range）"
                : "数据属性定义（domain → 数据类型）"}
          </div>
          <div className="mt-2 space-y-0.5 text-[11px] text-fg-subtle/70">
            <div className="truncate">源：{nodes.find((n) => n.id === edgeDetail.source)?.iri ?? edgeDetail.source}</div>
            <div className="truncate">目标：{nodes.find((n) => n.id === edgeDetail.target)?.iri ?? edgeDetail.target}</div>
            {edgeDetail.predIri && <div className="truncate">谓词：{edgeDetail.predIri}</div>}
          </div>
          {ontologyIri && (
            <div className="mt-2 flex gap-1 border-t border-border pt-1.5">
              {edgeDetail.kind === "hierarchy" ? (
                <button
                  type="button"
                  onClick={() => openClassEdit(edgeDetail.source)}
                  className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                >
                  <Pencil size={11} /> 编辑父类
                </button>
              ) : (
                edgeDetail.predIri && (
                  <>
                    <button
                      type="button"
                      onClick={() =>
                        openPropertyEdit(edgeDetail.predIri!, edgeDetail.kind === "object" ? "object" : "datatype")
                      }
                      className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover"
                    >
                      <Pencil size={11} /> 编辑
                    </button>
                    <button
                      type="button"
                      onClick={() => deletePropertyFromCard(edgeDetail.predIri!)}
                      disabled={deleteProp.isPending}
                      className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] text-red-600 transition hover:bg-bg-hover disabled:opacity-40"
                    >
                      <Trash2 size={11} /> 删除
                    </button>
                  </>
                )
              )}
            </div>
          )}
        </div>
      )}

      {/* 编辑面板：类（label/comment/父类）或属性（label/comment/domain/range）
          纵向 flex 布局。有变更清单时撑满父容器（页脚贴底、表单区内滚）；
          无变更清单时父容器按内容自适应，此时不能再 h-full（会塌缩/撑高），
          改为随内容自然高度。 */}
      {editing && (
        <div
          className={`flex min-h-0 flex-col p-3 ${
            sidebarBottom ? "h-full" : ""
          }`}
          onClick={(e) => e.stopPropagation()}
        >
          <div className="flex shrink-0 items-center justify-between">
            <h3 className="text-sm font-semibold">
              {editing.type === "class"
                ? "编辑类"
                : editing.kind === "object"
                  ? "编辑关系"
                  : "编辑属性"}
            </h3>
            <button
              type="button"
              onClick={cancelEdit}
              className="rounded p-0.5 text-fg-subtle hover:bg-bg-hover"
              title="关闭"
            >
              <X size={12} />
            </button>
          </div>
          <div className="mt-1 shrink-0 break-all text-[10px] text-fg-subtle/60">{editing.iri}</div>

          <div className={`mt-2 min-h-0 space-y-2 overflow-y-auto text-[13px] ${
            sidebarBottom ? "flex-1" : ""
          }`}>
            <label className="block">
              <span className="text-[11px] text-fg-subtle">rdfs:label</span>
              <input
                className="mt-0.5 w-full rounded border border-border bg-transparent px-1.5 py-1 text-[13px] outline-none focus:border-accent"
                value={form.label}
                onChange={(e) => setForm({ ...form, label: e.target.value })}
                placeholder="中文名称"
              />
            </label>
            <label className="block">
              <span className="text-[11px] text-fg-subtle">rdfs:comment</span>
              <textarea
                className="mt-0.5 w-full rounded border border-border bg-transparent px-1.5 py-1 text-[13px] outline-none focus:border-accent"
                rows={2}
                value={form.comment}
                onChange={(e) => setForm({ ...form, comment: e.target.value })}
                placeholder="定义说明"
              />
            </label>

            {editing.type === "class" ? (
              <div>
                <span className="text-[11px] text-fg-subtle">父类（rdfs:subClassOf，可多选）</span>
                <div className="mt-1 max-h-28 space-y-0.5 overflow-y-auto rounded border border-border p-1">
                  {(classIrisQ.data ?? [])
                    .filter((c) => c !== editing.iri)
                    .map((c) => {
                      const short = c.split(/[#/]/).pop() ?? c;
                      return (
                        <label key={c} className="flex items-center gap-1.5 text-[12px]">
                          <input
                            type="checkbox"
                            checked={form.parents.includes(c)}
                            onChange={(e) =>
                              setForm({
                                ...form,
                                parents: e.target.checked
                                  ? [...form.parents, c]
                                  : form.parents.filter((x) => x !== c),
                              })
                            }
                          />
                          <span className="truncate">{short}</span>
                        </label>
                      );
                    })}
                </div>
              </div>
            ) : (
              <>
                <label className="block">
                  <span className="text-[11px] text-fg-subtle">
                    {editing.kind === "object" ? "源端类" : "所属类"}
                  </span>
                  <select
                    className="mt-0.5 w-full rounded border border-border bg-transparent px-1 py-1 text-[13px] outline-none"
                    value={form.domain}
                    onChange={(e) => setForm({ ...form, domain: e.target.value })}
                  >
                    <option value="">（未设置）</option>
                    {(classIrisQ.data ?? []).map((c) => (
                      <option key={c} value={c}>
                        {c.split(/[#/]/).pop()}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block">
                  <span className="text-[11px] text-fg-subtle">
                    {editing.kind === "object" ? "目标类" : "数据类型"}
                  </span>
                  {editing.kind === "object" ? (
                    <select
                      className="mt-0.5 w-full rounded border border-border bg-transparent px-1 py-1 text-[13px] outline-none"
                      value={form.range}
                      onChange={(e) => setForm({ ...form, range: e.target.value })}
                    >
                      <option value="">（未设置）</option>
                      {(classIrisQ.data ?? []).map((c) => (
                        <option key={c} value={c}>
                          {c.split(/[#/]/).pop()}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <input
                      className="mt-0.5 w-full rounded border border-border bg-transparent px-1.5 py-1 text-[13px] outline-none focus:border-accent"
                      value={form.range}
                      onChange={(e) => setForm({ ...form, range: e.target.value })}
                      placeholder="xsd:string"
                    />
                  )}
                </label>
                <p className="text-[11px] text-fg-subtle/70">
                  类型已固定为{editing.kind === "object" ? "关系" : "属性"}；若需改类型请删除后新建。
                </p>
              </>
            )}
          </div>

          {formErr && <p className="mt-2 text-[11px] text-red-600">{formErr}</p>}
          {formOk && <p className="mt-2 text-[11px] text-green-600">{formOk}</p>}

          {/* 页脚固定：不参与表单区滚动 */}
          <div className="mt-2 flex shrink-0 gap-1 border-t border-border pt-2">
            <button
              type="button"
              onClick={cancelEdit}
              disabled={editClass.isPending || editProp.isPending}
              className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] transition hover:bg-bg-hover disabled:opacity-40"
            >
              <X size={11} /> 取消
            </button>
            <button
              type="button"
              onClick={saveEdit}
              disabled={editClass.isPending || editProp.isPending}
              className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] font-medium text-accent transition hover:bg-accent/10 disabled:opacity-40"
            >
              <Check size={11} /> 保存
            </button>
          </div>
        </div>
      )}
      </div>
      )}
      {/* 下段：变更清单 */}
      {sidebarBottom && (
        <div className="flex min-h-0 flex-1 flex-col">{sidebarBottom}</div>
      )}
      </aside>
      )}
    </div>
  );
}
