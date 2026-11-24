/**
 * palantir-modeling — Palantir 标准本体结构化建模（纯逻辑层，无 React 依赖）
 *
 * 复用 W3C 侧的「结构化建模编辑」交互语义（PropertiesPanel：属性列表 +
 * 编辑/新增/删除表单 + 前端预检），落到 Palantir 的 OntologyPayload 上：
 *
 *   W3C 路径：TtlPropertyEdit → ttl_edit.rs 文本手术 → surgery_import
 *   Palantir 路径：PropertyDef/LinkDef → 本模块变换 payload → preview_import 预演
 *                 → import_ontology(overwrite=[被编辑 OT]) 落库
 *
 * 语义映射（TtlPropertyEdit ↔ Palantir）：
 *   datatype 属性 → PropertyDef { api_name, display_name, data_type }（domain=所属 OT）
 *   object   关系 → LinkDef { api_name, display_name, target_object_type_api_name, cardinality }
 *                         cardinality ∈ ONE|MANY（Gaia 契约，非四元组；M:N 拆中间 OT）
 *
 * 写入安全网（对齐后端 store.rs）：
 *   - preview_import 的 errors 非空必须阻断应用
 *   - import overwrite 只列被编辑的 OT；未列入的同名 OT 走 skip 保护
 *   - overwrite 语义 = 删旧 OT（级联 properties）后重建，links 必须随 payload 一起带
 *
 * 命名规范（对齐 crates/ontology-store/src/naming.rs，前端预检给即时反馈）：
 *   ObjectType api_name：PascalCase（^[A-Z][a-zA-Z0-9]{0,99}$）
 *   Property/Link api_name：camelCase（^[a-z][a-zA-Z0-9]{0,99}$）
 *   data_type：DATA_TYPES 枚举（对齐 data_type.rs）
 */

import type {
    OntologyPayload,
    ObjectTypeDef,
    PropertyDef,
    LinkDef,
} from "@/lib/domain";

/** DataType 枚举（对齐 crates/ontology-store/src/data_type.rs DATA_TYPES）。 */
export const DATA_TYPES = [
    "STRING",
    "INTEGER",
    "SHORT",
    "LONG",
    "BOOLEAN",
    "BYTE",
    "FLOAT",
    "DOUBLE",
    "DECIMAL",
    "DATE",
    "TIMESTAMP",
    "ARRAY",
    "STRUCT",
    "VECTOR",
    "GEOPOINT",
    "GEOSHAPE",
    "GEOTEMPORAL_SERIES",
    "TIME_SERIES",
    "MEDIA_REFERENCE",
    "ATTACHMENT",
] as const;

export type DataType = (typeof DATA_TYPES)[number];

/**
 * LinkType 基数（**对齐 Gaia `LinkTypeDefCreate.cardinality: Literal["ONE","MANY"]`**）。
 *
 * 只有两值，不是四元组——这是 Gaia 契约（后端 store.rs::validate_link +
 * schema.rs CHECK 同源），语义见 CARDINALITY_HINTS：
 *   - ONE  = 严格 1:1
 *   - MANY = 关系里存在「多」的一端（1:N 与 N:1 都填 MANY，不做区分）
 * 方向由 source→target 声明（LinkDef 无 direction 字段，source 侧即隐含 outgoing），
 * 所以 M:N 不能直接建，必须拆中间 ObjectType（见 CARDINALITY_HELP）。
 */
export const CARDINALITIES = ["ONE", "MANY"] as const;

export type Cardinality = (typeof CARDINALITIES)[number];

/** 基数的用户可读说明（下拉选项文案 + 表单提示共用，避免两处漂移）。 */
export const CARDINALITY_HINTS: Record<Cardinality, string> = {
    ONE: "ONE（严格 1:1，两端各一个）",
    MANY: "MANY（存在「多」端：1:N 或 N:1 都填 MANY）",
};

/**
 * 基数选择处的常驻提示（对齐 Gaia 建模方法论，用户最容易误解的三点）。
 */
export const CARDINALITY_HELP =
    "基数只有 ONE / MANY 两档：ONE 仅用于严格 1:1；只要关系里存在「多」的一端就填 MANY" +
    "（1:N、N:1 不区分，方向由本关系声明的 源→目标 表达）。多对多不能直接建，" +
    "需先建一个中间对象类型（如 SupplierMaterialRel），再拆成两组 1:N。";

const PASCAL_CASE = /^[A-Z][a-zA-Z0-9]{0,99}$/;
const CAMEL_CASE = /^[a-z][a-zA-Z0-9]{0,99}$/;

/** ObjectType api_name 预检（PascalCase）。 */
export function isValidObjectTypeName(s: string): boolean {
    return PASCAL_CASE.test(s);
}

