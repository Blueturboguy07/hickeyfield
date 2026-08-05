# Bridging plan — OSS landscape, measured gaps, ranked work

**2026-08-04.** Produced by a 14-agent research workflow: 5 parallel survey modalities →
licence-filtered dedup → 6 deep reads → 2 gap measurements against this repo's actual source
→ synthesis. 50 candidates surveyed, 49 licence-viable, 28 gaps measured (6 critical, 12
high, 10 medium). Every claim below that drove a code change was independently re-verified
against the tree or against a live provider schema before acting on it.

Companion to [PARITY.md](PARITY.md), which measures Halation against *Higgsfield*. This file
measures it against the *open-source state of the art* and says what to do about it.

---

## 1. Landscape verdict

**"Nothing to fork" holds for the application. It is overturned for data and schema.**

The prior teardown (`~/higgsfield-research/oss-landscape.md`, 2026-08-02) was re-verified and
stands: `Anil-matcha/Open-Generative-AI` is at 25,526 stars with a genuine MIT licence and a
`middleware.js` that still rewrites every request to `api.muapi.ai` — a lead-gen funnel for a
paid aggregator, i.e. exactly the server-holds-the-key model Halation exists to reject. Only
its marketing copy changed in two days.

But that survey searched the literal string "higgsfield" sorted by stars, and therefore
missed three licence-clean assets that fill our top gaps. Searching by *capability* found
them:

| Asset | Licence | What it gives us |
|---|---|---|
| **`higgsfield-ai/skills`** | MIT, © 2026 Higgsfield AI | **The vendor published its own model catalogue under MIT.** `model-catalog.md` carries per-model constraint prose our `ModelDto` lacks entirely — *"Grok Video 1.5 … requires one --start-image or --image; duration 2-15s; resolution 480p or 720p"*. This is the authoritative fix for the fabricated-capabilities gap. |
| ~~`aqm857886159/Nomi`~~ | AGPL-3.0 | **Dropped 2026-08-05 — verified no fal.ai support.** Its 44 `modelArchetypes` target Volcengine, Apimart, Doubao, ModelScope and Dreamina. The *shape* is still worth reading (per-model modes, typed params, media slots carrying the wire key); the wire data is for providers we do not route to. |
| **`VedSoni-dev/openfield`** | MIT | 61 model-agnostic cinematography presets as plain data with `{subject}` compose templates and a `{id,label,category,desc,template,params,tags}` schema — a seed corpus *and* a working spec for the pack format we need. |

Also worth reading, all licence-verified by fetching the LICENSE file: **SwarmUI** (MIT,
actively converging on video from below), **Krita AI Diffusion** (GPL-3.0 — compatible with
our AGPL via GPLv3 §13; its `search_paths` model-filename table is months of accumulated
ecosystem knowledge and is legally takeable with attribution), **Stability Matrix** (AGPL-3.0,
same-licence reuse), **Fooocus** (GPL-3.0, and its 277-entry `sdxl_styles/*.json` corpus is
vendorable — **but `extras/expansion.py` and the Fooocus V2 prompt-expansion model are
CC-BY-NC 4.0 and hard-blocked**).

**Net: fork nothing. Adopt one MIT document, one AGPL schema with 44 worked examples, one MIT
preset corpus.**

---

## 2. The finding that reframes everything

> **Halation's harness is largely disconnected from its own shell.**

Presets, prompt-enhance and provider parameter translation are never executed on a real
generation, and every bridge call silently substituted fabricated mock results on failure —
so the app displayed invented completed videos with invented prices.

This is why the 371 green tests were not the reassurance they looked like: `PromptParts`,
`EnhanceInputs` and `PresetFamily` appear nowhere outside their own modules and `lib.rs`'s
re-exports. **The tests exercised code no user path reached.**

Standing rule adopted from this: *any new harness module needs at least one test that enters
through `commands.rs`, not only through the crate's own unit tests.*

---

## 3. Done on 2026-08-04

### ✅ Input media path (was: entirely missing)

`SubmitInput` carried no media and `submit_to_provider` sent only prompt + flags, so
image-to-video could not work at all. Built `crates/halation-core/src/media.rs` (roles →
wire flags, `Uploader` trait), fal + Higgsfield uploaders, and a Tauri-dialog file picker
that yields real filesystem paths. See PARITY.md §3.3a.

### ✅ Provider dialect (a defect in the above, caught by this workflow)

The binder emitted **Higgsfield's CLI names** — `image`, `start_image`, `end_image` — and
POSTed them to fal. Verified against seven live fal endpoint schemas: fal wants `image_url`,
and **fal is not internally consistent on the end frame** — `tail_image_url` (Kling),
`end_image_url` (MiniMax/Hailuo), `last_frame_url` (Wan VACE). Every fal image-to-video call
would have failed with a 422.

Added `media::Dialect` with a per-slug table; `app.rs` picks `Catalog` for Higgsfield's own
API and `Fal` for everything else. Eight new tests pin the mappings to the fetched schemas.

