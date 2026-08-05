import type {
  GenSettings,
  Model,
  ModelCapabilities,
  PresetFamily,
  PresetVariant,
  Route,
} from "../types";

/**
 * What we know when we know nothing.
 *
 * Deliberately *empty* rather than plausible. The previous version assumed
 * durations [5, 8, 10] and resolutions ["720p", "1080p"] for every model, so
 * the chip row confidently offered 10s on a 5s-only model, the estimator
 * quoted 10s, the Generate button displayed that price, and the provider
 * rejected the job after the round trip. An empty axis renders no chip, which
 * is honest; a wrong chip costs the user money.
 */
export const UNKNOWN_CAPABILITIES: ModelCapabilities = {
  supportsDuration: false,
  durations: [],
  supportsResolution: false,
  resolutions: [],
  supportsAspect: false,
  aspects: [],
  audio: false,
  constraints: [],
};

export function capabilitiesFor(model: Model | null): ModelCapabilities {
  return model?.capabilities ?? UNKNOWN_CAPABILITIES;
}

/**
 * Values a model will accept for an axis it supports but does not enumerate.
 *
 * Offered as a convenience ladder next to a free numeric input, not as the
 * truth about the model — which is why it is a separate function with a name
 * that says so, rather than a default folded into the capabilities.
 */
export function durationLadder(caps: ModelCapabilities): number[] {
  if (caps.durations.length > 0) return caps.durations;
  if (!caps.supportsDuration) return [];
  const common = [3, 4, 5, 6, 8, 10, 12];
  const d = caps.defaultDuration;
  return d != null && !common.includes(d)
    ? [...common, d].sort((a, b) => a - b)
    : common;
}

/**
 * Route selection.
 *
 * A stored preference wins only if the model still exposes it — routes come
 * and go as providers delist models, and a stale preference must not pin the
 * UI to a route that can no longer run.
 */
export function selectRoute(
  model: Model | null,
  preferredRouteId?: string | null,
): Route | null {
  if (!model || model.routes.length === 0) return null;
  if (preferredRouteId) {
    const match = model.routes.find((r) => r.id === preferredRouteId);
    if (match) return match;
  }
  return model.routes[0];
}

export function routeLabel(route: Route | null | undefined): string {
  if (!route) return "No route";
  return `${route.provider} · ${route.slug}`;
}

/**
 * Preset variant selection.
 *
 * Exact model match first, then the family's own default (its first variant),
 * then null. The middle step matters: most families are authored against one
 * model and derived for the rest, and falling straight to null would hide
 * every preset the moment a user switched model.
 */
export function selectVariant(
  family: PresetFamily | null | undefined,
  modelId: string | null | undefined,
): PresetVariant | null {
  if (!family?.variants || family.variants.length === 0) return null;
  if (modelId) {
    const exact = family.variants.find((v) => v.modelId === modelId);
    if (exact) return exact;
  }
  return family.variants[0];
}

/** True when the family was authored for this model rather than derived. */
export function isNativeVariant(
  family: PresetFamily | null | undefined,
  modelId: string | null | undefined,
): boolean {
  if (!family?.variants || !modelId) return false;
  return family.variants.some((v) => v.modelId === modelId);
}

function nearest(value: number, options: number[]): number {
  return options.reduce((best, option) => {
    const d = Math.abs(option - value);
    const bestD = Math.abs(best - value);
    // Ties go to the smaller value: rounding a duration up silently costs the
    // user money, rounding it down does not.
    if (d < bestD || (d === bestD && option < best)) return option;
    return best;
  }, options[0]);
}

/**
 * Clamp settings to what the selected model can actually run, so switching
 * model never leaves an unsupported value staged for submit. Returns the same
 * object when nothing changed, which keeps this safe to call in a render path.
 */
export function resolveSettings(
  settings: GenSettings,
  caps: ModelCapabilities,
): GenSettings {
  // An enumerated axis clamps to its options. A free-form or unknown axis is
  // left alone: clamping against a list we invented is how a wrong value gets
  // laundered into a confident one.
  const duration =
    caps.durations.length === 0
      ? settings.duration
      : caps.durations.includes(settings.duration)
        ? settings.duration
        : nearest(settings.duration, caps.durations);

  // Prefer the model's own default over its first option — "the first one in
  // the list" is an arbitrary choice, and several models list their cheapest
  // or lowest-quality option first.
  const resolution = caps.resolutions.includes(settings.resolution)
    ? settings.resolution
    : (caps.defaultResolution ?? caps.resolutions[0] ?? settings.resolution);

  const aspect = caps.aspects.includes(settings.aspect)
    ? settings.aspect
    : (caps.defaultAspect ?? caps.aspects[0] ?? settings.aspect);

  const audio = caps.audio ? settings.audio : false;

  if (
    duration === settings.duration &&
    resolution === settings.resolution &&
    aspect === settings.aspect &&
    audio === settings.audio
  ) {
    return settings;
  }
  return { ...settings, duration, resolution, aspect, audio };
}
