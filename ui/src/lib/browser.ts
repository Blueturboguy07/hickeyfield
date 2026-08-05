/**
 * The model browser's wire shape and every filtering decision it makes.
 *
 * Split out of the component because the question this surface answers — "what
 * can edit my clip?" — is a predicate over data, and the failure it exists to
 * prevent is silent. A user attached a video to a text-only model; the picker
 * offered no way to know; the provider ignored the attachment and returned an
 * unrelated generation that looked like a success. A predicate that lives
 * inside a component is a predicate nobody tests, which is how that shipped.
 *
 * The category vocabulary below is fal's own, read from its public catalogue
 * (`https://fal.ai/api/models?page=N`, unauthenticated) on 2026-08-05: 36
 * pages, 1418 live models, 26 distinct `category` values. It is transcribed
 * here only to *order* and *label* categories — the browser never drops a
 * category it does not recognise, because dropping one hides models, which is
 * the exact opacity this surface exists to remove.
 */

import type { MediaRole } from "../types";
import { UNKNOWN_COST_LABEL, formatUsd } from "./cost";

/* ── Wire shape ─────────────────────────────────────────────────────────── */

/**
 * What one call costs.
 *
 * `usd` is null far more often than it is set: 764 of fal's 1418 catalogue
 * entries publish no `pricingInfoOverride` at all, and several of the ones
 * that do describe token billing that no single number can express. Null is
 * therefore the routine state, not an error — and it is never zero. See
 * [`formatBrowsePrice`].
 */
export interface BrowsePrice {
  /** USD for one representative call, or null when nothing is published. */
  usd: number | null;
  /** What that number buys — "per image", "per second of video". Never guessed. */
  unit: string | null;
  /** The provider's own pricing prose, verbatim, for the row's tooltip. */
  note: string | null;
}

/**
 * One row of the browser.
 *
 * Mirrors the `browse_models` command. `acceptedRoles` and `requiredRoles` are
 * the load-bearing fields: they come from the endpoint's own published input
 * schema, not from the category, because the two genuinely disagree —
 * `fal-ai/nano-banana-2/edit` is categorised `image-to-image` and still
 * declares `video_url` and `audio_url` inputs.
 */
export interface BrowseModel {
  /** The same stable id `list_models` and `submit_job` use. */
  id: string;
  title: string;
  /** The provider's capability category, verbatim: "video-to-video", "llm", … */
  category: string;
  description: string;
  price: BrowsePrice;
  /** Whether a text prompt is one of the inputs. */
  takesPrompt: boolean;
  /** Media this endpoint declares an input for. Empty means text-only. */
  acceptedRoles: MediaRole[];
  /** The subset the endpoint refuses to run without. */
  requiredRoles: MediaRole[];
  /** Provider slug of the route that would run this, for the row's badge. */
  provider: string;
  /** Key present *and* a client implemented — same test as `RouteDto`. */
  runnable: boolean;
  /** Why not, when it cannot. Shown verbatim; never left blank on a grey row. */
  unavailableReason: string | null;
}

/* ── Media roles ────────────────────────────────────────────────────────── */

/**
 * Deliberately the same words as Rust's `MediaRole::label()`.
 *
 * Two vocabularies for one concept is one more thing to drift, and the drift
 * is invisible: a role labelled one way in the browser and another in the
 * error message reads as two different problems.
 */
export const ROLE_LABELS: Record<MediaRole, string> = {
  start: "start frame",
  end: "end frame",
  reference: "reference",
  video: "video",
  video_reference: "video reference",
  audio: "audio",
  audio_reference: "audio reference",
};

export function roleLabel(role: MediaRole): string {
  // An unrecognised role must still print as something a user can act on
  // rather than as `undefined`, which is what a bare lookup would render.
  return ROLE_LABELS[role] ?? role;
}

/** "video", "video and audio", "video, audio and reference". */
export function roleList(roles: MediaRole[]): string {
  const labels = roles.map(roleLabel);
  if (labels.length === 0) return "";
  if (labels.length === 1) return labels[0];
  return `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`;
}

