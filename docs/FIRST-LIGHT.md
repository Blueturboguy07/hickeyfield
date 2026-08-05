# First light — 2026-08-05

The first time Halation ever talked to a real provider. Run with a live fal key via
`crates/halation-core/examples/first_light.rs`, which reads the key from `FAL_KEY` so it never
lands in a file or a command line:

```sh
FAL_KEY=$(security find-generic-password -s ai.halation.keys -a fal -w) \
  cargo run -p halation-core --example first_light
```

Everything in the registry until now was transcribed from documents. This is the first
evidence about which parts of it are true.

---

## What worked

**Upload.** `media.rs`'s fal path moved real bytes on the first attempt:
`POST /storage/upload/initiate` → signed `PUT` → public URL at `v3b.fal.media`. The two-step
flow verified against the official clients was correct.

**The dialect fix.** Binding a start frame produced `{"image_url": "…"}` — fal's parameter
name, not the catalogue's `image`. Had this shipped before the `Dialect` layer landed, every
fal image-to-video call would have 422'd. This is now confirmed live rather than by
inspection.

**Auth.** `Authorization: Key {key}` is the correct header shape for fal.

---

## What broke: the input-mode suffix was never implemented

The first real submit failed:

```
model Seedream 5.0 Pro via bytedance/seedream/v5/pro
est   $0.0675
FAIL  submit rejected: HTTP 404: {"detail":"Application \"seedream\" not found"}
```

Probing all 36 fal route slugs against fal's public schema endpoint
(`fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`, unauthenticated) gave:

| | Count | Meaning |
|---|---|---|
| Valid as-is | **16** | image models, mostly single-endpoint |
| Valid **with an input-mode suffix** | **17** | the adapter gap — see below |
| Not on fal at all | **3** | `kling3_0_turbo`, `minimax_hailuo`, `wan2_6` |

**33 of 36 slugs are correct.** The failure is not bad data — it is a missing feature that
`registry.rs` documents in its own header and that was never built:

> **Route slugs are family roots.** A single Higgsfield model maps to one endpoint per input
> mode on every provider (`/text-to-video`, `/image-to-video`, `/reference-to-video`, …), and
> the input mode is not known until the user has attached their media. The adapter appends the
> suffix; the route names the family.

Confirmed by probe — the bare root 404s while both suffixed forms resolve:

```
404  fal-ai/kling-video/v2.5-turbo/pro
200  fal-ai/kling-video/v2.5-turbo/pro/text-to-video
200  fal-ai/kling-video/v2.5-turbo/pro/image-to-video
```

So `FalClient::submit` posts a family root that can never resolve. **Every video generation
would have failed**, and no test could have caught it, because the only source of truth is
fal's live endpoint list.

### Not every family offers every mode

The suffix cannot be assumed — it must be *known*:

- `kling-omni-flf` offers **only** `/image-to-video`.
- `wan2_2_video` offers all four, including the image ones.
- `seedream_v5_pro` and `_lite` are image models and take `/text-to-image`.

Guessing here fails the same way the bare root does. This is the argument for **Phase C2** —
fetch and cache fal's published per-endpoint OpenAPI rather than hand-maintaining a suffix
table that silently rots.

### Three models are not on fal

`kling3_0_turbo` (`fal-ai/kling-video/v3/turbo`), `minimax_hailuo`
(`fal-ai/minimax/hailuo-2.3`) and `wan2_6` (`fal-ai/wan/v2.6`) resolve under no suffix. These
came from Higgsfield's *own picker*, which lists their in-house and newer versions, not from
fal's catalogue. Either they route elsewhere or they should be excluded — the registry's
`EXCLUSIONS` mechanism already exists for exactly this.

---

## Consequences

1. **Build the input-mode resolver.** Selection is deterministic from the attached media:
   no media → `text-to-*`; a start frame → `image-to-*`; a source video → `video-to-video`.
   The mode's *availability* per route is not deterministic and must come from fal's schema.
2. **`EXCLUSIONS` the three missing models**, or find their real route.
3. **Cost accuracy is still unverified.** The run never reached a completed generation, so the
   estimator has still never been checked against a real charge. That remains open.
4. `docs/PARITY.md` §3.3 can drop "no live generation has ever completed" for upload and
   binding, but **not** for the end-to-end path.

## Still unproven after this run

- Any completed generation, and therefore any cost comparison.
- The poll → download → `local_path` path (`runner.rs`), since nothing completed.
- Reattach-on-relaunch.
- Everything on Windows.
