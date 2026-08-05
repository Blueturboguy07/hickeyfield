//! One honest description of what a model can be asked for — and where that
//! answer came from.
//!
//! Two sources describe our models and they describe *different things*:
//!
//! - [`crate::catalog`] is parsed from Higgsfield's MIT CLI spec. It is an
//!   accurate description of **Higgsfield's** API.
//! - [`crate::fal_schema`] is fetched from fal's published per-endpoint
//!   OpenAPI. It is an accurate description of **the endpoint we are actually
//!   going to POST to**.
//!
//! We call fal for most of the roster, so the catalogue is a *secondary*
//! source there, and reading it as primary cost real money. Measured
//! 2026-08-05: the catalogue says Gemini Omni takes `image_references` and
//! `video_references`; fal's `google/gemini-omni-flash` takes `prompt`,
//! `duration` and `aspect_ratio` and nothing else. A user attached a clip,
//! asked for an edit, and got an unrelated text-to-video generation — billed in
//! full — because fal drops a field it does not recognise instead of rejecting
//! the request. **An ignored input is not free; it is a charge for the wrong
//! thing.**
//!
//! So this module answers every question with a [`Source`] attached, and adds a
//! third possible answer — [`Support::Unknown`] — which must never be rendered
//! as a plausible default. "We do not know whether this model does 4k" and
//! "this model does 4k" are different sentences and the UI has to be able to
//! tell them apart.
//!
//! It builds on [`crate::catalog::Capabilities`] and [`crate::media`] rather
//! than restating them: the catalogue path is `Capabilities` re-labelled with
//! its provenance, and role support is decided against [`Dialect::Fal`]'s own
//! key table.
//!
//! Nothing here touches the network. The caller supplies the schema lookup
//! (usually [`crate::fal_schema::for_endpoint`]) so that listing 68 models does
//! not mean 68 HTTP requests, and so the tests stay offline.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::catalog::{Capabilities, Modality, ModelSpec};
use crate::fal_schema::EndpointSchema;
use crate::media::{self, Dialect, InputMode, MediaRef, MediaRole};
use crate::provider::ProviderId;

/// Every media role, in the order the UI should render slots.
///
/// Fixed and exhaustive so a caller can iterate slots without asking whether a
/// role was omitted because it is unsupported or because we forgot it — those
/// are different answers and only [`Support`] is allowed to carry them.
pub const ROLES: [MediaRole; 7] = [
    MediaRole::Start,
    MediaRole::End,
    MediaRole::Reference,
    MediaRole::Video,
    MediaRole::VideoReference,
    MediaRole::Audio,
    MediaRole::AudioReference,
];

/// Which fal fields the three named axes occupy.
///
/// Exact names only, no fuzzy matching. Anything else fal enumerates lands in
/// [`ModelCapability::unmapped_enums`] verbatim instead of being filed under an
/// axis it may not be: deciding that an unfamiliar enum is "really" the aspect
/// control would eventually put a value on the wire under a name nobody
/// verified, and fal drops what it does not recognise instead of rejecting it.
const MAPPED_FIELDS: [&str; 3] = [FAL_DURATION, FAL_RESOLUTION, FAL_ASPECT];

const FAL_DURATION: &str = "duration";
const FAL_RESOLUTION: &str = "resolution";
const FAL_ASPECT: &str = "aspect_ratio";

/// The two flags that mean "this model emits sound".
///
/// The pair [`crate::catalog`] keys off, attested throughout the vendored spec.
/// `audio`/`audio_references` are deliberately excluded: those are *inputs*, and
/// counting them would put an audio toggle on lipsync models that only consume
/// a track.
///
/// These names are the catalogue's and are only *tried* against a fal schema —
/// a miss costs nothing, because [`audio_output`] then falls through to the
/// catalogue's own answer and finally to `Unknown`. It can never produce a
/// false Yes.
const AUDIO_OUTPUT_FLAGS: [&str; 2] = ["generate_audio", "sound"];

/// Who told us.
///
/// Ordered worst-to-best so `max` picks the better of two answers, and so a
/// caller can sort or badge by confidence without a lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Nobody could answer. Carries no values and no default, ever.
    #[default]
    Unknown,
    /// The vendored Higgsfield CLI spec. Correct about Higgsfield's API, and a
    /// plausible-but-unverified guess about anyone else's.
    Catalog,
    /// fal's published schema for the exact endpoint this request will hit.
    FalEndpoint,
}

impl Source {
    /// True when the answer came from the API we are actually going to call.
    ///
    /// The UI can show a fal-sourced option set plainly and a catalogue-sourced
    /// one with a caveat; it must not present them identically, because one of
    /// them has already been wrong in a way that charged a user.
    pub fn is_authoritative(self) -> bool {
        matches!(self, Source::FalEndpoint)
    }

    /// Human-readable provenance, for a tooltip or an error.
    pub fn label(self) -> &'static str {
        match self {
            Source::Unknown => "not established",
            Source::Catalog => "the vendored Higgsfield catalogue",
            Source::FalEndpoint => "fal's published endpoint schema",
        }
    }
}

/// Does this control exist?
///
/// Three answers, not two. [`Support::Unknown`] is **not** [`Support::No`]:
/// rendering Unknown as No greys out a control that works, and rendering it as
/// Yes offers a control whose input fal may silently drop and bill for. Callers
/// must handle all three; that is the point of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Yes,
    No,
    #[default]
    Unknown,
}

impl Support {
    /// Turn a source that *did* answer into a verdict.
    ///
    /// Only call this when the absence of a thing is itself an answer — fal
    /// lists every field it accepts, so a field missing from the schema is a
    /// definite No. Never call it on a source that is merely silent.
    pub fn from_known(present: bool) -> Self {
        if present {
            Support::Yes
        } else {
            Support::No
        }
    }

    pub fn is_yes(self) -> bool {
        matches!(self, Support::Yes)
    }

    pub fn is_no(self) -> bool {
        matches!(self, Support::No)
    }

    /// True when some source answered either way.
    pub fn is_known(self) -> bool {
        !matches!(self, Support::Unknown)
    }
}

/// A yes/no capability with its provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Flag {
    pub support: Support,
    pub source: Source,
}

