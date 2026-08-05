import type { GenSettings, ModelCapabilities } from "../types";
import { durationLadder } from "../lib/variants";
import { ChevronDownIcon, ClockIcon, CropIcon, SparkleIcon } from "./Icons";

/**
 * A pill wrapping a real <select>.
 *
 * The alternative — a pill that cycles on click — needs no popover code but
 * gives no way to see the option set or jump to a value, and screen readers
 * get a button whose label changes under them. A native select in a styled
 * shell keeps the platform behaviour and stays a 32px pill.
 */
function ChipSelect({
  id,
  label,
  value,
  options,
  icon,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  options: { value: string; label: string }[];
  icon: React.ReactNode;
  onChange: (next: string) => void;
}) {
  return (
    <div className="chip chip-select">
      <label className="sr-only" htmlFor={id}>
        {label}
      </label>
      <span className="chip-icon" aria-hidden="true">
        {icon}
      </span>
      <select
        id={id}
        className="chip-select-input"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      <ChevronDownIcon size={12} className="chip-caret" />
    </div>
  );
}

export function ChipRow({
  capabilities,
  settings,
  onChange,
}: {
  capabilities: ModelCapabilities;
  settings: GenSettings;
  onChange: (patch: Partial<GenSettings>) => void;
}) {
  // Each chip is gated on the model declaring the axis, not on the list being
  // non-empty. Those are different questions: an image model has no duration
  // at all (hide it), while most video models take a free duration (show a
  // ladder). Rendering a select with no options — which is what an
  // unconditional chip does for a model that declares nothing — is a control
  // the user can focus and cannot use.
  const durations = durationLadder(capabilities);
  return (
    <div className="chip-row">
      {capabilities.supportsDuration && durations.length > 0 ? (
        <ChipSelect
          id="chip-duration"
          label="Duration"
          icon={<ClockIcon size={14} />}
          value={String(settings.duration)}
          options={durations.map((d) => ({
            value: String(d),
            label: `${d}s`,
          }))}
          onChange={(v) => onChange({ duration: Number(v) })}
        />
      ) : null}

      {capabilities.resolutions.length > 0 ? (
        <ChipSelect
          id="chip-resolution"
          label="Resolution"
          icon={<SparkleIcon size={14} />}
          value={settings.resolution}
          options={capabilities.resolutions.map((r) => ({ value: r, label: r }))}
          onChange={(v) => onChange({ resolution: v })}
        />
      ) : null}

      {capabilities.aspects.length > 0 ? (
        <ChipSelect
          id="chip-aspect"
          label="Aspect ratio"
          icon={<CropIcon size={14} />}
          value={settings.aspect}
          options={capabilities.aspects.map((a) => ({ value: a, label: a }))}
          onChange={(v) => onChange({ aspect: v })}
        />
      ) : null}
    </div>
  );
}
