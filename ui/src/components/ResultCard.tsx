import { useState } from "react";
import { previewOf } from "../lib/media-input";
import type { JobSet } from "../types";
import {
  aspectRatioOf,
  isPlayableVideo,
  posterFor,
  ratioFromAspectLabel,
} from "../lib/media";
import { isFailure, isRunning, statusLabel } from "../lib/status";
import { RainbowPlaceholder, StatusLine } from "./Loader";
import { AlertIcon } from "./Icons";
import { VideoPreview } from "./VideoPreview";

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

  // What the file turned out to be, which is not always what was asked for:
  // an endpoint with no aspect control returns the shape of its input, so the
  // requested ratio is a prediction and the measured one is the truth. Held in
  // state because it only becomes knowable once the media loads.
  const [measured, setMeasured] = useState<number | null>(null);

  // Order of preference: the file, then the file's declared size, then the
  // request. A card previously used the request alone and every result — a 9:16
  // vertical clip included — was drawn in a landscape box.
  const ratio = running
    ? // A running card holds the requested shape so it does not jump when the
      // result lands, which would shunt everything below it down the page.
      requested
    : (measured ?? (first ? aspectRatioOf(first) : requested));

  // A running generation is backed by whatever input we already have — the
  // start frame, or a reference. Blurred and scaled past the edges it reads as
  // ambient colour from the right scene rather than a grey hole.
  const firstInput = job.media?.[0];
  const backdrop = firstInput ? previewOf(firstInput) : undefined;

  return (
    <article
      className="result-card"
      data-portrait={ratio < 1 || undefined}
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
            <VideoPreview
              result={first}
              className="result-card-media"
              onMeasured={setMeasured}
            />
          ) : (
            <img
              className="result-card-media"
              src={posterFor(first)}
              alt=""
              onLoad={(e) => {
                const el = e.currentTarget;
                if (el.naturalWidth > 0 && el.naturalHeight > 0) {
                  setMeasured(el.naturalWidth / el.naturalHeight);
                }
              }}
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
