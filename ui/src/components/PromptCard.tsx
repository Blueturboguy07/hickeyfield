import { AtIcon, SpeakerIcon, WandIcon } from "./Icons";
import { Toggle } from "./Toggle";

const MAX_PROMPT = 4000;

/**
 * Prompt entry plus the two switches that ride with it.
 *
 * Enhance defaults on. It rewrites the prompt before submit, so which enhancer
 * ran is recorded on the job and shown in the meta rail — a rewritten prompt
 * you cannot attribute is impossible to debug when a result comes back wrong.
 */
export function PromptCard({
  value,
  onChange,
  audio,
  onAudioChange,
  audioSupported,
  enhance,
  onEnhanceChange,
  disabled = false,
  disabledReason,
}: {
  value: string;
  onChange: (next: string) => void;
  audio: boolean;
  onAudioChange: (next: boolean) => void;
  audioSupported: boolean;
  enhance: boolean;
  onEnhanceChange: (next: boolean) => void;
  disabled?: boolean;
  disabledReason?: string;
}) {
  const near = value.length > MAX_PROMPT - 200;

  return (
    <div className="prompt-card" data-disabled={disabled || undefined}>
      <div className="prompt-card-head">
        <label className="prompt-card-label" htmlFor="prompt-input">
          Prompt
        </label>
        {near ? (
          <span className="prompt-card-count">
            {value.length} / {MAX_PROMPT}
          </span>
        ) : null}
      </div>

      <textarea
        id="prompt-input"
        className="prompt-card-input"
        rows={4}
        maxLength={MAX_PROMPT}
        placeholder="Describe the scene you imagine, with details."
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.currentTarget.value)}
        aria-describedby={disabled && disabledReason ? "prompt-disabled" : undefined}
      />

      {disabled && disabledReason ? (
        <p id="prompt-disabled" className="prompt-card-note">
          {disabledReason}
        </p>
      ) : null}

      <div className="prompt-card-controls">
        <button type="button" className="chip chip-button" disabled={disabled}>
          <AtIcon size={14} />
          Elements
        </button>

        <Toggle
          id="toggle-audio"
          checked={audio && audioSupported}
          disabled={!audioSupported}
          onChange={onAudioChange}
          label="Audio"
          icon={<SpeakerIcon size={14} />}
          hint={
            audioSupported ? undefined : "This model has no audio track"
          }
        />

        <Toggle
          id="toggle-enhance"
          checked={enhance}
          onChange={onEnhanceChange}
          label="Enhance"
          icon={<WandIcon size={14} />}
          hint="Rewrites your prompt with the model's preferred phrasing before submit"
        />
      </div>
    </div>
  );
}
