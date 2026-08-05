import { useCallback, useId, useState } from "react";
import { openExternal, setKey, validateKey, type KeyState } from "../api";
import {
  keyStateComplete,
  keyStatusLabel,
  validationView,
  type ProviderInfo,
  type ValidationState,
} from "../lib/providers";
import { CheckIcon, ExternalIcon, EyeIcon, EyeOffIcon, TrashIcon } from "./Icons";

/**
 * A password field with a reveal toggle.
 *
 * Held in component state and never lifted: the value exists only long enough
 * to reach `set_key`, and nothing reads it back afterwards. The bridge returns
 * booleans only, so there is no path by which a stored key could be rendered.
 */
function SecretInput({
  id,
  label,
  placeholder,
  value,
  onChange,
  onCommit,
}: {
  id: string;
  label: string;
  placeholder: string;
  value: string;
  onChange: (next: string) => void;
  onCommit: () => void;
}) {
  const [shown, setShown] = useState(false);

  return (
    <div className="secret-field">
      <label className="sr-only" htmlFor={id}>
        {label}
      </label>
      <input
        id={id}
        className="secret-input"
        type={shown ? "text" : "password"}
        value={value}
        placeholder={placeholder}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        onChange={(e) => onChange(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            onCommit();
          }
        }}
      />
      <button
        type="button"
        className="btn btn-icon btn-ghost secret-reveal"
        aria-label={shown ? `Hide ${label}` : `Show ${label}`}
        aria-pressed={shown}
        onClick={() => setShown((s) => !s)}
      >
        {shown ? <EyeOffIcon size={16} /> : <EyeIcon size={16} />}
      </button>
    </div>
  );
}

/**
 * A bridge rejection carried up as text.
 *
 * Errors from `invoke` are plain strings from Rust rather than Error instances,
 * and nothing here is logged: a thrown value from a call that was handed a key
 * is exactly the sort of thing that should not reach the console.
 */
function describe(e: unknown, fallback: string): string {
  const raw = typeof e === "string" ? e : e instanceof Error ? e.message : "";
  return raw ? `${fallback}: ${raw}` : fallback;
}

