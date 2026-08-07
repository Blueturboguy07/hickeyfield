import type { JobResult } from "../types";

/**
 * Convert a filesystem path into something the webview can load.
 *
 * Set once at startup by `main.tsx` from Tauri's `convertFileSrc`. Held as a
 * module-level hook rather than imported directly so this module stays pure
 * and testable, and so the whole UI still runs in a plain browser — where no
 * such conversion exists and local paths simply are not renderable.
 */
let convertPath: ((path: string) => string) | null = null;

export function setPathConverter(fn: (path: string) => string): void {
  convertPath = fn;
}

/**
 * The URL to actually render for a result.
 *
 * **Local file first.** Provider URLs are signed and expiring — fal's lapse,
 * Higgsfield deletes after 7 days — so preferring them means a feed that goes
 * blank while the files sit safely on disk, directly contradicting the "your
 * results stay on this machine" promise the feed itself renders.
 */
export function displayUrl(result: JobResult): string {
  if (result.localPath && convertPath) return convertPath(result.localPath);
  return result.url;
}

/** True once the bytes are ours and the provider URL no longer matters. */
export function isSaved(result: JobResult): boolean {
  return Boolean(result.localPath);
}

const PLAYABLE = /\.(mp4|webm|mov|m4v)(\?|#|$)/i;

/**
 * A result can be tagged `video` before a playable file exists — mock data and
 * poster-only intermediate states both do it. Handing such a URL to <video>
 * gives a permanently black box with no error, so the element choice is made
 * on the URL, not on the kind.
 */
export function isPlayableVideo(result: JobResult): boolean {
  // Tested against the URL we will actually render: several providers hand
  // back an extensionless signed URL while the saved file is a plain .mp4, and
  // testing the wrong one leaves a permanently black <video> box.
  return (
    result.kind === "video" &&
    (PLAYABLE.test(result.localPath ?? "") || PLAYABLE.test(result.url))
  );
}

/** Same test for a bare URL, used by preview tiles that have no JobResult. */
export function isPlayableUrl(url: string | undefined): boolean {
  return Boolean(url) && PLAYABLE.test(url as string);
}

/** The still to show when the clip itself cannot render. */
export function posterFor(result: JobResult): string {
  return result.poster ?? displayUrl(result);
}

/**
 * The `src` to hand a paused feed video so that it paints a frame.
 *
 * A `<video>` with no poster shows *nothing* until it has decoded a frame, and
 * `preload="metadata"` fetches the container header only — dimensions and
 * duration, no picture. WKWebView is strict about this, so a feed of finished
 * generations rendered as empty boxes sitting next to perfectly good files.
 *
 * A media-fragment start time is the fix: it makes the element seek on load,
 * which forces exactly one frame to be decoded and painted. 0.1s rather than 0
 * because the first frame of a generated clip is frequently black — a fade-up
 * from nothing is the single most common opening in this whole medium, and a
 * black thumbnail is indistinguishable from the bug we are fixing.
 *
 * Fragments are resolved by the media element and never sent to the server, so
 * this is invisible to the asset protocol and to any provider URL.
 */
export function previewSrc(result: JobResult): string {
  const url = displayUrl(result);
  return url.includes("#") ? url : `${url}#t=0.1`;
}

/**
 * Feed cards are sized to the media's own aspect ratio, so a missing
 * width/height must still produce a box rather than a zero-height card.
 */
export function aspectRatioOf(result: JobResult | undefined): number {
  if (result?.width && result?.height && result.height > 0) {
    return result.width / result.height;
  }
  return 16 / 9;
}

export function ratioFromAspectLabel(label: string): number {
  const [w, h] = label.split(":").map(Number);
  if (Number.isFinite(w) && Number.isFinite(h) && h > 0) return w / h;
  return 16 / 9;
}

export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
  return `${seconds}s`;
}

export function formatCreatedAt(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/**
 * The closest aspect the model actually offers to a measured shape.
 *
 * Snapping rather than matching exactly, because a real photo is never exactly
 * 16:9 — a 4032×3024 phone still is 1.333, and the offered list holds `4:3`,
 * not `1.333`. Compared on the log of the ratio so that 2:1 and 1:2 are the
 * same distance from square; on a linear scale every portrait option would
 * crowd together below 1 and the nearest match to a tall clip would depend on
 * how many landscape options happened to be in the list.
 *
 * `null` when there is nothing to choose from — never a guess.
 */
export function nearestAspect(
  ratio: number,
  options: string[],
): string | null {
  if (!Number.isFinite(ratio) || ratio <= 0 || options.length === 0) return null;
  const target = Math.log(ratio);
  let best: string | null = null;
  let bestGap = Infinity;
  for (const option of options) {
    const candidate = ratioFromAspectLabel(option);
    // `ratioFromAspectLabel` answers 16:9 for anything it cannot parse, which
    // would make an unparseable option a plausible-looking match for a
    // widescreen clip. Only accept options that really are `w:h`.
    if (!/^\d+\s*:\s*\d+$/.test(option)) continue;
    const gap = Math.abs(Math.log(candidate) - target);
    if (gap < bestGap) {
      bestGap = gap;
      best = option;
    }
  }
  return best;
}

/**
 * Measure what an attachment actually is.
 *
 * Resolves to `null` rather than rejecting: a preview that will not load is a
 * missing measurement, not an error worth interrupting an attachment over.
 */
export function measureAspect(
  url: string,
  kind: "image" | "video",
): Promise<number | null> {
  return new Promise((resolve) => {
    if (kind === "video") {
      const el = document.createElement("video");
      el.preload = "metadata";
      el.onloadedmetadata = () =>
        resolve(el.videoWidth > 0 ? el.videoWidth / el.videoHeight : null);
      el.onerror = () => resolve(null);
      el.src = url;
      return;
    }
    const img = new Image();
    img.onload = () =>
      resolve(img.naturalWidth > 0 ? img.naturalWidth / img.naturalHeight : null);
    img.onerror = () => resolve(null);
    img.src = url;
  });
}
