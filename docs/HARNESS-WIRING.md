# Harness wiring — connecting `enhance.rs` to a real generation

**2026-08-05. Read-only trace; no code was changed to produce this.**

`crates/hickeyfield-core/src/enhance.rs` is 1,169 lines, fully tested, and **called by nothing
outside its own test module and `lib.rs`'s re-export**. `BRIDGE.md` §2 names this as the
finding that reframes the project. This file is the integration spec that closes it.

Companion reading: [BRIDGE.md](BRIDGE.md) §2 and §4 item 1/8, [PARITY.md](PARITY.md) §3.3,
[FIRST-LIGHT.md](FIRST-LIGHT.md).

---

## 0. What is actually missing

Two different things, and conflating them is why the gap has stayed open:

| | State | Where |
|---|---|---|
| **The compiler** — settings → one prompt string | Built, tested, unreachable | `enhance.rs:528–660` |
| **The decision** — should an LLM rewrite it | Built, tested, unreachable | `enhance.rs:343–374` |
| **The rewriter** — the LLM call itself | **Does not exist anywhere** | — |
| **The join** model → `JobType` | Built 2026-08-05 | `registry.rs:266–475` |

So this is not one change. It is a wiring job (compiler + decision into `submit_job`) plus a
new module (the rewriter). The wiring is worth doing even before the rewriter exists: it is
what makes a camera preset put its five-slot chain into the prompt at all.

### The current path, traced

```
ui/src/App.tsx:302  onSubmit
  └─ api.ts:499     submitJob({ modelId, routeId, prompt, presetId, settings, media })
       └─ commands.rs:484  submit_job
            ├─ 485  registry lookup                       ← Model, and now Model.job_type
            ├─ 495  Billable::from(&input.settings)
            ├─ 496  route::resolve(...)
            ├─ 508  JobSet {
            │        515   prompt: input.prompt.clone()   ← VERBATIM. No camera clause,
            │        516   enhanced_prompt: None,         ←   no preset text, no compile.
            │        517   preset_id: input.preset_id     ← stored, never resolved
            │       }
            ├─ 528  store.upsert
            └─ 530  app.rs:153  submit_to_provider
                     └─ 214    body["prompt"] = job.prompt ← the raw string reaches fal
```

`preset::get`, `PresetFamily::resolve_variant`, `PromptParts`, `enhance::build` and
`enhance::decide` appear nowhere on that path. A user picks "360 Orbit", the slug round-trips
through SQLite, and the provider receives the bare scene description.

---

## 1. Order of operations

The whole spec hangs on one dependency graph. **The enhance decision cannot run before the
preset is resolved and the media roles are known**, and the *rewriter* cannot run before
everything that could refuse the job has run.

```
model ─────────────┐
                   ├──► JobType ────────────────┐
preset_id ─► preset::get ─► PresetFamily ──┐    │
                   │            │          ├────┼──► EnhanceInputs ──► decide()
                   │            └► resolve_variant   │
media roles ─► has_end_frame ──────────────┘    │         │
                                                │         ▼
settings.enhance (Option<bool>) ────────────────┘   EnhanceDecision
                                                          │
family.camera_template ─┐                                 │
variant.prompt_template ├─► PromptParts ─► compile() ─────┤
input.prompt (scene) ───┘                                 ▼
                                                   CompiledPrompt
                                                          │
                          ┌───── refusals (free) ─────────┤
                          │  preset.validate()            │
                          │  has_unresolved_sentinel      │
                          │  media::bind() dry run        │
                          └───────────────────────────────┤
                                                          ▼
                                        rewriter (costs time and money)
                                                          │
                                                          ▼
                                              upload → submit_to_provider
```

Read top to bottom. Every arrow is a hard ordering constraint, and §7 lists what breaks if
any of them is reversed.

Two ordering facts worth stating because they are *not* obvious:

- **`has_end_frame` may be evaluated before upload.** `media::resolve` rewrites
  `MediaSource` but never touches `MediaRole` (`media.rs:285`), so the answer is invariant
  under uploading. Evaluate it early so a refusal costs no bandwidth.
- **The decision does not need the route.** `JobType` comes from `Model`, not from `Route`
  (`registry.rs:138`). Route resolution and the enhance decision are independent and may run
  in either order — but the *estimate* must be recomputed after preset baked-params merge
  (hazard H5).

---

## 2. `submit_job`, step by step

All line numbers are the file as it stands today.

### Step 0 — signature and threading

```rust
#[tauri::command(async)]                       // ← changed; see hazard H7
pub fn submit_job(state: State<'_, AppState>, input: SubmitInput) -> Result<String, String>
```

### Step 1 — resolve the preset id to a `PresetFamily`

Insert immediately after the registry lookup at `commands.rs:488`.

