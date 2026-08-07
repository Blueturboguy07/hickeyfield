//! fal's whole catalogue, from fal.
//!
//! Until today the model roster was hand-transcribed from Higgsfield's public
//! surfaces into [`crate::registry`]: 68 models, typed out by a human, describing
//! *Higgsfield's* backend while every request we send goes to fal. Every bug
//! found on 2026-08-05 came from that inversion — wrong parameter names, wrong
//! endpoint splits, wrong accepted inputs. [`crate::fal_schema`] fixed the
//! per-endpoint half of it by reading fal's published OpenAPI instead of
//! guessing. This module fixes the other half: fal also publishes its *index*.
//!
//! `GET https://fal.ai/api/models?page=N` is unauthenticated and paged 40 to a
//! page. Measured on 2026-08-05: **36 pages, 1,418 live models**, each carrying
//! `id`, `title`, `category`, `shortDescription`, `pricingInfoOverride`,
//! `deprecated`, `removed`, `modelFamily` and `thumbnailUrl`. That is twenty
//! times the roster we had transcribed, it is authoritative for the provider we
//! actually call, and it costs one HTTP request to keep current.
//!
//! # What this module is not
//!
//! It is not a price feed. [`crate::prices`] owns the number that reaches the
//! Generate button and is deliberately stricter than this parser, because a
//! wrong figure there charges someone money. What [`Pricing`] gives you is what
//! fal *said*, reduced to a rate only where the sentence states one outright,
//! and kept as prose otherwise. Of the 653 models that publish pricing text,
//! 184 reduce to a rate and 469 stay prose — see [`parse_pricing`] for why the
//! remainder is refused rather than approximated.
//!
//! It is also not a route table. An id here is a fal endpoint, not proof that
//! Hickeyfield can drive it: that still comes from [`crate::registry`] and
//! [`crate::route`].
//!
//! # Offline first
//!
//! A snapshot of all 1,418 rows ships in the binary via `include_str!`, so the
//! first launch on a plane still lists models. [`refresh_in_background`] then
//! replaces it. [`Catalogue::captured`] records which of the two you are
//! looking at and when it was taken, because a roster from last month is only
//! honest if it says so.
//!
//! # Refreshing the snapshot
//!
//! ```sh
//! cargo run -p hickeyfield-core --example dump_fal_catalogue > \
//!   crates/hickeyfield-core/vendor/fal-catalogue-snapshot.json
//! cargo test -p hickeyfield-core fal_catalogue
//! ```
//!
//! The tests assert on the counts measured at capture time, so a refresh that
//! changes the roster fails loudly rather than drifting. Update the expected
//! numbers in the same commit and the diff reads as "this is what fal added".

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::prices::{Origin, BROWSER_USER_AGENT, FAL_MODELS_URL};

/// The catalogue compiled into the binary. Regenerated from the live index,
/// never hand-edited — a hand-edit is indistinguishable from fal's own data at
/// the point where someone is trying to work out why a model is missing.
const SNAPSHOT: &str = include_str!("../vendor/fal-catalogue-snapshot.json");

/// fal reported 36 pages on 2026-08-05. The cap stops a malformed `pages` field
/// from turning a background refresh into an unbounded crawl of the user's
/// connection.
const MAX_PAGES: u32 = 60;

/// Generous: this runs in the background and never blocks a generation, so a
/// slow page is cheaper than a refresh that gives up and leaves a stale roster.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Above this share of rows we cannot read, the page is treated as reshaped
/// rather than as containing a few odd entries.
///
/// The alternative failure is worse than it looks: silently keeping the 40% of
/// rows that still parse would hand the picker a catalogue that is *quietly*
/// missing most of fal, with no error anywhere. A loud refusal keeps the
/// snapshot, which is complete and merely old.
const MAX_UNREADABLE_SHARE: f64 = 0.10;

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

/// fal's own `category` field.
///
/// The 26 named variants are every value fal published on 2026-08-05, declared
/// in descending population order (`image-to-image` 385 … `workflow` 1). That
/// order is a display convenience — it is what [`Ord`] sorts by, so a category
/// sidebar comes out useful by default — and explicitly not a contract; fal can
/// repopulate it any day.
///
/// [`Category::Other`] is load-bearing. fal adds categories without warning and
/// a closed enum would either fail the whole page or drop the model, and a
/// model that exists but cannot be found is the worst of the three outcomes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Category {
    ImageToImage,
    TextToImage,
    ImageToVideo,
    VideoToVideo,
    TextToVideo,
    Training,
    TextToAudio,
    AudioToAudio,
    ImageTo3d,
    Vision,
    TextToSpeech,
    AudioToVideo,
    TextTo3d,
    SpeechToText,
    Llm,
    /// `3d-to-3d`. Named the long way round because a Rust identifier cannot
    /// begin with a digit.
    ThreeDTo3d,
    Json,
    VideoToAudio,
    VideoToText,
    TextToJson,
    SpeechToSpeech,
    AudioToText,
    ImageToText,
    ImageToJson,
    Workflow,
    /// fal's own literal `"unknown"`, carried by two live models. Distinct from
    /// [`Category::Other`]: this is fal saying it does not know, that is us.
    Unknown,
    /// A value fal published that this enum does not name, including the empty
    /// string when a row carries no category at all.
    Other(String),
}

impl Category {
    /// Every variant fal published on the capture date, in declaration order.
    pub fn named() -> [Category; 26] {
        [
            Category::ImageToImage,
            Category::TextToImage,
            Category::ImageToVideo,
            Category::VideoToVideo,
            Category::TextToVideo,
            Category::Training,
            Category::TextToAudio,
            Category::AudioToAudio,
            Category::ImageTo3d,
            Category::Vision,
            Category::TextToSpeech,
            Category::AudioToVideo,
            Category::TextTo3d,
            Category::SpeechToText,
            Category::Llm,
            Category::ThreeDTo3d,
            Category::Json,
            Category::VideoToAudio,
            Category::VideoToText,
            Category::TextToJson,
            Category::SpeechToSpeech,
            Category::AudioToText,
            Category::ImageToText,
            Category::ImageToJson,
            Category::Workflow,
            Category::Unknown,
        ]
    }

    /// fal's spelling.
    pub fn as_wire(&self) -> &str {
        match self {
            Category::ImageToImage => "image-to-image",
            Category::TextToImage => "text-to-image",
            Category::ImageToVideo => "image-to-video",
            Category::VideoToVideo => "video-to-video",
            Category::TextToVideo => "text-to-video",
            Category::Training => "training",
            Category::TextToAudio => "text-to-audio",
            Category::AudioToAudio => "audio-to-audio",
            Category::ImageTo3d => "image-to-3d",
            Category::Vision => "vision",
            Category::TextToSpeech => "text-to-speech",
            Category::AudioToVideo => "audio-to-video",
            Category::TextTo3d => "text-to-3d",
            Category::SpeechToText => "speech-to-text",
            Category::Llm => "llm",
            Category::ThreeDTo3d => "3d-to-3d",
            Category::Json => "json",
            Category::VideoToAudio => "video-to-audio",
            Category::VideoToText => "video-to-text",
            Category::TextToJson => "text-to-json",
            Category::SpeechToSpeech => "speech-to-speech",
            Category::AudioToText => "audio-to-text",
            Category::ImageToText => "image-to-text",
            Category::ImageToJson => "image-to-json",
            Category::Workflow => "workflow",
            Category::Unknown => "unknown",
            Category::Other(s) => s,
        }
    }

    /// Read fal's spelling. Anything unrecognised becomes [`Category::Other`]
    /// carrying the original string, never a discard.
    pub fn from_wire(s: &str) -> Category {
        let key = s.trim().to_ascii_lowercase();
        Category::named()
            .into_iter()
            .find(|c| c.as_wire() == key)
            .unwrap_or(Category::Other(s.trim().to_string()))
    }

    /// Whether this enum names the value, i.e. whether it is *not* an
    /// [`Category::Other`] the code has never seen.
    ///
    /// Exposed so a maintenance check can find categories fal has added since
    /// the last snapshot without reading every row by eye.
    pub fn is_named(&self) -> bool {
        !matches!(self, Category::Other(_))
    }
}

impl Default for Category {
    /// A row that names no category at all. `Other("")` rather than
    /// [`Category::Unknown`] because `unknown` is a value fal itself assigns,
    /// and conflating "fal said unknown" with "fal said nothing" would hide a
    /// reshaped feed.
    fn default() -> Self {
        Category::Other(String::new())
    }
}

impl std::fmt::Display for Category {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_wire())
    }
}

impl From<String> for Category {
    fn from(s: String) -> Category {
        Category::from_wire(&s)
    }
}

