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

/**
 * What the session has cost, and how much of that is a guess.
 *
 * Providers mostly do not report what they charged — fal's status payload
 * carries no cost field at all — so a meter that counts only settled charges
 * reads "$0.00" after eighteen paid generations. That is the worst number this
 * app can display: it is not merely unhelpful, it is the opposite of true, and
 * it appears directly above a column of cards each showing a real estimate.
 *
 * So the meter falls back to the estimate and *says* that it did. The split is
 * returned rather than folded into one number, because "$3.60, all measured"
 * and "$3.60, all estimated" are different claims and the UI must not make the
 * stronger one on the weaker evidence.
 */
export function sessionSpend(
  jobs: Array<{ actualUsd?: number | null; estimatedUsd?: number | null }>,
): {
  usd: number;
  /** Jobs whose provider told us what it charged. */
  actualCount: number;
  /** Jobs counted at our own estimate because nothing better exists. */
  estimatedCount: number;
  /** Jobs with no number at all. Excluded from the sum, never read as free. */
  unknownCount: number;
} {
  let usd = 0;
  let actualCount = 0;
  let estimatedCount = 0;
  let unknownCount = 0;

  for (const job of jobs) {
    // `?? ` and not `=== undefined`: the wire mapper normalises a missing cost
    // to `null`, so an identity check against `undefined` matched nothing and
    // every job fell through to "unpriced". Both spellings type-check against
    // `number | null | undefined`, which is why it survived review.
    const actual = usable(job.actualUsd);
    if (actual !== null) {
      usd += actual;
      actualCount += 1;
      continue;
    }
    const estimated = usable(job.estimatedUsd);
    if (estimated !== null) {
      usd += estimated;
      estimatedCount += 1;
      continue;
    }
    unknownCount += 1;
  }

  return { usd, actualCount, estimatedCount, unknownCount };
}

function usable(v: number | null | undefined): number | null {
  return v === null || v === undefined || !Number.isFinite(v) || v < 0 ? null : v;
}

/**
 * How the spend meter's own number should be qualified, in the meter's own
 * words. Empty when every counted job reported a real charge.
 */
export function spendQualifier(spend: {
  estimatedCount: number;
  unknownCount: number;
}): string | null {
  const parts: string[] = [];
  if (spend.estimatedCount > 0) parts.push(`${spend.estimatedCount} estimated`);
  if (spend.unknownCount > 0) parts.push(`${spend.unknownCount} unpriced`);
  return parts.length > 0 ? parts.join(" · ") : null;
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
