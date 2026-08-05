/**
 * The bridge to the Rust core.
 *
 * Every call is lazily imported and falls back to a sane default when the Tauri
 * runtime is absent, so the whole UI runs in a plain browser via `pnpm dev`
 * without the shell. For a UI this size that is a large iteration-speed win.
 */

import type {
  CostEstimate,
  ModelCapabilities,
  GenSettings,
  Gap,
  JobResult,
  JobSet,
  MediaRef,
  Model,
  PresetFamily,
  Route,
  SubmitInput,
  UseCase,
} from "./types";
import {
  MOCK_MODELS,
  MOCK_PRESETS,
  mockCancel,
  mockEstimate,
  mockListJobs,
  mockSubmit,
  mockSubscribe,
} from "./mock";
import {
  PROVIDER_CATALOG,
  type LocalEndpoints,
  type ProviderInfo,
} from "./lib/providers";
import { planEnvImport } from "./lib/env-import";

async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export interface KeyState {
  provider: string;
  hasKey: boolean;
  hasSecret: boolean;
  needsKey: boolean;
  needsSecret: boolean;
}

/** Wire shape from Rust — serde emits snake_case. */
interface RawKeyState {
  provider: string;
  has_key: boolean;
  has_secret: boolean;
  needs_key: boolean;
  needs_secret: boolean;
}

const toKeyState = (r: RawKeyState): KeyState => ({
  provider: r.provider,
  hasKey: r.has_key,
  hasSecret: r.has_secret,
  needsKey: r.needs_key,
  needsSecret: r.needs_secret,
});

/**
 * Credential presence per provider. Note this only ever returns booleans —
 * the secret itself is never exposed to the webview by design.
 */
export async function keyStates(): Promise<KeyState[]> {
  try {
    const raw = await invoke<RawKeyState[]>("key_states");
    return raw.map(toKeyState);
  } catch {
    return [];
  }
}

/** Store or clear a credential. An empty `value` clears it. */
export async function setKey(
  provider: string,
  value: string,
  secretHalf = false,
): Promise<void> {
  await invoke("set_key", { provider, secretHalf, value });
}

export async function configuredProviders(): Promise<string[]> {
  try {
    return await invoke<string[]>("configured_providers");
  } catch {
    return [];
  }
}

export const isDesktop = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * Normalise whatever `invoke` rejected with into an `Error`.
 *
 * Tauri rejects with the command's `Err` payload, which for our commands is a
 * bare `String`. Re-throwing that raw gives React an error boundary with no
 * `.message`, so the useful half of a provider rejection — the part naming the
 * missing key or the rejected parameter — disappears on the way to the screen.
 */
export function asError(e: unknown): Error {
  if (e instanceof Error) return e;
  if (typeof e === "string") return new Error(e);
  return new Error(
    typeof e === "object" && e !== null ? JSON.stringify(e) : String(e),
  );
}

// ── Provider setup ─────────────────────────────────────────────────────────

interface RawProviderInfo {
  slug: string;
  display_name?: string;
  displayName?: string;
  needs_key?: boolean;
  needsKey?: boolean;
  needs_secret?: boolean;
  needsSecret?: boolean;
  key_url?: string;
  keyUrl?: string;
  env_names?: string[];
  envNames?: string[];
  blurb?: string;
}

/**
 * Descriptions of every provider: where to get a key, what it unlocks.
 *
 * Falls back to the static catalogue, which is what makes the setup screens
 * render in a browser at all. Fields the Rust side leaves blank fall back
 * individually, so a partial payload degrades field by field rather than
 * throwing the whole entry away.
 */
export async function providerInfo(): Promise<ProviderInfo[]> {
  try {
    const raw = await invoke<RawProviderInfo[]>("provider_info");
    if (!Array.isArray(raw) || raw.length === 0) return PROVIDER_CATALOG;
    return raw.map((r) => {
      const known = PROVIDER_CATALOG.find((p) => p.slug === r.slug);
      return {
        slug: r.slug,
        displayName:
          r.display_name ?? r.displayName ?? known?.displayName ?? r.slug,
        needsKey: r.needs_key ?? r.needsKey ?? known?.needsKey ?? true,
        needsSecret: r.needs_secret ?? r.needsSecret ?? known?.needsSecret ?? false,
        keyUrl: r.key_url ?? r.keyUrl ?? known?.keyUrl ?? "",
        envNames: r.env_names ?? r.envNames ?? known?.envNames ?? [],
        blurb: r.blurb ?? known?.blurb ?? "",
      };
    });
  } catch {
    return PROVIDER_CATALOG;
  }
}

