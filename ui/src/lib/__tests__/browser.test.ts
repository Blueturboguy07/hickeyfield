import { describe, expect, it } from "vitest";
import type { MediaRole } from "../../types";
import { UNKNOWN_COST_LABEL } from "../cost";
import {
  attachmentGap,
  blockReason,
  CATEGORY_ORDER,
  categoryChips,
  categoryIo,
  categoryLabel,
  filterBrowseModels,
  formatBrowsePrice,
  groupByCategory,
  hasBrowsePrice,
  inCategoryOrder,
  ioSummary,
  isBlocked,
  matchesQuery,
  MOCK_BROWSE_MODELS,
  NOT_STATED,
  orderedCategories,
  pageBrowseModels,
  priceUnit,
  requirementNote,
  roleList,
  type BrowseModel,
} from "../browser";

const model = (over: Partial<BrowseModel>): BrowseModel => ({
  id: "m",
  title: "Model",
  category: "text-to-video",
  description: "",
  price: { usd: null, unit: null, note: null },
  takesPrompt: true,
  acceptedRoles: [],
  requiredRoles: [],
  provider: "fal",
  runnable: true,
  unavailableReason: null,
  ...over,
});

/** A text-only model, a clip editor and an image editor — the three shapes. */
const catalog: BrowseModel[] = [
  model({
    id: "seedance",
    title: "Seedance 2.0 Text to Video",
    category: "text-to-video",
    description: "Cinematic output with native audio and multi-shot editing.",
    price: { usd: 0.3034, unit: "per second of 720p video", note: null },
  }),
  model({
    id: "bg-removal",
    title: "Video Background Removal",
    category: "video-to-video",
    description: "Remove background from any video. No green screen needed.",
    price: { usd: 0.012, unit: "per 30 frames", note: null },
    takesPrompt: false,
    acceptedRoles: ["video"],
    requiredRoles: ["video"],
  }),
  model({
    id: "nano-banana-edit",
    title: "Nano Banana 2",
    category: "image-to-image",
    description: "State-of-the-art image generation and editing.",
    price: { usd: 0.08, unit: "per image", note: null },
    acceptedRoles: ["reference", "video", "audio"],
  }),
];

const attached = (...roles: MediaRole[]): MediaRole[] => roles;

/* ── Categories ─────────────────────────────────────────────────────────── */

describe("categoryLabel", () => {
  it("titles the a-to-b slugs the user browses by", () => {
    expect(categoryLabel("video-to-video")).toBe("Video to Video");
    expect(categoryLabel("image-to-image")).toBe("Image to Image");
    expect(categoryLabel("text-to-video")).toBe("Text to Video");
  });

  it("keeps acronyms uppercase rather than sentence-casing them", () => {
    expect(categoryLabel("image-to-3d")).toBe("Image to 3D");
    expect(categoryLabel("3d-to-3d")).toBe("3D to 3D");
    expect(categoryLabel("text-to-json")).toBe("Text to JSON");
    expect(categoryLabel("llm")).toBe("LLM");
  });

  it("labels the non-directional categories too", () => {
    expect(categoryLabel("training")).toBe("Training");
    expect(categoryLabel("vision")).toBe("Vision");
    expect(categoryLabel("unknown")).toBe("Unknown");
  });

  it("never returns an empty heading, which would merge two sections", () => {
    expect(categoryLabel("")).toBe("Uncategorized");
    expect(categoryLabel("   ")).toBe("Uncategorized");
  });

  it("ignores surrounding case and whitespace", () => {
    expect(categoryLabel(" Video-To-Video ")).toBe("Video to Video");
  });
});

describe("CATEGORY_ORDER", () => {
  it("holds every category fal published on 2026-08-05, with no duplicates", () => {
    expect(new Set(CATEGORY_ORDER).size).toBe(CATEGORY_ORDER.length);
    expect(CATEGORY_ORDER).toHaveLength(26);
  });

  it("leads with the video categories this product is for", () => {
    expect(CATEGORY_ORDER[0]).toBe("video-to-video");
    expect(CATEGORY_ORDER.indexOf("text-to-video")).toBeLessThan(
      CATEGORY_ORDER.indexOf("training"),
    );
  });
});

