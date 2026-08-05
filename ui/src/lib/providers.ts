/**
 * What the UI knows about providers, and the rules that decide whether the app
 * can actually generate anything.
 *
 * The catalogue here is a *fallback*. `provider_info()` on the Rust side is the
 * source of truth once it answers; this table exists so the same screens render
 * in a plain browser, and so a Rust build that predates the command still shows
 * something useful instead of an empty list.
 */

export interface ProviderInfo {
  slug: string;
  displayName: string;
  needsKey: boolean;
  needsSecret: boolean;
  /** Where the user goes to create a key. Empty for keyless providers. */
  keyUrl: string;
  /** Conventional environment variable names, used by the .env importer. */
  envNames: string[];
  /** One line on what this key unlocks. */
  blurb: string;
}

/**
 * fal is the recommended starting point and the rest are genuinely optional:
 * most of the model roster has a fal route, so one key turns the catalogue on.
 * The onboarding screen keys off this constant rather than hardcoding a slug in
 * markup.
 */
export const PRIMARY_PROVIDER = "fal";

/** The keyless, free tier. Presented apart from the paid providers. */
export const LOCAL_PROVIDER = "local";

export const PROVIDER_CATALOG: ProviderInfo[] = [
  {
    slug: "fal",
    displayName: "fal.ai",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://fal.ai/dashboard/keys",
    envNames: ["FAL_KEY", "FAL_API_KEY"],
    blurb:
      "The backbone. Most of the roster routes through fal — Kling, Seedance, Wan, FLUX, Seedream and more.",
  },
  {
    slug: "vaig",
    displayName: "Vercel AI Gateway",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://vercel.com/dashboard/ai-gateway",
    envNames: ["AI_GATEWAY_API_KEY", "VERCEL_AI_GATEWAY_API_KEY"],
    blurb: "One key that fans out to several hosted models, billed by Vercel.",
  },
  {
    slug: "google",
    displayName: "Google",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://aistudio.google.com/apikey",
    envNames: [
      "GEMINI_API_KEY",
      "GOOGLE_API_KEY",
      "GOOGLE_GENERATIVE_AI_API_KEY",
    ],
    blurb: "Veo video and the Gemini image models, direct rather than resold.",
  },
  {
    slug: "openai",
    displayName: "OpenAI",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://platform.openai.com/api-keys",
    envNames: ["OPENAI_API_KEY"],
    blurb: "GPT image generation and the prompt enhancer.",
  },
  {
    slug: "xai",
    displayName: "xAI",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://console.x.ai",
    envNames: ["XAI_API_KEY", "GROK_API_KEY"],
    blurb: "Grok image generation.",
  },
  {
    slug: "bfl",
    displayName: "Black Forest Labs",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://dashboard.bfl.ai",
    envNames: ["BFL_API_KEY", "BFL_KEY"],
    blurb: "FLUX straight from the lab that trains it.",
  },
  {
    slug: "recraft",
    displayName: "Recraft",
    needsKey: true,
    needsSecret: false,
    keyUrl: "https://www.recraft.ai/profile/api",
    envNames: ["RECRAFT_API_KEY", "RECRAFT_TOKEN"],
    blurb: "Vector-aware brand and layout stills.",
  },
  {
    slug: "higgsfield",
    displayName: "Higgsfield",
    needsKey: true,
    needsSecret: true,
    keyUrl: "https://cloud.higgsfield.ai/settings/api-keys",
    envNames: ["HIGGSFIELD_API_KEY", "HIGGSFIELD_KEY"],
    blurb:
      "The only route to the literal Soul and DoP presets. Issues a key and a separate secret.",
  },
  {
    slug: LOCAL_PROVIDER,
    displayName: "Local",
    needsKey: false,
    needsSecret: false,
    keyUrl: "",
    envNames: [],
    blurb:
      "ComfyUI or Ollama running on this machine. No key, no cost, nothing leaves the computer.",
  },
];

