//! The prompt compiler and the forced-enhance rules.
//!
//! Two things live here, and they are deliberately separate:
//!
//! 1. **The compiler** — structured settings in, one prompt string out. No
//!    network call, no model, no randomness. It costs nothing, it runs in
//!    microseconds, and the same inputs always produce the same bytes, which is
//!    what makes Rerun, recipe export and cache keys work at all.
//! 2. **The enhance decision** — whether that prompt should additionally be
//!    handed to an LLM rewriter on the user's own key before submission.
//!
//! The three rules in [`decide`] are the core of the harness. They look small.
//! They are the reason the original product feels good: a preset is a promise
//! about the *look* of the output, and honouring a toggle that would break that
//! promise is not respect for the user, it is a worse result. The rules are
//! reproduced here because they are correct, and each one is commented with the
//! reasoning rather than just the behaviour.

use crate::camera::{self, CameraTemplate};
use crate::preset::{self, PresetFamily};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Per-job-type enhancement defaults
// ---------------------------------------------------------------------------

/// The kinds of job that carry a prompt, one variant per distinct enhancement
/// default. This is not the model list — several models share a job type, and
/// the default is a property of the *dialect*, not the checkpoint.
///
/// Wire names are spelled out per variant rather than derived from the Rust
/// identifier, because they are persisted in job records and recipes: renaming
/// a variant must not silently invalidate everyone's saved history.
///
/// Nothing in this module can tell you which job type a *model* is. That join
/// lives in [`crate::registry::JOB_TYPES`] and is what makes everything below
/// reachable from a generation the user actually started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum JobType {
    // -- Defaults ON: models that reward verbose cinematic prose ------------
    /// Every text-to-video and image-to-video path.
    #[serde(rename = "video")]
    Video,
    /// Our Characters/Styles image path.
    #[serde(rename = "image-styled")]
    ImageStyled,
    #[serde(rename = "image-wan2.2")]
    ImageWan22,
    /// The legacy GPT image surface. See the note on [`JobType::ImageGptImage2`]
    /// — this one really is on, and the two are not the same surface.
    #[serde(rename = "image-gpt")]
    ImageGpt,
    /// FLUX.1 Kontext.
    #[serde(rename = "image-flux")]
    ImageFlux,
    #[serde(rename = "image-flux-2")]
    ImageFlux2,
    #[serde(rename = "image-flux-2-flex")]
    ImageFlux2Flex,
    #[serde(rename = "image-flux-2-max")]
    ImageFlux2Max,
    #[serde(rename = "image-kling-omni")]
    ImageKlingOmni,
    #[serde(rename = "z-image")]
    ZImage,
    #[serde(rename = "avatar")]
    Avatar,
    #[serde(rename = "complex-avatar")]
    ComplexAvatar,
    /// The multi-step studio builder.
    #[serde(rename = "builder")]
    Builder,

    // -- Defaults OFF: instruction-following and editing models -------------
    /// Image-to-animation. Off by default; the source image already carries
    /// the composition, so rewriting mostly invents contradictions.
    #[serde(rename = "animate")]
    Animate,
    #[serde(rename = "image-nano-banana")]
    ImageNanoBanana,
    #[serde(rename = "image-nano-banana-2")]
    ImageNanoBanana2,
    #[serde(rename = "image-seedream")]
    ImageSeedream,
    /// GPT Image 2.
    #[serde(rename = "image-gpt-image-2")]
    ImageGptImage2,
    #[serde(rename = "image-gpt-image-2-mini")]
    ImageGptImage2Mini,
    /// Reference-driven edits.
    #[serde(rename = "reference")]
    Reference,
    #[serde(rename = "scene")]
    Scene,
    #[serde(rename = "product")]
    Product,
    #[serde(rename = "speech")]
    Speech,
    #[serde(rename = "lipsync")]
    Lipsync,
    #[serde(rename = "photodump")]
    Photodump,
    #[serde(rename = "fashion-factory")]
    FashionFactory,
}

impl JobType {
    pub const ALL: [JobType; 26] = [
        JobType::Video,
        JobType::ImageStyled,
        JobType::ImageWan22,
        JobType::ImageGpt,
        JobType::ImageFlux,
        JobType::ImageFlux2,
        JobType::ImageFlux2Flex,
        JobType::ImageFlux2Max,
        JobType::ImageKlingOmni,
        JobType::ZImage,
        JobType::Avatar,
        JobType::ComplexAvatar,
        JobType::Builder,
        JobType::Animate,
        JobType::ImageNanoBanana,
        JobType::ImageNanoBanana2,
        JobType::ImageSeedream,
        JobType::ImageGptImage2,
        JobType::ImageGptImage2Mini,
        JobType::Reference,
        JobType::Scene,
        JobType::Product,
        JobType::Speech,
        JobType::Lipsync,
        JobType::Photodump,
        JobType::FashionFactory,
    ];

