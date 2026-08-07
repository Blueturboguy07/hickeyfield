import { useEffect, useRef, useState } from "react";
import type { JobResult } from "../types";
import { isSaved, previewSrc } from "../lib/media";

/**
 * A generated clip, previewing in the feed.
 *
 * Three things here are load-bearing, and all three were previously absent —
 * which is why every finished generation rendered as an empty grey box:
 *
 * 1. **Something must actually request the bytes.** The old element carried
 *    `preload="none"` with no poster and no autoplay, so it never fetched a
 *    single byte and had no frame to paint. Nothing was broken downstream; the
 *    file was simply never asked for.
 * 2. **Playback is driven by visibility, not by mount.** Autoplaying every
 *    card would have eighteen H.264 decoders running for three visible cards.
 *    An IntersectionObserver plays what is on screen and pauses what is not.
 * 3. **A failure has to be visible.** A `<video>` that cannot load its source
 *    fails *silently* — same empty box, no console error, no event unless you
 *    listen for one. Since that silence is exactly what made this bug survive
 *    two attempted fixes, `error` is handled and rendered.
 */
export function VideoPreview({
  result,
  className,
  letterboxed,
  onMeasured,
}: {
  result: JobResult;
  className?: string;
  letterboxed?: boolean;
  /** The clip's true pixel ratio, once the metadata says what it is. */
  onMeasured?: (ratio: number) => void;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const [failed, setFailed] = useState(false);
  const src = previewSrc(result);

  // Re-arm on a new source: a card that failed once must not stay failed after
  // Rerun hands it a different file.
  useEffect(() => setFailed(false), [src]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    // jsdom has no IntersectionObserver, and neither does any browser old
    // enough to matter here — but a missing observer must degrade to "play it"
    // rather than to the silent blank box this component exists to prevent.
    if (typeof IntersectionObserver === "undefined") {
      void el.play().catch(() => {});
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          // Muted playback is allowed without a gesture; the rejection is
          // caught anyway because a rejected promise here is an unhandled
          // rejection in the console, not a user-visible failure.
          void el.play().catch(() => {});
        } else {
          el.pause();
        }
      },
      { threshold: 0.25 },
    );

    observer.observe(el);
    return () => observer.disconnect();
  }, [src]);

  if (failed) {
    // A job reports "completed" the moment the provider is done, which is
    // before the bytes are on disk — so a load failure in that window is a race
    // with our own download, not a broken result, and it clears itself when the
    // saved path arrives. Calling it an error there would teach the user to
    // distrust the error.
    return (
      <span className="result-card-empty" role="status">
        {isSaved(result) ? "Couldn’t play this file" : "Saving…"}
      </span>
    );
  }

  return (
    <video
      ref={ref}
      className={className}
      data-letterboxed={letterboxed || undefined}
      src={src}
      poster={result.poster}
      loop
      muted
      playsInline
      // Metadata, not "auto": the frame comes from the fragment seek in
      // `previewSrc`, and preloading eighteen full clips would read hundreds of
      // megabytes off disk to show three of them.
      preload="metadata"
      onError={() => setFailed(true)}
      onLoadedMetadata={(e) => {
        // The only trustworthy source for the shape of a result. The request
        // said what we wanted; several endpoints have no aspect control at all
        // and return whatever the input was, so the file itself is the fact.
        const el = e.currentTarget;
        if (el.videoWidth > 0 && el.videoHeight > 0) {
          onMeasured?.(el.videoWidth / el.videoHeight);
        }
      }}
    />
  );
}
