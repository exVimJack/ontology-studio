import { describe, expect, it } from "vitest";
import type { TtlVersionDiff } from "@/lib/domain";
import type { ConceptEdge, ConceptNode } from "@/lib/ontology-graph";
import {
  applyEffectiveDiffToGraph,
  countEffectiveDiff,
  filterDiffRelatedTo,
} from "@/lib/ontology-effective-diff";

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

const lead: ConceptNode = {
  id: "线索",
  label: "线索",
  iri: "https://ex.org/ont#线索",
  kind: "class",
};

describe("countEffectiveDiff", () => {
  it("删属性计入 removed，不进 modified", () => {
    const c = countEffectiveDiff(
      emptyDiff({
        removed_properties: [
          {
            iri: "https://ex.org/ont#channel",
            kind: "datatype",
            label: "channel",
            domain: "https://ex.org/ont#线索",
          },
        ],
      }),
    );
    expect(c.removed).toBe(1);
    expect(c.removedProps).toBe(1);
    expect(c.modified).toBe(0);
  });
});

describe("applyEffectiveDiffToGraph — 删数据属性", () => {
  it("删除 channel 时：线索标黄；幽灵属性节点 + 幽灵边", () => {
    const nodes: ConceptNode[] = [lead];
    const edges: ConceptEdge[] = [];
    const { nodes: outN, edges: outE } = applyEffectiveDiffToGraph(
      nodes,
      edges,
      emptyDiff({
        removed_properties: [
          {
            iri: "https://ex.org/ont#channel",
            kind: "datatype",
            label: "channel",
            domain: "https://ex.org/ont#线索",
          },
        ],
      }),
    );
    const leadN = outN.find((n) => n.id === "线索");
    expect(leadN?.diffStatus).toBe("modified");
    const channel = outN.find((n) => n.iri.endsWith("#channel"));
    expect(channel?.kind).toBe("datatype");
    expect(channel?.diffStatus).toBe("removed");
    const ghostEdge = outE.find((e) => e.kind === "datatype" && e.diffStatus === "removed");
    expect(ghostEdge?.source).toBe("线索");
    expect(ghostEdge?.target).toBe(channel?.id);
  });

  it("不覆盖类自身的 added/removed 状态", () => {
    const { nodes: outN } = applyEffectiveDiffToGraph(
      [{ ...lead, diffStatus: undefined }],
      [],
      emptyDiff({
        added_classes: ["https://ex.org/ont#线索"],
        removed_properties: [
          {
            iri: "https://ex.org/ont#channel",
            kind: "datatype",
            domain: "https://ex.org/ont#线索",
          },
        ],
      }),
    );
    // added_classes 先 mark，tintDomain 不得降级
    expect(outN.find((n) => n.id === "线索")?.diffStatus).toBe("added");
  });

  it("新增数据属性也会给定义域标黄", () => {
    const { nodes: outN } = applyEffectiveDiffToGraph(
      [lead],
      [],
      emptyDiff({
        added_properties: [
          {
            iri: "https://ex.org/ont#foo",
            kind: "datatype",
            domain: "https://ex.org/ont#线索",
          },
        ],
      }),
    );
    expect(outN.find((n) => n.id === "线索")?.diffStatus).toBe("modified");
  });
});

describe("filterDiffRelatedTo", () => {
  const full = emptyDiff({
    added_classes: ["https://ex.org/ont#微信聊天记录"],
    modified_classes: [
      {
        iri: "https://ex.org/ont#线索",
        before: { iri: "https://ex.org/ont#线索", kind: "class", label: "a" },
        after: { iri: "https://ex.org/ont#线索", kind: "class", label: "b" },
      },
    ],
    removed_properties: [
      {
        iri: "https://ex.org/ont#channel",
        kind: "datatype",
        domain: "https://ex.org/ont#线索",
      },
    ],
    added_properties: [
      {
        iri: "https://ex.org/ont#otherProp",
        kind: "datatype",
        domain: "https://ex.org/ont#用户",
      },
    ],
    removed_subclass_of: [
      { child: "https://ex.org/ont#Dog", parent: "https://ex.org/ont#Animal" },
    ],
  });

  it("无 focus 返回原 diff", () => {
    expect(filterDiffRelatedTo(full, null)).toBe(full);
  });

  it("类选中含自身修改 + domain 删属性，不含无关类/属性", () => {
    const r = filterDiffRelatedTo(full, {
      kind: "class",
      iri: "https://ex.org/ont#线索",
    });
    expect(r.modified_classes).toHaveLength(1);
    expect(r.removed_properties.map((p) => p.iri)).toEqual([
      "https://ex.org/ont#channel",
    ]);
    expect(r.added_classes).toHaveLength(0);
    expect(r.added_properties).toHaveLength(0);
  });

  it("属性边选中仅该 pred", () => {
    const r = filterDiffRelatedTo(full, {
      kind: "property",
      iri: "https://ex.org/ont#channel",
    });
    expect(r.removed_properties).toHaveLength(1);
    expect(r.modified_classes).toHaveLength(0);
    expect(r.added_classes).toHaveLength(0);
  });

  it("继承边选中仅匹配的 subclass_of", () => {
    const r = filterDiffRelatedTo(full, {
      kind: "hierarchy",
      childIri: "https://ex.org/ont#Dog",
      parentIri: "https://ex.org/ont#Animal",
    });
    expect(r.removed_subclass_of).toHaveLength(1);
    expect(r.removed_properties).toHaveLength(0);
  });
});
