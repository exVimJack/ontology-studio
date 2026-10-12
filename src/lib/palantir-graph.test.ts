import { describe, expect, it } from "vitest";
import {
    payloadToGraphData,
    describeObjectType,
    hasObjectType,
    cardinalityBadge,
} from "./palantir-graph";
import type { OntologyPayload } from "@/lib/domain";

// ── 测试夹具 ──

function makePayload(overrides?: Partial<OntologyPayload>): OntologyPayload {
    return {
        api_name: "test_ontology",
        display_name: "测试本体",
        description: "用于测试的本体",
        object_types: [
            {
                api_name: "Employee",
                display_name: "员工",
                storage_type: "TABLE",
                primary_key: "employee_id",
                description: "雇员信息",
                properties: [
                    {
                        api_name: "employee_id",
                        display_name: "工号",
                        data_type: "STRING",
                    },
                    {
                        api_name: "name",
                        display_name: "姓名",
                        data_type: "STRING",
                    },
                ],
                links: [
                    {
                        api_name: "works_in",
                        display_name: "就职于",
                        target_object_type_api_name: "Department",
                        cardinality: "MANY_TO_ONE",
                    },
                ],
            },
            {
                api_name: "Department",
                display_name: "部门",
                storage_type: "TABLE",
                properties: [
                    {
                        api_name: "dept_id",
                        display_name: "部门ID",
                        data_type: "STRING",
                    },
                ],
                links: [],
            },
        ],
        ...overrides,
    };
}

