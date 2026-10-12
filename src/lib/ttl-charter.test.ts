import { describe, expect, it } from "vitest";
import {
  extractTtlCharterFromContent,
  hasCharterContent,
} from "@/lib/ttl-charter";
import type { TtlCharter } from "@/lib/domain";

const TTL = `@prefix : <https://jslc.auto-retail.cn/ontology/AutoDealerRetail#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .

:AutoDealerRetailOntology a owl:Ontology ;
    rdfs:comment "汽车经销商零售业务本体：覆盖门店、库存、订单、售后" ;
    dcterms:description "汽车经销商零售领域知识图谱" ;
    skos:definition "核心业务对象为车辆与门店" ;
    owl:versionInfo "1.0" .

:Vehicle a owl:Class ;
    rdfs:comment "车辆" .
`;

describe("ttl-charter", () => {
  it("从 Turtle 提取宪章四字段", () => {
    const c = extractTtlCharterFromContent(TTL);
    expect(c).not.toBeNull();
    expect(c?.business_scenario).toBe("汽车经销商零售领域知识图谱");
    expect(c?.business_essence).toBe("核心业务对象为车辆与门店");
  });

  it("hasCharterContent 判断是否有内容", () => {
    const c: TtlCharter = {
      business_scenario: "",
      business_essence: "",
      updated_at: 0,
    };
    expect(hasCharterContent(c)).toBe(false);
  });
});