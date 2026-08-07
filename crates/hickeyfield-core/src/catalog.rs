//! The model catalogue, seeded from Higgsfield's own MIT-licensed CLI spec.
//!
//! `vendor/higgsfield-cli-MODELS.md` enumerates every model they expose with
//! its flags, defaults, enums, cardinalities and constraints. Parsing it beats
//! hand-transcribing 55 models, and it gives us a diffable record when their
//! roster moves.
//!
//! This seeds *what a model accepts*. It deliberately says nothing about which
//! provider serves it or what it costs — that is our route table's job.

use std::collections::BTreeMap;
use std::fmt;

/// Higgsfield's own MODELS.md, embedded at compile time.
pub const VENDORED_SPEC: &str = include_str!("../vendor/higgsfield-cli-MODELS.md");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modality {
    Image,
    Video,
    ThreeD,
    Audio,
    /// "Video explainer jobs" — a pipeline section, not a plain model list.
    Other,
}

impl Modality {
    fn from_heading(h: &str) -> Self {
        let h = h.to_ascii_lowercase();
        if h.starts_with("image") {
            Modality::Image
        } else if h.starts_with("video explainer") {
            Modality::Other
        } else if h.starts_with("video") {
            Modality::Video
        } else if h.starts_with("3d") {
            Modality::ThreeD
        } else if h.starts_with("audio") {
            Modality::Audio
        } else {
            Modality::Other
        }
    }
}

impl fmt::Display for Modality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Modality::Image => "image",
            Modality::Video => "video",
            Modality::ThreeD => "3d",
            Modality::Audio => "audio",
            Modality::Other => "other",
        })
    }
}

/// What a flag accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueSpec {
    Enum(Vec<String>),
    Integer,
    Number,
    Boolean,
    Text,
    /// An upload id or a local path. These are the media inputs.
    Media,
    Array,
    /// Anything the spec expresses in prose we haven't modelled. Kept verbatim
    /// rather than silently coerced.
    Unknown(String),
}

impl ValueSpec {
    fn parse(cell: &str) -> Self {
        let raw = cell.trim();
        if raw.contains('`') {
            let variants: Vec<String> = raw
                .split(',')
                .filter_map(|p| {
                    p.trim()
                        .strip_prefix('`')?
                        .strip_suffix('`')
                        .map(String::from)
                })
                .collect();
            if !variants.is_empty() {
                return ValueSpec::Enum(variants);
            }
        }
        match raw.to_ascii_lowercase().as_str() {
            "integer" => ValueSpec::Integer,
            "number" => ValueSpec::Number,
            "boolean" => ValueSpec::Boolean,
            "string" => ValueSpec::Text,
            "uuid or path" => ValueSpec::Media,
            "array" => ValueSpec::Array,
            "" | "—" | "-" => ValueSpec::Unknown(String::new()),
            _ => ValueSpec::Unknown(raw.to_string()),
        }
    }

    pub fn is_media(&self) -> bool {
        matches!(self, ValueSpec::Media)
    }
}

/// How many values a flag takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    One,
    Repeated,
    Range { min: u32, max: u32 },
}