impl Flag {
    /// Nobody answered.
    pub fn unknown() -> Self {
        Flag {
            support: Support::Unknown,
            source: Source::Unknown,
        }
    }

    /// `source` answered `support`.
    pub fn known(support: Support, source: Source) -> Self {
        Flag { support, source }
    }
}

/// One option axis — duration, resolution, aspect.
///
/// Four separate facts, because collapsing any pair of them has already
/// produced a bug:
///
/// - `support` — the control exists. `No` hides the chip row; `Unknown` must
///   not render as either a value or an absence the user can rely on.
/// - `values` — the enumerated choices. Empty *with* `support == Yes` means the
///   source declares a free value (28 of 32 video models declare `duration` as
///   a plain integer), and the UI must offer an input rather than invent a
///   list. See [`Axis::is_free_form`].
/// - `default` — what to open on. Always one of `values` when `values` is
///   non-empty; see [`Axis::supported`].
/// - `source` / `default_source` — who said so.
///
/// `support` and `values` always share one source, because an axis takes all of
/// its *content* from a single place: grafting the catalogue's enumeration onto
/// a fal-declared field is how the chip row came to offer 4k on a 720p model.
/// The default is the one exception and gets its own provenance — see
/// `default_source`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Axis {
    pub support: Support,
    pub values: Vec<String>,
    pub default: Option<String>,
    /// Where `support` and `values` came from.
    pub source: Source,
    /// Where `default` came from, which can differ from `source`.
    ///
    /// fal publishes a `default` per property but [`EndpointSchema`] does not
    /// parse it yet, so a fal-sourced axis borrows the catalogue's default —
    /// and only after [`EndpointSchema::coerce`] confirms fal still offers that
    /// exact value. `Source::Unknown` here with a `None` default means: send
    /// nothing and let the provider apply its own, which is always safer than
    /// guessing one.
    pub default_source: Source,
}

impl Axis {
    /// Nobody could answer. Renders no control and no value.
    pub fn unknown() -> Self {
        Axis {
            support: Support::Unknown,
            values: Vec::new(),
            default: None,
            source: Source::Unknown,
            default_source: Source::Unknown,
        }
    }

    /// `source` says this model has no such control.
    pub fn unsupported(source: Source) -> Self {
        Axis {
            support: Support::No,
            values: Vec::new(),
            default: None,
            source,
            default_source: Source::Unknown,
        }
    }

    /// `source` says the control exists, offering `values` (empty = free-form).
    ///
    /// Drops a `default` that is not among a non-empty `values`. That happens
    /// for real — the catalogue's default duration for a model is 5 while fal
    /// enumerates 4s/6s/8s — and keeping it would open the chip row on a value
    /// the provider rejects after the round trip, with the wrong price already
    /// printed on the Generate button. Dropping it is not a silent substitution:
    /// no neighbouring value is chosen in its place.
    pub fn supported(
        values: Vec<String>,
        default: Option<String>,
        source: Source,
        default_source: Source,
    ) -> Self {
        let default = default.filter(|d| values.is_empty() || values.iter().any(|v| v == d));
        let default_source = if default.is_some() {
            default_source
        } else {
            Source::Unknown
        };
        Axis {
            support: Support::Yes,
            values,
            default,
            source,
            default_source,
        }
    }

    /// The control exists but its source enumerates no choices, so the model
    /// takes a free value.
    ///
    /// Distinct from "no control at all", and conflating the two is what made
    /// the chip row report the same three durations for all 68 models.
    pub fn is_free_form(&self) -> bool {
        self.support.is_yes() && self.values.is_empty()
    }
}

/// Whether the endpoint genuinely accepts media in this role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSupport {
    pub role: MediaRole,
    /// `No` means: attaching this would be dropped on the floor and billed for.
    /// The UI should grey the slot out, and the submit path should refuse
    /// loudly rather than send it.
    pub support: Support,
    /// Whether the endpoint cannot run without it.
    pub required: Support,
    /// The wire key this role binds to, when a source named one.
    ///
    /// Two roles can share one key — fal has a single `video_url` for both
    /// [`MediaRole::Video`] and [`MediaRole::VideoReference`], and a single
    /// `audio_url` for both audio roles. The UI must render **one slot per
    /// distinct key**, because [`media::bind`] writes roles into a map and the
    /// second attachment would overwrite the first without saying so.
    pub key: Option<String>,
    pub source: Source,
}

impl RoleSupport {
    /// Nobody could answer for this role.
    pub fn unknown(role: MediaRole) -> Self {
        RoleSupport {
            role,
            support: Support::Unknown,
            required: Support::Unknown,
            key: None,
            source: Source::Unknown,
        }
    }
}

/// Everything one model can be asked for on one route, with provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapability {
    pub model_id: String,
    /// The best source that answered anything here. A one-glance confidence
    /// badge; the per-axis `source` is what a specific control should trust.
    pub source: Source,
    /// The exact endpoint these answers describe, when fal answered.
    ///
    /// `None` for a catalogue-derived description: the catalogue is about
    /// Higgsfield's API and names no fal endpoint.
    pub endpoint: Option<String>,
    pub prompt: Flag,
    pub duration: Axis,
    pub resolution: Axis,
    pub aspect: Axis,
    /// The model *produces* sound, as opposed to merely accepting a track.
    pub audio_output: Flag,
    /// One entry per [`ROLES`] member, in that order.
    pub roles: Vec<RoleSupport>,
    /// Media fields the endpoint accepts that no role claimed.
    ///
    /// Non-empty means [`Dialect::Fal`]'s key table is behind the endpoint —
    /// `wan/*/vace` alone renames three roles. Surfacing them is what keeps the
    /// affected roles at `Unknown` instead of a wrong `No` that would grey out
    /// a slot the endpoint really does serve.
    pub unmapped_media_keys: Vec<String>,
    /// Enumerated fields the endpoint offers that we have no axis for
    /// (`image_size`, `style`, camera controls). Carried verbatim so a future
    /// control can be built from fal's own list rather than transcribed.
    pub unmapped_enums: BTreeMap<String, Vec<String>>,
    /// Cross-field rules the flag tables cannot express, verbatim from the
    /// catalogue. fal's input schema publishes no prose, so these stay
    /// catalogue-sourced even on a fal-derived description — they are the only
    /// ones we have.
    pub constraints: Vec<String>,
}

