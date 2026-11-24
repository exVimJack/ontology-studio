/**
 * 将工作区图谱与「生效 → 工作区」结构化 diff 合并为带 diffStatus 的图数据。
 */
import type { TtlVersionDiff } from "@/lib/domain";
import type { ConceptEdge, ConceptNode, DiffStatus } from "@/lib/ontology-graph";
import { termToShort } from "@/lib/ontology-graph";

function normIri(iri: string): string {
  if (!iri) return "";
  return iri.startsWith("<") && iri.endsWith(">") ? iri.slice(1, -1) : iri;
}

function shortOf(iri: string): string {
  return termToShort(normIri(iri));
}

export type DiffCounts = {
  added: number;
  modified: number;
  removed: number;
  /** 类节点（图上默认可见） */
  addedClasses: number;
  removedClasses: number;
  /** 属性（默认「☐ 属性」时图上不可见） */
  addedProps: number;
  removedProps: number;
  /** 继承边 */
  addedHier: number;
  removedHier: number;
};

/** 结构化 diff 是否含任何可见变更项（含本体头、继承）。 */
export function diffHasEntries(diff: TtlVersionDiff | null | undefined): boolean {
  if (!diff) return false;
  const c = countEffectiveDiff(diff);
  return c.added + c.modified + c.removed > 0;
}

export function countEffectiveDiff(diff: TtlVersionDiff | null | undefined): DiffCounts {
  if (!diff) {
    return {
      added: 0,
      modified: 0,
      removed: 0,
      addedClasses: 0,
      removedClasses: 0,
      addedProps: 0,
      removedProps: 0,
      addedHier: 0,
      removedHier: 0,
    };
  }
  const addedHier = diff.added_subclass_of?.length ?? 0;
  const removedHier = diff.removed_subclass_of?.length ?? 0;
  const added =
    diff.added_classes.length + diff.added_properties.length + addedHier;
  const modified =
    diff.modified_classes.length +
    diff.modified_properties.length +
    (diff.ontology_meta_changed ? 1 : 0);
  const removed =
    diff.removed_classes.length +
    diff.removed_properties.length +
    removedHier;
  return {
    added,
    modified,
    removed,
    addedClasses: diff.added_classes.length,
    removedClasses: diff.removed_classes.length,
    addedProps: diff.added_properties.length,
    removedProps: diff.removed_properties.length,
    addedHier,
    removedHier,
  };
}

