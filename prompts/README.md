# `prompts/` — the enhancer corpus

This directory is the filmmaking knowledge Hickeyfield uses when it rewrites a user's prompt
before submitting a generation. It is content, not code: plain Markdown, versioned, immutable
once shipped, and pinned by id on every generation that used it.

It exists because of a specific hole in the product. The harness already models the *decision*
of whether to rewrite — `crates/hickeyfield-core/src/enhance.rs` implements the three rules and
the per-job-type default table, all tested — but there has never been a rewriter behind that
decision. A preset selected in the picker forces enhancement on, precisely because a preset's
look is delivered by the rewrite; with nothing behind it, every preset shipped its name and
none of its effect. These files are that missing half.

---

## Files

| File | Role | Approx. tokens |
|---|---|---|
| `enhancer.v1.md` | Base system prompt: output contract, invariants, shot grammar, camera motion, lens, light, motion and time, anti-patterns | ~8,800 |
| `enhancer.video.v1.md` | Video overlay; its two branches, t2v and i2v | ~4,050 |
| `enhancer.image.v1.md` | Image overlay — stills, with or without reference images | ~2,800 |
| `enhancer.edit.v1.md` | Edit overlay — changing an existing image or clip | ~3,400 |

Token figures are `bytes / 4`, which is a rough English-prose approximation and **not** a
measured count. A call is base + one overlay, so budget roughly 11,500–13,000 tokens of system
prompt. Measure properly before anyone quotes a per-generation cost from them, and note that
the system prompt is byte-identical across submissions by design — it is the ideal prompt-cache
prefix, and on a hosted key that is the difference between this being cheap and being the most
expensive part of a generation.

---

## How a call is assembled

```
system  = enhancer.v1.md  +  exactly one overlay
user    = enhancer::user_message(&EnhanceRequest)   (see §1 of enhancer.v1.md)
assistant returns  the prompt text, plus an optional notes block
```

**Exactly one overlay, always.** Never zero — the base alone has no mode discipline and will
happily write a text-to-video prompt onto an image-to-video job, which is the most expensive
mistake in the whole corpus. Never two — the overlays contradict each other on purpose, and
`enhancer.edit.v1.md` explicitly suspends a base rule that the other two rely on.

**Mode selection is derived, not asked for:**

| Condition | Mode |
|---|---|
| The route's output is a video | `video` |
| The route's output is a still image, and the operation generates rather than modifies | `image` |
| The operation modifies an attached image or clip (inpaint, replace, remove, relight, restyle, outpaint, video edit) | `edit` |
| `Modality::ThreeD`, `Audio` or `Other` | **do not run the rewriter** |

Image-to-video is `video`, not `edit` — it produces a new clip from a still, and the video
overlay has a dedicated branch for it. Video-to-video *editing* is `edit`.

Nothing in this corpus is about meshes or sound, so there is no overlay for them. The loader
should skip enhancement rather than pick the nearest mode; the base file also refuses in that
case, but a refusal is a round trip the user paid for in latency.

### The slot contract, and why the corpus tolerates two wirings

The harness composes a wire prompt from named slots, in a fixed order
(`PromptParts::compile`):

```
camera clause · preset clause · scene · Lighting: … · Lens: … · Mood: …
```

The intended wiring hands the rewriter the **scene alone** and re-composes afterwards, so a
camera move is structurally incapable of being mangled. But `EnhanceRequest.prompt` is
currently documented as receiving `compile()`'s *whole* output, and `Rewritten.prompt` is
submitted directly — under which the rewriter's reply is the entire prompt and anything it
drops is gone.

Those two wirings fail in opposite directions, so the corpus is written to be safe under both.
**B16** requires that any clause arriving with a harness label — `Camera:`, `Movement:`,
`Speed:`, `Framing:`, `End:`, `Lighting:`, `Lens:`, `Mood:` — is reproduced verbatim and in
place, and that no new one is invented. Under slot wiring there are no such clauses and the
rule is inert; under whole-prompt wiring it is what keeps the user's camera setting alive.

