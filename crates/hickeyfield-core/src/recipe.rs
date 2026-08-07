//! Recipes: reading a request back off the wire, and writing one to a file.
//!
//! Everything in this module exists to answer one question the app could not
//! previously answer: *given a request body, what did the user actually ask
//! for?* Rerun, Recreate, "reuse these settings" and recipe import/export all
//! reduce to that, and all of them were blocked on it — the harness could only
//! go forwards.
//!
//! Two directions live here and they are deliberately not symmetric:
//!
//! - [`Inputs::to_provider`] mirrors `submit_to_provider` in the Tauri shell:
//!   media through [`media::bind`], then the prompt, then only the settings the
//!   model's own spec declares.
//! - [`from_provider`] inverts it, using the same two sources of truth — the
//!   [`ModelSpec`] for *which roles this model accepts* and the [`Dialect`] for
//!   *what the endpoint calls them*.
//!
//! ## Round-tripping is lossy by design
//!
//! `Inputs → to_provider → from_provider → Inputs` is **exact for everything
//! the model declares** and drops exactly what the provider would have rejected
//! anyway:
//!
//! - a setting the spec does not declare is dropped, because an unknown key is
//!   a 422 on several providers. `submit_to_provider` already drops it, so the
//!   key never reached the provider and is not part of the request the user
//!   made;
//! - a `null` setting is dropped, as the shell drops it: a null is the absence
//!   of a setting rather than a setting whose value is null;
//! - a prompt is dropped for a model that declares no `prompt` flag;
//! - an empty prompt comes back as `None`, because the wire carries no
//!   difference between "no prompt" and "the empty prompt".
//!
//! The other direction is tight, and it is the one Rerun depends on:
//! `body → from_provider → to_provider → body` reproduces the body **byte for
//! byte** for every key the model declares, whenever the body is one
//! [`Inputs::to_provider`] could have written. A re-run has to be the same
//! request, not a similar one — the user is paying for it again. A *hand*-
//! written body can spell a media key the way the catalogue documents it rather
//! than the way [`media::bind`] emits it; that still decodes to the right role,
//! and then re-emits in the binder's spelling, which is the one the endpoint
//! sees anyway. `role_for_key` explains why the two differ.
//!
//! One thing recovery cannot always restore is the *label* on a piece of media,
//! because [`media::bind`] itself does not always write one. fal spells both
//! [`MediaRole::Video`] and [`MediaRole::VideoReference`] `video_url`; and in a
//! body written in the catalogue's own vocabulary, a bare `image` is the flag
//! that both [`MediaRole::Start`] and [`MediaRole::Reference`] fall back to.
//! Recovery picks the first role in [`ROLE_ORDER`] that the model declares, so
//! the request still round-trips; only the name the UI puts under the thumbnail
//! can change.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::catalog::ModelSpec;
use crate::engine::JobSet;
use crate::media::{self, BindError, Dialect, MediaRef, MediaRole, MediaSource};

/// The one field both dialects spell the same way: `submit_to_provider` sends
/// `prompt` whichever dialect it is speaking.
const PROMPT_KEY: &str = "prompt";

/// Roles in recovery order.
///
/// Two roles can share one wire key — see the module docs — so recovery needs a
/// tie-break, and it has to be a fixed one: a body that decoded to `Start` on
/// Monday and `Reference` on Tuesday would make an exported recipe a different
/// generation depending on when it was opened.
///
/// `Start` before `Reference` is a real judgement: on a model whose only media
/// flag is `image`, that flag *is* the input image, and image-to-video is the
/// dominant mode. The video and audio pairs are a coin flip — fal spells both
/// halves of each `video_url` / `audio_url`, so nothing in the body
/// distinguishes them. Fixed beats clever.
pub const ROLE_ORDER: [MediaRole; 7] = [
    MediaRole::Start,
    MediaRole::End,
    MediaRole::Reference,
    MediaRole::Video,
    MediaRole::VideoReference,
    MediaRole::Audio,
    MediaRole::AudioReference,
];

fn role_index(role: MediaRole) -> usize {
    ROLE_ORDER
        .iter()
        .position(|r| *r == role)
        .expect("ROLE_ORDER lists every MediaRole")
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// A request in the user's vocabulary rather than a provider's.
///
/// The same three things [`crate::engine::JobSet`] persists, minus everything
/// about *this particular run* — no ids, no timestamps, no price, no results.
/// That is what makes it portable: these three fields are meaningful against
/// any model, whereas a request id is meaningful against exactly one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Inputs {
    /// What the user typed, already compiled by [`crate::enhance`] if it was
    /// going to be. `None` means the body carried no prompt at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Non-media parameters, keyed exactly as they go on the wire. A
    /// [`serde_json::Map`] rather than a typed struct because every model
    /// declares its own flag set, and inventing one shared shape for all of
    /// them is how the UI's chip row came to lie about every model.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub settings: Map<String, Value>,
    /// Attachments, as roles. Ordered by [`ROLE_ORDER`] after recovery; order
    /// *within* a role is preserved verbatim, because reference order carries
    /// weight on the models that take several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<MediaRef>,
}

impl Inputs {
    /// A prompt-only request — the text-to-video case.
    pub fn prompt(text: impl Into<String>) -> Self {
        Inputs {
            prompt: Some(text.into()),
            ..Default::default()
        }
    }

    pub fn with_setting(mut self, key: &str, value: Value) -> Self {
        self.settings.insert(key.to_string(), value);
        self
    }

    pub fn with_media(mut self, media: MediaRef) -> Self {
        self.media.push(media);
        self
    }

