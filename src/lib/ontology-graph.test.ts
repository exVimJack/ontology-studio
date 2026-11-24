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
 *  - filterDatatypeVisibility / iriLooseEq（按类展开 ∪ 全局开关）
 *  - buildClassDeleteConfirmMessage（删除类确认框摘要）
 */
import { describe, expect, it } from "vitest";
import {
    buildClassDeleteConfirmMessage,
    buildGraphData,
    canvasBackground,
    computeDepth,
    depthStyle,
    edgeStyle,
    filterDatatypeVisibility,
    iriLooseEq,
    labelOf,
    nodeStyle,
    rdfTermForKind,
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

    it("objectProperty 自环边（domain=range）应保留", () => {
        const rows: GraphRow[] = [
            { s: "http://example.org#Person", type: "class" },
            { p: "http://example.org#selfRef", s: "http://example.org#Person", o: "http://example.org#Person", type: "objectProperty" },
        ];
        const { edges } = rowsToGraphData(rows, labelMap);
        expect(edges).toHaveLength(1);
        expect(edges[0]).toMatchObject({
            source: "Person",
            target: "Person",
            kind: "object",
            predIri: "http://example.org#selfRef",
        });
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
    it("深度 0 返回根类浅底（light）", () => {
        const s = depthStyle("light", 0);
        expect(s.fill).toBe("#f1f5f9");
        expect(s.stroke).toBe("#64748b");
        expect(s.labelFill).toBe("#0f172a");
    });

    it("深度 3 返回 tier3 色（light）", () => {
        const s = depthStyle("light", 3);
        expect(s.fill).toBe("#ffffff");
    });

    it("深度 4+ 兜底到 tier4", () => {
        const s = depthStyle("light", 10);
        expect(s.fill).toBe("#ffffff");
    });

    it("dark 主题返回 dark 色", () => {
        const s = depthStyle("dark", 0);
        expect(s.fill).toBe("#334155");
    });
});

describe("nodeStyle", () => {
    it("class 节点返回圆形样式", () => {
        const s = nodeStyle("light", "class");
        expect(s.size).toBe(52);
        expect(s.fill).toBe("#ffffff");
        expect(s.labelPlacement).toBe("center");
    });

    it("datatype 节点返回矩形样式", () => {
        const s = nodeStyle("light", "datatype");
        expect(s.size).toEqual([88, 32]);
        expect(s.fill).toBe("#fffbeb");
        expect(s.radius).toBe(6);
    });

    it("dark 主题 class 使用暗色", () => {
        const s = nodeStyle("dark", "class");
        expect(s.fill).toBe("#1e293b");
    });

    it("dark 主题 datatype 使用暗色", () => {
        const s = nodeStyle("dark", "datatype");
        expect(s.fill).toBe("#422006");
    });
});

describe("edgeStyle", () => {
    it("hierarchy 边浅灰虚线", () => {
        const s = edgeStyle("light", "hierarchy");
        expect(s.stroke).toBe("#cbd5e1");
        expect(s.lineDash).toEqual([5, 4]);
        expect(s.lineWidth).toBe(1);
        expect(s.endArrow).toBe(true);
    });

    it("object 边细灰蓝实线（不为翠绿，避免与新增绿冲突）", () => {
        const s = edgeStyle("light", "object");
        expect(s.stroke).toBe("#64748b");
        expect(s.lineWidth).toBe(1.25);
        expect(s.lineDash).toEqual([]);   // 显式实心：缺键会让上一帧虚线残留在画布上
    });


    it("无差异元素也显式写 lineDash/opacity（防上一帧虚线残留）", () => {
        // 真机现象：撤销删除后节点已无差异，描边颜色回到灰色，但虚线留着 ——
        // 因为无差异时 style 里根本没有 lineDash 键，G6 只更新给到的属性、不会清除旧值。
        const n: ConceptNode[] = [
            { id: "P", label: "P", iri: "ex:P", kind: "class" },
            { id: "D", label: "D", iri: "ex:D", kind: "datatype" },
        ];
        const e: ConceptEdge[] = [
            { id: "e1", source: "P", target: "D", kind: "object" },
        ];
        const data = buildGraphData(n, e, "light", true);
        for (const raw of data.nodes!) {
            const s = (raw as Record<string, unknown>).style as Record<string, unknown>;
            expect(s.lineDash).toEqual([]);
            expect(s.opacity).toBe(1);
        }
        const es = (data.edges![0] as Record<string, unknown>).style as Record<string, unknown>;
        expect(es.lineDash).toEqual([]);
        expect(es.opacity).toBe(1);
    });
    it("datatype 边琥珀色虚线", () => {
        const s = edgeStyle("light", "datatype");
        expect(s.stroke).toBe("#d97706");
        expect(s.lineDash).toEqual([4, 3]);
    });

    it("dark 主题边颜色切换", () => {
        const s = edgeStyle("dark", "object");
        expect(s.stroke).toBe("#94a3b8");
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

    it("类节点按深度浅底分层（根略深、叶近白）", () => {
        const data = buildGraphData(nodes, edges, "light", true);
        const animal = data.nodes!.find((n: Record<string, unknown>) => n.id === "Animal");
        expect(animal?.style).toMatchObject({ fill: "#f1f5f9" }); // depth 0
        const dog = data.nodes!.find((n: Record<string, unknown>) => n.id === "Dog");
        expect(dog?.style).toMatchObject({ fill: "#f8fafc" }); // depth 1
    });

    it("datatype 节点保持浅琥珀样式", () => {
        const data = buildGraphData(nodes, edges, "light", true);
        const age = data.nodes!.find((n: Record<string, unknown>) => n.id === "age");
        expect(age?.style).toMatchObject({ fill: "#fffbeb" });
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

    it("统一变更色：added/modified 粉底虚线且不透明", () => {
        const n: ConceptNode[] = [
            { id: "A", label: "A", iri: "ex:A", kind: "class", diffStatus: "added" },
            { id: "B", label: "B", iri: "ex:B", kind: "class", diffStatus: "modified" },
        ];
        const data = buildGraphData(n, [], "light", true);
        for (const node of data.nodes!) {
            expect(node.style).toMatchObject({
                fill: "#fecaca",
                stroke: "#b91c1c",
                lineDash: [5, 4],
                opacity: 1,
            });
        }
    });

    it("三态完全同形：added / modified / removed 样式逐项一致（含透明度）", () => {
        const n: ConceptNode[] = [
            { id: "A", label: "X", iri: "ex:A", kind: "class", diffStatus: "added" },
            { id: "M", label: "X", iri: "ex:M", kind: "class", diffStatus: "modified" },
            { id: "G", label: "X", iri: "ex:G", kind: "class", diffStatus: "removed" },
        ];
        const data = buildGraphData(n, [], "light", true);
        const styleOf = (id: string) => {
            const rec = (data.nodes! as Record<string, unknown>[]).find((x) => x.id === id)!;
            return rec.style as Record<string, unknown>;
        };
        const mark = {
            fill: "#fecaca",
            stroke: "#b91c1c",
            labelFill: "#7f1d1d",
            lineWidth: 1.5,
            lineDash: [5, 4],
            opacity: 1,
        };
        for (const id of ["A", "M", "G"]) expect(styleOf(id)).toMatchObject(mark);
        // 删除态不得比新增/修改更淡：整份 style 必须逐键相同
        expect(styleOf("G")).toEqual(styleOf("A"));
        expect(styleOf("G")).toEqual(styleOf("M"));
    });

    it("边同理：removed 幽灵边与变更边完全同形", () => {
        const n: ConceptNode[] = [
            { id: "A", label: "A", iri: "ex:A", kind: "class" },
            { id: "B", label: "B", iri: "ex:B", kind: "class" },
        ];
        const e: ConceptEdge[] = [
            { id: "eR", source: "A", target: "B", kind: "object", diffStatus: "removed" },
            { id: "eM", source: "B", target: "A", kind: "object", diffStatus: "modified" },
        ];
        const data = buildGraphData(n, e, "light", true);
        const styleOf = (id: string) => {
            const rec = (data.edges! as Record<string, unknown>[]).find((x) => x.id === id)!;
            return rec.style as Record<string, unknown>;
        };
        expect(styleOf("eR")).toEqual(styleOf("eM"));
    });

    it("expandedAttrClassIris：全局关时仅展开类的 datatype 可见", () => {
        const data = buildGraphData(
            nodes,
            edges,
            "light",
            false,
            new Set(["ex:Animal"]),
        );
        const ids = data.nodes!.map((n: Record<string, unknown>) => n.id);
        expect(ids).toContain("age");
        expect(ids).toContain("Animal");
        expect(ids).toContain("Dog");
        expect(data.edges!.some((e: Record<string, unknown>) => e.id === "d1")).toBe(
            true,
        );
    });
});

// ══════════════════════════════════════════════════════════════════════
// 7. filterDatatypeVisibility / iriLooseEq
// ══════════════════════════════════════════════════════════════════════

describe("iriLooseEq", () => {
    it("整串相等", () => {
        expect(iriLooseEq("ex:A", "ex:A")).toBe(true);
    });
    it("local name 相等", () => {
        expect(
            iriLooseEq("http://ex.org#Animal", "http://other.org/Animal"),
        ).toBe(true);
    });
    it("空串不相等", () => {
        expect(iriLooseEq("", "ex:A")).toBe(false);
    });
});

describe("filterDatatypeVisibility", () => {
    const nodes: ConceptNode[] = [
        { id: "A", label: "A", iri: "http://ex.org#A", kind: "class" },
        { id: "B", label: "B", iri: "http://ex.org#B", kind: "class" },
        { id: "age", label: "age", iri: "http://ex.org#age", kind: "datatype" },
        { id: "name", label: "name", iri: "http://ex.org#name", kind: "datatype" },
        { id: "orphan", label: "orphan", iri: "http://ex.org#orphan", kind: "datatype" },
    ];
    const edges: ConceptEdge[] = [
        { id: "d1", source: "A", target: "age", kind: "datatype" },
        { id: "d2", source: "B", target: "name", kind: "datatype" },
        {
            id: "o1",
            source: "A",
            target: "B",
            label: "rel",
            kind: "object",
        },
    ];

    it("全局开：全显含孤儿", () => {
        const r = filterDatatypeVisibility(nodes, edges, true, new Set());
        expect(r.nodes).toHaveLength(5);
        expect(r.edges).toHaveLength(3);
    });

    it("全局关 + 空展开：无 datatype", () => {
        const r = filterDatatypeVisibility(nodes, edges, false, new Set());
        expect(r.nodes.every((n) => n.kind !== "datatype")).toBe(true);
        expect(r.edges.every((e) => e.kind !== "datatype")).toBe(true);
        expect(r.edges.some((e) => e.kind === "object")).toBe(true);
    });

    it("多类并存：展开 A+B 同时可见；收起 A 只留 B", () => {
        const both = filterDatatypeVisibility(
            nodes,
            edges,
            false,
            new Set(["http://ex.org#A", "http://ex.org#B"]),
        );
        expect(both.nodes.map((n) => n.id).sort()).toEqual([
            "A",
            "B",
            "age",
            "name",
        ]);

        const onlyB = filterDatatypeVisibility(
            nodes,
            edges,
            false,
            new Set(["http://ex.org#B"]),
        );
        expect(onlyB.nodes.map((n) => n.id).sort()).toEqual(["A", "B", "name"]);
        expect(onlyB.nodes.some((n) => n.id === "orphan")).toBe(false);
    });

    it("松散 IRI：展开用不同前缀的 local name 仍匹配", () => {
        const r = filterDatatypeVisibility(
            nodes,
            edges,
            false,
            new Set(["http://other.org/A"]),
        );
        expect(r.nodes.some((n) => n.id === "age")).toBe(true);
    });
});

// ══════════════════════════════════════════════════════════════════════
// 7. buildClassDeleteConfirmMessage（删除类确认框摘要）
// ══════════════════════════════════════════════════════════════════════

describe("buildClassDeleteConfirmMessage", () => {
    const ROUTE = "http://ex.org#Route";

    it("摘要属性/关系/继承，且不出现 IRI 墙", () => {
        const msg = buildClassDeleteConfirmMessage({
            classIri: ROUTE,
            datatypeProps: [
                { iri: "http://ex.org#name", label: "线路名" },
                { iri: "http://ex.org#code" },
            ],
            objectProps: [{ iri: "http://ex.org#belongsTo", label: "所属门店" }],
            parentLabels: ["基础对象"],
            childLabels: ["快线", "支线"],
        });
        expect(msg).toContain("确定删除「Route」");
        expect(msg).toContain("将删除属性：线路名、code");
        expect(msg).toContain("将删除关系：所属门店");
        expect(msg).toContain("将解除继承：父类 基础对象；子类 快线、支线");
        expect(msg).toContain("关系上的基数/限定约束将随之移除");
        // 不向用户堆实现细节/IRI
        expect(msg).not.toContain("http://");
        expect(msg).not.toContain("rdfs:");
    });

    it("无关联时给出中性说明，仍提示共享属性的处理方式", () => {
        const msg = buildClassDeleteConfirmMessage({
            classIri: ROUTE,
            datatypeProps: [],
            objectProps: [],
            parentLabels: [],
            childLabels: [],
        });
        expect(msg).toContain("该对象暂无关联的属性、关系或继承。");
        expect(msg).toContain("共享属性");
    });

    it("长清单裁剪为 8 项 + 省略号", () => {
        const many = Array.from({ length: 11 }, (_, i) => ({
            iri: `http://ex.org#p${i}`,
        }));
        const msg = buildClassDeleteConfirmMessage({
            classIri: ROUTE,
            datatypeProps: many,
            objectProps: [],
            parentLabels: [],
            childLabels: [],
        });
        expect(msg).toContain("p0、p1、p2、p3、p4、p5、p6、p7…");
        expect(msg).not.toContain("p8");
    });
});

// ══════════════════════════════════════════════════════════════════════
// 8. rdfTermForKind（tooltip 术语映射：节点与边共用）
// ══════════════════════════════════════════════════════════════════════

describe("rdfTermForKind", () => {
    it("按元素真实类型映射，object 边不再被标成数据属性", () => {
        expect(rdfTermForKind("class")).toBe("owl:Class");
        expect(rdfTermForKind("datatype")).toBe("owl:DatatypeProperty");
        expect(rdfTermForKind("object")).toBe("owl:ObjectProperty");
        expect(rdfTermForKind("hierarchy")).toBe("rdfs:subClassOf");
        expect(rdfTermForKind(undefined)).toBe("");
        expect(rdfTermForKind("whatever")).toBe("");
    });
});
