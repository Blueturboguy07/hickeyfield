import { describe, expect, it } from "vitest";
import {
  LOCAL_PROVIDER,
  PRIMARY_PROVIDER,
  PROVIDER_CATALOG,
  isAppConfigured,
  keyStateComplete,
  keyStatusLabel,
  localStatusLabel,
  validationView,
  type ValidationState,
} from "../providers";

describe("isAppConfigured", () => {
  it("is false with nothing configured", () => {
    expect(isAppConfigured([], null)).toBe(false);
  });

  it("is true as soon as one hosted provider has a key", () => {
    expect(isAppConfigured(["fal"], null)).toBe(true);
  });

  it("does not count local just because it needs no key", () => {
    // The Rust vault reports Local as configured unconditionally. Trusting that
    // makes a fresh install look ready when nothing can actually run.
    expect(isAppConfigured(["local"], { comfyui: false, ollama: false })).toBe(
      false,
    );
    expect(isAppConfigured(["local"], null)).toBe(false);
  });

  it("counts local once something is actually listening", () => {
    expect(isAppConfigured(["local"], { comfyui: true, ollama: false })).toBe(
      true,
    );
    expect(isAppConfigured(["local"], { comfyui: false, ollama: true })).toBe(
      true,
    );
  });

  it("is true when a hosted key exists even with local down", () => {
    expect(
      isAppConfigured(["local", "fal"], { comfyui: false, ollama: false }),
    ).toBe(true);
  });
});

describe("keyStatusLabel", () => {
  const single = { needsKey: true, needsSecret: false };
  const pair = { needsKey: true, needsSecret: true };

  it("distinguishes set from unset", () => {
    expect(keyStatusLabel({ ...single, hasKey: false, hasSecret: false })).toBe(
      "Not set",
    );
    expect(keyStatusLabel({ ...single, hasKey: true, hasSecret: false })).toBe(
      "Key stored",
    );
  });

  it("calls out a half-configured key/secret pair", () => {
    expect(keyStatusLabel({ ...pair, hasKey: true, hasSecret: false })).toBe(
      "Secret missing",
    );
    expect(keyStatusLabel({ ...pair, hasKey: false, hasSecret: true })).toBe(
      "Key missing",
    );
    expect(keyStatusLabel({ ...pair, hasKey: true, hasSecret: true })).toBe(
      "Key and secret stored",
    );
  });

  it("says nothing is needed for a keyless provider", () => {
    expect(
      keyStatusLabel({
        needsKey: false,
        needsSecret: false,
        hasKey: false,
        hasSecret: false,
      }),
    ).toBe("No key needed");
  });

  it("never produces a label that could contain a credential", () => {
    const labels = [
      keyStatusLabel({ ...single, hasKey: true, hasSecret: false }),
      keyStatusLabel({ ...pair, hasKey: true, hasSecret: true }),
    ];
    for (const l of labels) expect(l).not.toMatch(/[•*]{3,}|[A-Za-z0-9_-]{20,}/);
  });
});

describe("keyStateComplete", () => {
  it("requires both halves when a provider issues a pair", () => {
    expect(
      keyStateComplete({
        needsKey: true,
        needsSecret: true,
        hasKey: true,
        hasSecret: false,
      }),
    ).toBe(false);
    expect(
      keyStateComplete({
        needsKey: true,
        needsSecret: true,
        hasKey: true,
        hasSecret: true,
      }),
    ).toBe(true);
  });

  it("treats a keyless provider as complete", () => {
    expect(
      keyStateComplete({
        needsKey: false,
        needsSecret: false,
        hasKey: false,
        hasSecret: false,
      }),
    ).toBe(true);
  });
});

describe("validationView", () => {
  it("renders nothing before a test has run", () => {
    expect(validationView({ kind: "idle" })).toBeNull();
  });

  it("shows progress while the call is out", () => {
    expect(validationView({ kind: "testing" })).toEqual({
      label: "Testing…",
      tone: "pending",
    });
  });

  it("separates a working key from a rejected one", () => {
    expect(validationView({ kind: "ok", detail: "Authenticated" })).toEqual({
      label: "Authenticated",
      tone: "ok",
    });
    expect(validationView({ kind: "bad", detail: "401 Unauthorized" })).toEqual({
      label: "401 Unauthorized",
      tone: "bad",
    });
  });

  it("falls back to wording when the provider explains nothing", () => {
    expect(validationView({ kind: "ok", detail: "   " })?.label).toBe(
      "Key works",
    );
    expect(validationView({ kind: "bad", detail: "" })?.label).toBe(
      "Key rejected",
    );
  });

  it("treats an untestable key as neutral, not invalid", () => {
    // Marking a perfectly good key red because the command is missing would
    // send people re-pasting a key that already works.
    const view = validationView({ kind: "unavailable" });
    expect(view?.tone).toBe("neutral");
    expect(view?.tone).not.toBe("bad");
  });

  it("clamps a runaway provider error to one line", () => {
    const state: ValidationState = {
      kind: "bad",
      detail: `<html>${"x".repeat(500)}</html>`,
    };
    const label = validationView(state)?.label ?? "";
    expect(label.length).toBeLessThanOrEqual(120);
    expect(label.endsWith("…")).toBe(true);
  });

  it("collapses whitespace so a multi-line body cannot break the row", () => {
    expect(
      validationView({ kind: "bad", detail: "line one\n\n  line two" })?.label,
    ).toBe("line one line two");
  });
});

describe("localStatusLabel", () => {
  it("distinguishes not-yet-checked from nothing-found", () => {
    expect(localStatusLabel(null)).toBe("Not checked yet");
    expect(localStatusLabel({ comfyui: false, ollama: false })).toBe(
      "Nothing detected on this machine",
    );
  });

  it("names what it found", () => {
    expect(localStatusLabel({ comfyui: true, ollama: false })).toBe(
      "ComfyUI detected",
    );
    expect(localStatusLabel({ comfyui: true, ollama: true })).toBe(
      "ComfyUI and Ollama detected",
    );
  });
});

describe("the fallback catalogue", () => {
  it("covers every provider the Rust side ships", () => {
    // Adding a provider in Rust without one here would leave a row the setup
    // screen cannot describe or link to.
    expect(PROVIDER_CATALOG.map((p) => p.slug).sort()).toEqual(
      [
        "bfl",
        "fal",
        "google",
        "higgsfield",
        "local",
        "openai",
        "recraft",
        "vaig",
        "xai",
      ].sort(),
    );
  });

  it("gives every keyed provider somewhere to get a key and an env name", () => {
    for (const p of PROVIDER_CATALOG) {
      if (!p.needsKey) continue;
      expect(p.keyUrl, p.slug).not.toBe("");
      expect(p.envNames.length, p.slug).toBeGreaterThan(0);
      expect(p.blurb, p.slug).not.toBe("");
    }
  });

  it("marks local keyless and the primary provider keyed", () => {
    const local = PROVIDER_CATALOG.find((p) => p.slug === LOCAL_PROVIDER);
    expect(local?.needsKey).toBe(false);
    const primary = PROVIDER_CATALOG.find((p) => p.slug === PRIMARY_PROVIDER);
    expect(primary?.needsKey).toBe(true);
  });
});