**The durable fix is to read fal's published per-endpoint OpenAPI**
(`fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`, unauthenticated) and cache it — the
same argument as fetching prices rather than transcribing them. Recorded in the module docs.

### ✅ Stopped fabricating results (rank 1)

`api.ts` had five `catch → mock*` arms. `invoke` rejects both when Tauri is absent *and* when
a Rust command returns `Err`, and a bare catch cannot tell them apart — so in the shipped
binary a real provider rejection produced a synthetic job that marched to "completed" with an
invented seed and price. All five are now gated on `isDesktop()` before falling back; inside
the app the error propagates. Added `asError` (Tauri rejects with a bare `String`, which
reaches React with no `.message`), a `catch` in `onSubmit` (previously try/finally only, so
the rejection was an unhandled promise the webview swallowed), and a `role="alert"` banner
above the feed styled per the brand rule that error state is **never** a solid fill.

**Tests: 417 green** (303 Rust + 114 UI). fmt · clippy `-D warnings` · deny · provenance ·
tsc · vitest all pass.

---

## 4. Ranked remaining work

Ordered by user-visible impact ÷ effort. Each is tagged with whether it makes Halation
**better** or merely **more like Higgsfield** — both legitimate, conflating them is not.

| # | Action | Effort | Kind |
|---|---|---|---|
| 1 | **Wire preset + enhance into `submit_job`.** Today a user picks a camera preset, the id round-trips through SQLite, and the provider receives the raw prompt with no camera clause. The preset is decorative. | M | better |
| 2 | **Intersect route availability with adapter existence.** `route.rs` filters by "user holds a key", but `client_for` implements only fal and Higgsfield. With the default *cheapest* policy, a user who adds a Vaig key gets Vaig selected and then told *"no credentials for Vercel AI Gateway — add a key"* — after adding exactly that key. Worst possible failure shape for a BYO-key product, ~12 lines to fix. | S | better |
| 3 | **Ship real per-model capabilities over the bridge.** `capabilitiesFor()` returns the same hardcoded defaults for all 68 models, so the chip row lies about every one: pick 10s on a 5s model, the estimator quotes 10s, the button shows that price, the provider rejects after the round trip. Seed from `higgsfield-ai/skills` (MIT). | M | better |
| 4 | **Make results real files.** `runner.rs` already downloads every output and sets `local_path`, but the wire shape drops it, so cards render the provider's *signed, expiring* URL. Once signatures lapse the feed goes blank while good files sit on disk. Add Download/Reveal; make Delete durable. | S | better |
| 5 | ~~**`to_provider` adapter layer.**~~ **Largely done 2026-08-05** — not by hand-authoring a table but by reading fal's published per-endpoint OpenAPI at submit time (`crates/halation-core/src/fal_schema.rs`). Drops fields the endpoint does not declare, reconciles spelling (`1k`→`1K`, `4`→`4s`), and refuses rather than silently ignoring attached media. Nomi turned out to have no fal data to seed it with, which is fine: fal publishes its own. | — | done |
| 6 | **Preset packs as versioned JSON.** `camera.rs` is `pub const TEMPLATES` — compile-time literals, Serialize-only. Adding one preset means editing Rust and shipping a binary; at 419 presets that is arithmetically impossible and third parties can never contribute. Seed from openfield (MIT, 61 presets). | L | both |
| 7 | **Library surface.** The app generates media the user cannot browse. Sequence *after* the pack format so the browser isn't built twice. | L | parity |
| 8 | **An actual enhancer.** `EnhanceReason` is fully modelled and tested; no LLM rewrite exists anywhere. Enhance rule 1 forces enhance on for a real preset *because the preset's aesthetic is delivered by the rewrite* — with no enhancer, every preset ships its name with none of its effect. | M | better |
| 9 | **Verify keys during onboarding.** `validate_key` exists but `Onboarding` renders rows without the `manage` prop, so a first-run user pastes a key and sees only a keychain-presence boolean. Add "fal — 40 models reachable". | M | better |
| 10 | **Honest model/route pickers.** `RouteDto.available` is documented as existing "so the picker can show the route and explain what unlocks it" — and is dropped on the floor by the TS wire type. Add search; grey unreachable rows. | M | better |
| 11 | **Bound concurrency, add queue mode, phase labels.** One poll loop per job, no limiter, despite a new fal account allowing 2 concurrent — the app amplifies its own rate limiting. Flat 600s timeout kills legitimate 4K jobs. Phase label (queued/uploading/generating/downloading), **not** a percentage. | M | better |
| 12 | **Preset picker dead ends.** No path back to "no preset" though the tile displays "General"; a chain preset permanently disables the prompt box. | S | better |
| 13 | **Earn the desktop binary.** ⌘↵ submit, ⌘K palette, OS notification on completion, re-roll grouping, real breakpoints. Native is the entire differentiation and it is currently forfeited. | M | better |
| 14 | **Complete the job record.** SQLite has no media column, so Rerun restores an i2v job as t2v and the user pays for the wrong generation. Also: `api.ts` reads `route` while Rust serialises `route_id` — `job.route?.id` is `undefined` for every persisted job. | M | better |

