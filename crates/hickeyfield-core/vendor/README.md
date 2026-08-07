# Vendored specifications

## `higgsfield-cli-MODELS.md`

Source: <https://github.com/higgsfield-ai/cli> — **MIT licensed** (see
`LICENSES/MIT-higgsfield-cli.txt` at the repo root).

985 lines enumerating 55 models across Image, Video, 3D and Audio with every
flag, default, enum value, cardinality and constraint. This is a
machine-readable specification of what each model accepts, published by
Higgsfield under a permissive licence, and it is parsed by `catalog.rs` into
typed `ModelSpec`s.

It seeds the catalogue; it does not define our routing. Which provider actually
serves a given model, and what it costs, come from our own route table and the
runtime price feeds — not from this file.

### Refreshing

```sh
curl -sL https://raw.githubusercontent.com/higgsfield-ai/cli/main/MODELS.md \
  -o crates/hickeyfield-core/vendor/higgsfield-cli-MODELS.md
cargo test -p hickeyfield-core catalog
```

The parser tests assert on real counts, so a refresh that changes the roster
will fail loudly rather than silently drifting. That is deliberate — update the
expected counts in the same commit and you have a readable record of what
Higgsfield added or dropped.
