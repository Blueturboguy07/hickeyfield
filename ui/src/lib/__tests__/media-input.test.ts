import { describe, expect, it } from "vitest";
import {
  acceptFor,
  basename,
  mergeMedia,
  previewOf,
  sourceKey,
} from "../media-input";
import type { MediaRef } from "../../types";

const local = (role: MediaRef["role"], path: string): MediaRef => ({
  role,
  source: { kind: "local", path },
  name: basename(path),
});

const url = (role: MediaRef["role"], u: string): MediaRef => ({
  role,
  source: { kind: "url", url: u },
});

describe("accept filters", () => {
  it("does not offer a video file for a still slot", () => {
    // Offering the wrong type lets someone pick a .mov for a start frame and
    // discover the mistake only when the provider rejects it.
    expect(acceptFor("start")).toBe("image/*");
    expect(acceptFor("end")).toBe("image/*");
    expect(acceptFor("reference")).toBe("image/*");
  });

  it("offers audio for the audio roles and video for the video roles", () => {
    expect(acceptFor("audio")).toBe("audio/*");
    expect(acceptFor("audio_reference")).toBe("audio/*");
    expect(acceptFor("video")).toBe("video/*");
    expect(acceptFor("video_reference")).toBe("video/*");
  });
});

describe("basename", () => {
  it("handles both path separators", () => {
    // The Windows build hands back backslashes; splitting on '/' alone shows
    // the whole path in the slot.
    expect(basename("/Users/x/a.png")).toBe("a.png");
    expect(basename("C:\\Users\\x\\a.png")).toBe("a.png");
    expect(basename("bare.png")).toBe("bare.png");
  });
});

describe("previewOf", () => {
  it("prefers the object URL when there is one", () => {
    const m: MediaRef = { ...local("start", "/tmp/a.png"), preview: "blob:xyz" };
    expect(previewOf(m)).toBe("blob:xyz");
  });

  it("returns nothing for a bare local path", () => {
    // The webview cannot load a filesystem path, so the caller must fall back
    // to the name chip instead of rendering a broken image.
    expect(previewOf(local("start", "/tmp/a.png"))).toBeUndefined();
  });

  it("uses a remote URL directly", () => {
    expect(previewOf(url("reference", "https://cdn/a.png"))).toBe(
      "https://cdn/a.png",
    );
  });
});

describe("sourceKey", () => {
  it("identifies an attachment by its source, not its preview", () => {
    // Two picks of the same file get different object URLs. Keying on the
    // preview would let the same file in twice.
    const a: MediaRef = { ...local("reference", "/tmp/a.png"), preview: "blob:1" };
    const b: MediaRef = { ...local("reference", "/tmp/a.png"), preview: "blob:2" };
    expect(sourceKey(a)).toBe(sourceKey(b));
  });

  it("separates different files", () => {
    expect(sourceKey(local("reference", "/tmp/a.png"))).not.toBe(
      sourceKey(local("reference", "/tmp/b.png")),
    );
  });

  it("does not carry a whole data URI as the key", () => {
    const big: MediaRef = {
      role: "start",
      source: { kind: "data_uri", data: `data:image/png;base64,${"A".repeat(5000)}` },
    };
    expect(sourceKey(big).length).toBeLessThan(120);
  });
});

describe("mergeMedia", () => {
  it("replaces rather than appends for a single-slot role", () => {
    // The bug this prevents: two start frames, of which the binder can only
    // send one — so the second pick appears to do nothing.
    const first = local("start", "/tmp/a.png");
    const second = local("start", "/tmp/b.png");
    const out = mergeMedia([first], [second], false);
    expect(out).toHaveLength(1);
    expect(out[0]).toBe(second);
  });

  it("appends for a repeatable role", () => {
    const out = mergeMedia(
      [local("reference", "/tmp/a.png")],
      [local("reference", "/tmp/b.png")],
      true,
    );
    expect(out).toHaveLength(2);
  });

  it("does not add the same file twice", () => {
    const out = mergeMedia(
      [local("reference", "/tmp/a.png")],
      [local("reference", "/tmp/a.png")],
      true,
    );
    expect(out).toHaveLength(1);
  });

  it("leaves other roles untouched when replacing one", () => {
    // Clearing the whole list on a start-frame swap would silently drop the
    // user's end frame.
    const start = local("start", "/tmp/a.png");
    const end = local("end", "/tmp/z.png");
    const out = mergeMedia([start, end], [local("start", "/tmp/b.png")], false);
    expect(out.filter((m) => m.role === "end")).toEqual([end]);
    expect(out).toHaveLength(2);
  });

  it("is a no-op when nothing was picked", () => {
    // Cancelling the dialog must not clear an existing attachment.
    const existing = [local("start", "/tmp/a.png")];
    expect(mergeMedia(existing, [], false)).toBe(existing);
  });
});
