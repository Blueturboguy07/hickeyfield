//! Presets as data: a family the UI shows, and per-model variants the resolver
//! picks between.
//!
//! ```text
//! preset_family  (what the user sees: name, category, thumbnail, description)
//!   └── preset_variant  (what the job gets: target models, baked params, text)
//! ```
//!
//! Higgsfield resolves this server-side and ships the client nothing but a
//! UUID — of their 419 public motion rows, the 229 in-house ones carry sampler
//! settings and the other 190 carry an empty `params` object, because the
//! prompt expansion happens on their backend. A BYO-key client cannot outsource
//! that, so the expansion lives here, in the open, as text we authored.
//!
//! One consequence worth stating plainly: a preset is not one prompt. The
//! phrasing that steers Veo is not the phrasing that steers Kling, so a family
//! holds several variants and the resolver picks by model id. That is why
//! [`PresetVariant::match_models`] holds glob patterns rather than exact ids —
//! `veo-3.1*` should keep working when a `veo-3.1-fast` shows up.

use crate::camera::{self, CameraTemplate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// The id of the neutral preset. Higgsfield ships one "General" row per model
/// family, all meaning "no preset, just do what I said" — and the enhance rules
/// in [`crate::enhance`] hinge on telling it apart from a real preset.
pub const GENERAL_ID: &str = "general";

// The chain-preset validator strings, verbatim. These are short functional
// error messages, reproduced exactly so that anyone who knows the original
// product recognises the same failure — and so a recipe exported from either
// side reads the same. Everything longer than a message like this is ours.
/// Emitted when a prompt is supplied to a preset that forbids one.
pub const PROMPT_MUST_BE_EMPTY: &str = "Prompt must be empty when preset is selected";
/// Emitted when a single-image chain preset gets zero images, or more than one.
pub const EXACTLY_ONE_IMAGE_REQUIRED: &str = "Exactly 1 image is required when preset is selected";
/// Emitted when video or audio is attached to an image-only chain preset.
pub const ONLY_IMAGE_MEDIA_ALLOWED: &str = "Only image media is allowed when preset is selected";

/// The seven catalogue categories, in the order the rail shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    New,
    Trending,
    /// Labelled "Effects" but slugged `vfx` — the mismatch is theirs and we
    /// keep it, because the slug is what the catalogue rows are keyed by.
    #[serde(rename = "vfx")]
    Effects,
    BasicCameraControl,
    EpicCameraControl,
    CatchThePulse,
    Mix,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::New,
        Category::Trending,
        Category::Effects,
        Category::BasicCameraControl,
        Category::EpicCameraControl,
        Category::CatchThePulse,
        Category::Mix,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Category::New => "new",
            Category::Trending => "trending",
            Category::Effects => "vfx",
            Category::BasicCameraControl => "basic_camera_control",
            Category::EpicCameraControl => "epic_camera_control",
            Category::CatchThePulse => "catch_the_pulse",
            Category::Mix => "mix",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Category::New => "New",
            Category::Trending => "Trending",
            Category::Effects => "Effects",
            Category::BasicCameraControl => "Basic Camera Control",
            Category::EpicCameraControl => "Epic Camera Control",
            Category::CatchThePulse => "Catch the Pulse",
            Category::Mix => "Mix",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.slug() == slug)
    }
}

/// Whether a field may, must, or must not be supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Requirement {
    /// Illegal. For the prompt this is what makes a chain preset a chain
    /// preset: the preset *is* the prompt, so the box gets disabled rather
    /// than merely ignored — a disabled box tells the truth, an ignored one
    /// silently discards what the user typed.
    Forbidden,
    #[default]
    Optional,
    Required,
}

/// A media kind, as counted by the validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    /// A saved element or character reference.
    Element,
}

/// Inclusive bounds on a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountRange {
    pub min: u32,
    pub max: u32,
}

impl CountRange {
    pub const fn new(min: u32, max: u32) -> Self {
        CountRange { min, max }
    }

    pub const fn exactly(n: u32) -> Self {
        CountRange { min: n, max: n }
    }

    pub const fn any() -> Self {
        CountRange {
            min: 0,
            max: u32::MAX,
        }
    }

    pub fn contains(&self, n: u32) -> bool {
        n >= self.min && n <= self.max
    }

    fn is_exactly_one(&self) -> bool {
        self.min == 1 && self.max == 1
    }
}

/// Optional capabilities a family offers. Drives which controls stay live.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Supports {
    /// Media kinds the family can be handed at all.
    pub media: Vec<MediaKind>,
    /// Whether an end frame may be attached. Note that attaching one flips
    /// enhancement off unconditionally — see [`crate::enhance`].
    pub end_frame: bool,
    pub negative_prompt: bool,
    pub audio: bool,
}