export function applyEffectiveDiffToGraph(
  baseNodes: ConceptNode[],
  baseEdges: ConceptEdge[],
  diff: TtlVersionDiff,
): { nodes: ConceptNode[]; edges: ConceptEdge[] } {
  const nodes = baseNodes.map((n) => ({ ...n }));
  const edges = baseEdges.map((e) => ({ ...e }));
  const nodeByIri = new Map<string, ConceptNode>();
  const nodeById = new Map<string, ConceptNode>();
  for (const n of nodes) {
    nodeByIri.set(normIri(n.iri), n);
    nodeById.set(n.id, n);
    // 短名兜底：SPARQL term 与 ttl_edit 全形偶发不一致时仍能命中
    const short = shortOf(n.iri);
    if (short && !nodeById.has(short)) nodeById.set(short, n);
  }

  const markNode = (iri: string, status: DiffStatus) => {
    const full = normIri(iri);
    const n = nodeByIri.get(full) ?? nodeById.get(shortOf(full));
    if (n) n.diffStatus = status;
  };

  /** 属性增删改时给定义域类标黄（不覆盖已有 added/removed）。
   *  类图默认隐藏数据属性节点，否则删掉 channel 后「线索」上看不到任何着色。 */
  const tintDomainClass = (domain: string | undefined | null) => {
    if (!domain) return;
    const full = normIri(domain);
    const n = nodeByIri.get(full) ?? nodeById.get(shortOf(full));
    if (n && !n.diffStatus) n.diffStatus = "modified";
  };

  /** 解析类节点 id；底图没有时注入幽灵删除节点。已在底图的节点不改写 status。 */
  const ensureGhostClass = (iri: string, labelHint?: string) => {
    const full = normIri(iri);
    if (nodeByIri.has(full)) return nodeByIri.get(full)!.id;
    const id = shortOf(full) || full;
    if (nodeById.has(id)) return nodeById.get(id)!.id;
    const ghost: ConceptNode = {
      id,
      label: labelHint || shortOf(full) || full,
      iri: full,
      kind: "class",
      diffStatus: "removed",
      kindText: "已删除",
    };
    nodes.push(ghost);
    nodeByIri.set(full, ghost);
    nodeById.set(id, ghost);
    return id;
  };

  for (const iri of diff.added_classes) markNode(iri, "added");
  for (const c of diff.modified_classes) markNode(c.iri, "modified");
  for (const iri of diff.removed_classes) {
    markNode(iri, "removed");
    ensureGhostClass(iri);
  }

  // 属性：datatype → 节点；object → 边（按 predIri）
  const markEdgesByPred = (predIri: string, status: DiffStatus) => {
    const p = normIri(predIri);
    for (const e of edges) {
      if (e.predIri && normIri(e.predIri) === p) e.diffStatus = status;
    }
  };

  for (const p of diff.added_properties) {
    if (p.kind === "datatype") markNode(p.iri, "added");
    else markEdgesByPred(p.iri, "added");
    tintDomainClass(p.domain);
  }
  for (const p of diff.modified_properties) {
    if (p.after.kind === "datatype" || p.before.kind === "datatype")
      markNode(p.iri, "modified");
    else markEdgesByPred(p.iri, "modified");
    tintDomainClass(p.after.domain || p.before.domain);
  }
  for (const p of diff.removed_properties) {
    if (p.kind === "datatype") {
      const full = normIri(p.iri);
      let propId = shortOf(full) || full;
      if (!nodeByIri.has(full)) {
        if (!nodeById.has(propId)) {
          const ghost: ConceptNode = {
            id: propId,
            label: p.label || shortOf(full),
            iri: full,
            kind: "datatype",
            diffStatus: "removed",
            kindText: "已删除",
          };
          nodes.push(ghost);
          nodeByIri.set(full, ghost);
          nodeById.set(propId, ghost);
        } else {
          const existing = nodeById.get(propId)!;
          existing.diffStatus = "removed";
          propId = existing.id;
        }
      } else {
        markNode(p.iri, "removed");
        propId = nodeByIri.get(full)!.id;
      }
      // 定义域仍在工作区：类标黄 + 幽灵边（勾选「属性」时可见红虚）
      tintDomainClass(p.domain);
      if (p.domain) {
        const src =
          nodeByIri.get(normIri(p.domain)) ??
          nodeById.get(shortOf(p.domain));
        if (src) {
          const eid = `ghost-d-${shortOf(full)}-${src.id}`;
          if (!edges.some((e) => e.id === eid)) {
            edges.push({
              id: eid,
              source: src.id,
              target: propId,
              label: p.label || shortOf(full),
              kind: "datatype",
              predIri: full,
              diffStatus: "removed",
              kindText: "已删除",
            });
          }
        }
      }
    } else {
      // 已删对象属性：幽灵边（需 domain/range）；定义域/值域类标黄（仍存在时）
      tintDomainClass(p.domain);
      tintDomainClass(p.range);
      const src = p.domain ? ensureGhostClass(p.domain) : "";
      const tgt = p.range ? ensureGhostClass(p.range) : src;
      if (src && tgt) {
        const id = `ghost-o-${shortOf(p.iri)}-${src}-${tgt}`;
        if (!edges.some((e) => e.id === id)) {
          edges.push({
            id,
            source: src,
            target: tgt,
            label: p.label || shortOf(p.iri),
            kind: "object",
            predIri: normIri(p.iri),
            diffStatus: "removed",
            kindText: "已删除",
          });
        }
      }
    }
  }

  for (const e of diff.added_subclass_of ?? []) {
    const childId = shortOf(e.child);
    const parentId = shortOf(e.parent);
    for (const edge of edges) {
      if (
        edge.kind === "hierarchy" &&
        edge.source === childId &&
        edge.target === parentId
      ) {
        edge.diffStatus = "added";
      }
    }
  }
  for (const e of diff.removed_subclass_of ?? []) {
    const childId = ensureGhostClass(e.child);
    const parentId = ensureGhostClass(e.parent);
    const id = `ghost-h-${childId}-${parentId}`;
    if (!edges.some((x) => x.id === id)) {
      edges.push({
        id,
        source: childId,
        target: parentId,
        label: "subClassOf",
        kind: "hierarchy",
        diffStatus: "removed",
        kindText: "已删除",
      });
    }
  }

  return { nodes: [...nodes], edges: [...edges] };
}

