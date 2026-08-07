import { forwardRef } from "react";
import type { JobSet, Model } from "../types";
import { sessionSpend, spendQualifier } from "../lib/cost";
import { formatUsd } from "../lib/cost";
import { EmptyState } from "./EmptyState";
import { MetaCard } from "./MetaCard";

/**
 * The right column. One card per generation, in the same order as the feed, so
 * the two columns read as rows even though they scroll independently.
 *
 * The spend meter prefers what a provider actually charged, falls back to our
 * own estimate, and labels which it used — rather than reporting `$0.00` for a
 * session of paid generations because almost no provider reports a charge.
 */
export const MetaRail = forwardRef<
  HTMLElement,
  {
    jobs: JobSet[];
    models: Model[];
    selectedId: string | null;
    onSelect: (id: string) => void;
    onRerun: (job: JobSet) => void;
    onDelete: (id: string) => void;
  }
>(function MetaRail({ jobs, models, selectedId, onSelect, onRerun, onDelete }, ref) {
  const spend = sessionSpend(jobs);
  const qualifier = spendQualifier(spend);
  // The tilde is the whole disclosure at a glance: an exact figure and an
  // approximate one must not be typeset identically.
  const approximate = spend.estimatedCount > 0;

  return (
    <aside className="rail rail-meta" aria-label="Generation details" ref={ref}>
      <div className="spend-meter">
        <span className="spend-meter-label">Session spend</span>
        <span className="spend-meter-value">
          {approximate ? "~" : ""}
          {formatUsd(spend.usd)}
        </span>
        {qualifier ? (
          <span className="spend-meter-note">{qualifier}</span>
        ) : null}
      </div>

      {jobs.length === 0 ? (
        <EmptyState
          heading="No details yet"
          explanation="Each generation gets a card here with its route, its real cost and the exact settings that produced it."
          tone="default"
        />
      ) : (
        <div className="meta-list">
          {jobs.map((job) => (
            <div key={job.id} data-meta-anchor={job.id}>
              <MetaCard
                job={job}
                model={models.find((m) => m.id === job.modelId) ?? null}
                selected={job.id === selectedId}
                onSelect={() => onSelect(job.id)}
                onRerun={() => onRerun(job)}
                onDelete={() => onDelete(job.id)}
              />
            </div>
          ))}
        </div>
      )}
    </aside>
  );
});
