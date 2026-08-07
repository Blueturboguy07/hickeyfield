import { useCallback, useId, useState } from "react";
import { importEnv, type EnvImportResult } from "../api";
import type { ProviderInfo } from "../lib/providers";
import { UploadIcon } from "./Icons";

const PLACEHOLDER = `FAL_KEY=…
OPENAI_API_KEY=…
export GEMINI_API_KEY="…"`;

/**
 * The fast path: paste a whole .env and be done.
 *
 * Most people already have these keys in a file somewhere, so retyping eight of
 * them one field at a time is the difference between configuring the app and
 * closing it. This is the most prominent control on the setup screen for that
 * reason.
 */
export function BulkImport({
  catalog,
  onImported,
}: {
  catalog: ProviderInfo[];
  onImported: () => void;
}) {
  const areaId = useId();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<EnvImportResult | null>(null);

  const nameOf = useCallback(
    (slug: string) => catalog.find((p) => p.slug === slug)?.displayName ?? slug,
    [catalog],
  );

  const run = useCallback(async () => {
    if (text.trim() === "") return;
    setBusy(true);
    try {
      const res = await importEnv(text);
      setResult(res);
      // The pasted block is dropped on success: it is a screenful of live
      // credentials and there is no reason for it to stay in the DOM.
      if (res.imported.length > 0) setText("");
      onImported();
    } finally {
      setBusy(false);
    }
  }, [text, onImported]);

  const onFile = useCallback((files: FileList | null) => {
    const file = files?.[0];
    if (!file) return;
    void file
      .text()
      .then((contents) => {
        setText(contents);
        setResult(null);
      })
      .catch(() => {
        // A file that cannot be read (permissions, a directory) must land in
        // the visible result rather than an unhandled rejection.
        setResult({ imported: [], unknown: [`${file.name}: could not read`] });
      });
  }, []);

  return (
    <section className="setup-block setup-block-primary">
      <div className="setup-block-head">
        <h3 className="setup-heading">Paste your keys</h3>
        <p className="setup-sub">
          Drop in .env-style lines and Hickeyfield stores every provider it
          recognises. Comments, <code>export</code> prefixes and quotes are all
          fine.
        </p>
      </div>

      <label className="sr-only" htmlFor={areaId}>
        Environment variables to import
      </label>
      <textarea
        id={areaId}
        className="env-textarea"
        value={text}
        onChange={(e) => setText(e.currentTarget.value)}
        placeholder={PLACEHOLDER}
        spellCheck={false}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        rows={7}
      />

      <div className="setup-actions">
        <button
          type="button"
          className="btn btn-primary"
          disabled={busy || text.trim() === ""}
          onClick={() => void run()}
        >
          {busy ? "Importing…" : "Import keys"}
        </button>

        {/* A real file input rather than the dialog plugin: the plugin returns a
            path, and reading it would need the fs plugin, which this app does
            not ship or grant. Inside the shell WKWebView this control opens the
            same native file panel anyway. */}
        <input
          id={`${areaId}-file`}
          className="file-input sr-only"
          type="file"
          accept=".env,.txt,text/plain"
          onChange={(e) => {
            onFile(e.currentTarget.files);
            e.currentTarget.value = "";
          }}
        />
        <label className="btn btn-outline" htmlFor={`${areaId}-file`}>
          <UploadIcon size={16} />
          Choose a .env file
        </label>
      </div>

      {result ? (
        <div className="import-result" role="status" aria-live="polite">
          {result.imported.length > 0 ? (
            <p className="import-line" data-tone="ok">
              Stored {result.imported.length}{" "}
              {result.imported.length === 1 ? "key" : "keys"}:{" "}
              {result.imported.map(nameOf).join(", ")}
            </p>
          ) : (
            <p className="import-line" data-tone="bad">
              Nothing recognised in that text.
            </p>
          )}

          {result.unknown.length > 0 ? (
            <div className="import-unknown">
              <p className="import-line" data-tone="neutral">
                Not imported:
              </p>
              <ul className="import-unknown-list">
                {result.unknown.map((line, i) => (
                  <li key={`${line}-${i}`}>
                    <code>{line}</code>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
