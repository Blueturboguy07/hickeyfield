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
