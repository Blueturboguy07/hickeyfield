import { describe, expect, it } from "vitest";
import { cameraClass } from "./CameraPreview";

/**
 * The camera slugs the Rust side ships. Kept as a literal rather than imported
 * so that adding a move on one side without the other fails here loudly.
 */
const SLUGS = [
  "aerial-pullback",
  "drone-orbit",
  "handheld",
  "bullet-time",
  "tracking-shot",
  "pan-right",
  "rack-focus",
  "push-in",
  "crane-up",
  "static-shot",
  "360-orbit",
  "tilt-up",
  "zoom-in",
  "dolly-in",
  "dolly-zoom",
  "pov-walk",
  "pan-left",
  "tilt-down",
  "crane-down",
  "zoom-out",
  "dolly-out",
  "push-out",
  "dolly-zoom-in",
  "arc-left",
  "arc-right",
];

describe("camera move previews", () => {
  it("maps every shipped camera move", () => {
    const unmapped = SLUGS.filter((s) => cameraClass(s) === "cam-push-in" && s !== "push-in" && s !== "dolly-in");
    expect(unmapped).toEqual([]);
  });

  it("falls back rather than throwing on an unknown slug", () => {
    // A preset added on the Rust side before the CSS lands should still render.
    expect(cameraClass("some-future-move")).toBe("cam-push-in");
  });

  it("gives opposite moves different animations", () => {
    // The bug this catches: a copy-paste in the table pointing pan-left at the
    // pan-right keyframes, which looks plausible and is exactly backwards.
    const pairs: [string, string][] = [
      ["pan-left", "pan-right"],
      ["tilt-up", "tilt-down"],
      ["crane-up", "crane-down"],
      ["zoom-in", "zoom-out"],
      ["arc-left", "arc-right"],
      ["dolly-zoom", "dolly-zoom-in"],
    ];
    for (const [a, b] of pairs) {
      expect(cameraClass(a), `${a} vs ${b}`).not.toBe(cameraClass(b));
    }
  });

  it("keeps a locked-off shot locked off", () => {
    // A static shot that drifted would misrepresent what the preset does.
    expect(cameraClass("static-shot")).toBe("cam-static");
  });

  it("treats a dolly and a zoom as different moves", () => {
    // They look similar and are optically distinct: a dolly has parallax.
    expect(cameraClass("dolly-in")).not.toBe(cameraClass("zoom-in"));
  });
});
