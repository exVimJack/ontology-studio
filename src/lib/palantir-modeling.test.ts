import { describe, expect, it } from "vitest";
import {
    DATA_TYPES,
    isValidObjectTypeName,
    isValidPropertyApiName,
    isValidDataType,
    isValidCardinality,
    propertyToDraft,
    draftToProperty,
    linkToDraft,
    draftToLink,
    validatePropertyDraft,
    validateLinkDraft,
    upsertProperty,
    removeProperty,
    upsertLink,
    removeLink,
    overwriteFor,
} from "./palantir-modeling";
import type { OntologyPayload } from "@/lib/domain";

// ── 测试夹具 ──

function makePayload(): OntologyPayload {
    return {
        api_name: "test_ont",
        display_name: "测试本体",
        object_types: [
            {
                api_name: "Employee",
                display_name: "员工",
                storage_type: "MANAGED",
                primary_key: "employeeId",
                properties: [
                    {
                        api_name: "employeeId",
                        display_name: "工号",
                        data_type: "STRING",
                        is_primary_key: true,
                    },
                    {
                        api_name: "hireDate",
                        display_name: "入职日期",
                        data_type: "DATE",
                    },
                ],
                links: [
                    {
                        api_name: "worksIn",
                        display_name: "就职于",
                        target_object_type_api_name: "Department",
                        cardinality: "MANY_TO_ONE",
                    },
                ],
            },
            {
                api_name: "Department",
                display_name: "部门",
                storage_type: "MANAGED",
                properties: [
                    {
                        api_name: "deptId",
                        display_name: "部门ID",
                        data_type: "STRING",
                        is_primary_key: true,
                    },
                ],
                links: [],
            },
        ],
    };
}

describe("命名规范预检（对齐 naming.rs）", () => {
    it("ObjectType: PascalCase 通过/失败", () => {
        expect(isValidObjectTypeName("PurchaseOrder")).toBe(true);
        expect(isValidObjectTypeName("A")).toBe(true);
        expect(isValidObjectTypeName("purchaseOrder")).toBe(false);
        expect(isValidObjectTypeName("Purchase_Order")).toBe(false);
        expect(isValidObjectTypeName("")).toBe(false);
    });

    it("Property: camelCase 通过/失败", () => {
        expect(isValidPropertyApiName("orderDate")).toBe(true);
        expect(isValidPropertyApiName("a")).toBe(true);
        expect(isValidPropertyApiName("OrderDate")).toBe(false);
        expect(isValidPropertyApiName("order_date")).toBe(false);
    });

    it("data_type 枚举（对齐 data_type.rs，大小写敏感）", () => {
        expect(isValidDataType("STRING")).toBe(true);
        expect(isValidDataType("TIMESTAMP")).toBe(true);
        expect(isValidDataType("BIGINT")).toBe(false);
        expect(isValidDataType("string")).toBe(false);
        expect(isValidDataType("")).toBe(false);
        expect(DATA_TYPES.length).toBe(20);
    });

    it("cardinality 枚举", () => {
        expect(isValidCardinality("MANY_TO_ONE")).toBe(true);
        expect(isValidCardinality("many_to_one")).toBe(false);
        expect(isValidCardinality("")).toBe(false);
    });
});

describe("草稿 ↔ 定义投影", () => {
    it("propertyToDraft / draftToProperty 往返一致", () => {
        const p = makePayload().object_types[0].properties[0];
        const d = propertyToDraft(p);
        expect(d.apiName).toBe("employeeId");
        expect(d.isPrimaryKey).toBe(true);
        const back = draftToProperty(d);
        // 投影会补全可选字段（description/searchable 等），逐语义字段断言
        expect(back.api_name).toBe(p.api_name);
        expect(back.display_name).toBe(p.display_name);
        expect(back.data_type).toBe(p.data_type);
        expect(back.is_primary_key).toBe(true);
        expect(back.searchable).toBe(true);
        // 原 optional 字段（vector_config/backing_mapping）不被伪造
        expect(back.vector_config).toBeUndefined();
        expect(back.backing_mapping).toBeUndefined();
    });

    it("linkToDraft / draftToLink 往返一致", () => {
        const l = makePayload().object_types[0].links[0];
        const d = linkToDraft(l);
        expect(d.targetObjectTypeName).toBe("Department");
        const back = draftToLink(d);
        expect(back.api_name).toBe(l.api_name);
        expect(back.display_name).toBe(l.display_name);
        expect(back.target_object_type_api_name).toBe(l.target_object_type_api_name);
        expect(back.cardinality).toBe(l.cardinality);
        // 原 optional 字段不被伪造
        expect(back.foreign_key_property_api_name).toBeUndefined();
        expect(back.temporal).toBeUndefined();
    });

    it("draftToProperty 空名回退 api_name", () => {
        const d = propertyToDraft({
            api_name: "foo",
            display_name: "",
            data_type: "STRING",
        });
        d.displayName = "";
        const p = draftToProperty(d);
        expect(p.display_name).toBe("foo");
    });
});