function dedupe(roles: MediaRole[]): MediaRole[] {
  const seen = new Set<MediaRole>();
  const out: MediaRole[] = [];
  for (const r of roles) {
    if (!seen.has(r)) {
      seen.add(r);
      out.push(r);
    }
  }
  return out;
}

/**
 * Attached roles this model has no input for.
 *
 * The whole reason the browser exists. Two start frames attached is still one
 * unmet role, so the result is de-duplicated — otherwise the reason string
 * reads "does not accept the attached start frame and start frame".
 */
export function attachmentGap(
  model: BrowseModel,
  attached: MediaRole[],
): MediaRole[] {
  const accepted = new Set(model.acceptedRoles);
  return dedupe(attached).filter((role) => !accepted.has(role));
}

/* ── Categories ─────────────────────────────────────────────────────────── */

/**
 * Display order for the category chips and sections.
 *
 * These 26 are the complete set observed in fal's public catalogue on
 * 2026-08-05. Order is by what this product is for — video first, then image,
 * then audio, then 3D, then the analysis and infrastructure categories that a
 * generator user is looking for last. A category missing from this list is
 * appended, never hidden; see [`orderedCategories`].
 */
export const CATEGORY_ORDER: readonly string[] = [
  "video-to-video",
  "image-to-video",
  "text-to-video",
  "audio-to-video",
  "image-to-image",
  "text-to-image",
  "text-to-audio",
  "audio-to-audio",
  "video-to-audio",
  "text-to-speech",
  "speech-to-speech",
  "speech-to-text",
  "audio-to-text",
  "image-to-3d",
  "text-to-3d",
  "3d-to-3d",
  "vision",
  "video-to-text",
  "image-to-text",
  "llm",
  "text-to-json",
  "image-to-json",
  "json",
  "training",
  "workflow",
  "unknown",
];

/** Tokens whose casing is not sentence case. */
const TOKEN_CASING: Record<string, string> = {
  "3d": "3D",
  llm: "LLM",
  json: "JSON",
  to: "to",
};

const norm = (s: string) => s.trim().toLowerCase();

/**
 * "video-to-video" → "Video to Video", "image-to-3d" → "Image to 3D".
 *
 * A blank category is labelled rather than skipped: a section with no heading
 * looks like the rows above it, which silently merges two capability groups.
 */
export function categoryLabel(category: string): string {
  const slug = norm(category);
  if (slug === "") return "Uncategorized";
  return slug
    .split("-")
    .filter(Boolean)
    .map((t) => TOKEN_CASING[t] ?? t.charAt(0).toUpperCase() + t.slice(1))
    .join(" ");
}

/**
 * The nouns either side of a `a-to-b` category.
 *
 * Only these seven words appear in fal's category slugs, so anything else —
 * `training`, `vision`, `llm`, `workflow`, `unknown` — has no input/output
 * pair to state and returns null rather than a plausible-sounding guess.
 */
const INPUT_NOUN: Record<string, string> = {
  text: "prompt",
  image: "image",
  video: "video",
  audio: "audio",
  speech: "speech",
  "3d": "3D model",
  json: "JSON",
};

const OUTPUT_NOUN: Record<string, string> = {
  text: "text",
  image: "image",
  video: "video",
  audio: "audio",
  speech: "speech",
  "3d": "3D model",
  json: "JSON",
};

/** What the category itself claims, or nulls when it is not directional. */
export function categoryIo(category: string): {
  takes: string | null;
  produces: string | null;
} {
  const parts = norm(category).split("-to-");
  if (parts.length !== 2) return { takes: null, produces: null };
  const takes = INPUT_NOUN[parts[0]] ?? null;
  const produces = OUTPUT_NOUN[parts[1]] ?? null;
  return { takes, produces };
}

/** Copy for the one thing a category chip cannot say. */
export const NOT_STATED = "not stated";

/**
 * The row's "takes → produces" line.
 *
 * Built from the endpoint's declared inputs first and the category second,
 * because the category is a coarse label the provider applies for browsing
 * while the input schema is what the endpoint will actually accept. Falling
 * back to the category only when there is nothing declared keeps the line
 * honest for the models whose schema we have not read yet.
 */