    /// Build the request body for one endpoint.
    ///
    /// This mirrors `submit_to_provider` in the Tauri shell, in the same order
    /// and with the same three rules, so that a recipe re-submitted through the
    /// shell produces the identical body. The rules:
    ///
    /// 1. media is bound first, by [`media::bind`], which owns every media key;
    /// 2. the prompt is sent only if the model declares one and it is non-empty;
    /// 3. a setting is sent only if the model's spec declares that flag and the
    ///    flag is not a media flag.
    ///
    /// Rule 3 is where the documented loss happens, and it is deliberate: an
    /// undeclared key is a 422 on several providers, and a settings blob that
    /// happens to carry `image` must not overwrite an uploaded frame.
    ///
    /// `slug` is the endpoint family, needed because fal's end-frame key varies
    /// by model family — see [`Dialect`].
    pub fn to_provider(
        &self,
        spec: &ModelSpec,
        model_name: &str,
        dialect: Dialect,
        slug: &str,
    ) -> Result<Map<String, Value>, BindError> {
        let mut body = media::bind(spec, model_name, &self.media, dialect, slug)?;

        if spec.takes_prompt() {
            if let Some(p) = self.prompt.as_deref().filter(|p| !p.is_empty()) {
                body.insert(PROMPT_KEY.to_string(), Value::String(p.to_string()));
            }
        }

        for (key, value) in &self.settings {
            if value.is_null() {
                continue;
            }
            let Some(flag) = spec.flag(key) else { continue };
            if flag.value.is_media() {
                continue;
            }
            body.insert(key.clone(), value.clone());
        }

        Ok(body)
    }
}

/// Recover what the user asked for from a request body.
///
/// The inverse of [`Inputs::to_provider`] and of [`media::bind`]. Infallible on
/// purpose, the same way [`crate::catalog::parse`] is: a body with one key we
/// cannot place is still a body whose prompt, media and other settings are
/// worth handing back. Refusing the whole import over one stray field would be
/// the worse failure — the user loses a recipe they can see is mostly fine.
///
/// Anything not recognised as the prompt or as media becomes a setting, even
/// when the model does not declare it. Keeping it means an imported recipe
/// still shows the user what it was written with; [`Inputs::to_provider`] drops
/// it again on the way out, so nothing undeclared can reach a provider.
///
/// `dialect` must be the one the body was written in. It is deliberately not
/// sniffed from the key spellings: the caller has a route, and the route
/// determines the dialect the same way `submit_to_provider` does — `Catalog`
/// for Higgsfield's own API, `Fal` for everything else. Guessing at an answer
/// we already hold is how a parameter turns into an attachment.
pub fn from_provider(spec: &ModelSpec, body: &Value, dialect: Dialect) -> Inputs {
    let mut out = Inputs::default();
    // `bind` only ever produces an object. Anything else did not come from a
    // request we built, and inventing fields from an array would be a guess.
    let Some(obj) = body.as_object() else {
        return out;
    };

    let mut found: Vec<(MediaRole, MediaSource)> = Vec::new();

    for (key, value) in obj {
        if key == PROMPT_KEY {
            // Recovered even when this model declares no prompt flag: the
            // prompt is the part of a recipe a user most wants to carry to
            // another model. `to_provider` still refuses to send it to a model
            // that cannot take one.
            match value {
                Value::String(text) => out.prompt = Some(text.clone()),
                other => {
                    out.settings.insert(key.clone(), other.clone());
                }
            }
            continue;
        }

        match (role_for_key(spec, dialect, key), sources(value)) {
            (Some(role), Some(items)) => found.extend(items.into_iter().map(|s| (role, s))),
            // A media key holding a number or an object was not written by
            // `bind`, which only ever writes a string or an array of strings.
            // Keeping it as a setting is how we avoid inventing a URL.
            _ => {
                out.settings.insert(key.clone(), value.clone());
            }
        }
    }

    // Stable sort: cross-role order is canonical, within-role order is the
    // order the provider was given, which is the one that changes the output.
    found.sort_by_key(|(role, _)| role_index(*role));
    out.media = found
        .into_iter()
        .map(|(role, source)| MediaRef::new(role, source))
        .collect();
    out
}

/// Which role a wire key came from, or `None` when it is not a media key.
fn role_for_key(spec: &ModelSpec, dialect: Dialect, key: &str) -> Option<MediaRole> {
    match dialect {
        // Two spellings are accepted here, and the reason is worth stating
        // because it looks like belt-and-braces and is not. `bind` decides
        // whether a role is allowed with `flag_for` — which falls back, so on a
        // model whose only media flag is `image` a start frame is allowed — and
        // then names the key with `Dialect::keys`, which does *not* fall back
        // and writes `start_image` regardless. So the key on the wire is often
        // not the flag the model documents. Recovery has to invert what `bind`
        // actually writes, and still read a body written in the catalogue's own
        // vocabulary, or importing one loses the attachment silently.
        Dialect::Catalog => ROLE_ORDER.into_iter().find(|role| {
            let declared = media::flag_for(spec, *role);
            // The role must be one this model accepts at all — the same check
            // `bind` makes before it writes anything.
            declared.is_some()
                && (declared == Some(key)
                    // `keys` ignores the slug for this dialect.
                    || dialect.keys(*role, "").first().is_some_and(|k| *k == key))
        }),
        Dialect::Fal => {
            let candidates = fal_roles_for_key(key);
            candidates
                .iter()
                .copied()
                .find(|role| media::flag_for(spec, *role).is_some())
                // No candidate role is one this model declares — so the body
                // was written for a different model. Still recovered as media,
                // because `to_provider` then refuses it by name ("X does not
                // accept a start frame"). Filing it under settings instead
                // would drop the attachment silently and re-run an
                // image-to-video job as text-to-video at the same price.
                .or_else(|| candidates.first().copied())
        }
    }
}

