import { previewOf } from "../lib/media-input";
import type { JobSet } from "../types";
import {
  aspectRatioOf,
  isPlayableVideo,
  posterFor,
  ratioFromAspectLabel,
  displayUrl,
} from "../lib/media";
import { isFailure, isRunning, statusLabel } from "../lib/status";
import { RainbowPlaceholder, StatusLine } from "./Loader";
import { AlertIcon } from "./Icons";

/**
 * One generation in the centre feed. No caption: the prompt, the model and
 * every number live in the paired meta card, and duplicating them here would
 * halve the media area for no new information.
 */
export function ResultCard({
  job,
  selected,
  onSelect,
  onCancel,
}: {
  job: JobSet;
  selected: boolean;
  onSelect: () => void;
  onCancel: () => void;
}) {
  const running = isRunning(job.status);
  const failed = isFailure(job.status);
  const first = job.results[0];
  const requested = ratioFromAspectLabel(job.settings?.aspect ?? "16:9");
  const mediaRatio = first ? aspectRatioOf(first) : requested;
  // Portrait media is letterboxed into a wide panel rather than given a card
  // of its own shape: a 9:16 result across a 1030px column would be nearly two
  // screens tall, and scrolling past one result is not a feed.
  const portrait = mediaRatio < 1;
  // A running card also holds the wide frame, so the card does not resize the
  // moment the result lands and shunt the rest of the feed down.
  const ratio = running || portrait ? 16 / 9 : mediaRatio;

  // A running generation is backed by whatever input we already have — the
  // start frame, or a reference. Blurred and scaled past the edges it reads as
  // ambient colour from the right scene rather than a grey hole.
  const firstInput = job.media?.[0];
  const backdrop = firstInput ? previewOf(firstInput) : undefined;

  return (
    <article
      className="result-card"
      data-selected={selected || undefined}
      data-status={job.status}
      aria-label={`Generation ${job.id}, ${statusLabel(job.status)}`}
    >
      <button
        type="button"
        className="result-card-surface"
        onClick={onSelect}
        style={
          {
            aspectRatio: String(ratio),
            ...(backdrop ? { "--bg-image": `url("${backdrop}")` } : {}),
          } as React.CSSProperties
        }
        data-blurred={running && backdrop ? true : undefined}
      >
        {running ? (
          <span className="result-card-running">
            <RainbowPlaceholder
              className="result-card-rainbow"
              ratio={requested}
            />
            <span className="result-card-status">
              <StatusLine label={statusLabel(job.status)} />
            </span>
          </span>
        ) : failed ? (
          <span className="result-card-failed">
            <AlertIcon size={22} />
            <span className="result-card-failed-title">
              {statusLabel(job.status)}
            </span>
            <span className="result-card-failed-body">
              {job.failReason ?? "The provider rejected this generation."}
            </span>
          </span>
        ) : first ? (
          isPlayableVideo(first) ? (
            <video
              className="result-card-media"
              data-letterboxed={portrait || undefined}
              src={displayUrl(first)}
              poster={first.poster}
              loop
              muted
              playsInline
              preload="none"
            />
          ) : (
            <img
              className="result-card-media"
              data-letterboxed={portrait || undefined}
              src={posterFor(first)}
              alt=""
            />
          )
        ) : (
          <span className="result-card-empty">No output returned</span>
        )}
        <span className="media-bevel" aria-hidden="true" />
      </button>

      {running ? (
        <button
          type="button"
          className="btn btn-scrim result-card-cancel"
          onClick={onCancel}
        >
          Cancel
        </button>
      ) : null}
    </article>
  );
}
