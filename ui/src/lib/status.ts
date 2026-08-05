import type { JobStatus } from "../types";

export type StatusTone = "queued" | "running" | "done" | "error" | "canceled";

/**
 * Status copy.
 *
 * Deliberately free of numbers. There is no ETA and no percentage anywhere in
 * this product: provider queues give us no honest basis for either, and a
 * fabricated bar that sits at 90% for four minutes is worse than a spinner
 * that never claimed to know. Expectations are set once, in copy, by
 * QUEUE_EXPECTATION below.
 */
const LABELS: Record<string, string> = {
  waiting: "Waiting",
  queued: "In queue",
  in_progress: "Generating",
  completed: "Ready",
  failed: "Failed",
  canceled: "Canceled",
  nsfw: "Blocked by content check",
  ip_detected: "Blocked by rights check",
  // Provider-specific intermediate stages. Named where we can name them
  // honestly, generic otherwise.
  ip_detect: "Checking content",
  script: "Writing a script",
  dna: "Analyzing input",
  visuals: "Composing visuals",
  vision: "Analyzing input",
  flow: "Running the graph",
};

const TONES: Record<string, StatusTone> = {
  waiting: "queued",
  queued: "queued",
  completed: "done",
  failed: "error",
  nsfw: "error",
  ip_detected: "error",
  canceled: "canceled",
};

const TERMINAL = new Set([
  "completed",
  "failed",
  "canceled",
  "nsfw",
  "ip_detected",
]);

export const QUEUE_EXPECTATION =
  "Most generations finish in under a few minutes. Busy providers can take longer.";

export function statusLabel(status: JobStatus): string {
  return LABELS[status] ?? "Working";
}

/** Anything not terminal is activity, including stages we have never seen. */
export function statusTone(status: JobStatus): StatusTone {
  return TONES[status] ?? "running";
}

export function isTerminal(status: JobStatus): boolean {
  return TERMINAL.has(status);
}

export function isRunning(status: JobStatus): boolean {
  return !isTerminal(status);
}

export function isFailure(status: JobStatus): boolean {
  return statusTone(status) === "error";
}

/** Only a job that is still moving can be cancelled. */
export function canCancel(status: JobStatus): boolean {
  return isRunning(status);
}

/** Rerun needs settings to reuse, so it waits for the job to settle. */
export function canRerun(status: JobStatus): boolean {
  return isTerminal(status);
}