impl Supports {
    /// The common case: still images in, no interpolation, no audio.
    pub fn image_input() -> Self {
        Supports {
            media: vec![MediaKind::Image],
            ..Default::default()
        }
    }

    pub fn accepts(&self, kind: MediaKind) -> bool {
        self.media.contains(&kind)
    }
}

/// Hard input requirements. Violating any of these is a validation error, not
/// a warning — the job would be rejected downstream anyway, and finding out
/// locally is free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requires {
    pub prompt: Requirement,
    pub images: CountRange,
    /// Reject any non-image media outright.
    pub images_only: bool,
}

impl Default for Requires {
    fn default() -> Self {
        Requires {
            prompt: Requirement::Optional,
            images: CountRange::any(),
            images_only: false,
        }
    }
}

impl Requires {
    /// A chain preset: exactly one image in, no prompt, nothing else attached.
    /// The whole generation is baked into the preset, which is precisely why
    /// there is nothing left for the user to say.
    pub fn chain_preset() -> Self {
        Requires {
            prompt: Requirement::Forbidden,
            images: CountRange::exactly(1),
            images_only: true,
        }
    }
}

/// What the user actually attached, as counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaCounts {
    /// Every image role summed — plain images, start frames and end frames all
    /// count, because the constraint is on how many pictures the model sees.
    pub images: u32,
    pub videos: u32,
    pub audios: u32,
    pub elements: u32,
}

impl MediaCounts {
    pub fn images(n: u32) -> Self {
        MediaCounts {
            images: n,
            ..Default::default()
        }
    }

    fn non_image(&self) -> u32 {
        self.videos + self.audios
    }
}

/// One validation failure, shaped like the adapter contract: where it happened
/// and what to say. `loc` lets the UI put the message on the offending control
/// rather than in a general error banner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    pub loc: Vec<String>,
    pub msg: String,
}

impl ValidationError {
    fn new(loc: &[&str], msg: impl Into<String>) -> Self {
        ValidationError {
            loc: loc.iter().map(|s| s.to_string()).collect(),
            msg: msg.into(),
        }
    }
}

/// One rendering of a family for a set of models.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PresetVariant {
    /// Glob patterns matched against the model id, e.g. `veo-3.1*`. An empty
    /// list matches nothing — use `["*"]` for a deliberate catch-all, so that
    /// "forgot to fill this in" and "meant to catch everything" stay
    /// distinguishable.
    pub match_models: Vec<String>,
    /// Provider parameters this variant forces. A `BTreeMap` rather than a
    /// `HashMap` because these get serialised into exported recipes and hashed
    /// for cache keys, and both need a stable order.
    pub baked: BTreeMap<String, Value>,
    /// The prompt fragment this preset contributes, fed to the compiler in
    /// [`crate::enhance`].
    pub prompt_template: String,
    pub negative_prompt: Option<String>,
}

impl PresetVariant {
    pub fn new(match_models: &[&str], prompt_template: &str) -> Self {
        PresetVariant {
            match_models: match_models.iter().map(|s| s.to_string()).collect(),
            baked: BTreeMap::new(),
            prompt_template: prompt_template.to_string(),
            negative_prompt: None,
        }
    }

    pub fn with_baked(mut self, key: &str, value: Value) -> Self {
        self.baked.insert(key.to_string(), value);
        self
    }

    pub fn with_negative_prompt(mut self, negative: &str) -> Self {
        self.negative_prompt = Some(negative.to_string());
        self
    }

    /// Whether any of this variant's patterns claims `model_id`.
    pub fn matches(&self, model_id: &str) -> bool {
        self.match_models.iter().any(|p| glob_match(p, model_id))
    }
}

/// A preset as the catalogue rail shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetFamily {
    pub id: String,
    pub display_name: String,
    pub category: Category,
    pub tags: Vec<String>,
    pub description: String,
    pub supports: Supports,
    pub requires: Requires,
    pub variants: Vec<PresetVariant>,
    /// Slug of a [`crate::camera`] template to prepend. Camera presets carry
    /// one; effect presets do not.
    pub camera_template: Option<String>,
}

impl PresetFamily {
    pub fn new(id: &str, display_name: &str, category: Category) -> Self {
        PresetFamily {
            id: id.to_string(),
            display_name: display_name.to_string(),
            category,
            tags: Vec::new(),
            description: String::new(),
            supports: Supports::default(),
            requires: Requires::default(),
            variants: Vec::new(),
            camera_template: None,
        }
    }

    pub fn with_variant(mut self, v: PresetVariant) -> Self {
        self.variants.push(v);
        self
    }