describe("categoryIo", () => {
  it("reads the input and output out of a directional slug", () => {
    expect(categoryIo("video-to-video")).toEqual({
      takes: "video",
      produces: "video",
    });
    expect(categoryIo("text-to-image")).toEqual({
      takes: "prompt",
      produces: "image",
    });
    expect(categoryIo("image-to-3d")).toEqual({
      takes: "image",
      produces: "3D model",
    });
  });

  it("guesses nothing for a category with no direction in it", () => {
    expect(categoryIo("training")).toEqual({ takes: null, produces: null });
    expect(categoryIo("llm")).toEqual({ takes: null, produces: null });
    expect(categoryIo("")).toEqual({ takes: null, produces: null });
  });

  it("returns null for a noun it does not know rather than echoing the slug", () => {
    expect(categoryIo("hologram-to-video").takes).toBeNull();
    expect(categoryIo("hologram-to-video").produces).toBe("video");
  });
});

/* ── The capability line ────────────────────────────────────────────────── */

describe("ioSummary", () => {
  it("states a text-only model as taking a prompt and nothing else", () => {
    // The bug: this row said nothing, so a user attached a clip to it.
    expect(ioSummary(catalog[0])).toEqual({
      takes: "prompt",
      produces: "video",
    });
  });

  it("lists the declared media inputs, prompt first", () => {
    expect(ioSummary(catalog[1]).takes).toBe("video");
    expect(
      ioSummary(model({ takesPrompt: true, acceptedRoles: ["start", "end"] }))
        .takes,
    ).toBe("prompt + start frame + end frame");
  });

  it("prefers the endpoint's declared inputs over the category's claim", () => {
    // Categorised image-to-image, yet it declares video and audio inputs.
    expect(ioSummary(catalog[2]).takes).toBe(
      "prompt + reference + video + audio",
    );
    expect(ioSummary(catalog[2]).produces).toBe("image");
  });

  it("falls back to the category only when nothing is declared", () => {
    expect(
      ioSummary(
        model({ category: "image-to-video", takesPrompt: false }),
      ).takes,
    ).toBe("image");
  });

  it("says so plainly when neither source states anything", () => {
    expect(ioSummary(model({ category: "training", takesPrompt: false }))).toEqual(
      { takes: NOT_STATED, produces: NOT_STATED },
    );
  });

  it("does not repeat a role that appears twice", () => {
    expect(
      ioSummary(model({ takesPrompt: false, acceptedRoles: ["video", "video"] }))
        .takes,
    ).toBe("video");
  });
});

describe("requirementNote", () => {
  it("names what the endpoint will not run without", () => {
    expect(requirementNote(catalog[1])).toBe("Requires video");
    expect(
      requirementNote(model({ requiredRoles: ["video", "start"] })),
    ).toBe("Requires video and start frame");
  });

  it("is silent when nothing is mandatory", () => {
    expect(requirementNote(catalog[0])).toBeNull();
  });
});

describe("roleList", () => {
  it("joins with a comma and a final and", () => {
    expect(roleList(["video"])).toBe("video");
    expect(roleList(["video", "audio"])).toBe("video and audio");
    expect(roleList(["video", "audio", "reference"])).toBe(
      "video, audio and reference",
    );
  });

  it("uses the same words as the Rust MediaRole labels", () => {
    expect(roleList(["start", "end", "video_reference"])).toBe(
      "start frame, end frame and video reference",
    );
  });
});

/* ── Search ─────────────────────────────────────────────────────────────── */

describe("matchesQuery", () => {
  it("searches the title and the description", () => {
    expect(matchesQuery(catalog[1], "background")).toBe(true);
    expect(matchesQuery(catalog[1], "green screen")).toBe(true);
    expect(matchesQuery(catalog[1], "seedance")).toBe(false);
  });

  it("requires every token, in any order", () => {
    expect(matchesQuery(catalog[0], "seedance cinematic")).toBe(true);
    expect(matchesQuery(catalog[0], "cinematic seedance")).toBe(true);
    expect(matchesQuery(catalog[0], "seedance background")).toBe(false);
  });

  it("ignores case and surrounding whitespace", () => {
    expect(matchesQuery(catalog[2], "  NANO banana ")).toBe(true);
  });

  it("matches everything on an empty query", () => {
    expect(matchesQuery(catalog[0], "")).toBe(true);
    expect(matchesQuery(catalog[0], "   ")).toBe(true);
  });

  it("does not match on the category, which is the chip row's job", () => {
    // Folding the category in would make all 385 image-to-image models match
    // "image" and bury the model actually named that.
    expect(matchesQuery(catalog[1], "video-to-video")).toBe(false);
  });
});