    /// The persisted wire name. Must agree with the serde rename above; a test
    /// holds the two together.
    pub fn slug(self) -> &'static str {
        use JobType::*;
        match self {
            Video => "video",
            ImageStyled => "image-styled",
            ImageWan22 => "image-wan2.2",
            ImageGpt => "image-gpt",
            ImageFlux => "image-flux",
            ImageFlux2 => "image-flux-2",
            ImageFlux2Flex => "image-flux-2-flex",
            ImageFlux2Max => "image-flux-2-max",
            ImageKlingOmni => "image-kling-omni",
            ZImage => "z-image",
            Avatar => "avatar",
            ComplexAvatar => "complex-avatar",
            Builder => "builder",
            Animate => "animate",
            ImageNanoBanana => "image-nano-banana",
            ImageNanoBanana2 => "image-nano-banana-2",
            ImageSeedream => "image-seedream",
            ImageGptImage2 => "image-gpt-image-2",
            ImageGptImage2Mini => "image-gpt-image-2-mini",
            Reference => "reference",
            Scene => "scene",
            Product => "product",
            Speech => "speech",
            Lipsync => "lipsync",
            Photodump => "photodump",
            FashionFactory => "fashion-factory",
        }
    }

    pub fn from_slug(slug: &str) -> Option<JobType> {
        JobType::ALL.into_iter().find(|j| j.slug() == slug)
    }

    /// Whether prompt enhancement starts on for this job type.
    ///
    /// The split is one editorial judgement applied consistently: **on** where
    /// the model responds to atmosphere, camera language and adjectives, and
    /// **off** where the prompt is an instruction to be obeyed literally.
    /// Rewriting "remove the background" into three sentences of cinematic
    /// prose does not produce a better edit, it produces a different picture.
    ///
    /// A wrinkle worth recording rather than smoothing over: the legacy
    /// [`JobType::ImageGpt`] surface defaults **on** while the newer GPT Image 2
    /// surfaces default **off**. Prose summaries of this table tend to collapse
    /// both into "GPT is off". They are different surfaces, and the newer one
    /// is the instruction-follower, so both values are right.
    pub fn default_enhance(self) -> bool {
        use JobType::*;
        match self {
            Video | ImageStyled | ImageWan22 | ImageGpt | ImageFlux | ImageFlux2
            | ImageFlux2Flex | ImageFlux2Max | ImageKlingOmni | ZImage | Avatar | ComplexAvatar
            | Builder => true,

            Animate | ImageNanoBanana | ImageNanoBanana2 | ImageSeedream | ImageGptImage2
            | ImageGptImage2Mini | Reference | Scene | Product | Speech | Lipsync | Photodump
            | FashionFactory => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The three forced-enhance rules
// ---------------------------------------------------------------------------

/// What the preset picker is showing, reduced to the only distinction the
/// enhance rules care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetSelection {
    /// No preset at all.
    #[default]
    None,
    /// The neutral "General" row — present in the picker, but it asks for
    /// nothing, so it must not take control away from the user.
    General,
    /// Any real preset.
    Real,
}

impl PresetSelection {
    /// Classify a preset id. `None` means nothing is selected.
    pub fn from_id(id: Option<&str>) -> Self {
        match id {
            None => PresetSelection::None,
            Some(id) if preset::is_general_id(id) => PresetSelection::General,
            Some(_) => PresetSelection::Real,
        }
    }

    pub fn from_family(family: Option<&PresetFamily>) -> Self {
        PresetSelection::from_id(family.map(|f| f.id.as_str()))
    }

    /// Whether this selection forces enhancement on.
    pub fn forces_enhance(self) -> bool {
        matches!(self, PresetSelection::Real)
    }
}

/// Everything the enhance decision depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhanceInputs {
    pub job: JobType,
    pub preset: PresetSelection,
    /// Whether a last frame has been attached, making this an interpolation.
    pub has_end_frame: bool,
    /// The Enhance toggle, if the user has moved it. `None` means untouched,
    /// in which case the per-job-type default stands. Distinguishing these is
    /// necessary: "off because they chose off" and "off because this job type
    /// starts off" must not become the same state, or switching model would
    /// silently discard a deliberate choice.
    pub user_toggle: Option<bool>,
}

impl EnhanceInputs {
    pub fn new(job: JobType) -> Self {
        EnhanceInputs {
            job,
            preset: PresetSelection::None,
            has_end_frame: false,
            user_toggle: None,
        }
    }

    pub fn with_preset(mut self, preset: PresetSelection) -> Self {
        self.preset = preset;
        self
    }

    pub fn with_end_frame(mut self, has_end_frame: bool) -> Self {
        self.has_end_frame = has_end_frame;
        self
    }

    pub fn with_toggle(mut self, on: bool) -> Self {
        self.user_toggle = Some(on);
        self
    }
}

