import { forwardRef } from "react";
import type { JobSet } from "../types";
import { isRunning, QUEUE_EXPECTATION } from "../lib/status";
import { EmptyState } from "./EmptyState";
import { ResultCard } from "./ResultCard";

/**
 * The centre column: newest first, one card per generation.
 *
 * The header carries the running count and the expectation copy instead of a
 * progress bar. That is the whole of our progress reporting, deliberately —
 * providers give us no calibrated ETA, and a bar that guesses is a lie the
 * user can time.
 */
export const ResultsFeed = forwardRef<
  HTMLDivElement,
  {
    jobs: JobSet[];
    /** Last submit failure, or null. Rendered above the feed. */
    error?: string | null;
    onDismissError?: () => void;
    selectedId: string | null;
    onSelect: (id: string) => void;
    onCancel: (id: string) => void;
    onFocusPrompt: () => void;
  }
>(function ResultsFeed(
  { jobs, error, onDismissError, selectedId, onSelect, onCancel, onFocusPrompt },
  ref,
) {
  const running = jobs.filter((j) => isRunning(j.status)).length;

  return (
    <section className="feed" aria-label="Results" ref={ref}>
      <header className="feed-header">
        <h1 className="feed-title">RESULTS</h1>
        <p className="feed-sub">
          {running > 0
            ? `${running} running · ${QUEUE_EXPECTATION}`
            : `${jobs.length} generation${jobs.length === 1 ? "" : "s"}`}
        </p>
      </header>

      {error ? (
        // role="alert" so a screen reader announces a refusal the user did not
        // scroll to. The provider's own wording is shown verbatim: it is far
        // more specific than anything we could paraphrase.
        <div className="feed-error" role="alert">
          <p className="feed-error-text">{error}</p>
          {onDismissError ? (
            <button
              type="button"
              className="btn btn-quiet feed-error-dismiss"
              onClick={onDismissError}
            >
              Dismiss
            </button>
          ) : null}
        </div>
      ) : null}

      {jobs.length === 0 ? (
        <EmptyState
          heading="Nothing generated yet"
          explanation="Your results land here, newest first, and stay on this machine. Nothing is uploaded to us."
          action={
            <button type="button" className="btn btn-primary" onClick={onFocusPrompt}>
              Write a prompt
            </button>
          }
        />
      ) : (
        <div className="feed-list">
          {jobs.map((job) => (
            <div key={job.id} data-job-anchor={job.id}>
              <ResultCard
                job={job}
                selected={job.id === selectedId}
                onSelect={() => onSelect(job.id)}
                onCancel={() => onCancel(job.id)}
              />
            </div>
          ))}
        </div>
      )}
    </section>
  );
});
