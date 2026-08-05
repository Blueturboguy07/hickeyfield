import type { CostEstimate } from "../types";
import { costBasisNote, formatCost, hasPrice } from "../lib/cost";
import { KeyIcon, SparkleIcon } from "./Icons";

/**
 * The submit control.
 *
 * The cost sits inside the button in USD, not credits — there is no currency
 * of ours between the user and the provider. When the provider publishes no
 * price the button says so in words; it never falls back to a zero, and it
 * stays enabled, because an unknown price is not a reason to block a
 * generation the user has already decided to pay for.
 *
 * A missing key is different from a missing prompt: it is not something the
 * rail can be fixed to satisfy, so the button changes into the way to fix it
 * rather than sitting greyed out with an explanation nobody can act on.
 */
export function GenerateButton({
  estimate,
  pending,
  blockedReason,
  needsSetup = false,
  onSubmit,
  onSetup,
}: {
  estimate: CostEstimate | null;
  pending: boolean;
  blockedReason: string | null;
  /** No provider is usable — nothing can be generated at any price. */
  needsSetup?: boolean;
  onSubmit: () => void;
  onSetup?: () => void;
}) {
  const priced = hasPrice(estimate);
  const note = costBasisNote(estimate);

  if (needsSetup) {
    return (
      <div className="generate">
        <button
          type="button"
          className="generate-button"
          onClick={onSetup}
          aria-describedby="generate-note"
        >
          <span className="generate-label">Add a provider key</span>
          <KeyIcon size={16} className="generate-spark" />
        </button>
        <p id="generate-note" className="generate-note">
          Halation generates with your own keys. Nothing can run until one is
          set.
        </p>
      </div>
    );
  }

  return (
    <div className="generate">
      <button
        type="button"
        className="generate-button"
        disabled={Boolean(blockedReason) || pending}
        onClick={onSubmit}
        aria-describedby="generate-note"
      >
        <span className="generate-label">Generate</span>
        <SparkleIcon size={16} className="generate-spark" />
        <span className="generate-cost" data-unknown={!priced || undefined}>
          {formatCost(estimate)}
        </span>
      </button>
      <p id="generate-note" className="generate-note">
        {blockedReason ?? note ?? ""}
      </p>
    </div>
  );
}