```rust
/// Resolve what the picker sent into a catalogue family.
///
/// Three outcomes, and collapsing any two of them is a bug:
///   * `Ok(None)`             — nothing selected, or the neutral General row.
///   * `Ok(Some(family))`     — a real preset.
///   * `Err(..)`              — an id we do not ship.
///
/// The refusal is the point. `preset::get` documents (preset.rs:446–452) that it will not
/// nearest-neighbour a miss, because an id arrives verbatim from persisted state and from
/// recipes, and quietly resolving `push-inn` to `push-in` runs a generation the user never
/// asked for and bills them for it. Treating the miss as `None` instead is the same bug
/// wearing a different hat: the job runs with no aesthetic and full price.
fn resolve_preset(id: Option<&str>) -> Result<Option<&'static PresetFamily>, String> {
    let Some(id) = id else { return Ok(None) };
    if let Some(f) = hickeyfield_core::preset::get(id) {
        return Ok(Some(f));
    }
    // A General sentinel has no family — the catalogue holds camera moves only
    // (preset.rs:441–444) — but it is a legitimate selection meaning "no preset".
    // `PresetSelection` already treats None and General identically
    // (enhance.rs test `no_preset_behaves_exactly_like_general`).
    if hickeyfield_core::preset::is_general_id(id) {
        return Ok(None);
    }
    Err(format!(
        "unknown preset: {id} — it may have come from an older library or a recipe \
         built against a different preset pack"
    ))
}
```

Then:

```rust
let family = resolve_preset(input.preset_id.as_deref())?;
let variant = family.and_then(|f| f.resolve_variant(&model.id));   // preset.rs:355
```

### Step 2 — validate the preset's own requirements, before any spend

```rust
/// `MediaCounts` from attached roles. Every image role counts, because the
/// constraint the presets express is on how many pictures the model sees
/// (preset.rs:216–225).
fn media_counts(media: &[hickeyfield_core::MediaRef]) -> hickeyfield_core::preset::MediaCounts {
    use hickeyfield_core::MediaRole::*;
    let mut c = hickeyfield_core::preset::MediaCounts::default();
    for m in media {
        match m.role {
            Start | End | Reference => c.images += 1,
            Video | VideoReference => c.videos += 1,
            Audio | AudioReference => c.audios += 1,
        }
    }
    c
}

if let Some(f) = family {
    let errs = f.validate(&input.prompt, &media_counts(&input.media));   // preset.rs:381
    if !errs.is_empty() {
        // Every problem at once. A form that reveals its errors one at a time
        // is a form people give up on — preset.rs:378–380 says so and the
        // validator already returns all of them.
        return Err(errs.iter().map(|e| e.msg.as_str())
                       .collect::<Vec<_>>().join("; "));
    }
}
```

### Step 3 — build `PromptParts`, including the camera clause

**This is where the duplication trap lives.** A camera family carries a `camera_template`
slug *and* a single catch-all variant whose `prompt_template` is the same rendered chain
(`preset.rs:491–510`). `PromptParts::compile` emits the camera template first and then the
preset clause (`enhance.rs:590–604`), so setting both naively ships the five-slot chain
twice.

```rust
let rendered_camera = family.and_then(|f| f.camera()).map(|t| t.render());  // preset.rs:363

let preset_clause: Option<String> = variant
    .map(|v| v.prompt_template.trim())
    .filter(|t| !t.is_empty())
    // Drop the variant text when it *is* the camera chain. Without this filter every
    // camera preset compiles to "Camera: … End: …. Camera: … End: …. <scene>." — a
    // doubled directive the user pays for and never sees, because nothing renders the
    // compiled prompt today. The comparison is on the rendered string rather than on
    // "does the family have a camera slug", so a future VFX preset that carries both a
    // move and its own distinct body text keeps both.
    .filter(|t| Some(*t) != rendered_camera.as_deref())
    .map(str::to_string);

let mut parts = hickeyfield_core::PromptParts::scene(&input.prompt);
if let Some(slug) = family.and_then(|f| f.camera_template.as_deref()) {
    parts = parts.with_camera(slug);
}
if let Some(text) = preset_clause {
    parts = parts.with_preset(&text);
}
// lighting / lens / mood stay None: no UI control and no DTO field exists for them yet.
// They are slots in the compiler (enhance.rs:534–538), not features.
```

An unknown camera slug is *dropped*, not pasted (`enhance.rs:585–589`). If a preset ever
resolves to a slug `camera::get` does not know, `parts.camera_template()` returns `None` and
the move is silently lost — worth a warning on the job (see H11), not a refusal.

### Step 4 — the three inputs, and `enhance::build`

```rust
let inputs = hickeyfield_core::EnhanceInputs {
    job: model.job_type,                                        // registry.rs:51
    preset: hickeyfield_core::PresetSelection::from_family(family),// enhance.rs:231
    has_end_frame: hickeyfield_core::media::has_end_frame(&input.media), // media.rs:679
    user_toggle: input.settings.enhance,                        // Option<bool> after H4
};

let compiled = hickeyfield_core::enhance::build(
    &parts,
    inputs,
    variant.and_then(|v| v.negative_prompt.as_deref()),
);
```

Note the **struct literal, not the builders**. `EnhanceInputs::with_toggle` can only produce
`Some(_)` (`enhance.rs:276–279`); there is no `with_toggle(None)`, and `None` is the state
that means "untouched, use the job-type default". All four fields are `pub`, so the literal
is the intended escape hatch.

### Step 5 — refuse an unresolved sentinel

```rust
if compiled.has_unresolved_sentinel {
    // The prompt still points at a *particular* generation's attachments. Submitted
    // as-is it reaches the provider as literal `<<<image_1>>>` gibberish. enhance.rs:477
    // states the rule; this is the call site that has to honour it.
    return Err(format!(
        "This prompt still refers to an attachment that is not attached: {}",
        hickeyfield_core::enhance::sentinels(&compiled.prompt)
            .iter().map(|s| s.token()).collect::<Vec<_>>().join(", ")
    ));
}
```

