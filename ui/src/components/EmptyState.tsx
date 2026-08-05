import type { ReactNode } from "react";

/**
 * Every empty state is two lines plus an action: a display heading that says
 * what is missing, an explanation of why, and one thing to do about it. A bare
 * "Nothing here" leaves the user unsure whether the app is broken.
 *
 * The heading renders in Bebas Neue, which has no lowercase glyphs at all, so
 * the text must already be uppercase — lowercase input silently falls back to
 * the substitute face mid-word.
 */
export function EmptyState({
  heading,
  explanation,
  action,
  tone = "default",
}: {
  heading: string;
  explanation: string;
  action?: ReactNode;
  tone?: "default" | "error";
}) {
  return (
    <div className={`empty-state empty-state-${tone}`}>
      <h2 className="empty-state-heading">{heading.toUpperCase()}</h2>
      <p className="empty-state-explanation">{explanation}</p>
      {action ? <div className="empty-state-action">{action}</div> : null}
    </div>
  );
}