export function ioSummary(model: BrowseModel): {
  takes: string;
  produces: string;
} {
  const io = categoryIo(model.category);
  const parts: string[] = [];
  if (model.takesPrompt) parts.push("prompt");
  for (const role of dedupe(model.acceptedRoles)) parts.push(roleLabel(role));

  const takes =
    parts.length > 0 ? parts.join(" + ") : (io.takes ?? NOT_STATED);
  return { takes, produces: io.produces ?? NOT_STATED };
}

/** "Requires a video and a start frame", or null when nothing is mandatory. */
export function requirementNote(model: BrowseModel): string | null {
  const required = dedupe(model.requiredRoles);
  if (required.length === 0) return null;
  return `Requires ${roleList(required)}`;
}

/* ── Search ─────────────────────────────────────────────────────────────── */

/**
 * All tokens must match, in title or description, in any order.
 *
 * Deliberately *not* the category: category is the browser's primary axis and
 * already has its own chip row, so folding it into search makes every one of
 * the 385 image-to-image models match the word "image" and buries the model
 * actually called that.
 */
export function matchesQuery(model: BrowseModel, query: string): boolean {
  const tokens = norm(query).split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return true;
  const hay = norm(`${model.title} ${model.description}`);
  return tokens.every((t) => hay.includes(t));
}

/* ── Runnability ────────────────────────────────────────────────────────── */

/**
 * Why this row is greyed out, or null when it can run.
 *
 * Credential/route failure is reported ahead of an attachment mismatch on
 * purpose: removing the attachment would not make an unroutable model run, so
 * leading with "does not accept the attached video" would send the user to fix
 * the wrong thing.
 */
export function blockReason(
  model: BrowseModel,
  attached: MediaRole[] = [],
): string | null {
  if (!model.runnable) {
    const stated = model.unavailableReason?.trim();
    // A greyed row with no reason is the opacity this surface exists to
    // remove, so an absent or blank reason still says something.
    return stated && stated.length > 0
      ? stated
      : "No usable route for this model";
  }
  const gap = attachmentGap(model, attached);
  if (gap.length > 0) return `Does not accept the attached ${roleList(gap)}`;
  return null;
}

export function isBlocked(
  model: BrowseModel,
  attached: MediaRole[] = [],
): boolean {
  return blockReason(model, attached) !== null;
}

/* ── Price ──────────────────────────────────────────────────────────────── */

/**
 * The price string on a row.
 *
 * Zero is treated as unpublished, not as free. No model in fal's catalogue
 * costs nothing, so a zero here means a parse failed upstream, and "Free" or
 * "$0.00" on a row that then bills the user destroys the credibility of every
 * other number in the app. Formatting itself is delegated to `lib/cost` so
 * there is exactly one money formatter in the UI.
 */
export function formatBrowsePrice(price: BrowsePrice | null | undefined): string {
  const usd = price?.usd;
  if (usd === null || usd === undefined) return UNKNOWN_COST_LABEL;
  if (!Number.isFinite(usd) || usd <= 0) return UNKNOWN_COST_LABEL;
  return formatUsd(usd);
}

/** True when there is a number the user can rely on. */
export function hasBrowsePrice(price: BrowsePrice | null | undefined): boolean {
  return formatBrowsePrice(price) !== UNKNOWN_COST_LABEL;
}

/** The small line under the number. Suppressed when there is no number. */
export function priceUnit(price: BrowsePrice | null | undefined): string | null {
  if (!hasBrowsePrice(price)) return null;
  const unit = price?.unit?.trim();
  return unit && unit.length > 0 ? unit : null;
}

/* ── Filtering, grouping, paging ────────────────────────────────────────── */

export interface BrowseFilter {
  query?: string;
  /** null or undefined means every category. */
  category?: string | null;
  /** Roles the user has already attached, from `MediaRef[]`. */
  attachedRoles?: MediaRole[];
  /** Keep only models that accept everything attached. */
  onlyCompatible?: boolean;
}

/**
 * Filter without reordering. Catalogue order is the provider's relevance
 * order and re-sorting it here would quietly override it.
 */