/* ── Attachment compatibility ───────────────────────────────────────────── */

describe("attachmentGap", () => {
  it("reports the attached role a text-only model has no input for", () => {
    expect(attachmentGap(catalog[0], attached("video"))).toEqual(["video"]);
  });

  it("is empty when the model declares an input for everything attached", () => {
    expect(attachmentGap(catalog[1], attached("video"))).toEqual([]);
    expect(attachmentGap(catalog[2], attached("video", "audio"))).toEqual([]);
  });

  it("is empty when nothing is attached", () => {
    expect(attachmentGap(catalog[0], [])).toEqual([]);
  });

  it("de-duplicates so the reason never repeats a role", () => {
    expect(attachmentGap(catalog[0], attached("video", "video"))).toEqual([
      "video",
    ]);
  });

  it("reports only the unmet roles when some are accepted", () => {
    expect(attachmentGap(catalog[2], attached("video", "start"))).toEqual([
      "start",
    ]);
  });
});

describe("blockReason", () => {
  it("explains the mismatch that produced an unrelated generation", () => {
    expect(blockReason(catalog[0], attached("video"))).toBe(
      "Does not accept the attached video",
    );
    expect(isBlocked(catalog[0], attached("video"))).toBe(true);
  });

  it("names every unmet role in one sentence", () => {
    expect(blockReason(catalog[0], attached("video", "audio"))).toBe(
      "Does not accept the attached video and audio",
    );
  });

  it("is null for a model that can run with what is attached", () => {
    expect(blockReason(catalog[1], attached("video"))).toBeNull();
    expect(blockReason(catalog[0], [])).toBeNull();
    expect(isBlocked(catalog[0], [])).toBe(false);
  });

  it("reports the credential failure ahead of the attachment mismatch", () => {
    // Removing the attachment would not make an unroutable model run, so
    // leading with the media reason sends the user to fix the wrong thing.
    const unroutable = model({
      runnable: false,
      unavailableReason: "No fal credentials — add a key in Settings",
    });
    expect(blockReason(unroutable, attached("video"))).toBe(
      "No fal credentials — add a key in Settings",
    );
  });

  it("never greys a row without saying why", () => {
    expect(blockReason(model({ runnable: false, unavailableReason: null }))).toBe(
      "No usable route for this model",
    );
    expect(blockReason(model({ runnable: false, unavailableReason: "  " }))).toBe(
      "No usable route for this model",
    );
  });
});

/* ── Price ──────────────────────────────────────────────────────────────── */

describe("formatBrowsePrice", () => {
  it("renders a published price at the shared cost precision", () => {
    expect(formatBrowsePrice({ usd: 2.1, unit: null, note: null })).toBe("$2.10");
    expect(formatBrowsePrice({ usd: 0.28, unit: null, note: null })).toBe("$0.28");
    // Sub-dime prices keep a third digit; two would round a real per-image
    // charge to "$0.00", which is the false-free reading we refuse everywhere.
    expect(formatBrowsePrice({ usd: 0.08, unit: null, note: null })).toBe(
      "$0.080",
    );
    expect(formatBrowsePrice({ usd: 0.002, unit: null, note: null })).toBe(
      "$0.0020",
    );
  });

  it("says the price is unavailable rather than free when none is published", () => {
    expect(formatBrowsePrice({ usd: null, unit: null, note: null })).toBe(
      UNKNOWN_COST_LABEL,
    );
    expect(formatBrowsePrice(null)).toBe(UNKNOWN_COST_LABEL);
    expect(formatBrowsePrice(undefined)).toBe(UNKNOWN_COST_LABEL);
  });

  it("never renders zero as a price", () => {
    // No model in the catalogue costs nothing, so a zero is a failed parse
    // upstream. "$0.00" on a row that then bills the user is the one number
    // that would discredit every other number in the app.
    expect(formatBrowsePrice({ usd: 0, unit: null, note: null })).toBe(
      UNKNOWN_COST_LABEL,
    );
    expect(formatBrowsePrice({ usd: -1, unit: null, note: null })).toBe(
      UNKNOWN_COST_LABEL,
    );
    expect(formatBrowsePrice({ usd: NaN, unit: null, note: null })).toBe(
      UNKNOWN_COST_LABEL,
    );
  });

  it("reports whether a number can be relied on", () => {
    expect(hasBrowsePrice({ usd: 0.08, unit: null, note: null })).toBe(true);
    expect(hasBrowsePrice({ usd: 0, unit: null, note: null })).toBe(false);
  });
});

