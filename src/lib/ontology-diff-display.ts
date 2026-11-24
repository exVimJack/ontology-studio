/**
 * 变更清单展示投影：语义分组（属性/关系/类/继承）+ 中文主名。
 */
import type {
  TtlVersionDiff,
  TtlPropertyEdit,
  WorkspaceDiffRevokeItem,
} from "@/lib/domain";
import { termToShort } from "@/lib/ontology-graph";

export type DiffListSection =
  | "attribute"
  | "relation"
  | "class"
  | "hierarchy"
  | "meta";

export type DiffListOp = "add" | "remove" | "modify";

/** 行级撤销入参（与后端 WorkspaceDiffRevokeItem 对齐）。 */
export type DiffRevokeTarget =
  | { kind: "class"; op: DiffListOp; iri: string }
  | { kind: "property"; op: DiffListOp; iri: string }
  | {
      kind: "subclass";
      op: "add" | "remove";
      child: string;
      parent: string;
    }
  | { kind: "meta"; op: "modify" };

/** 前端清单行 → IPC 撤销入参。 */
export function toWorkspaceRevokeItem(
  target: DiffRevokeTarget,
): WorkspaceDiffRevokeItem {
  switch (target.kind) {
    case "class":
      return { kind: "class", op: target.op, iri: target.iri };
    case "property":
      return { kind: "property", op: target.op, iri: target.iri };
    case "subclass":
      return {
        kind: "subclass",
        op: target.op,
        child: target.child,
        parent: target.parent,
      };
    case "meta":
      return { kind: "meta", op: "modify" };
  }
}

export type DiffListItem = {
  key: string;
  section: DiffListSection;
  op: DiffListOp;
  /** 主显示名（中文 label 优先） */
  primary: string;
  /** 有中文主名时辅以短 IRI */
  secondary?: string;
  detail?: string;
  revoke: DiffRevokeTarget;
};

export const DIFF_SECTION_ORDER: DiffListSection[] = [
  "attribute",
  "relation",
  "class",
  "hierarchy",
  "meta",
];

export const DIFF_SECTION_LABEL: Record<DiffListSection, string> = {
  attribute: "属性",
  relation: "关系",
  class: "类",
  hierarchy: "继承",
  meta: "本体",
};

function shortIri(iri: string): string {
  return termToShort(iri);
}

function displayName(
  iri: string,
  label: string | undefined | null,
  labelMap?: Map<string, string> | null,
): { primary: string; secondary?: string } {
  const short = shortIri(iri);
  const fromProp = (label ?? "").trim();
  const fromMap = labelMap?.get(iri)?.trim() || labelMap?.get(short)?.trim() || "";
  const zh = fromProp || fromMap;
  if (zh && zh !== short) return { primary: zh, secondary: short };
  return { primary: short };
}

function propSection(kind: string): DiffListSection {
  return kind === "object" ? "relation" : "attribute";
}

function propDetail(before: TtlPropertyEdit, after: TtlPropertyEdit): string | undefined {
  const bits: string[] = [];
  if (before.label !== after.label) {
    bits.push(`名称: ${before.label || "—"} → ${after.label || "—"}`);
  }
  if (before.comment !== after.comment) bits.push("说明有变");
  if (before.domain !== after.domain) bits.push("定义域有变");
  if (before.range !== after.range) bits.push("值域有变");
  if (before.kind !== after.kind) bits.push("类型有变");
  return bits.length ? bits.join(" · ") : undefined;
}

/**
 * 将结构化 diff 投影为分组清单行（先语义桶，组内 +/-/~ 由调用方按 op 排序亦可）。
 */