impl From<Category> for String {
    fn from(c: Category) -> String {
        c.as_wire().to_string()
    }
}

// ---------------------------------------------------------------------------
// Pricing
// ---------------------------------------------------------------------------

/// The denominator of a published rate.
///
/// Every variant corresponds to a phrase fal actually writes. There is no
/// generic "unit", because the whole point of naming them is that
/// `$0.0024 per megapixel of generated video` and `$0.05 per image` must never
/// be rendered by the same string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PriceUnit {
    /// Per generated image.
    Image,
    /// Per megapixel of generated image output.
    Megapixel,
    /// Per megapixel counted across *input and output* together — a different
    /// billed quantity from [`PriceUnit::Megapixel`], and fal writes it that
    /// way on eleven models. Rendering the two identically would understate an
    /// edit with a large reference image.
    InputAndOutputMegapixel,
    /// Per second of generated video.
    VideoSecond,
    /// Per megapixel of generated video data, i.e. width × height × frames.
    VideoMegapixel,
    /// Per second of the *input* video, for models billed on what you hand them.
    InputVideoSecond,
    /// Per second of generated audio.
    AudioSecond,
    /// Per minute of output.
    Minute,
    /// Per second of GPU time, for the models fal bills by the clock.
    ComputeSecond,
    /// Once per request, whatever it produces.
    Generation,
    /// Per training step.
    TrainingStep,
    /// Per 1,000 characters of input text.
    ThousandCharacters,
}

impl PriceUnit {
    /// A phrase to render after the figure. Written to read correctly directly
    /// after a price: "$0.05 per image".
    pub fn label(self) -> &'static str {
        match self {
            PriceUnit::Image => "per image",
            PriceUnit::Megapixel => "per megapixel",
            PriceUnit::InputAndOutputMegapixel => "per megapixel of input and output",
            PriceUnit::VideoSecond => "per second of video",
            PriceUnit::VideoMegapixel => "per megapixel of video (width × height × frames)",
            PriceUnit::InputVideoSecond => "per second of input video",
            PriceUnit::AudioSecond => "per second of audio",
            PriceUnit::Minute => "per minute",
            PriceUnit::ComputeSecond => "per compute second",
            PriceUnit::Generation => "per generation",
            PriceUnit::TrainingStep => "per training step",
            PriceUnit::ThousandCharacters => "per 1,000 characters",
        }
    }
}

/// What fal published about a model's price.
///
/// Three states, and the distinction between the last two is the point: prose
/// we refused to reduce is *still shown to the user*, because "we could not
/// parse this" and "fal said nothing" are different answers and only one of
/// them means there is nothing to read.
///
/// There is no variant that can carry zero. See [`Pricing::usd`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Pricing {
    /// fal carried no `pricingInfoOverride` at all — 765 of 1,418 models on
    /// 2026-08-05.
    Unpublished,
    /// fal published prose that states more than one rate, conditions its rate
    /// on something we cannot evaluate, or names a unit we do not model. Kept
    /// verbatim so the surface can print the sentence instead of a number.
    Unparsed { prose: String },
    /// A single unconditional rate the prose states outright.
    Rate {
        usd: f64,
        unit: PriceUnit,
        prose: String,
    },
}

impl Pricing {
    /// The rate, when there is exactly one and it is a real number.
    ///
    /// Never `Some(0.0)`: [`parse_pricing`] rejects a non-finite or
    /// non-positive figure outright, so there is no path from "we do not know"
    /// to a free-looking Generate button. This is the same rule
    /// [`crate::prices`] enforces and it is enforced twice on purpose.
    pub fn usd(&self) -> Option<f64> {
        match self {
            Pricing::Rate { usd, .. } => Some(*usd),
            _ => None,
        }
    }

    /// The unit the rate is charged in, when there is a rate.
    pub fn unit(&self) -> Option<PriceUnit> {
        match self {
            Pricing::Rate { unit, .. } => Some(*unit),
            _ => None,
        }
    }

    /// fal's own sentence, when there is one.
    pub fn prose(&self) -> Option<&str> {
        match self {
            Pricing::Unpublished => None,
            Pricing::Unparsed { prose } | Pricing::Rate { prose, .. } => Some(prose),
        }
    }

    /// Whether we have a figure to show. False for both "fal said nothing" and
    /// "fal said something we will not reduce".
    pub fn is_known(&self) -> bool {
        matches!(self, Pricing::Rate { .. })
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One row of fal's index.
///
/// Field names are fal's, so a single `Deserialize` reads both the live pages
/// and the bundled snapshot and the snapshot cannot drift into a private
/// dialect of the same data. Everything is defaulted: a row missing a field is
/// a row with less information, not a page we throw away.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    /// fal's endpoint id, e.g. `blackforestlabs/flux-3/text-to-video`. Unique
    /// across the index and the key everything else joins on.
    pub id: String,
    /// fal's display name, e.g. `Flux 3 Text to Video`.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub category: Category,
    #[serde(rename = "shortDescription", default)]
    pub short_description: String,
    /// Marketing prose about price, not data. Read it through
    /// [`Model::pricing`] rather than showing the raw markdown.
    #[serde(rename = "pricingInfoOverride", default)]
    pub pricing_info_override: Option<String>,
    /// fal's own retirement flag. Zero rows carried it on 2026-08-05, which is
    /// exactly why the filter has to exist before one does.
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub removed: bool,
    /// e.g. `Flux 3`. Absent on 589 of 1,418 rows.
    #[serde(rename = "modelFamily", default)]
    pub model_family: Option<String>,
    /// A `https://` still on fal's CDN. Absent rows exist; a card must handle
    /// having no image rather than rendering a broken one.
    #[serde(rename = "thumbnailUrl", default)]
    pub thumbnail_url: Option<String>,
}

impl Model {
    /// Read [`Model::pricing_info_override`].
    ///
    /// Parsed on demand rather than at load: it costs a few string scans and
    /// only a visible card needs it, whereas doing all 1,418 up front would put
    /// the work on the launch path where it delays the first paint.
    pub fn pricing(&self) -> Pricing {
        parse_pricing(self.pricing_info_override.as_deref())
    }

    /// Whether fal has retired this endpoint. Such rows never enter a
    /// [`Catalogue`]; the method exists for callers holding a raw row.
    pub fn is_retired(&self) -> bool {
        self.deprecated || self.removed
    }
}

// ---------------------------------------------------------------------------
// Catalogue
// ---------------------------------------------------------------------------

/// When a set of rows was captured.
///
/// One field rather than a date plus an origin flag, so the two can never
/// disagree about whether you are looking at the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "kebab-case")]
pub enum Captured {
    /// The `snapshot_date` recorded in the bundled file, `YYYY-MM-DD`.
    Snapshot { date: String },
    /// Unix seconds at which a live fetch completed.
    Live { unix_seconds: u64 },
}

impl Captured {
    fn now() -> Captured {
        Captured::Live {
            unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        }
    }

    /// The snapshot's `YYYY-MM-DD`, or `None` for a live fetch — which records
    /// a timestamp instead and should be rendered from that.
    pub fn date(&self) -> Option<&str> {
        match self {
            Captured::Snapshot { date } => Some(date),
            Captured::Live { .. } => None,
        }
    }

    /// Unix seconds for a live fetch, `None` for the bundle.
    pub fn unix_seconds(&self) -> Option<u64> {
        match self {
            Captured::Live { unix_seconds } => Some(*unix_seconds),
            Captured::Snapshot { .. } => None,
        }
    }

