import { describe, expect, it } from "vitest";
import type {
  GenSettings,
  Model,
  ModelCapabilities,
  PresetFamily,
} from "../../types";
import {
  capabilitiesFor,
  UNKNOWN_CAPABILITIES,
  durationLadder,
  isNativeVariant,
  resolveSettings,
  routeLabel,
  selectRoute,
  selectVariant,
} from "../variants";

const model = (over: Partial<Model> = {}): Model => ({
  id: "kling_3_0",
  displayName: "Kling 3.0",
  modality: "video",
  isLaunch: true,
  routes: [
    { id: "fal:kling", provider: "fal", slug: "kling" },
    { id: "vaig:kling", provider: "vaig", slug: "kling-3" },
  ],
  ...over,
});

const family = (variants?: PresetFamily["variants"]): PresetFamily => ({
  id: "orbit",
  displayName: "Orbit 360",
  category: "Camera Control",
  tags: [],
  description: "",
  variants,
});

const settings = (over: Partial<GenSettings> = {}): GenSettings => ({
  duration: 5,
  resolution: "1080p",
  aspect: "16:9",
  audio: false,
  enhance: true,
  ...over,
});

describe("selectRoute", () => {
  it("honours a preference the model still exposes", () => {
    expect(selectRoute(model(), "vaig:kling")?.provider).toBe("vaig");
  });

  it("falls back to the first route when the preference is stale", () => {
    expect(selectRoute(model(), "deleted:route")?.provider).toBe("fal");
  });

  it("returns null when there is nothing to run on", () => {
    expect(selectRoute(model({ routes: [] }))).toBeNull();
    expect(selectRoute(null)).toBeNull();
  });
});

describe("routeLabel", () => {
  it("renders provider and slug, and survives a missing route", () => {
    expect(routeLabel({ id: "a", provider: "fal", slug: "kling" })).toBe(
      "fal · kling",
    );
    expect(routeLabel(null)).toBe("No route");
  });
});

describe("selectVariant", () => {
  it("prefers an exact model match", () => {
    const f = family([{ modelId: "wan_2_7" }, { modelId: "kling_3_0" }]);
    expect(selectVariant(f, "kling_3_0")?.modelId).toBe("kling_3_0");
  });

  it("falls back to the family default rather than hiding the preset", () => {
    const f = family([{ modelId: "wan_2_7" }]);
    expect(selectVariant(f, "kling_3_0")?.modelId).toBe("wan_2_7");
  });

  it("returns null for a family with no variants at all", () => {
    expect(selectVariant(family(), "kling_3_0")).toBeNull();
    expect(selectVariant(family([]), "kling_3_0")).toBeNull();
    expect(selectVariant(null, "kling_3_0")).toBeNull();
  });

  it("reports whether the variant was authored or derived", () => {
    const f = family([{ modelId: "wan_2_7" }]);
    expect(isNativeVariant(f, "wan_2_7")).toBe(true);
    expect(isNativeVariant(f, "kling_3_0")).toBe(false);
    expect(isNativeVariant(family(), "wan_2_7")).toBe(false);
  });
});

const caps4 = (over: Partial<ModelCapabilities> = {}): ModelCapabilities => ({
  ...UNKNOWN_CAPABILITIES,
  supportsDuration: true,
  durations: [4, 6, 8],
  supportsResolution: true,
  resolutions: ["720p", "1080p"],
  supportsAspect: true,
  aspects: ["16:9", "9:16"],
  ...over,
});

describe("capabilitiesFor", () => {
  it("claims nothing when the shell sent no descriptor", () => {
    // The bug this replaces: assuming durations [5, 8, 10] and resolutions
    // 720p/1080p for every model, so the chip row offered 10s on a 5s-only
    // model, the estimator quoted 10s and the provider rejected the job.
    expect(capabilitiesFor(model())).toEqual(UNKNOWN_CAPABILITIES);
    expect(capabilitiesFor(null)).toEqual(UNKNOWN_CAPABILITIES);
  });

  it("renders no chips at all rather than plausible wrong ones", () => {
    const caps = capabilitiesFor(model());
    expect(caps.supportsDuration).toBe(false);
    expect(caps.resolutions).toEqual([]);
    expect(caps.aspects).toEqual([]);
  });

  it("passes the model's own descriptor straight through", () => {
    const own = caps4({ audio: true });
    expect(capabilitiesFor(model({ capabilities: own }))).toBe(own);
  });
});

describe("durationLadder", () => {
  it("uses the model's enumerated durations when it has them", () => {
    expect(durationLadder(caps4())).toEqual([4, 6, 8]);
  });

  it("offers a common ladder for a free-form duration", () => {
    // 28 of 32 video models declare duration as a plain integer.
    const ladder = durationLadder(
      caps4({ durations: [], defaultDuration: 5 }),
    );
    expect(ladder).toContain(5);
    expect(ladder.length).toBeGreaterThan(3);
  });

  it("includes an unusual default rather than dropping it", () => {
    expect(durationLadder(caps4({ durations: [], defaultDuration: 7 }))).toContain(7);
  });

  it("offers nothing when the model has no duration axis", () => {
    expect(durationLadder(UNKNOWN_CAPABILITIES)).toEqual([]);
  });
});

describe("resolveSettings", () => {
  const caps = caps4();

  it("returns the same object when nothing needs clamping", () => {
    const s = settings({ duration: 8 });
    expect(resolveSettings(s, caps)).toBe(s);
  });

  it("snaps an unsupported duration to the nearest, preferring the cheaper", () => {
    expect(resolveSettings(settings({ duration: 5 }), caps).duration).toBe(4);
    expect(resolveSettings(settings({ duration: 7 }), caps).duration).toBe(6);
    expect(resolveSettings(settings({ duration: 20 }), caps).duration).toBe(8);
  });

  it("prefers the model's own default over the first option", () => {
    // "First in the list" is arbitrary, and several models list their cheapest
    // or lowest-quality option first.
    const out = resolveSettings(
      settings({ duration: 4, resolution: "4K", aspect: "21:9" }),
      caps4({ defaultResolution: "1080p", defaultAspect: "9:16" }),
    );
    expect(out.resolution).toBe("1080p");
    expect(out.aspect).toBe("9:16");
  });

  it("falls back to the first supported resolution and aspect", () => {
    const out = resolveSettings(
      settings({ duration: 4, resolution: "4K", aspect: "21:9" }),
      caps,
    );
    expect(out.resolution).toBe("720p");
    expect(out.aspect).toBe("16:9");
  });

  it("switches audio off on a model that has none", () => {
    expect(resolveSettings(settings({ duration: 4, audio: true }), caps).audio).toBe(
      false,
    );
    expect(
      resolveSettings(settings({ duration: 4, audio: true }), {
        ...caps,
        audio: true,
      }).audio,
    ).toBe(true);
  });

  it("leaves duration alone for models with no duration axis", () => {
    const out = resolveSettings(
      settings({ duration: 5 }),
      caps4({ durations: [], resolutions: ["1K"], aspects: ["1:1"] }),
    );
    expect(out.duration).toBe(5);
    expect(out.resolution).toBe("1K");
  });
});
