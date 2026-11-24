import { describe, expect, it } from "vitest";
import type { TtlVersionDiff } from "@/lib/domain";
import {
  collectClassStrongDeps,
  describeClassCascadeOutcome,
  describeClassCascadeSkips,
  groupDiffListItems,
  isLeafRestoreBlockedByDeletedClass,
  projectDiffListItems,
  summarizeClassStrongDeps,
} from "@/lib/ontology-diff-display";

function emptyDiff(over: Partial<TtlVersionDiff> = {}): TtlVersionDiff {
  return {
    rev_a: 1,
    rev_b: 0,
    added_classes: [],
    removed_classes: [],
    modified_classes: [],
    added_properties: [],
    removed_properties: [],
    modified_properties: [],
    added_subclass_of: [],
    removed_subclass_of: [],
    ontology_meta_changed: false,
    ...over,
  };
}

describe("projectDiffListItems", () => {
  it("groups datatype as 属性 and object as 关系; prefers Chinese label", () => {
    const diff = emptyDiff({
      added_properties: [
        {
          iri: "http://ex#chatStatus",
          kind: "datatype",
          label: "状态",
        },
        {
          iri: "http://ex#belongsTo",
          kind: "object",
          label: "属于用户",
        },
      ],
      added_classes: ["http://ex#ChatRecord"],
    });
    const labelMap = new Map([["http://ex#ChatRecord", "微信聊天记录"]]);
    const items = projectDiffListItems(diff, labelMap);
    const groups = groupDiffListItems(items);
    expect(groups.map((g) => g.label)).toEqual(["属性", "关系", "类"]);
    expect(groups[0].items[0].primary).toBe("状态");
    expect(groups[0].items[0].secondary).toBe("chatStatus");
    expect(groups[1].items[0].primary).toBe("属于用户");
    expect(groups[2].items[0].primary).toBe("微信聊天记录");
    expect(groups[2].items[0].secondary).toBe("ChatRecord");
  });

  it("falls back to short IRI without label", () => {
    const items = projectDiffListItems(
      emptyDiff({
        added_properties: [
          { iri: "http://ex#fooBar", kind: "datatype" },
        ],
      }),
    );
    expect(items[0].primary).toBe("fooBar");
    expect(items[0].secondary).toBeUndefined();
  });
});