export interface ValidationResult {
  ok: boolean;
  detail: string;
}

/**
 * Ask the core to make a real authenticated call with the stored key.
 *
 * `null` means the check could not run at all — no shell, or a build that
 * predates the command. That is a third state, not a failure: rendering an
 * untestable key as "invalid" would send people re-pasting a key that works.
 */
export async function validateKey(
  provider: string,
): Promise<ValidationResult | null> {
  try {
    const raw = await invoke<{ ok: boolean; detail?: string }>("validate_key", {
      provider,
    });
    return { ok: Boolean(raw?.ok), detail: raw?.detail ?? "" };
  } catch {
    return null;
  }
}

export interface EnvImportResult {
  imported: string[];
  unknown: string[];
}

/**
 * Parse .env-style text and store every credential it recognises.
 *
 * The fallback path is not cosmetic: `set_key` already works, so on a desktop
 * build without `import_env` the TypeScript parser plus per-key writes gives
 * the real feature. Outside the shell nothing can be stored, so we report what
 * *would* be imported — the entire browser path is a simulation anyway.
 */
export async function importEnv(text: string): Promise<EnvImportResult> {
  try {
    const raw = await invoke<{ imported?: string[]; unknown?: string[] }>(
      "import_env",
      { text },
    );
    return { imported: raw?.imported ?? [], unknown: raw?.unknown ?? [] };
  } catch {
    const plan = planEnvImport(text);
    if (!isDesktop()) {
      return { imported: plan.providers, unknown: plan.unknown };
    }
    const stored = new Set<string>();
    const failed: string[] = [];
    for (const a of plan.assignments) {
      try {
        await setKey(a.provider, a.value, a.secretHalf);
        stored.add(a.provider);
      } catch {
        failed.push(`${a.provider}${a.secretHalf ? " (secret)" : ""}`);
      }
    }
    return {
      imported: [...stored],
      unknown: [...plan.unknown, ...failed.map((f) => `${f}: could not store`)],
    };
  }
}

/** Which local inference endpoints are up right now. */
export async function localEndpoints(): Promise<LocalEndpoints> {
  try {
    return await invoke<LocalEndpoints>("local_endpoints");
  } catch {
    return { comfyui: false, ollama: false };
  }
}

/** Absolute path of the generated-asset library. Null outside the shell. */
export async function libraryRoot(): Promise<string | null> {
  try {
    return await invoke<string>("library_root");
  } catch {
    return null;
  }
}

/**
 * Open a URL in the user's real browser.
 *
 * A bare `<a href>` inside the shell navigates the webview itself, which
 * replaces the app with a login page and offers no way back. The opener plugin
 * hands the URL to the OS instead.
 */
