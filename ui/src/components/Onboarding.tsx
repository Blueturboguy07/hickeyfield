import { useMemo } from "react";
import type { KeyState } from "../api";
import {
  LOCAL_PROVIDER,
  PRIMARY_PROVIDER,
  localStatusLabel,
  type LocalEndpoints,
  type ProviderInfo,
} from "../lib/providers";
import { Overlay } from "./Overlay";
import { BulkImport } from "./BulkImport";
import { ProviderRow } from "./ProviderRow";
import { KeyIcon } from "./Icons";

/**
 * First run.
 *
 * Deliberately one screen rather than a wizard: the only thing standing between
 * a new install and a generation is a key, so the screen is the key entry and
 * nothing else. The bulk paste comes first because most people already have
 * these in a file and typing eight of them by hand is where they give up.
 */
export function Onboarding({
  open,
  onClose,
  onSkip,
  catalog,
  states,
  local,
  onChanged,
}: {
  open: boolean;
  onClose: () => void;
  onSkip: () => void;
  catalog: ProviderInfo[];
  states: KeyState[];
  local: LocalEndpoints | null;
  onChanged: () => void;
}) {
  // Split on whether a credential is needed rather than on the slug, so a
  // future keyless provider lands in the free-tier section automatically
  // instead of getting a key field it can do nothing with.
  const { primary, others, localInfo } = useMemo(() => {
    const keyed = catalog.filter((p) => p.needsKey || p.needsSecret);
    return {
      primary: keyed.find((p) => p.slug === PRIMARY_PROVIDER) ?? null,
      others: keyed.filter((p) => p.slug !== PRIMARY_PROVIDER),
      localInfo: catalog.find((p) => p.slug === LOCAL_PROVIDER) ?? null,
    };
  }, [catalog]);

  const stateFor = (slug: string) =>
    states.find((s) => s.provider === slug) ?? null;

  return (
    <Overlay open={open} onClose={onClose} title="Set up Hickeyfield" variant="full">
      <div className="setup">
        <section className="setup-block setup-intro">
          <KeyIcon size={20} className="setup-intro-icon" />
          <p className="setup-intro-text">
            Hickeyfield runs on your own provider keys. There is no Hickeyfield
            account, no proxy and no server of ours in the path — every request
            goes straight from this app to the provider you are paying. Keys are
            stored in the operating system keychain, the app reads them only when
            it assembles a request, and they are never sent anywhere else.
          </p>
        </section>

        <BulkImport catalog={catalog} onImported={onChanged} />

        <section className="setup-block">
          <div className="setup-block-head">
            <h3 className="setup-heading">Or add them one at a time</h3>
            <p className="setup-sub">
              You only need one. {primary?.displayName ?? "fal.ai"} covers most
              of the model roster — everything below it is optional and adds
              routes.
            </p>
          </div>

          <ul className="provider-list">
            {primary ? (
              <ProviderRow
                key={primary.slug}
                info={primary}
                state={stateFor(primary.slug)}
                recommended
                onChanged={onChanged}
              />
            ) : null}
            {others.map((info) => (
              <ProviderRow
                key={info.slug}
                info={info}
                state={stateFor(info.slug)}
                onChanged={onChanged}
              />
            ))}
          </ul>
        </section>

        {localInfo ? (
          <section className="setup-block">
            <div className="setup-block-head">
              <h3 className="setup-heading">Local — the free tier</h3>
              <p className="setup-sub">{localInfo.blurb}</p>
            </div>
            <div className="local-status" data-up={Boolean(local?.comfyui || local?.ollama) || undefined}>
              <span className="local-status-dot" aria-hidden="true" />
              <span>{localStatusLabel(local)}</span>
              <span className="local-status-note">
                No key required. Start ComfyUI or Ollama and re-check from
                Settings.
              </span>
            </div>
          </section>
        ) : null}

        <div className="setup-footer">
          <button type="button" className="btn btn-ghost" onClick={onSkip}>
            Skip for now
          </button>
          <button type="button" className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </Overlay>
  );
}
