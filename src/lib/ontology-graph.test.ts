/**
 * 单元测试：ontology-graph.ts（本体概念图谱纯函数）
 * 运行：npm test（vitest run）
 *
 * 覆盖范围：
 *  - termToShort / labelOf（IRI 工具）
 *  - rowsToGraphData（SPARQL 行 → 节点/边：class/hierarchy/objectProperty/datatypeProperty 4 路 UNION）
 *  - computeDepth（subClassOf 层级深度计算）
 *  - depthStyle / nodeStyle / edgeStyle（主题感知样式工厂）
 *  - buildGraphData（组装 G6 GraphData：Datatype 过滤 + 深度染色）
 */
import { describe, expect, it } from "vitest";
import {
    buildGraphData,
    canvasBackground,
    computeDepth,
    depthStyle,
    edgeStyle,
    labelOf,
    nodeStyle,
    rowsToGraphData,
    termToShort,
} from "./ontology-graph";
import type { ConceptNode, ConceptEdge, GraphRow } from "./ontology-graph";

// ══════════════════════════════════════════════════════════════════════
// 1. termToShort
// ══════════════════════════════════════════════════════════════════════

describe("termToShort", () => {
    it("常规 # 分隔 IRI", () => {
        expect(termToShort("http://example.org#Person")).toBe("Person");
    });

    it("常规 / 分隔 IRI", () => {
        expect(termToShort("http://example.org/ontology/Person")).toBe("Person");
    });

    it("带尖括号的 IRI", () => {
        expect(termToShort("<http://example.org#Person>")).toBe("Person");
    });

    it("无法解析时返回原串", () => {
        expect(termToShort("Person")).toBe("Person");
    });

    it("空串返回空串", () => {
        expect(termToShort("")).toBe("");
    });

    it("只有 scheme 的 IRI", () => {
        expect(termToShort("http://example.org")).toBe("example.org");
    });

    it("多级路径取最后一段", () => {
        expect(termToShort("http://example.org/a/b/c")).toBe("c");
    });
});

// ══════════════════════════════════════════════════════════════════════
// 2. labelOf
// ══════════════════════════════════════════════════════════════════════

describe("labelOf", () => {
    const labelMap = new Map([
        ["http://example.org#Person", "人"],
        ["http://example.org#Animal", "动物"],
    ]);

    it("labelMap 命中时返回中文名", () => {
        expect(labelOf(labelMap, "http://example.org#Person")).toBe("人");
    });

    it("labelMap 未命中时回退到 IRI 短名", () => {
        expect(labelOf(labelMap, "http://example.org#Unknown")).toBe("Unknown");
    });

    it("带尖括号的 IRI", () => {
        expect(labelOf(labelMap, "<http://example.org#Person>")).toBe("人");
    });

    it("labelMap undefined 时回退", () => {
        expect(labelOf(undefined, "http://example.org#Foo")).toBe("Foo");
    });
});

// ══════════════════════════════════════════════════════════════════════
// 3. rowsToGraphData
// ══════════════════════════════════════════════════════════════════════