### Step 6 — merge baked params, *then* price, *then* resolve the route

`PresetVariant.baked` (`preset.rs:269`) is currently dead data — nothing reads it. It must
be merged into the settings before `Billable::from` at `commands.rs:495`, or the Generate
button quotes a price for settings the preset then overrode. See H5.

### Step 7 — persist the row **with the compiled prompt on the first INSERT**

```rust
let mut job = JobSet {
    ...
    prompt: compiled.prompt.clone(),                 // ← was input.prompt.clone()
    enhanced_prompt: None,                           // filled in at step 8
    enhancer_version: None,                          // new field
    enhance_reason: Some(compiled.reason),           // new field
    prompt_parts: Some(parts.clone()),               // new field — see H9
    preset_id: family.map(|f| f.id.clone()),         // the canonical id, not the raw string
    ...
};
state.store.upsert(&job).map_err(|e| e.to_string())?;
```

`prompt`, `preset_id`, `settings`, `estimated_usd` and `media` are **not** in the store's
`ON CONFLICT DO UPDATE SET` list (`store.rs:197–204`). They must be right on the first
insert; a later "fix-up" upsert is silently discarded. See H8.

### Step 8 — run the rewriter, iff the decision says so

```rust
if compiled.enhance {
    let choice = input.enhancer.clone()
        .ok_or_else(|| "Prompt enhancement is on but no enhancer is configured. \
                        Pick one in Settings, or turn Enhance off.".to_string())?;

    let out = hickeyfield_core::rewrite::run(&choice, &hickeyfield_core::rewrite::Request {
        prompt: &compiled.prompt,
        job: model.job_type,
        model_name: &model.display_name,
        negative_prompt: compiled.negative_prompt.as_deref(),
    }).map_err(|e| format!(
        "Could not enhance the prompt: {e}. \
         {}", match compiled.reason {
            EnhanceReason::Preset =>
                "This preset's look is delivered by the rewrite, so generating without it \
                 would ship the preset's name and none of its effect.",
            _ => "Turn Enhance off to generate with your prompt as written.",
        }
    ))?;

    job.enhanced_prompt = Some(out.text);
    job.enhancer_version = Some(out.version);
    job.updated_at = now_secs();
    state.store.upsert(&job).map_err(|e| e.to_string())?;   // enhanced_prompt IS in the
}                                                           // ON CONFLICT list already
```

**Refuse, do not degrade.** A rewriter failure that silently submits the unrewritten prompt
is a generation the user pays for that is not the one they asked for — the exact failure the
house rule names. Offer "Generate without enhancement" as a *second explicit click* in the
UI, never as an automatic fallback.

### Step 9 — send the right string

`app.rs:214–219` currently sends `job.prompt`. It must send the rewrite when there is one:

```rust
// app.rs, replacing lines 214–219
let outgoing = job.enhanced_prompt.as_deref().unwrap_or(&job.prompt);
if model.spec.takes_prompt() && !outgoing.is_empty() {
    body.insert("prompt".into(), serde_json::Value::String(outgoing.to_string()));
}
```

---

## 3. `Model.job_type` → `EnhanceInputs`

The join added today (`registry.rs:266–475`) is the whole reason any of this is reachable.

- `Model.job_type: JobType` is assigned in `assemble` at `registry.rs:138` from
  `job_type_for(&spec.id, spec.modality)` (`registry.rs:450`).
- `job_type_for` looks the model id up in `JOB_TYPES` (16 rows, 68 models) and falls back to
  a per-modality default with a `debug_assert!` naming the hazard.
- The value reaches the decision as `EnhanceInputs.job`, and is consumed by exactly one
  thing: `JobType::default_enhance()` (`enhance.rs:188–199`) under rule 3.

Consequences the wiring must respect:

1. **`job_type` never changes the outcome when the user has touched the toggle or a real
   preset is selected.** It is a default, not a policy (`registry.rs:246–248`). So the cost
   of a debatable `JOB_TYPES` row is one wrong initial toggle position, not a wrong
   generation — *provided* `user_toggle` can actually be `None` (H4).
2. **13 of 26 job types default off.** Video, the styled/FLUX/Z-Image/avatar/builder image
   types and the legacy `image-gpt` surface default on; `animate`, all the Nano Banana and
   Seedream and GPT-Image-2 surfaces, `reference`, `scene`, `product`, `speech`, `lipsync`,
   `photodump` and `fashion-factory` default off. The UI must therefore compute the toggle's
   initial position from Rust, not from a TypeScript constant.
3. `job_type` is not serialised on the job record today. It does not need to be — it is
   recoverable from `model_id` — but `enhance_reason` is *not* recoverable, which is why §4
   persists it.

---

## 4. The three rules, end to end

`decide` (`enhance.rs:343–374`) applies end frame → preset → user, in that order. Here is
each rule's full path from a click to a request body.

### Rule 1 — a real preset forces enhance ON

```
PresetPicker onPick → App.setPresetId("360-orbit")
  → SubmitInput.presetId = "360-orbit"
  → resolve_preset → Some(PresetFamily{ id:"360-orbit", camera_template:Some("360-orbit") })
  → PresetSelection::from_family(Some(f)) → from_id("360-orbit")
      → is_general_id("360-orbit") == false → PresetSelection::Real
  → decide → { enhance: true, reason: Preset }   (overrides settings.enhance == Some(false))
  → job.enhance_reason = Preset  → Toggle renders locked, hint "Turned on: a preset is selected"
  → rewriter runs → job.enhanced_prompt = Some(..) → body["prompt"] = enhanced
```