---

## 5. Do not do

- **Remotion / `@remotion/*`** — found in at least four projects here. Note the pattern:
  OpenMontage and clipforge are both AGPL-3.0 while bundling a source-available
  tiered-restriction dependency, i.e. plausibly violating their own outbound grants. Take
  neither the code nor the habit. Standing block also on `@diffusionstudio/core`, Open WebUI
  License, LobeHub Community License, Inngest server (SSPL), tldraw, Twick.
- **Any repo with no LICENSE file** — all rights reserved. Re-verified today by reading the
  actual root listing rather than a README badge: `SegFault42/HeliosGen`,
  `ClabstreamTeam/Open-Higgsfield-AI` (README still claims "MIT licensed" while the homepage
  now points at muapi.ai — a live demonstration of why badges are not licences),
  `higgsfield-ai/higgsfield-js`, and both `agnes-ai-*` repos (156 stars each, and each is
  literally a README plus an index.html — SEO landing pages, not code).
- **`FarisHijazi/higgsfield-web-api`** as an endpoint source, even for API facts —
  reverse-engineered access to their *consumer web app*, advertising "free unlimited image
  generation the official CLI doesn't". Near-certain ToS violation. `higgsfield-ai/skills`
  (MIT) and the vendored CLI `MODELS.md` cover the same ground legitimately.
- **Any Higgsfield-owned creative asset, even from their own MIT repo.** MIT grants copyright,
  not trademark: their logo, icon and brandkit are out. Take the catalogue *facts*.
- **Port SwarmUI or Krita AI Diffusion line by line** — C#/.NET and Python/PyQt5 against our
  Rust/React. Every apparent shortcut is a rewrite in disguise, and reimplementing from a
  description creates no attribution obligation at all.
- **Inherit SwarmUI's in-band string sentinel** (`%%_COMFYFIXME_${id:default}_ENDFIXME_%%`).
  It already forces escaping literal `${`/`}` into `(`/`)`, silently corrupting any default
  containing those characters. Steal the concept, not the escaping bug.
- **Persist references by display name** — SwarmUI saves the preset stack as titles joined by
  `|||` and silently drops anything unresolvable on reload, so a rename breaks restoration
  invisibly. Persist by stable ID.
- **Add an ETA or any percentage that can reach 100% before the file is on disk.**
  `lib/status.ts` already documents the no-ETA policy and it is correct. The gap is a phase
  label, not a number.
- **Build canvas / studios / characters / lipsync / audio yet.** The most visible parity gaps
  and the most seductive — but each multiplies the surface area of a generator that until
  today posted wrong parameter names to wrong endpoints and fabricated results when it
  failed. Parity theatre on a broken harness just adds places for the same bugs to hide.
- **Hand-write large literal tables without a validating test.** Krita AI Diffusion ships a
  live bug proving it: a missing comma in `resources.py` makes Python concatenate two adjacent
  string literals into `"canny-sdxlcontrol-lora-canny-rank"`, silently breaking detection of
  two ControlNet families.
- **Re-run the query `text to video web ui`** — near-zero signal. What worked:
  `topic:ai-video-generator pushed:>2026-06-01 stars:>40`, `higgsfield alternative OR clone
  in:readme`, and decisively, searching by *capability* rather than by the string "higgsfield".

---

## 6. Open questions for the owner

1. **Is our licence AGPL-3.0-only or -or-later?** *Less urgent since Nomi was dropped — nothing is being copied, so nothing gets pinned.* Stability Matrix and Nomi ship bare AGPL-3.0
   with no or-later grant, so any file absorbing their code is pinned to *-only*. Record the
   decision in `LICENSES/` rather than letting a copy-paste make it.
2. **Is openfield's preset prose original?** Its own `camera.ts` header reads *"Higgsfield's
   'moat' is this list surfaced as clickable buttons."* MIT covers the repo but cannot launder
   someone else's copyrighted text. The 61 entries read as generic craft vocabulary (dolly in,
   crane up, orbit), which is almost certainly fine — spot-check against Higgsfield's motion
   names before bulk import, and be prepared to rewrite.
3. **Do we ship a local ComfyUI provider?** The biggest branch point. If yes, Krita AI
   Diffusion's `search_paths` table becomes high-value GPL-3.0 data legally takeable with
   attribution, and capability discovery via `/object_info` becomes the highest-value change
   to `clients.rs`. If no, all of that is dead weight.
4. **Who authors the ~400 remaining presets, and who pays for the preview renders?** The pack
   format makes third-party authoring possible for the first time, but that is a distribution
   answer, not a content answer.
5. **Which providers get direct adapters vs stay proxied through fal?** Determines whether the
   adapter layer needs nine implementations or three.
6. **Has the shell ever run against a live provider?** No. Several findings here — the
   `route`/`route_id` serde mismatch, the fabricated capabilities, the layout collapse — are
   the kind a single real session surfaces immediately. **One manual end-to-end run with a real
   fal key may reorder this entire list and should happen before item 5 starts.**
