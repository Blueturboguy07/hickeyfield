//! Input media: what the user attached, and which wire field it becomes.
//!
//! This is the layer that makes image-to-video possible, which matters because
//! i2v is the dominant mode — Higgsfield's own empty state reads
//! *ADD IMAGE → CHOOSE PRESET → GET VIDEO*. A generator that only does
//! text-to-video is a different, much smaller product.
//!
//! Two problems are solved here and they are genuinely separate:
//!
//! 1. **Binding.** The user thinks in roles ("this is my start frame"). The
//!    provider thinks in flags (`start_image`, `image`, `image_references`).
//!    Which flag a role lands in depends on the model: a model with no
//!    `start_image` flag but a plain `image` flag still does i2v perfectly
//!    well, and refusing it because the exact flag is missing would be wrong.
//!
//! 2. **Reach.** Most providers need a *public URL*, not bytes. Resolving a
//!    local file into something the provider can fetch is [`Uploader`]'s job,
//!    kept behind a trait so the binding logic above is testable without a
//!    network and so each provider's upload endpoint can differ.
//!
//! The flag names are not invented. They are the ones in the vendored MIT
//! `MODELS.md`, by frequency: `image` (32 models), `image_references` (32),
//! `start_image` (19), `end_image` (13), `video` (10), `video_references` (8),
//! `audio` (8), `audio_references` (8).

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::catalog::{Arity, ModelSpec};
use crate::provider::ProviderId;

/// What a piece of attached media is *for*.
///
/// Roles, not flags. The user attaches a start frame; whether that becomes
/// `start_image` or `image` on the wire is [`bind`]'s problem, not theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaRole {
    /// The input image, or the first frame of an interpolation.
    Start,
    /// The last frame. Its presence unconditionally disables prompt
    /// enhancement — see [`crate::enhance`] — because rewriting the prompt
    /// between two fixed frames produces something that matches neither.
    End,
    /// Style, character or subject reference. Usually repeatable.
    Reference,
    /// A video to transform (v2v, motion transfer, recast).
    Video,
    /// A video used as reference rather than as the thing being transformed.
    VideoReference,
    /// Speech or music driving the generation (lipsync, dub).
    Audio,
    /// Audio used as a reference — voice cloning rather than lipsync.
    AudioReference,
}

impl MediaRole {
    /// Every role, for callers that must ask about all of them — the picker
    /// asks "which of these slots can this model+route actually take".
    pub const ALL: [MediaRole; 7] = [
        MediaRole::Start,
        MediaRole::End,
        MediaRole::Reference,
        MediaRole::Video,
        MediaRole::VideoReference,
        MediaRole::Audio,
        MediaRole::AudioReference,
    ];

    /// The wire name, matching this enum's serde spelling and the strings the
    /// UI keys its slot table on.
    pub fn slug(self) -> &'static str {
        match self {
            MediaRole::Start => "start",
            MediaRole::End => "end",
            MediaRole::Reference => "reference",
            MediaRole::Video => "video",
            MediaRole::VideoReference => "video_reference",
            MediaRole::Audio => "audio",
            MediaRole::AudioReference => "audio_reference",
        }
    }

    /// The input mode attaching this role implies.
    ///
    /// A start frame or a reference still makes a request image-driven; a
    /// source clip makes it video-driven. This is what lets a fal route be
    /// judged against the modes it actually serves rather than against
    /// Higgsfield's catalogue.
    pub fn implies_mode(self) -> InputMode {
        match self {
            MediaRole::Video | MediaRole::VideoReference => InputMode::Video,
            _ => InputMode::Image,
        }
    }

    /// Wire flags this role may bind to, best first.
    ///
    /// The ordering is the whole design. `Start` prefers `start_image` because
    /// a model offering both means something specific by it, but falls back to
    /// `image` so the 32 models with only a plain `image` flag still do i2v.
    /// `End` has no fallback on purpose: there is no way to express "final
    /// frame" through a flag that means "input image", and silently binding it
    /// there would produce a generation that ignores the user's end frame
    /// while looking like it worked.
    pub fn candidate_flags(self) -> &'static [&'static str] {
        match self {
            MediaRole::Start => &["start_image", "image"],
            MediaRole::End => &["end_image"],
            MediaRole::Reference => &["image_references", "image"],
            MediaRole::Video => &["video"],
            MediaRole::VideoReference => &["video_references", "video"],
            MediaRole::Audio => &["audio"],
            MediaRole::AudioReference => &["audio_references", "audio"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MediaRole::Start => "start frame",
            MediaRole::End => "end frame",
            MediaRole::Reference => "reference",
            MediaRole::Video => "video",
            MediaRole::VideoReference => "video reference",
            MediaRole::Audio => "audio",
            MediaRole::AudioReference => "audio reference",
        }
    }
}

/// Whose parameter vocabulary a request is written in.
///
/// This exists because the catalogue and the endpoint disagree. Our
/// [`ModelSpec`] is parsed from Higgsfield's CLI spec, which says `image`,
/// `start_image`, `end_image`. **fal wants entirely different names**, verified
/// against seven live endpoint schemas on 2026-08-04:
///
/// | role | Higgsfield CLI | fal |
/// |---|---|---|
/// | start | `start_image` / `image` | `image_url` |
/// | end | `end_image` | `tail_image_url` *(Kling)*, `end_image_url` *(MiniMax/Hailuo)*, `last_frame_url` *(Wan VACE)* |
/// | reference | `image_references` | `image_urls`, `ref_image_urls` *(Wan VACE)* |
/// | video | `video` | `video_url` |
/// | audio | `audio` | `audio_url` |
///
/// Posting the catalogue's names to fal is a 422 on every image-to-video call,
/// which is exactly what this app did before this type existed.
///
/// Note the end-frame row: fal is not internally consistent, so the mapping
/// cannot be one table per provider — it needs the endpoint slug too. The
/// durable fix is to read fal's published per-endpoint OpenAPI
/// (`fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`, unauthenticated) and
/// cache it, the same way prices should be fetched rather than transcribed.
/// Until then this table carries only mappings verified against that schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// The model spec's own flag names. Correct for Higgsfield's own API, whose
    /// CLI the spec documents.
    Catalog,
    /// fal's queue API.
    Fal,
}

impl Dialect {
    /// Wire keys for this role, best first.
    ///
    /// `slug` is the endpoint family, needed only because fal's end-frame key
    /// varies by model family.
    pub fn keys(self, role: MediaRole, slug: &str) -> &'static [&'static str] {
        match self {
            Dialect::Catalog => role.candidate_flags(),
            Dialect::Fal => fal_keys(role, slug),
        }
    }

    /// Whether a key of this name takes an array.
    ///
    /// fal signals plurality in the name itself, which is more reliable than
    /// the catalogue's arity here because the catalogue describes a different
    /// API's shape.
    pub fn is_array_key(self, key: &str) -> bool {
        match self {
            Dialect::Fal => key.ends_with("_urls"),
            Dialect::Catalog => false,
        }
    }
}

fn fal_keys(role: MediaRole, slug: &str) -> &'static [&'static str] {
    // Wan VACE renames three roles at once, so it is matched before the rest.
    let vace = slug.contains("vace");
    match role {
        MediaRole::Start => {
            if vace {
                &["first_frame_url"]
            } else {
                &["image_url"]
            }
        }
        MediaRole::End => {
            // Measured across every video endpoint in the registry on
            // 2026-08-28 by reading fal's own schemas. The previous default was
            // `tail_image_url`, commented "Kling's spelling, and the most
            // common one" — it is neither. Exactly ONE endpoint uses it
            // (`kling-video/v2.5-turbo`); Kling o1, 2.6, v3 and o3 all say
            // `end_image_url`, as do both Seedance lines, Hailuo and Wan 2.7.
            // So every model but one silently lost its end frame: fal drops a
            // key it does not declare, and the user is billed for a
            // start-frame-only render they did not ask for.
            if vace {
                &["last_frame_url"]
            } else if slug.contains("v2.5-turbo") {
                &["tail_image_url"]
            } else {
                &["end_image_url"]
            }
        }
        MediaRole::Reference => {
            if vace {
                &["ref_image_urls"]
            } else {
                &["image_urls"]
            }
        }
        MediaRole::Video | MediaRole::VideoReference => &["video_url"],
        MediaRole::Audio | MediaRole::AudioReference => &["audio_url"],
    }
}

/// Where the bytes currently are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaSource {
    /// A file on the user's machine. Needs an [`Uploader`] before most
    /// providers can see it.
    Local { path: String },
    /// Already fetchable by the provider — a previous generation's output, or
    /// something the user pasted.
    Url { url: String },
    /// Inline `data:` URI. Some endpoints take this directly, which avoids a
    /// round trip; [`Uploader::accepts_data_uri`] decides.
    DataUri { data: String },
}