The classification runs on the **resolved** family, not the raw id. `PresetSelection::from_id`
would classify a typo'd id as `Real` (`enhance.rs:223–229` — anything not General is Real),
force enhance on, and spend an LLM call for a preset that does not exist.

### Rule 2 — an end frame forces enhance OFF, unconditionally

```
FrameSlots slot { role:"end" } → addMedia("end", …) → MediaRef{ role: End }
  → SubmitInput.media contains role == "end"
  → media::has_end_frame(&input.media) == true          (media.rs:679)
  → decide returns early, before the preset check       (enhance.rs:345–350)
  → { enhance: false, reason: EndFrame }
  → rewriter is not called, no LLM cost
  → job.enhance_reason = EndFrame → Toggle locked off, "Turned off: an end frame is attached"
```

Unconditional means unconditional: it beats a real preset and it beats an explicit
`enhance: true` toggle. `enhance.rs:793–815` pins the precedence with a test that flips only
the end frame and shows the answer change.

Interaction with binding: `MediaRole::End` has **no fallback flag** (`media.rs:66–76`), so a
model with no `end_image`/`tail_image_url` will make `media::bind` refuse. Run the dry bind
before the rewriter so the two agree (H2).

### Rule 3 — "General" honours the toggle

```
PresetTile shows "General" (preset === null) → SubmitInput.presetId = null
  → resolve_preset → Ok(None)
  → PresetSelection::from_family(None) → PresetSelection::None
  → forces_enhance() == false                            (enhance.rs:236–239)
  → decide falls through to user_toggle:
        Some(on) → { enhance: on,                      reason: UserToggle }
        None     → { enhance: job.default_enhance(),   reason: JobDefault  }
```

`PresetSelection::None` and `PresetSelection::General` are behaviourally identical under
`decide` — pinned by `no_preset_behaves_exactly_like_general` across all 26 job types and all
three toggle states. So the UI's cosmetic confusion (the tile says "General" when nothing is
selected, `PresetTile.tsx:40`, with no path back to none — `BRIDGE.md` §4 item 12) is *safe
for enhance*. Do not "fix" it by sending `presetId: "general"`: `preset::get("general")`
returns `None` because the catalogue holds camera families only, which is why
`resolve_preset` needs its explicit `is_general_id` branch.

---

## 5. Persistence

### 5.1 `JobSet` — three new fields

`engine.rs:20–58`. `enhanced_prompt` (line 39) and `preset_id` (line 40) already exist.

```rust
/// Which rewriter produced `enhanced_prompt`, as `{backend}/{model}+{template}` —
/// e.g. `ollama/llama3.1:8b+hickeyfield-rewrite-v1`.
///
/// The template id is not decoration. When our system prompt changes, identical inputs
/// start producing different text, and a record naming only the model cannot explain
/// why yesterday's generation and today's disagree. `None` means no rewrite ran.
#[serde(default)]
pub enhancer_version: Option<String>,

/// Why enhancement ended up on or off. Not recoverable from `enhanced_prompt`:
/// "off because an end frame was attached" and "off because you turned it off" are
/// the same absent string, and the meta rail has to be able to tell them apart.
#[serde(default)]
pub enhance_reason: Option<crate::enhance::EnhanceReason>,

/// The structured settings `prompt` was compiled from.
///
/// Without this, Rerun cannot restore what the user typed — `prompt` is now the
/// *compiled* string, and loading it back into the composer would prepend the camera
/// chain a second time on the next submit, and a third on the one after that.
/// `PromptParts` is fully serialisable precisely so a generation can be reconstructed
/// from its record (enhance.rs:526–528).
#[serde(default)]
pub prompt_parts: Option<crate::enhance::PromptParts>,
```

Three prompt strings now exist on a record and they are all needed:

| Field | Contents | Used for |
|---|---|---|
| `prompt_parts.scene` | what the user typed | Rerun puts this back in the box |
| `prompt` | the compiled prompt | shown as "Compiled", copyable |
| `enhanced_prompt` | the rewrite | **what was sent**, shown and copyable |

### 5.2 `store.rs` — schema v4

Append after the v3 block at `store.rs:105–116`. Migrations are append-only; never renumber.

```rust
if version < 4 {
    // The rewrite and its attribution. Before this, `submit_job` hardcoded
    // enhanced_prompt: None and the whole enhance subsystem left no trace on the record.
    conn.execute_batch(
        r#"
        ALTER TABLE job_sets ADD COLUMN enhancer_version TEXT;
        ALTER TABLE job_sets ADD COLUMN enhance_reason   TEXT;
        ALTER TABLE job_sets ADD COLUMN prompt_parts     TEXT;
        PRAGMA user_version = 4;
        "#,
    ).map_err(map_err)?;
}
```

Then, in `upsert` (`store.rs:190–224`): add the three columns to the INSERT list and the
`params!` tuple, **and** add them to `ON CONFLICT DO UPDATE SET` — `enhancer_version` and
`enhance_reason` change on the second upsert of step 8, so omitting them from the update
list makes them permanently NULL. And in `row_to_job` (`store.rs:139–173`), read them back;
follow the existing pattern of degrading a malformed blob to `None` rather than dropping the
whole job.

