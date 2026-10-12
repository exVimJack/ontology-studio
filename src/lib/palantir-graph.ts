/**
 * palantir-graph — Palantir 标准本体 → 概念图谱数据的纯逻辑转换模块
 *
 * 复用 W3C 侧已重写的「概念图谱」能力（ConceptGraph + G6）：
 *   W3C 路径：SPARQL rows → rowsToGraphData() → ConceptNode/ConceptEdge → ConceptGraph
 *   Palantir 路径：OntologyPayload(ObjectTypeDef[]) → payloadToGraphData() → 同一套渲染
 *
 * 语义映射（对齐 ontology-graph.ts 的 ConceptNode/ConceptEdge）：
 *   ObjectTypeDef        → class 节点（圆，蓝色系）
 *   PropertyDef          → datatype 节点（方，琥珀色系；domain=所属 ObjectType）
 *   LinkDef              → object 边（虚线带标签，source=所在 ObjectType，target=target_object_type_api_name）
 *   （无 subClassOf 对应；Palantir 无类层级语义，不产生 hierarchy 边）
 *
 * 图数据全部来自前端已有的 OntologyPayload（export_ontology / useOntologyPayload），
 * 不新增 IPC 命令，零后端改动。
 */

import type { ConceptNode, ConceptEdge } from "./ontology-graph";
import type { OntologyPayload, ObjectTypeDef } from "@/lib/domain";

// ── 输入行类型（与 domain/index.ts 手写 interface 对齐） ──

interface PropertyLike {
    api_name: string;
    display_name: string;
    data_type: string;
}

interface LinkLike {
    api_name: string;
    display_name: string;
    target_object_type_api_name: string;
    cardinality: string;
}

interface ObjectTypeLike {
    api_name: string;
    display_name: string;
    properties: PropertyLike[];
    links: LinkLike[];
}

/** Palantir 属性节点的 id 前缀（与 ObjectType api_name 隔离，避免撞名） */
const DATATYPE_PREFIX = "dt:";

/**
 * cardinality → 紧凑标注（边标签用）。
 * 语义：source 侧基数 : target 侧基数。
 */
export function cardinalityBadge(cardinality: string): string {
    switch (cardinality) {
        case "ONE_TO_ONE":
            return "1:1";
        case "ONE_TO_MANY":
            return "1:N";
        case "MANY_TO_ONE":
            return "N:1";
        case "MANY_TO_MANY":
            return "N:N";
        default:
            return cardinality;
    }
}

/**
 * OntologyPayload → 概念图谱数据（节点=对象类型+数据属性，边=LinkType）。
 *
 * 规则：
 * - 每个 ObjectTypeDef 产出一个 class 节点（label=display_name，
 *   kindText 覆盖为 Palantir 语义「对象类型 ObjectType」，不露 RDF 术语）
 * - 每个属性产出一个 datatype 节点（id 加 `dt:` 前缀防撞名，label=display_name，
 *   data_type 追加到 iri 语义位供详情弹层展示，kindText=「属性 Property」）
 * - 每条 LinkDef 产出一条 object 边（label=display_name + cardinality 紧凑标注，
 *   如「就职于（N:1）」；kindText=「对象关联 LinkType」）
 *   —— target ObjectType 未在本体中定义时仍保留边（悬空引用在图上可见），
 *      但不为其伪造节点；节点以实际定义为准。
 * - 空属性/空链接的对象类型只出 class 节点
 *
 * @param payload 前端已 JSON.parse 的本体定义
 * @param opts.showDatatypes false 时不产出 datatype 节点/边（与 ConceptGraph 的开关对齐）
 */