/** Property/Link api_name 预检（camelCase）。 */
export function isValidPropertyApiName(s: string): boolean {
    return CAMEL_CASE.test(s);
}

/** data_type 枚举校验。 */
export function isValidDataType(s: string): boolean {
    return (DATA_TYPES as readonly string[]).includes(s);
}

/** cardinality 枚举校验。 */
export function isValidCardinality(s: string): boolean {
    return (CARDINALITIES as readonly string[]).includes(s);
}

/** 属性编辑草稿（对齐 W3C PropertyDraft 的可编辑投影语义）。 */
export interface PropertyDraft {
    /** 编辑态 = 原 api_name（身份不可改）；新增态 = 用户输入 */
    apiName: string;
    displayName: string;
    description: string;
    dataType: string;
    searchable: boolean;
    isPrimaryKey: boolean;
    isTitleProperty: boolean;
}

/** 关系编辑草稿。 */
export interface LinkDraft {
    /** 编辑态 = 原 api_name（身份不可改）；新增态 = 用户输入 */
    apiName: string;
    displayName: string;
    description: string;
    targetObjectTypeName: string;
    cardinality: string;
}

// ════════════════════════════════════════════════════════════════
// 草稿 ↔ 定义 投影
// ════════════════════════════════════════════════════════════════

export function propertyToDraft(p: PropertyDef): PropertyDraft {
    return {
        apiName: p.api_name,
        displayName: p.display_name ?? "",
        description: p.description ?? "",
        dataType: p.data_type,
        searchable: p.searchable ?? true,
        isPrimaryKey: p.is_primary_key ?? false,
        isTitleProperty: p.is_title_property ?? false,
    };
}

export function draftToProperty(d: PropertyDraft): PropertyDef {
    return {
        api_name: d.apiName,
        display_name: d.displayName || d.apiName,
        description: d.description,
        data_type: d.dataType,
        searchable: d.searchable,
        is_primary_key: d.isPrimaryKey || undefined,
        is_title_property: d.isTitleProperty || undefined,
    };
}

export function linkToDraft(l: LinkDef): LinkDraft {
    return {
        apiName: l.api_name,
        displayName: l.display_name ?? "",
        description: l.description ?? "",
        targetObjectTypeName: l.target_object_type_api_name,
        cardinality: l.cardinality,
    };
}

export function draftToLink(d: LinkDraft): LinkDef {
    return {
        api_name: d.apiName,
        display_name: d.displayName || d.apiName,
        description: d.description,
        target_object_type_api_name: d.targetObjectTypeName,
        cardinality: d.cardinality,
    };
}

// ════════════════════════════════════════════════════════════════
// 表单预检（对齐 W3C PropertiesPanel 的前端即时反馈，后端校验为准）
// ════════════════════════════════════════════════════════════════

/**
 * 属性草稿预检。返回错误消息（null = 通过）。
 * 规则：apiName camelCase、displayName 必填、data_type 枚举。
 */
export function validatePropertyDraft(
    d: PropertyDraft,
    opts: { isNew: boolean; existingApiNames: string[] },
): string | null {
    if (!isValidPropertyApiName(d.apiName)) {
        return `属性名「${d.apiName || "（空）"}」不符合 camelCase 规范（小写字母开头，仅字母数字）`;
    }
    if (opts.isNew && opts.existingApiNames.includes(d.apiName)) {
        return `属性「${d.apiName}」已存在`;
    }
    if (!d.displayName.trim()) {
        return "名称（display_name）不能为空";
    }
    if (!isValidDataType(d.dataType)) {
        return `数据类型「${d.dataType}」不在合法枚举内`;
    }
    return null;
}

/**
 * 关系草稿预检。返回错误消息（null = 通过）。
 * 规则：apiName camelCase、displayName 必填、target 必填且须为本体已定义的 ObjectType。
 */
export function validateLinkDraft(
    d: LinkDraft,
    opts: { isNew: boolean; existingApiNames: string[]; objectTypeNames: string[] },
): string | null {
    if (!isValidPropertyApiName(d.apiName)) {
        return `关系名「${d.apiName || "（空）"}」不符合 camelCase 规范（小写字母开头，仅字母数字）`;
    }
    if (opts.isNew && opts.existingApiNames.includes(d.apiName)) {
        return `关系「${d.apiName}」已存在`;
    }
    if (!d.displayName.trim()) {
        return "名称（display_name）不能为空";
    }
    if (!d.targetObjectTypeName) {
        return "必须指定目标对象类型";
    }
    if (!opts.objectTypeNames.includes(d.targetObjectTypeName)) {
        return `目标对象类型「${d.targetObjectTypeName}」未在本体中定义`;
    }
    if (!isValidCardinality(d.cardinality)) {
        return (
            `基数「${d.cardinality}」不在合法枚举内（只接受 ${CARDINALITIES.join(
                " / ",
            )}；1:N 与 N:1 统一填 MANY，多对多需拆中间对象类型）`
        );
    }
    return null;
}