describe("rowsToGraphData", () => {
    const labelMap = new Map([
        ["http://example.org#Person", "人"],
        ["http://example.org#Animal", "动物"],
        ["http://example.org#hasName", "有名称"],
    ]);

    it("空行返回空", () => {
        const { nodes, edges } = rowsToGraphData([], labelMap);
        expect(nodes).toHaveLength(0);
        expect(edges).toHaveLength(0);
    });

    it("class 行生成节点", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
        ];
        const { nodes, edges } = rowsToGraphData(rows, labelMap);
        expect(nodes).toHaveLength(1);
        expect(nodes[0]).toMatchObject({
            id: "Person",
            label: "人",
            iri: "http://example.org#Person",
            kind: "class",
        });
        expect(edges).toHaveLength(0);
    });

    it("hierarchy 行生成层级边", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { s: "http://example.org#Animal", type: "class" },
            { s: "http://example.org#Person", o: "http://example.org#Animal", type: "hierarchy" },
        ];
        const { nodes, edges } = rowsToGraphData(rows, labelMap);
        expect(nodes).toHaveLength(2);
        expect(edges).toHaveLength(1);
        expect(edges[0]).toMatchObject({
            id: "h-Person-Animal",
            source: "Person",
            target: "Animal",
            kind: "hierarchy",
        });
    });

    it("hierarchy 自环边被忽略", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { s: "http://example.org#Person", o: "http://example.org#Person", type: "hierarchy" },
        ];
        const { edges } = rowsToGraphData(rows, labelMap);
        expect(edges).toHaveLength(0);
    });

    it("objectProperty 行生成带标签边", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { s: "http://example.org#Animal", type: "class" },
            { p: "http://example.org#hasName", s: "http://example.org#Person", o: "http://example.org#Animal", type: "objectProperty" },
        ];
        const { nodes, edges } = rowsToGraphData(rows, labelMap);
        expect(nodes).toHaveLength(2);
        expect(edges).toHaveLength(1);
        expect(edges[0]).toMatchObject({
            id: "o-hasName-Person-Animal",
            source: "Person",
            target: "Animal",
            label: "有名称",
            kind: "object",
        });
    });

    it("objectProperty 自环边被忽略", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { p: "http://example.org#selfRef", s: "http://example.org#Person", o: "http://example.org#Person", type: "objectProperty" },
        ];
        const { edges } = rowsToGraphData(rows, labelMap);
        expect(edges).toHaveLength(0);
    });

    it("datatypeProperty 行生成属性节点和琥珀边", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { p: "http://example.org#hasName", s: "http://example.org#Person", type: "datatypeProperty" },
        ];
        const { nodes, edges } = rowsToGraphData(rows, labelMap);
        const nodeIds = nodes.map((n) => n.id).sort();
        expect(nodeIds).toEqual(["Person", "hasName"]);
        const person = nodes.find((n) => n.id === "Person");
        expect(person?.kind).toBe("class");
        const hasName = nodes.find((n) => n.id === "hasName");
        expect(hasName?.kind).toBe("datatype");
        expect(hasName?.label).toBe("有名称");
        expect(edges).toHaveLength(1);
        expect(edges[0]).toMatchObject({
            id: "d-hasName-Person",
            source: "Person",
            target: "hasName",
            kind: "datatype",
        });
    });

    it("datatypeProperty 带 rdfs:range 时标签追加取值类型", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { p: "http://example.org#hasName", s: "http://example.org#Person", o: "http://www.w3.org/2001/XMLSchema#string", type: "datatypeProperty" },
        ];
        const { edges } = rowsToGraphData(rows, labelMap);
        expect(edges).toHaveLength(1);
        expect(edges[0].label).toBe("有名称 (string)");
    });

    it("datatypeProperty 无 rdfs:range 时标签不含括号", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { p: "http://example.org#hasName", s: "http://example.org#Person", type: "datatypeProperty" },
        ];
        const { edges } = rowsToGraphData(rows, labelMap);
        expect(edges[0].label).toBe("有名称");
    });

    it("datatypeProperty 自环边被忽略", () => {
        const rows: GraphRow[] = [
            { p: "http://example.org#selfRef", s: "http://example.org#selfRef", type: "datatypeProperty" },
        ];
        const { edges } = rowsToGraphData(rows, labelMap);
        expect(edges).toHaveLength(0);
    });

    it("class 去重：同一 IRI 只生成一个节点", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { s: "http://example.org#Person", type: "class" },
            { s: "http://example.org#Person", type: "class" },
        ];
        const { nodes } = rowsToGraphData(rows, labelMap);
        expect(nodes).toHaveLength(1);
    });

    it("综合：class + hierarchy + objectProperty + datatypeProperty 混合", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { s: "http://example.org#Animal", type: "class" },
            { s: "http://example.org#Dog", type: "class" },
            { s: "http://example.org#Dog", o: "http://example.org#Animal", type: "hierarchy" },
            { p: "http://example.org#hasName", s: "http://example.org#Person", o: "http://example.org#Animal", type: "objectProperty" },
            { p: "http://example.org#age", s: "http://example.org#Person", type: "datatypeProperty" },
        ];
        const { nodes, edges } = rowsToGraphData(rows, labelMap);
        expect(nodes).toHaveLength(4); // Person, Animal, Dog, age
        expect(edges).toHaveLength(3); // hierarchy, objectProperty, datatypeProperty
    });

    it("空 s 行被忽略", () => {
        const rows: GraphRow[] = [
            { s: "", type: "class" },
            { s: "http://example.org#Person", type: "class" },
        ];
        const { nodes } = rowsToGraphData(rows, labelMap);
        expect(nodes).toHaveLength(1);
    });
});

// ══════════════════════════════════════════════════════════════════════
// 4. computeDepth
// ══════════════════════════════════════════════════════════════════════