    pub fn with_camera(mut self, slug: &str) -> Self {
        self.camera_template = Some(slug.to_string());
        self
    }

    pub fn with_requires(mut self, requires: Requires) -> Self {
        self.requires = requires;
        self
    }

    /// Pick the variant for a model: the first whose glob matches, otherwise
    /// the first variant in the list.
    ///
    /// Falling back rather than failing is deliberate. A preset with imperfect
    /// wording for an unlisted model still produces something recognisable,
    /// whereas refusing would make every new model launch look like the
    /// catalogue is broken. Authors put their best-general variant first.
    pub fn resolve_variant(&self, model_id: &str) -> Option<&PresetVariant> {
        self.variants
            .iter()
            .find(|v| v.matches(model_id))
            .or_else(|| self.variants.first())
    }

    /// The camera move this preset carries, if any and if the slug is known.
    pub fn camera(&self) -> Option<&'static CameraTemplate> {
        self.camera_template.as_deref().and_then(camera::get)
    }

    /// The neutral preset. Kept as a predicate rather than an enum variant
    /// because it arrives as an id from persisted state and from the UI.
    pub fn is_general(&self) -> bool {
        is_general_id(&self.id)
    }

    /// Whether the prompt box must be disabled while this preset is selected.
    pub fn disables_prompt(&self) -> bool {
        self.requires.prompt == Requirement::Forbidden
    }

    /// Check an intended submission against this preset's requirements.
    /// Returns every problem at once, not just the first — a form that reveals
    /// its errors one at a time is a form people give up on.
    pub fn validate(&self, prompt: &str, media: &MediaCounts) -> Vec<ValidationError> {
        let mut out = Vec::new();

        match self.requires.prompt {
            Requirement::Forbidden if !prompt.trim().is_empty() => {
                out.push(ValidationError::new(&["prompt"], PROMPT_MUST_BE_EMPTY));
            }
            Requirement::Required if prompt.trim().is_empty() => {
                out.push(ValidationError::new(&["prompt"], "A prompt is required"));
            }
            _ => {}
        }

        if !self.requires.images.contains(media.images) {
            let r = self.requires.images;
            let msg = if r.is_exactly_one() {
                EXACTLY_ONE_IMAGE_REQUIRED.to_string()
            } else if r.max == u32::MAX {
                format!("At least {} images are needed for this preset", r.min)
            } else {
                format!("This preset takes {} to {} images", r.min, r.max)
            };
            out.push(ValidationError::new(&["media", "image"], msg));
        }

        if self.requires.images_only && media.non_image() > 0 {
            out.push(ValidationError::new(
                &["media", "video"],
                ONLY_IMAGE_MEDIA_ALLOWED,
            ));
        }

        out
    }
}

/// Whether a preset id denotes the neutral "General" preset.
///
/// Accepts a bare `general` plus any `{family}-general` sentinel, because the
/// catalogue carries one neutral row per model family and they must all behave
/// identically under the enhance rules. Getting this wrong is not cosmetic: a
/// General preset misread as a real one silently takes the enhance toggle away
/// from the user.
pub fn is_general_id(id: &str) -> bool {
    id == GENERAL_ID || id.ends_with("-general") || id.ends_with("_general")
}

// ---- The catalogue ---------------------------------------------------------

/// Every preset family the app offers, in catalogue order.
///
/// Composed once and handed out as `&'static`, so two callers can never
/// disagree about what presets exist — the picker, a persisted job and a
/// recipe import all resolve the same id to the same family.
///
/// Today the list is the twenty-five camera moves in [`crate::camera`], which
/// are the reproducible half of the catalogue: their five-slot schema is
/// public, so we can render real prompt text rather than shipping a name with
/// nothing behind it. The remaining families have to be authored — see
/// `docs/PARITY.md` §1.
pub fn catalog() -> &'static [PresetFamily] {
    static CATALOG: OnceLock<Vec<PresetFamily>> = OnceLock::new();
    CATALOG.get_or_init(build_catalog).as_slice()
}

/// Look one family up by id. `None` for an unknown id.
///
/// Exact match only, and deliberately no nearest-neighbour fallback, for the
/// same reason as [`camera::get`]: an id reaches us verbatim from persisted
/// state, from a recipe or from the UI, and quietly resolving `push-inn` to
/// `push-in` would run a generation the user never asked for and charge them
/// for it. A miss is a bug the caller can see; a substitution is not.
pub fn get(id: &str) -> Option<&'static PresetFamily> {
    catalog().iter().find(|f| f.id == id)
}