`enhance_reason` serialises kebab-case (`enhance.rs:286`) — `end-frame`, `preset`,
`user-toggle`, `job-default`. Store the bare token, not the JSON-quoted form, matching how
`status` is handled at `store.rs:177–180`.

---

## 6. Provider vs user: two strings, both visible

**Sent:** `enhanced_prompt` when present, else `prompt`. One place, `app.rs:214`.

**Shown:** both, separately copyable. Today `MetaCard.tsx`:

- line 78 renders `job.prompt` as the headline;
- lines 80–85 render `job.enhancedPrompt` in a `<details>`;
- line 52 `copyPrompt` copies `job.enhancedPrompt || job.prompt` — **one button, one value**.
  The moment a rewrite exists the original becomes uncopyable, which is precisely backwards:
  the original is the thing you want to paste into the composer and try again.

Required change: two copy affordances, labelled. Copy of the *sent* string should say so —
"Copy sent prompt" — because after this wiring lands the headline `job.prompt` is no longer
what the provider received, and a user debugging a bad result who copies the wrong one
learns nothing.

Also: `MetaCard.tsx:155–158` already renders an "Enhancer version" row reading
`job.enhancerVersion ?? "Not enhanced"`. That string is wrong for three of the four reasons —
an end frame or a job-type default did not "not enhance", they *declined* to. Render
`EnhanceReason::explanation()` (`enhance.rs:305–312`) when `enhancerVersion` is null.

---

## 7. Ordering hazards

Each names the failure it prevents. H4, H5, H8 and H9 are live defects that this wiring
either creates or exposes; the rest are traps to avoid while writing it.

**H1 — Classify the *resolved* preset, never the raw id.**
`PresetSelection::from_id` returns `Real` for any non-General string. A typo, a stale recipe
id or a preset from an uninstalled pack therefore forces enhance on and spends an LLM call
for an aesthetic that does not exist. Resolve first; refuse on a miss.

**H2 — The rewriter must run after every free refusal.**
`media::bind`'s no-fallback refusal for `MediaRole::End` (`media.rs:66–76`) currently lives
inside `submit_to_provider` (`app.rs:180`), i.e. *after* where the rewrite would go. Move a
dry bind into `submit_job` before step 8, or pay for a rewrite on jobs that are about to be
refused. Same argument for `preset.validate` and the sentinel check.

**H3 — The camera chain will be emitted twice.**
`camera_family` sets both `camera_template` and a catch-all variant holding the identical
rendered string (`preset.rs:491–510`); `compile` emits camera then preset
(`enhance.rs:593–597`). Every one of the 25 shipped presets hits this. See step 3 for the
filter. Nothing renders the compiled prompt today, so this would ship invisibly.

**H4 — `user_toggle` cannot currently be `None`, and that is not cosmetic.**
`SettingsDto.enhance` is `bool` with `#[serde(default)]` (`commands.rs:375`); TS
`GenSettings.enhance` is `boolean` (`types.ts:165`); `App.tsx:61` initialises it to `true`.
Result: `EnhanceReason::JobDefault` is unreachable, and every job on the 13 off-by-default
job types is reported as an explicit user "on". The app would rewrite "remove the background"
into three sentences of cinematic prose on Nano Banana, Seedream and GPT Image 2 — the exact
outcome `enhance.rs:180–187` exists to prevent. Both DTOs must become `Option<bool>` /
`boolean | null | undefined`, and `App.tsx`'s default must become "untouched".

**H5 — Baked preset params must be merged before the price is quoted.**
`PresetVariant.baked` (`preset.rs:269`) is read by nothing. Once it is, a preset that bakes
`duration: 10` over a user's 5 makes `Billable::from(&input.settings)` (`commands.rs:495`)
quote the 5-second price on the Generate button and bill the 10-second one. Merge baked into
the effective settings, build the `Billable` from the merged value, persist the merged value,
and — house rule — surface every key the preset overrode rather than swapping it silently.

**H6 — Our `enhance` flag must never reach the model's `enhance_prompt` flag.**
Seven models declare a provider-side `enhance_prompt` boolean: `seedance_pro`,
`kling-v2-5-turbo`, `wan2_5_video`, `wan2_2_video`, `image2video`, `cinematic_studio_3_0`,
`cinematic_studio_video_3_5`. Today `ModelSpec::flag("enhance")` does not match
`enhance_prompt` (`catalog.rs:220–225` compares name-or-alias exactly), so the settings loop
at `app.rs:235–256` drops it — **by accident, not by design**. Adding
`"enhance" => &["enhance_prompt"]` to the candidates table at `app.rs:241` would ask the
provider to rewrite a prompt we already rewrote, unrecorded, over the top of the camera
clause. Pin the exclusion with a test.

**H7 — `submit_job` is a synchronous Tauri v2 command.**
Sync commands run on the main thread. `submit_job` already blocks it on media uploads
(`app.rs:190–200`) and on `fal_schema::for_endpoint`, which is a live HTTP fetch
(`fal_schema.rs:165–175`). Adding a 5–20 s LLM rewrite freezes the window. No command in
`src-tauri/src/commands.rs` is `async` today. Use `#[tauri::command(async)]`, or move the
rewrite off the submit path entirely. *Verify this on the running app before relying on the
diagnosis — it is inferred from the Tauri v2 threading model, not observed here.*