/// Why enhancement ended up on or off. Surfaced in the UI so that a toggle
/// which visibly disagrees with the outcome can explain itself, instead of
/// looking broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnhanceReason {
    /// Rule 2. An end frame is attached.
    EndFrame,
    /// Rule 1. A real preset is selected.
    Preset,
    /// Rule 3, toggle moved.
    UserToggle,
    /// Rule 3, toggle untouched: the per-job-type default.
    JobDefault,
}

impl EnhanceReason {
    /// Whether this outcome overrode the user's toggle.
    pub fn is_forced(self) -> bool {
        matches!(self, EnhanceReason::EndFrame | EnhanceReason::Preset)
    }

    /// One line of UI copy explaining the outcome.
    pub fn explanation(self) -> &'static str {
        match self {
            EnhanceReason::EndFrame => "Turned off: an end frame is attached",
            EnhanceReason::Preset => "Turned on: a preset is selected",
            EnhanceReason::UserToggle => "Your setting",
            EnhanceReason::JobDefault => "Default for this model",
        }
    }
}

/// The outcome of the three rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnhanceDecision {
    pub enhance: bool,
    pub reason: EnhanceReason,
}

impl EnhanceDecision {
    /// Whether the toggle should render as locked.
    pub fn is_forced(&self) -> bool {
        self.reason.is_forced()
    }
}

/// Apply the three forced-enhance rules.
///
/// Precedence is **end frame, then preset, then the user** — and the order is
/// the whole rule, not an implementation detail. When a preset and an end frame
/// are both present, the end frame wins.
///
/// Why that way round: with a first and last frame supplied, the generation is
/// an interpolation between two fixed images. The prompt is a hint about the
/// path between them, and the endpoints are not negotiable. An LLM rewrite
/// reliably introduces detail that contradicts one of the two frames, and the
/// model then has to choose between the prompt and the pixels. Every outcome of
/// that choice is a worse video. A preset's promise about the look, by contrast,
/// is already largely carried by the baked parameters and the fixed frames — so
/// it is the cheaper promise to break.
pub fn decide(input: EnhanceInputs) -> EnhanceDecision {
    // Rule 2 first, because it is the only unconditional one.
    if input.has_end_frame {
        return EnhanceDecision {
            enhance: false,
            reason: EnhanceReason::EndFrame,
        };
    }

    // Rule 1. A preset is a commitment to an aesthetic that the rewriter is
    // what actually delivers — the preset text is steering *for* the rewrite.
    // Honouring an "off" toggle here would ship the preset's name and none of
    // its effect, which reads to the user as the preset being broken.
    if input.preset.forces_enhance() {
        return EnhanceDecision {
            enhance: true,
            reason: EnhanceReason::Preset,
        };
    }

    // Rule 3. Nothing is claiming the prompt, so it belongs to the user.
    match input.user_toggle {
        Some(on) => EnhanceDecision {
            enhance: on,
            reason: EnhanceReason::UserToggle,
        },
        None => EnhanceDecision {
            enhance: input.job.default_enhance(),
            reason: EnhanceReason::JobDefault,
        },
    }
}

// ---------------------------------------------------------------------------
// Media reference sentinels
// ---------------------------------------------------------------------------

/// The opening delimiter of a media reference sentinel.
const OPEN: &str = "<<<";
/// The closing delimiter.
const CLOSE: &str = ">>>";

/// What a sentinel points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SentinelKind {
    Image,
    Video,
    Element,
}

impl SentinelKind {
    pub fn tag(self) -> &'static str {
        match self {
            SentinelKind::Image => "image",
            SentinelKind::Video => "video",
            SentinelKind::Element => "element",
        }
    }

    fn parse(tag: &str) -> Option<SentinelKind> {
        match tag {
            "image" => Some(SentinelKind::Image),
            "video" => Some(SentinelKind::Video),
            "element" => Some(SentinelKind::Element),
            _ => None,
        }
    }
}

/// One sentinel found in a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sentinel {
    pub kind: SentinelKind,
    /// The 1-based index carried in the token.
    pub index: u32,
    /// Byte range of the whole token, delimiters included.
    pub start: usize,
    pub end: usize,
}

impl Sentinel {
    pub fn token(&self) -> String {
        sentinel(self.kind, self.index)
    }
}

/// Build a sentinel token: `<<<image_1>>>`.
pub fn sentinel(kind: SentinelKind, index: u32) -> String {
    format!("{OPEN}{}_{index}{CLOSE}", kind.tag())
}