/** 图选中焦点 → 变更清单「中+宽」相关过滤（见 design D2）。 */
export type GraphDiffFocus =
  | { kind: "class"; iri: string }
  | { kind: "property"; iri: string }
  | { kind: "hierarchy"; childIri: string; parentIri: string };

function iriMatch(a: string, b: string): boolean {
  if (!a || !b) return false;
  const na = normIri(a);
  const nb = normIri(b);
  if (na === nb) return true;
  const sa = shortOf(na);
  const sb = shortOf(nb);
  return !!sa && (sa === sb || sa === nb || na === sb);
}

function propTouchesClass(
  p: { iri: string; domain?: string; range?: string },
  classIri: string,
): boolean {
  return iriMatch(p.domain ?? "", classIri) || iriMatch(p.range ?? "", classIri);
}

function propIriIs(
  p: { iri: string },
  iri: string,
): boolean {
  return iriMatch(p.iri, iri);
}

function emptyDiffKeepMeta(diff: TtlVersionDiff): TtlVersionDiff {
  return {
    ...diff,
    added_classes: [],
    removed_classes: [],
    modified_classes: [],
    added_properties: [],
    removed_properties: [],
    modified_properties: [],
    added_subclass_of: [],
    removed_subclass_of: [],
    ontology_meta_changed: false,
  };
}

/**
 * 无 focus 时返回原 diff；有 focus 时只保留相关项。
 * 类：自身类变更 + domain/range 落在该类的属性 + 端点含该类的继承边。
 * 属性：仅该 pred 的增删改。
 * 继承边：仅匹配的 subclass_of。
 */
export function filterDiffRelatedTo(
  diff: TtlVersionDiff,
  focus: GraphDiffFocus | null | undefined,
): TtlVersionDiff {
  if (!focus) return diff;
  const out = emptyDiffKeepMeta(diff);

  if (focus.kind === "class") {
    const c = focus.iri;
    out.added_classes = diff.added_classes.filter((iri) => iriMatch(iri, c));
    out.removed_classes = diff.removed_classes.filter((iri) => iriMatch(iri, c));
    out.modified_classes = diff.modified_classes.filter((m) =>
      iriMatch(m.iri, c),
    );
    out.added_properties = diff.added_properties.filter((p) =>
      propTouchesClass(p, c),
    );
    out.removed_properties = diff.removed_properties.filter((p) =>
      propTouchesClass(p, c),
    );
    out.modified_properties = diff.modified_properties.filter(
      (m) =>
        propTouchesClass(m.after, c) || propTouchesClass(m.before, c),
    );
    out.added_subclass_of = (diff.added_subclass_of ?? []).filter(
      (e) => iriMatch(e.child, c) || iriMatch(e.parent, c),
    );
    out.removed_subclass_of = (diff.removed_subclass_of ?? []).filter(
      (e) => iriMatch(e.child, c) || iriMatch(e.parent, c),
    );
    return out;
  }

  if (focus.kind === "property") {
    const p = focus.iri;
    out.added_properties = diff.added_properties.filter((x) => propIriIs(x, p));
    out.removed_properties = diff.removed_properties.filter((x) =>
      propIriIs(x, p),
    );
    out.modified_properties = diff.modified_properties.filter((m) =>
      iriMatch(m.iri, p),
    );
    return out;
  }

  // hierarchy edge
  const { childIri, parentIri } = focus;
  const matchEdge = (e: { child: string; parent: string }) =>
    iriMatch(e.child, childIri) && iriMatch(e.parent, parentIri);
  out.added_subclass_of = (diff.added_subclass_of ?? []).filter(matchEdge);
  out.removed_subclass_of = (diff.removed_subclass_of ?? []).filter(matchEdge);
  return out;
}