/// Compose the catalogue.
///
/// This is the extension point for preset packs: a later JSON loader appends
/// its parsed families to this vector and every caller of [`catalog`] and
/// [`get`] picks them up with no signature change. Built-ins are pushed first
/// on purpose — see [`dedupe_by_id`].
fn build_catalog() -> Vec<PresetFamily> {
    dedupe_by_id(camera_families().collect())
}

/// Keep the first family claiming each id and drop any later duplicate.
///
/// The composition order is the policy: built-ins first, so a third-party pack
/// can never take over a shipped preset id. Without this, [`get`] would return
/// whichever copy it reached first while the picker rendered both tiles, and
/// the family a saved recipe resolves to would depend on pack load order.
fn dedupe_by_id(families: Vec<PresetFamily>) -> Vec<PresetFamily> {
    let mut seen = BTreeSet::new();
    families
        .into_iter()
        .filter(|f| seen.insert(f.id.clone()))
        .collect()
}

/// One family per camera move, in [`camera::TEMPLATES`] order.
fn camera_families() -> impl Iterator<Item = PresetFamily> {
    camera::TEMPLATES.iter().map(camera_family)
}

/// Wrap a camera move as a catalogue family.
///
/// The family id *is* the template slug. That is load-bearing twice over:
/// [`PresetFamily::camera`] resolves the move, and the UI's CSS move preview
/// is keyed by the preset id it is handed.
fn camera_family(t: &'static CameraTemplate) -> PresetFamily {
    let rendered = t.render();
    let mut f = PresetFamily::new(t.slug, t.display_name, camera_category(t.slug))
        .with_camera(t.slug)
        // One catch-all variant: the five slots never name what is being
        // filmed, so the same sentence chain steers every model. A model that
        // needs different wording gets a variant inserted *ahead* of this one,
        // because `resolve_variant` takes the first glob that matches.
        .with_variant(PresetVariant::new(&["*"], &rendered));
    // A camera move is the canonical image-to-video preset. No end frame: the
    // fifth slot already asserts what the shot resolves to, and attaching one
    // switches enhancement off outright (see [`Supports::end_frame`]).
    f.supports = Supports::image_input();
    f.tags = vec!["camera".to_string()];
    // The same description `list_presets` ships today, so moving that command
    // onto the catalogue leaves the picker's search behaviour unchanged — the
    // UI matches queries against the description.
    f.description = rendered;
    f
}

/// The moves that belong on the Epic Camera Control rail. Everything else
/// defaults to Basic, which is where their catalogue puts the plain
/// single-axis rig moves.
///
/// Sixteen of our twenty-five slugs correspond to a row in their live motion
/// catalogue (re-derived from the 419-row `motions-live-2026-08-02.json` in
/// `~/higgsfield-research`) and take that row's category as-is; two of those
/// sixteen are named differently there — `dolly-zoom` is their Dolly Zoom Out
/// and `static-shot` their Static, and the dolly-zoom pair is Epic in both
/// directions, so our naming convention for it cannot change the answer.
///
/// The other nine are ours, decided by the rule their split visibly follows: a
/// compound or subjective move is Epic, a plain single-axis one is Basic.
/// Their FPV Drone, Object POV and Focus Change rows are Epic, which puts
/// `drone-orbit`, `pov-walk` and `rack-focus` there; their Car Chasing and
/// Dolly In rows are Basic, which is why `tracking-shot`, the two pans and the
/// two pushes fall through.
const EPIC_CAMERA_SLUGS: &[&str] = &[
    // Same move, same category, in their live catalogue.
    "360-orbit",
    "arc-left",
    "arc-right",
    "bullet-time",
    "dolly-zoom",
    "dolly-zoom-in",
    // Ours, by analogy with the rows named above.
    "aerial-pullback",
    "drone-orbit",
    "pov-walk",
    "rack-focus",
];

/// Which of the two camera rails a move belongs on.
fn camera_category(slug: &str) -> Category {
    if EPIC_CAMERA_SLUGS.contains(&slug) {
        Category::EpicCameraControl
    } else {
        Category::BasicCameraControl
    }
}