describe("computeDepth", () => {
    it("根类（无父节点）深度 0", () => {
        const nodes: ConceptNode[] = [{ id: "A", label: "A", iri: "", kind: "class" }];
        const edges: ConceptEdge[] = [];
        const depth = computeDepth(nodes, edges);
        expect(depth.get("A")).toBe(0);
    });

    it("简单链：A→B→C 深度为 0→1→2", () => {
        const nodes: ConceptNode[] = [
            { id: "A", label: "A", iri: "", kind: "class" },
            { id: "B", label: "B", iri: "", kind: "class" },
            { id: "C", label: "C", iri: "", kind: "class" },
        ];
        const edges: ConceptEdge[] = [
            { id: "e1", source: "B", target: "A", kind: "hierarchy" },
            { id: "e2", source: "C", target: "B", kind: "hierarchy" },
        ];
        const depth = computeDepth(nodes, edges);
        expect(depth.get("A")).toBe(0);
        expect(depth.get("B")).toBe(1);
        expect(depth.get("C")).toBe(2);
    });

    it("钻石继承：A 有两个子类 B、C，它们深度均为 1", () => {
        const nodes: ConceptNode[] = [
            { id: "A", label: "A", iri: "", kind: "class" },
            { id: "B", label: "B", iri: "", kind: "class" },
            { id: "C", label: "C", iri: "", kind: "class" },
        ];
        const edges: ConceptEdge[] = [
            { id: "e1", source: "B", target: "A", kind: "hierarchy" },
            { id: "e2", source: "C", target: "A", kind: "hierarchy" },
        ];
        const depth = computeDepth(nodes, edges);
        expect(depth.get("A")).toBe(0);
        expect(depth.get("B")).toBe(1);
        expect(depth.get("C")).toBe(1);
    });

    it("datatype 节点不参与深度计算（无深度值）", () => {
        const nodes: ConceptNode[] = [
            { id: "A", label: "A", iri: "", kind: "class" },
            { id: "age", label: "age", iri: "", kind: "datatype" },
        ];
        const edges: ConceptEdge[] = [];
        const depth = computeDepth(nodes, edges);
        expect(depth.get("A")).toBe(0);
        expect(depth.has("age")).toBe(false);
    });

    it("孤立节点深度 0", () => {
        const nodes: ConceptNode[] = [
            { id: "A", label: "A", iri: "", kind: "class" },
            { id: "B", label: "B", iri: "", kind: "class" },
        ];
        const edges: ConceptEdge[] = [];
        const depth = computeDepth(nodes, edges);
        expect(depth.get("A")).toBe(0);
        expect(depth.get("B")).toBe(0);
    });
});

// ══════════════════════════════════════════════════════════════════════
// 5. 样式工厂
// ══════════════════════════════════════════════════════════════════════

describe("canvasBackground", () => {
    it("light 返回白色背景", () => {
        expect(canvasBackground("light")).toBe("#ffffff");
    });

    it("dark 返回深色背景", () => {
        expect(canvasBackground("dark")).toBe("#0f172a");
    });
});

describe("depthStyle", () => {
    it("深度 0 返回根类色（light）", () => {
        const s = depthStyle("light", 0);
        expect(s.fill).toBe("#4f46e5");
        expect(s.stroke).toBe("#4338ca");
        expect(s.labelFill).toBe("#ffffff");
    });

    it("深度 3 返回 tier3 色（light）", () => {
        const s = depthStyle("light", 3);
        expect(s.fill).toBe("#93c5fd");
    });

    it("深度 4+ 兜底到 tier4", () => {
        const s = depthStyle("light", 10);
        expect(s.fill).toBe("#dbeafe");
    });

    it("dark 主题返回 dark 色", () => {
        const s = depthStyle("dark", 0);
        expect(s.fill).toBe("#6366f1");
    });
});

describe("nodeStyle", () => {
    it("class 节点返回圆形样式", () => {
        const s = nodeStyle("light", "class");
        expect(s.size).toBe(52);
        expect(s.fill).toBe("#dbeafe");
        expect(s.labelPlacement).toBe("center");
    });

    it("datatype 节点返回矩形样式", () => {
        const s = nodeStyle("light", "datatype");
        expect(s.size).toEqual([88, 32]);
        expect(s.fill).toBe("#fef3c7");
        expect(s.radius).toBe(6);
    });

    it("dark 主题 class 使用暗色", () => {
        const s = nodeStyle("dark", "class");
        expect(s.fill).toBe("#1e3a5f");
    });

    it("dark 主题 datatype 使用暗色", () => {
        const s = nodeStyle("dark", "datatype");
        expect(s.fill).toBe("#5c3d0e");
    });
});