// ════════════════════════════════════════════════════════════════
// payload 变换（不可变更新，供 import overwrite 落库）
// ════════════════════════════════════════════════════════════════

function findOT(
    payload: OntologyPayload,
    objectTypeName: string,
): { ot: ObjectTypeDef; index: number } | null {
    const index = (payload.object_types ?? []).findIndex(
        (o) => o.api_name === objectTypeName,
    );
    if (index < 0) return null;
    return { ot: payload.object_types[index], index };
}

/**
 * 新增/编辑属性：返回替换了 properties 的新 payload（原 payload 不变）。
 * 编辑 = 按 apiName 原位替换；新增 = 追加到末尾。
 *
 * @throws Error 目标对象类型不存在（不该发生——UI 从已定义 OT 进入）
 */
export function upsertProperty(
    payload: OntologyPayload,
    objectTypeName: string,
    prop: PropertyDef,
    isNew: boolean,
): OntologyPayload {
    const found = findOT(payload, objectTypeName);
    if (!found) throw new Error(`对象类型「${objectTypeName}」不存在`);
    const props = [...(found.ot.properties ?? [])];
    if (isNew) {
        if (props.some((p) => p.api_name === prop.api_name)) {
            throw new Error(`属性「${prop.api_name}」已存在`);
        }
        props.push(prop);
    } else {
        const idx = props.findIndex((p) => p.api_name === prop.api_name);
        if (idx < 0) throw new Error(`属性「${prop.api_name}」不存在，无法编辑`);
        props[idx] = prop;
    }
    const ot: ObjectTypeDef = { ...found.ot, properties: props };
    const object_types = [...payload.object_types];
    object_types[found.index] = ot;
    return { ...payload, object_types };
}

/**
 * 删除属性。返回新 payload（原 payload 不变）。
 *
 * @throws Error 目标对象类型或属性不存在
 */
export function removeProperty(
    payload: OntologyPayload,
    objectTypeName: string,
    propApiName: string,
): OntologyPayload {
    const found = findOT(payload, objectTypeName);
    if (!found) throw new Error(`对象类型「${objectTypeName}」不存在`);
    const props = (found.ot.properties ?? []).filter(
        (p) => p.api_name !== propApiName,
    );
    if (props.length === (found.ot.properties?.length ?? 0)) {
        throw new Error(`属性「${propApiName}」不存在`);
    }
    const ot: ObjectTypeDef = { ...found.ot, properties: props };
    const object_types = [...payload.object_types];
    object_types[found.index] = ot;
    return { ...payload, object_types };
}

/**
 * 新增/编辑关系。返回替换了 links 的新 payload（原 payload 不变）。
 * 注意：PK/标题属性被引用检查留给后端 preview（errors 阻断）。
 */
export function upsertLink(
    payload: OntologyPayload,
    objectTypeName: string,
    link: LinkDef,
    isNew: boolean,
): OntologyPayload {
    const found = findOT(payload, objectTypeName);
    if (!found) throw new Error(`对象类型「${objectTypeName}」不存在`);
    const links = [...(found.ot.links ?? [])];
    if (isNew) {
        if (links.some((l) => l.api_name === link.api_name)) {
            throw new Error(`关系「${link.api_name}」已存在`);
        }
        links.push(link);
    } else {
        const idx = links.findIndex((l) => l.api_name === link.api_name);
        if (idx < 0) throw new Error(`关系「${link.api_name}」不存在，无法编辑`);
        links[idx] = link;
    }
    const ot: ObjectTypeDef = { ...found.ot, links };
    const object_types = [...payload.object_types];
    object_types[found.index] = ot;
    return { ...payload, object_types };
}

/**
 * 删除关系。返回新 payload（原 payload 不变）。
 */
export function removeLink(
    payload: OntologyPayload,
    objectTypeName: string,
    linkApiName: string,
): OntologyPayload {
    const found = findOT(payload, objectTypeName);
    if (!found) throw new Error(`对象类型「${objectTypeName}」不存在`);
    const links = (found.ot.links ?? []).filter((l) => l.api_name !== linkApiName);
    if (links.length === (found.ot.links?.length ?? 0)) {
        throw new Error(`关系「${linkApiName}」不存在`);
    }
    const ot: ObjectTypeDef = { ...found.ot, links };
    const object_types = [...payload.object_types];
    object_types[found.index] = ot;
    return { ...payload, object_types };
}

/**
 * 构造编辑落库的 overwrite 参数：只覆盖被编辑的 OT（其余同名 OT 走 skip 保护）。
 * 数据源未被编辑，overwrite 数据源列表恒为空。
 */
export function overwriteFor(objectTypeName: string): string[] {
    return [objectTypeName];
}
