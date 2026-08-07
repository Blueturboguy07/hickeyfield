/**
 * Bulk key import from .env-style text.
 *
 * Pasting a block of `FAL_KEY=…` lines is the fast path onto the app, so this
 * has to accept the shapes people actually have on disk: `export` prefixes from
 * a shell profile, quoted values, inline comments, CRLF from a Windows editor.
 *
 * Anything it cannot turn into a stored credential is reported back verbatim
 * rather than dropped. A silently ignored line is how someone ends up staring
 * at a "no key" error with the key sitting in their clipboard.
 */

import {
  PROVIDER_CATALOG,
  SECRET_ENV_NAMES,
  type ProviderInfo,
} from "./providers";

export interface EnvEntry {
  name: string;
  value: string;
}

export interface EnvParseResult {
  entries: EnvEntry[];
  /** Lines that are neither blank, a comment, nor a `NAME=value` pair. */
  unparsed: string[];
}

export interface EnvAssignment {
  provider: string;
  secretHalf: boolean;
  value: string;
}

export interface EnvImportPlan {
  assignments: EnvAssignment[];
  /** Distinct providers the assignments cover, in first-seen order. */
  providers: string[];
  /** Lines that produced no credential, verbatim, for display. */
  unknown: string[];
}

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * Only the escapes a double-quoted shell value actually defines. An API key
 * containing a backslash is vanishingly rare, but a value pasted out of a JSON
 * blob is not, and mangling one is worse than handling none.
 */
function unescapeDouble(raw: string): string {
  return raw.replace(/\\([nrt"\\])/g, (_m, ch: string) => {
    switch (ch) {
      case "n":
        return "\n";
      case "r":
        return "\r";
      case "t":
        return "\t";
      default:
        return ch;
    }
  });
}

function readValue(rest: string): string {
  const s = rest.trim();
  if (s.startsWith('"')) {
    const end = s.indexOf('"', 1);
    if (end > 0) return unescapeDouble(s.slice(1, end));
  }
  if (s.startsWith("'")) {
    const end = s.indexOf("'", 1);
    if (end > 0) return s.slice(1, end);
  }
  // Unquoted: a `#` only starts a comment when whitespace precedes it, so a key
  // that legitimately contains `#` survives.
  return s.replace(/\s+#.*$/, "").trim();
}

export function parseEnvText(text: string): EnvParseResult {
  const entries: EnvEntry[] = [];
  const unparsed: string[] = [];

  for (const rawLine of text.split(/\r\n|\r|\n/)) {
    const line = rawLine.trim();
    if (line === "" || line.startsWith("#")) continue;

    const body = line.replace(/^export\s+/, "");
    const eq = body.indexOf("=");
    if (eq <= 0) {
      unparsed.push(line);
      continue;
    }

    const name = body.slice(0, eq).trim();
    if (!NAME_RE.test(name)) {
      unparsed.push(line);
      continue;
    }

    entries.push({ name, value: readValue(body.slice(eq + 1)) });
  }

  return { entries, unparsed };
}

interface Target {
  provider: string;
  secretHalf: boolean;
}

/**
 * Env name to credential slot. Built per call because the catalogue can come
 * from `provider_info()` at runtime, which may know names this file does not.
 */
function buildLookup(catalog: ProviderInfo[]): Map<string, Target> {
  const map = new Map<string, Target>();
  const put = (name: string, target: Target) => {
    const key = name.trim().toUpperCase();
    // First definition wins, so a provider cannot hijack another's env name by
    // ordering alone.
    if (key && !map.has(key)) map.set(key, target);
  };

  for (const p of catalog) {
    if (!p.needsKey && !p.needsSecret) continue;
    for (const name of p.envNames ?? []) put(name, { provider: p.slug, secretHalf: false });
    for (const name of SECRET_ENV_NAMES[p.slug] ?? [])
      put(name, { provider: p.slug, secretHalf: true });

    // The app's own dev overrides, mirroring vault.rs `hickeyfield_{SLUG}_{HALF}`.
    const slug = p.slug.toUpperCase().replace(/-/g, "_");
    put(`hickeyfield_${slug}_KEY`, { provider: p.slug, secretHalf: false });
    if (p.needsSecret)
      put(`hickeyfield_${slug}_SECRET`, { provider: p.slug, secretHalf: true });
  }

  return map;
}

/**
 * Turn pasted text into the set of `set_key` calls it implies.
 *
 * A recognised name with a blank value is reported as not-imported rather than
 * applied: an empty value clears a credential on the Rust side, and a template
 * `.env` full of `FAL_KEY=` lines would then wipe a working install.
 */
export function planEnvImport(
  text: string,
  catalog: ProviderInfo[] = PROVIDER_CATALOG,
): EnvImportPlan {
  const lookup = buildLookup(catalog);
  const { entries, unparsed } = parseEnvText(text);
  const unknown = [...unparsed];

  // Keyed by slot so a repeated name resolves to the last occurrence, which is
  // what every .env loader does and what someone appending a new key expects.
  const bySlot = new Map<string, EnvAssignment>();

  for (const entry of entries) {
    const target = lookup.get(entry.name.toUpperCase());
    if (!target) {
      unknown.push(`${entry.name}=…`);
      continue;
    }
    if (entry.value === "") {
      unknown.push(`${entry.name}= (no value)`);
      continue;
    }
    bySlot.set(`${target.provider}:${target.secretHalf}`, {
      provider: target.provider,
      secretHalf: target.secretHalf,
      value: entry.value,
    });
  }

  const assignments = [...bySlot.values()];
  const providers: string[] = [];
  for (const a of assignments) {
    if (!providers.includes(a.provider)) providers.push(a.provider);
  }

  return { assignments, providers, unknown };
}