impl ModelCapability {
    /// What we know when we know nothing.
    ///
    /// Deliberately barren. Every axis is `Unknown` with no values and no
    /// default, and every role is `Unknown`. The predecessor of this type
    /// assumed durations [5, 8, 10] and resolutions ["720p", "1080p"] for every
    /// model, so the chip row offered 10s on a 5s-only model, the estimator
    /// quoted 10s, the button printed that price and the provider rejected the
    /// job after the round trip.
    pub fn unknown(model_id: &str) -> Self {
        ModelCapability {
            model_id: model_id.to_string(),
            source: Source::Unknown,
            endpoint: None,
            prompt: Flag::unknown(),
            duration: Axis::unknown(),
            resolution: Axis::unknown(),
            aspect: Axis::unknown(),
            audio_output: Flag::unknown(),
            roles: ROLES.iter().copied().map(RoleSupport::unknown).collect(),
            unmapped_media_keys: Vec::new(),
            unmapped_enums: BTreeMap::new(),
            constraints: Vec::new(),
        }
    }

    /// Source #2: the vendored catalogue's own answer, labelled as such.
    ///
    /// Correct for Higgsfield's API and the best guess we have for anyone
    /// else's. Used whenever fal cannot be asked — a non-fal route, an
    /// unresolvable endpoint, or no network.
    pub fn from_catalog(spec: &ModelSpec) -> Self {
        let cat = spec.capabilities();

        let axis = |supported: bool, values: Vec<String>, default: Option<String>| {
            if supported {
                Axis::supported(values, default, Source::Catalog, Source::Catalog)
            } else {
                Axis::unsupported(Source::Catalog)
            }
        };

        let roles = ROLES
            .iter()
            .copied()
            .map(|role| {
                // The catalogue answers role support through the flag table,
                // which is exactly what `media::bind` checks before binding —
                // so this cannot disagree with what a request would actually do.
                let flag = media::flag_for(spec, role);
                RoleSupport {
                    role,
                    support: Support::from_known(flag.is_some()),
                    required: match flag.and_then(|f| spec.flag(f)) {
                        Some(f) => Support::from_known(f.required),
                        None => Support::No,
                    },
                    key: flag.map(String::from),
                    source: Source::Catalog,
                }
            })
            .collect();

        ModelCapability {
            model_id: spec.id.clone(),
            source: Source::Catalog,
            endpoint: None,
            prompt: Flag::known(Support::from_known(spec.takes_prompt()), Source::Catalog),
            duration: axis(
                cat.supports_duration,
                cat.durations.iter().copied().map(seconds_label).collect(),
                cat.default_duration.map(seconds_label),
            ),
            resolution: axis(
                cat.supports_resolution,
                cat.resolutions.clone(),
                cat.default_resolution.clone(),
            ),
            aspect: axis(
                cat.supports_aspect,
                cat.aspects.clone(),
                cat.default_aspect.clone(),
            ),
            audio_output: audio_output(None, &cat),
            roles,
            unmapped_media_keys: Vec::new(),
            unmapped_enums: BTreeMap::new(),
            constraints: cat.constraints,
        }
    }