export function payloadToGraphData(
    payload: OntologyPayload,
    opts: { showDatatypes?: boolean } = {},
): { nodes: ConceptNode[]; edges: ConceptEdge[] } {
    const showDatatypes = opts.showDatatypes ?? true;
    const nodeMap = new Map<string, ConceptNode>();
    const edgeList: ConceptEdge[] = [];

    const ensureClass = (ot: { api_name: string; display_name: string }) => {
        if (!ot.api_name) return null;
        if (!nodeMap.has(ot.api_name)) {
            nodeMap.set(ot.api_name, {
                id: ot.api_name,
                label: ot.display_name || ot.api_name,
                iri: ot.api_name,
                kind: "class",
                // Palantir 语义覆盖：tooltip/详情显示 ObjectType 而非 owl:Class
                kindText: "对象类型 ObjectType",
            });
        }
        return nodeMap.get(ot.api_name)!;
    };

    const ensureDatatype = (ot: ObjectTypeLike, p: PropertyLike) => {
        const id = `${DATATYPE_PREFIX}${ot.api_name}.${p.api_name}`;
        if (!nodeMap.has(id)) {
            nodeMap.set(id, {
                id,
                label: p.display_name || p.api_name,
                // iri 语义位放 data_type，详情弹层展示属性类型
                iri: p.data_type || "",
                kind: "datatype",
                // Palantir 语义覆盖：tooltip/详情显示 Property 而非 owl:DatatypeProperty
                kindText: "属性 Property",
            });
        }
        return nodeMap.get(id)!;
    };

    for (const ot of payload.object_types ?? []) {
        ensureClass(ot);
        if (!showDatatypes) continue;
        for (const p of ot.properties ?? []) {
            if (!p.api_name) continue;
            const dtNode = ensureDatatype(ot, p);
            // datatype 边（琥珀虚线）：source=类节点，target=属性节点
            edgeList.push({
                id: `d-${p.api_name}-${ot.api_name}`,
                source: ot.api_name,
                target: dtNode.id,
                label: p.data_type || undefined,
                kind: "datatype",
                kindText: "属性 Property",
            });
        }
    }

    for (const ot of payload.object_types ?? []) {
        for (const link of ot.links ?? []) {
            if (!link.api_name || !link.target_object_type_api_name) continue;
            // 悬空引用不伪造节点：target 未定义时边仍出（图上可见），节点以定义为准
            // label 附 cardinality 紧凑标注（Palantir Link 语义，W3C object 边无此信息）
            edgeList.push({
                id: `l-${ot.api_name}-${link.api_name}`,
                source: ot.api_name,
                target: link.target_object_type_api_name,
                label: `${link.display_name || link.api_name}（${cardinalityBadge(
                    link.cardinality ?? "",
                )}）`,
                kind: "object",
                kindText: "对象关联 LinkType",
            });
        }
    }

    return { nodes: [...nodeMap.values()], edges: edgeList };
}

/**
 * 图谱节点详情弹层的语义适配：
 * Palantir 侧没有 Turtle 源码可跳转，改为返回对象类型的定义摘要。
 * ConceptGraph 的 onJumpToSource 对 Palantir 语义 = 「查看对象类型定义」。
 *
 * @returns 人类可读的定义摘要（api_name/主键/存储类型/属性数/关系数）
 */
export function describeObjectType(payload: OntologyPayload, apiName: string): string | null {
    const ot: ObjectTypeDef | undefined = (payload.object_types ?? []).find(
        (o) => o.api_name === apiName,
    );
    if (!ot) return null;
    const lines = [
        `对象类型：${ot.display_name || ot.api_name}`,
        `api_name：${ot.api_name}`,
    ];
    if (ot.primary_key) lines.push(`主键：${ot.primary_key}`);
    if (ot.storage_type) lines.push(`存储：${ot.storage_type}`);
    const propCount = ot.properties?.length ?? 0;
    const linkCount = ot.links?.length ?? 0;
    lines.push(`属性 ${propCount} 个 · 关系 ${linkCount} 条`);
    if (ot.description) lines.push(ot.description);
    return lines.join("\n");
}

/**
 * 判断 class 节点在 payload 中是否有定义（详情弹层展示与悬空引用区分）。
 */
export function hasObjectType(payload: OntologyPayload, apiName: string): boolean {
    return (payload.object_types ?? []).some((o) => o.api_name === apiName);
}
