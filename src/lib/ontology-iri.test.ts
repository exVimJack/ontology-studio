import { describe, expect, it } from "vitest";
import {
  DEFAULT_ONTOLOGY_IRI_PREFIX,
  buildOntologyIri,
  isValidOntologyCode,
  isValidOntologyIriPrefix,
  normalizeOntologyIriPrefix,
} from "./ontology-iri";

describe("ontology-iri", () => {
  it("normalizes prefix with trailing slash", () => {
    expect(normalizeOntologyIriPrefix("https://onto-studio.local/ontology")).toBe(
      "https://onto-studio.local/ontology/",
    );
    expect(
      normalizeOntologyIriPrefix("https://onto-studio.local/ontology/"),
    ).toBe("https://onto-studio.local/ontology/");
    expect(normalizeOntologyIriPrefix("https://ex.org/ns#")).toBe(
      "https://ex.org/ns#",
    );
  });

  it("builds iri as prefix + code + #", () => {
    expect(buildOntologyIri(DEFAULT_ONTOLOGY_IRI_PREFIX, "UsedCar")).toBe(
      "https://onto-studio.local/ontology/UsedCar#",
    );
  });

  it("validates code pattern", () => {
    expect(isValidOntologyCode("UsedCar")).toBe(true);
    expect(isValidOntologyCode("used_car-1")).toBe(true);
    expect(isValidOntologyCode("1bad")).toBe(false);
    expect(isValidOntologyCode("has space")).toBe(false);
    expect(isValidOntologyCode("")).toBe(false);
  });

  it("validates prefix must be http(s)", () => {
    expect(isValidOntologyIriPrefix("https://onto-studio.local/ontology/")).toBe(
      true,
    );
    expect(isValidOntologyIriPrefix("ftp://x/")).toBe(false);
    expect(isValidOntologyIriPrefix("not-a-url")).toBe(false);
  });
});
