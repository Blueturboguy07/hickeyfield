import type { CostEstimate } from "../types";

/**
 * The one string the whole cost surface hangs on.
 *
 * Several provider price feeds ship blank `pricingInfoOverride` fields, so an
 * unknown price is a routine state, not an error. It must never collapse into
 * "$0.00" or "Free": a user who reads "free" and gets billed loses trust in
 * every other number we show.
 */
export const UNKNOWN_COST_LABEL = "Price unavailable";

/**
 * USD with a decimal count that scales to the magnitude.
 *
 * Video calls land around $1-4 where cents are what matter; per-image calls
 * land around $0.004 where two decimals would round everything to "$0.00" —
 * which is precisely the false-free reading we refuse elsewhere.
 */
export function formatUsd(usd: number): string {
  if (!Number.isFinite(usd) || usd < 0) return UNKNOWN_COST_LABEL;
  if (usd === 0) return "$0.00";
  if (usd >= 0.1) return `$${usd.toFixed(2)}`;
  if (usd >= 0.01) return `$${usd.toFixed(3)}`;
  return `$${usd.toFixed(4)}`;
}

/** Renders an estimate, including the explicit unknown case. */
export function formatCost(estimate: CostEstimate | null | undefined): string {
  if (!estimate) return UNKNOWN_COST_LABEL;
  return formatUsd(estimate.usd);
}

/** True when we have a number the user can rely on. */
export function hasPrice(estimate: CostEstimate | null | undefined): boolean {
  return formatCost(estimate) !== UNKNOWN_COST_LABEL;
}

/**
 * The secondary line under a cost: how it was derived, and whether a provider
 * floor kicked in (fal bills a 15s minimum on some models, so a 4s clip can
 * cost the same as a 15s one — silently showing the floor price with no
 * explanation reads as a bug).
 */
export function costBasisNote(
  estimate: CostEstimate | null | undefined,
): string | null {
  if (!estimate) return "No published price for this route";
  if (estimate.minimumApplied) {
    return estimate.basis
      ? `${estimate.basis} · provider minimum applied`
      : "Provider minimum applied";
  }
  return estimate.basis || null;
}

/** The inline cost on the Generate button. */
export function generateButtonCost(
  estimate: CostEstimate | null | undefined,
): string {
  return formatCost(estimate);
}

/**
 * Actual cost is only meaningful once a job has settled, and a settled job
 * that reports nothing is different from one that reported zero.
 */
export function formatActual(actualUsd: number | null | undefined): string {
  if (actualUsd === null || actualUsd === undefined) return "Not reported";
  return formatUsd(actualUsd);
}

/** Sum for the spend meter. Unknown prices are excluded, never counted as 0. */
export function totalSpend(values: Array<number | null | undefined>): {
  usd: number;
  unknownCount: number;
} {
  let usd = 0;
  let unknownCount = 0;
  for (const v of values) {
    if (v === null || v === undefined || !Number.isFinite(v) || v < 0) {
      unknownCount += 1;
    } else {
      usd += v;
    }
  }
  return { usd, unknownCount };
}
