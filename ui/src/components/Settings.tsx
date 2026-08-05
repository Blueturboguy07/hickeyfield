import { useMemo, useState } from "react";
import type { KeyState } from "../api";
import {
  PRIMARY_PROVIDER,
  localStatusLabel,
  type LocalEndpoints,
  type ProviderInfo,
} from "../lib/providers";
import { Overlay } from "./Overlay";
import { BulkImport } from "./BulkImport";
import { ProviderRow } from "./ProviderRow";

/**
 * Key management, always reachable from the titlebar.
 *
 * Same rows as onboarding but in management mode: what is stored, whether it
 * actually works, and how to get rid of it. Nothing here can display a key —
 * the bridge only ever hands back booleans, and this screen keeps it that way.
 */
export function Settings({
  open,
  onClose,
  catalog,
  states,
  local,
  libraryPath,
  onChanged,
  onRecheckLocal,
  onRunOnboarding,
}: {
  open: boolean;
  onClose: () => void;
  catalog: ProviderInfo[];
  states: KeyState[];
  local: LocalEndpoints | null;
  libraryPath: string | null;
  onChanged: () => void;
  onRecheckLocal: () => Promise<void> | void;
  onRunOnboarding: () => void;
}) {
  const [rechecking, setRechecking] = useState(false);

  const providers = useMemo(() => {
    const keyed = catalog.filter((p) => p.needsKey || p.needsSecret);
    // The recommended provider stays pinned to the top here too, so the row
    // people most often need is in the same place on both screens.
    return [
      ...keyed.filter((p) => p.slug === PRIMARY_PROVIDER),
      ...keyed.filter((p) => p.slug !== PRIMARY_PROVIDER),
    ];
  }, [catalog]);

  const stateFor = (slug: string) =>
    states.find((s) => s.provider === slug) ?? null;

  const recheck = async () => {
    setRechecking(true);
    try {
      await onRecheckLocal();
    } finally {
      setRechecking(false);
    }
  };

  return (
    <Overlay open={open} onClose={onClose} title="Settings" variant="full">
      <div className="setup">
        <section className="setup-block">
          <div className="setup-block-head">
            <h3 className="setup-heading">Provider keys</h3>
            <p className="setup-sub">
              Stored in the OS keychain. Halation can tell you whether a key is
              present and whether it works — it cannot show you the key itself,
              and neither can anything else in the app.
            </p>
          </div>

          <ul className="provider-list">
            {providers.map((info) => (
              <ProviderRow
                key={info.slug}
                info={info}
                state={stateFor(info.slug)}
                recommended={info.slug === PRIMARY_PROVIDER}
                manage
                onChanged={onChanged}
              />
            ))}
          </ul>
        </section>

        <BulkImport catalog={catalog} onImported={onChanged} />

        <section className="setup-block">
          <div className="setup-block-head">
            <h3 className="setup-heading">Local endpoints</h3>
            <p className="setup-sub">
              Detected on this machine. Free, keyless, and never leaves the
              computer.
            </p>
          </div>
          <div
            className="local-status"
            data-up={Boolean(local?.comfyui || local?.ollama) || undefined}
          >
            <span className="local-status-dot" aria-hidden="true" />
            <span>{localStatusLabel(local)}</span>
            <button
              type="button"
              className="btn btn-sm btn-outline"
              disabled={rechecking}
              onClick={() => void recheck()}
            >
              {rechecking ? "Checking…" : "Re-check"}
            </button>
          </div>
        </section>

        <section className="setup-block">
          <div className="setup-block-head">
            <h3 className="setup-heading">Library</h3>
            <p className="setup-sub">Where generated assets are written.</p>
          </div>
          <p className="path-display">
            <code>{libraryPath ?? "Unavailable outside the desktop app"}</code>
          </p>
        </section>

        <div className="setup-footer">
          <button
            type="button"
            className="btn btn-outline"
            onClick={onRunOnboarding}
          >
            Run first-run setup again
          </button>
          <button type="button" className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </Overlay>
  );
}
