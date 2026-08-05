import { beforeEach, describe, expect, it } from "vitest";
import {
  aspectRatioOf,
  displayUrl,
  isPlayableVideo,
  isSaved,
  posterFor,
  setPathConverter,
} from "../media";
import type { JobResult } from "../../types";

const result = (over: Partial<JobResult> = {}): JobResult => ({
  url: "https://queue.fal.run/signed/abc?token=expires-soon",
  kind: "image",
  ...over,
});

beforeEach(() => {
  // The shell's converter, stubbed. Tauri's real one produces an
  // `asset://localhost/...` URL; the shape does not matter here, only that
  // the local path is what gets converted.
  setPathConverter((p) => `asset://${p}`);
});

describe("displayUrl", () => {
  it("prefers the local file over the provider URL", () => {
    // The bug this prevents: fal's signed URLs expire and Higgsfield deletes
    // after 7 days, so a feed rendering `url` goes blank while perfectly good
    // files sit on disk — contradicting the promise the feed itself renders.
    const r = result({ localPath: "/Users/x/Halation/2026-08-04/shot.png" });
    expect(displayUrl(r)).toBe(
      "asset:///Users/x/Halation/2026-08-04/shot.png",
    );
  });

  it("falls back to the provider URL before the download lands", () => {
    // There is a real window between "completed" and "saved", and the result
    // must be visible during it.
    expect(displayUrl(result())).toBe(result().url);
  });

  it("uses the provider URL when there is no converter", () => {
    // A plain browser has no asset protocol; a local path is not loadable, so
    // rendering it would produce a broken image rather than a fallback.
    setPathConverter(undefined as unknown as (p: string) => string);
    const r = result({ localPath: "/tmp/a.png" });
    expect(displayUrl(r)).toBe(r.url);
  });
});

describe("isSaved", () => {
  it("is true only once the bytes are ours", () => {
    expect(isSaved(result())).toBe(false);
    expect(isSaved(result({ localPath: "/tmp/a.png" }))).toBe(true);
  });
});

describe("isPlayableVideo", () => {
  it("recognises a saved mp4 behind an extensionless signed URL", () => {
    // Several providers hand back a signed URL with no extension while the
    // saved file is a plain .mp4. Testing only the provider URL left a
    // permanently black <video> box with no error.
    const r = result({
      kind: "video",
      url: "https://queue.fal.run/requests/abc/output",
      localPath: "/Users/x/Halation/clip.mp4",
    });
    expect(isPlayableVideo(r)).toBe(true);
  });

  it("still recognises a playable provider URL", () => {
    expect(
      isPlayableVideo(result({ kind: "video", url: "https://a/b.mp4" })),
    ).toBe(true);
  });

  it("does not treat an image as a video", () => {
    expect(isPlayableVideo(result({ url: "https://a/b.png" }))).toBe(false);
  });

  it("does not hand a poster-only video to <video>", () => {
    // A result can be tagged video before any playable file exists.
    expect(
      isPlayableVideo(result({ kind: "video", url: "https://a/pending" })),
    ).toBe(false);
  });
});

describe("posterFor", () => {
  it("prefers an explicit poster", () => {
    const r = result({ poster: "https://a/poster.jpg", localPath: "/tmp/a.png" });
    expect(posterFor(r)).toBe("https://a/poster.jpg");
  });

  it("otherwise uses the local file rather than the expiring URL", () => {
    expect(posterFor(result({ localPath: "/tmp/a.png" }))).toBe(
      "asset:///tmp/a.png",
    );
  });
});

describe("aspectRatioOf", () => {
  it("falls back to 16:9 rather than a zero-height card", () => {
    expect(aspectRatioOf(undefined)).toBeCloseTo(16 / 9);
    expect(aspectRatioOf(result())).toBeCloseTo(16 / 9);
  });

  it("uses the media's own ratio when known", () => {
    expect(aspectRatioOf(result({ width: 1080, height: 1920 }))).toBeCloseTo(
      1080 / 1920,
    );
  });
});