impl MediaSource {
    /// True when this is already something a provider can fetch.
    pub fn is_reachable(&self) -> bool {
        matches!(self, MediaSource::Url { .. })
    }
}

/// One attached input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRef {
    pub role: MediaRole,
    pub source: MediaSource,
}

impl MediaRef {
    pub fn new(role: MediaRole, source: MediaSource) -> Self {
        MediaRef { role, source }
    }

    pub fn url(role: MediaRole, url: impl Into<String>) -> Self {
        MediaRef::new(role, MediaSource::Url { url: url.into() })
    }

    pub fn local(role: MediaRole, path: impl Into<String>) -> Self {
        MediaRef::new(role, MediaSource::Local { path: path.into() })
    }
}

/// Why a piece of media could not be attached to this model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindError {
    /// The model declares no flag this role could bind to.
    UnsupportedRole { role: MediaRole, model: String },
    /// More items than the flag's arity allows.
    TooMany {
        role: MediaRole,
        flag: String,
        max: u32,
        got: usize,
    },
}

impl fmt::Display for BindError {
    /// These strings reach the user, so they name the model and say what to do.
    /// "invalid input" would send someone to the issue tracker.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BindError::UnsupportedRole { role, model } => write!(
                f,
                "{model} does not accept a {} — remove it or pick a different model",
                role.label()
            ),
            BindError::TooMany { role, max, got, .. } => write!(
                f,
                "at most {max} {}{} allowed here, but {got} were attached",
                role.label(),
                if *max == 1 { "" } else { "s" }
            ),
        }
    }
}

impl std::error::Error for BindError {}

/// Turns local files into something a provider can fetch.
///
/// A trait rather than a concrete client because every provider does this
/// differently — fal has `fal.storage.upload`, Higgsfield issues a signed PUT
/// via `POST /files/generate-upload-url` — and because binding must stay
/// testable without a network.
pub trait Uploader: Send + Sync {
    /// Upload local bytes and return a URL the provider can fetch.
    fn upload(&self, path: &str) -> Result<String, String>;

    /// Whether this provider takes a `data:` URI inline. Overriding to `true`
    /// saves a round trip; the default is the safe answer, because sending a
    /// data URI to an endpoint that wants a URL is a 422 rather than a
    /// helpful error.
    fn accepts_data_uri(&self) -> bool {
        false
    }
}

/// Resolve every source to something the provider can actually fetch.
///
/// Kept separate from [`bind`] so that binding — the part with the interesting
/// logic — needs no network in tests, and so a caller can resolve once and bind
/// to several candidate models when comparing routes.
pub fn resolve(media: &[MediaRef], uploader: &dyn Uploader) -> Result<Vec<MediaRef>, String> {
    media
        .iter()
        .map(|m| {
            let source = match &m.source {
                // Already fetchable. Re-uploading would cost time and, on some
                // providers, money.
                MediaSource::Url { .. } => m.source.clone(),
                MediaSource::DataUri { data } => {
                    if uploader.accepts_data_uri() {
                        m.source.clone()
                    } else {
                        MediaSource::Url {
                            url: uploader.upload(data)?,
                        }
                    }
                }
                MediaSource::Local { path } => MediaSource::Url {
                    url: uploader.upload(path)?,
                },
            };
            Ok(MediaRef {
                role: m.role,
                source,
            })
        })
        .collect()
}

/// Which vocabulary a route speaks.
///
/// Was written out by hand in three places — the submit path, the picker
/// filter and the capability lookup — which is exactly how the three drift.
/// The rule: fal everywhere, plus Higgsfield's fal-shaped mirror; the
/// catalogue only for Higgsfield's own in-house surfaces.
pub fn dialect_for(provider: ProviderId, slug: &str) -> Dialect {
    match provider {
        ProviderId::Higgsfield if !higgsfield_speaks_fal(slug) => Dialect::Catalog,
        _ => Dialect::Fal,
    }
}

/// Can this model take this role, in this dialect?
///
/// **The authority differs by dialect, and using the wrong one fails in both
/// directions.** The catalogue describes Higgsfield's API:
///
/// - Trusting it for a fal route hides real capability. Wan 2.2's fal
///   `/video-to-video` endpoint takes a clip; Higgsfield's catalogue entry
///   never declared one, so a catalogue check excluded the one model verified
///   live to edit video.
/// - Ignoring it for a Higgsfield route would invent capability, since there
///   the catalogue *is* the spec.
///
/// So for fal we defer: the endpoint served the requested input mode (checked
/// by [`resolve_endpoint`]), and `fal_schema` refuses attachments the endpoint
/// has no field for at submit. That is a stronger check than the catalogue and
/// it reads the right document.
pub fn can_bind(spec: &ModelSpec, role: MediaRole, dialect: Dialect, slug: &str) -> bool {
    match dialect {
        Dialect::Catalog => flag_for(spec, role).is_some(),
        // The measured mode table, not the catalogue. `&[]` means the slug is
        // a complete endpoint whose modes we have not enumerated — permissive,
        // because the schema gate at submit is the backstop and refusing an
        // unmeasured endpoint would hide working models.
        Dialect::Fal => {
            if takes_no_media(slug) {
                return false;
            }
            // Measured to have nowhere to put a second frame. Refusing here is
            // the only thing standing between the user and a full-price render
            // that quietly ignored the file they attached.
            if role == MediaRole::End && takes_no_end_frame(slug) {
                return false;
            }
            match modes_for(slug) {
                Some([]) | None => true,
                Some(modes) => modes.contains(&role.implies_mode()),
            }
        }
    }
}

/// Which wire flag this role binds to on this model, if any.
pub fn flag_for(spec: &ModelSpec, role: MediaRole) -> Option<&str> {
    role.candidate_flags()
        .iter()
        .copied()
        .find(|name| spec.flag(name).is_some_and(|f| f.value.is_media()))
}

/// Bind resolved media onto the wire fields the *endpoint* expects.
///
/// Two things are decided per role, and they come from different places on
/// purpose:
///
/// - **Whether the model accepts this role at all** — from the catalogue spec,
///   which is the only description of the model's capabilities we have.
/// - **What the field is called** — from the [`Dialect`], because the
///   catalogue describes Higgsfield's CLI and we are usually talking to fal.
///
/// Roles are grouped before binding so two references land in one array rather
/// than overwriting each other.
pub fn bind(
    spec: &ModelSpec,
    model_name: &str,
    media: &[MediaRef],
    dialect: Dialect,
    slug: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, BindError> {
    use serde_json::Value;

    let mut out = serde_json::Map::new();
    if media.is_empty() {
        return Ok(out);
    }

    // Preserve the order the user attached things in, but group by role. A
    // BTreeMap on the role's discriminant would reorder references, which is
    // visible in the output for models where reference order carries weight.
    let mut roles: Vec<MediaRole> = Vec::new();
    for m in media {
        if !roles.contains(&m.role) {
            roles.push(m.role);
        }
    }

    for role in roles {
        let items: Vec<&MediaRef> = media.iter().filter(|m| m.role == role).collect();

        // Capability, from whichever document is authoritative here.
        if !can_bind(spec, role, dialect, slug) {
            return Err(BindError::UnsupportedRole {
                role,
                model: model_name.to_string(),
            });
        }
        // Arity still comes from the catalogue when it has an opinion; a fal
        // route with no catalogue entry for the role falls back to One, and the
        // key name itself decides array-ness below.
        let arity = flag_for(spec, role)
            .and_then(|f| spec.flag(f))
            .map(|f| f.arity)
            .unwrap_or(Arity::One);

        let key = dialect.keys(role, slug).first().copied().ok_or_else(|| {
            BindError::UnsupportedRole {
                role,
                model: model_name.to_string(),
            }
        })?;

        // Plurality: trust the dialect's own signal where it has one (fal says
        // it in the key name), and fall back to the catalogue's arity.
        let wants_array = match dialect {
            Dialect::Fal => dialect.is_array_key(key),
            Dialect::Catalog => !matches!(arity, Arity::One),
        };

        let max = if wants_array { arity.max() } else { Some(1) };
        if let Some(max) = max {
            if items.len() as u32 > max {
                return Err(BindError::TooMany {
                    role,
                    flag: key.to_string(),
                    max,
                    got: items.len(),
                });
            }
        }

        let urls: Vec<Value> = items
            .iter()
            .map(|m| match &m.source {
                MediaSource::Url { url } => Value::String(url.clone()),
                MediaSource::DataUri { data } => Value::String(data.clone()),
                // resolve() should have converted these. Emitting the path
                // rather than panicking keeps a programming error visible in
                // the provider's error message instead of killing the app.
                MediaSource::Local { path } => Value::String(path.clone()),
            })
            .collect();

        // A single reference on a repeated key must still be an array, or
        // providers that validate strictly reject it.
        let value = if wants_array {
            Value::Array(urls)
        } else {
            urls.into_iter().next().unwrap_or(Value::Null)
        };
        if !value.is_null() {
            out.insert(key.to_string(), value);
        }
    }

    Ok(out)
}

/// Which endpoint variant a request needs, given what the user attached.
///
/// fal splits one logical model across several endpoints — one per input mode —
/// and the registry stores only the **family root**. Posting the root answers
/// 404 (`Application "seedream" not found`), which is how every video
/// generation failed until this existed. Measured 2026-08-05: 17 of 36 fal
/// routes are roots needing a suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Text,
    Image,
    Video,
}

