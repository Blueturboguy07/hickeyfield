import { forwardRef } from "react";
import type { JobSet, Model } from "../types";
import { totalSpend } from "../lib/cost";
import { formatUsd } from "../lib/cost";
import { EmptyState } from "./EmptyState";
import { MetaCard } from "./MetaCard";

/**
 * The right column. One card per generation, in the same order as the feed, so
 * the two columns read as rows even though they scroll independently.
 *
 * The spend meter counts only priced jobs and says how many it could not
 * price, rather than quietly treating unknown as zero and under-reporting what
 * the session cost.
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
  const spend = totalSpend(
    jobs.map((j) => (j.actualUsd === undefined ? j.estimatedUsd : j.actualUsd)),
  );

  return (
    <aside className="rail rail-meta" aria-label="Generation details" ref={ref}>
      <div className="spend-meter">
        <span className="spend-meter-label">Session spend</span>
        <span className="spend-meter-value">{formatUsd(spend.usd)}</span>
        {spend.unknownCount > 0 ? (
          <span className="spend-meter-note">
            {spend.unknownCount} unpriced
          </span>
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