export function projectDiffListItems(
  diff: TtlVersionDiff,
  labelMap?: Map<string, string> | null,
): DiffListItem[] {
  const items: DiffListItem[] = [];

  for (const iri of diff.added_classes) {
    const { primary, secondary } = displayName(iri, null, labelMap);
    items.push({
      key: `ac:${iri}`,
      section: "class",
      op: "add",
      primary,
      secondary,
      revoke: { kind: "class", op: "add", iri },
    });
  }
  for (const iri of diff.removed_classes) {
    const { primary, secondary } = displayName(iri, null, labelMap);
    items.push({
      key: `rc:${iri}`,
      section: "class",
      op: "remove",
      primary,
      secondary,
      revoke: { kind: "class", op: "remove", iri },
    });
  }
  for (const c of diff.modified_classes) {
    const label = c.after.label || c.before.label;
    const { primary, secondary } = displayName(c.iri, label, labelMap);
    const detailBits: string[] = [];
    if (c.before.label !== c.after.label) {
      detailBits.push(`名称: ${c.before.label || "—"} → ${c.after.label || "—"}`);
    }
    if (c.before.comment !== c.after.comment) detailBits.push("说明有变");
    items.push({
      key: `mc:${c.iri}`,
      section: "class",
      op: "modify",
      primary,
      secondary,
      detail: detailBits.length ? detailBits.join(" · ") : undefined,
      revoke: { kind: "class", op: "modify", iri: c.iri },
    });
  }

  for (const p of diff.added_properties) {
    const { primary, secondary } = displayName(p.iri, p.label, labelMap);
    items.push({
      key: `ap:${p.iri}`,
      section: propSection(p.kind),
      op: "add",
      primary,
      secondary,
      revoke: { kind: "property", op: "add", iri: p.iri },
    });
  }
  for (const p of diff.removed_properties) {
    const { primary, secondary } = displayName(p.iri, p.label, labelMap);
    items.push({
      key: `rp:${p.iri}`,
      section: propSection(p.kind),
      op: "remove",
      primary,
      secondary,
      revoke: { kind: "property", op: "remove", iri: p.iri },
    });
  }
  for (const p of diff.modified_properties) {
    const label = p.after.label || p.before.label;
    const { primary, secondary } = displayName(p.iri, label, labelMap);
    items.push({
      key: `mp:${p.iri}`,
      section: propSection(p.after.kind || p.before.kind),
      op: "modify",
      primary,
      secondary,
      detail: propDetail(p.before, p.after),
      revoke: { kind: "property", op: "modify", iri: p.iri },
    });
  }

  for (const e of diff.added_subclass_of ?? []) {
    const child = displayName(e.child, null, labelMap);
    const parent = displayName(e.parent, null, labelMap);
    items.push({
      key: `ah:${e.child}|${e.parent}`,
      section: "hierarchy",
      op: "add",
      primary: `${child.primary} ⊑ ${parent.primary}`,
      secondary:
        child.secondary || parent.secondary
          ? `${child.secondary ?? shortIri(e.child)} ⊑ ${parent.secondary ?? shortIri(e.parent)}`
          : undefined,
      revoke: {
        kind: "subclass",
        op: "add",
        child: e.child,
        parent: e.parent,
      },
    });
  }
  for (const e of diff.removed_subclass_of ?? []) {
    const child = displayName(e.child, null, labelMap);
    const parent = displayName(e.parent, null, labelMap);
    items.push({
      key: `rh:${e.child}|${e.parent}`,
      section: "hierarchy",
      op: "remove",
      primary: `${child.primary} ⊑ ${parent.primary}`,
      secondary:
        child.secondary || parent.secondary
          ? `${child.secondary ?? shortIri(e.child)} ⊑ ${parent.secondary ?? shortIri(e.parent)}`
          : undefined,
      revoke: {
        kind: "subclass",
        op: "remove",
        child: e.child,
        parent: e.parent,
      },
    });
  }

  if (diff.ontology_meta_changed) {
    items.push({
      key: "meta",
      section: "meta",
      op: "modify",
      primary: "本体头部元信息（名称/说明）有变化",
      revoke: { kind: "meta", op: "modify" },
    });
  }

  return items;
}

