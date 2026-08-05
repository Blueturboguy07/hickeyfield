import { useState } from "react";
import type { Gap } from "../types";
import { Overlay } from "./Overlay";

/**
 * The questions asked between Generate and submit.
 *
 * A generation model never says "you didn't tell me" — it decides, plausibly
 * and silently, and bills you for the decision. Asked for "a red door" the
 * rewriter produced a whole living room nobody mentioned. This is the one
 * chance to say what you meant before it costs anything.
 *
 * Three rules the design rests on:
 *
 * - **Skipping is free and obvious.** A prompt that changes because you
 *   declined to answer would make this something to avoid. Skip submits
 *   exactly what you typed.
 * - **Every question shows its cost.** "What is the light like?" is friction;
 *   "the model will pick a time of day for you" is a reason.
 * - **Options, not a text box.** A blank field invites the same vagueness the
 *   gap came from. Typing is still allowed, but it is not the default path.
 */
export function FollowUps({
  open,
  gaps,
  onSkip,
  onAnswer,
}: {
  open: boolean;
  gaps: Gap[];
  /** Submit as typed. */
  onSkip: () => void;
  /** Submit with `[gapId, answer]` pairs; unanswered questions are absent. */
  onAnswer: (answers: [string, string][]) => void;
}) {
  const [picked, setPicked] = useState<Record<string, string>>({});

  if (gaps.length === 0) return null;

  const answers = Object.entries(picked).filter(([, v]) => v.trim() !== "");
  const answered = answers.length;

  return (
    <Overlay
      open={open}
      onClose={onSkip}
      title="A few things this prompt does not say"
      variant="sheet"
    >
      <div className="followups">
        <p className="followups-intro">
          Answer what you care about. Anything you skip, the model decides for
          you.
        </p>

        {gaps.map((gap) => (
          <fieldset className="followup" key={gap.id}>
            <legend className="followup-q">{gap.question}</legend>
            <p className="followup-why">{gap.consequence}</p>
            <div className="followup-options">
              {gap.options.map((opt) => {
                const on = picked[gap.id] === opt;
                return (
                  <button
                    key={opt}
                    type="button"
                    className="chip chip-button"
                    aria-pressed={on}
                    onClick={() =>
                      // Clicking the chosen option again clears it, so a
                      // mis-click is not a decision you are stuck with.
                      setPicked((p) => ({ ...p, [gap.id]: on ? "" : opt }))
                    }
                  >
                    {opt}
                  </button>
                );
              })}
            </div>
          </fieldset>
        ))}

        <div className="followups-actions">
          <button type="button" className="btn btn-outline" onClick={onSkip}>
            Skip — send as written
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={answered === 0}
            onClick={() => onAnswer(answers as [string, string][])}
          >
            {answered === 0
              ? "Generate"
              : `Generate with ${answered} ${answered === 1 ? "answer" : "answers"}`}
          </button>
        </div>
      </div>
    </Overlay>
  );
}