describe("collectClassStrongDeps", () => {
  const C = "http://ex#ChatRecord";
  const U = "http://ex#User";

  it("collects added class package: domain attrs, both-end relations, hierarchy", () => {
    const diff = emptyDiff({
      added_classes: [C],
      added_properties: [
        { iri: "http://ex#logTime", kind: "datatype", domain: C },
        { iri: "http://ex#orphan", kind: "datatype", domain: U },
        {
          iri: "http://ex#belongs",
          kind: "object",
          domain: C,
          range: U,
        },
        {
          iri: "http://ex#mentions",
          kind: "object",
          domain: U,
          range: C,
        },
      ],
      added_subclass_of: [
        { child: C, parent: "http://ex#Record" },
        { child: "http://ex#Other", parent: U },
      ],
    });
    const deps = collectClassStrongDeps(diff, C);
    expect(deps.classOp).toBe("add");
    expect(deps.addedProperties.map((p) => p.iri)).toEqual([
      "http://ex#logTime",
      "http://ex#belongs",
      "http://ex#mentions",
    ]);
    expect(deps.addedSubclassOf).toEqual([{ child: C, parent: "http://ex#Record" }]);
    expect(summarizeClassStrongDeps(deps)).toContain("1 个新增属性");
    expect(summarizeClassStrongDeps(deps)).toContain("2 个新增关系");
  });

  it("collects removed class package and domain ~ only when ±C present", () => {
    const diff = emptyDiff({
      removed_classes: [C],
      removed_properties: [
        { iri: "http://ex#name", kind: "datatype", domain: C },
      ],
      modified_properties: [
        {
          iri: "http://ex#createTime",
          before: {
            iri: "http://ex#createTime",
            kind: "datatype",
            domain: C,
          },
          after: {
            iri: "http://ex#createTime",
            kind: "datatype",
            domain: U,
          },
        },
      ],
    });
    const deps = collectClassStrongDeps(diff, C);
    expect(deps.classOp).toBe("remove");
    expect(deps.removedProperties).toHaveLength(1);
    expect(deps.modifiedProperties).toHaveLength(1);
    expect(summarizeClassStrongDeps(deps).some((l) => l.includes("共享属性"))).toBe(
      true,
    );
  });

  it("does not treat isolated ~ as strong dep without ±C", () => {
    const diff = emptyDiff({
      modified_properties: [
        {
          iri: "http://ex#createTime",
          before: {
            iri: "http://ex#createTime",
            kind: "datatype",
            domain: C,
          },
          after: {
            iri: "http://ex#createTime",
            kind: "datatype",
            domain: U,
          },
        },
      ],
    });
    const deps = collectClassStrongDeps(diff, C);
    expect(deps.classOp).toBeNull();
    expect(deps.modifiedProperties).toHaveLength(0);
  });

  describe("对端存在性门禁（恢复方向）", () => {
    const rel = (iri: string, domain: string, range: string) => ({
      iri,
      kind: "object",
      label: iri.split("#").pop(),
      domain,
      range,
    });

    it("关系对端类仍为删除态时跳过，不随本类恢复", () => {
      const diff = emptyDiff({
        removed_classes: [C, U],
        removed_properties: [
          { iri: "http://ex#name", kind: "datatype", domain: C },
          rel("http://ex#sourcedFrom", C, U),
        ],
      });
      const deps = collectClassStrongDeps(diff, C);
      expect(deps.classOp).toBe("remove");
      expect(deps.removedProperties.map((p) => p.iri)).toEqual(["http://ex#name"]);
      expect(deps.skipped).toEqual([
        {
          iri: "http://ex#sourcedFrom",
          name: "sourcedFrom",
          missingClass: "User",
          kind: "relation",
        },
      ]);
      expect(summarizeClassStrongDeps(deps).some((x) => x.includes("不处理 1 项"))).toBe(
        true,
      );
      expect(describeClassCascadeSkips(deps)).toContain("对端类「User」尚未恢复");
      // 图上增/改/删同形（既定设计），回执负责解释「为什么撤销后还是虚线」
      const note = describeClassCascadeOutcome(deps, "线索");
      expect(note).toContain("已恢复「线索」");
      expect(note).toContain("图上「线索」仍标为有变更");
    });

    it("对端类存在（不在删除差异）时正常恢复；自环与 xsd 值域不受影响", () => {
      const diff = emptyDiff({
        removed_classes: [C],
        removed_properties: [
          rel("http://ex#ok", C, "http://ex#Advisor"),
          rel("http://ex#selfLoop", C, C),
          { iri: "http://ex#chan", kind: "datatype", domain: C, range: "http://www.w3.org/2001/XMLSchema#string" },
        ],
      });
      const deps = collectClassStrongDeps(diff, C);
      expect(deps.skipped).toEqual([]);
      expect(deps.removedProperties).toHaveLength(3);
      expect(describeClassCascadeSkips(deps)).toBe("");
    });

    it("继承边对端父类仍删除时跳过；父类正是被恢复类时保留", () => {
      const diff = emptyDiff({
        removed_classes: [C, U],
        removed_subclass_of: [
          { child: C, parent: U },
          { child: "http://ex#Sub", parent: C },
        ],
      });
      const deps = collectClassStrongDeps(diff, C);
      expect(deps.removedSubclassOf).toEqual([{ child: "http://ex#Sub", parent: C }]);
      expect(deps.skipped).toHaveLength(1);
      expect(deps.skipped[0].kind).toBe("hierarchy");
      expect(deps.skipped[0].missingClass).toBe("User");
    });

    it("~ 回滚后指向仍删除的类时跳过（按 before 定义判定）", () => {
      const diff = emptyDiff({
        removed_classes: [C, U],
        modified_properties: [
          {
            iri: "http://ex#rel",
            before: { iri: "http://ex#rel", kind: "object", domain: C, range: U },
            after: { iri: "http://ex#rel", kind: "object", domain: "http://ex#Other", range: U },
          },
        ],
      });
      const deps = collectClassStrongDeps(diff, C);
      expect(deps.modifiedProperties).toHaveLength(0);
      expect(deps.skipped[0].iri).toBe("http://ex#rel");
      expect(deps.skipped[0].missingClass).toBe("User");
    });

    it("撤销新增类方向不做对端过滤（保持既有连带删除语义）", () => {
      const diff = emptyDiff({
        added_classes: [C],
        removed_classes: [U],
        removed_properties: [rel("http://ex#x", C, U)],
        added_properties: [rel("http://ex#y", C, "http://ex#Advisor")],
      });
      const deps = collectClassStrongDeps(diff, C);
      expect(deps.classOp).toBe("add");
      expect(deps.skipped).toEqual([]);
      expect(deps.addedProperties).toHaveLength(1);
    });
  });
});

describe("isLeafRestoreBlockedByDeletedClass", () => {
  it("blocks restoring removed property while class still deleted", () => {
    const C = "http://ex#LeadSource";
    const diff = emptyDiff({
      removed_classes: [C],
      removed_properties: [
        { iri: "http://ex#leadSourceName", kind: "datatype", domain: C },
      ],
    });
    const msg = isLeafRestoreBlockedByDeletedClass(diff, {
      kind: "property",
      op: "remove",
      iri: "http://ex#leadSourceName",
    });
    expect(msg).toMatch(/请先恢复类/);
  });

  it("blocks restoring relation whose range is the deleted class", () => {
    const C = "http://ex#LeadSource";
    const U = "http://ex#Lead";
    const diff = emptyDiff({
      removed_classes: [C],
      removed_properties: [
        {
          iri: "http://ex#sourceChannel",
          kind: "object",
          domain: U,
          range: C,
        },
      ],
    });
    expect(
      isLeafRestoreBlockedByDeletedClass(diff, {
        kind: "property",
        op: "remove",
        iri: "http://ex#sourceChannel",
      }),
    ).toMatch(/请先恢复类/);
  });

  it("blocks even when kind is missing but range touches deleted class", () => {
    const C = "http://ex#LeadSource";
    const diff = emptyDiff({
      removed_classes: [C],
      removed_properties: [
        {
          iri: "http://ex#sourceChannel",
          kind: "",
          domain: "http://ex#Lead",
          range: C,
        },
      ],
    });
    expect(
      isLeafRestoreBlockedByDeletedClass(diff, {
        kind: "property",
        op: "remove",
        iri: "http://ex#sourceChannel",
      }),
    ).toMatch(/请先恢复类/);
  });

  it("allows revoking added property under new class", () => {
    const C = "http://ex#ChatRecord";
    const diff = emptyDiff({
      added_classes: [C],
      added_properties: [
        { iri: "http://ex#logTime", kind: "datatype", domain: C },
      ],
    });
    expect(
      isLeafRestoreBlockedByDeletedClass(diff, {
        kind: "property",
        op: "add",
        iri: "http://ex#logTime",
      }),
    ).toBeNull();
  });
});