export function ProviderRow({
  info,
  state,
  recommended = false,
  manage = false,
  onChanged,
}: {
  info: ProviderInfo;
  /** Presence booleans from `key_states`. Null while the first load is in flight. */
  state: KeyState | null;
  recommended?: boolean;
  /** Settings mode: adds Test and Remove alongside the input. */
  manage?: boolean;
  onChanged: () => void;
}) {
  const baseId = useId();
  const [keyDraft, setKeyDraft] = useState("");
  const [secretDraft, setSecretDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  /** A keychain write that failed. Distinct from a key the provider rejected. */
  const [storeError, setStoreError] = useState<string | null>(null);
  const [validation, setValidation] = useState<ValidationState>({
    kind: "idle",
  });

  const presence = state ?? {
    provider: info.slug,
    hasKey: false,
    hasSecret: false,
    needsKey: info.needsKey,
    needsSecret: info.needsSecret,
  };
  const complete = keyStateComplete(presence);
  const view = validationView(validation);

  const save = useCallback(async () => {
    const key = keyDraft.trim();
    const secret = secretDraft.trim();
    if (!key && !secret) return;
    setBusy(true);
    setStoreError(null);
    try {
      if (key) await setKey(info.slug, key, false);
      if (secret) await setKey(info.slug, secret, true);
      // Drafts are cleared on success so a key never lingers in the DOM after
      // it has been handed to the keychain. On failure they are kept, or the
      // user loses a key they just pasted.
      setKeyDraft("");
      setSecretDraft("");
      setValidation({ kind: "idle" });
      onChanged();
    } catch (e) {
      // A keychain write really can be refused — a locked login keychain, a
      // denied prompt. Swallowing that would leave the row saying "Not set"
      // with no explanation.
      setStoreError(describe(e, "Could not store the key"));
    } finally {
      setBusy(false);
    }
  }, [info.slug, keyDraft, secretDraft, onChanged]);

  const test = useCallback(async () => {
    setValidation({ kind: "testing" });
    const res = await validateKey(info.slug);
    if (!res) {
      setValidation({ kind: "unavailable" });
      return;
    }
    setValidation({
      kind: res.ok ? "ok" : "bad",
      detail: res.detail,
    });
  }, [info.slug]);

  const remove = useCallback(async () => {
    setBusy(true);
    setStoreError(null);
    try {
      // An empty value clears, and the secret half is cleared too so a partial
      // credential cannot survive a "Remove".
      await setKey(info.slug, "", false);
      if (info.needsSecret) await setKey(info.slug, "", true);
      setValidation({ kind: "idle" });
      setConfirmRemove(false);
      onChanged();
    } catch (e) {
      setStoreError(describe(e, "Could not remove the key"));
    } finally {
      setBusy(false);
    }
  }, [info.slug, info.needsSecret, onChanged]);

  const dirty = keyDraft.trim() !== "" || secretDraft.trim() !== "";

  return (
    <li className="provider-row" data-recommended={recommended || undefined}>
      <div className="provider-row-head">
        <span className="provider-row-name">
          {info.displayName}
          {recommended ? <span className="badge">START HERE</span> : null}
        </span>
        <span className="provider-row-status" data-complete={complete || undefined}>
          {complete ? <CheckIcon size={14} /> : null}
          {keyStatusLabel(presence)}
        </span>
      </div>

      {info.blurb ? <p className="provider-row-blurb">{info.blurb}</p> : null}

      <div className="provider-row-fields">
        {info.needsKey ? (
          <SecretInput
            id={`${baseId}-key`}
            label={`${info.displayName} API key`}
            placeholder={
              presence.hasKey ? "Replace stored key" : "Paste API key"
            }
            value={keyDraft}
            onChange={setKeyDraft}
            onCommit={() => void save()}
          />
        ) : null}
        {info.needsSecret ? (
          <SecretInput
            id={`${baseId}-secret`}
            label={`${info.displayName} API secret`}
            placeholder={
              presence.hasSecret ? "Replace stored secret" : "Paste API secret"
            }
            value={secretDraft}
            onChange={setSecretDraft}
            onCommit={() => void save()}
          />
        ) : null}
        <button
          type="button"
          className="btn btn-sm btn-primary"
          disabled={!dirty || busy}
          onClick={() => void save()}
        >
          Save
        </button>
      </div>

      <div className="provider-row-actions">
        {info.keyUrl ? (
          <button
            type="button"
            className="link-button"
            onClick={() => void openExternal(info.keyUrl)}
          >
            Get a key
            <ExternalIcon size={13} />
          </button>
        ) : null}

        {info.envNames.length > 0 ? (
          <code className="provider-row-env">{info.envNames[0]}</code>
        ) : null}

        {manage ? (
          <>
            <button
              type="button"
              className="btn btn-sm btn-outline"
              disabled={!complete || validation.kind === "testing"}
              onClick={() => void test()}
            >
              Test
            </button>
            {/* An inline two-step confirm rather than a dialog: this row already
                lives inside a modal, and stacking one on top traps focus. */}
            {complete ? (
              confirmRemove ? (
                <span className="confirm-inline">
                  <span className="confirm-inline-text">Remove key?</span>
                  <button
                    type="button"
                    className="btn btn-sm btn-danger"
                    disabled={busy}
                    onClick={() => void remove()}
                  >
                    Confirm
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => setConfirmRemove(false)}
                  >
                    Cancel
                  </button>
                </span>
              ) : (
                <button
                  type="button"
                  className="btn btn-sm btn-danger"
                  onClick={() => setConfirmRemove(true)}
                >
                  <TrashIcon size={14} />
                  Remove
                </button>
              )
            ) : null}
          </>
        ) : null}

        {storeError ? (
          <span className="validation-note" data-tone="bad" role="alert">
            {storeError}
          </span>
        ) : null}

        {view ? (
          <span
            className="validation-note"
            data-tone={view.tone}
            role="status"
            aria-live="polite"
          >
            {view.label}
          </span>
        ) : null}
      </div>
    </li>
  );
}