describe("priceUnit", () => {
  it("qualifies a real number", () => {
    expect(priceUnit({ usd: 0.08, unit: "per image", note: null })).toBe(
      "per image",
    );
  });

  it("is suppressed when there is no number for it to qualify", () => {
    // "per image" under "Price unavailable" reads as a price we withheld.
    expect(priceUnit({ usd: null, unit: "per image", note: null })).toBeNull();
    expect(priceUnit({ usd: 0.08, unit: "  ", note: null })).toBeNull();
  });
});

/* ── Filtering ──────────────────────────────────────────────────────────── */

describe("filterBrowseModels", () => {
  it("returns everything by default", () => {
    expect(filterBrowseModels(catalog)).toHaveLength(3);
  });

  it("filters by category, treating null as no filter", () => {
    expect(
      filterBrowseModels(catalog, { category: "video-to-video" }).map((m) => m.id),
    ).toEqual(["bg-removal"]);
    expect(filterBrowseModels(catalog, { category: null })).toHaveLength(3);
  });

  it("keeps only models that accept everything attached", () => {
    const out = filterBrowseModels(catalog, {
      attachedRoles: attached("video"),
      onlyCompatible: true,
    });
    expect(out.map((m) => m.id)).toEqual(["bg-removal", "nano-banana-edit"]);
  });

  it("is a no-op when the toggle is on but nothing is attached", () => {
    // A toggle that empties the catalogue reads as a broken app rather than
    // as an unmatched query.
    expect(
      filterBrowseModels(catalog, { onlyCompatible: true, attachedRoles: [] }),
    ).toHaveLength(3);
  });

  it("does not hide incompatible models unless asked to", () => {
    expect(
      filterBrowseModels(catalog, { attachedRoles: attached("video") }),
    ).toHaveLength(3);
  });

  it("does not hide models that merely cannot run", () => {
    // Unrunnable rows are greyed with a reason, never removed — the user has
    // to be able to see that the model exists and what would unlock it.
    const withDead = [...catalog, model({ id: "dead", runnable: false })];
    expect(filterBrowseModels(withDead)).toHaveLength(4);
  });

  it("combines category, query and attachment", () => {
    expect(
      filterBrowseModels(catalog, {
        category: "image-to-image",
        query: "editing",
        attachedRoles: attached("video"),
        onlyCompatible: true,
      }).map((m) => m.id),
    ).toEqual(["nano-banana-edit"]);

    expect(
      filterBrowseModels(catalog, {
        category: "text-to-video",
        attachedRoles: attached("video"),
        onlyCompatible: true,
      }),
    ).toEqual([]);
  });

  it("preserves catalogue order", () => {
    expect(filterBrowseModels(catalog, { query: "e" }).map((m) => m.id)).toEqual([
      "seedance",
      "bg-removal",
      "nano-banana-edit",
    ]);
  });
});

/* ── Grouping ───────────────────────────────────────────────────────────── */

describe("orderedCategories", () => {
  it("orders by CATEGORY_ORDER, not by catalogue order", () => {
    expect(orderedCategories(catalog)).toEqual([
      "video-to-video",
      "text-to-video",
      "image-to-image",
    ]);
  });

  it("appends a category it has never seen rather than dropping it", () => {
    // A dropped category hides its models with no chip to reveal them, which
    // is the exact opacity this surface exists to remove.
    const exotic = [
      ...catalog,
      model({ id: "z", category: "zebra-to-video" }),
      model({ id: "a", category: "aardvark" }),
    ];
    expect(orderedCategories(exotic)).toEqual([
      "video-to-video",
      "text-to-video",
      "image-to-image",
      "aardvark",
      "zebra-to-video",
    ]);
  });

  it("reports each category once", () => {
    const dupes = [...catalog, model({ id: "second", category: "text-to-video" })];
    expect(orderedCategories(dupes)).toHaveLength(3);
  });
});