    /// Reuses the price feed's vocabulary, because a surface showing both an
    /// old price and an old roster should describe them with the same word.
    pub fn origin(&self) -> Origin {
        match self {
            Captured::Snapshot { .. } => Origin::Snapshot,
            Captured::Live { .. } => Origin::Live,
        }
    }
}

/// Lower-cased text for one model, built once at load.
///
/// A filter box runs `search` on every keystroke. Lower-casing 1,418 titles and
/// descriptions per key is ~280 KB of allocation for each character typed, and
/// it is entirely avoidable.
#[derive(Debug, Clone)]
struct Needle {
    title: String,
    description: String,
}

/// fal's index, filtered and indexed.
#[derive(Debug, Clone)]
pub struct Catalogue {
    models: Vec<Model>,
    needles: Vec<Needle>,
    by_id: BTreeMap<String, usize>,
    by_category: BTreeMap<Category, Vec<usize>>,
    captured: Captured,
    retired: usize,
    unreadable: usize,
}

impl Catalogue {
    /// Build from raw rows, dropping what must not be listed.
    ///
    /// Three drops, all counted rather than silent:
    ///
    /// - **retired** — `deprecated` or `removed`. Offering one is offering a
    ///   generation that 404s after the user has picked a preset and a price.
    /// - **id-less** — a row with no `id` cannot be submitted to, opened, or
    ///   joined to a route, so listing it is offering a dead tile.
    /// - **duplicate id** — first wins. fal's index was duplicate-free on
    ///   2026-08-05; the guard exists so a paging glitch that repeats a page
    ///   cannot double the catalogue.
    pub fn new(rows: Vec<Model>, captured: Captured) -> Catalogue {
        let mut models: Vec<Model> = Vec::with_capacity(rows.len());
        let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
        let mut retired = 0usize;
        let mut unreadable = 0usize;

        for row in rows {
            if row.is_retired() {
                retired += 1;
                continue;
            }
            if row.id.trim().is_empty() {
                unreadable += 1;
                continue;
            }
            if by_id.contains_key(&row.id) {
                continue;
            }
            by_id.insert(row.id.clone(), 0);
            models.push(row);
        }

        // Sorted by id so `all()` is stable across a refresh: an unsorted feed
        // would reshuffle the picker every time the background fetch lands.
        models.sort_by(|a, b| a.id.cmp(&b.id));

        by_id.clear();
        let mut by_category: BTreeMap<Category, Vec<usize>> = BTreeMap::new();
        let mut needles = Vec::with_capacity(models.len());
        for (i, m) in models.iter().enumerate() {
            by_id.insert(m.id.clone(), i);
            by_category.entry(m.category.clone()).or_default().push(i);
            needles.push(Needle {
                title: m.title.to_lowercase(),
                description: m.short_description.to_lowercase(),
            });
        }

        Catalogue {
            models,
            needles,
            by_id,
            by_category,
            captured,
            retired,
            unreadable,
        }
    }

    /// The catalogue compiled into the binary.
    ///
    /// A snapshot that will not parse yields an empty catalogue rather than a
    /// panic: a corrupt vendored file must not stop the app opening, and the
    /// test `the_bundled_snapshot_parses_and_holds_every_model` is what makes
    /// that branch unreachable in a shipped build.
    pub fn bundled() -> Catalogue {
        match serde_json::from_str::<SnapshotFile>(SNAPSHOT) {
            Ok(f) => Catalogue::new(
                f.items,
                Captured::Snapshot {
                    date: f.snapshot_date,
                },
            ),
            Err(e) => {
                tracing::error!(error = %e, "bundled fal catalogue snapshot is unreadable");
                Catalogue::new(
                    Vec::new(),
                    Captured::Snapshot {
                        date: String::new(),
                    },
                )
            }
        }
    }

    /// Every listed model, sorted by id.
    pub fn all(&self) -> &[Model] {
        &self.models
    }

    /// The models fal filed under one category.
    pub fn by_category(&self, category: &Category) -> Vec<&Model> {
        self.by_category
            .get(category)
            .map(|ix| ix.iter().map(|&i| &self.models[i]).collect())
            .unwrap_or_default()
    }

    /// Every category present, with how many models carry it. Ordered by
    /// [`Category`]'s declaration order.
    pub fn categories(&self) -> BTreeMap<Category, usize> {
        self.by_category
            .iter()
            .map(|(c, ix)| (c.clone(), ix.len()))
            .collect()
    }

    /// One model by its exact fal endpoint id.
    ///
    /// Exact, not prefix: `fal-ai/flux` and `fal-ai/flux/dev` are different
    /// endpoints with different prices, and a prefix match would quietly hand
    /// back whichever sorted first.
    pub fn get(&self, id: &str) -> Option<&Model> {
        self.by_id.get(id).map(|&i| &self.models[i])
    }

    /// Free-text search over title and short description.
    ///
    /// Every whitespace-separated term must appear somewhere in the two fields
    /// combined — an AND, so typing more narrows rather than widens. Results
    /// are ranked title-first, because someone typing `kling` wants the Kling
    /// models above the thirty descriptions that mention Kling.
    ///
    /// A blank query is *no filter*, returning everything. Returning nothing
    /// would be the more literal reading and it makes a search box show an
    /// empty catalogue the moment the user clears it.
    pub fn search(&self, query: &str) -> Vec<&Model> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.models.iter().collect();
        }
        let terms: Vec<&str> = q.split_whitespace().collect();

        let mut hits: Vec<(u8, &Model)> = Vec::new();
        for (i, m) in self.models.iter().enumerate() {
            if let Some(rank) = rank_of(&self.needles[i], &q, &terms) {
                hits.push((rank, m));
            }
        }
        hits.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.title.cmp(&b.1.title))
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        hits.into_iter().map(|(_, m)| m).collect()
    }

    /// When these rows were captured, and whether they came from the bundle.
    pub fn captured(&self) -> &Captured {
        &self.captured
    }

    /// How many rows were dropped for being `deprecated` or `removed`.
    pub fn retired(&self) -> usize {
        self.retired
    }

    /// How many rows were dropped for being unusable — no id, or JSON this
    /// module could not read. Non-zero is a signal that fal reshaped something.
    pub fn unreadable(&self) -> usize {
        self.unreadable
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// Where a model matched, smaller is better. `None` means it did not.
fn rank_of(n: &Needle, query: &str, terms: &[&str]) -> Option<u8> {
    let all_terms_present = terms
        .iter()
        .all(|t| n.title.contains(t) || n.description.contains(t));
    if !all_terms_present {
        return None;
    }
    if n.title == query {
        return Some(0);
    }
    if n.title.starts_with(query) {
        return Some(1);
    }
    if n.title.contains(query) {
        return Some(2);
    }
    if terms.iter().all(|t| n.title.contains(t)) {
        return Some(3);
    }
    Some(4)
}

/// The shape of `vendor/fal-catalogue-snapshot.json`.
///
/// `items` is deliberately the same field name and the same row shape as a live
/// page, so refreshing the file is a copy rather than a translation.
#[derive(Debug, Deserialize)]
struct SnapshotFile {
    snapshot_date: String,
    items: Vec<Model>,
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// Why the index did not answer.
///
/// Deliberately not [`crate::prices::FeedError`], though the variants coincide
/// — it is the same host behind the same WAF. What differs is the sentence a
/// user reads: `FeedError`'s checkpoint message ends "showing bundled prices
/// instead", which is the wrong thing to say when what failed was the model
/// list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogueError {
    /// A Vercel WAF challenge. Named apart from a plain 429 because the remedy
    /// differs: a rate limit clears by waiting, a challenge does not clear at
    /// all without solving JavaScript, so retrying is pointless.
    BotCheckpoint {
        url: String,
    },
    Http {
        url: String,
        status: u16,
    },
    Transport {
        url: String,
        msg: String,
    },
    /// The response parsed as JSON but was not an index, or too much of it was
    /// unreadable to trust the part that was.
    Malformed {
        url: String,
        msg: String,
    },
}

impl std::fmt::Display for CatalogueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogueError::BotCheckpoint { url } => write!(
                f,
                "{url} answered with a bot checkpoint, not the model index \
                 — keeping the bundled catalogue"
            ),
            CatalogueError::Http { url, status } => write!(f, "{url} returned HTTP {status}"),
            CatalogueError::Transport { url, msg } => write!(f, "{url} was unreachable: {msg}"),
            CatalogueError::Malformed { url, msg } => {
                write!(f, "{url} sent something unreadable: {msg}")
            }
        }
    }
}

impl std::error::Error for CatalogueError {}

/// One page of fal's index.
///
/// `items` is `Value` rather than `Model` so a single odd row costs one model
/// instead of the whole page — see [`MAX_UNREADABLE_SHARE`] for where that
/// leniency stops.
#[derive(Debug, Deserialize)]
struct Page {
    #[serde(default)]
    items: Vec<serde_json::Value>,
    #[serde(default)]
    pages: u32,
}

/// Fetch the whole index, blocking.
///
/// 36 requests as of 2026-08-05. Call it off the UI thread — or call
/// [`refresh_in_background`], which does that for you.
pub fn fetch() -> Result<Catalogue, CatalogueError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(BROWSER_USER_AGENT)
        .build()
        .map_err(|e| CatalogueError::Transport {
            url: "(http client)".into(),
            msg: e.to_string(),
        })?;

    let mut rows: Vec<Model> = Vec::new();
    let mut unreadable = 0usize;
    let mut page = 1u32;
    let mut pages = 1u32;

    while page <= pages && page <= MAX_PAGES {
        let url = format!("{FAL_MODELS_URL}?page={page}");
        let doc = get_json(&client, &url)?;
        let parsed: Page = serde_json::from_value(doc).map_err(|e| CatalogueError::Malformed {
            url: url.clone(),
            msg: e.to_string(),
        })?;
        if page == 1 {
            pages = parsed.pages.max(1);
        }
        if parsed.items.is_empty() {
            break;
        }
        for item in parsed.items {
            match serde_json::from_value::<Model>(item) {
                Ok(m) => rows.push(m),
                Err(_) => unreadable += 1,
            }
        }
        page += 1;
    }

    if rows.is_empty() {
        return Err(CatalogueError::Malformed {
            url: FAL_MODELS_URL.to_string(),
            msg: "index returned no models".into(),
        });
    }
    let total = rows.len() + unreadable;
    if (unreadable as f64) > (total as f64) * MAX_UNREADABLE_SHARE {
        return Err(CatalogueError::Malformed {
            url: FAL_MODELS_URL.to_string(),
            msg: format!("{unreadable} of {total} rows were unreadable — the index has reshaped"),
        });
    }

    let mut catalogue = Catalogue::new(rows, Captured::now());
    catalogue.unreadable += unreadable;
    Ok(catalogue)
}