function iriLooseEq(a: string, b: string): boolean {
  if (!a || !b) return false;
  if (a === b) return true;
  const sa = a.split(/[#/]/).pop() ?? "";
  const sb = b.split(/[#/]/).pop() ?? "";
  return !!sa && sa === sb;
}

function propTouchesClass(
  p: { domain?: string; range?: string },
  classIri: string,
): boolean {
  return (
    iriLooseEq(p.domain ?? "", classIri) || iriLooseEq(p.range ?? "", classIri)
  );
}

function domainRangeDiffTouchesC(
  before: TtlPropertyEdit,
  after: TtlPropertyEdit,
  classIri: string,
): boolean {
  const bDom = before.domain ?? "";
  const aDom = after.domain ?? "";
  const bRange = before.range ?? "";
  const aRange = after.range ?? "";
  const domChanged = bDom !== aDom;
  const rangeChanged = bRange !== aRange;
  if (domChanged) {
    if (iriLooseEq(bDom, classIri) || iriLooseEq(aDom, classIri)) return true;
  }
  if (rangeChanged) {
    if (iriLooseEq(bRange, classIri) || iriLooseEq(aRange, classIri)) return true;
  }
  return false;
}

/** 级联跳过的条目：恢复方向上对端类仍为删除态，故不随本类恢复。 */
export type CascadeSkipped = {
  iri: string;
  /** 显示名：中文 label 优先，回退短 IRI */
  name: string;
  /** 仍为删除态的对端类短名（回执里说明「等谁」） */
  missingClass: string;
  kind: "attribute" | "relation" | "hierarchy";
};

/** 类 C 相对生效 diff 的强依赖子集（不含类行本身）。 */
export type ClassStrongDeps = {
  classOp: "add" | "remove" | null;
  addedProperties: TtlPropertyEdit[];
  removedProperties: TtlPropertyEdit[];
  modifiedProperties: NonNullable<TtlVersionDiff["modified_properties"]>;
  addedSubclassOf: NonNullable<TtlVersionDiff["added_subclass_of"]>;
  removedSubclassOf: NonNullable<TtlVersionDiff["removed_subclass_of"]>;
  /**
   * 恢复方向被「对端存在性门禁」跳过的条目。与后端 build_class_cascade_steps 同口径：
   * 构件的非 C 端落在 removed_classes（生效版有、工作区没有的类）即跳过——
   * 等价于后端「端点是已声明类且恢复后工作区仍无它」的判据。
   */
  skipped: CascadeSkipped[];
};

/**
 * 从完整 diff 抽出类 C 的强依赖（design D2）。
 * - 属性 ±：domain=C
 * - 关系 ±：domain 或 range=C
 * - 继承 ±：端点含 C
 * - ~：同批存在 ±C，且 before/after 的 domain/range 差集含 C
 * - 恢复方向额外过滤：非 C 端仍为删除态的条目进 skipped，不入级联
 */
export function collectClassStrongDeps(
  diff: TtlVersionDiff,
  classIri: string,
): ClassStrongDeps {
  const classOp: "add" | "remove" | null = diff.added_classes.some((i) =>
    iriLooseEq(i, classIri),
  )
    ? "add"
    : diff.removed_classes.some((i) => iriLooseEq(i, classIri))
      ? "remove"
      : null;

  // domain|range 均算（与后端级联 / 禁止序一致；不依赖 kind，避免值域侧漏网）
  const addedProperties = (diff.added_properties ?? []).filter((p) =>
    propTouchesClass(p, classIri),
  );

  const removedSubclassOf = (diff.removed_subclass_of ?? []).filter(
    (e) => iriLooseEq(e.child, classIri) || iriLooseEq(e.parent, classIri),
  );
  const addedSubclassOf = (diff.added_subclass_of ?? []).filter(
    (e) => iriLooseEq(e.child, classIri) || iriLooseEq(e.parent, classIri),
  );

  const removedCandidates = (diff.removed_properties ?? []).filter((p) =>
    propTouchesClass(p, classIri),
  );
  const modifiedCandidates =
    classOp != null
      ? (diff.modified_properties ?? []).filter((p) =>
          domainRangeDiffTouchesC(p.before, p.after, classIri),
        )
      : [];

  const skipped: CascadeSkipped[] = [];
  if (classOp !== "remove") {
    return {
      classOp,
      addedProperties,
      removedProperties: removedCandidates,
      modifiedProperties: modifiedCandidates,
      addedSubclassOf,
      removedSubclassOf,
      skipped,
    };
  }

  // 对端存在性门禁：非 C 端仍是「已删除类」→ 该构件不随本类恢复
  const stillDeletedOtherEnd = (iri: string | undefined): string => {
    if (!iri || iriLooseEq(iri, classIri)) return "";
    return (diff.removed_classes ?? []).some((c) => iriLooseEq(c, iri)) ? iri : "";
  };

  const removedProperties: TtlPropertyEdit[] = [];
  for (const p of removedCandidates) {
    const missing = stillDeletedOtherEnd(p.domain) || stillDeletedOtherEnd(p.range);
    if (missing) {
      skipped.push({
        iri: p.iri,
        name: p.label || termToShort(p.iri),
        missingClass: termToShort(missing),
        kind: p.kind === "object" ? "relation" : "attribute",
      });
      continue;
    }
    removedProperties.push(p);
  }

  const modifiedProperties: NonNullable<TtlVersionDiff["modified_properties"]> = [];
  for (const m of modifiedCandidates) {
    // 回滚目标是生效版定义（before），按回滚后的端点判存在性
    const missing =
      stillDeletedOtherEnd(m.before.domain) || stillDeletedOtherEnd(m.before.range);
    if (missing) {
      skipped.push({
        iri: m.iri,
        name: m.before.label || termToShort(m.iri),
        missingClass: termToShort(missing),
        kind: m.before.kind === "object" ? "relation" : "attribute",
      });
      continue;
    }
    modifiedProperties.push(m);
  }

  const keptSubclass: NonNullable<TtlVersionDiff["removed_subclass_of"]> = [];
  for (const e of removedSubclassOf) {
    const missing = stillDeletedOtherEnd(e.child) || stillDeletedOtherEnd(e.parent);
    if (missing) {
      skipped.push({
        iri: e.child,
        name: `${termToShort(e.child)} ⊑ ${termToShort(e.parent)}`,
        missingClass: termToShort(missing),
        kind: "hierarchy",
      });
      continue;
    }
    keptSubclass.push(e);
  }

  return {
    classOp,
    addedProperties,
    removedProperties,
    modifiedProperties,
    addedSubclassOf,
    removedSubclassOf: keptSubclass,
    skipped,
  };
}

/** 确认框业务摘要（不含类名本身）。 */
export function summarizeClassStrongDeps(deps: ClassStrongDeps): string[] {
  const lines: string[] = [];
  const nAttrAdd = deps.addedProperties.filter((p) => p.kind !== "object").length;
  const nRelAdd = deps.addedProperties.filter((p) => p.kind === "object").length;
  const nAttrRm = deps.removedProperties.filter((p) => p.kind !== "object").length;
  const nRelRm = deps.removedProperties.filter((p) => p.kind === "object").length;
  if (nAttrAdd) lines.push(`${nAttrAdd} 个新增属性`);
  if (nRelAdd) lines.push(`${nRelAdd} 个新增关系`);
  if (nAttrRm) lines.push(`${nAttrRm} 个已删属性`);
  if (nRelRm) lines.push(`${nRelRm} 个已删关系`);
  if (deps.addedSubclassOf.length)
    lines.push(`${deps.addedSubclassOf.length} 条新增继承`);
  if (deps.removedSubclassOf.length)
    lines.push(`${deps.removedSubclassOf.length} 条已删继承`);
  if (deps.modifiedProperties.length) {
    lines.push(
      `将恢复该类在 ${deps.modifiedProperties.length} 个共享属性/关系上的定义域或值域`,
    );
  }
  if (deps.skipped.length) {
    lines.push(
      `不处理 ${deps.skipped.length} 项：其对端类尚未恢复，待对端恢复后自动一并处理`,
    );
  }
  return lines;
}

/** 级联跳过项的人话回执（一行）。 */
export function describeClassCascadeSkips(deps: ClassStrongDeps): string {
  if (!deps.skipped.length) return "";
  const byClass = new Map<string, { n: number; kind: CascadeSkipped["kind"] }>();
  for (const s of deps.skipped) {
    const cur = byClass.get(s.missingClass);
    byClass.set(s.missingClass, {
      n: (cur?.n ?? 0) + 1,
      kind: cur?.kind ?? s.kind,
    });
  }
  const noun: Record<CascadeSkipped["kind"], string> = {
    attribute: "属性",
    relation: "关系",
    hierarchy: "继承",
  };
  const parts = [...byClass.entries()].map(
    ([cls, { n, kind }]) => `${n} 项（对端类「${cls}」尚未恢复，恢复后自动带回${noun[kind]}）`,
  );
  return `未处理 ${deps.skipped.length} 项：${parts.join("；")}`;
}

/**
 * 类级联撤销完成后的业务回执（一行）：恢复了什么、按对端存在性规则没处理什么。
 * 数字取自与后端同口径的前端预览（collectClassStrongDeps）。
 */
export function describeClassCascadeOutcome(
  deps: ClassStrongDeps,
  classShort: string,
): string {
  if (deps.classOp === "remove") {
    const n =
      deps.removedProperties.length +
      deps.modifiedProperties.length +
      deps.removedSubclassOf.length;
    const head = `已恢复「${classShort}」${n ? `及其 ${n} 项关联定义` : ""}`;
    const skip = describeClassCascadeSkips(deps);
    if (!skip) return head;
    // 图上增/改/删共用统一变更色（既定设计：类型区分由变更清单承担），撤销后该类仍可能
    // 是「有变更」态——回执必须自己把这层解释掉，否则用户以为没生效。
    return `${head}；${skip}；图上「${classShort}」仍标为有变更即因这些项未处理（增/改/删同形，类型看变更清单）`;
  }
  const n =
    deps.addedProperties.length +
    deps.modifiedProperties.length +
    deps.addedSubclassOf.length;
  return `已撤销新增类「${classShort}」${n ? `及其 ${n} 项关联定义` : ""}`;
}

/**
 * 已删除类 C 仍在 diff 时，禁止单独恢复其强依赖构件。
 * 属性/关系：与清单过滤一致，domain 或 range 触及 C 即拦（不依赖 kind，避免
 * 「值域落在已删类」的关系漏放）。
 */
export function isLeafRestoreBlockedByDeletedClass(
  diff: TtlVersionDiff,
  target: DiffRevokeTarget,
): string | null {
  if (target.kind === "class" || target.kind === "meta") return null;
  if (target.op !== "remove") return null;

  for (const classIri of diff.removed_classes) {
    if (target.kind === "property") {
      const prop = (diff.removed_properties ?? []).find((p) =>
        iriLooseEq(p.iri, target.iri),
      );
      if (prop && propTouchesClass(prop, classIri)) {
        const short = classIri.split(/[#/]/).pop() || classIri;
        return `请先恢复类「${short}」，再处理其属性或关系`;
      }
    }
    if (target.kind === "subclass") {
      const hit = (diff.removed_subclass_of ?? []).some(
        (e) =>
          (iriLooseEq(e.child, classIri) || iriLooseEq(e.parent, classIri)) &&
          iriLooseEq(e.child, target.child) &&
          iriLooseEq(e.parent, target.parent),
      );
      if (hit) {
        const short = classIri.split(/[#/]/).pop() || classIri;
        return `请先恢复类「${short}」，再处理其继承关系`;
      }
    }
  }
  return null;
}

/** 按语义分组；组内顺序：add → modify → remove。 */
export function groupDiffListItems(
  items: DiffListItem[],
): { section: DiffListSection; label: string; items: DiffListItem[] }[] {
  const opOrder: Record<DiffListOp, number> = { add: 0, modify: 1, remove: 2 };
  const bySection = new Map<DiffListSection, DiffListItem[]>();
  for (const it of items) {
    const list = bySection.get(it.section) ?? [];
    list.push(it);
    bySection.set(it.section, list);
  }
  const groups: { section: DiffListSection; label: string; items: DiffListItem[] }[] =
    [];
  for (const section of DIFF_SECTION_ORDER) {
    const list = bySection.get(section);
    if (!list?.length) continue;
    list.sort((a, b) => opOrder[a.op] - opOrder[b.op] || a.primary.localeCompare(b.primary, "zh"));
    groups.push({
      section,
      label: DIFF_SECTION_LABEL[section],
      items: list,
    });
  }
  return groups;
}