    /// Source #1: fal's own schema for the endpoint this request will hit.
    ///
    /// fal's document lists every field the endpoint accepts, so a field that
    /// is absent is a **definite No**, not silence — that is what makes this
    /// authoritative and what would have caught the Gemini Omni charge. The
    /// catalogue is still consulted for two things fal's *input* schema cannot
    /// answer: the constraint prose, and whether the model emits audio.
    pub fn from_fal(spec: &ModelSpec, endpoint: &str, schema: &EndpointSchema) -> Self {
        let cat = spec.capabilities();
        let (roles, unmapped_media_keys) = fal_roles(schema, endpoint);

        ModelCapability {
            model_id: spec.id.clone(),
            source: Source::FalEndpoint,
            endpoint: Some(endpoint.to_string()),
            prompt: Flag::known(
                Support::from_known(schema.accepts("prompt")),
                Source::FalEndpoint,
            ),
            duration: fal_axis(
                schema,
                FAL_DURATION,
                cat.default_duration.map(seconds_label),
            ),
            resolution: fal_axis(schema, FAL_RESOLUTION, cat.default_resolution.clone()),
            aspect: fal_axis(schema, FAL_ASPECT, cat.default_aspect.clone()),
            audio_output: audio_output(Some(schema), &cat),
            roles,
            unmapped_media_keys,
            unmapped_enums: schema
                .enums
                .iter()
                .filter(|(k, _)| !MAPPED_FIELDS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            constraints: cat.constraints,
        }
    }

    /// This model's answer for one role.
    ///
    /// An entry missing entirely (only reachable through a hand-built or
    /// deserialised value) degrades to `Unknown` rather than panicking — the
    /// safe direction, since `Unknown` never renders as support.
    pub fn role(&self, role: MediaRole) -> RoleSupport {
        self.roles
            .iter()
            .find(|r| r.role == role)
            .cloned()
            .unwrap_or_else(|| RoleSupport::unknown(role))
    }

    /// Attached roles this endpoint is **known** to drop.
    ///
    /// The whole reason this module exists. fal ignores a field it does not
    /// recognise and bills for the generation anyway, so a non-empty result
    /// here must stop the submit and say which attachment cannot be used —
    /// refusing costs a click, sending costs the user money and their edit.
    pub fn ignored_roles(&self, media: &[MediaRef]) -> Vec<MediaRole> {
        self.attached_roles(media, Support::is_no)
    }

    /// Attached roles no source could vouch for.
    ///
    /// Not the same list as [`ModelCapability::ignored_roles`] and not
    /// actionable the same way: these may work. The caller decides whether to
    /// warn or to proceed, but it must not silently treat them as supported.
    pub fn unverified_roles(&self, media: &[MediaRef]) -> Vec<MediaRole> {
        self.attached_roles(media, |s| !s.is_known())
    }

    fn attached_roles(&self, media: &[MediaRef], pick: impl Fn(Support) -> bool) -> Vec<MediaRole> {
        let mut out: Vec<MediaRole> = Vec::new();
        for m in media {
            if pick(self.role(m.role).support) && !out.contains(&m.role) {
                out.push(m.role);
            }
        }
        out
    }

    /// The duration choices as seconds, for the estimator.
    ///
    /// `None` when any offered value does not parse as seconds. A filtered list
    /// would be worse than no list: the Generate button prices whatever the
    /// user picked, so quietly dropping one option can only produce a quote for
    /// something else.
    pub fn duration_seconds(&self) -> Option<Vec<f64>> {
        self.duration
            .values
            .iter()
            .map(|v| parse_seconds(v))
            .collect()
    }

    /// The default duration in seconds, when there is one and it parses.
    pub fn default_duration_seconds(&self) -> Option<f64> {
        self.duration.default.as_deref().and_then(parse_seconds)
    }
}

/// The fal endpoint a request on this route will actually be posted to.
///
/// `None` — meaning "do not ask fal about this" — for a non-fal provider, for a
/// slug fal is measured not to serve at all, and for a mode this family does
/// not offer. In every one of those cases a fetched schema would be about the
/// wrong thing or about nothing.
pub fn fal_endpoint(
    spec: &ModelSpec,
    provider: ProviderId,
    slug: &str,
    mode: InputMode,
) -> Option<String> {
    if provider != ProviderId::Fal || media::route_is_missing(slug) {
        return None;
    }
    media::resolve_endpoint(slug, mode, spec.modality == Modality::Video).ok()
}

/// Describe this model on this route, in the documented source order:
/// fal's endpoint schema, then the vendored catalogue, then unknown.
///
/// `schema_for` is passed in rather than called directly so this module makes
/// no network requests: hand it [`crate::fal_schema::for_endpoint`] on the
/// submit path where one blocking fetch is worth it, and `|_| None` when
/// listing the whole roster — which would otherwise be 68 HTTP requests to draw
/// a picker.
///
/// Note the fallback is the *catalogue*, not `unknown`: being offline must not
/// blank the UI, it must only downgrade its confidence.
pub fn for_route(
    spec: &ModelSpec,
    provider: ProviderId,
    slug: &str,
    mode: InputMode,
    schema_for: impl FnOnce(&str) -> Option<EndpointSchema>,
) -> ModelCapability {
    match fal_endpoint(spec, provider, slug, mode) {
        Some(endpoint) => match schema_for(&endpoint) {
            Some(schema) => ModelCapability::from_fal(spec, &endpoint, &schema),
            None => ModelCapability::from_catalog(spec),
        },
        None => ModelCapability::from_catalog(spec),
    }
}

/// One axis from fal's schema.
///
/// A field absent from `accepted` is `No`, not `Unknown`: fal's document lists
/// every field the endpoint takes, so absence is an answer. A field present
/// with no published enum is free-form — reporting the catalogue's enumeration
/// there would be describing Higgsfield's API while claiming fal's authority.
fn fal_axis(schema: &EndpointSchema, field: &str, catalog_default: Option<String>) -> Axis {
    if !schema.accepts(field) {
        return Axis::unsupported(Source::FalEndpoint);
    }
    let values = schema.enums.get(field).cloned().unwrap_or_default();
    // Borrow the catalogue's default only when fal enumerates the field and
    // `coerce` finds that exact value in fal's own list — including across the
    // spelling differences that are pure noise (`1k`/`1K`, `4`/`4s`). With no
    // enum to check against there is nothing to validate, so we send nothing
    // and let fal apply its own default rather than assert one.
    let default = if values.is_empty() {
        None
    } else {
        catalog_default.and_then(|d| schema.coerce(field, &d))
    };
    Axis::supported(values, default, Source::FalEndpoint, Source::Catalog)
}

/// Whether the model emits sound.
///
/// The one axis fal's *input* schema structurally cannot settle: audio output
/// is a property of the result, and an endpoint only reveals it when it happens
/// to expose a toggle. Veo 3.1 always produces audio and publishes no flag, so
/// reading fal's silence as "no audio" would deny it on the models most likely
/// to have it. Hence: fal's toggle if there is one, else the catalogue's
/// affirmative, else `Unknown` — never a manufactured `No`.
fn audio_output(schema: Option<&EndpointSchema>, cat: &Capabilities) -> Flag {
    if let Some(s) = schema {
        if AUDIO_OUTPUT_FLAGS.iter().any(|f| s.accepts(f)) {
            return Flag::known(Support::Yes, Source::FalEndpoint);
        }
    }
    if cat.audio {
        return Flag::known(Support::Yes, Source::Catalog);
    }
    Flag::unknown()
}

/// Role support from fal, plus the media keys no role claimed.
fn fal_roles(schema: &EndpointSchema, endpoint: &str) -> (Vec<RoleSupport>, Vec<String>) {
    // Which key each role would bind to, using the same table `media::bind`
    // writes with — so a `Yes` here means a request would genuinely carry it.
    let bound: Vec<(MediaRole, Option<&'static str>)> = ROLES
        .iter()
        .map(|&role| {
            let key = Dialect::Fal
                .keys(role, endpoint)
                .iter()
                .copied()
                .find(|k| schema.accepts(k));
            (role, key)
        })
        .collect();

    let claimed: BTreeSet<&str> = bound.iter().filter_map(|(_, k)| *k).collect();
    let unmapped: Vec<String> = schema
        .media_keys()
        .into_iter()
        .filter(|k| !claimed.contains(k))
        .map(String::from)
        .collect();

    let roles = bound
        .iter()
        .map(|&(role, key)| {
            let support = match key {
                Some(_) => Support::Yes,
                // The endpoint takes media of this kind under a name our key
                // table does not know — `wan/*/vace` renames three roles at
                // once. Saying `No` would grey out a slot that works, so the
                // honest answer is that we cannot tell.
                None if unmapped.iter().any(|k| could_carry(role, k)) => Support::Unknown,
                None => Support::No,
            };
            RoleSupport {
                role,
                support,
                required: match key {
                    Some(k) => Support::from_known(schema.required.contains(k)),
                    // A role the endpoint does not accept cannot be required;
                    // a role we cannot place stays unplaceable.
                    None => support,
                },
                key: key.map(String::from),
                source: Source::FalEndpoint,
            }
        })
        .collect();

    (roles, unmapped)
}

/// Image, video or sound — the coarse kind of a media field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Medium {
    Image,
    Video,
    Audio,
}

fn medium_of_role(role: MediaRole) -> Medium {
    match role {
        MediaRole::Start | MediaRole::End | MediaRole::Reference => Medium::Image,
        MediaRole::Video | MediaRole::VideoReference => Medium::Video,
        MediaRole::Audio | MediaRole::AudioReference => Medium::Audio,
    }
}

/// The kind of media a fal field name carries, when the name says.
///
/// Audio is matched first because `audio_url` would otherwise be caught by
/// nothing and `video` before `image` because no fal key names both. `frame`
/// counts as an image: `first_frame_url` and `last_frame_url` take stills.
fn medium_of_key(key: &str) -> Option<Medium> {
    if ["audio", "voice", "speech", "music"]
        .iter()
        .any(|s| key.contains(s))
    {
        Some(Medium::Audio)
    } else if key.contains("video") {
        Some(Medium::Video)
    } else if ["image", "frame", "mask"].iter().any(|s| key.contains(s)) {
        Some(Medium::Image)
    } else {
        None
    }
}

/// Could this unclaimed field be this role's, under a name we do not know?
///
/// A field whose name says nothing (`input_url`) counts for every role: it is
/// precisely the case where we cannot rule the role out, and the point of
/// `Unknown` is to stop us pretending we can.
fn could_carry(role: MediaRole, key: &str) -> bool {
    medium_of_key(key).is_none_or(|m| m == medium_of_role(role))
}

/// `5` not `5.0`, `5.5` kept. Rust has no `{:g}`.
fn seconds_label(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Seconds written `5`, `5s` or `5.5`. Anything else is `None` rather than a
/// guess, because a duration we misread becomes a price we misquote.
fn parse_seconds(raw: &str) -> Option<f64> {
    raw.trim().trim_end_matches('s').trim().parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{self, Arity, FlagSpec, ValueSpec};

    fn schema(accepted: &[&str], required: &[&str], enums: &[(&str, &[&str])]) -> EndpointSchema {
        EndpointSchema {
            accepted: accepted.iter().map(|s| s.to_string()).collect(),
            required: required.iter().map(|s| s.to_string()).collect(),
            enums: enums
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect(),
        }
    }

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

    fn media_flag(name: &str) -> FlagSpec {
        flag(name, ValueSpec::Media)
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

    /// The catalogue's real entry for the model that cost a user money.
    ///
    /// Not a hand-built approximation: this is the vendored row itself —
    /// `--image-references` (0..7), `--video-references` (single), duration
    /// 4/6/8/10, resolution 720p — because the bug was that we believed it.
    fn gemini_omni_spec() -> ModelSpec {
        catalog::catalogue()
            .remove("gemini_omni")
            .expect("gemini_omni is in the vendored spec")
    }

    // ── The bug that named this module ─────────────────────────────────────

    #[test]
    fn a_source_clip_slot_is_closed_on_an_endpoint_that_takes_no_video() {
        // Measured 2026-08-05: the catalogue says Gemini Omni takes video
        // references; `google/gemini-omni-flash` takes prompt, duration and
        // aspect_ratio. The user's clip was dropped and the generation billed.
        let s = schema(&["prompt", "duration", "aspect_ratio"], &["prompt"], &[]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "google/gemini-omni-flash", &s);

        assert_eq!(cap.role(MediaRole::Video).support, Support::No);
        assert_eq!(cap.role(MediaRole::Reference).support, Support::No);
        assert_eq!(cap.role(MediaRole::Start).support, Support::No);
        assert!(cap.unmapped_media_keys.is_empty());
        // And the verdict is fal's, so the UI can grey the slot out plainly
        // rather than hedging.
        assert!(cap.role(MediaRole::Video).source.is_authoritative());
    }

    #[test]
    fn the_catalogue_and_fal_disagree_and_fal_wins() {
        let spec = gemini_omni_spec();
        let from_cat = ModelCapability::from_catalog(&spec);
        assert_eq!(from_cat.role(MediaRole::Video).support, Support::Yes);

        let s = schema(&["prompt", "duration", "aspect_ratio"], &["prompt"], &[]);
        let cap = for_route(
            &spec,
            ProviderId::Fal,
            "google/gemini-omni-flash",
            InputMode::Video,
            |_| Some(s.clone()),
        );
        assert_eq!(cap.source, Source::FalEndpoint);
        assert_eq!(cap.role(MediaRole::Video).support, Support::No);
    }

    #[test]
    fn an_attachment_the_endpoint_would_drop_is_named_before_it_is_billed() {
        let s = schema(&["prompt", "duration"], &["prompt"], &[]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "google/gemini-omni-flash", &s);
        let attached = [
            MediaRef::url(MediaRole::Video, "https://cdn.example/clip.mp4"),
            MediaRef::url(MediaRole::Video, "https://cdn.example/other.mp4"),
        ];
        // Reported once, not once per file — this is a message to a human.
        assert_eq!(cap.ignored_roles(&attached), vec![MediaRole::Video]);
        assert!(cap.unverified_roles(&attached).is_empty());
    }

    #[test]
    fn an_attachment_the_endpoint_accepts_is_not_reported_as_ignored() {
        let s = schema(&["prompt", "image_url"], &["prompt", "image_url"], &[]);
        let cap = ModelCapability::from_fal(
            &spec(vec![flag("prompt", ValueSpec::Text), media_flag("image")]),
            "fal-ai/kling-video/v3/standard/image-to-video",
            &s,
        );
        let attached = [MediaRef::url(MediaRole::Start, "https://cdn.example/a.png")];
        assert!(cap.ignored_roles(&attached).is_empty());
        assert_eq!(cap.role(MediaRole::Start).support, Support::Yes);
        assert_eq!(cap.role(MediaRole::Start).required, Support::Yes);
        assert_eq!(cap.role(MediaRole::Start).key.as_deref(), Some("image_url"));
    }

    // ── Unknown must never look like an answer ─────────────────────────────

    #[test]
    fn unknown_carries_no_values_and_no_defaults() {
        let cap = ModelCapability::unknown("mystery_model");
        for axis in [&cap.duration, &cap.resolution, &cap.aspect] {
            assert_eq!(axis.support, Support::Unknown);
            assert!(axis.values.is_empty());
            assert_eq!(axis.default, None);
            assert_eq!(axis.source, Source::Unknown);
            assert_eq!(axis.default_source, Source::Unknown);
            // An unknown axis is not a free-form axis: one renders an input,
            // the other must render nothing.
            assert!(!axis.is_free_form());
        }
        assert_eq!(cap.audio_output.support, Support::Unknown);
        assert_eq!(cap.prompt.support, Support::Unknown);
        assert!(cap.roles.iter().all(|r| r.support == Support::Unknown));
        assert!(cap.constraints.is_empty());
    }

    #[test]
    fn unknown_support_is_neither_yes_nor_no() {
        // Callers that branch on `is_yes()` alone would offer nothing, and
        // callers that branch on `!is_no()` would offer everything. Both are
        // wrong, so the third state has to be visible in the type.
        assert!(!Support::Unknown.is_yes());
        assert!(!Support::Unknown.is_no());
        assert!(!Support::Unknown.is_known());
        assert!(Support::No.is_known());
    }

    #[test]
    fn every_constructor_answers_for_every_role() {
        // `role()` degrades an absent entry to Unknown, so a missing role would
        // silently close a slot rather than fail — the invariant is checked
        // here instead.
        let s = schema(&["prompt"], &[], &[]);
        for cap in [
            ModelCapability::unknown("x"),
            ModelCapability::from_catalog(&gemini_omni_spec()),
            ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s),
        ] {
            assert_eq!(cap.roles.len(), ROLES.len());
            for role in ROLES {
                assert!(cap.roles.iter().any(|r| r.role == role), "missing {role:?}");
            }
        }
    }

    // ── Axis provenance and defaults ───────────────────────────────────────

    #[test]
    fn a_field_fal_does_not_list_is_a_definite_no_not_unknown() {
        // fal's document enumerates every accepted field, so absence is an
        // answer. Reporting Unknown here would leave a resolution chip row up
        // on a model that has no such control.
        let s = schema(&["prompt"], &["prompt"], &[]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s);
        assert_eq!(cap.resolution.support, Support::No);
        assert_eq!(cap.resolution.source, Source::FalEndpoint);
        assert!(cap.resolution.values.is_empty());
    }

    #[test]
    fn a_fal_field_with_no_published_enum_is_free_form_not_absent() {
        let s = schema(&["prompt", "duration"], &[], &[]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s);
        assert!(cap.duration.is_free_form());
        assert_eq!(cap.duration.support, Support::Yes);
        // The catalogue enumerates 4/6/8/10 for this model. Grafting that onto
        // a fal-declared field would claim fal's authority for Higgsfield's
        // answer, which is the inversion this whole module exists to undo.
        assert!(cap.duration.values.is_empty());
    }

    #[test]
    fn a_catalogue_default_fal_does_not_offer_is_dropped_not_snapped_to_a_neighbour() {
        // The catalogue's default duration is 5; fal offers 4s/6s/8s. Opening
        // on 4s would charge for a length the user never picked, and opening on
        // 5 would 422 after the round trip with the wrong price already shown.
        let s = schema(
            &["prompt", "duration"],
            &[],
            &[("duration", &["4s", "6s", "8s"])],
        );
        let m = spec(vec![FlagSpec {
            default: Some("5".into()),
            ..flag("duration", ValueSpec::Integer)
        }]);
        let cap = ModelCapability::from_fal(&m, "x/y", &s);
        assert_eq!(cap.duration.values, ["4s", "6s", "8s"]);
        assert_eq!(cap.duration.default, None);
        assert_eq!(cap.duration.default_source, Source::Unknown);
    }

    #[test]
    fn a_catalogue_default_survives_when_only_the_spelling_differs() {
        // Measured: we say `8`, fal's Veo says `8s`; we say `1k`, fal's Nano
        // Banana says `1K`. Same value, and refusing to carry it across would
        // leave the chip row with nothing selected for no reason.
        let s = schema(
            &["duration", "resolution"],
            &[],
            &[
                ("duration", &["4s", "6s", "8s"]),
                ("resolution", &["1K", "2K"]),
            ],
        );
        let m = spec(vec![
            FlagSpec {
                default: Some("8".into()),
                ..flag("duration", ValueSpec::Integer)
            },
            FlagSpec {
                default: Some("1k".into()),
                ..flag(
                    "resolution",
                    ValueSpec::Enum(vec!["1k".into(), "2k".into()]),
                )
            },
        ]);
        let cap = ModelCapability::from_fal(&m, "x/y", &s);
        assert_eq!(cap.duration.default.as_deref(), Some("8s"));
        assert_eq!(cap.resolution.default.as_deref(), Some("1K"));
        // The values are fal's; the default is the catalogue's, checked against
        // them. The UI can tell those apart because the sources differ.
        assert_eq!(cap.duration.source, Source::FalEndpoint);
        assert_eq!(cap.duration.default_source, Source::Catalog);
    }

    #[test]
    fn a_default_outside_the_offered_values_is_dropped_by_construction() {
        let a = Axis::supported(
            vec!["720p".into(), "1080p".into()],
            Some("4k".into()),
            Source::Catalog,
            Source::Catalog,
        );
        assert_eq!(a.default, None);
        assert_eq!(a.default_source, Source::Unknown);

        // A free-form axis has nothing to check the default against, so a
        // default alone is legitimate — 28 of 32 video models are shaped this
        // way (`duration` is a plain integer with a documented default).
        let free = Axis::supported(vec![], Some("5".into()), Source::Catalog, Source::Catalog);
        assert_eq!(free.default.as_deref(), Some("5"));
    }

    #[test]
    fn an_unsupported_axis_keeps_no_leftover_values() {
        let a = Axis::unsupported(Source::FalEndpoint);
        assert!(a.values.is_empty());
        assert_eq!(a.default, None);
        assert!(!a.is_free_form());
    }

    // ── Audio output ───────────────────────────────────────────────────────

    #[test]
    fn audio_output_is_unknown_rather_than_no_when_neither_source_affirms_it() {
        // Veo 3.1 always produces audio and fal's input schema has no field for
        // it. `No` would be a claim neither source made.
        let s = schema(&["prompt", "duration"], &[], &[]);
        let cap =
            ModelCapability::from_fal(&spec(vec![flag("prompt", ValueSpec::Text)]), "x/y", &s);
        assert_eq!(cap.audio_output.support, Support::Unknown);
        assert_eq!(cap.audio_output.source, Source::Unknown);
    }

    #[test]
    fn a_fal_audio_toggle_settles_audio_output() {
        let s = schema(&["prompt", "generate_audio"], &[], &[]);
        let cap =
            ModelCapability::from_fal(&spec(vec![flag("prompt", ValueSpec::Text)]), "x/y", &s);
        assert_eq!(cap.audio_output.support, Support::Yes);
        assert!(cap.audio_output.source.is_authoritative());
    }

    #[test]
    fn the_catalogue_can_affirm_audio_output_fal_does_not_mention() {
        let s = schema(&["prompt"], &[], &[]);
        let m = spec(vec![
            flag("prompt", ValueSpec::Text),
            flag("generate_audio", ValueSpec::Boolean),
        ]);
        let cap = ModelCapability::from_fal(&m, "x/y", &s);
        assert_eq!(cap.audio_output.support, Support::Yes);
        assert_eq!(cap.audio_output.source, Source::Catalog);
    }

    // ── Roles fal names differently ────────────────────────────────────────

    #[test]
    fn a_media_key_no_role_claims_leaves_that_role_unknown_not_closed() {
        // `wan/*/vace` renames start, end and reference at once. If the slug
        // does not say `vace`, our key table misses all three — and answering
        // `No` would grey out slots the endpoint genuinely serves.
        let s = schema(
            &["prompt", "first_frame_url", "last_frame_url"],
            &["prompt"],
            &[],
        );
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "fal-ai/wan/v2.2-a14b", &s);
        assert_eq!(cap.role(MediaRole::Start).support, Support::Unknown);
        assert_eq!(cap.role(MediaRole::End).support, Support::Unknown);
        // Different medium: no unclaimed key could be a source clip, so this
        // stays a definite No.
        assert_eq!(cap.role(MediaRole::Video).support, Support::No);
        assert_eq!(cap.role(MediaRole::Audio).support, Support::No);
        assert_eq!(
            cap.unmapped_media_keys,
            ["first_frame_url", "last_frame_url"]
        );
    }

    #[test]
    fn the_vace_slug_maps_its_renamed_keys_instead_of_reporting_unknown() {
        let s = schema(
            &[
                "prompt",
                "first_frame_url",
                "last_frame_url",
                "ref_image_urls",
            ],
            &["prompt"],
            &[],
        );
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "fal-ai/wan/vace/14b", &s);
        assert_eq!(cap.role(MediaRole::Start).support, Support::Yes);
        assert_eq!(cap.role(MediaRole::End).support, Support::Yes);
        assert_eq!(cap.role(MediaRole::Reference).support, Support::Yes);
        assert!(cap.unmapped_media_keys.is_empty());
    }