describe("categoryChips", () => {
  it("counts what clicking the chip will actually show", () => {
    const dupes = [...catalog, model({ id: "second", category: "text-to-video" })];
    expect(categoryChips(dupes)).toEqual([
      { category: "video-to-video", label: "Video to Video", count: 1 },
      { category: "text-to-video", label: "Text to Video", count: 2 },
      { category: "image-to-image", label: "Image to Image", count: 1 },
    ]);
  });

  it("has no chips for an empty catalogue", () => {
    expect(categoryChips([])).toEqual([]);
  });
});

describe("groupByCategory", () => {
  it("groups in chip order and keeps input order inside a group", () => {
    const extra = model({ id: "second", category: "text-to-video" });
    const groups = groupByCategory([...catalog, extra]);
    expect(groups.map((g) => g.category)).toEqual([
      "video-to-video",
      "text-to-video",
      "image-to-image",
    ]);
    expect(groups[1].models.map((m) => m.id)).toEqual(["seedance", "second"]);
    expect(groups[0].label).toBe("Video to Video");
  });

  it("emits no empty groups", () => {
    expect(groupByCategory([]).length).toBe(0);
    expect(groupByCategory(catalog).every((g) => g.models.length > 0)).toBe(true);
  });
});

describe("inCategoryOrder", () => {
  it("flattens into the order the sections render in", () => {
    // Paging catalogue order instead would drop new rows into sections the
    // user has already scrolled past when they press Load more.
    expect(inCategoryOrder(catalog).map((m) => m.id)).toEqual([
      "bg-removal",
      "seedance",
      "nano-banana-edit",
    ]);
  });

  it("loses nothing, including categories it has never seen", () => {
    const exotic = [...catalog, model({ id: "z", category: "zebra-to-video" })];
    expect(inCategoryOrder(exotic)).toHaveLength(4);
    expect(inCategoryOrder(exotic).map((m) => m.id)).toContain("z");
  });

  it("makes Load more append-only", () => {
    const ordered = inCategoryOrder(catalog);
    const first = pageBrowseModels(ordered, 1).page;
    const second = pageBrowseModels(ordered, 2).page;
    expect(second.slice(0, 1)).toEqual(first);
  });
});

describe("pageBrowseModels", () => {
  it("pages and reports the remainder", () => {
    expect(pageBrowseModels(catalog, 2)).toEqual({
      page: [catalog[0], catalog[1]],
      remaining: 1,
    });
  });

  it("never slices past the end or below zero", () => {
    expect(pageBrowseModels(catalog, 99).remaining).toBe(0);
    expect(pageBrowseModels(catalog, -5).page).toEqual([]);
  });
});

/* ── Mock catalogue ─────────────────────────────────────────────────────── */

describe("MOCK_BROWSE_MODELS", () => {
  it("never claims a zero price", () => {
    for (const m of MOCK_BROWSE_MODELS) {
      expect(m.price.usd === null || m.price.usd > 0).toBe(true);
    }
  });

  it("includes an unpriced model, which is the majority case upstream", () => {
    const unpriced = MOCK_BROWSE_MODELS.filter((m) => !hasBrowsePrice(m.price));
    expect(unpriced.length).toBeGreaterThan(0);
    expect(formatBrowsePrice(unpriced[0].price)).toBe(UNKNOWN_COST_LABEL);
  });

  it("carries the text-only model that reproduces the attachment bug", () => {
    const seedance = MOCK_BROWSE_MODELS.find(
      (m) => m.id === "bytedance/seedance-2.0/text-to-video",
    );
    expect(seedance).toBeDefined();
    expect(seedance!.acceptedRoles).toEqual([]);
    expect(blockReason(seedance!, attached("video"))).toBe(
      "Does not accept the attached video",
    );
  });

  it("includes a row that cannot run, so the greyed path renders in dev", () => {
    const dead = MOCK_BROWSE_MODELS.filter((m) => !m.runnable);
    expect(dead.length).toBeGreaterThan(0);
    for (const m of dead) expect(blockReason(m)).toBeTruthy();
  });

  it("only uses categories the provider actually publishes", () => {
    for (const m of MOCK_BROWSE_MODELS) {
      expect(CATEGORY_ORDER).toContain(m.category);
    }
  });

  it("requires nothing it does not also accept", () => {
    for (const m of MOCK_BROWSE_MODELS) {
      for (const role of m.requiredRoles) {
        expect(m.acceptedRoles).toContain(role);
      }
    }
  });
});