This matters because of the failure it prevents: a user sets a dolly-in, the rewriter returns
a scene without it, the render ignores it, and **nothing in the UI says so.** Silently
discarding a control the user set is the failure this repository is explicitly designed
against.

Pick a wiring and pin it. Both are defensible; leaving it ambiguous is not.

### The notes channel is off until something splits it

`Notes: enabled` is an optional line in the context block that turns on a second output
channel — the model appends `===HICKEYFIELD-NOTES===` and one line per rule it applied, so the UI
can tell the user what was traded and why.

**`enhancer::user_message` does not emit that line today, and `enhancer::clean_reply` does not
strip the block.** That combination is safe in exactly one direction: with the line absent, the
corpus tells the model to emit the prompt alone, so nothing is appended and nothing needs
splitting. Sending the line before the splitter exists would submit `===HICKEYFIELD-NOTES===` and
every note to the provider as part of the prompt.

So the ordering is fixed: **build the splitter first, wire the line second.** The splitter cuts
at the first line that is exactly `===HICKEYFIELD-NOTES===`, submits everything before it, and
keeps the rest as structured display text. If the sentinel is absent the whole reply is the
prompt — that must remain the default, because it is what makes a model that ignores the notes
instruction harmless.

---

## How versioning works

**One rule: a shipped file is frozen.** `enhancer.v1.md` will never be edited again once a
generation has referenced it. Not for a typo, not for a clarification, not for "obviously
better" wording. Every change ships as a new file with a new version number, and the old file
stays on disk forever.

That is stricter than it sounds necessary, and the reason is the next section.

### Why the version is pinned per generation

A prompt rewriter is not a formatter. It is a creative step whose output the user pays for,
looks at, keeps, shares and re-runs. If the corpus changes underneath it, three things break:

1. **Rerun stops meaning rerun.** `Recipe` exists so a generation can be reproduced — the
   model, the route, the settings, the media, the prompt. A rewriter that produced *this* text
   from *that* input is part of the generation just as much as the sampler seed is. Re-running
   through a corpus that has since been "improved" gives a different prompt, a different
   output, and a different price, while the UI still says the word rerun.
2. **A shared recipe becomes a lie.** Recipes are the community surface Hickeyfield kept in place
   of a social product. A recipe that cannot name the rewriter that wrote its prompt cannot
   reproduce its own result on someone else's machine.
3. **Nothing can be evaluated.** The only way to know whether a corpus change helped is to
   compare outputs across versions on identical inputs. Mutating files in place destroys the
   control group before the experiment starts.

There is a fourth, quieter reason. Every rule in these files is a claim about how models
behave, and models change. When a claim stops being true — when an endpoint starts honouring
negative concepts, say, or stops degrading on long text — the fix is a new version, and the
old one has to remain readable so the change is auditable. A corpus edited in place has no
history of what it used to believe.

### Where the pin lives

`Recipe::enhancer_version: Option<String>` already exists for exactly this and is currently
`None` on every recipe Hickeyfield writes, with a comment saying it is `None` because no rewriter
exists yet. This corpus is that rewriter.

Recorded format:

```
<corpus-id>/<mode>[+<provider>/<model>]

enhancer.v1/video
enhancer.v1/edit+anthropic/<rewriter-model-id>
```

- `<corpus-id>` is the base file's `id`, which names the whole release. The overlay is implied
  by `<mode>` and is never versioned independently — base and overlays are only ever written
  and tested together, so shipping them on separate version lines would let an untested
  combination reach a user.
- `<mode>` is the overlay that was actually used, recorded rather than re-derived. Mode
  selection depends on route metadata that can change; the recipe must say what happened, not
  what would happen today.
- The optional suffix is the LLM that performed the rewrite. **The same corpus through a
  different model is a different rewriter.** Reproducibility needs both, so record both when
  the field can carry it. Do not fabricate this half — if the harness does not know which
  model ran, omit the suffix rather than guessing a default.
