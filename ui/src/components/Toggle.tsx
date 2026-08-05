import type { ReactNode } from "react";

/**
 * A switch, built on a real checkbox rather than a div with aria-checked, so
 * it is reachable, toggleable with space, and announced correctly without any
 * key handling of our own. The visible track is a sibling the input drives.
 */
export function Toggle({
  id,
  checked,
  onChange,
  label,
  icon,
  disabled = false,
  hint,
}: {
  id: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  hint?: string;
}) {
  return (
    <label className="toggle" htmlFor={id} data-disabled={disabled || undefined}>
      <input
        id={id}
        type="checkbox"
        className="toggle-input"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.currentTarget.checked)}
        aria-describedby={hint ? `${id}-hint` : undefined}
      />
      <span className="toggle-track" aria-hidden="true">
        <span className="toggle-thumb" />
      </span>
      <span className="toggle-label">
        {icon}
        {label}
      </span>
      {hint ? (
        <span id={`${id}-hint`} className="sr-only">
          {hint}
        </span>
      ) : null}
    </label>
  );
}