/// Every sentinel in `text`, in order of appearance.
///
/// Hand-rolled rather than a regex crate: the grammar is three fixed tags and a
/// run of digits, and this keeps the dependency budget where it belongs.
pub fn sentinels(text: &str) -> Vec<Sentinel> {
    let mut out = Vec::new();
    let mut at = 0usize;

    while let Some(rel) = text[at..].find(OPEN) {
        let start = at + rel;
        let body_start = start + OPEN.len();
        let Some(rel_end) = text[body_start..].find(CLOSE) else {
            break;
        };
        let body_end = body_start + rel_end;
        let end = body_end + CLOSE.len();

        // Advance past the opener even on a miss, so `<<<<<<image_1>>>` and
        // other malformed input cannot spin.
        at = body_start;

        let body = &text[body_start..body_end];
        if let Some((tag, digits)) = body.rsplit_once('_') {
            let digits_ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
            if let (Some(kind), true) = (SentinelKind::parse(tag), digits_ok) {
                out.push(Sentinel {
                    kind,
                    // Saturating rather than skipping: an absurd index is still
                    // an unresolved reference, and reporting "no sentinels" for
                    // one would let it reach a provider verbatim.
                    index: digits.parse::<u32>().unwrap_or(u32::MAX),
                    start,
                    end,
                });
                at = end;
            }
        }
    }

    out
}

/// Whether `text` still carries a media reference that has not been bound.
///
/// "Reuse this prompt" must refuse while this is true. The sentinel is a
/// pointer into a *particular* generation's attachments; pasted into a fresh
/// composer with nothing attached, it either reaches the provider as literal
/// gibberish or silently binds to whatever happens to be in slot 1. Refusing is
/// the only honest option.
pub fn has_unresolved_sentinel(text: &str) -> bool {
    !sentinels(text).is_empty()
}

/// The inverse, phrased for the call site that needs it.
pub fn can_reuse_prompt(text: &str) -> bool {
    !has_unresolved_sentinel(text)
}

/// Replace sentinels with real text. Anything `resolve` declines stays exactly
/// as it was, so a partial resolution is still detectable by
/// [`has_unresolved_sentinel`] rather than quietly disappearing.
pub fn resolve_sentinels<F>(text: &str, resolve: F) -> String
where
    F: Fn(SentinelKind, u32) -> Option<String>,
{
    let found = sentinels(text);
    if found.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for s in found {
        out.push_str(&text[cursor..s.start]);
        match resolve(s.kind, s.index) {
            Some(replacement) => out.push_str(&replacement),
            None => out.push_str(&text[s.start..s.end]),
        }
        cursor = s.end;
    }
    out.push_str(&text[cursor..]);
    out
}

// ---------------------------------------------------------------------------
// The deterministic prompt compiler
// ---------------------------------------------------------------------------

/// The structured settings a prompt is compiled from.
///
/// Fully serialisable so a generation can be reconstructed from its record —
/// that is what powers Rerun and recipe import/export, and it only works if
/// nothing in the compile step depends on state that is not in this struct.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptParts {
    /// Slug of a [`crate::camera`] template.
    pub camera: Option<String>,
    /// The preset's contribution, from the resolved variant.
    pub preset: Option<String>,
    /// What the user typed.
    pub scene: String,
    pub lighting: Option<String>,
    pub lens: Option<String>,
    pub mood: Option<String>,
}

impl PromptParts {
    pub fn scene(scene: &str) -> Self {
        PromptParts {
            scene: scene.to_string(),
            ..Default::default()
        }
    }

    pub fn with_camera(mut self, slug: &str) -> Self {
        self.camera = Some(slug.to_string());
        self
    }

    pub fn with_preset(mut self, text: &str) -> Self {
        self.preset = Some(text.to_string());
        self
    }

    pub fn with_lighting(mut self, text: &str) -> Self {
        self.lighting = Some(text.to_string());
        self
    }

    pub fn with_lens(mut self, text: &str) -> Self {
        self.lens = Some(text.to_string());
        self
    }

    pub fn with_mood(mut self, text: &str) -> Self {
        self.mood = Some(text.to_string());
        self
    }

    /// The camera move this prompt uses, if the slug names one we ship.
    pub fn camera_template(&self) -> Option<&'static CameraTemplate> {
        self.camera.as_deref().and_then(camera::get)
    }

    /// Compile to the final prompt string.
    ///
    /// Order is fixed: camera move, preset, scene, lighting, lens, mood. The
    /// move goes first because it is the one part guaranteed to say nothing
    /// about the content, so it reads as a directive rather than as competing
    /// scene description.
    ///
    /// An unrecognised camera slug is **omitted**, not inserted as raw text.
    /// A stale slug in an imported recipe should cost the move, not paste
    /// `dolly-inn` into the prompt where the model will try to render it.
    /// Callers that need to warn can check [`PromptParts::camera_template`].
    pub fn compile(&self) -> String {
        let mut out = String::new();

        if let Some(t) = self.camera_template() {
            // Already a terminated five-sentence chain.
            out.push_str(&t.render());
        }
        push_clause(&mut out, None, self.preset.as_deref());
        push_clause(&mut out, None, Some(self.scene.as_str()));
        push_clause(&mut out, Some("Lighting"), self.lighting.as_deref());
        push_clause(&mut out, Some("Lens"), self.lens.as_deref());
        push_clause(&mut out, Some("Mood"), self.mood.as_deref());

        out
    }
}

