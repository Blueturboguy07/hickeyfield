import { useState } from "react";
import { previewOf, sourceKey } from "../lib/media-input";
import { revealResult } from "../api";
import type { JobSet, Model } from "../types";
import { formatActual, formatUsd, UNKNOWN_COST_LABEL } from "../lib/cost";
import { canRerun, isRunning, statusLabel, statusTone } from "../lib/status";
import { formatCreatedAt, formatDuration } from "../lib/media";
import { routeLabel } from "../lib/variants";
import {
  ClockIcon,
  CopyIcon,
  RerunIcon,
  SeedIcon,
  SpeakerIcon,
  SpeakerOffIcon,
  StepsIcon,
  TrashIcon,
  FolderIcon,
} from "./Icons";

/**
 * The metadata card paired with one generation.
 *
 * Route, estimated cost, actual cost and enhancer version are ours — the
 * product we are rebuilding shows none of them. They are the whole argument
 * for bringing your own key: which provider actually ran this, what we thought
 * it would cost, what it did cost, and what rewrote your prompt.
 */
export function MetaCard({
  job,
  model,
  selected,
  onSelect,
  onRerun,
  onDelete,
}: {
  job: JobSet;
  model: Model | null;
  selected: boolean;
  onSelect: () => void;
  onRerun: () => void;
  onDelete: () => void;
}) {
  const [copied, setCopied] = useState(false);
  // The first output that has made it to disk, if any.
  const saved = job.results?.find((r) => r.localPath)?.localPath ?? null;
  const settings = job.settings;
  const refs = job.media ?? [];

  const copyPrompt = async () => {
    try {
      await navigator.clipboard.writeText(job.enhancedPrompt || job.prompt);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      // Clipboard access can be refused by the webview. Failing quietly is
      // right here: the prompt is already on screen and selectable.
    }
  };

  return (
    <article
      className="meta-card"
      data-selected={selected || undefined}
      data-tone={statusTone(job.status)}
      onMouseEnter={onSelect}
    >
      <header className="meta-card-head">
        <span className="pill pill-model">
          {model?.displayName ?? job.modelId}
        </span>
        <span className="meta-card-status" data-tone={statusTone(job.status)}>
          {statusLabel(job.status)}
        </span>
        <span className="meta-card-time">{formatCreatedAt(job.createdAt)}</span>
      </header>

      <p className="meta-card-prompt">{job.prompt}</p>

      {job.enhancedPrompt ? (
        <details className="meta-card-enhanced">
          <summary>Enhanced prompt</summary>
          <p>{job.enhancedPrompt}</p>
        </details>
      ) : null}

      {refs.length > 0 ? (
        <ul className="meta-card-refs" aria-label="Reference inputs">
          {refs.map((ref) => (
            <li key={sourceKey(ref)}>
              {previewOf(ref) ? (
                <img
                  src={previewOf(ref)}
                  alt={ref.name ?? ref.role}
                  title={ref.role}
                />
              ) : (
                <span className="meta-card-ref-name" title={ref.name}>
                  {ref.name}
                </span>
              )}
            </li>
          ))}
        </ul>
      ) : null}

      {settings ? (
        <div className="meta-card-chips">
          <span className="chip chip-static">
            <ClockIcon size={13} />
            {formatDuration(settings.duration)}
          </span>
          <span className="chip chip-static">
            {settings.audio ? (
              <SpeakerIcon size={13} />
            ) : (
              <SpeakerOffIcon size={13} />
            )}
            {settings.audio ? "Audio" : "Silent"}
          </span>
          {settings.steps ? (
            <span className="chip chip-static">
              <StepsIcon size={13} />
              {settings.steps}
            </span>
          ) : null}
          {settings.seed ? (
            <span className="chip chip-static">
              <SeedIcon size={13} />
              {settings.seed}
            </span>
          ) : null}
        </div>
      ) : null}

      <dl className="meta-card-fields">
        <div>
          <dt>Route</dt>
          <dd>{routeLabel(job.route)}</dd>
        </div>
        <div>
          <dt>Estimated cost</dt>
          <dd data-unknown={job.estimatedUsd === null || undefined}>
            {job.estimatedUsd === null
              ? UNKNOWN_COST_LABEL
              : formatUsd(job.estimatedUsd)}
          </dd>
        </div>
        <div>
          <dt>Actual cost</dt>
          <dd data-pending={isRunning(job.status) || undefined}>
            {isRunning(job.status) ? "Pending" : formatActual(job.actualUsd)}
          </dd>
        </div>
        <div>
          <dt>Enhancer</dt>
          {/* Three distinct states, and collapsing them would hide the most
              useful one: rewritten (show which corpus and model), not
              rewritten *for a reason* (show the reason), and never asked. */}
          <dd data-unknown={!job.enhancerVersion && job.enhanceNote ? "" : undefined}>
            {job.enhancerVersion ?? job.enhanceNote ?? "Not enhanced"}
          </dd>
        </div>
      </dl>

      {job.advisories && job.advisories.length > 0 ? (
        // Not styled as an error: the generation happened and is valid. This
        // is "here is what the model could not honour", which the user needs
        // in order to trust the chips they set.
        <ul className="meta-card-advisories">
          {job.advisories.map((a) => (
            <li key={a}>{a}</li>
          ))}
        </ul>
      ) : null}

      <footer className="meta-card-footer">
        <button
          type="button"
          className="btn btn-outline btn-sm"
          onClick={onRerun}
          disabled={!canRerun(job.status)}
        >
          <RerunIcon size={14} />
          Rerun
        </button>
        <div className="meta-card-actions">
          <button
            type="button"
            className="btn btn-icon btn-ghost"
            onClick={copyPrompt}
            aria-label="Copy prompt"
          >
            <CopyIcon size={15} />
          </button>
          {saved ? (
            // Only offered once the bytes are actually ours. A Reveal button
            // that opens an empty folder is worse than no button.
            <button
              type="button"
              className="btn btn-icon btn-ghost"
              onClick={() => void revealResult(saved)}
              aria-label="Show in folder"
              title="Show in folder"
            >
              <FolderIcon size={15} />
            </button>
          ) : null}
          <button
            type="button"
            className="btn btn-icon btn-ghost btn-danger"
            onClick={onDelete}
            aria-label="Delete generation"
          >
            <TrashIcon size={15} />
          </button>
        </div>
        <span className="meta-card-copied" aria-live="polite">
          {copied ? "Copied" : ""}
        </span>
      </footer>
    </article>
  );
}