/// fal's media keys, reversed.
///
/// A hand-written table because fal's names are not derivable from the
/// catalogue's — see [`Dialect`] — and because [`from_provider`] is handed a
/// body without the endpoint slug that [`Dialect::keys`] needs to go forwards.
/// `fal_reverse_table_agrees_with_the_binder_for_every_route_we_ship` pins it
/// to the forward table so the two cannot drift apart in silence.
fn fal_roles_for_key(key: &str) -> &'static [MediaRole] {
    match key {
        // `first_frame_url` is Wan VACE's spelling of the same slot.
        "image_url" | "first_frame_url" => &[MediaRole::Start],
        // Three spellings, one meaning: Kling, MiniMax/Hailuo, Wan VACE.
        "tail_image_url" | "end_image_url" | "last_frame_url" => &[MediaRole::End],
        "image_urls" | "ref_image_urls" => &[MediaRole::Reference],
        // Genuinely ambiguous on the wire: `bind` sends both roles here.
        "video_url" => &[MediaRole::Video, MediaRole::VideoReference],
        "audio_url" => &[MediaRole::Audio, MediaRole::AudioReference],
        _ => &[],
    }
}

/// The media items a wire value holds, or `None` if it holds none.
fn sources(value: &Value) -> Option<Vec<MediaSource>> {
    match value {
        Value::String(s) => Some(vec![source_of(s)]),
        Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().map(source_of))
            .collect::<Option<Vec<_>>>(),
        _ => None,
    }
}

/// Classify one media string.
///
/// A body that reached a provider carries URLs, so everything that is not a
/// `data:` URI is recovered as [`MediaSource::Url`] — never as
/// [`MediaSource::Local`]. A recipe is meant to be shared, and resurrecting the
/// author's filesystem path on someone else's machine would either read the
/// wrong file or fail with a confusing disk error at submit time.
fn source_of(raw: &str) -> MediaSource {
    if raw.starts_with("data:") {
        MediaSource::DataUri {
            data: raw.to_string(),
        }
    } else {
        MediaSource::Url {
            url: raw.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Recipe
// ---------------------------------------------------------------------------

/// The format version [`Recipe`] writes.
///
/// Bumped only when an existing field changes *meaning*. Adding a field does
/// not need it, because [`Recipe::extra`] carries fields an older build has
/// never heard of through unchanged.
pub const RECIPE_VERSION: u32 = 1;

/// A generation, saved.
///
/// This is the shareable artefact — the community surface we kept, in place of
/// the social product we deliberately did not build. It has to survive being
/// pasted into a message, opened by a build older or newer than the one that
/// wrote it, and re-run months later against a route table that has moved.
///
/// Everything here is a stable id or a literal value. Nothing is a display
/// name: SwarmUI persists its preset stack as titles joined by `|||` and
/// silently drops what it cannot resolve on load, so renaming a preset breaks
/// restoration invisibly. Ids only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// See [`RECIPE_VERSION`]. Required: an arbitrary JSON object is not a
    /// recipe, and reading one as if it were would produce a request nobody
    /// asked for.
    pub version: u32,
    /// The logical model id, e.g. `seedance_2_0` — Higgsfield's own
    /// `job_set_type`, which is what [`crate::registry`] keys on.
    pub model_id: String,
    /// A pinned `provider:slug` route, when the author wanted this exact one.
    ///
    /// Optional because a pin is a promise about price and provider that a
    /// recipient may not be able to keep: [`crate::route::resolve`] refuses a
    /// pinned route the user holds no key for. An export flow that is sharing
    /// rather than archiving should clear it and let the recipient's own route
    /// policy choose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_pin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The preset id, not its name — see the type docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub settings: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<MediaRef>,
    /// Which prompt rewriter produced [`Recipe::prompt`], when one did.
    ///
    /// `None` on every recipe Hickeyfield writes today, because no rewriter exists
    /// yet (BRIDGE.md §4 item 8) — so there is no version to record, and a
    /// string invented here would make a hand-typed prompt look enhanced. The
    /// field is present now rather than later because the same prompt through a
    /// different rewriter is a different generation, and a recipe that cannot
    /// name the rewriter cannot reproduce its own output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enhancer_version: Option<String>,
    /// Fields this build does not know about, carried through untouched.
    ///
    /// Without this, opening a recipe written by a newer Hickeyfield and saving it
    /// again would quietly delete whatever that build recorded — the user's own
    /// data, destroyed by an upgrade they did not make.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Recipe {
    /// A recipe for `model_id` from a set of inputs.
    pub fn new(model_id: impl Into<String>, inputs: Inputs) -> Self {
        Recipe {
            version: RECIPE_VERSION,
            model_id: model_id.into(),
            route_pin: None,
            prompt: inputs.prompt,
            preset_id: None,
            settings: inputs.settings,
            media: inputs.media,
            enhancer_version: None,
            extra: BTreeMap::new(),
        }
    }

    /// Export a finished (or running) job.
    ///
    /// Carries `media` deliberately. Without it, Rerun restores an
    /// image-to-video job as text-to-video: a different, cheaper-looking
    /// generation that is not the one the user asked to repeat, charged the
    /// same.
    ///
    /// The route the job actually ran on becomes the pin, because reproducing a
    /// generation means reproducing the provider too — the same logical model
    /// is a different price on each, sometimes by 2x. See [`Recipe::route_pin`]
    /// for when to clear it.
    pub fn from_job(job: &JobSet) -> Self {
        Recipe {
            version: RECIPE_VERSION,
            model_id: job.model_id.clone(),
            route_pin: Some(job.route_id.clone()),
            prompt: Some(job.prompt.clone()).filter(|p| !p.is_empty()),
            preset_id: job.preset_id.clone(),
            // A non-object settings blob is not something submission ever
            // wrote; the shell applies the same rule when building the body.
            settings: job.settings.as_object().cloned().unwrap_or_default(),
            media: job.media.clone(),
            // `JobSet` records the rewritten prompt but not which rewriter
            // produced it, because there is no rewriter yet.
            enhancer_version: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn with_route_pin(mut self, route_id: impl Into<String>) -> Self {
        self.route_pin = Some(route_id.into());
        self
    }

    pub fn with_preset(mut self, preset_id: impl Into<String>) -> Self {
        self.preset_id = Some(preset_id.into());
        self
    }

    /// Drop the route pin, for sharing with someone whose keys we do not know.
    pub fn unpinned(mut self) -> Self {
        self.route_pin = None;
        self
    }

    /// The three fields [`Inputs::to_provider`] needs.
    pub fn inputs(&self) -> Inputs {
        Inputs {
            prompt: self.prompt.clone(),
            settings: self.settings.clone(),
            media: self.media.clone(),
        }
    }

    /// Serialise for a `.json` file or the clipboard.
    ///
    /// Pretty-printed because this lands somewhere a human reads it — a gist, a
    /// diff, a bug report. Key order is deterministic ([`serde_json::Map`] is
    /// ordered), so hashing the output to detect "same recipe" stays valid.
    pub fn to_json(&self) -> Result<String, RecipeError> {
        serde_json::to_string_pretty(self).map_err(|e| RecipeError::Malformed(e.to_string()))
    }

    /// Parse a recipe, refusing one this build cannot read correctly.
    pub fn from_json(text: &str) -> Result<Self, RecipeError> {
        let recipe: Recipe =
            serde_json::from_str(text).map_err(|e| RecipeError::Malformed(e.to_string()))?;
        // [`Recipe::extra`] protects against *added* fields, not against a
        // field whose meaning changed. Only a version bump signals that, and
        // silently re-running a request we have misread is the failure this
        // refusal exists to prevent — it costs real money.
        if recipe.version > RECIPE_VERSION {
            return Err(RecipeError::UnsupportedVersion {
                found: recipe.version,
                supported: RECIPE_VERSION,
            });
        }
        Ok(recipe)
    }
}

/// Why a recipe could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeError {
    /// Not a recipe, or not valid JSON.
    Malformed(String),
    /// Written by a newer Hickeyfield.
    UnsupportedVersion { found: u32, supported: u32 },
}

impl fmt::Display for RecipeError {
    /// Both strings reach the user, so each names the remedy rather than the
    /// internals — "invalid input" sends someone to the issue tracker.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecipeError::Malformed(why) => {
                write!(f, "this file is not a Hickeyfield recipe: {why}")
            }
            RecipeError::UnsupportedVersion { found, supported } => write!(
                f,
                "this recipe is version {found} and this build reads up to {supported} — update Hickeyfield to open it"
            ),
        }
    }
}