**H8 — `ON CONFLICT DO UPDATE` silently discards most fields, including a live one.**
`store.rs:197–204` updates only `status`, `enhanced_prompt`, `updated_at`, `results`,
`actual_usd`, `fail_reason`, `endpoint`. Two consequences:

  (a) *For this work:* `prompt`, `preset_id`, `settings` and `estimated_usd` must be final on
  the first INSERT at `commands.rs:528`. A "write raw now, compile later" shape would leave
  the raw prompt in the database while the provider got the compiled one.

  (b) *Pre-existing bug on the same statement, found while tracing this:* `request_id` is
  absent from the update list. `commands.rs:531–534` sets `job.request_id` from the provider
  and upserts — the row keeps `request_id = ''`. Live polling still works because
  `runner.watch(job)` holds the in-memory copy, but `resume_all()` on relaunch reads the row
  and has no handle to poll with. That is `PARITY.md` §3.3's "reattach-on-relaunch:
  implemented, never exercised" with a cause. Add `request_id = excluded.request_id` while
  adding the v4 columns.

**H9 — Rerun will compound the compiled prompt.**
`App.tsx:339` does `setPrompt(job.prompt)`. Once `job.prompt` is the compiled string, Rerun
loads "Camera: … End: …. A cat." into the composer and the next submit prepends the chain
again. Hence `prompt_parts` in §5.1: Rerun must restore `prompt_parts.scene`, not `prompt`.

**H10 — `list_presets` does not go through the catalogue.**
`commands.rs:336–347` maps `camera::TEMPLATES` directly and hardcodes
`category: "camera-control"`. It works only because `camera_family` makes the family id
identical to the template slug (`preset.rs:488–490`) — a coincidence the spec relies on and
should not. Move it to `preset::catalog()` and widen `PresetDto` (§8.3). Note that the
category string then becomes `basic_camera_control` / `epic_camera_control`, and that
`ui/src/lib/presets.ts:10–18` matches neither the old value nor the new one — **category
filtering in the picker is already broken and this change does not fix it.** Send
`Category::display_name()` alongside the slug.

**H11 — Dropped inputs must reach the user, not just the log.**
Three silent drops on this path: an unknown camera slug (`enhance.rs:585–589`), a preset
negative prompt on a model with no such flag (the vendored spec declares `negative_prompt`
once in the entire document and `registry.rs` declares it zero times), and fal schema
mismatches, which `app.rs:305` reports with `tracing::warn!` — invisible inside a packaged
app. Add `warnings: Vec<String>` to `JobSet` (`#[serde(default)]`, one more v4 column) and
render them on the meta card. This is the generalisation of the existing warn, not a new
subsystem.

**H12 — An empty rewrite would submit no prompt at all.**
`app.rs:214` guards on `!prompt.is_empty()`. If a rewriter returns whitespace, the key is
omitted and a text-to-video model runs with no text — a full-price generation of nothing.
The rewriter must refuse empty output (§8.1) rather than let this guard swallow it.

---

## 8. The rewriter, and the UI that chooses it

### 8.1 New module: `crates/hickeyfield-core/src/rewrite.rs`

Named `rewrite`, not `enhancer`: `enhance` and `enhancer` differ by two characters and would
be indistinguishable in an import list. `enhance.rs` decides; `rewrite.rs` executes.

```rust
/// What the rewriter is asked to do. Borrowed, because none of it outlives the call.
pub struct Request<'a> {
    pub prompt: &'a str,
    /// Lets the system prompt speak the model's dialect — a video model wants camera
    /// language, an editing model wants the instruction left alone.
    pub job: JobType,
    pub model_name: &'a str,
    pub negative_prompt: Option<&'a str>,
}

pub struct Rewritten {
    pub text: String,
    /// `{backend}/{model}+{template}`. See the field doc on `JobSet::enhancer_version`.
    pub version: String,
}

#[derive(Debug)]
pub enum RewriteError {
    /// The backend did not answer. Names the endpoint so the user can check it.
    Unreachable(String),
    NoCredential(String),
    /// The model answered with nothing usable. Never submit this — see hazard H12.
    Empty,
    /// The rewrite dropped or invented a media sentinel.
    SentinelDrift { before: usize, after: usize },
    TooLong { got: usize, max: usize },
}

pub trait Rewriter: Send + Sync {
    fn rewrite(&self, req: &Request<'_>) -> Result<Rewritten, RewriteError>;
    fn version(&self) -> String;
}

/// Dispatch on the user's choice.
pub fn run(choice: &EnhancerChoice, req: &Request<'_>) -> Result<Rewritten, RewriteError>;
```

Four guards the rewriter owes its caller, each of which needs a test named for it:

1. **Sentinels must survive.** `enhance::sentinels(&out)` must match
   `enhance::sentinels(&in)` as a `(kind, index)` multiset. An LLM that helpfully turns
   `<<<image_1>>>` into "the attached image" destroys the binding `resolve_sentinels`
   (`enhance.rs:496`) depends on, and the reference silently stops pointing anywhere.
2. **Non-empty.** Whitespace-only output is `RewriteError::Empty`, never a submit (H12).
3. **Under the cap.** `PromptCard.tsx:4` enforces 4,000 characters on input; the rewrite must
   not exceed it either, or the composer cannot hold what was actually sent.
