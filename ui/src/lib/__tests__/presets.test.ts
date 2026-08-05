import { describe, expect, it } from "vitest";
import type { PresetFamily } from "../../types";
import {
  availableCategories,
  filterPresets,
  matchesQuery,
  pagePresets,
  PRESET_CATEGORIES,
  supportsModel,
} from "../presets";

const preset = (over: Partial<PresetFamily>): PresetFamily => ({
  id: "p",
  displayName: "Preset",
  category: "Effects",
  tags: [],
  description: "",
  ...over,
});

const catalog: PresetFamily[] = [
  preset({
    id: "orbit",
    displayName: "Orbit 360",
    category: "Camera Control",
    tags: ["orbit", "camera"],
    description: "A full circular orbit around the subject.",
    variants: [{ modelId: "kling_3_0" }, { modelId: "wan_2_7" }],
  }),
  preset({
    id: "datamosh",
    displayName: "Datamosh",
    category: "Effects",
    tags: ["glitch"],
    description: "Pixels bleed between frames.",
    variants: [{ modelId: "seedance_2_0" }],
  }),
  preset({
    id: "neon",
    displayName: "Neon City",
    category: "Mood",
    tags: ["neon", "night"],
    description: "Wet asphalt and saturated signage.",
  }),
];

describe("PRESET_CATEGORIES", () => {
  it("is the seven chips, starting with All", () => {
    expect(PRESET_CATEGORIES).toHaveLength(7);
    expect(PRESET_CATEGORIES[0]).toBe("All");
  });
});

describe("matchesQuery", () => {
  it("searches name, description, category and tags", () => {
    const p = catalog[0];
    expect(matchesQuery(p, "orbit")).toBe(true);
    expect(matchesQuery(p, "circular")).toBe(true);
    expect(matchesQuery(p, "camera control")).toBe(true);
    expect(matchesQuery(p, "glitch")).toBe(false);
  });

  it("requires every token, in any order", () => {
    const p = catalog[0];
    expect(matchesQuery(p, "orbit subject")).toBe(true);
    expect(matchesQuery(p, "subject orbit")).toBe(true);
    expect(matchesQuery(p, "orbit glitch")).toBe(false);
  });

  it("ignores case and surrounding whitespace", () => {
    expect(matchesQuery(catalog[1], "  DATAMOSH ")).toBe(true);
  });

  it("matches everything on an empty query", () => {
    expect(matchesQuery(catalog[2], "")).toBe(true);
    expect(matchesQuery(catalog[2], "   ")).toBe(true);
  });
});

describe("supportsModel", () => {
  it("respects an explicit variant list", () => {
    expect(supportsModel(catalog[0], "kling_3_0")).toBe(true);
    expect(supportsModel(catalog[0], "seedance_2_0")).toBe(false);
  });

  it("treats a family with no variants as model-agnostic", () => {
    expect(supportsModel(catalog[2], "anything_at_all")).toBe(true);
  });
});

describe("filterPresets", () => {
  it("returns everything by default", () => {
    expect(filterPresets(catalog)).toHaveLength(3);
  });

  it("filters by category, treating All as no filter", () => {
    expect(filterPresets(catalog, { category: "Mood" })).toHaveLength(1);
    expect(filterPresets(catalog, { category: "All" })).toHaveLength(3);
  });

  it("filters by model, keeping model-agnostic families", () => {
    const out = filterPresets(catalog, { modelId: "kling_3_0" });
    expect(out.map((p) => p.id)).toEqual(["orbit", "neon"]);
  });

  it("combines category, model and query", () => {
    expect(
      filterPresets(catalog, {
        category: "Camera Control",
        modelId: "wan_2_7",
        query: "orbit",
      }).map((p) => p.id),
    ).toEqual(["orbit"]);

    expect(
      filterPresets(catalog, { category: "Camera Control", query: "neon" }),
    ).toEqual([]);
  });

  it("preserves catalog order", () => {
    expect(filterPresets(catalog, { query: "e" }).map((p) => p.id)).toEqual([
      "orbit",
      "datamosh",
      "neon",
    ]);
  });
});

describe("availableCategories", () => {
  it("drops chips with nothing behind them but keeps All", () => {
    expect(availableCategories(catalog)).toEqual([
      "All",
      "Camera Control",
      "Effects",
      "Mood",
    ]);
  });
});

describe("pagePresets", () => {
  it("pages and reports the remainder", () => {
    expect(pagePresets(catalog, 2)).toEqual({
      page: [catalog[0], catalog[1]],
      remaining: 1,
    });
  });

  it("never slices past the end or below zero", () => {
    expect(pagePresets(catalog, 99).remaining).toBe(0);
    expect(pagePresets(catalog, -5).page).toEqual([]);
  });
});
