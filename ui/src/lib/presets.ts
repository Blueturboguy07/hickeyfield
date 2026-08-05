import type { PresetFamily } from "../types";

/**
 * The category chip row in the preset picker.
 *
 * Seven, in this order. "All" is a chip rather than a cleared filter so the
 * row always has a visibly selected item — a chip row with nothing lit reads
 * as broken rather than unfiltered.
 */
export const PRESET_CATEGORIES = [
  "All",
  "Trending",
  "Camera Control",
  "Effects",
  "Viral",
  "Mood",
  "Anime",
] as const;

export type PresetCategory = (typeof PRESET_CATEGORIES)[number];

export interface PresetQuery {
  query?: string;
  category?: string;
  /** Keep only families that can compile for this model. */
  modelId?: string;
}

const norm = (s: string) => s.trim().toLowerCase();

function haystack(preset: PresetFamily): string {
  return norm(
    [preset.displayName, preset.description, preset.category, ...preset.tags]
      .filter(Boolean)
      .join(" "),
  );
}

/**
 * A family with no `variants` array is model-agnostic — it compiles from the
 * generic template for whatever model is selected. Treating a missing array as
 * "supports nothing" would empty the picker for every model the seed catalog
 * has not been annotated for yet.
 */
export function supportsModel(preset: PresetFamily, modelId: string): boolean {
  if (!preset.variants || preset.variants.length === 0) return true;
  return preset.variants.some((v) => v.modelId === modelId);
}

/**
 * All search tokens must match, in any field and any order. Users type
 * "camera orbit" expecting the intersection, not the union.
 */
export function matchesQuery(preset: PresetFamily, query: string): boolean {
  const tokens = norm(query).split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return true;
  const hay = haystack(preset);
  return tokens.every((t) => hay.includes(t));
}

export function filterPresets(
  presets: PresetFamily[],
  { query = "", category = "All", modelId }: PresetQuery = {},
): PresetFamily[] {
  const wantCategory = norm(category);
  return presets.filter((preset) => {
    if (wantCategory && wantCategory !== "all") {
      if (norm(preset.category) !== wantCategory) return false;
    }
    if (modelId && !supportsModel(preset, modelId)) return false;
    return matchesQuery(preset, query);
  });
}

/** Categories present in a catalog, in chip order, dropping empty ones. */
export function availableCategories(presets: PresetFamily[]): string[] {
  const present = new Set(presets.map((p) => norm(p.category)));
  return PRESET_CATEGORIES.filter(
    (c) => norm(c) === "all" || present.has(norm(c)),
  );
}

/** Paging for "Load more". Never slices past the end. */
export function pagePresets(
  presets: PresetFamily[],
  shown: number,
): { page: PresetFamily[]; remaining: number } {
  const clamped = Math.max(0, Math.min(shown, presets.length));
  return {
    page: presets.slice(0, clamped),
    remaining: presets.length - clamped,
  };
}