4. **Deterministic version string.** Two rewrites from the same backend, model and template
   must report the same `version`, or the record cannot be used to explain a difference.

### 8.2 Backends

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum EnhancerChoice {
    /// Local Ollama. No key, no cost, no egress — the same argument as the Local
    /// provider in `client_for` (app.rs:26–30): only possible because we are native.
    Ollama { model: String },
    /// The user's own hosted LLM key.
    Hosted { vendor: LlmVendor, model: String },
}
```

**Ollama.** `clients.rs:514` already gives `OLLAMA_URL = http://127.0.0.1:11434`, and
`detect_local()` (`clients.rs:532–550`) already probes `GET /api/tags`. Two things that probe
does *not* prove and the UI must not imply:

- it proves the daemon answers, **not** that any model is pulled. Read the `models` array
  from that same `/api/tags` response and populate the picker from it; an Ollama with nothing
  pulled must be reported as "running, no models — `ollama pull <model>`" rather than offered
  as an enhancer that fails at submit time;
- it proves `/api/tags` exists, **not** the chat endpoint. **The chat path and request shape
  must be verified against a running Ollama before shipping.** I did not verify them here and
  have deliberately not written one into this document.

**Hosted.** Verify each vendor's endpoint against that vendor's current documentation before
adding it, and do not hardcode a model id — hosted model names churn faster than our release
cadence. Ship the vendors you have verified and no others.

**Cost.** A local rewrite is genuinely $0. A hosted rewrite costs tokens, and we cannot know
how many before the call. Per the house rule, that is **unknown, which is not zero**: the
Generate button's estimate covers the generation only, and the enhancer line under it must
say the LLM's token cost is separate and unquoted. Do not invent a cent figure.

### 8.3 Credentials — a separate keychain namespace

`vault.rs` keys the keychain by `ProviderId::slug()` (`vault.rs:16–22`), and
`configured_providers()` (`commands.rs:31–38`) feeds the availability list that
`route::resolve` uses. Storing an OpenAI *text* key under `ProviderId::OpenAi` would make the
router believe the user can run OpenAI *image* models — the same class of bug that
`commands.rs:40–54` documents for `Local`, where "needs no key" read as "ready to generate".

So: a parallel namespace, `enhancer.openai`, with its own `enhancer_key_states()` and
`set_enhancer_key()` commands. `vault::account` is private; either make a public
`vault::set_raw(account, value)` or add a small sibling module. The one rule that already
holds everywhere applies unchanged — the value never crosses the bridge.

### 8.4 New and changed commands

```rust
/// The enhance outcome for the current composer state, without submitting.
///
/// Pure: no network, no cost. Exists so the Toggle can render locked with the right
/// explanation, and so the rail can show the compiled prompt before the user pays.
/// Computing it in TypeScript instead would put a second copy of the three rules in a
/// language with no test coverage of them.
#[tauri::command]
pub fn enhance_preview(
    model_id: String,
    preset_id: Option<String>,
    prompt: String,
    media: Vec<MediaRef>,
    enhance: Option<bool>,
) -> Result<EnhancePreviewDto, String>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancePreviewDto {
    pub enhance: bool,
    /// kebab-case: end-frame | preset | user-toggle | job-default
    pub reason: String,
    /// `EnhanceReason::explanation()` — one line of UI copy.
    pub explanation: String,
    /// Render the toggle locked. `EnhanceDecision::is_forced()`.
    pub forced: bool,
    /// The compiled prompt, so the rail can show what will actually be sent.
    pub compiled_prompt: String,
    pub has_unresolved_sentinel: bool,
}

/// Which enhancers this machine can actually run right now.
#[tauri::command]
pub fn enhancer_options() -> Vec<EnhancerOptionDto>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancerOptionDto {
    pub id: String,             // "ollama:llama3.1:8b"
    pub label: String,          // "Llama 3.1 8B (local)"
    pub backend: String,        // "ollama" | "openai" | …
    pub model: String,
    /// Free and offline. Drives the "$0, nothing leaves this machine" badge.
    pub local: bool,
    pub available: bool,
    /// Why not, in words the picker can render. Same contract as `RouteDto`
    /// (commands.rs:113–128) — show it and explain, never hide it.
    pub unavailable_reason: Option<String>,
}
```

Changed:

- `SubmitInput` (`commands.rs:468–481`) gains `pub enhancer: Option<EnhancerChoice>`.
- `SettingsDto.enhance` (`commands.rs:375`) becomes `Option<bool>` — see H4.
- `PresetDto` (`commands.rs:321–327`) gains `is_general: bool`, `forces_enhance: bool`,
  `disables_prompt: bool` (`preset.rs:374`), `camera_slug: Option<String>`, and
  `category_label: String`.
- `list_presets` (`commands.rs:336`) sources from `preset::catalog()` — see H10.

### 8.5 UI state and types

`ui/src/types.ts`:

```ts
export interface GenSettings {
  ...
  /** Tri-state. `null`/absent means untouched, so the per-job-type default stands —
   *  see the field doc on Rust's `EnhanceInputs.user_toggle`. A plain boolean here
   *  makes "off because I chose off" and "off because this model starts off" the same
   *  state, and switching model then silently discards a deliberate choice. */
  enhance?: boolean | null;
}

export type EnhanceReason = "end-frame" | "preset" | "user-toggle" | "job-default";

export interface EnhancePreview {
  enhance: boolean;
  reason: EnhanceReason;
  explanation: string;
  forced: boolean;
  compiledPrompt: string;
  hasUnresolvedSentinel: boolean;
}

export interface JobSet {
  ...
  /** The structured settings `prompt` was compiled from. Rerun restores
   *  `promptParts.scene`, never `prompt` — see hazard H9. */
  promptParts?: { camera?: string | null; preset?: string | null; scene: string } | null;
  enhanceReason?: EnhanceReason | null;
  /** already declared at types.ts:187 — the Rust side just never filled it */
  enhancerVersion?: string | null;
  warnings?: string[];
}
```

`ui/src/api.ts`: `RawJobSet` already reads `enhanced_prompt` (line 356) and
`enhancer_version` (line 375), and `toJobSet` already maps both (lines 431, 448) — **the TS
side has been ready the whole time; only Rust was sending null.** Add `enhance_reason`,
`prompt_parts` and `warnings` to both. Add `enhancePreview()` and `enhancerOptions()`
wrappers following the existing `try/catch → isDesktop()` shape.

`ui/src/components/PromptCard.tsx` — the enhance `Toggle` (lines 85–92) needs:

```tsx
<Toggle
  id="toggle-enhance"
  checked={preview?.enhance ?? false}     // the *effective* value, from Rust
  disabled={preview?.forced ?? false}     // locked when a rule overrode the user
  onChange={onEnhanceChange}
  label="Enhance"
  hint={preview?.explanation}
  note={preview?.forced ? preview.explanation : undefined}   // visible, not sr-only
/>
```

`Toggle` (`Toggle.tsx:43–48`) currently renders `hint` inside `.sr-only`. A forced state must
be *visible*: a switch that visibly disagrees with the outcome and says nothing reads as
broken. That is the stated purpose of `EnhanceReason` existing at all (`enhance.rs:282–284`).

`ui/src/App.tsx` — three changes, all small:

- `DEFAULT_SETTINGS.enhance` (line 61) becomes `null` / omitted, not `true`;
- new `enhancer` state plus a debounced `enhancePreview` effect keyed on
  `[modelId, presetId, prompt, media, settings.enhance]`, mirroring the existing
  `estimateCost` effect at lines 217–236 (200 ms debounce, `live` guard);
- `onRerun` (line 339) restores `job.promptParts?.scene ?? job.prompt`, and restores
  `job.enhancerVersion`'s backend as the selected enhancer if it is still available.

Also worth fixing while in `api.ts`: `submitJob` (line 504–508) reads
`res.job_set_id ?? res.jobSetId ?? ""` from a command that returns a bare `String`
(`commands.rs:484`), so it always resolves to `""`. Harmless today only because `App.tsx:307`
discards the value.

---

## 9. Tests this needs

`BRIDGE.md` §2's standing rule: *any new harness module needs at least one test that enters
through `commands.rs`, not only through the crate's own unit tests.* Named for the bug each
prevents:

Through `commands.rs::submit_job` (with a stub client and a stub rewriter):

1. `a_camera_preset_puts_its_five_slot_chain_in_the_prompt_exactly_once` — H3.
2. `an_unknown_preset_id_is_refused_rather_than_silently_dropped` — H1.
3. `a_general_id_resolves_to_no_preset_instead_of_being_refused` — the `is_general_id` branch.
4. `an_end_frame_skips_the_rewriter_entirely` — rule 2, asserted by the stub never being
   called, so it also proves we do not pay for a rewrite we discard.
5. `a_real_preset_rewrites_even_with_the_toggle_off` — rule 1.
6. `an_untouched_toggle_takes_the_job_type_default_on_an_editing_model` — H4; fails today
   because the DTO cannot express untouched.
7. `the_provider_receives_the_rewrite_and_the_record_keeps_both` — §6.
8. `the_compiled_prompt_survives_a_store_round_trip` — H8(a).
9. `an_empty_rewrite_is_refused_rather_than_submitting_a_promptless_job` — H12.
10. `our_enhance_toggle_never_reaches_the_models_enhance_prompt_flag` — H6; assert across
    all seven models that declare it.

In `rewrite.rs`: the four guards in §8.1, against a fake backend.

In `store.rs`: `a_v3_database_upgrades_and_keeps_its_rows` (mirroring
`a_v1_database_upgrades_without_losing_rows` at line 392), and
`a_second_upsert_persists_the_rewrite_and_the_request_id` — H8(b).

---

## 10. What I could not verify

- **The Tauri v2 main-thread claim (H7)** is inferred from the framework's threading model,
  not observed. Confirm against the running app before treating it as the reason for a
  freeze.
- **Ollama's chat endpoint and payload shape** — deliberately not written down here. Verify
  against a running daemon. `detect_local` only proves `/api/tags` answers.
- **Any hosted LLM endpoint** — none verified, none written.
- **Whether any preset baked param actually conflicts with a UI setting today.** All 25
  shipped families are camera moves with an empty `baked` map, so H5 is a latent bug rather
  than a live one — it becomes live with the first preset pack.
- **The request_id bug (H8b) has not been reproduced**, only read out of the SQL. It is one
  `SELECT request_id` after a submit away from being confirmed or dismissed.
- **Nothing here has been run end to end against a live provider.** `PARITY.md` §3.3 still
  stands: no generation has ever completed.