/// Shell-style glob matching with `*` (any run, including empty) and `?` (one
/// byte). ASCII case-insensitive.
///
/// Hand-rolled rather than pulled in as a dependency: this is twenty lines, it
/// runs on every preset for every model-picker render, and the crate is kept
/// deliberately dependency-light. Patterns are expected to be ASCII — model ids
/// are — and `?` matches a byte, so it would split a multi-byte character.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0usize, 0usize);
    // Where the most recent `*` sits, and how much of `text` it has eaten.
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if let Some(s) = star {
            // Backtrack: let the last `*` swallow one more byte.
            pi = s + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn family() -> PresetFamily {
        PresetFamily::new(
            "earth-zoom-out",
            "Earth Zoom Out",
            Category::EpicCameraControl,
        )
        .with_camera("aerial-pullback")
        .with_variant(
            PresetVariant::new(
                &["veo-3.1*", "veo-3*"],
                "the ground falls away as altitude climbs",
            )
            .with_baked("duration", json!(8)),
        )
        .with_variant(
            PresetVariant::new(&["kling-*"], "altitude builds and the view keeps widening")
                .with_negative_prompt("warping, jitter"),
        )
        .with_variant(PresetVariant::new(
            &["seedance*"],
            "climb until the whole view fits",
        ))
    }

    #[test]
    fn the_seven_categories_carry_their_slugs_and_labels() {
        assert_eq!(Category::ALL.len(), 7);
        let slugs: Vec<_> = Category::ALL.iter().map(|c| c.slug()).collect();
        assert_eq!(
            slugs,
            vec![
                "new",
                "trending",
                "vfx",
                "basic_camera_control",
                "epic_camera_control",
                "catch_the_pulse",
                "mix",
            ]
        );
        assert_eq!(Category::CatchThePulse.display_name(), "Catch the Pulse");
        // Effects is the one whose slug is not its label; round-trip it.
        assert_eq!(Category::from_slug("vfx"), Some(Category::Effects));
        assert_eq!(Category::from_slug("effects"), None);
        for c in Category::ALL {
            assert_eq!(Category::from_slug(c.slug()), Some(c));
        }
    }

    #[test]
    fn categories_serialise_with_the_catalogue_slug() {
        assert_eq!(
            serde_json::to_string(&Category::Effects).unwrap(),
            "\"vfx\""
        );
        assert_eq!(
            serde_json::to_string(&Category::BasicCameraControl).unwrap(),
            "\"basic_camera_control\""
        );
        let back: Category = serde_json::from_str("\"catch_the_pulse\"").unwrap();
        assert_eq!(back, Category::CatchThePulse);
    }

    #[test]
    fn resolve_variant_picks_the_first_matching_glob() {
        let f = family();
        assert_eq!(
            f.resolve_variant("veo-3.1-fast").unwrap().prompt_template,
            "the ground falls away as altitude climbs"
        );
        assert_eq!(
            f.resolve_variant("kling-3.0")
                .unwrap()
                .negative_prompt
                .as_deref(),
            Some("warping, jitter")
        );
        assert_eq!(
            f.resolve_variant("seedance-2.0-mini")
                .unwrap()
                .prompt_template,
            "climb until the whole view fits"
        );
    }

    #[test]
    fn resolve_variant_falls_back_to_the_first_variant() {
        let f = family();
        // Nothing claims wan; the authored best-general variant answers.
        let v = f.resolve_variant("wan-2.7").unwrap();
        assert_eq!(
            v.prompt_template,
            "the ground falls away as altitude climbs"
        );
        assert_eq!(v.baked.get("duration"), Some(&json!(8)));
        // The fallback is the first variant even for an empty model id.
        assert_eq!(
            f.resolve_variant("").unwrap().prompt_template,
            "the ground falls away as altitude climbs"
        );
    }

    #[test]
    fn a_family_with_no_variants_resolves_to_nothing() {
        let f = PresetFamily::new("empty", "Empty", Category::New);
        assert!(f.resolve_variant("veo-3.1").is_none());
    }

    #[test]
    fn an_empty_pattern_list_matches_nothing_but_a_star_matches_all() {
        let silent = PresetVariant::new(&[], "unreachable by pattern");
        assert!(!silent.matches("veo-3.1"));
        assert!(!silent.matches(""));

        let catch_all = PresetVariant::new(&["*"], "anything");
        assert!(catch_all.matches("veo-3.1"));
        assert!(catch_all.matches(""));

        // An empty list still works as the index-0 fallback, so a preset
        // authored that way is degraded rather than dead.
        let f = PresetFamily::new("x", "X", Category::New)
            .with_variant(silent)
            .with_variant(PresetVariant::new(&["kling-*"], "kling"));
        assert_eq!(
            f.resolve_variant("veo-3.1").unwrap().prompt_template,
            "unreachable by pattern"
        );
        assert_eq!(
            f.resolve_variant("kling-3.0").unwrap().prompt_template,
            "kling"
        );
    }

    #[test]
    fn glob_handles_prefix_suffix_infix_and_multiple_stars() {
        assert!(glob_match("veo-3.1*", "veo-3.1"));
        assert!(glob_match("veo-3.1*", "veo-3.1-fast"));
        assert!(!glob_match("veo-3.1*", "veo-3.0"));
        assert!(!glob_match("veo-3.1*", "xveo-3.1"));

        assert!(glob_match("*-fast", "seedance-2.0-fast"));
        assert!(glob_match("*kling*", "fal-ai/kling-video"));
        assert!(glob_match("a*b*c", "azzbzzc"));
        assert!(!glob_match("a*b*c", "azzbzz"));
        assert!(glob_match("**", "anything at all"));
        assert!(glob_match("*", ""));

        // A literal pattern is an exact match, and `.` is not a wildcard.
        assert!(glob_match("wan-2.7", "wan-2.7"));
        assert!(!glob_match("wan-2.7", "wan-2x7"));

        // `?` is exactly one character.
        assert!(glob_match("veo-3.?", "veo-3.1"));
        assert!(!glob_match("veo-3.?", "veo-3.10"));

        // Case-insensitive, because model ids reach us from several sources.
        assert!(glob_match("VEO-3.1*", "veo-3.1-fast"));
        assert!(glob_match("veo-3.1*", "VEO-3.1-FAST"));

        // The classic backtracking trap: a greedy `*` must give bytes back.
        assert!(glob_match("*ab", "aaab"));
        assert!(!glob_match("*ab", "aaba"));
    }

    // ---- Chain presets: the prompt is illegal ------------------------------

    fn chain() -> PresetFamily {
        let mut f = PresetFamily::new("noir-portrait", "Noir Portrait", Category::Trending)
            .with_requires(Requires::chain_preset())
            .with_variant(PresetVariant::new(
                &["seedance-2.0*"],
                "hard key, deep shadow",
            ));
        f.supports = Supports::image_input();
        f
    }

    #[test]
    fn a_chain_preset_disables_the_prompt_box() {
        assert!(chain().disables_prompt());
        assert!(!family().disables_prompt());
    }

    #[test]
    fn a_chain_preset_emits_the_exact_validator_strings() {
        let f = chain();

        // A prompt where none is allowed.
        let errs = f.validate("a wolf howling", &MediaCounts::images(1));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].msg, "Prompt must be empty when preset is selected");
        assert_eq!(errs[0].loc, vec!["prompt"]);

        // Zero images, and two images, both fail the same way.
        for n in [0, 2] {
            let errs = f.validate("", &MediaCounts::images(n));
            assert_eq!(errs.len(), 1, "{n} images");
            assert_eq!(
                errs[0].msg,
                "Exactly 1 image is required when preset is selected"
            );
            assert_eq!(errs[0].loc, vec!["media", "image"]);
        }

        // Video or audio alongside the image.
        let media = MediaCounts {
            images: 1,
            videos: 1,
            ..Default::default()
        };
        let errs = f.validate("", &media);
        assert_eq!(errs.len(), 1);
        assert_eq!(
            errs[0].msg,
            "Only image media is allowed when preset is selected"
        );
        assert_eq!(errs[0].loc, vec!["media", "video"]);

        let media = MediaCounts {
            images: 1,
            audios: 2,
            ..Default::default()
        };
        assert_eq!(
            f.validate("", &media)[0].msg,
            "Only image media is allowed when preset is selected"
        );
    }

    #[test]
    fn all_three_problems_are_reported_together() {
        let f = chain();
        let media = MediaCounts {
            images: 0,
            videos: 1,
            audios: 1,
            elements: 0,
        };
        let msgs: Vec<_> = f
            .validate("something", &media)
            .into_iter()
            .map(|e| e.msg)
            .collect();
        assert_eq!(
            msgs,
            vec![
                PROMPT_MUST_BE_EMPTY.to_string(),
                EXACTLY_ONE_IMAGE_REQUIRED.to_string(),
                ONLY_IMAGE_MEDIA_ALLOWED.to_string(),
            ]
        );
    }

    #[test]
    fn a_correct_chain_submission_validates_clean() {
        assert!(chain().validate("", &MediaCounts::images(1)).is_empty());
        // Whitespace is not a prompt.
        assert!(chain()
            .validate("   \n ", &MediaCounts::images(1))
            .is_empty());
    }

    #[test]
    fn an_ordinary_preset_imposes_nothing() {
        let f = family();
        let media = MediaCounts {
            images: 3,
            videos: 1,
            audios: 1,
            elements: 2,
        };
        assert!(f.validate("anything at all", &media).is_empty());
        assert!(f.validate("", &MediaCounts::default()).is_empty());
    }

    #[test]
    fn other_count_bounds_get_their_own_wording() {
        // The exact validator string belongs to the exactly-one case only.
        let mut f = PresetFamily::new("collage", "Collage", Category::Mix);
        f.requires.images = CountRange::new(2, 4);
        let msg = f.validate("", &MediaCounts::images(1))[0].msg.clone();
        assert_eq!(msg, "This preset takes 2 to 4 images");

        f.requires.images = CountRange::new(2, u32::MAX);
        let msg = f.validate("", &MediaCounts::images(1))[0].msg.clone();
        assert_eq!(msg, "At least 2 images are needed for this preset");
    }

    #[test]
    fn a_required_prompt_is_enforced_too() {
        let mut f = PresetFamily::new("freeform", "Freeform", Category::New);
        f.requires.prompt = Requirement::Required;
        assert_eq!(
            f.validate("  ", &MediaCounts::default())[0].loc,
            vec!["prompt"]
        );
        assert!(f.validate("a scene", &MediaCounts::default()).is_empty());
    }

    // ---- General, camera wiring, serde -------------------------------------

    #[test]
    fn general_is_recognised_however_the_catalogue_spells_it() {
        assert!(is_general_id("general"));
        assert!(is_general_id("veo3-general"));
        assert!(is_general_id("seedance_general"));
        assert!(!is_general_id("general-purpose-glow"));
        assert!(!is_general_id("generally-cinematic"));
        assert!(!is_general_id(""));

        let g = PresetFamily::new(GENERAL_ID, "General", Category::New);
        assert!(g.is_general());
        assert!(!family().is_general());
    }

    #[test]
    fn a_camera_preset_resolves_its_template() {
        let f = family();
        let t = f.camera().expect("aerial-pullback exists");
        assert_eq!(t.slug, "aerial-pullback");
        assert!(t.render().starts_with("Camera: "));

        // An unknown slug resolves to nothing rather than to a guess.
        let bad = PresetFamily::new("x", "X", Category::New).with_camera("aerial-pullbaq");
        assert!(bad.camera().is_none());

        // Effect presets carry no camera move.
        assert!(chain().camera().is_none());
    }

    #[test]
    fn baked_params_keep_a_stable_order_for_recipe_export() {
        let v = PresetVariant::new(&["*"], "x")
            .with_baked("steps", json!(20))
            .with_baked("frames", json!(81))
            .with_baked("guide_scale", json!(6.0))
            .with_baked("strength", json!(1.0));
        let s = serde_json::to_string(&v.baked).unwrap();
        assert_eq!(
            s,
            r#"{"frames":81,"guide_scale":6.0,"steps":20,"strength":1.0}"#
        );
        // Serialising twice must be byte-identical: recipe hashes depend on it.
        assert_eq!(s, serde_json::to_string(&v.baked).unwrap());
    }

    #[test]
    fn a_family_round_trips_through_json() {
        let f = family();
        let s = serde_json::to_string(&f).unwrap();
        let back: PresetFamily = serde_json::from_str(&s).unwrap();
        assert_eq!(back, f);
        assert_eq!(
            back.resolve_variant("veo-3.1").unwrap(),
            f.resolve_variant("veo-3.1").unwrap()
        );
    }

    #[test]
    fn supports_reports_what_it_accepts() {
        let s = Supports::image_input();
        assert!(s.accepts(MediaKind::Image));
        assert!(!s.accepts(MediaKind::Video));
        assert!(!s.end_frame);
    }

    #[test]
    fn count_range_bounds_are_inclusive() {
        let r = CountRange::new(1, 3);
        assert!(!r.contains(0));
        assert!(r.contains(1));
        assert!(r.contains(3));
        assert!(!r.contains(4));
        assert!(CountRange::any().contains(u32::MAX));
        assert!(CountRange::exactly(1).is_exactly_one());
    }

    // ---- The catalogue -----------------------------------------------------

    #[test]
    fn the_catalogue_carries_every_camera_move_in_order() {
        let ids: Vec<&str> = catalog().iter().map(|f| f.id.as_str()).collect();
        let slugs: Vec<&str> = camera::slugs().collect();
        assert_eq!(ids, slugs);
        assert_eq!(catalog().len(), 25);
    }

    #[test]
    fn get_round_trips_every_id_the_catalogue_advertises() {
        // The bug this prevents: a picker built from `catalog()` sends an id
        // back on submit, `get` misses it, and the job silently loses its
        // preset while the UI still shows the tile as selected.
        for f in catalog() {
            let found = get(&f.id)
                .unwrap_or_else(|| panic!("catalog() lists {} but get() cannot find it", f.id));
            assert_eq!(found, f);
            assert!(
                std::ptr::eq(found, f),
                "{} resolved to a copy rather than the catalogue's own row",
                f.id
            );
        }
    }

    #[test]
    fn an_unknown_id_resolves_to_nothing_rather_than_a_neighbour() {
        assert!(get("push-inn").is_none());
        assert!(get("").is_none());
        assert!(get("camera/push-in").is_none());
        // Exact match, like `camera::get`: ids arrive verbatim from persisted
        // state, so a case variation is a miss. Resolving it to a different
        // preset would run — and bill — a generation nobody asked for.
        assert!(get("Push-In").is_none());
        assert!(get("push-in").is_some());
    }

    #[test]
    fn every_camera_family_carries_a_rendered_five_slot_prompt() {
        for f in catalog() {
            let t = f
                .camera()
                .unwrap_or_else(|| panic!("{} names a camera template that does not exist", f.id));
            // Nothing may fall through to "no variant": a family with an empty
            // prompt template ships its name with none of its effect, which is
            // exactly the failure `docs/BRIDGE.md` §4.8 describes.
            for model in ["veo-3.1-fast", "kling-3.0", "seedance-2.0", "wan-2.7", ""] {
                let v = f
                    .resolve_variant(model)
                    .unwrap_or_else(|| panic!("{} has nothing for {model:?}", f.id));
                assert_eq!(v.prompt_template, t.render());
                assert!(v.prompt_template.starts_with("Camera: "));
            }
            assert!(!f.description.is_empty(), "{} has no description", f.id);
            assert!(!f.display_name.is_empty(), "{} has no label", f.id);
        }
    }

    #[test]
    fn no_two_catalogue_families_share_an_id() {
        let mut seen = BTreeSet::new();
        for f in catalog() {
            assert!(seen.insert(&f.id), "duplicate catalogue id {}", f.id);
        }
    }

    #[test]
    fn a_later_pack_cannot_take_over_a_shipped_preset_id() {
        let builtin = PresetFamily::new("push-in", "Push in", Category::BasicCameraControl)
            .with_variant(PresetVariant::new(&["*"], "ours"));
        let shadow = PresetFamily::new("push-in", "Push in (pack)", Category::Trending)
            .with_variant(PresetVariant::new(&["*"], "theirs"));
        let fresh = PresetFamily::new("pack-only", "Pack only", Category::New);

        let out = dedupe_by_id(vec![builtin.clone(), shadow, fresh]);
        assert_eq!(out.len(), 2, "the shadowing family must be dropped");
        assert_eq!(out[0], builtin);
        assert_eq!(out[1].id, "pack-only", "a fresh id is still admitted");
    }

    #[test]
    fn the_epic_rail_lists_only_real_moves_and_the_rest_are_basic() {
        for slug in EPIC_CAMERA_SLUGS {
            assert!(
                camera::get(slug).is_some(),
                "{slug} is not a camera move — a typo here files the move under \
                 Basic instead, silently and with no other symptom"
            );
        }
        assert_eq!(
            get("360-orbit").unwrap().category,
            Category::EpicCameraControl
        );
        assert_eq!(
            get("zoom-in").unwrap().category,
            Category::BasicCameraControl
        );

        let epic = catalog()
            .iter()
            .filter(|f| f.category == Category::EpicCameraControl)
            .count();
        assert_eq!(epic, EPIC_CAMERA_SLUGS.len());
        assert!(catalog().iter().all(|f| matches!(
            f.category,
            Category::BasicCameraControl | Category::EpicCameraControl
        )));
    }

    #[test]
    fn no_catalogued_preset_is_mistaken_for_the_neutral_one() {
        // A real preset misread as General takes the enhance toggle away from
        // the user without saying so — see `is_general_id`.
        for f in catalog() {
            assert!(!f.is_general(), "{} reads as General", f.id);
            assert!(!is_general_id(&f.id));
        }
        // And the neutral row is not authored yet, so the picker's "General"
        // tile has no family behind it (`docs/BRIDGE.md` §4.12).
        assert!(get(GENERAL_ID).is_none());
    }

    #[test]
    fn the_catalogue_is_composed_once() {
        // `&'static` is a promise to the UI: an id it read on one call must
        // still resolve on the next, against the same rows.
        assert!(std::ptr::eq(catalog().as_ptr(), catalog().as_ptr()));
        assert_eq!(catalog().len(), catalog().len());
    }

    #[test]
    fn catalogue_families_survive_a_recipe_round_trip() {
        // Recipes persist a family by value; the camera slug has to come back
        // or the restored job loses its move.
        let f = get("dolly-zoom").expect("dolly-zoom is catalogued");
        let back: PresetFamily = serde_json::from_str(&serde_json::to_string(f).unwrap()).unwrap();
        assert_eq!(&back, f);
        assert_eq!(back.camera().map(|t| t.slug), Some("dolly-zoom"));
        assert_eq!(back.category, Category::EpicCameraControl);
    }
}