fn get_json(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<serde_json::Value, CatalogueError> {
    // The same browser-shaped headers the price feed sends. Measured on
    // 2026-08-05: the JSON API answers a plain request too, and the HTML site
    // answers neither, so this is insurance against fal widening its WAF rule
    // to `/api/*` rather than the thing that makes the request work.
    let resp = client
        .get(url)
        .header("Accept", "application/json,text/html;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Dest", "empty")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "same-origin")
        .send()
        .map_err(|e| CatalogueError::Transport {
            url: url.to_string(),
            msg: e.to_string(),
        })?;

    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.text().map_err(|e| CatalogueError::Transport {
        url: url.to_string(),
        msg: e.to_string(),
    })?;

    if is_bot_checkpoint(status, &headers, &body) {
        return Err(CatalogueError::BotCheckpoint {
            url: url.to_string(),
        });
    }
    if !(200..300).contains(&status) {
        return Err(CatalogueError::Http {
            url: url.to_string(),
            status,
        });
    }
    serde_json::from_str(&body).map_err(|e| CatalogueError::Malformed {
        url: url.to_string(),
        msg: e.to_string(),
    })
}

/// True when a response is a Vercel WAF challenge rather than content.
///
/// Header-first: `x-vercel-mitigated: challenge` is what the edge stamps on a
/// blocked request whatever the body turns out to be. The body check is the
/// backstop — without it a 33 KB challenge page reaches `serde_json` and gets
/// reported as "unreadable", which is the one thing we understand exactly.
fn is_bot_checkpoint(status: u16, headers: &reqwest::header::HeaderMap, body: &str) -> bool {
    if headers
        .get("x-vercel-mitigated")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("challenge"))
        .unwrap_or(false)
    {
        return true;
    }
    if headers.contains_key("x-vercel-challenge-token") {
        return true;
    }
    (status == 429 || status == 403) && body.contains("Vercel Security Checkpoint")
}

// ---------------------------------------------------------------------------
// The process-wide catalogue
// ---------------------------------------------------------------------------

fn cell() -> &'static Mutex<Arc<Catalogue>> {
    static CELL: OnceLock<Mutex<Arc<Catalogue>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(Arc::new(Catalogue::bundled())))
}

/// The catalogue in force: the bundle until a refresh lands, then the live one.
///
/// Cheap — it clones an `Arc`, not 1,418 models — so callers can hold the lock
/// for as short a time as possible and then read at leisure.
///
/// A poisoned mutex is recovered from rather than propagated. Nothing here
/// panics while holding the lock (the critical section is one `Arc::clone`), so
/// poisoning can only arrive from outside, and refusing to list any models
/// because of an unrelated panic elsewhere would be a worse answer than the
/// perfectly intact catalogue sitting behind the lock.
pub fn catalogue() -> Arc<Catalogue> {
    let guard = cell().lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(&guard)
}

/// Replace the catalogue in force. Used by [`refresh`] and by tests.
pub fn install(catalogue: Catalogue) {
    let mut guard = cell().lock().unwrap_or_else(|e| e.into_inner());
    *guard = Arc::new(catalogue);
}

/// Every listed model. Owned, so it can cross the Tauri bridge.
pub fn all() -> Vec<Model> {
    catalogue().all().to_vec()
}

/// Every listed model in one category.
pub fn by_category(category: &Category) -> Vec<Model> {
    catalogue()
        .by_category(category)
        .into_iter()
        .cloned()
        .collect()
}

/// One model by its exact fal endpoint id.
pub fn get(id: &str) -> Option<Model> {
    catalogue().get(id).cloned()
}

/// Free-text search over title and short description. See
/// [`Catalogue::search`] for the ranking and for what a blank query means.
pub fn search(query: &str) -> Vec<Model> {
    catalogue().search(query).into_iter().cloned().collect()
}

/// Fetch the live index and install it. Blocking.
pub fn refresh() -> Result<Arc<Catalogue>, CatalogueError> {
    let fresh = fetch()?;
    install(fresh);
    Ok(catalogue())
}

/// Kick off a refresh on a background thread and return immediately.
///
/// Failure is logged and dropped on purpose: the bundle is already installed
/// and complete, so the honest outcome of a failed refresh is an older
/// catalogue, not an error in the user's face on launch. Call it once at
/// startup.
pub fn refresh_in_background() {
    static RUNNING: AtomicBool = AtomicBool::new(false);

    /// Clears the in-flight flag even if the fetch panics. Without this a
    /// single panic would leave `RUNNING` true for the life of the process and
    /// every later refresh would silently do nothing.
    struct InFlight;
    impl Drop for InFlight {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::SeqCst);
        }
    }

    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let _flag = InFlight;
        match fetch() {
            Ok(c) => {
                tracing::info!(models = c.len(), "refreshed the fal catalogue");
                install(c);
            }
            Err(e) => tracing::warn!(error = %e, "keeping the bundled fal catalogue"),
        }
    });
}

// ---------------------------------------------------------------------------
// pricingInfoOverride
// ---------------------------------------------------------------------------

/// Resolution words. Their presence means the rate is conditional on something
/// the sentence is about to qualify, so no single figure describes it.
///
/// `p video` / `p each` / `p your` catch fal's spaced spelling, as in
/// *"For 720 p each video second will cost 0.05 $"*.
const RESOLUTION_WORDS: [&str; 21] = [
    "360p",
    "480p",
    "512p",
    "540p",
    "576p",
    "580p",
    "720p",
    "848px",
    "1024p",
    "1080p",
    "1440p",
    "2160p",
    "0.5k",
    "1k",
    "2k",
    "4k",
    "hdr",
    "1536x1536",
    "2048x2048",
    "p video",
    "p each",
];

/// Words that mean the sentence is still qualifying its own figure.
///
/// Each was added because a real string needed it. `multipl` covers
/// multiplier/multiplied/multiplies in one; it is what stops
/// *"$0.021 per megapixel. LoRAs over 2GB … multiply the price by 1.5x"* from
/// being read as a flat $0.021.
const CONDITIONAL_WORDS: [&str; 33] = [
    "turbo",
    "balanced",
    "quality",
    "pro mode",
    "audio on",
    "audio off",
    "with audio",
    "without audio",
    "textures",
    "vector style",
    "lowpoly",
    "geometry",
    "multi-view",
    "pbr",
    "quad",
    "multipl",
    "surcharge",
    "discount",
    "candidate",
    "sample",
    "token",
    "tier",
    "if you",
    "when enabled",
    "if enabled",
    "adds ",
    "plus ",
    "additional",
    "extra",
    "first megapixel",
    "subsequent",
    "free",
    "minimum",
];

/// Sentence openings that restate a rate rather than adding one, e.g.
/// *"For $1 you can run this model approximately 9 times."* Left in, they make
/// every one-rate sentence look like a two-rate sentence and nothing parses.
const ILLUSTRATION_OPENINGS: [&str; 6] = [
    "for example",
    "for instance",
    "e.g.",
    "for $1",
    "for 1$",
    "with 1000 steps",
];

/// Phrases that mark a whole sentence as a restatement wherever they appear.
const ILLUSTRATION_PHRASES: [&str; 4] = [
    "you can run this model",
    "you can generate",
    "you can run generate",
    "you can fine-tune",
];

