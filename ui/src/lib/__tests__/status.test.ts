import { describe, expect, it } from "vitest";
import {
  canCancel,
  canRerun,
  isFailure,
  isRunning,
  isTerminal,
  QUEUE_EXPECTATION,
  statusLabel,
  statusTone,
} from "../status";

describe("statusLabel", () => {
  it("maps the provider status machine to copy", () => {
    expect(statusLabel("waiting")).toBe("Waiting");
    expect(statusLabel("queued")).toBe("In queue");
    expect(statusLabel("in_progress")).toBe("Generating");
    expect(statusLabel("completed")).toBe("Ready");
    expect(statusLabel("failed")).toBe("Failed");
    expect(statusLabel("canceled")).toBe("Canceled");
  });

  it("names intermediate provider stages", () => {
    expect(statusLabel("script")).toBe("Writing a script");
    expect(statusLabel("ip_detect")).toBe("Checking content");
  });

  it("falls back to generic activity for an unseen stage", () => {
    expect(statusLabel("some_new_provider_stage")).toBe("Working");
  });

  it("never reports a number", () => {
    const labels = [
      "waiting",
      "queued",
      "in_progress",
      "completed",
      "failed",
      "canceled",
      "nsfw",
      "mystery",
    ].map(statusLabel);
    for (const label of labels) expect(label).not.toMatch(/\d/);
    expect(QUEUE_EXPECTATION).not.toMatch(/%/);
  });
});

describe("statusTone", () => {
  it("classifies the known states", () => {
    expect(statusTone("queued")).toBe("queued");
    expect(statusTone("completed")).toBe("done");
    expect(statusTone("failed")).toBe("error");
    expect(statusTone("nsfw")).toBe("error");
    expect(statusTone("canceled")).toBe("canceled");
  });

  it("treats anything unrecognized as still running", () => {
    expect(statusTone("visuals")).toBe("running");
  });
});

describe("lifecycle predicates", () => {
  it("agrees on what is terminal", () => {
    expect(isTerminal("completed")).toBe(true);
    expect(isTerminal("canceled")).toBe(true);
    expect(isTerminal("in_progress")).toBe(false);
    expect(isRunning("in_progress")).toBe(true);
    expect(isRunning("completed")).toBe(false);
  });

  it("only allows cancel while moving and rerun once settled", () => {
    expect(canCancel("queued")).toBe(true);
    expect(canCancel("completed")).toBe(false);
    expect(canRerun("completed")).toBe(true);
    expect(canRerun("in_progress")).toBe(false);
  });

  it("flags content blocks as failures", () => {
    expect(isFailure("nsfw")).toBe(true);
    expect(isFailure("canceled")).toBe(false);
  });
});