- `None`, not a placeholder string, when no rewrite happened. A hand-typed prompt must never
  look enhanced. This is the same principle as an unknown price being `None` and never zero:
  the field goes into an artefact the user will trust later.

### Adding a version

1. Copy the file, bump the number in the filename, the `id` and the `version` front-matter
   field. Do not touch the old one.
2. Update `pairs_with`/`requires` in the new set so a v2 base and v2 overlays reference each
   other. A v2 base with a v1 overlay is an untested combination; the loader should refuse it
   rather than run it.
3. Leave the old files in the tree. They are small, and a recipe from six months ago is
   entitled to be reproducible.
4. Bump base and overlays together, even if only one changed. The version names a release, not
   a file, and a mixed release cannot be reasoned about.

### What justifies a new version

Anything that can change output: a rule added, removed, renumbered or reworded; an example
changed; a section reordered; the output contract touched. Front-matter `status` may move
between `stable` and `deprecated` in place, because it is metadata the model never sees.

If you are unsure whether a change is output-affecting, it is. Ship the version.

---

## Rule ids

Every enforceable rule has a stable id — `B*` invariants and `O*` output contract in the base,
`V*` video, `I*` image, `D*` edit — and the model cites them in its notes block. Worked-example
labels (`A1`, `IV3`, `E4`, `F2`) are **not** rule ids; nothing cites them. This is not
decoration:

- A note that says `V3: three actions requested, kept the chopping` tells the user what was
  traded and lets the UI link to the reason.
- Tests are named after the rule they defend, in the same spirit as the rest of this codebase:
  a fixture asserting that a two-camera-move prompt comes back with one move and a `V2` note
  is a test named for the bug it prevents.
- Ids are stable across versions where the rule survives. A renumbered rule is a broken
  reference in every note already written, so **append rather than renumber**. `D17` sits below
  the `D13`–`D16` video block for exactly that reason, and the file says so in place.
- Ids are unique within a loaded pair (base + one overlay), which is all that is ever in
  context at once. `I*` image rules and `IV*` video example labels never coexist.

Current coverage: `O1`–`O5`, `B1`–`B16`, `V1`–`V18`, `I1`–`I15`, `D1`–`D17`.

---

## Evaluating a change

There is no evaluation harness yet, and no measurement behind any claim in these files beyond
craft reasoning and observed behaviour. That is stated plainly in §12 of the base file and it
should stay stated until it stops being true.

What a real evaluation needs, when someone builds it:

- A frozen input set — real user prompts, not ones written to flatter the corpus. The failure
  cases in the worked examples are a starting point and nothing more.
- Both versions run on identical inputs, with the rewritten prompt captured but **not**
  submitted. Text comparison is nearly free; generation is not.
- Rule-firing assertions first: does the negative get converted, does the second camera move
  get dropped, does the sentinel survive intact. These are cheap, deterministic and catch
  most regressions.
- Only then, paid side-by-side renders on a small sample, and only for changes claiming
  aesthetic improvement rather than rule compliance.

Until that exists, treat the corpus as what it is: a well-reasoned hypothesis with a version
number, which is precisely why it has one.

---

## Provenance

⚖️ Every line in this directory was written for Hickeyfield.

The *shape* of several ideas here comes from studying how Higgsfield's product behaves — that
a rewriter should be preset-aware, that enhancement should default on for prose-hungry models
and off for instruction-following ones, that an image-to-video prompt needs an explicit
preservation clause, and that a clip holds together best when it contains a single camera
movement paired with a single thing the subject does. Those are functional facts and
uncopyrightable methods, and adopting them is deliberate.

Their wording is not adopted, and none of it is reproduced here. `scripts/lint-provenance.py`
checks every shipped string in this repo against a one-way shingle index of their copy and
fails CI on a match; it passes on this directory, and it must keep passing. If you edit these
files, run it. If it fires, rewrite the line — do not relax the threshold.