export function filterBrowseModels(
  models: BrowseModel[],
  {
    query = "",
    category = null,
    attachedRoles = [],
    onlyCompatible = false,
  }: BrowseFilter = {},
): BrowseModel[] {
  const wantCategory = category === null ? null : norm(category);
  // With nothing attached the compatibility filter has nothing to test, so it
  // is a no-op rather than a filter that empties the list — a toggle that
  // blanks the catalogue reads as a broken app, not as an unmatched query.
  const compat = onlyCompatible && attachedRoles.length > 0;
  return models.filter((model) => {
    if (wantCategory !== null && norm(model.category) !== wantCategory) {
      return false;
    }
    if (compat && attachmentGap(model, attachedRoles).length > 0) return false;
    return matchesQuery(model, query);
  });
}

/**
 * Category slugs present in a set, known ones in [`CATEGORY_ORDER`] and the
 * rest appended alphabetically. Never dropped: an unrecognised category that
 * vanished would hide its models with no chip to reveal them.
 */
export function orderedCategories(models: BrowseModel[]): string[] {
  const present = new Set(models.map((m) => norm(m.category)));
  const known = CATEGORY_ORDER.filter((c) => present.has(c));
  const unknown = [...present]
    .filter((c) => !CATEGORY_ORDER.includes(c))
    .sort((a, b) => a.localeCompare(b));
  return [...known, ...unknown];
}

export interface CategoryChip {
  category: string;
  label: string;
  count: number;
}

/** The chip row, with counts so a chip never leads to an empty section. */
export function categoryChips(models: BrowseModel[]): CategoryChip[] {
  return orderedCategories(models).map((category) => ({
    category,
    label: categoryLabel(category),
    count: models.filter((m) => norm(m.category) === category).length,
  }));
}

export interface BrowseGroup {
  category: string;
  label: string;
  models: BrowseModel[];
}

/** Grouped sections in chip order, preserving input order inside each group. */
export function groupByCategory(models: BrowseModel[]): BrowseGroup[] {
  return orderedCategories(models).map((category) => ({
    category,
    label: categoryLabel(category),
    models: models.filter((m) => norm(m.category) === category),
  }));
}

/**
 * Flatten into the order the sections render in.
 *
 * Paging runs over this rather than over catalogue order so "Load more" is
 * append-only. Paging the raw order instead drops new rows into sections the
 * user has already scrolled past, and the whole list appears to shuffle under
 * them every time they ask for more.
 */
export function inCategoryOrder(models: BrowseModel[]): BrowseModel[] {
  return groupByCategory(models).flatMap((group) => group.models);
}

/**
 * Paging for "Load more", applied to the flat list before grouping so a first
 * paint of the unfiltered catalogue is a few dozen rows rather than 1418.
 */
export function pageBrowseModels(
  models: BrowseModel[],
  shown: number,
): { page: BrowseModel[]; remaining: number } {
  const clamped = Math.max(0, Math.min(shown, models.length));
  return { page: models.slice(0, clamped), remaining: models.length - clamped };
}

/* ── Mock ───────────────────────────────────────────────────────────────── */

/**
 * Enough of a catalogue to render the browser without the shell.
 *
 * Every field here is real: ids, titles, categories, descriptions and prices
 * were read from fal's public catalogue on 2026-08-05, and the role lists were
 * read from each endpoint's own published input schema
 * (`fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`) on the same day.
 * Nothing is invented, so what the browser shows in `pnpm dev` is what it will
 * show in the shell.
 *
 * **Never use this as a fallback.** `api.ts` used to substitute mock data when
 * a bridge call rejected, so a real provider failure rendered as a plausible
 * success. The browser takes its models as a required prop for that reason.
 */
