/**
 * Getting an attached file from the webview to the provider.
 *
 * The subtlety that makes this a module rather than three lines in a click
 * handler: `<input type="file">` gives a `File` whose path the browser
 * deliberately hides, and `URL.createObjectURL` returns a `blob:` URL that only
 * this webview can resolve. Handing either to Rust produces a request that
 * fails with something unhelpful about a malformed URL. The Rust side needs a
 * **real filesystem path**, and on desktop the only thing that yields one is
 * the Tauri dialog plugin.
 *
 * So there are two paths on purpose:
 *
 * - **In the app**, the dialog plugin returns real paths. Rust opens the file
 *   and uploads it. Nothing large ever crosses the bridge.
 * - **In a plain browser** (`pnpm dev` with no shell), there is no such thing
 *   as a path, so the file is read into a `data:` URI instead. That keeps the
 *   whole UI runnable without the shell — a large iteration-speed win — at the
 *   cost of holding the bytes in memory, which is why it is the fallback and
 *   not the default.
 */

import type { MediaRef, MediaRole, MediaSource } from "../types";

/** Roles that take a still; used to filter the dialog and the file input. */
const IMAGE_ROLES: MediaRole[] = ["start", "end", "reference"];

export function acceptFor(role: MediaRole): string {
  if (IMAGE_ROLES.includes(role)) return "image/*";
  if (role === "audio" || role === "audio_reference") return "audio/*";
  return "video/*";
}

function extensionsFor(role: MediaRole): { name: string; extensions: string[] } {
  if (IMAGE_ROLES.includes(role)) {
    return { name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif", "heic"] };
  }
  if (role === "audio" || role === "audio_reference") {
    return { name: "Audio", extensions: ["mp3", "wav", "m4a", "flac", "ogg"] };
  }
  return { name: "Video", extensions: ["mp4", "mov", "webm", "m4v"] };
}

/** What to show in the slot or the reference strip. */
export function previewOf(m: MediaRef): string | undefined {
  if (m.preview) return m.preview;
  if (m.source.kind === "url") return m.source.url;
  if (m.source.kind === "data_uri") return m.source.data;
  // A local path is not loadable by the webview without the asset protocol,
  // so callers fall back to the filename chip rather than a broken image.
  return undefined;
}

/**
 * A stable identity for one attachment.
 *
 * Used as a React key and by removal. Deliberately the source rather than the
 * preview: two picks of the same file produce different object URLs but are
 * the same attachment, and keying on the preview would let a duplicate through.
 */
export function sourceKey(m: MediaRef): string {
  switch (m.source.kind) {
    case "local":
      return `local:${m.source.path}`;
    case "url":
      return `url:${m.source.url}`;
    default:
      // Data URIs can be megabytes; the head is enough to distinguish picks
      // and keeps the key cheap to compare.
      return `data:${m.source.data.slice(0, 96)}`;
  }
}

export function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

function readAsDataUri(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(new Error(`could not read ${file.name}`));
    r.readAsDataURL(file);
  });
}

/** Browser fallback: turn picked `File`s into data-URI attachments. */
export async function fromFileList(
  role: MediaRole,
  files: FileList | null,
  multiple: boolean,
): Promise<MediaRef[]> {
  if (!files || files.length === 0) return [];
  const chosen = multiple ? Array.from(files) : [files[0]];
  return Promise.all(
    chosen.map(async (file) => ({
      role,
      source: { kind: "data_uri", data: await readAsDataUri(file) } as MediaSource,
      // An object URL renders instantly; the data URI would too, but this
      // avoids re-decoding a multi-megabyte string on every render.
      preview: URL.createObjectURL(file),
      name: file.name,
    })),
  );
}

/**
 * Open the native file dialog and return real paths.
 *
 * Returns `null` — not `[]` — when the shell is absent, so the caller can tell
 * "no Tauri here, use the browser path" apart from "the user cancelled".
 */
export async function pickViaDialog(
  role: MediaRole,
  multiple: boolean,
): Promise<MediaRef[] | null> {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const picked = await open({
      multiple,
      directory: false,
      filters: [extensionsFor(role)],
    });
    if (picked === null) return [];
    const paths = Array.isArray(picked) ? picked : [picked];

    // A local path is not loadable by the webview until the asset protocol is
    // told about it, so ask before building a preview URL. Without this the
    // rail shows a filename where a thumbnail belongs and nothing can measure
    // the shape of the input.
    let convert: ((p: string) => string) | null = null;
    try {
      const { invoke, convertFileSrc } = await import("@tauri-apps/api/core");
      await invoke("allow_media_preview", { paths });
      convert = convertFileSrc;
    } catch {
      // No grant, no preview — the filename chip still works, and an
      // attachment the user cannot see a thumbnail of is a smaller failure
      // than an attachment they cannot make at all.
    }

    return paths.map((path) => ({
      role,
      source: { kind: "local", path } as MediaSource,
      preview: convert ? convert(path) : undefined,
      name: basename(path),
    }));
  } catch {
    // No shell — the import itself fails in a plain browser.
    return null;
  }
}

/**
 * Merge new attachments into the existing set.
 *
 * Single-slot roles replace; repeatable roles append and de-duplicate. Getting
 * this wrong is how you end up with two start frames, which binds to one flag
 * and silently drops the other.
 */
export function mergeMedia(
  existing: MediaRef[],
  added: MediaRef[],
  multiple: boolean,
): MediaRef[] {
  if (added.length === 0) return existing;
  const role = added[0].role;
  if (!multiple) {
    return [...existing.filter((m) => m.role !== role), added[0]];
  }
  const seen = new Set(existing.map(sourceKey));
  const fresh = added.filter((m) => !seen.has(sourceKey(m)));
  return [...existing, ...fresh];
}