impl InputMode {
    /// Deduce the mode from the attachments. Deterministic, unlike *whether the
    /// endpoint exists*, which only fal can answer.
    pub fn of(media: &[MediaRef]) -> Self {
        if media
            .iter()
            .any(|m| matches!(m.role, MediaRole::Video | MediaRole::VideoReference))
        {
            InputMode::Video
        } else if media.is_empty() {
            InputMode::Text
        } else {
            InputMode::Image
        }
    }

    /// How to say this in an error the user can act on.
    pub fn describe(self) -> &'static str {
        match self {
            InputMode::Text => "a prompt on its own",
            InputMode::Image => "a still image",
            InputMode::Video => "a source clip",
        }
    }

    /// The suffix fal appends, given the output modality.
    fn suffix(self, produces_video: bool) -> &'static str {
        match (self, produces_video) {
            (InputMode::Text, true) => "/text-to-video",
            (InputMode::Image, true) => "/image-to-video",
            (InputMode::Video, true) => "/video-to-video",
            (InputMode::Text, false) => "/text-to-image",
            (InputMode::Image, false) => "/image-to-image",
            (InputMode::Video, false) => "/image-to-image",
        }
    }
}

/// Which input modes each fal route actually serves.
///
/// Measured 2026-08-05 by probing every fal route in the registry against
/// `fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`. An **empty** mode
/// list means the slug is already a complete endpoint and must not be
/// suffixed.
///
/// This cannot be derived, which is the whole reason it is a table:
/// `fal-ai/veo3.1` is complete while `fal-ai/kling-video/v3/standard` is a
/// root, and they look identical. Only `wan/v2.2-a14b` serves video-to-video;
/// every other video family is text and image only. Guessing produced
/// `404 Path /mini/video-to-video not found` against a live account.
///
/// **Provisional.** Phase C2 replaces this by reading fal's schema per
/// endpoint and caching it. Until then a slug absent from this table is
/// suffixed optimistically, which costs one 404 and no money.
const FAL_ROUTE_MODES: [(&str, Exact, &[InputMode]); 38] = [
    // Video editors. Each takes a clip and nothing else — offering them for
    // "Animate Image" produced "does not accept a start frame" at submit.
    // A complete text-to-image endpoint. Its editing counterpart is a separate
    // path (`.../edit`), not a suffix on this one — verified 2026-08-05, where
    // both `/text-to-image` and `/image-to-image` answer 404 here.
    ("xai/grok-imagine-image", true, &[InputMode::Text]),
    (
        "xai/grok-imagine-video/edit-video",
        true,
        &[InputMode::Video],
    ),
    (
        "xai/grok-imagine-video/extend-video",
        true,
        &[InputMode::Video],
    ),
    (
        "fal-ai/luma-dream-machine/ray-2/modify",
        true,
        &[InputMode::Video],
    ),
    // VACE takes a clip, a first frame, or reference stills.
    (
        "fal-ai/wan-vace-14b",
        true,
        &[InputMode::Text, InputMode::Image, InputMode::Video],
    ),
    // Complete endpoints — never suffix.
    ("bytedance/seed-audio-1.0", true, &[]),
    ("fal-ai/flux-2-pro", true, &[]),
    ("fal-ai/flux-pro/kontext", true, &[]),
    ("fal-ai/inworld-tts", true, &[]),
    ("fal-ai/nano-banana-2", true, &[]),
    ("fal-ai/nano-banana-pro", true, &[]),
    ("fal-ai/recraft/v4.1/text-to-image", true, &[]),
    ("fal-ai/veo3.1", true, &[]),
    ("fal-ai/veo3.1/fast", true, &[]),
    ("fal-ai/veo3.1/lite", true, &[]),
    ("fal-ai/wan-25-preview", true, &[]),
    ("google/gemini-omni-flash", true, &[]),
    ("google/nano-banana-2-lite", true, &[]),
    ("mirelo-ai/sfx1.6/text-to-audio", true, &[]),
    ("openai/gpt-image-2", true, &[]),
    ("sonilo/v1.1/text-to-music", true, &[]),
    // Family roots, with the modes each actually serves.
    (
        "bytedance/seedance-2.0",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "bytedance/seedance-2.0/fast",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "bytedance/seedance-2.0/mini",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "bytedance/seedance-2.5",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    ("bytedance/seedream/v5/lite", false, &[InputMode::Text]),
    ("bytedance/seedream/v5/pro", false, &[InputMode::Text]),
    (
        "fal-ai/bytedance/seedance/v1.5/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "fal-ai/bytedance/seedance/v1/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    // Image-to-video only: a first/last-frame model has nothing to do with a
    // bare prompt.
    ("fal-ai/kling-video/o1", false, &[InputMode::Image]),
    (
        "fal-ai/kling-video/o3/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "fal-ai/kling-video/v2.5-turbo/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "fal-ai/kling-video/v2.6/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "fal-ai/kling-video/v3/standard",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    // The only route that serves video-to-video.
    (
        "fal-ai/wan/v2.2-a14b",
        false,
        &[InputMode::Text, InputMode::Image, InputMode::Video],
    ),
    (
        "fal-ai/wan/v2.7",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    ("minimax/h3", false, &[InputMode::Text, InputMode::Image]),
    (
        "xai/grok-imagine-video/v1.5",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
];

/// Routes the registry lists that fal does not serve at all.
///
/// Measured 2026-08-05: no suffix resolves, so there is nothing to fall back
/// to. These came from Higgsfield's own picker — which lists their in-house and
/// newer versions — rather than from fal's catalogue.
///
/// Being absent from [`FAL_ROUTE_MODES`] is not enough to catch them: an
/// unknown slug is suffixed optimistically so the table cannot gate new models,
/// and that optimism is exactly what turned `fal-ai/wan/v2.6` into
/// `404 Path /v2.6/video-to-video not found` instead of an honest "not
/// available here".
pub const FAL_MISSING_ROUTES: [&str; 3] = [
    "fal-ai/kling-video/v3/turbo",
    "fal-ai/minimax/hailuo-2.3",
    "fal-ai/wan/v2.6",
];

/// True when fal is known not to serve this slug at all.
pub fn route_is_missing(slug: &str) -> bool {
    FAL_MISSING_ROUTES.contains(&slug)
}

/// fal endpoints measured to accept **no media at all**.
///
/// Distinct from "we have not measured it": these were checked against fal's
/// published schema on 2026-08-05 and have no media field of any kind. Gemini
/// Omni is the one that reached a user — they attached a clip, asked for an
/// edit, and were billed for an unrelated generation because fal silently
/// ignores a key it does not recognise.
///
/// Without this, an *exact* endpoint (no suffix to resolve) had no mode
/// information and `can_bind` fell through to permissive.
const FAL_NO_MEDIA: [&str; 10] = [
    "bytedance/seedream/v5/pro",
    "fal-ai/flux-2-pro",
    "fal-ai/nano-banana-2",
    "fal-ai/nano-banana-pro",
    "fal-ai/veo3.1",
    "fal-ai/veo3.1/fast",
    "fal-ai/veo3.1/lite",
    "google/gemini-omni-flash",
    "google/nano-banana-2-lite",
    "openai/gpt-image-2",
];

/// True when fal's schema shows this endpoint takes no attachments.
pub fn takes_no_media(slug: &str) -> bool {
    FAL_NO_MEDIA.contains(&slug)
}

/// Routes that take a start frame but have **no end-frame field at all**.
///
/// The third way an attachment can vanish, and the one that had no guard.
/// [`FAL_NO_MEDIA`] catches endpoints that take nothing; the mode table catches
/// the wrong input mode; neither catches an endpoint that happily accepts your
/// still and has nowhere to put the second one. fal ignores keys it does not
/// declare, so the render runs, looks plausible, and is billed in full.
///
/// Measured 2026-08-28 against every video route in the registry — fal via
/// `fal.ai/api/openapi/queue/openapi.json`, Higgsfield via their own
/// `openapi.json`. The Higgsfield entries matter most: that provider gets no
/// schema sweep at submit, so nothing downstream would have caught them.
///
/// Keyed by the **route slug** the registry stores, not the resolved endpoint,
/// because that is what [`can_bind`] is given. fal and Higgsfield slugs cannot
/// collide — fal's carry a vendor prefix (`fal-ai/`, `xai/`) where the mirror's
/// do not.
const NO_END_FRAME: [&str; 8] = [
    // -- fal -------------------------------------------------------------
    "xai/grok-imagine-video/v1.5",
    "fal-ai/wan-25-preview",
    // -- Higgsfield's mirror ----------------------------------------------
    // Not one of their image-to-video paths declares an end frame. Only DoP
    // does, and DoP is judged by the catalogue, not by this list.
    "bytedance/seedance/v1/pro/fast",
    "kling-video/v2.5-turbo/pro",
    "minimax/hailuo-2.3/pro",
    "veo3.1",
    "veo3.1/fast",
    "wan-25-preview",
];

/// True when this route accepts media but cannot take an end frame.
pub fn takes_no_end_frame(slug: &str) -> bool {
    NO_END_FRAME.contains(&slug)
}

/// Whether a slug is already a complete endpoint.
///
/// Split from the mode list because they are **independent facts**, and
/// conflating them shipped a 404: the Grok editors are complete endpoints that
/// accept only a clip, and a non-empty mode list used to mean "family root", so
/// `xai/grok-imagine-video/edit-video` was suffixed into
/// `…/edit-video/video-to-video`.
type Exact = bool;

fn entry(slug: &str) -> Option<(Exact, &'static [InputMode])> {
    FAL_ROUTE_MODES
        .iter()
        .find(|(s, _, _)| *s == slug)
        .map(|(_, e, m)| (*e, *m))
}

fn modes_for(slug: &str) -> Option<&'static [InputMode]> {
    entry(slug).map(|(_, m)| m)
}

/// Whether this slug already names a complete fal endpoint.
pub fn endpoint_is_exact(slug: &str) -> bool {
    matches!(entry(slug), Some((true, _)))
}

/// The model does not serve the mode the attachments imply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedMode {
    pub wanted: InputMode,
    pub supported: &'static [InputMode],
}

impl fmt::Display for UnsupportedMode {
    /// Names what the model *can* do. "404 not found" tells the user nothing
    /// they can act on; "this one takes a prompt or a still, not a clip" does.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let can: Vec<&str> = self.supported.iter().map(|m| m.describe()).collect();
        if can.is_empty() {
            return write!(
                f,
                "this provider does not serve this model — pick another route or model"
            );
        }
        write!(
            f,
            "this model takes {}, not {}",
            can.join(" or "),
            self.wanted.describe()
        )
    }
}

impl std::error::Error for UnsupportedMode {}

/// Resolve a family root into the endpoint that can serve this request.
///
/// Errors rather than guessing when the mode is unsupported: attaching a clip
/// to Seedance produced `404 Path /mini/video-to-video not found` after a full
/// round trip, when we already knew it only does text and image.
pub fn resolve_endpoint(
    slug: &str,
    mode: InputMode,
    produces_video: bool,
) -> Result<String, UnsupportedMode> {
    if route_is_missing(slug) {
        // No mode resolves, so `supported: &[]` is the truth and Display says
        // so without listing alternatives that do not exist.
        return Err(UnsupportedMode {
            wanted: mode,
            supported: &[],
        });
    }
    match entry(slug) {
        // Measured to serve this endpoint but not this mode.
        Some((_, modes)) if !modes.is_empty() && !modes.contains(&mode) => Err(UnsupportedMode {
            wanted: mode,
            supported: modes,
        }),
        // A complete endpoint. Suffixing it produced
        // `.../edit-video/video-to-video`, a 404 that read as a routing bug.
        Some((true, _)) => Ok(slug.to_string()),
        _ => {
            // A slug that already names its mode must never be suffixed twice.
            const MODE_TAILS: [&str; 5] = [
                "-to-video",
                "-to-image",
                "-to-audio",
                "-to-music",
                "-to-speech",
            ];
            if MODE_TAILS.iter().any(|t| slug.ends_with(t)) || slug.ends_with("/edit") {
                return Ok(slug.to_string());
            }
            Ok(format!("{slug}{}", mode.suffix(produces_video)))
        }
    }
}

// ---------------------------------------------------------------------------
// Higgsfield's own key-pair platform
// ---------------------------------------------------------------------------

/// Which input modes `api.higgsfield.ai` serves, per family root.
///
/// Read from `https://docs.higgsfield.ai/docs/openapi.json` on 2026-08-28: 47
/// model paths, and the third-party families among them are consistently **one
/// version behind** the picker (Kling 2.1/2.5 against our 2.6/3.0, Wan 2.5
/// against our 2.6/2.7, Hailuo 02/2.3 against our H3). That is why this table
/// is short: it lists only the families where a slug in the registry and a path
/// in their spec are the same model, not every path they publish.
///
/// Separate from [`FAL_ROUTE_MODES`] because the two providers disagree about
/// the same family. fal's `veo3.1` is a complete endpoint that takes no media
/// at all; Higgsfield's `/veo3.1` is the *text* endpoint and `/veo3.1/
/// image-to-video` sits beside it. Sharing one table would have to lie about
/// one of them.
///
/// An empty mode list means the path is complete and must not be suffixed —
/// same convention as the fal table, and what keeps the pre-existing DoP and
/// Soul routes passing through untouched.
const HIGGSFIELD_ROUTE_MODES: [(&str, Exact, &[InputMode]); 9] = [
    (
        "bytedance/seedance/v1/pro/fast",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "kling-video/v2.5-turbo/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "minimax/hailuo-2.3/pro",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    (
        "wan-25-preview",
        false,
        &[InputMode::Text, InputMode::Image],
    ),
    ("veo3.1", false, &[InputMode::Text, InputMode::Image]),
    ("veo3.1/fast", false, &[InputMode::Text, InputMode::Image]),
    // Complete paths, no mode axis. Listed so they are documented as checked
    // rather than merely unmeasured.
    ("higgsfield-ai/dop/standard", true, &[]),
    ("higgsfield-ai/dop/lite", true, &[]),
    ("higgsfield-ai/soul/standard", true, &[]),
];

/// Families whose **text** path is the bare root rather than a suffix.
///
/// Veo is the only one, and it is not derivable: `/veo3.1/image-to-video`
/// exists, so the family reads like every other root, but `/veo3.1/
/// text-to-video` is a 404 — `/veo3.1` itself is the text endpoint. Suffixing
/// uniformly would break exactly the two Veo routes and nothing else, which is
/// the hardest kind of bug to see.
const HIGGSFIELD_BARE_TEXT: [&str; 2] = ["veo3.1", "veo3.1/fast"];

fn higgsfield_entry(slug: &str) -> Option<(Exact, &'static [InputMode])> {
    HIGGSFIELD_ROUTE_MODES
        .iter()
        .find(|(s, _, _)| *s == slug)
        .map(|(_, e, m)| (*e, *m))
}

/// Resolve a Higgsfield family root into the path this request can be posted to.
///
/// The counterpart to [`resolve_endpoint`], which speaks only fal. A slug this
/// table has never heard of passes through unchanged rather than erroring: the
/// Soul and DoP routes predate the table and must keep working, and an unknown
/// slug costs one 404 and no money.
pub fn resolve_higgsfield_endpoint(
    slug: &str,
    mode: InputMode,
    produces_video: bool,
) -> Result<String, UnsupportedMode> {
    match higgsfield_entry(slug) {
        // Their spec says this family does not serve this mode. Refusing here
        // is the whole point of the table — the alternative is a round trip
        // that answers `model_not_found`, which reads as "your key is wrong".
        Some((_, modes)) if !modes.is_empty() && !modes.contains(&mode) => Err(UnsupportedMode {
            wanted: mode,
            supported: modes,
        }),
        Some((true, _)) => Ok(slug.to_string()),
        Some((_, _)) if mode == InputMode::Text && HIGGSFIELD_BARE_TEXT.contains(&slug) => {
            Ok(slug.to_string())
        }
        Some((_, _)) => Ok(format!("{slug}{}", mode.suffix(produces_video))),
        None => Ok(slug.to_string()),
    }
}

/// True when this slug appears in Higgsfield's published spec at all.
///
/// The guard for the `dop/preview` class of bug: that route sat in the registry
/// quoting a tier their platform has never had (they publish lite, standard and
/// turbo), so every generation on it 404'd with `model_not_found` — which the
/// client then reported as "this surface exists only inside their web app".
/// A wrong slug and an unreachable surface are indistinguishable from the
/// outside, so the only place to catch one is here.
pub fn higgsfield_is_measured(slug: &str) -> bool {
    higgsfield_entry(slug).is_some()
}

/// True when this Higgsfield path speaks **fal's** parameter vocabulary.
///
/// Their key-pair platform is a fal-shaped mirror, and the shape goes deeper
/// than the slugs: `POST /kling-video/v2.5-turbo/pro/image-to-video` requires
/// `prompt` and **`image_url`** — fal's name — not the `start_image` their CLI
/// uses or the `input_image` our hand-authored specs guessed. Verified against
/// their `openapi.json` on 2026-08-28 for all five image-to-video mirrors.
///
/// So the usual rule inverts for these. [`can_bind`] says the catalogue is the
/// spec for a Higgsfield route, and that is right for their **in-house**
/// surfaces — Soul, DoP, popcorn, all under `higgsfield-ai/` — which the CLI
/// spec genuinely describes. It is wrong for the mirror, where posting
/// `start_image` is a 422 on every image-to-video call.
///
/// Deliberately keyed off the measured table rather than the `higgsfield-ai/`
/// prefix: the Studio compilers and the 3D family are bare job-set-type names
/// we have never seen a path for, and guessing fal's vocabulary for them would
/// invent a wire format.
pub fn higgsfield_speaks_fal(slug: &str) -> bool {
    higgsfield_is_measured(slug) && !slug.starts_with("higgsfield-ai/")
}

/// True when Higgsfield's key-pair platform is measured to serve this slug in
/// this mode. Used by the picker so a route is not offered for a job it cannot
/// take.
pub fn higgsfield_serves(slug: &str, mode: InputMode) -> bool {
    match higgsfield_entry(slug) {
        Some((_, modes)) if !modes.is_empty() => modes.contains(&mode),
        _ => true,
    }
}

/// Does this set of attachments include an end frame?
///
/// Lifted out because [`crate::enhance`] needs exactly this question and
/// getting it wrong silently rewrites a prompt that must not be rewritten.
pub fn has_end_frame(media: &[MediaRef]) -> bool {
    media.iter().any(|m| m.role == MediaRole::End)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{FlagSpec, Modality, ValueSpec};

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

    struct FakeUploader {
        accepts_data: bool,
    }

    impl Uploader for FakeUploader {
        fn upload(&self, path: &str) -> Result<String, String> {
            Ok(format!(
                "https://cdn.example/{}",
                path.trim_start_matches('/')
            ))
        }
        fn accepts_data_uri(&self) -> bool {
            self.accepts_data
        }
    }

    #[test]
    fn a_start_frame_prefers_the_dedicated_flag() {
        let s = spec(vec![
            media_flag("image", Arity::One),
            media_flag("start_image", Arity::One),
        ]);
        assert_eq!(flag_for(&s, MediaRole::Start), Some("start_image"));
    }

    #[test]
    fn a_start_frame_falls_back_to_a_plain_image_flag() {
        // 32 models in the spec have `image` and no `start_image`. Refusing
        // them would disable image-to-video on half the roster.
        let s = spec(vec![media_flag("image", Arity::One)]);
        assert_eq!(flag_for(&s, MediaRole::Start), Some("image"));
    }

    #[test]
    fn an_end_frame_never_falls_back_to_the_input_image_flag() {
        // The bug this prevents: binding an end frame to `image` produces a
        // generation that silently ignores it and looks like it worked.
        let s = spec(vec![media_flag("image", Arity::One)]);
        assert_eq!(flag_for(&s, MediaRole::End), None);
    }

    #[test]
    fn an_unsupported_role_names_the_model_and_says_what_to_do() {
        let s = spec(vec![media_flag("image", Arity::One)]);
        let err = bind(
            &s,
            "Kling 3.0",
            &[MediaRef::url(MediaRole::Audio, "https://a/b.wav")],
            Dialect::Catalog,
            "",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Kling 3.0"), "got: {msg}");
        assert!(msg.contains("audio"), "got: {msg}");
        assert!(msg.contains("different model"), "got: {msg}");
    }

    #[test]
    fn several_references_land_in_one_array_rather_than_overwriting() {
        // The bug: binding per-item instead of per-role leaves only the last
        // reference, which looks like the model ignoring the others.
        let s = spec(vec![media_flag("image_references", Arity::Repeated)]);
        let body = bind(
            &s,
            "Test",
            &[
                MediaRef::url(MediaRole::Reference, "https://a/1.png"),
                MediaRef::url(MediaRole::Reference, "https://a/2.png"),
                MediaRef::url(MediaRole::Reference, "https://a/3.png"),
            ],
            Dialect::Catalog,
            "",
        )
        .unwrap();
        let arr = body["image_references"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], "https://a/1.png");
        assert_eq!(arr[2], "https://a/3.png");
    }

    #[test]
    fn a_single_item_on_a_repeated_flag_is_still_an_array() {
        // Providers that validate strictly reject a bare string here.
        let s = spec(vec![media_flag("image_references", Arity::Repeated)]);
        let body = bind(
            &s,
            "Test",
            &[MediaRef::url(MediaRole::Reference, "https://a/1.png")],
            Dialect::Catalog,
            "",
        )
        .unwrap();
        assert!(body["image_references"].is_array());
    }

    #[test]
    fn a_scalar_flag_is_not_wrapped_in_an_array() {
        let s = spec(vec![media_flag("start_image", Arity::One)]);
        let body = bind(
            &s,
            "Test",
            &[MediaRef::url(MediaRole::Start, "https://a/1.png")],
            Dialect::Catalog,
            "",
        )
        .unwrap();
        assert_eq!(body["start_image"], "https://a/1.png");
    }

    #[test]
    fn exceeding_arity_is_caught_before_the_request_costs_money() {
        let s = spec(vec![media_flag(
            "image_references",
            Arity::Range { min: 1, max: 3 },
        )]);
        let four: Vec<MediaRef> = (0..4)
            .map(|i| MediaRef::url(MediaRole::Reference, format!("https://a/{i}.png")))
            .collect();
        let err = bind(&s, "Test", &four, Dialect::Catalog, "").unwrap_err();
        assert!(matches!(err, BindError::TooMany { max: 3, got: 4, .. }));
        assert!(err.to_string().contains('3'));
    }

    #[test]
    fn start_and_end_frames_bind_to_different_flags() {
        let s = spec(vec![
            media_flag("start_image", Arity::One),
            media_flag("end_image", Arity::One),
        ]);
        let body = bind(
            &s,
            "Test",
            &[
                MediaRef::url(MediaRole::Start, "https://a/first.png"),
                MediaRef::url(MediaRole::End, "https://a/last.png"),
            ],
            Dialect::Catalog,
            "",
        )
        .unwrap();
        assert_eq!(body["start_image"], "https://a/first.png");
        assert_eq!(body["end_image"], "https://a/last.png");
    }

    #[test]
    fn a_non_media_flag_of_the_same_name_is_not_bound_to() {
        // `image` as a boolean "return an image too" flag exists on at least
        // one model. Binding a file to it would be a 422.
        let s = spec(vec![FlagSpec {
            name: "image".into(),
            alias: None,
            required: false,
            default: None,
            value: ValueSpec::Boolean,
            arity: Arity::One,
        }]);
        assert_eq!(flag_for(&s, MediaRole::Start), None);
    }

    #[test]
    fn local_files_are_uploaded_and_urls_are_left_alone() {
        let up = FakeUploader {
            accepts_data: false,
        };
        let out = resolve(
            &[
                MediaRef::local(MediaRole::Start, "/tmp/a.png"),
                MediaRef::url(MediaRole::End, "https://already/there.png"),
            ],
            &up,
        )
        .unwrap();
        assert_eq!(
            out[0].source,
            MediaSource::Url {
                url: "https://cdn.example/tmp/a.png".into()
            }
        );
        // Re-uploading something already fetchable wastes time and, on some
        // providers, money.
        assert_eq!(
            out[1].source,
            MediaSource::Url {
                url: "https://already/there.png".into()
            }
        );
    }

    #[test]
    fn a_data_uri_is_uploaded_unless_the_provider_takes_it_inline() {
        let strict = FakeUploader {
            accepts_data: false,
        };
        let inline = FakeUploader { accepts_data: true };
        let item = [MediaRef::new(
            MediaRole::Start,
            MediaSource::DataUri {
                data: "data:image/png;base64,AAAA".into(),
            },
        )];
        assert!(resolve(&item, &strict).unwrap()[0].source.is_reachable());
        assert!(matches!(
            resolve(&item, &inline).unwrap()[0].source,
            MediaSource::DataUri { .. }
        ));
    }

    #[test]
    fn an_upload_failure_surfaces_rather_than_submitting_a_broken_request() {
        struct Broken;
        impl Uploader for Broken {
            fn upload(&self, _: &str) -> Result<String, String> {
                Err("disk read failed".into())
            }
        }
        let e = resolve(&[MediaRef::local(MediaRole::Start, "/tmp/a.png")], &Broken).unwrap_err();
        assert!(e.contains("disk read failed"));
    }

    #[test]
    fn no_media_produces_no_wire_fields() {
        // A text-to-video request must not carry empty media keys; several
        // providers 422 on a null image.
        let s = spec(vec![media_flag("image", Arity::One)]);
        assert!(bind(&s, "Test", &[], Dialect::Catalog, "")
            .unwrap()
            .is_empty());
    }

    // ── Dialect ────────────────────────────────────────────────────────────
    //
    // Every expectation below was read off fal's own published per-endpoint
    // OpenAPI (`fal.ai/api/openapi/queue/openapi.json?endpoint_id=…`) on
    // 2026-08-04. They are pinned because the bug they replace was silent: the
    // binder emitted Higgsfield's CLI names, so every fal image-to-video call
    // would have been rejected with a 422 about a missing `image_url`.

    #[test]
    fn fal_wants_image_url_not_the_catalogues_name() {
        let s = spec(vec![media_flag("image", Arity::One)]);
        let body = bind(
            &s,
            "Kling 2.5",
            &[MediaRef::url(MediaRole::Start, "https://a/1.png")],
            Dialect::Fal,
            "fal-ai/kling-video/v2.5-turbo/pro/image-to-video",
        )
        .unwrap();
        assert_eq!(body["image_url"], "https://a/1.png");
        assert!(
            !body.contains_key("image"),
            "the catalogue's name must not reach fal: {body:?}"
        );
    }

    #[test]
    fn fal_end_frame_key_differs_by_model_family() {
        // fal is not internally consistent here, which is precisely why the
        // mapping needs the slug and not just the provider.
        assert_eq!(
            fal_keys(
                MediaRole::End,
                "fal-ai/kling-video/v2.5-turbo/pro/image-to-video"
            ),
            ["tail_image_url"]
        );
        assert_eq!(
            fal_keys(
                MediaRole::End,
                "fal-ai/minimax/hailuo-02/standard/image-to-video"
            ),
            ["end_image_url"]
        );
        assert_eq!(
            fal_keys(MediaRole::End, "fal-ai/wan-vace-14b"),
            ["last_frame_url"]
        );
    }

    #[test]
    fn wan_vace_renames_three_roles_at_once() {
        assert_eq!(
            fal_keys(MediaRole::Start, "fal-ai/wan-vace-14b"),
            ["first_frame_url"]
        );
        assert_eq!(
            fal_keys(MediaRole::Reference, "fal-ai/wan-vace-14b"),
            ["ref_image_urls"]
        );
    }

    #[test]
    fn fal_reference_lists_are_plural_and_arrayed() {
        // Verified: fal-ai/bytedance/seedream/v4/edit and fal-ai/nano-banana/edit
        // both require `image_urls` as an array.
        let s = spec(vec![media_flag("image_references", Arity::Repeated)]);
        let body = bind(
            &s,
            "Seedream 4",
            &[MediaRef::url(MediaRole::Reference, "https://a/1.png")],
            Dialect::Fal,
            "fal-ai/bytedance/seedream/v4/edit",
        )
        .unwrap();
        assert!(body["image_urls"].is_array(), "got: {body:?}");
    }

    #[test]
    fn fal_plurality_comes_from_the_key_name_not_the_catalogue() {
        // The catalogue calls `image` singular; fal's `image_urls` is an array.
        // Trusting arity alone would send a bare string to an array field.
        assert!(Dialect::Fal.is_array_key("image_urls"));
        assert!(!Dialect::Fal.is_array_key("image_url"));
        assert!(Dialect::Fal.is_array_key("ref_image_urls"));
    }

    #[test]
    fn fal_video_and_audio_keys_are_uniform() {
        // Verified against sync-lipsync/v2 and topaz/upscale/video.
        assert_eq!(
            fal_keys(MediaRole::Video, "fal-ai/sync-lipsync/v2"),
            ["video_url"]
        );
        assert_eq!(
            fal_keys(MediaRole::Audio, "fal-ai/sync-lipsync/v2"),
            ["audio_url"]
        );
    }

    #[test]
    fn the_catalogue_dialect_still_serves_higgsfields_own_api() {
        // Their public API is the one place the CLI's vocabulary is correct.
        let s = spec(vec![media_flag("start_image", Arity::One)]);
        let body = bind(
            &s,
            "Soul",
            &[MediaRef::url(MediaRole::Start, "https://a/1.png")],
            Dialect::Catalog,
            "higgsfield-ai/soul/standard",
        )
        .unwrap();
        assert_eq!(body["start_image"], "https://a/1.png");
    }

    #[test]
    fn each_dialect_refuses_using_its_own_authority() {
        // The two dialects read different documents on purpose, and the earlier
        // version of this test asserted they behave identically — which is what
        // hid the real bug. The catalogue describes Higgsfield's API, so it is
        // the wrong authority for a fal route: trusting it there excluded Wan
        // 2.2 from video editing, the one model verified live to do it.
        let s = spec(vec![media_flag("image", Arity::One)]);
        let audio = [MediaRef::url(MediaRole::Audio, "https://a/b.wav")];

        // Catalogue: the spec declares no audio flag, so no.
        assert!(bind(&s, "Test", &audio, Dialect::Catalog, "").is_err());

        // fal: judged on the endpoint's measured modes. An image-only endpoint
        // refuses a clip...
        let clip = [MediaRef::url(MediaRole::Video, "https://a/b.mp4")];
        assert!(
            bind(&s, "Test", &clip, Dialect::Fal, "fal-ai/kling-video/o1").is_err(),
            "kling o1 serves image-to-video only and must refuse a clip"
        );
        // ...and an unmeasured endpoint is permissive, because the schema gate
        // at submit is the backstop and refusing would hide working models.
        assert!(bind(&s, "Test", &clip, Dialect::Fal, "some/new/model").is_ok());
    }

    // ── Endpoint resolution ────────────────────────────────────────────────

    #[test]
    fn a_family_root_gains_the_mode_the_media_implies() {
        // The bug: posting the bare root answered
        // `404 Application "seedream" not found`, so every video generation
        // failed. Measured 2026-08-05 — 17 of 36 fal routes are roots.
        let root = "fal-ai/kling-video/v2.5-turbo/pro";
        assert_eq!(
            resolve_endpoint(root, InputMode::Text, true).unwrap(),
            "fal-ai/kling-video/v2.5-turbo/pro/text-to-video"
        );
        assert_eq!(
            resolve_endpoint(root, InputMode::Image, true).unwrap(),
            "fal-ai/kling-video/v2.5-turbo/pro/image-to-video"
        );
    }

    #[test]
    fn a_verified_exact_endpoint_is_left_alone() {
        // Suffixing one of the 16 that already resolve would break it — the
        // mirror image of the original bug.
        assert!(endpoint_is_exact("fal-ai/veo3.1"));
        assert_eq!(
            resolve_endpoint("fal-ai/veo3.1", InputMode::Text, true).unwrap(),
            "fal-ai/veo3.1"
        );
    }

    #[test]
    fn a_slug_that_already_names_its_mode_is_never_suffixed_twice() {
        // Belt and braces for the exact-list: `.../text-to-image/text-to-image`
        // is a 404 that would look exactly like the bug we just fixed.
        for slug in [
            "fal-ai/recraft/v4.1/text-to-image",
            "mirelo-ai/sfx1.6/text-to-audio",
            "sonilo/v1.1/text-to-music",
            "fal-ai/bytedance/seedream/v4/edit",
        ] {
            assert_eq!(
                resolve_endpoint(slug, InputMode::Text, false).unwrap(),
                slug
            );
        }
    }

    #[test]
    fn the_mode_comes_from_what_was_actually_attached() {
        assert_eq!(InputMode::of(&[]), InputMode::Text);
        assert_eq!(
            InputMode::of(&[MediaRef::url(MediaRole::Start, "u")]),
            InputMode::Image
        );
        // A source clip wins over a reference still: video-to-video is a
        // different endpoint from image-to-video, and picking by "is there any
        // media" would send a v2v job to the wrong one.
        assert_eq!(
            InputMode::of(&[
                MediaRef::url(MediaRole::Reference, "img"),
                MediaRef::url(MediaRole::Video, "clip"),
            ]),
            InputMode::Video
        );
    }

    #[test]
    fn an_image_model_gets_image_suffixes_not_video_ones() {
        assert_eq!(
            resolve_endpoint("some/root", InputMode::Text, false).unwrap(),
            "some/root/text-to-image"
        );
    }

    #[test]
    fn attaching_a_clip_to_a_model_that_cannot_take_one_is_refused_up_front() {
        // Observed live: Seedance Mini with a source clip produced
        // `404 Path /mini/video-to-video not found` after a full round trip,
        // when the mode table already knew it does text and image only.
        let err =
            resolve_endpoint("bytedance/seedance-2.0/mini", InputMode::Video, true).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("source clip"), "got: {msg}");
        // It must name what the model CAN do, or the user has nothing to act on.
        assert!(
            msg.contains("prompt") && msg.contains("still"),
            "got: {msg}"
        );
    }

    #[test]
    fn the_one_route_that_serves_video_to_video_still_does() {
        // Wan 2.2 is the sole exception, so a blanket "no v2v" rule would be
        // just as wrong as the guess it replaces.
        assert_eq!(
            resolve_endpoint("fal-ai/wan/v2.2-a14b", InputMode::Video, true).unwrap(),
            "fal-ai/wan/v2.2-a14b/video-to-video"
        );
    }

    #[test]
    fn a_first_last_frame_model_refuses_a_bare_prompt() {
        // kling-video/o1 serves image-to-video only: interpolating between
        // frames is meaningless without them.
        assert!(resolve_endpoint("fal-ai/kling-video/o1", InputMode::Text, true).is_err());
        assert!(resolve_endpoint("fal-ai/kling-video/o1", InputMode::Image, true).is_ok());
    }

    #[test]
    fn an_unknown_slug_is_suffixed_optimistically() {
        // A route missing from the table costs one 404, not a refusal — the
        // table is provisional and must not become a gate on new models.
        assert_eq!(
            resolve_endpoint("some/brand-new/model", InputMode::Image, true).unwrap(),
            "some/brand-new/model/image-to-video"
        );
    }

    #[test]
    fn a_complete_endpoint_is_never_suffixed_even_when_its_modes_are_known() {
        // The 404 this prevents: `xai/grok-imagine-video/edit-video` is a
        // complete endpoint that accepts only a clip. Recording the mode used
        // to imply "family root", so it was suffixed into
        // `.../edit-video/video-to-video` — a path fal does not have.
        //
        // Exactness and accepted-modes are independent facts and the table now
        // carries both.
        for slug in [
            "xai/grok-imagine-video/edit-video",
            "xai/grok-imagine-video/extend-video",
            "fal-ai/luma-dream-machine/ray-2/modify",
            "fal-ai/wan-vace-14b",
        ] {
            assert!(endpoint_is_exact(slug), "{slug} should be exact");
            assert_eq!(
                resolve_endpoint(slug, InputMode::Video, true).unwrap(),
                slug,
                "{slug} must not gain a suffix"
            );
        }
    }

    #[test]
    fn no_resolved_endpoint_ever_doubles_a_mode_segment() {
        // A structural guard over the whole registry: whatever we would POST
        // to, in any mode, must not contain two mode segments. That is the
        // shape every suffixing bug has taken.
        let reg = crate::registry::registry();
        for m in reg.values() {
            for r in m
                .routes
                .iter()
                .filter(|r| r.provider == crate::ProviderId::Fal)
            {
                for mode in [InputMode::Text, InputMode::Image, InputMode::Video] {
                    let Ok(ep) = resolve_endpoint(&r.slug, mode, true) else {
                        continue;
                    };
                    let segments = ["-to-video", "-to-image", "-to-audio"]
                        .iter()
                        .map(|t| ep.matches(t).count())
                        .sum::<usize>();
                    assert!(
                        segments <= 1,
                        "{} resolved to {ep}, which names its mode twice",
                        m.id
                    );
                }
            }
        }
    }

    #[test]
    fn every_exact_endpoint_is_a_route_the_registry_actually_has() {
        // Guards the measured table against the registry moving under it: a
        // stale entry here silently stops suffixing a route that now needs it.
        let reg = crate::registry::registry();
        let known: std::collections::HashSet<&str> = reg
            .values()
            .flat_map(|m| m.routes.iter())
            .filter(|r| r.provider == crate::ProviderId::Fal)
            .map(|r| r.slug.as_str())
            .collect();
        for (slug, _, _) in FAL_ROUTE_MODES {
            assert!(
                known.contains(slug),
                "{slug} is in the mode table but no fal route uses it"
            );
        }
    }

    #[test]
    fn end_frame_detection_drives_the_enhance_rule() {
        assert!(!has_end_frame(&[MediaRef::url(MediaRole::Start, "u")]));
        assert!(has_end_frame(&[
            MediaRef::url(MediaRole::Start, "u"),
            MediaRef::url(MediaRole::End, "v"),
        ]));
    }

    #[test]
    fn real_models_in_the_registry_accept_a_start_frame() {
        // Guards the binding table against the vendored spec being re-parsed
        // into different flag names. If MODELS.md renames `start_image`, this
        // fails loudly rather than disabling i2v across the roster.
        let reg = crate::registry::registry();
        let with_media: Vec<_> = reg
            .values()
            .filter(|m| m.spec.media_flags().next().is_some())
            .collect();
        assert!(
            with_media.len() > 20,
            "expected many models to take media, got {}",
            with_media.len()
        );
        let bindable = with_media
            .iter()
            .filter(|m| flag_for(&m.spec, MediaRole::Start).is_some())
            .count();
        assert!(
            bindable > 20,
            "only {bindable} models can take a start frame — the binding table has drifted"
        );
    }

    // -- Higgsfield's own platform ------------------------------------------

    #[test]
    fn higgsfield_veo_text_is_the_bare_root() {
        // The trap this table exists for. `/veo3.1/image-to-video` exists, so
        // the family looks like every other root — but `/veo3.1/text-to-video`
        // is a 404 and `/veo3.1` is itself the text endpoint.
        assert_eq!(
            resolve_higgsfield_endpoint("veo3.1", InputMode::Text, true).unwrap(),
            "veo3.1"
        );
        assert_eq!(
            resolve_higgsfield_endpoint("veo3.1", InputMode::Image, true).unwrap(),
            "veo3.1/image-to-video"
        );
        assert_eq!(
            resolve_higgsfield_endpoint("veo3.1/fast", InputMode::Text, true).unwrap(),
            "veo3.1/fast"
        );
        assert_eq!(
            resolve_higgsfield_endpoint("veo3.1/fast", InputMode::Image, true).unwrap(),
            "veo3.1/fast/image-to-video"
        );
    }

    #[test]
    fn higgsfield_veo_differs_from_fal_veo() {
        // Same family name, different shape on each provider — the reason
        // these are two tables and not one. fal's is complete and media-free.
        assert!(endpoint_is_exact("fal-ai/veo3.1"));
        assert!(takes_no_media("fal-ai/veo3.1"));
        assert_eq!(
            resolve_higgsfield_endpoint("veo3.1", InputMode::Image, true).unwrap(),
            "veo3.1/image-to-video"
        );
    }

    #[test]
    fn higgsfield_roots_take_the_mode_suffix() {
        for (root, want) in [
            (
                "bytedance/seedance/v1/pro/fast",
                "bytedance/seedance/v1/pro/fast/image-to-video",
            ),
            (
                "kling-video/v2.5-turbo/pro",
                "kling-video/v2.5-turbo/pro/image-to-video",
            ),
            (
                "minimax/hailuo-2.3/pro",
                "minimax/hailuo-2.3/pro/image-to-video",
            ),
            ("wan-25-preview", "wan-25-preview/image-to-video"),
        ] {
            assert_eq!(
                resolve_higgsfield_endpoint(root, InputMode::Image, true).unwrap(),
                want,
                "{root} resolved wrong"
            );
        }
    }

    #[test]
    fn higgsfield_complete_paths_are_never_suffixed() {
        // DoP and Soul predate the table. Adding it must not change them.
        for slug in [
            "higgsfield-ai/dop/standard",
            "higgsfield-ai/dop/lite",
            "higgsfield-ai/soul/standard",
        ] {
            for mode in [InputMode::Text, InputMode::Image, InputMode::Video] {
                assert_eq!(
                    resolve_higgsfield_endpoint(slug, mode, true).unwrap(),
                    slug,
                    "{slug} was suffixed"
                );
            }
        }
    }

    #[test]
    fn higgsfield_refuses_a_mode_it_does_not_serve() {
        // None of their third-party families take a clip. Refusing here saves
        // a round trip that would answer `model_not_found` and read as a bad
        // key rather than a wrong job.
        let e = resolve_higgsfield_endpoint("kling-video/v2.5-turbo/pro", InputMode::Video, true)
            .unwrap_err();
        assert_eq!(e.wanted, InputMode::Video);
        assert!(e.supported.contains(&InputMode::Image));
        assert!(!higgsfield_serves(
            "kling-video/v2.5-turbo/pro",
            InputMode::Video
        ));
        assert!(higgsfield_serves(
            "kling-video/v2.5-turbo/pro",
            InputMode::Image
        ));
    }

    #[test]
    fn an_unmeasured_higgsfield_slug_passes_through() {
        // Soul's siblings and the Studio slugs are not in the table and must
        // keep reaching the provider — one honest 404 beats hiding a route.
        assert_eq!(
            resolve_higgsfield_endpoint("marketing_studio_video", InputMode::Image, true).unwrap(),
            "marketing_studio_video"
        );
        assert!(higgsfield_serves(
            "marketing_studio_video",
            InputMode::Image
        ));
        assert!(!higgsfield_is_measured("marketing_studio_video"));
        assert!(higgsfield_is_measured("higgsfield-ai/dop/lite"));
    }

    #[test]
    fn the_dop_preview_tier_never_existed() {
        // The regression guard. Their spec publishes lite/standard/turbo.
        assert!(!higgsfield_is_measured("higgsfield-ai/dop/preview"));
    }

    #[test]
    fn the_higgsfield_mirror_speaks_fal_not_the_catalogue() {
        // Their platform requires `image_url` on every image-to-video mirror,
        // read from their openapi.json. The catalogue says `start_image`, and
        // the hand-authored picker specs say `input_image` — posting either is
        // a 422.
        for slug in [
            "kling-video/v2.5-turbo/pro",
            "minimax/hailuo-2.3/pro",
            "bytedance/seedance/v1/pro/fast",
            "wan-25-preview",
            "veo3.1",
            "veo3.1/fast",
        ] {
            assert!(higgsfield_speaks_fal(slug), "{slug} is a fal-shaped mirror");
        }
        // Their in-house surfaces are the opposite case: the CLI spec really
        // does describe them.
        for slug in [
            "higgsfield-ai/dop/standard",
            "higgsfield-ai/dop/lite",
            "higgsfield-ai/soul/standard",
        ] {
            assert!(!higgsfield_speaks_fal(slug), "{slug} is in-house");
        }
        // And a surface we have never seen a path for must not be guessed at.
        assert!(!higgsfield_speaks_fal("marketing_studio_video"));
        assert!(!higgsfield_speaks_fal("cinematic_studio_3_0"));
    }

    #[test]
    fn a_mirrored_route_binds_the_field_their_spec_requires() {
        let spec = spec(vec![media_flag("input_image", Arity::One)]);
        let media = [MediaRef::new(
            MediaRole::Start,
            MediaSource::Url {
                url: "https://ex/a.png".into(),
            },
        )];
        let body = bind(
            &spec,
            "Kling 2.5",
            &media,
            Dialect::Fal,
            "kling-video/v2.5-turbo/pro",
        )
        .expect("the mirror takes a start frame");
        assert_eq!(
            body.get("image_url").and_then(|v| v.as_str()),
            Some("https://ex/a.png"),
            "their openapi requires `image_url`; got {body:?}"
        );
    }

    #[test]
    fn the_end_frame_key_is_the_one_fal_actually_declares() {
        // Measured from fal's own schemas 2026-08-28. `end_image_url` is the
        // rule; `tail_image_url` is one endpoint's exception, not the default
        // it used to be.
        for (slug, want) in [
            ("bytedance/seedance-2.0", "end_image_url"),
            ("bytedance/seedance-2.0/fast", "end_image_url"),
            ("bytedance/seedance-2.0/mini", "end_image_url"),
            ("fal-ai/bytedance/seedance/v1/pro", "end_image_url"),
            ("fal-ai/bytedance/seedance/v1.5/pro", "end_image_url"),
            ("fal-ai/kling-video/v2.6/pro", "end_image_url"),
            ("fal-ai/kling-video/v3/standard", "end_image_url"),
            ("fal-ai/kling-video/o1", "end_image_url"),
            ("fal-ai/kling-video/o3/pro", "end_image_url"),
            ("fal-ai/minimax/hailuo-2.3", "end_image_url"),
            ("fal-ai/wan/v2.7", "end_image_url"),
            ("minimax/h3", "end_image_url"),
            // The two genuine exceptions.
            ("fal-ai/kling-video/v2.5-turbo/pro", "tail_image_url"),
            ("fal-ai/wan-vace-14b", "last_frame_url"),
        ] {
            assert_eq!(
                Dialect::Fal.keys(MediaRole::End, slug).first().copied(),
                Some(want),
                "{slug} end-frame key"
            );
        }
    }

    #[test]
    fn seedance_binds_an_end_frame_fal_will_read() {
        // The regression this fixes: the end frame reached the job record and
        // then vanished into a swept key, and the render was billed without it.
        let spec = spec(vec![
            media_flag("start_image", Arity::One),
            media_flag("end_image", Arity::One),
        ]);
        let media = [
            MediaRef::new(
                MediaRole::Start,
                MediaSource::Url {
                    url: "https://ex/start.png".into(),
                },
            ),
            MediaRef::new(
                MediaRole::End,
                MediaSource::Url {
                    url: "https://ex/end.jpg".into(),
                },
            ),
        ];
        let body = bind(
            &spec,
            "Seedance 2.0",
            &media,
            Dialect::Fal,
            "bytedance/seedance-2.0",
        )
        .expect("both frames bind");
        assert_eq!(
            body.get("end_image_url").and_then(|v| v.as_str()),
            Some("https://ex/end.jpg"),
            "fal declares end_image_url; got {body:?}"
        );
        assert!(
            !body.contains_key("tail_image_url"),
            "the old key would be swept away unread: {body:?}"
        );
    }

    #[test]
    fn an_end_frame_is_refused_where_the_endpoint_has_nowhere_to_put_it() {
        // The costly-mistake guard. fal ignores keys it does not declare, so
        // without this the render runs, looks fine, and is billed in full
        // having never opened the second file.
        let spec = spec(vec![
            media_flag("start_image", Arity::One),
            media_flag("end_image", Arity::One),
        ]);
        for slug in [
            "fal-ai/wan-25-preview",
            "xai/grok-imagine-video/v1.5",
            // Higgsfield's mirror gets no schema sweep at submit, so this list
            // is the only thing standing between the user and a silent drop.
            "bytedance/seedance/v1/pro/fast",
            "kling-video/v2.5-turbo/pro",
            "veo3.1",
        ] {
            assert!(
                !can_bind(&spec, MediaRole::End, Dialect::Fal, slug),
                "{slug} has no end-frame field and must refuse one"
            );
            // ...but the start frame it does take is unaffected.
            assert!(
                can_bind(&spec, MediaRole::Start, Dialect::Fal, slug),
                "{slug} still takes a start frame"
            );
        }
    }

    #[test]
    fn models_that_do_have_an_end_frame_still_take_one() {
        let spec = spec(vec![
            media_flag("start_image", Arity::One),
            media_flag("end_image", Arity::One),
        ]);
        for slug in [
            "bytedance/seedance-2.0",
            "fal-ai/kling-video/v2.5-turbo/pro",
            "fal-ai/kling-video/v3/standard",
            "fal-ai/wan/v2.7",
            "fal-ai/wan-vace-14b",
            "minimax/h3",
        ] {
            assert!(
                can_bind(&spec, MediaRole::End, Dialect::Fal, slug),
                "{slug} declares an end frame and must accept one"
            );
        }
    }

    #[test]
    fn binding_an_end_frame_to_a_route_without_one_names_the_model() {
        let spec = spec(vec![
            media_flag("start_image", Arity::One),
            media_flag("end_image", Arity::One),
        ]);
        let media = [MediaRef::new(
            MediaRole::End,
            MediaSource::Url {
                url: "https://ex/end.jpg".into(),
            },
        )];
        let err = bind(
            &spec,
            "Wan 2.5",
            &media,
            Dialect::Fal,
            "fal-ai/wan-25-preview",
        )
        .expect_err("must refuse rather than drop");
        let msg = err.to_string();
        assert!(msg.contains("Wan 2.5"), "{msg}");
        assert!(msg.contains("end frame"), "{msg}");
    }

    #[test]
    fn one_rule_decides_the_dialect() {
        // Was written out by hand in three places. Higgsfield's mirror speaks
        // fal; only their in-house surfaces speak the catalogue.
        assert_eq!(
            dialect_for(ProviderId::Higgsfield, "kling-video/v2.5-turbo/pro"),
            Dialect::Fal
        );
        assert_eq!(
            dialect_for(ProviderId::Higgsfield, "higgsfield-ai/dop/standard"),
            Dialect::Catalog
        );
        assert_eq!(
            dialect_for(ProviderId::Higgsfield, "marketing_studio_video"),
            Dialect::Catalog
        );
        assert_eq!(dialect_for(ProviderId::Fal, "fal-ai/veo3.1"), Dialect::Fal);
    }
}