export const MOCK_BROWSE_MODELS: BrowseModel[] = [
  {
    id: "veed/video-background-removal/fast",
    title: "Video Background Removal",
    category: "video-to-video",
    description:
      "Remove background from any video with people and objects. No green screen needed.",
    price: {
      usd: 0.012,
      unit: "per 30 frames",
      note: "Your request will cost $0.012 per 30 frames (Refine Foreground Edges: ON) / $0.008 (Refine: OFF).",
    },
    takesPrompt: false,
    acceptedRoles: ["video"],
    requiredRoles: ["video"],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
  {
    id: "fal-ai/wan/v2.2-14b/animate/move",
    title: "Wan-2.2 Animate Move",
    category: "video-to-video",
    description:
      "Generates high-fidelity character videos by replicating the expressions and movements of a driving video.",
    price: {
      usd: 0.08,
      unit: "per second of 720p video",
      note: "Billed by frame count at 16 frames per video second: 720p $0.08, 580p $0.06, 480p $0.04 per video second.",
    },
    takesPrompt: false,
    acceptedRoles: ["video", "start"],
    requiredRoles: ["video", "start"],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
  {
    id: "fal-ai/bytedance/dreamactor/v2",
    title: "Bytedance Dreamactor V2",
    category: "video-to-video",
    description:
      "Transfer motion from a video to characters in an image. Handles non-human and multiple subjects.",
    // One of the 764 catalogue entries that publish no price at all.
    price: { usd: null, unit: null, note: null },
    takesPrompt: false,
    acceptedRoles: ["video", "start"],
    requiredRoles: ["video", "start"],
    provider: "fal",
    // Exercises the greyed-with-a-reason path in `pnpm dev`.
    runnable: false,
    unavailableReason: "No fal credentials — add a key in Settings",
  },
  {
    id: "fal-ai/kling-video/v3/pro/image-to-video",
    title: "Kling Video v3 Image to Video [Pro]",
    category: "image-to-video",
    description:
      "Top-tier image-to-video with cinematic visuals, fluid motion and native audio generation.",
    price: {
      usd: 0.112,
      unit: "per second of video, audio off",
      note: "$0.112 per second with audio off, $0.168 with audio on, $0.196 when voice control is used.",
    },
    takesPrompt: true,
    acceptedRoles: ["start", "end"],
    requiredRoles: ["start"],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
  {
    id: "bytedance/seedance-2.0/text-to-video",
    title: "Seedance 2.0 Text to Video",
    category: "text-to-video",
    description:
      "Cinematic output with native audio, multi-shot editing and real-world physics.",
    price: {
      usd: 0.3034,
      unit: "per second of 720p video",
      note: "$0.3034 per second at 720p, $0.682 per second at 1080p.",
    },
    takesPrompt: true,
    // Declares prompt, duration, resolution, generate_audio, aspect_ratio and
    // bitrate_mode — and no media input of any kind. This is the row that
    // reproduces the bug: attaching a video here has no effect whatsoever.
    acceptedRoles: [],
    requiredRoles: [],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
  {
    id: "fal-ai/minimax/hailuo-2.3/standard/text-to-video",
    title: "MiniMax Hailuo 2.3 [Standard]",
    category: "text-to-video",
    description:
      "Advanced text-to-video generation model at 768p resolution.",
    price: {
      usd: 0.28,
      unit: "per 6 second video",
      note: "$0.28 per 6 second video, $0.56 per 10 second video.",
    },
    takesPrompt: true,
    acceptedRoles: [],
    requiredRoles: [],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
  {
    id: "fal-ai/nano-banana-2/edit",
    title: "Nano Banana 2",
    category: "image-to-image",
    // Mock copy is written by us, never lifted from a provider's marketing
    // text — scripts/lint-provenance.py checks shipped strings against an
    // 80,015-shingle index and caught the pasted version of this line.
    description: "Generates and edits stills from a prompt.",
    price: {
      usd: 0.08,
      unit: "per image",
      note: "$0.08 per image. 2K is charged at 1.5x and 4K at 2x the standard rate.",
    },
    takesPrompt: true,
    // Categorised image-to-image and still declares video_url and audio_url:
    // the reason roles come from the endpoint schema and not the category.
    acceptedRoles: ["reference", "video", "audio"],
    requiredRoles: [],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
  {
    id: "fal-ai/index-tts-2/text-to-speech",
    title: "Index TTS 2.0",
    category: "text-to-speech",
    description: "Generate natural, clear speech with a reference voice.",
    price: {
      usd: 0.002,
      unit: "per second of audio",
      note: "$0.002 per generated audio second.",
    },
    takesPrompt: true,
    acceptedRoles: ["audio_reference"],
    requiredRoles: ["audio_reference"],
    provider: "fal",
    runnable: true,
    unavailableReason: null,
  },
];