impl Arity {
    pub fn max(self) -> Option<u32> {
        match self {
            Arity::One => Some(1),
            Arity::Repeated => None,
            Arity::Range { max, .. } => Some(max),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagSpec {
    /// Canonical name with no leading dashes, underscored: `aspect_ratio`.
    pub name: String,
    /// The documented alternate spelling, e.g. `image` for `image-references`.
    pub alias: Option<String>,
    pub required: bool,
    pub default: Option<String>,
    pub value: ValueSpec,
    pub arity: Arity,
}

impl FlagSpec {
    /// Every spelling that should resolve to this flag. The spec states that
    /// `--aspect_ratio` and `--aspect-ratio` are always equivalent.
    pub fn accepted_names(&self) -> Vec<String> {
        let mut names = vec![self.name.clone(), self.name.replace('_', "-")];
        if let Some(a) = &self.alias {
            names.push(a.clone());
            names.push(a.replace('_', "-"));
        }
        names.sort();
        names.dedup();
        names
    }
}

/// The option sets a model actually offers.
///
/// Each axis carries **two** facts, because they mean different things to the
/// UI and conflating them is what produced the fabricated chip row:
///
/// - `supports_*` — the model has this control at all. False on an image model
///   asked about duration; the chip should be *hidden*, not defaulted.
/// - the list — the enumerated choices. Empty with `supports_*` true means the
///   model takes a free value (28 of 32 video models declare `duration` as a
///   plain integer). The UI must offer a number input, **not** invent a list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Capabilities {
    pub supports_duration: bool,
    pub durations: Vec<f64>,
    pub default_duration: Option<f64>,

    pub supports_resolution: bool,
    pub resolutions: Vec<String>,
    pub default_resolution: Option<String>,

    pub supports_aspect: bool,
    pub aspects: Vec<String>,
    pub default_aspect: Option<String>,

    /// The model can *produce* sound, not merely accept it.
    pub audio: bool,

    /// Cross-field rules the flag table cannot express, verbatim from the spec
    /// ("End_image requires start_image to also be provided"). 40 of 68 models
    /// carry at least one.
    pub constraints: Vec<String>,
}

fn enum_options(f: &FlagSpec) -> Vec<String> {
    match &f.value {
        ValueSpec::Enum(v) => v.clone(),
        _ => Vec::new(),
    }
}

/// Seconds may be written `5`, `5s` or `5.5`. Anything else is left out rather
/// than coerced — a duration we misread becomes a price we misquote.
fn parse_seconds(raw: &str) -> Option<f64> {
    raw.trim().trim_end_matches('s').trim().parse::<f64>().ok()
}

fn numeric_options(f: &FlagSpec) -> Vec<f64> {
    match &f.value {
        ValueSpec::Enum(v) => v.iter().filter_map(|s| parse_seconds(s)).collect(),
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Higgsfield's `job_set_type`, e.g. `seedance_2_0`. We reuse it as our own
    /// model id so their preset and cost data line up without a mapping table.
    pub id: String,
    pub display_name: String,
    pub modality: Modality,
    pub flags: Vec<FlagSpec>,
    /// Prose constraints, kept verbatim — several encode cross-field rules the
    /// table cannot express ("At most 14 image references are allowed").
    pub constraints: Vec<String>,
}

impl ModelSpec {
    pub fn flag(&self, name: &str) -> Option<&FlagSpec> {
        let want = name.trim_start_matches('-').replace('-', "_");
        self.flags
            .iter()
            .find(|f| f.name == want || f.alias.as_deref() == Some(want.as_str()))
    }

    pub fn required_flags(&self) -> impl Iterator<Item = &FlagSpec> {
        self.flags.iter().filter(|f| f.required)
    }

    /// Media inputs, which are the flags needing upload handling.
    pub fn media_flags(&self) -> impl Iterator<Item = &FlagSpec> {
        self.flags.iter().filter(|f| f.value.is_media())
    }

    /// What this model can actually be asked for.
    ///
    /// Derived from the model's own declared flags rather than assumed, which
    /// is the whole point: the UI previously showed one hardcoded option set
    /// for all 68 models, so picking 10s on a 5s-only model quoted a 10s price
    /// and then failed at the provider after the round trip.
    pub fn capabilities(&self) -> Capabilities {
        let dur = self.flag("duration");
        let res = self.flag("resolution");
        let asp = self.flag("aspect_ratio");

        Capabilities {
            supports_duration: dur.is_some(),
            durations: dur.map(numeric_options).unwrap_or_default(),
            default_duration: dur
                .and_then(|f| f.default.as_deref())
                .and_then(parse_seconds),

            supports_resolution: res.is_some(),
            resolutions: res.map(enum_options).unwrap_or_default(),
            default_resolution: res.and_then(|f| f.default.clone()),

            supports_aspect: asp.is_some(),
            aspects: asp.map(enum_options).unwrap_or_default(),
            default_aspect: asp.and_then(|f| f.default.clone()),

            // `audio`/`audio_references` are *inputs*; only these two mean the
            // model can produce sound.
            audio: self.flag("generate_audio").is_some() || self.flag("sound").is_some(),

            constraints: self.constraints.clone(),
        }
    }

    pub fn takes_prompt(&self) -> bool {
        self.flag("prompt").is_some()
    }
}

/// Parse the vendored spec. Infallible by design: a malformed row is skipped
/// rather than failing the whole catalogue, because a partial catalogue still
/// launches the app and a hard failure at startup would not.
pub fn parse(md: &str) -> Vec<ModelSpec> {
    let mut models = Vec::new();
    let mut modality = Modality::Other;
    let mut current: Option<ModelSpec> = None;
    let mut in_constraints = false;

    for line in md.lines() {
        let t = line.trim();

        if let Some(h) = t.strip_prefix("## ") {
            if let Some(m) = current.take() {
                models.push(m);
            }
            modality = Modality::from_heading(h);
            in_constraints = false;
            continue;
        }

        if let Some(h) = t.strip_prefix("### ") {
            if let Some(m) = current.take() {
                models.push(m);
            }
            in_constraints = false;
            // "seedance_2_0 — Seedance 2.0" (em dash)
            let (id, display) = match h.split_once('—') {
                Some((a, b)) => (a.trim(), b.trim()),
                None => (h.trim(), h.trim()),
            };
            current = Some(ModelSpec {
                id: id.trim_matches('`').to_string(),
                display_name: display.to_string(),
                modality,
                flags: Vec::new(),
                constraints: Vec::new(),
            });
            continue;
        }

        let Some(model) = current.as_mut() else {
            continue;
        };

        if t.starts_with("Constraints:") {
            in_constraints = true;
            continue;
        }

        if in_constraints {
            if let Some(c) = t.strip_prefix("- ") {
                model.constraints.push(c.trim().to_string());
                continue;
            }
            if !t.is_empty() {
                in_constraints = false;
            }
        }

        if t.starts_with("| `--") {
            if let Some(flag) = parse_flag_row(t) {
                model.flags.push(flag);
            }
        }
    }

    if let Some(m) = current.take() {
        models.push(m);
    }
    models
}

/// `| `--image-references` (or `--image`) (0..14) | false | — | UUID or path |`
fn parse_flag_row(row: &str) -> Option<FlagSpec> {
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
    if cells.len() < 4 {
        return None;
    }

    let (name, alias, arity) = parse_flag_name(cells[0])?;
    let required = cells[1].eq_ignore_ascii_case("true");
    let default = match cells[2].trim() {
        "" | "—" | "-" => None,
        d => Some(d.trim_matches('`').to_string()),
    };

    Some(FlagSpec {
        name,
        alias,
        required,
        default,
        value: ValueSpec::parse(cells[3]),
        arity,
    })
}

fn parse_flag_name(cell: &str) -> Option<(String, Option<String>, Arity)> {
    let mut names = cell
        .split('`')
        .filter(|s| s.starts_with("--"))
        .map(|s| s.trim_start_matches('-').replace('-', "_"));

    let name = names.next()?;
    let alias = names.next();

    // Cardinality is whatever parenthesised token is not the "(or --x)" alias.
    let arity = if cell.contains("(repeated)") {
        Arity::Repeated
    } else if let Some(range) = extract_range(cell) {
        range
    } else {
        Arity::One
    };

    Some((name, alias, arity))
}

/// `(0..14)` -> `Arity::Range { min: 0, max: 14 }`
fn extract_range(cell: &str) -> Option<Arity> {
    let start = cell.find("..")?;
    let before = &cell[..start];
    let after = &cell[start + 2..];
    let min: u32 = before
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|s| !s.is_empty())?
        .parse()
        .ok()?;
    let max: u32 = after
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|s| !s.is_empty())?
        .parse()
        .ok()?;
    Some(Arity::Range { min, max })
}

/// The parsed vendored catalogue, keyed by model id.
pub fn catalogue() -> BTreeMap<String, ModelSpec> {
    parse(VENDORED_SPEC)
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Vec<ModelSpec> {
        parse(VENDORED_SPEC)
    }

    #[test]
    fn parses_the_whole_vendored_spec() {
        let models = cat();
        // The spec's own section headers claim Image(23) + Video(22) + 3D(5) +
        // Audio(5) = 55, plus the explainer pipeline entries. If this trips
        // after a vendor refresh, update it deliberately — the diff is the
        // record of what Higgsfield changed.
        assert!(
            models.len() >= 55,
            "expected at least 55 models, got {}",
            models.len()
        );
        assert!(models.iter().all(|m| !m.id.is_empty()));
        assert!(models.iter().all(|m| !m.flags.is_empty()));
    }

    #[test]
    fn model_ids_are_unique() {
        let models = cat();
        let mut seen = std::collections::HashSet::new();
        for m in &models {
            assert!(seen.insert(m.id.clone()), "duplicate model id {}", m.id);
        }
    }

    #[test]
    fn section_counts_match_the_headings() {
        let models = cat();
        let n = |m: Modality| models.iter().filter(|x| x.modality == m).count();
        assert_eq!(n(Modality::Image), 23, "Image (23)");
        assert_eq!(n(Modality::Video), 22, "Video (22)");
        assert_eq!(n(Modality::ThreeD), 5, "3D (5)");
        assert_eq!(n(Modality::Audio), 5, "Audio (5)");
    }

    #[test]
    fn parses_enums_defaults_and_required() {
        let c = catalogue();
        let veo = c.get("veo3_1").expect("veo3_1 present");
        assert_eq!(veo.display_name, "Google Veo 3.1");
        assert_eq!(veo.modality, Modality::Video);

        let ar = veo.flag("aspect_ratio").unwrap();
        assert_eq!(
            ar.value,
            ValueSpec::Enum(vec!["16:9".into(), "9:16".into()])
        );
        assert_eq!(ar.default.as_deref(), Some("16:9"));
        assert!(!ar.required);

        let prompt = veo.flag("prompt").unwrap();
        assert!(prompt.required);
        assert_eq!(prompt.value, ValueSpec::Text);
        assert_eq!(prompt.default, None);

        let dur = veo.flag("duration").unwrap();
        assert_eq!(
            dur.value,
            ValueSpec::Enum(vec!["4".into(), "6".into(), "8".into()])
        );
    }

    #[test]
    fn parses_media_arity_and_aliases() {
        let c = catalogue();

        // `--image-references` (or `--image`) (0..14)
        let cs = c.get("cinematic_studio_2_5").unwrap();
        let refs = cs.flag("image_references").unwrap();
        assert_eq!(refs.arity, Arity::Range { min: 0, max: 14 });
        assert_eq!(refs.alias.as_deref(), Some("image"));
        assert!(refs.value.is_media());
        // The alias must resolve too, or "--image" would silently do nothing.
        assert_eq!(cs.flag("image").map(|f| &f.name), Some(&refs.name));

        // `--image-references` (or `--image`) (repeated)
        let flux = c.get("flux_2").unwrap();
        assert_eq!(
            flux.flag("image_references").unwrap().arity,
            Arity::Repeated
        );

        // `--start-image` (single)
        let veo = c.get("veo3_1").unwrap();
        let start = veo.flag("start_image").unwrap();
        assert_eq!(start.arity, Arity::One);
        assert!(start.value.is_media());
    }

    #[test]
    fn dashed_and_underscored_spellings_both_resolve() {
        let c = catalogue();
        let m = c.get("flux_2").unwrap();
        assert!(m.flag("aspect_ratio").is_some());
        assert!(m.flag("aspect-ratio").is_some());
        assert!(m.flag("--aspect-ratio").is_some());

        let names = m.flag("image_references").unwrap().accepted_names();
        assert!(names.contains(&"image_references".to_string()));
        assert!(names.contains(&"image-references".to_string()));
        assert!(names.contains(&"image".to_string()));
    }

    #[test]
    fn captures_prose_constraints_verbatim() {
        let c = catalogue();
        let cs = c.get("cinematic_studio_2_5").unwrap();
        assert!(
            cs.constraints
                .iter()
                .any(|x| x.contains("At most 14 image references")),
            "constraints were {:?}",
            cs.constraints
        );
        // A constraints block must not leak into the next model.
        let flux = c.get("flux_2").unwrap();
        assert!(flux.constraints.is_empty());
    }

    #[test]
    fn scalar_value_types_are_distinguished() {
        let c = catalogue();
        let cs = c.get("cinematic_studio_2_5").unwrap();
        assert_eq!(cs.flag("batch_size").unwrap().value, ValueSpec::Integer);
        assert_eq!(cs.flag("folder_id").unwrap().value, ValueSpec::Text);
    }

    #[test]
    fn every_model_that_takes_a_prompt_marks_it_required_or_optional_explicitly() {
        // Guards against the default column and the required column being
        // swapped by a future format change.
        for m in cat() {
            if let Some(p) = m.flag("prompt") {
                assert!(
                    matches!(p.value, ValueSpec::Text | ValueSpec::Unknown(_)),
                    "{} prompt parsed as {:?}",
                    m.id,
                    p.value
                );
            }
        }
    }

    #[test]
    fn media_flags_are_discoverable_for_upload_handling() {
        let c = catalogue();
        let veo = c.get("veo3_1").unwrap();
        let media: Vec<_> = veo.media_flags().map(|f| f.name.as_str()).collect();
        assert!(media.contains(&"start_image"), "got {media:?}");
    }

    // ── Capabilities ───────────────────────────────────────────────────────

    #[test]
    fn an_image_model_does_not_claim_a_duration() {
        // The bug this replaces: every model reported durations [5, 8, 10], so
        // the chip row offered a duration control on a still-image model.
        let c = catalogue();
        let nb = c.get("nano_banana_2").unwrap();
        let caps = nb.capabilities();
        assert!(!caps.supports_duration);
        assert!(caps.durations.is_empty());
    }

    #[test]
    fn resolution_options_are_the_models_own_not_a_house_default() {
        let c = catalogue();
        // Video models speak in p; image models speak in k. Both are real, and
        // the old hardcoded ["720p", "1080p"] was wrong for half the roster.
        let seedance = c.get("seedance_2_0").unwrap().capabilities();
        assert_eq!(seedance.resolutions, ["480p", "720p", "1080p", "4k"]);
        assert_eq!(seedance.default_resolution.as_deref(), Some("720p"));

        let nb = c.get("nano_banana_2").unwrap().capabilities();
        assert_eq!(nb.resolutions, ["1k", "2k", "4k"]);
        assert_eq!(nb.default_resolution.as_deref(), Some("2k"));
    }

    #[test]
    fn a_free_form_duration_is_reported_as_supported_but_unenumerated() {
        // 28 of 32 video models declare `duration` as a plain integer. The UI
        // must offer a number input rather than inventing a list of choices.
        let c = catalogue();
        let kling = c.get("kling3_0").unwrap().capabilities();
        assert!(kling.supports_duration);
        assert!(
            kling.durations.is_empty(),
            "kling enumerates nothing; got {:?}",
            kling.durations
        );
        assert_eq!(kling.default_duration, Some(5.0));
    }

    #[test]
    fn audio_means_the_model_can_produce_sound_not_merely_accept_it() {
        // `--audio` and `--audio-references` are *inputs*. Treating them as
        // "has audio" would put an audio toggle on lipsync models that only
        // consume a track.
        let c = catalogue();
        for spec in c.values() {
            if spec.capabilities().audio {
                assert!(
                    spec.flag("generate_audio").is_some() || spec.flag("sound").is_some(),
                    "{} claims audio output with neither flag",
                    spec.id
                );
            }
        }
    }

    #[test]
    fn constraint_prose_survives_into_capabilities() {
        // These encode cross-field rules the table cannot express, and they are
        // the difference between a preventable 422 and a confusing one.
        let c = catalogue();
        let csv = c.get("cinematic_studio_video").unwrap().capabilities();
        assert!(
            csv.constraints.iter().any(|s| s.contains("start_image")),
            "got {:?}",
            csv.constraints
        );
    }

    #[test]
    fn every_model_reports_capabilities_without_panicking() {
        // Guards the parse helpers against a spec row shaped unlike the rest.
        let c = catalogue();
        let mut with_aspect = 0;
        for spec in c.values() {
            let caps = spec.capabilities();
            if caps.supports_aspect {
                with_aspect += 1;
            }
            // A default must always be one of the offered options, or the chip
            // row opens on a value the provider will reject.
            if let (Some(d), false) = (&caps.default_resolution, caps.resolutions.is_empty()) {
                assert!(
                    caps.resolutions.contains(d),
                    "{}: default resolution {d} is not in {:?}",
                    spec.id,
                    caps.resolutions
                );
            }
            if let (Some(d), false) = (&caps.default_aspect, caps.aspects.is_empty()) {
                assert!(
                    caps.aspects.contains(d),
                    "{}: default aspect {d} is not in {:?}",
                    spec.id,
                    caps.aspects
                );
            }
        }
        assert!(with_aspect > 40, "expected most models to take an aspect");
    }

    #[test]
    fn seconds_parse_the_shapes_the_spec_actually_uses() {
        assert_eq!(parse_seconds("5"), Some(5.0));
        assert_eq!(parse_seconds("5s"), Some(5.0));
        assert_eq!(parse_seconds(" 5.5 "), Some(5.5));
        // Not coerced: a misread duration becomes a misquoted price.
        assert_eq!(parse_seconds("auto"), None);
        assert_eq!(parse_seconds(""), None);
    }

    #[test]
    fn range_extraction_handles_each_documented_width() {
        for (input, want) in [
            ("(0..2)", Arity::Range { min: 0, max: 2 }),
            ("(0..14)", Arity::Range { min: 0, max: 14 }),
            ("(0..16)", Arity::Range { min: 0, max: 16 }),
        ] {
            assert_eq!(extract_range(input), Some(want), "input {input}");
        }
        assert_eq!(extract_range("(repeated)"), None);
        assert_eq!(extract_range("(single)"), None);
    }
}