impl std::error::Error for RecipeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Arity, FlagSpec, Modality, ValueSpec};
    use crate::job::JobStatus;
    use crate::provider::ProviderId;
    use serde_json::json;

    fn flag(name: &str, value: ValueSpec) -> FlagSpec {
        FlagSpec {
            name: name.to_string(),
            alias: None,
            required: false,
            default: None,
            value,
            arity: Arity::One,
        }
    }

    fn media_flag(name: &str, arity: Arity) -> FlagSpec {
        FlagSpec {
            name: name.to_string(),
            alias: None,
            required: false,
            default: None,
            value: ValueSpec::Media,
            arity,
        }
    }

    fn spec(flags: Vec<FlagSpec>) -> ModelSpec {
        ModelSpec {
            id: "test_model".into(),
            display_name: "Test Model".into(),
            modality: Modality::Video,
            flags,
            constraints: vec![],
        }
    }

    /// A video model shaped like the real ones: prompt, two scalar settings,
    /// a start and end frame, and repeatable references.
    fn video_spec() -> ModelSpec {
        spec(vec![
            flag("prompt", ValueSpec::Text),
            flag("duration", ValueSpec::Integer),
            flag(
                "resolution",
                ValueSpec::Enum(vec!["720p".into(), "1080p".into()]),
            ),
            media_flag("start_image", Arity::One),
            media_flag("end_image", Arity::One),
            media_flag("image_references", Arity::Repeated),
        ])
    }

    fn url(role: MediaRole, u: &str) -> MediaRef {
        MediaRef::url(role, u)
    }

    // ── the round trip ─────────────────────────────────────────────────────

    #[test]
    fn settings_and_media_survive_a_round_trip_through_both_dialects() {
        // Both dialects, because the whole point of `Dialect` is that the wire
        // keys differ — a recovery that only inverts the catalogue's names
        // would silently lose every attachment on fal, which is every provider
        // except Higgsfield's own API.
        let s = video_spec();
        let inputs = Inputs::prompt("a lighthouse in fog")
            .with_setting("duration", json!(5))
            .with_setting("resolution", json!("1080p"))
            .with_media(url(MediaRole::Start, "https://cdn/first.png"))
            .with_media(url(MediaRole::End, "https://cdn/last.png"));

        for (dialect, slug) in [
            (Dialect::Catalog, "higgsfield-ai/dop/standard"),
            (
                Dialect::Fal,
                "fal-ai/kling-video/v2.5-turbo/pro/image-to-video",
            ),
        ] {
            let body = inputs.to_provider(&s, "Test Model", dialect, slug).unwrap();
            let back = from_provider(&s, &Value::Object(body), dialect);
            assert_eq!(back, inputs, "{dialect:?} did not round-trip");
        }
    }

    #[test]
    fn several_references_keep_their_order_through_the_round_trip() {
        // Reference order carries weight on the models that take several, so a
        // recovery that reorders them produces a different generation from the
        // same recipe.
        let s = video_spec();
        let inputs = Inputs::prompt("x")
            .with_media(url(MediaRole::Reference, "https://cdn/1.png"))
            .with_media(url(MediaRole::Reference, "https://cdn/2.png"))
            .with_media(url(MediaRole::Reference, "https://cdn/3.png"));
        let body = inputs
            .to_provider(&s, "Test Model", Dialect::Fal, "fal-ai/whatever")
            .unwrap();
        let back = from_provider(&s, &Value::Object(body), Dialect::Fal);
        assert_eq!(back.media, inputs.media);
    }

    #[test]
    fn a_setting_the_model_does_not_declare_is_dropped_rather_than_sent() {
        // The lossy half of the contract. `seed` is not on this model, and an
        // unknown key is a 422 on several providers — so it never reached the
        // provider in the first place and is not part of the request.
        let s = video_spec();
        let inputs = Inputs::prompt("a cat")
            .with_setting("duration", json!(5))
            .with_setting("seed", json!(1234));

        let body = inputs
            .to_provider(&s, "Test Model", Dialect::Fal, "fal-ai/whatever")
            .unwrap();
        assert!(!body.contains_key("seed"), "got: {body:?}");

        let back = from_provider(&s, &Value::Object(body), Dialect::Fal);
        assert_eq!(back.settings.get("duration"), Some(&json!(5)));
        assert_eq!(back.settings.get("seed"), None);
        assert_eq!(back.prompt.as_deref(), Some("a cat"));
    }

    #[test]
    fn a_wire_body_survives_from_provider_then_to_provider_byte_for_byte() {
        // The property Rerun needs: re-running must submit the same request,
        // not a similar one. The user pays for it again.
        let s = video_spec();
        let body = json!({
            "prompt": "a lighthouse",
            "duration": 5,
            "resolution": "1080p",
            "image_url": "https://cdn/first.png",
            "tail_image_url": "https://cdn/last.png",
            "image_urls": ["https://cdn/a.png", "https://cdn/b.png"],
        });
        let back = from_provider(&s, &body, Dialect::Fal);
        let rebuilt = back
            .to_provider(
                &s,
                "Test Model",
                Dialect::Fal,
                "fal-ai/kling-video/v2.5-turbo/pro/image-to-video",
            )
            .unwrap();
        assert_eq!(
            serde_json::to_string(&Value::Object(rebuilt)).unwrap(),
            serde_json::to_string(&body).unwrap()
        );
    }

    #[test]
    fn a_null_setting_is_not_sent() {
        // Mirrors the shell, which skips nulls when building the body. A null
        // is the absence of a setting, not a setting whose value is null —
        // sending it would assert a choice the user never made.
        let s = video_spec();
        let body = Inputs::prompt("x")
            .with_setting("duration", Value::Null)
            .to_provider(&s, "Test Model", Dialect::Fal, "fal-ai/whatever")
            .unwrap();
        assert!(!body.contains_key("duration"), "got: {body:?}");
    }

    #[test]
    fn a_settings_blob_carrying_a_media_flag_does_not_clobber_an_uploaded_frame() {
        // Media keys belong to the binder. A recipe whose settings happen to
        // carry `start_image` must not overwrite the frame the user attached.
        let s = video_spec();
        let body = Inputs::prompt("x")
            .with_media(url(MediaRole::Start, "https://cdn/real.png"))
            .with_setting("start_image", json!("https://cdn/stale.png"))
            .to_provider(&s, "Test Model", Dialect::Catalog, "higgsfield-ai/dop")
            .unwrap();
        assert_eq!(body["start_image"], "https://cdn/real.png");
    }

    // ── prompt handling ────────────────────────────────────────────────────

    #[test]
    fn an_empty_prompt_is_not_sent_and_comes_back_as_none() {
        let s = video_spec();
        let body = Inputs::prompt("")
            .to_provider(&s, "Test Model", Dialect::Fal, "fal-ai/whatever")
            .unwrap();
        assert!(!body.contains_key("prompt"), "got: {body:?}");
        assert_eq!(
            from_provider(&s, &Value::Object(body), Dialect::Fal).prompt,
            None
        );
    }

    #[test]
    fn a_prompt_is_recovered_from_a_model_that_declares_none_but_not_resent() {
        // Recovery is generous so the text can be carried to another model;
        // submission is strict so a model with no prompt flag never receives
        // one. Both halves matter and they disagree on purpose.
        let promptless = spec(vec![media_flag("image", Arity::One)]);
        let body = json!({ "prompt": "carried over" });
        let back = from_provider(&promptless, &body, Dialect::Fal);
        assert_eq!(back.prompt.as_deref(), Some("carried over"));

        let rebuilt = back
            .to_provider(&promptless, "Test", Dialect::Fal, "fal-ai/whatever")
            .unwrap();
        assert!(!rebuilt.contains_key("prompt"), "got: {rebuilt:?}");
    }

    // ── media recovery ─────────────────────────────────────────────────────

    #[test]
    fn an_end_frame_is_never_recovered_as_a_start_frame() {
        // The mirror of the binder's cardinal rule. Recovering `tail_image_url`
        // as a start frame would re-run the job with the last frame as the
        // first — a plausible-looking video that is not the one being repeated.
        let s = video_spec();
        for (dialect, key) in [
            (Dialect::Catalog, "end_image"),
            (Dialect::Fal, "tail_image_url"),
            (Dialect::Fal, "end_image_url"),
            (Dialect::Fal, "last_frame_url"),
        ] {
            let body = json!({ key: "https://cdn/last.png" });
            let back = from_provider(&s, &body, dialect);
            assert_eq!(
                back.media,
                vec![url(MediaRole::End, "https://cdn/last.png")],
                "{dialect:?}/{key}"
            );
        }
    }

    #[test]
    fn fal_media_keys_come_back_as_the_role_that_produced_them() {
        let s = video_spec();
        let body = json!({
            "image_url": "https://cdn/s.png",
            "image_urls": ["https://cdn/r.png"],
        });
        let back = from_provider(&s, &body, Dialect::Fal);
        assert_eq!(
            back.media,
            vec![
                url(MediaRole::Start, "https://cdn/s.png"),
                url(MediaRole::Reference, "https://cdn/r.png"),
            ]
        );
        assert!(back.settings.is_empty(), "got: {:?}", back.settings);
    }

    #[test]
    fn the_catalogue_dialect_recovers_the_flag_this_model_actually_declares() {
        // What `image` means is a property of the model, not of the key. On a
        // model with a plain `image` flag and no `start_image`, `image` is
        // the start frame. On a model that declares both, `start_image` claims
        // Start and `image` is what a *reference* falls back to — which is
        // exactly where `bind` would have put one. A fixed key→role table would
        // get one of these two wrong.
        let plain = spec(vec![media_flag("image", Arity::One)]);
        let back = from_provider(
            &plain,
            &json!({ "image": "https://cdn/a.png" }),
            Dialect::Catalog,
        );
        assert_eq!(back.media, vec![url(MediaRole::Start, "https://cdn/a.png")]);

        let both = spec(vec![
            media_flag("image", Arity::One),
            media_flag("start_image", Arity::One),
        ]);
        assert_eq!(
            media::flag_for(&both, MediaRole::Reference),
            Some("image"),
            "the binder's own fallback: this is what makes the recovery below right"
        );
        let back = from_provider(
            &both,
            &json!({ "image": "https://cdn/a.png" }),
            Dialect::Catalog,
        );
        assert_eq!(
            back.media,
            vec![url(MediaRole::Reference, "https://cdn/a.png")]
        );
    }

    #[test]
    fn a_data_uri_is_recovered_as_a_data_uri_not_a_url() {
        // `resolve` leaves a data URI in place for providers that take one
        // inline. Recovering it as a URL would make the next submission try to
        // fetch a `data:` string as an endpoint.
        let s = video_spec();
        let back = from_provider(
            &s,
            &json!({ "image_url": "data:image/png;base64,AAAA" }),
            Dialect::Fal,
        );
        assert_eq!(
            back.media,
            vec![MediaRef::new(
                MediaRole::Start,
                MediaSource::DataUri {
                    data: "data:image/png;base64,AAAA".into()
                }
            )]
        );
    }

    #[test]
    fn a_local_path_in_a_body_is_recovered_as_a_url_not_as_a_file() {
        // Recipes are shared. Recovering a bare string as `Local` would point
        // at the author's disk on the recipient's machine — either the wrong
        // file or a confusing read error, both worse than the provider saying
        // it cannot fetch the input.
        let s = video_spec();
        let back = from_provider(&s, &json!({ "image_url": "/Users/x/a.png" }), Dialect::Fal);
        assert_eq!(back.media, vec![url(MediaRole::Start, "/Users/x/a.png")]);
    }

    #[test]
    fn a_media_key_holding_a_number_is_kept_as_a_setting_rather_than_invented_into_a_url() {
        let s = video_spec();
        let back = from_provider(&s, &json!({ "image_url": 7 }), Dialect::Fal);
        assert!(back.media.is_empty());
        assert_eq!(back.settings.get("image_url"), Some(&json!(7)));
    }

    #[test]
    fn media_for_a_model_that_cannot_take_it_is_recovered_so_submission_refuses_by_name() {
        // Importing an image-to-video recipe onto a text-only model must fail
        // loudly. Dropping the attachment instead would run a text-to-video
        // job the user did not ask for, at the same price.
        let text_only = spec(vec![flag("prompt", ValueSpec::Text)]);
        let back = from_provider(
            &text_only,
            &json!({ "prompt": "x", "image_url": "https://cdn/a.png" }),
            Dialect::Fal,
        );
        assert_eq!(back.media.len(), 1);
        let err = back
            // A *measured* text-only endpoint. `fal-ai/whatever` is unmeasured
            // and therefore permissive, which would have made this pass for the
            // wrong reason.
            .to_provider(&text_only, "Veo 3.1 Text", Dialect::Fal, "fal-ai/veo3.1")
            .unwrap_err();
        assert!(err.to_string().contains("Veo 3.1 Text"), "got: {err}");
    }

    #[test]
    fn a_non_object_body_recovers_nothing_rather_than_panicking() {
        let s = video_spec();
        for body in [json!([1, 2, 3]), json!("nope"), Value::Null] {
            assert_eq!(from_provider(&s, &body, Dialect::Fal), Inputs::default());
        }
    }

    // ── drift guards against the binder ────────────────────────────────────

    #[test]
    fn fal_reverse_table_agrees_with_the_binder_for_every_route_we_ship() {
        // The reverse table is hand-written and the forward one is not, so
        // nothing but this holds them together. The fixpoint: whatever role a
        // key decodes to must re-encode to that same key. If `media::fal_keys`
        // grows a branch — a new model family with its own end-frame spelling —
        // this fails instead of the key silently becoming a setting and the
        // attachment vanishing from every imported recipe.
        let mut slugs: Vec<String> = crate::registry::registry()
            .values()
            .flat_map(|m| m.routes.iter())
            .filter(|r| r.provider == ProviderId::Fal)
            .map(|r| r.slug.clone())
            .collect();
        assert!(
            slugs.len() > 20,
            "expected many fal routes, got {}",
            slugs.len()
        );
        // The three families whose end-frame key differs, in case the registry
        // ever stops shipping one of them.
        slugs.extend([
            "fal-ai/kling-video/v2.5-turbo/pro/image-to-video".to_string(),
            "fal-ai/minimax/hailuo-02/standard/image-to-video".to_string(),
            "fal-ai/wan-vace-14b".to_string(),
        ]);

        for slug in &slugs {
            for role in ROLE_ORDER {
                let key = Dialect::Fal.keys(role, slug)[0];
                let decoded = *fal_roles_for_key(key).first().unwrap_or_else(|| {
                    panic!("{key} (from {role:?} on {slug}) is not in the reverse table")
                });
                assert_eq!(
                    Dialect::Fal.keys(decoded, slug)[0],
                    key,
                    "{key} on {slug} decodes to {decoded:?}, which re-encodes to something else"
                );
            }
        }
    }

    #[test]
    fn catalogue_recovery_agrees_with_the_binder_for_every_model_we_ship() {
        // Same fixpoint on the other dialect, over the real vendored specs —
        // which is where the aliasing lives (`--video-references` is also
        // spelled `--video`, so two roles resolve through one FlagSpec).
        //
        // Both spellings are checked per role, because they are not the same
        // string: `bind` writes `Dialect::keys`, while the model documents what
        // `flag_for` resolved. Either one arriving in a body must come back as
        // media, and the written one must re-encode to itself.
        let reg = crate::registry::registry();
        let mut checked = 0;
        for model in reg.values() {
            for role in ROLE_ORDER {
                let Some(declared) = media::flag_for(&model.spec, role) else {
                    continue; // `bind` refuses this role on this model anyway.
                };
                let written = Dialect::Catalog.keys(role, "")[0];

                let decoded = role_for_key(&model.spec, Dialect::Catalog, written)
                    .unwrap_or_else(|| panic!("{}: {written} decoded to nothing", model.id));
                assert_eq!(
                    Dialect::Catalog.keys(decoded, "")[0],
                    written,
                    "{}: {written} decodes to {decoded:?}, which re-encodes to something else",
                    model.id
                );
                assert!(
                    role_for_key(&model.spec, Dialect::Catalog, declared).is_some(),
                    "{}: {declared}, the flag this model documents for {role:?}, decoded to \
                     nothing — a body written in the catalogue's own vocabulary would lose it",
                    model.id
                );
                checked += 1;
            }
        }
        assert!(checked > 50, "expected many media flags, checked {checked}");
    }

    #[test]
    fn whatever_key_the_binder_writes_for_a_start_frame_decodes_back_to_one() {
        // `bind` allows a start frame on a model whose only media flag is
        // `image` (that is the documented fallback) but still names the key
        // `start_image`, because `Dialect::keys` does not fall back. Recovery
        // must invert what the binder writes today and keep working if the two
        // are ever reconciled — so this asserts the property, not the spelling.
        let plain = spec(vec![media_flag("image", Arity::One)]);
        let attached = [url(MediaRole::Start, "https://cdn/a.png")];
        let body = media::bind(&plain, "T", &attached, Dialect::Catalog, "").unwrap();
        let back = from_provider(&plain, &Value::Object(body), Dialect::Catalog);
        assert_eq!(back.media, attached.to_vec());

        // And the catalogue's own spelling — what Higgsfield's CLI documents
        // for that model — decodes the same way.
        let back = from_provider(
            &plain,
            &json!({ "image": "https://cdn/a.png" }),
            Dialect::Catalog,
        );
        assert_eq!(back.media, attached.to_vec());
    }

    #[test]
    fn a_boolean_flag_named_image_stays_a_setting() {
        // At least one model declares `image` as a boolean "return an image
        // too" flag rather than an upload. Recovering it as an attachment would
        // turn a checkbox into a file — and drop the setting the user chose.
        let s = spec(vec![
            flag("prompt", ValueSpec::Text),
            flag("image", ValueSpec::Boolean),
        ]);
        let back = from_provider(
            &s,
            &json!({ "prompt": "x", "image": true }),
            Dialect::Catalog,
        );
        assert!(back.media.is_empty());
        assert_eq!(back.settings.get("image"), Some(&json!(true)));
        assert_eq!(
            back.to_provider(&s, "T", Dialect::Catalog, "").unwrap()["image"],
            json!(true)
        );
    }

    #[test]
    fn a_real_launch_model_round_trips_its_own_body() {
        // Enters through the registry rather than a hand-built spec, so a
        // change to the vendored catalogue that breaks recovery is caught here
        // and not by a user whose imported recipe lost its start frame.
        let reg = crate::registry::registry();
        // Deliberately NOT veo3_1, which this test used to use: fal's Veo
        // endpoint accepts no media at all — measured 2026-08-05 — so asserting
        // a start frame round-trips through it was testing behaviour that fails
        // in production. Kling 3.0 really does serve image-to-video.
        let model = reg.get("kling3_0").expect("kling3_0 is in the registry");
        // The real slug, read from the route table rather than transcribed, so
        // this keeps testing the endpoint we would actually POST to.
        let slug = &model
            .routes
            .iter()
            .find(|r| r.provider == ProviderId::Fal)
            .expect("kling3_0 has a fal route")
            .slug;
        let inputs = Inputs::prompt("a lighthouse in fog")
            .with_setting("aspect_ratio", json!("16:9"))
            .with_media(url(MediaRole::Start, "https://cdn/first.png"));
        let body = inputs
            .to_provider(&model.spec, &model.display_name, Dialect::Fal, slug)
            .unwrap();
        assert_eq!(body["image_url"], "https://cdn/first.png");
        assert_eq!(
            from_provider(&model.spec, &Value::Object(body), Dialect::Fal),
            inputs
        );
    }

    // ── Recipe ─────────────────────────────────────────────────────────────

    fn recipe() -> Recipe {
        Recipe::new(
            "seedance_2_0",
            Inputs::prompt("a lighthouse in fog")
                .with_setting("duration", json!(5))
                .with_media(url(MediaRole::Start, "https://cdn/first.png")),
        )
        .with_route_pin("fal:fal-ai/bytedance/seedance/v2")
        .with_preset("camera:dolly-in")
    }

    #[test]
    fn a_recipe_round_trips_through_json() {
        let r = recipe();
        assert_eq!(Recipe::from_json(&r.to_json().unwrap()).unwrap(), r);
    }

    #[test]
    fn an_unknown_setting_survives_to_json_and_from_json() {
        // A recipe is a document, not a request. Settings are filtered against
        // the model's spec at submission time, by `to_provider` — filtering
        // them again at rest would mean switching to a model that *does*
        // declare the key could never recover it.
        let mut r = recipe();
        r.settings
            .insert("sampler_steps".into(), json!({ "n": 20, "why": "leaked" }));
        let back = Recipe::from_json(&r.to_json().unwrap()).unwrap();
        assert_eq!(
            back.settings.get("sampler_steps"),
            Some(&json!({ "n": 20, "why": "leaked" }))
        );
    }

    #[test]
    fn an_unknown_top_level_field_survives_to_json_and_from_json() {
        // Written by a newer build. Dropping it would mean opening someone's
        // recipe and saving it silently deleted data this build never saw.
        let text = r#"{
            "version": 1,
            "model_id": "seedance_2_0",
            "prompt": "a lighthouse",
            "character_id": "chr_7",
            "future": { "nested": [1, 2] }
        }"#;
        let r = Recipe::from_json(text).unwrap();
        assert_eq!(r.extra.get("character_id"), Some(&json!("chr_7")));

        let back = Recipe::from_json(&r.to_json().unwrap()).unwrap();
        assert_eq!(back.extra.get("character_id"), Some(&json!("chr_7")));
        assert_eq!(back.extra.get("future"), Some(&json!({ "nested": [1, 2] })));
        assert_eq!(back, r);
    }

    #[test]
    fn a_recipe_from_the_future_is_refused_rather_than_half_read() {
        // `extra` covers added fields; it cannot cover a field whose meaning
        // changed. Half-reading one submits a request nobody authored, and the
        // user is charged for it.
        let text = format!(
            r#"{{ "version": {}, "model_id": "x" }}"#,
            RECIPE_VERSION + 1
        );
        let err = Recipe::from_json(&text).unwrap_err();
        assert!(
            matches!(err, RecipeError::UnsupportedVersion { .. }),
            "got {err:?}"
        );
        assert!(err.to_string().contains("update Hickeyfield"), "got: {err}");
    }

    #[test]
    fn json_that_is_not_a_recipe_is_refused() {
        // No version field: an arbitrary object must not decode into a recipe
        // whose every field is a default.
        assert!(matches!(
            Recipe::from_json(r#"{ "hello": "world" }"#),
            Err(RecipeError::Malformed(_))
        ));
        assert!(matches!(
            Recipe::from_json("not json at all"),
            Err(RecipeError::Malformed(_))
        ));
    }

    #[test]
    fn recipe_json_is_byte_stable_across_serialisations() {
        // Recipe hashes and "is this the same recipe?" both depend on it.
        let r = recipe();
        assert_eq!(r.to_json().unwrap(), r.to_json().unwrap());
    }

    #[test]
    fn a_recipe_carries_no_enhancer_version_rather_than_an_invented_one() {
        // There is no rewriter yet. A version string here would make a
        // hand-typed prompt look enhanced, and the recipe would claim to
        // reproduce something it cannot.
        assert_eq!(recipe().enhancer_version, None);
        assert!(
            !recipe().to_json().unwrap().contains("enhancer_version"),
            "an unknown value must be absent, not null or empty"
        );
    }

    #[test]
    fn exporting_a_job_carries_the_media_so_rerun_does_not_downgrade_i2v_to_t2v() {
        // The named bug: without media, Rerun restores an image-to-video job as
        // text-to-video — a different generation, charged the same.
        let job = JobSet {
            id: "j1".into(),
            endpoint: String::new(),
            model_id: "seedance_2_0".into(),
            route_id: "fal:fal-ai/bytedance/seedance/v2".into(),
            request_id: "req-1".into(),
            status: JobStatus::Completed,
            prompt: "a lighthouse".into(),
            enhanced_prompt: None,
            enhancer_version: None,
            enhance_note: None,
            advisories: Vec::new(),
            preset_id: Some("camera:dolly-in".into()),
            created_at: 0,
            updated_at: 0,
            results: vec![],
            estimated_usd: Some(0.67),
            actual_usd: None,
            fail_reason: None,
            settings: json!({ "duration": 5 }),
            media: vec![url(MediaRole::Start, "https://cdn/first.png")],
        };
        let r = Recipe::from_job(&job);
        assert_eq!(r.media, job.media);
        assert_eq!(r.model_id, "seedance_2_0");
        assert_eq!(r.preset_id.as_deref(), Some("camera:dolly-in"));
        assert_eq!(r.settings.get("duration"), Some(&json!(5)));
        assert_eq!(
            r.route_pin.as_deref(),
            Some("fal:fal-ai/bytedance/seedance/v2"),
            "the route that ran is what reproduces the price"
        );
        assert_eq!(r.clone().unpinned().route_pin, None);
        // And the exported recipe is submittable again without a translation
        // step: its inputs are exactly what `to_provider` takes.
        assert_eq!(r.inputs().media, job.media);
    }

    #[test]
    fn exporting_a_promptless_job_does_not_record_an_empty_prompt() {
        // `JobSet::prompt` is a String, so "no prompt" arrives as "". Writing
        // that into a recipe would send an empty prompt on the re-run, which
        // some providers reject outright.
        let mut job = JobSet {
            id: "j1".into(),
            endpoint: String::new(),
            model_id: "m".into(),
            route_id: "fal:x".into(),
            request_id: "r".into(),
            status: JobStatus::Completed,
            prompt: String::new(),
            enhanced_prompt: None,
            enhancer_version: None,
            enhance_note: None,
            advisories: Vec::new(),
            preset_id: None,
            created_at: 0,
            updated_at: 0,
            results: vec![],
            estimated_usd: None,
            actual_usd: None,
            fail_reason: None,
            settings: Value::Null,
            media: vec![],
        };
        job.settings = Value::Null;
        let r = Recipe::from_job(&job);
        assert_eq!(r.prompt, None);
        assert!(
            r.settings.is_empty(),
            "a null settings blob is not an object"
        );
    }
}