/// Anchor phrases mapped to the unit they name, checked in order.
///
/// Order matters: `per megapixel of generated video data` has to be tried
/// before `per megapixel`, or 61 video models get priced as if they billed per
/// image megapixel — a factor of roughly a hundred on a five-second clip.
const UNIT_ANCHORS: [(&str, PriceUnit); 26] = [
    (
        "per megapixel of generated video data",
        PriceUnit::VideoMegapixel,
    ),
    (
        "per megapixel of generated hdr video data",
        PriceUnit::VideoMegapixel,
    ),
    ("per megapixel of video data", PriceUnit::VideoMegapixel),
    (
        "per megapixel of input and output",
        PriceUnit::InputAndOutputMegapixel,
    ),
    (
        "per megapixel on both input and output",
        PriceUnit::InputAndOutputMegapixel,
    ),
    ("per second of generated video", PriceUnit::VideoSecond),
    (
        "per second of generated output video",
        PriceUnit::VideoSecond,
    ),
    ("per second of output video", PriceUnit::VideoSecond),
    ("per output video second", PriceUnit::VideoSecond),
    ("per video second", PriceUnit::VideoSecond),
    ("per generated second of video", PriceUnit::VideoSecond),
    ("per generated second", PriceUnit::VideoSecond),
    ("per second of video", PriceUnit::VideoSecond),
    ("per second of input video", PriceUnit::InputVideoSecond),
    ("per second of generated audio", PriceUnit::AudioSecond),
    ("per second of output audio", PriceUnit::AudioSecond),
    ("per generated audio second", PriceUnit::AudioSecond),
    ("per audio second", PriceUnit::AudioSecond),
    ("per generated image", PriceUnit::Image),
    ("per image generated", PriceUnit::Image),
    ("per image", PriceUnit::Image),
    ("each image costs", PriceUnit::Image),
    ("per compute second", PriceUnit::ComputeSecond),
    ("per output megapixel", PriceUnit::Megapixel),
    ("per megapixel", PriceUnit::Megapixel),
    ("per step", PriceUnit::TrainingStep),
];

/// Anchors for units that are only meaningful once the more specific tables
/// above have missed. Split out purely so the array above stays under the
/// specificity ordering it depends on.
const LOOSE_UNIT_ANCHORS: [(&str, PriceUnit); 7] = [
    ("per video generation", PriceUnit::Generation),
    ("per generation", PriceUnit::Generation),
    ("per request", PriceUnit::Generation),
    ("per video", PriceUnit::Generation),
    ("per generated minute", PriceUnit::Minute),
    ("per minute", PriceUnit::Minute),
    ("per 1000 characters", PriceUnit::ThousandCharacters),
];

/// Read a `pricingInfoOverride`, or refuse.
///
/// The rule is: after removing sentences that only restate a figure, the prose
/// must name **exactly one** dollar amount, must not condition it on anything
/// (resolution, mode, audio, an add-on), and must name a unit this module
/// models. Anything else is [`Pricing::Unparsed`] with the sentence kept.
///
/// Measured against all 1,418 rows on 2026-08-05: 765 publish nothing, 184
/// reduce to a rate and 469 stay prose. That 72% refusal rate is the feature.
/// fal writes conditional rate tables as English sentences —
/// *"$0.15 for 360p and 540p, $0.2 for 720p and $0.4 for 1080p"* — and the only
/// numbers a parser can take from those are the wrong ones. The prose is
/// returned so a surface can print the sentence, which is strictly more useful
/// than an authoritative-looking figure that is off by 2.7×.
pub fn parse_pricing(raw: Option<&str>) -> Pricing {
    let Some(raw) = raw else {
        return Pricing::Unpublished;
    };
    let prose = raw.trim();
    if prose.is_empty() {
        return Pricing::Unpublished;
    }
    let unparsed = || Pricing::Unparsed {
        prose: prose.to_string(),
    };

    let stated = strip_illustrations(prose);
    let low = stated.to_lowercase();

    // Training is the one template whose rate is not written with a dollar
    // sign: "The formula is: 0.0043 * steps." Checked first, because the only
    // `$` figure in those sentences is the illustration.
    // Deliberately given the *unstripped* prose as well. The worked example
    // ("With 1000 steps, your request will cost $43") is itself an
    // illustration, so it is gone from `low` — and it is exactly what the
    // cross-check needs. Reading only the stripped text meant the check had
    // nothing to compare against and silently passed, which is how an upstream
    // typo would have shipped as an authoritative rate.
    let low_full = prose.to_lowercase();
    if let Some(usd) = training_rate(&low, &low_full) {
        return Pricing::Rate {
            usd,
            unit: PriceUnit::TrainingStep,
            prose: prose.to_string(),
        };
    }
    if low.contains("* steps") || low.contains("*steps") {
        // A step formula we did not recognise — a reference multiplier, or a
        // coefficient that disagrees with its own worked example. Refuse rather
        // than fall through to the dollar figure, which is the cost of 1,000
        // steps and would be read as the cost of one.
        return unparsed();
    }

    let mut figures = money(&stated);
    figures.sort_by(f64::total_cmp);
    figures.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
    if figures.len() != 1 {
        return unparsed();
    }
    let usd = figures[0];
    if !usd.is_finite() || usd <= 0.0 {
        return unparsed();
    }

    if RESOLUTION_WORDS.iter().any(|w| low.contains(w)) {
        return unparsed();
    }
    if CONDITIONAL_WORDS.iter().any(|w| low.contains(w)) {
        return unparsed();
    }
    match unit_of(&low) {
        Some(unit) => Pricing::Rate {
            usd,
            unit,
            prose: prose.to_string(),
        },
        None => unparsed(),
    }
}

/// `The cost of training depends on the number of steps. The formula is:
/// 0.0043 * steps. With 1000 steps, your request will cost $4.3.`
///
/// The coefficient is cross-checked against fal's own worked example before it
/// is believed. That check is not theatre: it is the difference between
/// shipping fal's typo and noticing it, and it costs one multiplication.
fn training_rate(low: &str, low_full: &str) -> Option<f64> {
    let at = low.find("formula is:")?;
    let rest = &low[at + "formula is:".len()..];
    let (usd, end) = leading_number(rest.trim_start())?;
    if !usd.is_finite() || usd <= 0.0 {
        return None;
    }
    // The formula must be exactly `<n> * steps`, with nothing multiplied onto
    // it. `0.002 * steps * reference_multiplier` is a rate we cannot evaluate.
    let tail = rest.trim_start()[end..].trim_start();
    let tail = tail.strip_prefix('*')?.trim_start();
    let tail = tail.strip_prefix("steps")?;
    if !tail.starts_with('.') && !tail.starts_with('\n') && !tail.is_empty() {
        return None;
    }

    // Cross-check against "With N steps, your request will cost $M." — read
    // from the full prose, because this sentence is an illustration and has
    // been stripped from `low`.
    if let Some(w) = low_full.find("with ") {
        let after = &low_full[w + "with ".len()..];
        if let Some((steps, _)) = leading_number(after) {
            if after
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
                .trim_start()
                .starts_with("steps")
            {
                let quoted = money(&low_full[w..]);
                if let Some(total) = quoted.first() {
                    if (usd * steps - total).abs() > (total * 0.01).max(1e-9) {
                        return None;
                    }
                }
            }
        }
    }
    Some(usd)
}

/// The first unit anchor the prose names, or `None`.
fn unit_of(low: &str) -> Option<PriceUnit> {
    for (anchor, unit) in UNIT_ANCHORS.iter().chain(LOOSE_UNIT_ANCHORS.iter()) {
        if let Some(at) = low.find(anchor) {
            // `per megapixel per second` is a compound unit, not a per-megapixel
            // rate: reading it as one drops the duration entirely and prices a
            // ten-second upscale as if it were one frame.
            let tail = low[at + anchor.len()..].trim_start_matches(['*', ' ']);
            if tail.starts_with("per ") {
                continue;
            }
            return Some(*unit);
        }
    }
    None
}

/// Drop sentences that restate a figure instead of adding one.
fn strip_illustrations(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for sentence in sentences(text) {
        let low: String = sentence
            .trim()
            .to_lowercase()
            .chars()
            .filter(|c| *c != '*')
            .collect();
        if ILLUSTRATION_OPENINGS.iter().any(|o| low.starts_with(o)) {
            continue;
        }
        if ILLUSTRATION_PHRASES.iter().any(|p| low.contains(p)) {
            continue;
        }
        kept.push(sentence);
    }
    kept.join(" ")
}

/// Split on `.` or newline followed by whitespace.
///
/// Byte indexing is safe here: `.`, `\n` and ASCII whitespace are single-byte
/// and never appear inside a multi-byte character, so every cut lands on a
/// character boundary even though the prose is full of `×` and `≈`.
fn sentences(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'.' || b[i] == b'\n' {
            let mut j = i + 1;
            if j < b.len() && b[j].is_ascii_whitespace() {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                out.push(&text[start..=i]);
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Every dollar amount in the text.
///
/// Hand-rolled rather than a regex because this crate carries no regex
/// dependency and the grammar is four characters wide. fal writes the sign on
/// either side and sprinkles markdown emphasis between the two: `$0.05`,
/// `**$0.05**`, `$**0.04**`, `0.07 $` and `0.1$` all occur in the live index.
/// A bare number with no `$` anywhere near it is not money — that is what keeps
/// `1024x1024`, `121 frames` and `30 FPS` out of the figure count.
fn money(text: &str) -> Vec<f64> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let mut end = i;
        if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            end = i;
        }
        if !(dollar_before(b, start) || dollar_after(b, end)) {
            continue;
        }
        if let Ok(v) = text[start..end].parse::<f64>() {
            out.push(v);
        }
    }
    out
}