/// Append one clause, normalised: trimmed, de-duplicated terminator, single
/// space separator. Normalising here rather than trusting the inputs is what
/// makes the output canonical — two settings that differ only in a trailing
/// period must not produce two different cache keys.
fn push_clause(out: &mut String, label: Option<&str>, body: Option<&str>) {
    let Some(body) = body else { return };
    let body = body.trim().trim_end_matches(['.', ' ']).trim();
    if body.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push(' ');
    }
    if let Some(label) = label {
        out.push_str(label);
        out.push_str(": ");
    }
    out.push_str(body);
    out.push('.');
}

/// A prompt, ready to submit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledPrompt {
    pub prompt: String,
    pub negative_prompt: Option<String>,
    /// Whether to run the rewriter before submitting.
    pub enhance: bool,
    pub reason: EnhanceReason,
    /// Set when the prompt still points at attachments. A caller that has not
    /// bound them must not submit, and must not offer "reuse this prompt".
    pub has_unresolved_sentinel: bool,
}

/// Compile a prompt and settle the enhance question in one call. This is the
/// harness entry point; everything above it is separately testable on purpose.
pub fn build(
    parts: &PromptParts,
    inputs: EnhanceInputs,
    negative_prompt: Option<&str>,
) -> CompiledPrompt {
    let prompt = parts.compile();
    let decision = decide(inputs);
    CompiledPrompt {
        has_unresolved_sentinel: has_unresolved_sentinel(&prompt),
        prompt,
        negative_prompt: negative_prompt
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        enhance: decision.enhance,
        reason: decision.reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::{Category, PresetFamily};

    // ---- Rule 3: the per-job-type default table ---------------------------

    #[test]
    fn the_default_table_splits_prose_models_from_instruction_models() {
        let on = [
            JobType::Video,
            JobType::ImageStyled,
            JobType::ImageWan22,
            JobType::ImageGpt,
            JobType::ImageFlux,
            JobType::ImageFlux2,
            JobType::ImageFlux2Flex,
            JobType::ImageFlux2Max,
            JobType::ImageKlingOmni,
            JobType::ZImage,
            JobType::Avatar,
            JobType::ComplexAvatar,
            JobType::Builder,
        ];
        let off = [
            JobType::Animate,
            JobType::ImageNanoBanana,
            JobType::ImageNanoBanana2,
            JobType::ImageSeedream,
            JobType::ImageGptImage2,
            JobType::ImageGptImage2Mini,
            JobType::Reference,
            JobType::Scene,
            JobType::Product,
            JobType::Speech,
            JobType::Lipsync,
            JobType::Photodump,
            JobType::FashionFactory,
        ];

        for j in on {
            assert!(j.default_enhance(), "{j:?} should default on");
        }
        for j in off {
            assert!(!j.default_enhance(), "{j:?} should default off");
        }
        // Nothing may be added to the enum without a deliberate default.
        assert_eq!(on.len() + off.len(), JobType::ALL.len());
    }

    #[test]
    fn the_two_gpt_surfaces_differ_on_purpose() {
        // Not a typo: the legacy image state enhances, GPT Image 2 does not.
        assert!(JobType::ImageGpt.default_enhance());
        assert!(!JobType::ImageGptImage2.default_enhance());
        assert!(!JobType::ImageGptImage2Mini.default_enhance());
    }

    #[test]
    fn job_types_serialise_stably_and_slugs_agree() {
        assert_eq!(
            serde_json::to_string(&JobType::ImageNanoBanana2).unwrap(),
            "\"image-nano-banana-2\""
        );
        let back: JobType = serde_json::from_str("\"z-image\"").unwrap();
        assert_eq!(back, JobType::ZImage);

        // The serde name and `slug()` are two hand-written lists; they must
        // never drift, because both end up in persisted job records.
        for j in JobType::ALL {
            let wire = serde_json::to_string(&j).unwrap();
            assert_eq!(wire, format!("\"{}\"", j.slug()), "{j:?}");
            assert_eq!(JobType::from_slug(j.slug()), Some(j));
        }
        assert_eq!(JobType::from_slug("nope"), None);
    }

    // ---- Rule 1: a real preset forces enhancement on ----------------------

    #[test]
    fn rule_1_a_real_preset_forces_enhance_on_against_the_toggle() {
        for job in [JobType::Video, JobType::ImageSeedream, JobType::Animate] {
            let d = decide(
                EnhanceInputs::new(job)
                    .with_preset(PresetSelection::Real)
                    .with_toggle(false),
            );
            assert!(d.enhance, "{job:?}: preset must override an off toggle");
            assert_eq!(d.reason, EnhanceReason::Preset);
            assert!(d.is_forced());
        }
    }

    #[test]
    fn rule_1_beats_a_job_type_that_defaults_off() {
        // Seedream defaults off; a preset still forces it on.
        let d =
            decide(EnhanceInputs::new(JobType::ImageSeedream).with_preset(PresetSelection::Real));
        assert!(d.enhance);
        assert_eq!(d.reason, EnhanceReason::Preset);
    }

    // ---- Rule 2: an end frame forces enhancement off ----------------------

    #[test]
    fn rule_2_an_end_frame_forces_enhance_off_unconditionally() {
        for toggle in [None, Some(true), Some(false)] {
            for selection in [
                PresetSelection::None,
                PresetSelection::General,
                PresetSelection::Real,
            ] {
                let mut inputs = EnhanceInputs::new(JobType::Video)
                    .with_preset(selection)
                    .with_end_frame(true);
                inputs.user_toggle = toggle;

                let d = decide(inputs);
                assert!(
                    !d.enhance,
                    "end frame must win over {selection:?} + toggle {toggle:?}"
                );
                assert_eq!(d.reason, EnhanceReason::EndFrame);
                assert!(d.is_forced());
            }
        }
    }

    /// The interaction that decides the precedence order. Both rules fire;
    /// rule 2 wins.
    #[test]
    fn rule_2_beats_rule_1_when_a_preset_and_an_end_frame_are_both_present() {
        let d = decide(
            EnhanceInputs::new(JobType::Video)
                .with_preset(PresetSelection::Real)
                .with_end_frame(true)
                .with_toggle(true),
        );
        assert!(
            !d.enhance,
            "first-last-frame interpolation must never be rewritten, preset or not"
        );
        assert_eq!(d.reason, EnhanceReason::EndFrame);

        // And without the end frame the same setup enhances, proving the end
        // frame is what changed the answer.
        let d = decide(
            EnhanceInputs::new(JobType::Video)
                .with_preset(PresetSelection::Real)
                .with_toggle(true),
        );
        assert!(d.enhance);
        assert_eq!(d.reason, EnhanceReason::Preset);
    }

    // ---- Rule 3: General honours the user ---------------------------------

    #[test]
    fn rule_3_general_honours_the_toggle_in_both_directions() {
        for on in [true, false] {
            let d = decide(
                EnhanceInputs::new(JobType::Video)
                    .with_preset(PresetSelection::General)
                    .with_toggle(on),
            );
            assert_eq!(d.enhance, on);
            assert_eq!(d.reason, EnhanceReason::UserToggle);
            assert!(!d.is_forced());
        }
    }

    #[test]
    fn rule_3_falls_back_to_the_job_default_when_untouched() {
        let d = decide(EnhanceInputs::new(JobType::Video).with_preset(PresetSelection::General));
        assert!(d.enhance);
        assert_eq!(d.reason, EnhanceReason::JobDefault);

        let d = decide(EnhanceInputs::new(JobType::ImageSeedream));
        assert!(!d.enhance);
        assert_eq!(d.reason, EnhanceReason::JobDefault);
    }

    #[test]
    fn no_preset_behaves_exactly_like_general() {
        for job in JobType::ALL {
            for toggle in [None, Some(true), Some(false)] {
                let mut a = EnhanceInputs::new(job).with_preset(PresetSelection::None);
                let mut b = EnhanceInputs::new(job).with_preset(PresetSelection::General);
                a.user_toggle = toggle;
                b.user_toggle = toggle;
                assert_eq!(decide(a), decide(b), "{job:?} {toggle:?}");
            }
        }
    }

    /// Exhaustive: every combination of the three inputs, checked against the
    /// rules stated independently of the implementation.
    #[test]
    fn the_three_rules_hold_across_every_combination() {
        for job in JobType::ALL {
            for preset in [
                PresetSelection::None,
                PresetSelection::General,
                PresetSelection::Real,
            ] {
                for end_frame in [false, true] {
                    for toggle in [None, Some(true), Some(false)] {
                        let mut inputs = EnhanceInputs::new(job)
                            .with_preset(preset)
                            .with_end_frame(end_frame);
                        inputs.user_toggle = toggle;
                        let got = decide(inputs);

                        let want = if end_frame {
                            false
                        } else if preset == PresetSelection::Real {
                            true
                        } else {
                            toggle.unwrap_or_else(|| job.default_enhance())
                        };
                        assert_eq!(
                            got.enhance, want,
                            "{job:?} {preset:?} {end_frame} {toggle:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn preset_selection_is_derived_from_the_catalogue_id() {
        assert_eq!(PresetSelection::from_id(None), PresetSelection::None);
        assert_eq!(
            PresetSelection::from_id(Some("general")),
            PresetSelection::General
        );
        assert_eq!(
            PresetSelection::from_id(Some("veo3-general")),
            PresetSelection::General
        );
        assert_eq!(
            PresetSelection::from_id(Some("earth-zoom-out")),
            PresetSelection::Real
        );

        let real = PresetFamily::new("earth-zoom-out", "Earth Zoom Out", Category::Effects);
        let general = PresetFamily::new("general", "General", Category::New);
        assert_eq!(
            PresetSelection::from_family(Some(&real)),
            PresetSelection::Real
        );
        assert_eq!(
            PresetSelection::from_family(Some(&general)),
            PresetSelection::General
        );
        assert_eq!(PresetSelection::from_family(None), PresetSelection::None);
    }

    #[test]
    fn every_reason_can_explain_itself() {
        for r in [
            EnhanceReason::EndFrame,
            EnhanceReason::Preset,
            EnhanceReason::UserToggle,
            EnhanceReason::JobDefault,
        ] {
            assert!(!r.explanation().is_empty());
        }
        assert!(EnhanceReason::EndFrame.is_forced());
        assert!(EnhanceReason::Preset.is_forced());
        assert!(!EnhanceReason::UserToggle.is_forced());
        assert!(!EnhanceReason::JobDefault.is_forced());
    }

    // ---- Sentinels --------------------------------------------------------

    #[test]
    fn sentinel_tokens_have_the_exact_wire_shape() {
        assert_eq!(sentinel(SentinelKind::Image, 1), "<<<image_1>>>");
        assert_eq!(sentinel(SentinelKind::Video, 2), "<<<video_2>>>");
        assert_eq!(sentinel(SentinelKind::Element, 3), "<<<element_3>>>");
    }

    #[test]
    fn sentinels_are_found_with_their_kind_index_and_span() {
        let text = "Blend <<<image_1>>> with <<<video_2>>> and <<<element_30>>>.";
        let found = sentinels(text);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].kind, SentinelKind::Image);
        assert_eq!(found[0].index, 1);
        assert_eq!(&text[found[0].start..found[0].end], "<<<image_1>>>");
        assert_eq!(found[1].kind, SentinelKind::Video);
        assert_eq!(found[1].index, 2);
        assert_eq!(found[2].kind, SentinelKind::Element);
        assert_eq!(found[2].index, 30);
        assert_eq!(found[2].token(), "<<<element_30>>>");
    }

    #[test]
    fn reuse_prompt_refuses_while_a_sentinel_is_present() {
        assert!(has_unresolved_sentinel("use <<<image_1>>> here"));
        assert!(!can_reuse_prompt("use <<<image_1>>> here"));
        assert!(can_reuse_prompt("use the attached photo here"));
        assert!(can_reuse_prompt(""));
    }

    #[test]
    fn near_misses_are_not_sentinels() {
        for text in [
            "<<<image>>>",     // no index
            "<<<image_>>>",    // empty index
            "<<<sticker_1>>>", // unknown kind
            "<<<image_one>>>", // non-numeric index
            "<<image_1>>",     // wrong delimiters
            "<<<image_1",      // unterminated
            "image_1",         // bare
            "<<<IMAGE_1>>>",   // case matters; the wire form is lowercase
            "a < b <<< c",     // stray opener
        ] {
            assert!(
                !has_unresolved_sentinel(text),
                "{text:?} must not read as a sentinel"
            );
        }
        // A malformed opener must not stop a later valid one being found.
        assert!(has_unresolved_sentinel("<<<image_>>> then <<<video_9>>>"));
    }

    #[test]
    fn resolving_replaces_only_what_it_can_and_leaves_the_rest_detectable() {
        let text = "Put <<<image_1>>> next to <<<video_2>>>.";
        let out = resolve_sentinels(text, |kind, i| match (kind, i) {
            (SentinelKind::Image, 1) => Some("the red bicycle".into()),
            _ => None,
        });
        assert_eq!(out, "Put the red bicycle next to <<<video_2>>>.");
        // Still unresolved, so still not reusable.
        assert!(has_unresolved_sentinel(&out));

        let all = resolve_sentinels(&out, |_, _| Some("X".into()));
        assert_eq!(all, "Put the red bicycle next to X.");
        assert!(can_reuse_prompt(&all));

        // Text with no sentinels comes back untouched.
        assert_eq!(resolve_sentinels("plain", |_, _| Some("X".into())), "plain");
    }

    #[test]
    fn resolution_survives_multibyte_text() {
        let text = "café <<<image_1>>> — naïve";
        let out = resolve_sentinels(text, |_, _| Some("piña".into()));
        assert_eq!(out, "café piña — naïve");
    }

    // ---- The compiler -----------------------------------------------------

    fn full_parts() -> PromptParts {
        PromptParts::scene("A cracked porcelain teapot on a windowsill at dawn")
            .with_camera("push-in")
            .with_preset("soft film grain and gentle halation")
            .with_lighting("low winter sun through dusty glass")
            .with_lens("50mm, shallow depth of field")
            .with_mood("quiet, unhurried")
    }

    #[test]
    fn compiles_the_slots_in_a_fixed_order() {
        let got = full_parts().compile();
        assert_eq!(
            got,
            "Camera: a slow creep straight at the subject. \
             Movement: advance along the lens axis, closing the gap. \
             Speed: so gradual it is barely noticeable. \
             Framing: narrow the frame steadily so pressure builds. \
             End: arrive at a close-up and stop. \
             soft film grain and gentle halation. \
             A cracked porcelain teapot on a windowsill at dawn. \
             Lighting: low winter sun through dusty glass. \
             Lens: 50mm, shallow depth of field. \
             Mood: quiet, unhurried."
        );
    }

    #[test]
    fn compilation_is_deterministic_and_free_of_hidden_state() {
        let p = full_parts();
        let first = p.compile();
        for _ in 0..64 {
            assert_eq!(p.compile(), first);
        }
        // Reconstructed from its own serialised form, byte-identical.
        let json = serde_json::to_string(&p).unwrap();
        let back: PromptParts = serde_json::from_str(&json).unwrap();
        assert_eq!(back.compile(), first);
        assert_eq!(back, p);
    }

    #[test]
    fn optional_slots_are_simply_absent() {
        assert_eq!(
            PromptParts::scene("A lone lighthouse").compile(),
            "A lone lighthouse."
        );
        assert_eq!(PromptParts::default().compile(), "");
        assert_eq!(
            PromptParts::scene("  ")
                .with_lighting("")
                .with_mood("   ")
                .compile(),
            ""
        );
    }

    #[test]
    fn terminators_and_whitespace_are_normalised_to_one_canonical_form() {
        // Two settings differing only in punctuation must compile identically,
        // or they would produce different cache keys for the same generation.
        let a = PromptParts::scene("A lone lighthouse.").with_mood("bleak.");
        let b = PromptParts::scene("  A lone lighthouse  ").with_mood(" bleak ");
        assert_eq!(a.compile(), "A lone lighthouse. Mood: bleak.");
        assert_eq!(a.compile(), b.compile());
    }

    #[test]
    fn an_unknown_camera_slug_is_dropped_not_pasted_into_the_prompt() {
        let p = PromptParts::scene("A lone lighthouse").with_camera("dolly-inn");
        assert!(p.camera_template().is_none());
        assert_eq!(p.compile(), "A lone lighthouse.");
        assert!(!p.compile().contains("dolly-inn"));
    }

    #[test]
    fn the_camera_move_survives_a_scene_swap_byte_for_byte() {
        let move_text = camera::get("360-orbit").unwrap().render();
        for scene in ["A brass diving helmet", "A city seen from orbit"] {
            let out = PromptParts::scene(scene).with_camera("360-orbit").compile();
            assert!(out.starts_with(&move_text), "{out}");
            assert!(out.ends_with(&format!("{scene}.")));
        }
    }

    #[test]
    fn every_shipped_camera_move_compiles_into_a_prompt() {
        for slug in camera::slugs() {
            let out = PromptParts::scene("A scene").with_camera(slug).compile();
            assert!(out.starts_with("Camera: "), "{slug}");
            assert!(out.ends_with("A scene."), "{slug}");
            assert!(!out.contains(".."), "{slug} produced a doubled period");
        }
    }

    // ---- The whole harness together ---------------------------------------

    #[test]
    fn build_compiles_and_decides_in_one_pass() {
        let out = build(
            &full_parts(),
            EnhanceInputs::new(JobType::Video)
                .with_preset(PresetSelection::Real)
                .with_toggle(false),
            Some("  warping, jitter "),
        );
        assert!(out.enhance, "rule 1");
        assert_eq!(out.reason, EnhanceReason::Preset);
        assert_eq!(out.negative_prompt.as_deref(), Some("warping, jitter"));
        assert!(!out.has_unresolved_sentinel);
        assert!(out.prompt.starts_with("Camera: "));
    }

    #[test]
    fn build_reports_an_unresolved_sentinel_from_the_scene() {
        let out = build(
            &PromptParts::scene("Match the style of <<<image_1>>>"),
            EnhanceInputs::new(JobType::Video),
            None,
        );
        assert!(out.has_unresolved_sentinel);
        assert!(!can_reuse_prompt(&out.prompt));
        assert_eq!(out.negative_prompt, None);
    }

    #[test]
    fn build_honours_the_end_frame_override_end_to_end() {
        let out = build(
            &full_parts(),
            EnhanceInputs::new(JobType::Video)
                .with_preset(PresetSelection::Real)
                .with_end_frame(true)
                .with_toggle(true),
            None,
        );
        assert!(!out.enhance);
        assert_eq!(out.reason, EnhanceReason::EndFrame);
        // The prompt itself is unaffected by the enhance decision.
        assert_eq!(out.prompt, full_parts().compile());
    }

    #[test]
    fn an_empty_negative_prompt_is_dropped_rather_than_sent_blank() {
        let out = build(
            &PromptParts::scene("A scene"),
            EnhanceInputs::new(JobType::Video),
            Some("   "),
        );
        assert_eq!(out.negative_prompt, None);
    }
}