export async function openExternal(url: string): Promise<void> {
  try {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

// ── Wire shapes ────────────────────────────────────────────────────────────
// Hand-written for now. These get replaced by tauri-specta output once the
// commands land; until then the converters are the single place snake_case
// stops, so no component ever has to know the wire spelling.

interface RawRoute {
  id: string;
  provider: string;
  slug: string;
  note?: string | null;
  available?: boolean;
  unavailable_reason?: string | null;
  unavailableReason?: string | null;
}

/**
 * Rust spells these snake_case. Mapped explicitly rather than passed straight
 * through, because a silent spelling mismatch here reads as "the model has no
 * capabilities" — which is precisely the state that used to make the UI invent
 * its own.
 */
interface RawCapabilities {
  supports_duration?: boolean;
  durations?: number[];
  default_duration?: number | null;
  supports_resolution?: boolean;
  resolutions?: string[];
  default_resolution?: string | null;
  supports_aspect?: boolean;
  aspects?: string[];
  default_aspect?: string | null;
  audio?: boolean;
  constraints?: string[];
}

const toCapabilities = (
  c: RawCapabilities | undefined,
): ModelCapabilities | undefined =>
  c
    ? {
        supportsDuration: c.supports_duration ?? false,
        durations: c.durations ?? [],
        defaultDuration: c.default_duration ?? null,
        supportsResolution: c.supports_resolution ?? false,
        resolutions: c.resolutions ?? [],
        defaultResolution: c.default_resolution ?? null,
        supportsAspect: c.supports_aspect ?? false,
        aspects: c.aspects ?? [],
        defaultAspect: c.default_aspect ?? null,
        audio: c.audio ?? false,
        constraints: c.constraints ?? [],
      }
    : undefined;

interface RawModel {
  id: string;
  display_name?: string;
  displayName?: string;
  modality: Model["modality"];
  routes: RawRoute[];
  is_launch?: boolean;
  isLaunch?: boolean;
  subtitle?: string;
  capabilities?: RawCapabilities;
}

interface RawPreset {
  id: string;
  display_name?: string;
  displayName?: string;
  category: string;
  tags?: string[];
  description?: string;
  variants?: { model_id?: string; modelId?: string; preview_url?: string }[];
}

interface RawEstimate {
  usd: number;
  basis?: string;
  minimum_applied?: boolean;
  minimumApplied?: boolean;
}

interface RawJobSet {
  id: string;
  model_id?: string;
  modelId?: string;
  status: string;
  prompt: string;
  enhanced_prompt?: string | null;
  created_at?: string;
  createdAt?: string;
  results?: (JobResult & { poster_url?: string; local_path?: string | null })[];
  /**
   * Rust serialises `route_id` (a bare string). `route` is accepted too so a
   * future richer payload works without a flag day — but reading only `route`
   * left `job.route` undefined on every persisted job, which silently broke
   * Rerun's route restore.
   */
  route?: RawRoute | string;
  route_id?: string;
  routeId?: string;
  estimated_usd?: number | null;
  actual_usd?: number | null;
  preset_id?: string | null;
  preset_name?: string | null;
  settings?: GenSettings;
  media?: MediaRef[];
  enhancer_version?: string | null;
  enhance_note?: string | null;
  advisories?: string[];
  fail_reason?: string | null;
}

const toModel = (r: RawModel): Model => ({
  id: r.id,
  displayName: r.display_name ?? r.displayName ?? r.id,
  modality: r.modality,
  routes: (r.routes ?? []).map(toRoute),
  isLaunch: r.is_launch ?? r.isLaunch ?? false,
  subtitle: r.subtitle,
  capabilities: toCapabilities(r.capabilities),
});

const toPreset = (r: RawPreset): PresetFamily => ({
  id: r.id,
  displayName: r.display_name ?? r.displayName ?? r.id,
  category: r.category,
  tags: r.tags ?? [],
  description: r.description ?? "",
  variants: r.variants?.map((v) => ({
    modelId: v.model_id ?? v.modelId ?? "",
    previewUrl: v.preview_url,
  })),
});

/**
 * `route` may arrive as a bare id while the resolver is still being wired on
 * the Rust side. A rail that shows "fal:kling-video/v3" is worse than one that
 * shows the parsed pair, but far better than one that renders "[object
 * Object]" or crashes.
 */
function toRoute(raw: RawRoute | string): Route {
  if (typeof raw !== "string") {
    return {
      id: raw.id,
      provider: raw.provider,
      slug: raw.slug,
      note: raw.note ?? undefined,
      // Absent means "the shell did not say", which for a route we are being
      // handed is far likelier to be usable than not. A default of false would
      // grey out every route on an older build.
      available: raw.available ?? true,
      unavailableReason:
        raw.unavailable_reason ?? raw.unavailableReason ?? undefined,
    };
  }
  const [provider, ...rest] = raw.split(":");
  return { id: raw, provider, slug: rest.join(":"), available: true };
}

const toJobSet = (r: RawJobSet): JobSet => ({
  id: r.id,
  modelId: r.model_id ?? r.modelId ?? "",
  status: r.status,
  prompt: r.prompt,
  enhancedPrompt: r.enhanced_prompt ?? null,
  createdAt: r.created_at ?? r.createdAt ?? new Date().toISOString(),
  results: (r.results ?? []).map((res) => ({
    url: res.url,
    kind: res.kind,
    width: res.width,
    height: res.height,
    poster: res.poster ?? res.poster_url,
    localPath: res.local_path ?? res.localPath ?? null,
  })),
  route: toRoute(r.route ?? r.route_id ?? r.routeId ?? ""),
  estimatedUsd: r.estimated_usd ?? null,
  actualUsd: r.actual_usd ?? null,
  presetId: r.preset_id ?? null,
  presetName: r.preset_name ?? null,
  settings: r.settings,
  media: r.media ?? [],
  enhancerVersion: r.enhancer_version ?? null,
  enhanceNote: r.enhance_note ?? null,
  advisories: r.advisories ?? [],
  failReason: r.fail_reason ?? null,
});

// ── Generator bridge ───────────────────────────────────────────────────────

export async function listModels(): Promise<Model[]> {
  try {
    const raw = await invoke<RawModel[]>("list_models");
    return raw.map(toModel);
  } catch {
    return MOCK_MODELS;
  }
}

export async function listPresets(): Promise<PresetFamily[]> {
  try {
    const raw = await invoke<RawPreset[]>("list_presets");
    return raw.map(toPreset);
  } catch {
    return MOCK_PRESETS;
  }
}

/**
 * A null return is a real answer, not a failure: the provider publishes no
 * price for this call. Callers must render that as "price unavailable" and
 * never as free — see lib/cost.ts.
 */
export async function estimateCost(args: {
  modelId: string;
  routeId: string;
  settings: GenSettings;
}): Promise<CostEstimate | null> {
  try {
    const raw = await invoke<RawEstimate | null>("estimate_cost", args);
    if (!raw) return null;
    return {
      usd: raw.usd,
      basis: raw.basis ?? "",
      minimumApplied: raw.minimum_applied ?? raw.minimumApplied,
    };
  } catch (e) {
    // Only the absence of the shell justifies a fabricated number. A command
    // that ran and failed must surface, or the Generate button quotes a price
    // no provider agreed to.
    if (!isDesktop()) return mockEstimate(args.modelId, args.settings);
    throw asError(e);
  }
}

/**
 * Authoritative capabilities for one model, asked at selection time.
 *
 * Deliberately not part of `listModels`: answering it properly means asking fal
 * for the endpoint schema, and doing that for all 68 models to draw a picker
 * would be 68 blocking requests. Returns `null` when we could not find out —
 * the caller must keep the catalogue's answer rather than blanking the chips.
 */
export async function modelCapabilities(
  modelId: string,
  routeId: string | null,
  hasMedia: boolean,
): Promise<ModelCapabilities | null> {
  try {
    const raw = await invoke<RawCapabilities>("model_capabilities", {
      modelId,
      routeId,
      hasMedia,
    });
    return toCapabilities(raw) ?? null;
  } catch {
    return null;
  }
}

/** The workspace tabs, and the media slots each one needs. */
export async function listUseCases(): Promise<UseCase[]> {
  try {
    const raw = await invoke<
      {
        slug: string;
        label: string;
        blurb: string;
        slots: [string, boolean][];
        requires_media?: boolean;
        requiresMedia?: boolean;
      }[]
    >("list_use_cases");
    return raw.map((u) => ({
      slug: u.slug,
      label: u.label,
      blurb: u.blurb,
      slots: u.slots ?? [],
      requiresMedia: u.requires_media ?? u.requiresMedia ?? false,
    }));
  } catch {
    return [];
  }
}

/**
 * Models that can do one job.
 *
 * Filtered in Rust, not here: the picker must not be able to offer something
 * the submit path would refuse.
 */
export async function modelsForUseCase(useCase: string): Promise<Model[]> {
  try {
    const raw = await invoke<RawModel[]>("models_for_use_case", { useCase });
    return raw.map(toModel);
  } catch {
    return MOCK_MODELS;
  }
}

/**
 * What this prompt leaves unsaid, for this job and these attachments.
 *
 * Deterministic and instant in Rust — a question that appears on one run and
 * not the next would be worse than none.
 */
export async function detectGaps(args: {
  prompt: string;
  useCase: string;
  mediaRoles: string[];
}): Promise<Gap[]> {
  // An object, not three positionals. `detectGaps(tab, prompt, roles)` type-
  // checked cleanly against `(prompt, useCase, roles)` because both are
  // strings, and would have asked video questions about an image job.
  try {
    return await invoke<Gap[]>("detect_gaps", args);
  } catch {
    // No shell, or the command is missing: generating must never be blocked by
    // an optional prompt to improve the prompt.
    return [];
  }
}

export async function submitJob(input: SubmitInput): Promise<string> {
  try {
    // Wrapped in `{ input }`, not spread. Tauri matches command parameters by
    // name, and `submit_job` takes a single `input` argument — spreading the
    // fields gave "command submit_job missing required key input".
    const res = await invoke<{ job_set_id?: string; jobSetId?: string }>(
      "submit_job",
      { input: { ...input, gap_answers: input.gapAnswers ?? [] } } as unknown as Record<string, unknown>,
    );
    return res.job_set_id ?? res.jobSetId ?? "";
  } catch (e) {
    // The bug this replaces: `invoke` rejects both when Tauri is absent and
    // when the Rust command returns Err, and a bare catch cannot tell them
    // apart — so in the shipped binary a real provider rejection produced a
    // synthetic job that marched to "completed" with an invented seed and an
    // invented price. Never fabricate a generation inside the app.
    if (!isDesktop()) return mockSubmit(input);
    throw asError(e);
  }
}

export async function listJobs(): Promise<JobSet[]> {
  try {
    const raw = await invoke<RawJobSet[]>("list_jobs");
    return raw.map(toJobSet);
  } catch (e) {
    if (!isDesktop()) return mockListJobs();
    throw asError(e);
  }
}

/**
 * Show a generated file in Finder or Explorer.
 *
 * Returns false when there is no shell to ask, so the caller can hide the
 * control rather than offering something that does nothing.
 */
export async function revealResult(path: string): Promise<boolean> {
  if (!isDesktop()) return false;
  try {
    await invoke("reveal_result", { path });
    return true;
  } catch {
    return false;
  }
}

export async function cancelJob(jobSetId: string): Promise<void> {
  try {
    await invoke("cancel_job", { jobSetId });
  } catch (e) {
    if (!isDesktop()) return mockCancel(jobSetId);
    throw asError(e);
  }
}

/**
 * Remove a generation from history, and optionally its file.
 *
 * `deleteFiles` defaults to false: a row leaving a list is a much smaller act
 * than destroying an asset the user paid a provider to produce, so the
 * irreversible half is always explicit.
 *
 * Errors propagate. The caller must not drop the row from its own state until
 * this resolves, or a failed delete looks like a successful one until the next
 * refresh puts it back.
 */
export async function deleteJob(
  jobSetId: string,
  deleteFiles = false,
): Promise<void> {
  try {
    await invoke("delete_job", { jobSetId, deleteFiles });
  } catch (e) {
    if (!isDesktop()) return;
    throw asError(e);
  }
}

/**
 * Streamed job state. Returns an unsubscribe function.
 *
 * Async setup with a sync teardown: React effects cannot await their own
 * cleanup, so the flag guards the window where the effect is torn down before
 * `listen` has resolved. Without it a fast unmount leaks a live listener.
 */
export function subscribeJobs(onUpdate: (job: JobSet) => void): () => void {
  let cancelled = false;
  let detach: (() => void) | null = null;

  void (async () => {
    try {
      const { listen } = await import("@tauri-apps/api/event");
      const un = await listen<RawJobSet>("job:update", (event) => {
        onUpdate(toJobSet(event.payload));
      });
      if (cancelled) un();
      else detach = un;
    } catch {
      const un = mockSubscribe(onUpdate);
      if (cancelled) un();
      else detach = un;
    }
  })();

  return () => {
    cancelled = true;
    detach?.();
  };
}
