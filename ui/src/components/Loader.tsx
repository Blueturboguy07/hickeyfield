import { GRAIN_DATA_URI } from "../lib/placeholder";

/**
 * The 2x2 blinking dot grid.
 *
 * The four delays are 0s / .3s / .9s / .6s — the third dot is .9s, not .6s, so
 * the pulse travels diagonally rather than clockwise. It reads as a subtle
 * shimmer instead of a spinner, which is the point: the motion carries no
 * implication of progress, because we have none to report.
 */
export function DotLoader({
  size = "md",
  label,
}: {
  size?: "sm" | "md" | "lg";
  label?: string;
}) {
  return (
    <span
      className={`dot-loader dot-loader-${size}`}
      role="status"
      aria-live="polite"
    >
      <span className="dot-loader-dots" aria-hidden="true">
        <span className="dot-loader-dot" />
        <span className="dot-loader-dot" />
        <span className="dot-loader-dot" />
        <span className="dot-loader-dot" />
      </span>
      {label ? <span className="sr-only">{label}</span> : null}
    </span>
  );
}

/**
 * The rainbow + animated-grain placeholder shown while a clip does not exist
 * yet. Four fixed hues on a 400% gradient with a fractal-noise layer over the
 * top in multiply. The palette is intentionally unbranded, so it stays honest
 * about being a placeholder rather than pretending to be a frame of the
 * result.
 */
export function RainbowPlaceholder({
  className = "",
  ratio,
}: {
  className?: string;
  ratio?: number;
}) {
  return (
    <div
      className={`rainbow-placeholder ${className}`.trim()}
      style={ratio ? { aspectRatio: String(ratio) } : undefined}
      aria-hidden="true"
    >
      <span className="rainbow-placeholder-grain" style={{ backgroundImage: `url("${GRAIN_DATA_URI}")` }} />
    </div>
  );
}

/** Indeterminate status line: text plus dots, never a number. */
export function StatusLine({ label }: { label: string }) {
  return (
    <span className="status-line">
      <DotLoader size="sm" />
      <span className="status-line-text">{label}</span>
    </span>
  );
}