describe("属性草稿预检", () => {
    const base = {
        apiName: "newProp",
        displayName: "新属性",
        description: "",
        dataType: "STRING",
        searchable: true,
        isPrimaryKey: false,
        isTitleProperty: false,
    };

    it("新增：合法通过", () => {
        expect(
            validatePropertyDraft(base, { isNew: true, existingApiNames: ["a"] }),
        ).toBeNull();
    });

    it("新增：camelCase 违规报错", () => {
        expect(
            validatePropertyDraft({ ...base, apiName: "NewProp" }, { isNew: true, existingApiNames: [] }),
        ).toContain("camelCase");
    });

    it("新增：重名报错", () => {
        expect(
            validatePropertyDraft(base, { isNew: true, existingApiNames: ["newProp"] }),
        ).toContain("已存在");
    });

    it("编辑：同名不算重名（isNew=false）", () => {
        expect(
            validatePropertyDraft(base, { isNew: false, existingApiNames: ["newProp"] }),
        ).toBeNull();
    });

    it("displayName 必填", () => {
        expect(
            validatePropertyDraft({ ...base, displayName: " " }, { isNew: true, existingApiNames: [] }),
        ).toContain("不能为空");
    });

    it("data_type 非法枚举报错", () => {
        expect(
            validatePropertyDraft({ ...base, dataType: "INT" }, { isNew: true, existingApiNames: [] }),
        ).toContain("枚举");
    });
});

describe("关系草稿预检", () => {
    const base = {
        apiName: "belongsTo",
        displayName: "属于",
        description: "",
        targetObjectTypeName: "Department",
        cardinality: "MANY_TO_ONE",
    };
    const opts = { isNew: true, existingApiNames: [], objectTypeNames: ["Employee", "Department"] };

    it("合法通过", () => {
        expect(validateLinkDraft(base, opts)).toBeNull();
    });

    it("target 缺失报错", () => {
        expect(
            validateLinkDraft({ ...base, targetObjectTypeName: "" }, opts),
        ).toContain("目标对象类型");
    });

    it("target 未在本体定义报错", () => {
        expect(
            validateLinkDraft({ ...base, targetObjectTypeName: "Ghost" }, opts),
        ).toContain("未在本体中定义");
    });

    it("cardinality 非法报错", () => {
        expect(
            validateLinkDraft({ ...base, cardinality: "N_TO_N" }, opts),
        ).toContain("枚举");
    });
});

describe("payload 变换：属性", () => {
    it("新增属性：追加到该 OT，原 payload 不被修改（不可变）", () => {
        const p0 = makePayload();
        const next = upsertProperty(p0, "Employee", {
            api_name: "salary",
            display_name: "薪资",
            data_type: "DECIMAL",
        }, true);
        expect(next.object_types[0].properties.some((p) => p.api_name === "salary")).toBe(true);
        // 原 payload 不变
        expect(p0.object_types[0].properties.some((p) => p.api_name === "salary")).toBe(false);
        // 其他 OT 不受影响
        expect(next.object_types[1].properties).toHaveLength(1);
    });

    it("编辑属性：按 api_name 原位替换", () => {
        const p0 = makePayload();
        const next = upsertProperty(p0, "Employee", {
            api_name: "hireDate",
            display_name: "入职时间",
            data_type: "TIMESTAMP",
        }, false);
        const edited = next.object_types[0].properties.find((p) => p.api_name === "hireDate");
        expect(edited!.data_type).toBe("TIMESTAMP");
        expect(edited!.display_name).toBe("入职时间");
        // 属性数量不变
        expect(next.object_types[0].properties).toHaveLength(2);
    });

    it("新增重名属性抛错", () => {
        const p0 = makePayload();
        expect(() =>
            upsertProperty(p0, "Employee", {
                api_name: "hireDate", display_name: "x", data_type: "STRING",
            }, true),
        ).toThrow("已存在");
    });

    it("编辑不存在的属性抛错", () => {
        const p0 = makePayload();
        expect(() =>
            upsertProperty(p0, "Employee", {
                api_name: "ghost", display_name: "x", data_type: "STRING",
            }, false),
        ).toThrow("不存在");
    });

    it("删除属性：数量减一", () => {
        const p0 = makePayload();
        const next = removeProperty(p0, "Employee", "hireDate");
        expect(next.object_types[0].properties).toHaveLength(1);
        expect(() => removeProperty(p0, "Employee", "ghost")).toThrow("不存在");
    });
});

describe("payload 变换：关系", () => {
    it("新增关系：指向已定义 OT", () => {
        const p0 = makePayload();
        const next = upsertLink(p0, "Department", {
            api_name: "hasHead",
            display_name: "主管",
            target_object_type_api_name: "Employee",
            cardinality: "ONE_TO_ONE",
        }, true);
        expect(next.object_types[1].links).toHaveLength(1);
        expect(next.object_types[1].links[0].api_name).toBe("hasHead");
        // 原 payload 不变
        expect(p0.object_types[1].links).toHaveLength(0);
    });

    it("编辑关系：原位替换", () => {
        const p0 = makePayload();
        const next = upsertLink(p0, "Employee", {
            api_name: "worksIn",
            display_name: "任职于",
            target_object_type_api_name: "Department",
            cardinality: "MANY_TO_ONE",
        }, false);
        const l = next.object_types[0].links.find((x) => x.api_name === "worksIn");
        expect(l!.display_name).toBe("任职于");
    });

    it("编辑不存在的关系抛错", () => {
        const p0 = makePayload();
        expect(() =>
            upsertLink(p0, "Employee", {
                api_name: "ghost", display_name: "x",
                target_object_type_api_name: "Department", cardinality: "MANY_TO_ONE",
            }, false),
        ).toThrow("不存在");
    });

    it("删除关系", () => {
        const p0 = makePayload();
        const next = removeLink(p0, "Employee", "worksIn");
        expect(next.object_types[0].links).toHaveLength(0);
        expect(() => removeLink(p0, "Employee", "ghost")).toThrow("不存在");
    });
});

describe("overwrite 参数", () => {
    it("只覆盖被编辑的 OT", () => {
        expect(overwriteFor("Employee")).toEqual(["Employee"]);
    });
});