describe("edgeStyle", () => {
    it("hierarchy 边无色虚线", () => {
        const s = edgeStyle("light", "hierarchy");
        expect(s.stroke).toBe("#94a3b8");
        expect(s.lineDash).toEqual([6, 4]);
        expect(s.endArrow).toBe(true);
    });

    it("object 边绿色实线", () => {
        const s = edgeStyle("light", "object");
        expect(s.stroke).toBe("#10b981");
        expect(s.lineDash).toBeUndefined();
    });

    it("datatype 边琥珀色虚线", () => {
        const s = edgeStyle("light", "datatype");
        expect(s.stroke).toBe("#f59e0b");
        expect(s.lineDash).toEqual([4, 3]);
    });

    it("dark 主题边颜色切换", () => {
        const s = edgeStyle("dark", "object");
        expect(s.stroke).toBe("#34d399");
    });
});

// ══════════════════════════════════════════════════════════════════════
// 6. buildGraphData
// ══════════════════════════════════════════════════════════════════════

describe("buildGraphData", () => {
    const nodes: ConceptNode[] = [
        { id: "Animal", label: "Animal", iri: "ex:Animal", kind: "class" },
        { id: "Dog", label: "Dog", iri: "ex:Dog", kind: "class" },
        { id: "age", label: "age", iri: "ex:age", kind: "datatype" },
    ];
    const edges: ConceptEdge[] = [
        { id: "h1", source: "Dog", target: "Animal", kind: "hierarchy" },
        { id: "d1", source: "Animal", target: "age", kind: "datatype" },
    ];

    it("showDatatypes=true 保留所有节点", () => {
        const data = buildGraphData(nodes, edges, "light", true);
        expect(data.nodes).toHaveLength(3);
        expect(data.edges).toHaveLength(2);
    });

    it("showDatatypes=false 过滤 datatype 节点及关联边", () => {
        const data = buildGraphData(nodes, edges, "light", false);
        const ids = data.nodes!.map((n: Record<string, unknown>) => n.id);
        expect(ids).toEqual(["Animal", "Dog"]);
        expect(data.edges).toHaveLength(1);
        expect(data.edges![0].id).toBe("h1");
    });

    it("类节点按深度染色（根深叶浅）", () => {
        const data = buildGraphData(nodes, edges, "light", true);
        const animal = data.nodes!.find((n: Record<string, unknown>) => n.id === "Animal");
        expect(animal?.style).toMatchObject({ fill: "#4f46e5" }); // depth 0
        const dog = data.nodes!.find((n: Record<string, unknown>) => n.id === "Dog");
        expect(dog?.style).toMatchObject({ fill: "#2563eb" }); // depth 1
    });

    it("datatype 节点保持琥珀色样式", () => {
        const data = buildGraphData(nodes, edges, "light", true);
        const age = data.nodes!.find((n: Record<string, unknown>) => n.id === "age");
        expect(age?.style).toMatchObject({ fill: "#fef3c7" });
    });

    it("hierarchy 边 labelText 兜底为 subClassOf", () => {
        const data = buildGraphData(nodes, edges, "light", true);
        const hEdge = data.edges!.find((e: Record<string, unknown>) => e.id === "h1");
        expect(hEdge?.style).toMatchObject({ labelText: "subClassOf" });
    });

    it("object 边 labelText 使用 edge.label", () => {
        const objNodes: ConceptNode[] = [
            { id: "A", label: "A", iri: "ex:A", kind: "class" },
            { id: "B", label: "B", iri: "ex:B", kind: "class" },
        ];
        const objEdges: ConceptEdge[] = [
            { id: "o1", source: "A", target: "B", label: "hasRelation", kind: "object" },
        ];
        const data = buildGraphData(objNodes, objEdges, "light", true);
        expect(data.edges![0].style).toMatchObject({ labelText: "hasRelation" });
    });

    it("datatype 边 labelText 使用 edge.label", () => {
        const dtNodes: ConceptNode[] = [
            { id: "A", label: "A", iri: "ex:A", kind: "class" },
            { id: "age", label: "age", iri: "ex:age", kind: "datatype" },
        ];
        const dtEdges: ConceptEdge[] = [
            { id: "d1", source: "A", target: "age", label: "age", kind: "datatype" },
        ];
        const data = buildGraphData(dtNodes, dtEdges, "light", true);
        expect(data.edges![0].style).toMatchObject({ labelText: "age" });
    });
});