/// `$`, skipping markdown emphasis and at most one space.
///
/// One space, not any run: an unbounded skip turns `**5** **$0.35**` into two
/// readings of $0.35 and $5, and the extra figure makes a parseable sentence
/// look conditional.
fn dollar_before(b: &[u8], start: usize) -> bool {
    let mut k = start;
    while k > 0 && b[k - 1] == b'*' {
        k -= 1;
    }
    if k > 0 && b[k - 1] == b' ' {
        k -= 1;
    }
    while k > 0 && b[k - 1] == b'*' {
        k -= 1;
    }
    k > 0 && b[k - 1] == b'$'
}

fn dollar_after(b: &[u8], end: usize) -> bool {
    let mut k = end;
    while k < b.len() && b[k] == b'*' {
        k += 1;
    }
    if k < b.len() && b[k] == b' ' {
        k += 1;
    }
    while k < b.len() && b[k] == b'*' {
        k += 1;
    }
    k < b.len() && b[k] == b'$'
}

/// A bare decimal at the head of the string, and how many bytes it took.
fn leading_number(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let mut i = 0usize;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == 0 {
        return None;
    }
    s[..i].parse::<f64>().ok().map(|v| (v, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Counts measured against the live index on 2026-08-05 and frozen into
    // vendor/fal-catalogue-snapshot.json. A refresh that changes them should
    // fail here first.
    const SNAPSHOT_MODELS: usize = 1418;
    const SNAPSHOT_DATE: &str = "2026-08-05";
    const SNAPSHOT_PARSED_RATES: usize = 184;
    const SNAPSHOT_UNPUBLISHED: usize = 765;
    const SNAPSHOT_UNPARSED: usize = 469;

    fn row(id: &str, title: &str, category: &str, description: &str) -> Model {
        Model {
            id: id.into(),
            title: title.into(),
            category: Category::from_wire(category),
            short_description: description.into(),
            pricing_info_override: None,
            deprecated: false,
            removed: false,
            model_family: None,
            thumbnail_url: None,
        }
    }

    // -- the bundle ---------------------------------------------------------

    #[test]
    fn the_bundled_snapshot_parses_and_holds_every_model() {
        // Catalogue::bundled() swallows a parse failure so the app still opens.
        // This is the test that makes that branch unreachable in a shipped
        // build; without it a corrupt vendored file ships as an empty picker.
        let c = Catalogue::bundled();
        assert_eq!(c.len(), SNAPSHOT_MODELS);
        assert_eq!(c.unreadable(), 0);
    }

    #[test]
    fn the_bundled_snapshot_records_the_date_it_was_captured() {
        // A roster from last month is only honest if it says so.
        let c = Catalogue::bundled();
        assert_eq!(c.captured().date(), Some(SNAPSHOT_DATE));
        assert_eq!(c.captured().origin(), Origin::Snapshot);
        assert_eq!(c.captured().unix_seconds(), None);
    }

    #[test]
    fn every_category_in_the_bundle_is_one_this_enum_names() {
        // Other(_) is the safety net, not the destination. A hit here means fal
        // added a category and Category should learn it in the same commit as
        // the snapshot refresh.
        let c = Catalogue::bundled();
        let unnamed: Vec<String> = c
            .categories()
            .keys()
            .filter(|cat| !cat.is_named())
            .map(|cat| cat.to_string())
            .collect();
        assert!(unnamed.is_empty(), "unnamed categories: {unnamed:?}");
        assert_eq!(c.categories().len(), 26);
    }

    #[test]
    fn the_bundled_category_counts_are_the_ones_that_were_measured() {
        let c = Catalogue::bundled();
        let counts = c.categories();
        for (wire, want) in [
            ("image-to-image", 385),
            ("text-to-image", 195),
            ("image-to-video", 194),
            ("video-to-video", 190),
            ("text-to-video", 127),
            ("training", 53),
            ("3d-to-3d", 7),
            ("workflow", 1),
            ("unknown", 2),
        ] {
            assert_eq!(
                counts.get(&Category::from_wire(wire)).copied(),
                Some(want),
                "{wire}"
            );
        }
        assert_eq!(counts.values().sum::<usize>(), SNAPSHOT_MODELS);
    }

    #[test]
    fn every_bundled_model_carries_an_id_a_title_and_a_category() {
        // These four fields are what a picker tile is made of. A row missing
        // one renders as a blank clickable rectangle.
        for m in Catalogue::bundled().all() {
            assert!(!m.id.trim().is_empty());
            assert!(!m.title.trim().is_empty(), "{}", m.id);
            assert!(m.category.is_named(), "{}", m.id);
            assert!(!m.is_retired(), "{}", m.id);
        }
    }

    #[test]
    fn the_bundle_prices_exactly_what_was_measured_and_refuses_the_rest() {
        // The 72% refusal rate is the feature, not a gap: fal writes
        // conditional rate tables as English and the only numbers a parser can
        // take from those are the wrong ones. If this count jumps, the parser
        // got looser, not smarter.
        let c = Catalogue::bundled();
        let mut rates = 0;
        let mut unpublished = 0;
        let mut unparsed = 0;
        for m in c.all() {
            match m.pricing() {
                Pricing::Rate { .. } => rates += 1,
                Pricing::Unpublished => unpublished += 1,
                Pricing::Unparsed { .. } => unparsed += 1,
            }
        }
        assert_eq!(rates, SNAPSHOT_PARSED_RATES);
        assert_eq!(unpublished, SNAPSHOT_UNPUBLISHED);
        assert_eq!(unparsed, SNAPSHOT_UNPARSED);
    }

    #[test]
    fn no_price_in_the_bundle_is_zero_or_nonsense() {
        // The figure goes on the Generate button. A zero there reads as free.
        for m in Catalogue::bundled().all() {
            if let Some(usd) = m.pricing().usd() {
                assert!(usd.is_finite() && usd > 0.0, "{} priced {usd}", m.id);
            }
        }
    }

    #[test]
    fn unparsed_pricing_keeps_falss_own_sentence() {
        // "We could not parse this" and "fal said nothing" are different
        // answers and only one of them means there is nothing to show.
        let c = Catalogue::bundled();
        let mut seen = 0;
        for m in c.all() {
            if let Pricing::Unparsed { prose } = m.pricing() {
                // Trimmed on both sides: the property is that fal's own
                // sentence survived, not that its trailing newline did. A
                // stored string goes straight into the UI, where trailing
                // whitespace is noise.
                assert_eq!(
                    Some(prose.trim()),
                    m.pricing_info_override.as_deref().map(str::trim)
                );
                seen += 1;
            }
        }
        assert_eq!(seen, SNAPSHOT_UNPARSED);
    }

    // -- filtering ----------------------------------------------------------

    #[test]
    fn a_deprecated_model_is_never_listed() {
        // Offering one is offering a generation that 404s after the user has
        // picked a preset and read a price. fal carried zero on the capture
        // date, which is exactly why the filter has to exist before one does.
        let mut dead = row("x/dead", "Dead", "text-to-image", "");
        dead.deprecated = true;
        let c = Catalogue::new(
            vec![dead, row("x/live", "Live", "text-to-image", "")],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c.retired(), 1);
        assert!(c.get("x/dead").is_none());
    }

    #[test]
    fn a_removed_model_is_never_listed() {
        let mut gone = row("x/gone", "Gone", "text-to-image", "");
        gone.removed = true;
        let c = Catalogue::new(vec![gone], Captured::Snapshot { date: "t".into() });
        assert!(c.is_empty());
        assert_eq!(c.retired(), 1);
    }

    #[test]
    fn a_row_with_no_id_is_dropped_and_counted() {
        // It cannot be submitted to, opened, or joined to a route, so listing
        // it is offering a dead tile. Counted so a reshaped feed is visible.
        let c = Catalogue::new(
            vec![row("", "Nameless", "vision", "")],
            Captured::Snapshot { date: "t".into() },
        );
        assert!(c.is_empty());
        assert_eq!(c.unreadable(), 1);
    }

    #[test]
    fn a_repeated_page_cannot_double_the_catalogue() {
        let a = row("x/one", "One", "vision", "");
        let c = Catalogue::new(
            vec![a.clone(), a.clone(), a],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn models_come_back_in_a_stable_order() {
        // An unsorted feed would reshuffle the picker every time the background
        // refresh lands, under whatever the user was pointing at.
        let c = Catalogue::new(
            vec![
                row("z/last", "Z", "vision", ""),
                row("a/first", "A", "vision", ""),
            ],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(
            c.all().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a/first", "z/last"]
        );
    }

    // -- categories ---------------------------------------------------------

    #[test]
    fn an_unrecognised_category_keeps_the_model_and_the_string() {
        // The whole reason Other exists: fal adds categories without warning
        // and a closed enum would either fail the page or lose the model.
        let c = Catalogue::new(
            vec![row("x/new", "New", "text-to-hologram", "")],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(c.len(), 1);
        let cat = &c.all()[0].category;
        assert_eq!(cat, &Category::Other("text-to-hologram".into()));
        assert!(!cat.is_named());
        assert_eq!(cat.to_string(), "text-to-hologram");
        assert_eq!(c.by_category(cat).len(), 1);
    }

    #[test]
    fn a_missing_category_is_not_confused_with_fals_own_unknown() {
        // fal publishes a literal "unknown" on two live models. Defaulting an
        // absent field to it would hide a reshaped feed inside real data.
        let absent: Model = serde_json::from_str(r#"{"id":"x/y"}"#).unwrap();
        assert_eq!(absent.category, Category::Other(String::new()));
        assert_ne!(absent.category, Category::Unknown);
    }

    #[test]
    fn a_category_round_trips_through_fals_spelling() {
        for c in Category::named() {
            assert_eq!(Category::from_wire(c.as_wire()), c, "{c}");
        }
        assert_eq!(Category::from_wire("3d-to-3d"), Category::ThreeDTo3d);
        assert_eq!(Category::from_wire("IMAGE-TO-3D"), Category::ImageTo3d);
    }

    #[test]
    fn by_category_returns_only_that_category() {
        let c = Catalogue::bundled();
        let v = c.by_category(&Category::TextTo3d);
        assert_eq!(v.len(), 11);
        assert!(v.iter().all(|m| m.category == Category::TextTo3d));
    }

    // -- lookup and search --------------------------------------------------

    #[test]
    fn get_is_exact_and_never_a_prefix_match() {
        // `fal-ai/flux` and `fal-ai/flux/dev` are different endpoints with
        // different prices; a prefix match hands back whichever sorted first.
        let c = Catalogue::new(
            vec![
                row("fal-ai/flux/dev", "Flux Dev", "text-to-image", ""),
                row("fal-ai/flux/schnell", "Flux Schnell", "text-to-image", ""),
            ],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(
            c.get("fal-ai/flux/dev").map(|m| m.title.as_str()),
            Some("Flux Dev")
        );
        assert!(c.get("fal-ai/flux").is_none());
        assert!(c.get("fal-ai/flux/").is_none());
    }

    #[test]
    fn an_empty_search_is_no_filter_rather_than_no_results() {
        // A filter box that empties the catalogue the moment the user clears it
        // reads as a broken app.
        let c = Catalogue::bundled();
        assert_eq!(c.search("").len(), c.len());
        assert_eq!(c.search("   ").len(), c.len());
    }

    #[test]
    fn search_matches_the_description_as_well_as_the_title() {
        let c = Catalogue::new(
            vec![
                row("x/a", "Alpha", "vision", "counts the pelicans"),
                row("x/b", "Beta", "vision", "unrelated"),
            ],
            Captured::Snapshot { date: "t".into() },
        );
        let hits = c.search("pelicans");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "x/a");
    }

    #[test]
    fn search_ranks_a_title_hit_above_a_description_hit() {
        // Someone typing "kling" wants the Kling models, not the thirty
        // descriptions that mention Kling in passing.
        let c = Catalogue::new(
            vec![
                row(
                    "x/mentions",
                    "Something Else",
                    "vision",
                    "works well with kling",
                ),
                row("x/is", "Kling 2.5", "vision", ""),
            ],
            Captured::Snapshot { date: "t".into() },
        );
        let hits = c.search("kling");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "x/is");
    }

    #[test]
    fn every_search_term_must_match_so_typing_more_narrows() {
        let c = Catalogue::new(
            vec![
                row("x/a", "Kling Standard", "vision", ""),
                row("x/b", "Kling Pro", "vision", ""),
            ],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(c.search("kling").len(), 2);
        assert_eq!(c.search("kling pro").len(), 1);
        assert_eq!(c.search("kling nonexistent").len(), 0);
    }

    #[test]
    fn search_is_case_insensitive_in_both_directions() {
        let c = Catalogue::new(
            vec![row("x/a", "SeeDance PRO", "vision", "")],
            Captured::Snapshot { date: "t".into() },
        );
        assert_eq!(c.search("seedance").len(), 1);
        assert_eq!(c.search("PRO").len(), 1);
    }

    #[test]
    fn search_over_the_real_bundle_finds_a_known_family() {
        let c = Catalogue::bundled();
        let hits = c.search("flux 3");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|m| {
            let hay = format!("{} {}", m.title, m.short_description).to_lowercase();
            hay.contains("flux") && hay.contains('3')
        }));
    }

    // -- pricing prose ------------------------------------------------------

    #[test]
    fn a_model_with_no_pricing_prose_is_unpublished_not_free() {
        assert_eq!(parse_pricing(None), Pricing::Unpublished);
        assert_eq!(parse_pricing(Some("   ")), Pricing::Unpublished);
        assert_eq!(parse_pricing(None).usd(), None);
        assert!(!parse_pricing(None).is_known());
    }

    #[test]
    fn a_flat_per_image_rate_is_read() {
        let p = parse_pricing(Some("Your request will cost **$0.05** per image."));
        assert_eq!(p.usd(), Some(0.05));
        assert_eq!(p.unit(), Some(PriceUnit::Image));
    }

    #[test]
    fn an_illustration_sentence_is_not_mistaken_for_a_second_rate() {
        // Without stripping it, every one-rate sentence looks like two and
        // nothing parses. Verbatim from fal's index on 2026-08-05.
        let p = parse_pricing(Some(
            "Your request will cost **$0.039** per image. \
             For **$1.00**, you can run this model **25 times.**",
        ));
        assert_eq!(p.usd(), Some(0.039));
    }

    #[test]
    fn a_resolution_tiered_price_is_refused_not_reduced_to_its_first_figure() {
        // Verbatim. Taking $0.15 here understates 1080p by 2.7x, and the number
        // goes on the Generate button.
        let raw = "For 5s video your request will cost **$0.15** for 360p and 540p, \
                   **$0.2** for 720p and **$0.4** for 1080p.";
        let p = parse_pricing(Some(raw));
        assert_eq!(p.usd(), None);
        assert_eq!(p.prose(), Some(raw));
    }

    #[test]
    fn an_audio_conditioned_price_is_refused() {
        let p = parse_pricing(Some(
            "For every second of video you generated, you will be charged **$0.112** \
             (audio off) or **$0.14** (audio on).",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_mode_conditioned_price_is_refused() {
        let p = parse_pricing(Some(
            "Your request will cost **$0.03** with TURBO, **$0.06** with BALANCED, \
             and **$0.09** with QUALITY.",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_rate_with_an_add_on_is_refused() {
        // "$0.102 plus $0.003 per reference image" has no single rate, and the
        // add-on is exactly what an edit with references will actually cost.
        let p = parse_pricing(Some(
            "Each edit costs **$0.102** plus **$0.003** per reference/source image.",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_lora_size_multiplier_is_refused_even_though_only_one_figure_is_quoted() {
        // One dollar figure and a clean "per megapixel" ending, so only the
        // multiplier guard catches it.
        let p = parse_pricing(Some(
            "Your request will cost **$0.021** per megapixel. LoRAs over 2GB in size \
             will accrue an extra 50% charge per GB (2-3GB total lora size will \
             multiply the price by 1.5x).",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_video_megapixel_rate_is_not_read_as_an_image_megapixel_rate() {
        // 61 models write it this way. Matching the shorter "per megapixel"
        // anchor first would price a five-second clip like a single frame.
        let p = parse_pricing(Some(
            "Your request will cost $0.0024075 per megapixel of generated video data \
             (width × height × frames), rounded up. For example, if you generate a video \
             that is 121 frames long at 1280 × 720, your total generated video is ≈112 MP, \
             and your request will cost $0.2696.",
        ));
        assert_eq!(p.usd(), Some(0.0024075));
        assert_eq!(p.unit(), Some(PriceUnit::VideoMegapixel));
    }

    #[test]
    fn a_compound_unit_is_not_read_as_its_first_half() {
        // "per megapixel per second": reading it as per-megapixel drops the
        // duration entirely.
        let p = parse_pricing(Some("Pricing is **$0.10 per megapixel per second**."));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn input_and_output_megapixels_are_labelled_as_a_different_unit() {
        // Eleven models bill on input plus output. Rendering that identically
        // to a per-output-megapixel rate understates an edit with a large
        // reference image by exactly the size of the reference.
        let p = parse_pricing(Some(
            "Requests cost **$0.012** per megapixel of input and output. \
             Input images will be resized to 1MP.",
        ));
        assert_eq!(p.usd(), Some(0.012));
        assert_eq!(p.unit(), Some(PriceUnit::InputAndOutputMegapixel));
    }

    #[test]
    fn a_ten_second_audio_bucket_is_not_read_as_a_per_second_rate() {
        // "$0.01 per 10 second of generated audio" is not "$0.01 per second".
        // The anchors are exact phrases precisely so this misses.
        let p = parse_pricing(Some(
            "Your request will cost **$0.01** per 10 second of generated audio.",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_training_formula_is_read_from_its_coefficient_not_its_illustration() {
        // The only $ figure in the sentence is the cost of 1,000 steps. Reading
        // it as the rate overstates training by a factor of a thousand.
        let p = parse_pricing(Some(
            "The cost of training depends on the number of steps. \
             The formula is: 0.0043 * steps. With 1000 steps, your request will cost **$4.3**.",
        ));
        assert_eq!(p.usd(), Some(0.0043));
        assert_eq!(p.unit(), Some(PriceUnit::TrainingStep));
    }

    #[test]
    fn a_training_formula_with_a_reference_multiplier_is_refused() {
        let p = parse_pricing(Some(
            "The cost of training depends on the number of steps and the reference images. \
             The formula is: 0.002 * steps * reference_multiplier. The reference multiplier \
             for 1, 2, 3 and 4 images is 2.11, 3.44, 5.09, and 6.95 respectively.",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_training_formula_that_contradicts_its_own_example_is_refused() {
        // Guards against shipping an upstream typo as an authoritative number.
        let p = parse_pricing(Some(
            "The cost of training depends on the number of steps. \
             The formula is: 0.0043 * steps. With 1000 steps, your request will cost **$43**.",
        ));
        assert_eq!(p.usd(), None);
    }

    #[test]
    fn a_price_written_with_a_trailing_dollar_sign_is_still_read() {
        // fal writes it both ways, sometimes with markdown between the two.
        let p = parse_pricing(Some("Your request will cost 0.07 $ per video."));
        assert_eq!(p.usd(), Some(0.07));
        assert_eq!(p.unit(), Some(PriceUnit::Generation));
        let q = parse_pricing(Some(
            "Your request will cost **0.1$** per output video second.",
        ));
        assert_eq!(q.usd(), Some(0.1));
    }

    #[test]
    fn a_bare_number_next_to_no_dollar_sign_is_not_money() {
        // "1280x704", "93 frames", "16 frames per second" all sit in the same
        // sentence as the one real figure. Counting any of them makes the
        // sentence look conditional and the model loses its price.
        let p = parse_pricing(Some(
            "Your request will cost $0.20 per video. Videos have a fixed size of 1280x704 \
             and a fixed duration of 93 frames at 16 frames per second (5.8 seconds.)",
        ));
        assert_eq!(p.usd(), Some(0.20));
        assert_eq!(p.unit(), Some(PriceUnit::Generation));
    }

    #[test]
    fn a_token_billed_model_is_refused_rather_than_priced() {
        // GPT-Image-shaped billing. There is no per-request number to show and
        // inventing one is the failure this module exists to avoid.
        assert_eq!(
            parse_pricing(Some(
                "You will be charged based on the number of input and output tokens."
            ))
            .usd(),
            None
        );
        assert_eq!(
            parse_pricing(Some(
                "Your request will cost $0.4 per million input tokens, and $3.5 per million \
                 output tokens."
            ))
            .usd(),
            None
        );
    }

    #[test]
    fn a_negative_or_zero_figure_never_becomes_a_rate() {
        assert_eq!(
            parse_pricing(Some("Your request will cost $0 per image.")).usd(),
            None
        );
        assert_eq!(
            parse_pricing(Some("Your request will cost $0.00 per image.")).usd(),
            None
        );
    }

    #[test]
    fn prose_with_multibyte_characters_does_not_panic_the_sentence_splitter() {
        // The live index is full of ×, ≈ and curly quotes; byte-indexed cuts
        // have to land on character boundaries.
        let p = parse_pricing(Some(
            "Your request will cost $0.0024075 per megapixel of generated video data \
             (width × height × frames). For example, ≈112 MP costs $0.2696. It’s rounded up.",
        ));
        assert_eq!(p.usd(), Some(0.0024075));
    }

    // -- fetch plumbing -----------------------------------------------------

    #[test]
    fn a_page_where_most_rows_are_unreadable_is_a_refusal_not_a_shrunken_catalogue() {
        // Keeping the 40% that still parse would hand the picker a catalogue
        // quietly missing most of fal, with no error anywhere.
        let share = MAX_UNREADABLE_SHARE;
        assert!(share > 0.0 && share < 0.5);
        let unreadable = 60usize;
        let total = 100usize;
        assert!((unreadable as f64) > (total as f64) * share);
    }

    #[test]
    fn a_single_odd_row_costs_one_model_not_the_page() {
        // Deserialised per row, so a type change on one entry does not freeze
        // the whole catalogue at the snapshot until we ship an update.
        let good: Result<Model, _> =
            serde_json::from_value(serde_json::json!({"id": "x/a", "title": "A"}));
        let bad: Result<Model, _> =
            serde_json::from_value(serde_json::json!({"id": 7, "title": "A"}));
        assert!(good.is_ok());
        assert!(bad.is_err());
    }

    #[test]
    fn a_row_deserialises_from_fals_own_field_names() {
        // The snapshot and the live pages must be read by the same impl, or the
        // vendored file drifts into a private dialect of the same data.
        let m: Model = serde_json::from_str(
            r#"{"id":"a/b","title":"T","category":"text-to-video",
                "shortDescription":"d","pricingInfoOverride":"p",
                "modelFamily":"F","thumbnailUrl":"https://x/y.jpg","deprecated":false}"#,
        )
        .unwrap();
        assert_eq!(m.category, Category::TextToVideo);
        assert_eq!(m.short_description, "d");
        assert_eq!(m.model_family.as_deref(), Some("F"));
        assert_eq!(m.thumbnail_url.as_deref(), Some("https://x/y.jpg"));
        assert_eq!(m.pricing_info_override.as_deref(), Some("p"));
    }

    #[test]
    fn a_bot_checkpoint_is_recognised_as_itself() {
        // Otherwise a 33 KB challenge page reaches serde_json and is reported
        // as "unreadable" — the one failure we understand exactly.
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("x-vercel-mitigated", "challenge".parse().unwrap());
        assert!(is_bot_checkpoint(200, &h, ""));

        let empty = reqwest::header::HeaderMap::new();
        assert!(is_bot_checkpoint(
            429,
            &empty,
            "… Vercel Security Checkpoint …"
        ));
        assert!(!is_bot_checkpoint(200, &empty, r#"{"items":[]}"#));
    }

    #[test]
    fn a_live_fetch_records_a_timestamp_rather_than_a_snapshot_date() {
        let c = Catalogue::new(vec![row("x/a", "A", "vision", "")], Captured::now());
        assert_eq!(c.captured().origin(), Origin::Live);
        assert!(c.captured().date().is_none());
        assert!(c.captured().unix_seconds().unwrap_or(0) > 1_700_000_000);
    }

    // -- the process-wide catalogue ----------------------------------------
    //
    // One test owns the global. Splitting it would let the two race: Rust runs
    // tests in parallel threads inside a single binary, so a second test
    // reading `catalogue()` could observe whatever this one installed.

    #[test]
    fn the_free_functions_read_the_installed_catalogue() {
        assert_eq!(all().len(), SNAPSHOT_MODELS);
        assert!(get("blackforestlabs/flux-3/text-to-video").is_some());
        assert!(!by_category(&Category::TextToVideo).is_empty());
        assert_eq!(search("").len(), SNAPSHOT_MODELS);

        install(Catalogue::new(
            vec![row("x/only", "Only", "vision", "")],
            Captured::now(),
        ));
        assert_eq!(all().len(), 1);
        assert_eq!(get("x/only").map(|m| m.title), Some("Only".to_string()));
        assert!(get("blackforestlabs/flux-3/text-to-video").is_none());
        assert_eq!(search("only").len(), 1);
        assert_eq!(catalogue().captured().origin(), Origin::Live);

        install(Catalogue::bundled());
        assert_eq!(all().len(), SNAPSHOT_MODELS);
    }
}
