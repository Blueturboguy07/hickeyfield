import { beforeEach, describe, expect, it } from "vitest";
import {
  aspectRatioOf,
  displayUrl,
  isPlayableVideo,
  isSaved,
  nearestAspect,
  posterFor,
  previewSrc,
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

describe("previewSrc", () => {
  it("appends a start time so a paused card paints a frame", () => {
    // Without this the element decodes nothing under preload="metadata" and
    // renders as an empty box — the exact symptom of the feed showing grey
    // rectangles next to eighteen perfectly good files on disk.
    const r = result({ kind: "video", localPath: "/tmp/clip.mp4" });
    expect(previewSrc(r)).toBe("asset:///tmp/clip.mp4#t=0.1");
  });

  it("does not seek to 0, where generated clips are usually black", () => {
    expect(previewSrc(result({ localPath: "/tmp/c.mp4" }))).not.toMatch(
      /#t=0$/,
    );
  });

  it("leaves an existing fragment alone rather than making it unparseable", () => {
    const r = result({ url: "https://p.example/clip.mp4#t=2" });
    expect(previewSrc(r)).toBe("https://p.example/clip.mp4#t=2");
  });

  it("still works before the download lands", () => {
    expect(previewSrc(result({ url: "https://p.example/x.mp4" }))).toBe(
      "https://p.example/x.mp4#t=0.1",
    );
  });
});

describe("nearestAspect", () => {
  const OFFERED = ["16:9", "9:16", "1:1", "4:3", "3:4"];

  it("snaps a portrait phone video to the portrait option", () => {
    // The bug: animating a 1080x1920 clip requested 16:9 because that was the
    // untouched default, and the result came back landscape.
    expect(nearestAspect(1080 / 1920, OFFERED)).toBe("9:16");
  });

  it("snaps an ordinary phone photo to 4:3, not 1:1", () => {
    expect(nearestAspect(4032 / 3024, OFFERED)).toBe("4:3");
  });

  it("keeps a widescreen clip widescreen", () => {
    expect(nearestAspect(1920 / 1080, OFFERED)).toBe("16:9");
  });

  it("treats a portrait and its mirror as equally far from square", () => {
    // A linear comparison would put 2:1 nearer to 1:1 than 1:2 is, so a tall
    // clip would snap to square more readily than a wide one.
    expect(nearestAspect(2, ["1:1", "2:1", "1:2"])).toBe("2:1");
    expect(nearestAspect(0.5, ["1:1", "2:1", "1:2"])).toBe("1:2");
  });

  it("ignores an option that is not a ratio", () => {
    // `ratioFromAspectLabel` falls back to 16:9 for anything it cannot read,
    // which would make "auto" a perfect match for every widescreen clip.
    expect(nearestAspect(1.77, ["auto", "1:1"])).toBe("1:1");
  });

  it("returns null rather than guessing", () => {
    expect(nearestAspect(1.77, [])).toBeNull();
    expect(nearestAspect(0, OFFERED)).toBeNull();
    expect(nearestAspect(Number.NaN, OFFERED)).toBeNull();
  });
});
