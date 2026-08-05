#!/usr/bin/env python3
"""Build the provenance index used by lint-provenance.py.

We need to detect whether a string we ship is verbatim Higgsfield copy. The
obvious way — vendor their 24,835-string i18n bundle and diff against it — is
exactly the thing we must not do: that bundle is the riskiest artifact in the
whole research corpus, and committing it would put their copyrighted product
copy in our repository permanently.

So we commit *hashes*, never text. This script reads the reference bundle from
wherever it happens to live on a developer's machine, reduces every string to a
set of normalized word-shingles, and writes out truncated SHA-256 digests. The
index is one-way: it can answer "is this phrase in the corpus?" but it cannot
reconstruct a single sentence of theirs.

Run once, commit the output, and the corpus itself never enters the repo.

Usage:
    scripts/build-provenance-index.py ~/higgsfield-research/en.json
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path

# A 5-word window is long enough that ordinary English overlap ("click here to
# get started") does not trip it, and short enough to catch a lifted sentence
# that has been lightly edited.
SHINGLE = 5

# Strings at or below this length are functional UI labels — "Generate",
# "Aspect ratio", "Enhance prompt". Those are unavoidable and uncopyrightable,
# and indexing them would produce nothing but false positives.
MIN_WORDS = 8

_WORD = re.compile(r"[a-z0-9]+")


def normalize(text: str) -> list[str]:
    """Lowercase, drop punctuation and markup, collapse to bare words.

    Normalizing this hard is deliberate: it means reordered punctuation or a
    swapped bit of formatting will not let lifted copy slip past.
    """
    text = re.sub(r"<[^>]+>", " ", text)
    text = re.sub(r"\{\{?[^}]*\}?\}", " ", text)  # {name}, {{count}} placeholders
    return _WORD.findall(text.lower())


# 48 bits. With ~80k digests the birthday collision probability is ~1e-5, and a
# collision costs at most one spurious shingle match — the lint requires several
# before it flags anything.
DIGEST_HEX = 12


def shingles(words: list[str], n: int = SHINGLE) -> set[str]:
    if len(words) < n:
        return set()
    return {
        hashlib.sha256(" ".join(words[i : i + n]).encode()).hexdigest()[:DIGEST_HEX]
        for i in range(len(words) - n + 1)
    }


def walk_strings(node) -> list[str]:
    """The bundle nests strings inside single-element lists and dicts."""
    out: list[str] = []
    if isinstance(node, str):
        out.append(node)
    elif isinstance(node, list):
        for v in node:
            out.extend(walk_strings(v))
    elif isinstance(node, dict):
        for v in node.values():
            out.extend(walk_strings(v))
    return out


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2

    src = Path(sys.argv[1]).expanduser()
    if not src.exists():
        print(f"reference bundle not found: {src}", file=sys.stderr)
        return 1

    strings = walk_strings(json.loads(src.read_text()))
    indexed = 0
    digests: set[str] = set()

    for s in strings:
        words = normalize(s)
        if len(words) < MIN_WORDS:
            continue
        sh = shingles(words)
        if sh:
            digests |= sh
            indexed += 1

    out = Path(__file__).resolve().parent.parent / "provenance" / "i18n-shingles.txt"
    out.parent.mkdir(exist_ok=True)
    header = [
        "# One-way SHA-256 shingle digests of third-party reference copy, used",
        "# to detect verbatim reuse. Contains no text and cannot be reversed.",
        "# Regenerate with scripts/build-provenance-index.py.",
        f"# shingle_words={SHINGLE} min_words={MIN_WORDS} digest_hex={DIGEST_HEX}",
        f"# source_strings={len(strings)} indexed_strings={indexed}",
    ]
    out.write_text("\n".join(header + sorted(digests)) + "\n")

    print(f"{len(strings)} strings read, {indexed} long enough to index")
    print(f"{len(digests)} shingle digests -> {out.relative_to(out.parent.parent)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