    #[test]
    fn two_roles_that_share_one_key_both_report_it_so_the_ui_can_dedupe() {
        // fal has a single `video_url`. `media::bind` writes roles into a map,
        // so rendering two slots against one key means the second attachment
        // overwrites the first without saying so.
        let s = schema(&["prompt", "video_url"], &[], &[]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "fal-ai/wan/v2.2-a14b", &s);
        let video = cap.role(MediaRole::Video);
        let reference = cap.role(MediaRole::VideoReference);
        assert_eq!(video.support, Support::Yes);
        assert_eq!(reference.support, Support::Yes);
        assert_eq!(video.key, reference.key);
    }

    #[test]
    fn an_unnamed_media_key_cannot_rule_any_role_out() {
        // `input_url` says nothing about its medium, so every role it might be
        // stays Unknown. Guessing would be the same silent-drop failure.
        let s = schema(&["prompt", "input_url"], &[], &[]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s);
        for role in ROLES {
            assert_eq!(cap.role(role).support, Support::Unknown, "{role:?}");
        }
    }

    // ── Fields we have no axis for ─────────────────────────────────────────

    #[test]
    fn an_enum_with_no_axis_is_carried_rather_than_forced_into_one() {
        // An unfamiliar enum survives as itself. The two named axes report a
        // truthful `No` — this endpoint really has no `aspect_ratio` and no
        // `resolution` field — while the choice fal *does* offer stays visible
        // rather than being guessed into one of them.
        let s = schema(
            &["prompt", "image_size"],
            &[],
            &[("image_size", &["square_hd", "portrait_4_3"])],
        );
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "fal-ai/flux-2-pro", &s);
        assert_eq!(cap.aspect.support, Support::No);
        assert_eq!(cap.resolution.support, Support::No);
        assert_eq!(
            cap.unmapped_enums.get("image_size").map(Vec::as_slice),
            Some(["square_hd".to_string(), "portrait_4_3".to_string()].as_slice())
        );
    }

    #[test]
    fn a_mapped_axis_is_not_repeated_in_the_unmapped_pile() {
        let s = schema(&["duration"], &[], &[("duration", &["4s"])]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s);
        assert!(cap.unmapped_enums.is_empty());
    }

    // ── Route resolution and fallback ──────────────────────────────────────

    #[test]
    fn the_schema_is_looked_up_for_the_mode_resolved_endpoint() {
        // The family root 404s; the mode decides the suffix. Asking fal about
        // the root would describe nothing.
        let mut asked = None;
        let m = spec(vec![flag("prompt", ValueSpec::Text), media_flag("image")]);
        let cap = for_route(
            &m,
            ProviderId::Fal,
            "fal-ai/kling-video/v3/standard",
            InputMode::Image,
            |e| {
                asked = Some(e.to_string());
                Some(schema(&["prompt", "image_url"], &["image_url"], &[]))
            },
        );
        assert_eq!(
            asked.as_deref(),
            Some("fal-ai/kling-video/v3/standard/image-to-video")
        );
        assert_eq!(
            cap.endpoint.as_deref(),
            Some("fal-ai/kling-video/v3/standard/image-to-video")
        );
    }

    #[test]
    fn being_unable_to_reach_fal_downgrades_to_the_catalogue_rather_than_blanking() {
        // Offline must not mean "this model does nothing" — that would block
        // every generation, a worse failure than the one we are preventing.
        let m = gemini_omni_spec();
        let cap = for_route(
            &m,
            ProviderId::Fal,
            "google/gemini-omni-flash",
            InputMode::Text,
            |_| None,
        );
        assert_eq!(cap.source, Source::Catalog);
        assert!(!cap.source.is_authoritative());
        assert_eq!(cap.role(MediaRole::Video).support, Support::Yes);
        assert_eq!(cap.endpoint, None);
    }

    #[test]
    fn a_mode_the_family_does_not_serve_is_not_asked_about() {
        // Seedance serves text and image only. Resolving a video request would
        // produce `/video-to-video`, which is a 404 and describes nothing.
        let m = spec(vec![flag("prompt", ValueSpec::Text)]);
        assert_eq!(
            fal_endpoint(
                &m,
                ProviderId::Fal,
                "bytedance/seedance-2.0",
                InputMode::Video
            ),
            None
        );
        assert_eq!(
            fal_endpoint(
                &m,
                ProviderId::Fal,
                "bytedance/seedance-2.0",
                InputMode::Image
            )
            .as_deref(),
            Some("bytedance/seedance-2.0/image-to-video")
        );
    }

    #[test]
    fn a_route_fal_does_not_serve_at_all_is_not_asked_about() {
        let m = spec(vec![flag("prompt", ValueSpec::Text)]);
        assert!(media::route_is_missing("fal-ai/wan/v2.6"));
        assert_eq!(
            fal_endpoint(&m, ProviderId::Fal, "fal-ai/wan/v2.6", InputMode::Text),
            None
        );
    }

    #[test]
    fn a_non_fal_route_is_answered_by_the_catalogue_and_says_so() {
        let m = gemini_omni_spec();
        let called = std::cell::Cell::new(false);
        let cap = for_route(
            &m,
            ProviderId::Higgsfield,
            "higgsfield-ai/dop/standard",
            InputMode::Text,
            |_| {
                called.set(true);
                None
            },
        );
        // fal's schema describes fal's endpoint; it says nothing about
        // Higgsfield's, so it must not even be fetched.
        assert!(!called.get());
        assert_eq!(cap.source, Source::Catalog);
    }

    // ── Numbers the estimator depends on ───────────────────────────────────

    #[test]
    fn durations_convert_to_seconds_across_both_spellings() {
        let s = schema(&["duration"], &[], &[("duration", &["4s", "6s", "8s"])]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s);
        assert_eq!(cap.duration_seconds(), Some(vec![4.0, 6.0, 8.0]));

        let cat = ModelCapability::from_catalog(&gemini_omni_spec());
        assert_eq!(cat.duration.values, ["4", "6", "8", "10"]);
        assert_eq!(cat.duration_seconds(), Some(vec![4.0, 6.0, 8.0, 10.0]));
        assert_eq!(cat.default_duration_seconds(), Some(8.0));
    }

    #[test]
    fn a_duration_option_that_is_not_a_number_refuses_the_whole_list() {
        // A filtered list would let the UI offer 8s while quietly hiding the
        // option next to it, and the Generate button prices whatever is picked.
        let s = schema(&["duration"], &[], &[("duration", &["4s", "auto"])]);
        let cap = ModelCapability::from_fal(&gemini_omni_spec(), "x/y", &s);
        assert_eq!(cap.duration_seconds(), None);
        // The raw values are still there, so the UI can render them as labels.
        assert_eq!(cap.duration.values, ["4s", "auto"]);
    }

    #[test]
    fn seconds_round_trip_without_a_trailing_zero() {
        assert_eq!(seconds_label(5.0), "5");
        assert_eq!(seconds_label(5.5), "5.5");
        assert_eq!(parse_seconds("5"), Some(5.0));
        assert_eq!(parse_seconds("5s"), Some(5.0));
        assert_eq!(parse_seconds("auto"), None);
    }

    // ── The real roster ────────────────────────────────────────────────────

    #[test]
    fn every_catalogued_model_derives_without_inventing_anything() {
        for m in catalog::catalogue().values() {
            let cap = ModelCapability::from_catalog(m);
            assert_eq!(cap.model_id, m.id);
            for (name, axis) in [
                ("duration", &cap.duration),
                ("resolution", &cap.resolution),
                ("aspect", &cap.aspect),
            ] {
                if !axis.support.is_yes() {
                    assert!(axis.values.is_empty(), "{} {name} kept values", m.id);
                    assert!(axis.default.is_none(), "{} {name} kept a default", m.id);
                }
                if let (Some(d), false) = (&axis.default, axis.values.is_empty()) {
                    assert!(
                        axis.values.contains(d),
                        "{} {name}: default {d} is not in {:?}",
                        m.id,
                        axis.values
                    );
                }
                // A catalogue-derived axis never claims fal's authority.
                assert!(!axis.source.is_authoritative(), "{} {name}", m.id);
            }
            assert_eq!(cap.roles.len(), ROLES.len(), "{}", m.id);
        }
    }

    #[test]
    fn a_still_image_model_reports_no_duration_control() {
        let c = catalog::catalogue();
        let cap = ModelCapability::from_catalog(c.get("nano_banana_2").unwrap());
        assert_eq!(cap.duration.support, Support::No);
        assert!(!cap.duration.is_free_form());
        assert_eq!(cap.resolution.values, ["1k", "2k", "4k"]);
        assert_eq!(cap.resolution.default.as_deref(), Some("2k"));
        assert_eq!(cap.resolution.source, Source::Catalog);
    }

    #[test]
    fn constraint_prose_survives_onto_a_fal_derived_description() {
        // fal's input schema publishes no prose, and these encode cross-field
        // rules that are the difference between a preventable 422 and a
        // confusing one — so the catalogue keeps supplying them.
        let c = catalog::catalogue();
        let m = c.get("cinematic_studio_video").unwrap();
        let s = schema(&["prompt", "image_url"], &["prompt"], &[]);
        let cap = ModelCapability::from_fal(m, "x/y", &s);
        assert!(
            cap.constraints.iter().any(|x| x.contains("start_image")),
            "got {:?}",
            cap.constraints
        );
    }
}
