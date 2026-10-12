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
  useRef,
  useMemo,
  useState,
  useSyncExternalStore,
} from "react";
import {
  Search,
  X,
  Info,
  ExternalLink,
  Copy,
  Share2,
  Maximize,
  Minimize,
  FileCode2,
  Plus,
  Minus,
} from "lucide-react";
import { Graph } from "@antv/g6";
import type { LayoutOptions } from "@antv/g6";
import {
  buildGraphData,
  canvasBackground,
  computeDepth,
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

interface ConceptGraphProps {
  nodes: ConceptNode[];
  edges: ConceptEdge[];
  /** 持久化键：拖拽后的节点位置按此键存 localStorage（同一本体固定不变） */
  storageKey?: string;
  className?: string;
  /** 图谱→Turtle 联动：点击节点跳转到 Turtle 源码定义行 */
  onJumpToSource?: (iri: string) => void;
}

export function ConceptGraph({
  nodes,
  edges,
  storageKey,
  className,
  onJumpToSource,
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

  // 属性开关（默认显示）
  const [showDatatypes, setShowDatatypes] = useState(true);
  // 搜索定位
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);

  const searchResults = useMemo(() => {
    if (!searchQuery.trim()) return [];
    const q = searchQuery.toLowerCase();
    const visible = showDatatypes
      ? nodes
      : nodes.filter((n) => n.kind !== "datatype");
    return visible
      .filter(
        (n) =>
          n.label.toLowerCase().includes(q) || n.id.toLowerCase().includes(q),
      )
      .slice(0, 20);
  }, [nodes, searchQuery, showDatatypes]);

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

  // 清除全部高亮（详情关闭 / Esc / 画布·边·minimap 点击时调用）
  const clearHighlight = (graph: Graph | null) => {
    if (!graph) return;
    const nodeUpdates = graph
      .getNodeData()
      .filter((n) => n.id)
      .map((n) => ({ id: n.id as string, style: { opacity: 1 } }));
    const edgeUpdates = graph
      .getEdgeData()
      .filter((e) => e.id)
      .map((e) => ({ id: e.id as string, style: { opacity: 1 } }));
    graph.updateNodeData(nodeUpdates);
    graph.updateEdgeData(edgeUpdates);
    try {
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

  const closeDetail = () => {
    setDetail(null);
    clearHighlight(graphRef.current);
  };

  // 构建 G6 数据（主题感知），稳定引用避免重复渲染
  const buildData = useCallback(
    () => buildGraphData(nodes, edges, theme, showDatatypes),
    [nodes, edges, theme, showDatatypes],
  );

  // 内容签名：相同节点/边数据跳过 setData+render
  const dataSignature = useMemo(() => {
    let h = 0;
    for (const n of nodes) h = ((h << 5) - h + n.id.length) | 0;
    for (const e of edges) h = ((h << 5) - h + e.id.length) | 0;
    // 显示开关/主题也参与签名：它们改变 buildData 输出（过滤/配色），
    // 但不改变 nodes/edges——不进签名会被增量更新去重跳过，
    // 画布不同步（例：取消勾选「属性」后属性节点残留）。
    h = ((h << 3) - h + (showDatatypes ? 1 : 0)) | 0;
    h = ((h << 3) - h + (theme === "dark" ? 2 : 0)) | 0;
    return h;
  }, [nodes, edges, showDatatypes, theme]);

  // dataRef：graph.on 回调（[] 闭包）中读取最新节点/边数据
  const dataRef = useRef({ nodes, edges });
  dataRef.current = { nodes, edges };

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
            const kindText =
              d.kindText ??
              (d.kind === "class" ? "owl:Class" : "owl:DatatypeProperty");
            return `<div style="font-size:11px;line-height:1.5">
              <div style="font-weight:600">${d.label ?? ""}</div>
              <div style="color:#64748b">${kindText}</div>
              <div style="color:#94a3b8;word-break:break-all;max-width:240px">${d.iri ?? ""}</div>
            </div>`;
          },
        },
      ],
      autoFit: "view",
      animation: false,
      background: canvasBackground(theme),
    });

    // ResizeObserver：容器尺寸变化时通知 G6；
    // 从 display:none（尺寸 0）切回可见时额外 fitView，保证图谱重新适配视口
    let lastSize = 0;
    const resizeObserver = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const area = entry.contentRect.width * entry.contentRect.height;
      if (area > 0) {
        graph.resize();
        // 容器 resize 后也重新定位 minimap（全屏/切回窗口）；
        // setupMinimap 幂等，重复调用安全
        setupMinimap();
        if (lastSize === 0) {
          graph.fitView();
        }
      }
      lastSize = area;
    });
    resizeObserver.observe(container);

    graph.on("node:click", (evt: unknown) => {
      const target = (evt as { target?: { id?: string } }).target;
      const nodeId = target?.id;
      const { nodes: curNodes } = dataRef.current;
      if (nodeId) {
        const node = curNodes.find((n) => n.id === nodeId);
        if (node) {
          setDetail({ node, x: 0, y: 0 });
          applyHighlight(graph, node.id);
        }
      } else {
        setDetail(null);
        clearHighlight(graph);
      }
    });

    graph.on("canvas:click", () => {
      setDetail(null);
      clearHighlight(graph);
    });

    // 边也视为空白：默认无 edge:click handler 时点边不会派发 canvas:click，
    // 用户点在边线上（尤其放大视图后边很密）同样表现为"点空白无响应"。
    graph.on("edge:click", () => {
      setDetail(null);
      clearHighlight(graph);
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
          clearHighlight(graphRef.current);
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

  // 全屏切换：对整个图谱容器请求/退出全屏
  const handleFullscreen = async () => {
    const root = rootRef.current;
    if (!root) return;
    try {
      if (document.fullscreenElement) {
        await document.exitFullscreen();
      } else {
        await root.requestFullscreen();
      }
    } catch {
      /* 浏览器拒绝时静默 */
    }
  };

  // 监听全屏状态变化：同步按钮图标，并在切换后重新自适应
  useEffect(() => {
    const onFsChange = () => {
      const fs = Boolean(document.fullscreenElement);
      setIsFullscreen(fs);
      if (fs) {
        // 等全屏布局稳定后重新 fitView
        setTimeout(() => graphRef.current?.fitView(), 100);
      }
    };
    document.addEventListener("fullscreenchange", onFsChange);
    return () => document.removeEventListener("fullscreenchange", onFsChange);
  }, []);

  // Ctrl+F / Cmd+F 打开图谱搜索；Esc 关闭弹层
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setSearchOpen(true);
        setTimeout(() => searchInputRef.current?.focus(), 50);
      }
      if (e.key === "Escape") {
        // 搜索定位后 detail 可能为 null，但仍需清除高亮
        if (detail) {
          setDetail(null);
        }
        clearHighlight(graphRef.current);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [detail]);
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

  // 计算节点邻居/边信息（用于详情面板）
  const neighborCache = useMemo(() => {
    const nei = new Map<
      string,
      { neighbors: ConceptNode[]; relatedEdges: ConceptEdge[] }
    >();
    for (const n of nodes) {
      const neighbors: ConceptNode[] = [];
      const relatedEdges: ConceptEdge[] = [];
      for (const e of edges) {
        if (e.source === n.id) {
          const t = nodes.find((nn) => nn.id === e.target);
          if (t) neighbors.push(t);
          relatedEdges.push(e);
        } else if (e.target === n.id) {
          const s = nodes.find((nn) => nn.id === e.source);
          if (s) neighbors.push(s);
          relatedEdges.push(e);
        }
      }
      nei.set(n.id, {
        neighbors: neighbors.slice(0, 20), // 最多20个邻居
        relatedEdges,
      });
    }
    return nei;
  }, [nodes, edges]);

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

  return (
    <div
      ref={rootRef}
      style={isFullscreen ? { background: canvasBackground(theme) } : undefined}
      className={`relative flex h-full w-full flex-col ${className ?? ""}`}
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
              fill="#4f46e5"
              stroke="#4338ca"
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
              fill="#2563eb"
              stroke="#1d4ed8"
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
              fill="#60a5fa"
              stroke="#3b82f6"
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
              stroke="#f59e0b"
              strokeWidth="1"
              strokeDasharray="4 3"
            />
            <polygon points="18,3 24,6 18,9" fill="#f59e0b" />
            <circle
              cx="32"
              cy="6"
              r="4.5"
              fill="#fef3c7"
              stroke="#f59e0b"
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
              stroke="#94a3b8"
              strokeWidth="1"
              strokeDasharray="5 3"
            />
            <polygon points="13,3.5 18,6 13,8.5" fill="#94a3b8" />
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
              stroke="#10b981"
              strokeWidth="1"
            />
            <polygon points="13,3 19,6 13,9" fill="#10b981" />
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

      {/* 画布 */}
      <div className="relative min-h-0 flex-1">
        <div ref={containerRef} className="h-full w-full" />
        {nodeCount === 0 && (
          <div className="absolute inset-0 flex items-center justify-center text-xs text-fg-subtle/50">
            该本体无类/属性
          </div>
        )}
      </div>

      {/* 详情弹层 */}
      {detail &&
        (() => {
          const info = neighborCache.get(detail.node.id);
          const neighbors = info?.neighbors ?? [];
          const relatedEdges = info?.relatedEdges ?? [];
          const edgeCount = relatedEdges.length;
          const neighborCount = neighbors.length;
          return (
            <div
              className="pointer-events-auto absolute right-3 top-16 z-20 w-64 rounded-lg border border-border bg-surface/98 p-3 shadow-lg backdrop-blur-sm"
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
              <div className="mt-1 text-[10px] text-fg-subtle">
                {detail.node.kindText ??
                  (detail.node.kind === "class"
                    ? "owl:Class"
                    : "owl:DatatypeProperty")}
              </div>
              <div className="mt-1 break-all text-[10px] text-fg-subtle/60">
                {detail.node.iri}
              </div>

              {/* 统计信息 */}
              <div className="mt-2 flex gap-3 border-t border-border pt-2 text-[10px] text-fg-subtle">
                <span className="inline-flex items-center gap-1">
                  <Info size={10} />
                  {neighborCount} 邻居
                </span>
                <span className="inline-flex items-center gap-1">
                  <Share2 size={10} />
                  {edgeCount} 边
                </span>
              </div>

              {/* 邻居列表 */}
              {neighbors.length > 0 && (
                <div className="mt-2 max-h-28 overflow-y-auto border-t border-border pt-1.5">
                  <div className="mb-1 text-[10px] font-medium text-fg-subtle">
                    关联节点
                  </div>
                  {neighbors.map((nb) => (
                    <div
                      key={nb.id}
                      className="flex items-center gap-1.5 rounded px-1 py-0.5 text-[10px] transition hover:bg-bg-hover"
                    >
                      <span
                        className={`inline-block h-1.5 w-1.5 shrink-0 rounded-${
                          nb.kind === "class" ? "full" : "sm"
                        } ${
                          nb.kind === "class" ? "bg-blue-500" : "bg-amber-500"
                        }`}
                      />
                      <span className="truncate">{nb.label}</span>
                      <span className="ml-auto shrink-0 text-fg-subtle/50">
                        {nb.kind === "class" ? "类" : "属性"}
                      </span>
                    </div>
                  ))}
                </div>
              )}

              {/* 操作按钮 */}
              <div className="mt-2 flex gap-1 border-t border-border pt-1.5">
                <button
                  type="button"
                  onClick={() => {
                    navigator.clipboard
                      ?.writeText(detail.node.iri)
                      .catch(() => {});
                  }}
                  className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] transition hover:bg-bg-hover"
                >
                  <Copy size={10} /> 复制 IRI
                </button>
                <button
                  type="button"
                  onClick={() => {
                    window.open(detail.node.iri, "_blank", "noopener");
                  }}
                  className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] transition hover:bg-bg-hover"
                >
                  <ExternalLink size={10} /> 打开
                </button>
                {onJumpToSource && (
                  <button
                    type="button"
                    onClick={() => {
                      onJumpToSource(detail.node.iri);
                    }}
                    className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[10px] transition hover:bg-bg-hover"
                  >
                    <FileCode2 size={10} /> 源码
                  </button>
                )}
              </div>
            </div>
          );
        })()}
    </div>
  );
}