/** Env var names for the secret half, for the providers that have one. */
export const SECRET_ENV_NAMES: Record<string, string[]> = {
  higgsfield: ["HIGGSFIELD_SECRET", "HIGGSFIELD_API_SECRET"],
};

export interface LocalEndpoints {
  comfyui: boolean;
  ollama: boolean;
}

/**
 * Is the app usable at all?
 *
 * The subtlety: `configured_providers()` always contains `local`, because the
 * vault reports a provider as configured when it needs no credential — and
 * Local needs none. Taken at face value that makes a completely unconfigured
 * install look ready. Local only counts once something is actually listening.
 */
export function isAppConfigured(
  configured: string[],
  local?: LocalEndpoints | null,
): boolean {
  return configured.some((slug) =>
    slug === LOCAL_PROVIDER ? Boolean(local?.comfyui || local?.ollama) : true,
  );
}

/** Human summary of local detection, for the free-tier row. */
export function localStatusLabel(local: LocalEndpoints | null): string {
  if (!local) return "Not checked yet";
  const up = [
    local.comfyui ? "ComfyUI" : null,
    local.ollama ? "Ollama" : null,
  ].filter(Boolean);
  if (up.length === 0) return "Nothing detected on this machine";
  return `${up.join(" and ")} detected`;
}

/**
 * What we are allowed to say about a stored credential.
 *
 * The bridge only ever returns booleans, so this is the complete vocabulary for
 * the "is it set" display. There is deliberately no branch that could render a
 * value, masked or otherwise.
 */
export function keyStatusLabel(state: {
  hasKey: boolean;
  hasSecret: boolean;
  needsKey: boolean;
  needsSecret: boolean;
}): string {
  if (!state.needsKey && !state.needsSecret) return "No key needed";
  if (state.needsSecret) {
    if (state.hasKey && state.hasSecret) return "Key and secret stored";
    if (state.hasKey) return "Secret missing";
    if (state.hasSecret) return "Key missing";
    return "Not set";
  }
  return state.hasKey ? "Key stored" : "Not set";
}

/** True when everything this provider needs is present. */
export function keyStateComplete(state: {
  hasKey: boolean;
  hasSecret: boolean;
  needsKey: boolean;
  needsSecret: boolean;
}): boolean {
  return (
    (!state.needsKey || state.hasKey) && (!state.needsSecret || state.hasSecret)
  );
}

// ── Validation ─────────────────────────────────────────────────────────────

export type ValidationState =
  | { kind: "idle" }
  | { kind: "testing" }
  | { kind: "ok"; detail: string }
  | { kind: "bad"; detail: string }
  /** `validate_key` is not reachable — no Tauri shell, or an older Rust build. */
  | { kind: "unavailable" };

export type ValidationTone = "pending" | "ok" | "bad" | "neutral";

export interface ValidationView {
  label: string;
  tone: ValidationTone;
}

/**
 * Provider error bodies are occasionally a whole HTML page. Clamping here keeps
 * one bad response from pushing every other row off the screen.
 */
const MAX_DETAIL = 120;

function clamp(detail: string): string {
  const trimmed = detail.trim().replace(/\s+/g, " ");
  if (trimmed.length <= MAX_DETAIL) return trimmed;
  return `${trimmed.slice(0, MAX_DETAIL - 1)}…`;
}

/**
 * The one place validation turns into something on screen. Returning null for
 * idle means a row that has never been tested shows nothing at all, rather than
 * an "unknown" chip that reads like a problem.
 */
export function validationView(state: ValidationState): ValidationView | null {
  switch (state.kind) {
    case "idle":
      return null;
    case "testing":
      return { label: "Testing…", tone: "pending" };
    case "ok":
      return { label: clamp(state.detail) || "Key works", tone: "ok" };
    case "bad":
      return { label: clamp(state.detail) || "Key rejected", tone: "bad" };
    case "unavailable":
      return {
        label: "Testing needs the desktop app",
        tone: "neutral",
      };
  }
}