describe("payloadToGraphData", () => {
    it("对象类型 → class 节点（label 用 display_name）", () => {
        const { nodes } = payloadToGraphData(makePayload());
        const emp = nodes.find((n) => n.id === "Employee");
        expect(emp).toBeDefined();
        expect(emp!.kind).toBe("class");
        expect(emp!.label).toBe("员工");
    });

    it("属性 → datatype 节点（dt: 前缀隔离，iri 位放 data_type）", () => {
        const { nodes } = payloadToGraphData(makePayload());
        const dt = nodes.find((n) => n.id === "dt:Employee.employee_id");
        expect(dt).toBeDefined();
        expect(dt!.kind).toBe("datatype");
        expect(dt!.label).toBe("工号");
        expect(dt!.iri).toBe("STRING");
    });

    it("属性 → datatype 边（source=类，target=dt 节点，label=data_type）", () => {
        const { edges } = payloadToGraphData(makePayload());
        const e = edges.find(
            (x) => x.id === "d-employee_id-Employee",
        );
        expect(e).toBeDefined();
        expect(e!.kind).toBe("datatype");
        expect(e!.source).toBe("Employee");
        expect(e!.target).toBe("dt:Employee.employee_id");
        expect(e!.label).toBe("STRING");
    });

    it("LinkDef → object 边（label 用 display_name + cardinality 紧凑标注）", () => {
        const { edges } = payloadToGraphData(makePayload());
        const e = edges.find((x) => x.id === "l-Employee-works_in");
        expect(e).toBeDefined();
        expect(e!.kind).toBe("object");
        expect(e!.source).toBe("Employee");
        expect(e!.target).toBe("Department");
        expect(e!.label).toBe("就职于（N:1）");
    });

    it("Palantir 语义覆盖：节点/边带 kindText，不露 RDF 术语", () => {
        const { nodes, edges } = payloadToGraphData(makePayload());
        const emp = nodes.find((n) => n.id === "Employee");
        expect(emp!.kindText).toBe("对象类型 ObjectType");
        const dt = nodes.find((n) => n.id === "dt:Employee.name");
        expect(dt!.kindText).toBe("属性 Property");
        const link = edges.find((x) => x.kind === "object");
        expect(link!.kindText).toBe("对象关联 LinkType");
    });

    it("无属性的 ObjectType 不产生 datatype 节点/边", () => {
        const payload = makePayload();
        payload.object_types = [
            {
                api_name: "Bare",
                display_name: "裸类型",
                storage_type: "TABLE",
                properties: [],
                links: [],
            },
        ];
        const { nodes, edges } = payloadToGraphData(payload);
        expect(nodes).toHaveLength(1);
        expect(nodes[0].id).toBe("Bare");
        expect(edges).toHaveLength(0);
    });

    it("showDatatypes=false 时不产出 datatype 节点与边", () => {
        const { nodes, edges } = payloadToGraphData(makePayload(), {
            showDatatypes: false,
        });
        expect(nodes.every((n) => n.kind === "class")).toBe(true);
        expect(edges.every((e) => e.kind === "object")).toBe(true);
        // object 边仍在
        expect(edges.some((e) => e.id === "l-Employee-works_in")).toBe(true);
    });

    it("悬空引用：target 未在本体定义时边保留、节点不伪造", () => {
        const payload = makePayload();
        payload.object_types = payload.object_types!.filter(
            (o) => o.api_name !== "Department",
        );
        const { nodes, edges } = payloadToGraphData(payload, {
            showDatatypes: false,
        });
        // Employee 类节点在，Department 节点不在
        expect(nodes.some((n) => n.id === "Employee")).toBe(true);
        expect(nodes.some((n) => n.id === "Department")).toBe(false);
        // 悬空边保留（图上可见）
        expect(edges.some((e) => e.target === "Department")).toBe(true);
    });

    it("空本体 → 空图", () => {
        const { nodes, edges } = payloadToGraphData({
            api_name: "empty",
            display_name: "空",
            object_types: [],
        });
        expect(nodes).toHaveLength(0);
        expect(edges).toHaveLength(0);
    });

    it("object_types undefined（宽松 JSON）不抛错", () => {
        const { nodes, edges } = payloadToGraphData({
            api_name: "loose",
            display_name: "宽松",
        } as OntologyPayload);
        expect(nodes).toHaveLength(0);
        expect(edges).toHaveLength(0);
    });

    it("同名属性在不同 ObjectType 下不互相覆盖（dt: 前缀含类型名）", () => {
        const { nodes } = payloadToGraphData(makePayload());
        const d1 = nodes.find((n) => n.id === "dt:Employee.dept_id");
        const d2 = nodes.find((n) => n.id === "dt:Department.dept_id");
        // Employee 无 dept_id，Department 有——两者只应出现其一对应实体
        expect(d1).toBeUndefined();
        expect(d2).toBeDefined();
    });
});

describe("describeObjectType", () => {
    it("返回对象类型摘要（含主键/存储/统计）", () => {
        const s = describeObjectType(makePayload(), "Employee");
        expect(s).toContain("员工");
        expect(s).toContain("employee_id");
        expect(s).toContain("TABLE");
        expect(s).toContain("属性 2 个 · 关系 1 条");
    });

    it("未定义的对象类型返回 null", () => {
        expect(describeObjectType(makePayload(), "Nope")).toBeNull();
    });
});

describe("cardinalityBadge", () => {
    it("四种标准基数 → 紧凑标注", () => {
        expect(cardinalityBadge("ONE_TO_ONE")).toBe("1:1");
        expect(cardinalityBadge("ONE_TO_MANY")).toBe("1:N");
        expect(cardinalityBadge("MANY_TO_ONE")).toBe("N:1");
        expect(cardinalityBadge("MANY_TO_MANY")).toBe("N:N");
    });
    it("未知值/空值原样返回", () => {
        expect(cardinalityBadge("WEIRD")).toBe("WEIRD");
        expect(cardinalityBadge("")).toBe("");
    });
});

describe("hasObjectType", () => {
    it("定义存在返回 true", () => {
        expect(hasObjectType(makePayload(), "Employee")).toBe(true);
    });
    it("定义不存在返回 false", () => {
        expect(hasObjectType(makePayload(), "Ghost")).toBe(false);
    });
});
