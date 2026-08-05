//! Runtime price feed.
//!
//! The plan says prices are fetched at runtime, never hardcoded.
//! [`crate::registry`] still carries hand-transcribed USD figures; this module
//! is the feed that supersedes them, and the two are deliberately independent
//! so a wrong transcription shows up as a disagreement rather than being
//! confirmed by itself.
//!
//! Two unauthenticated sources, both verified against the live services on
//! 2026-08-05 before this parser was written:
//!
//! - **fal** — `GET https://fal.ai/api/models?page=N`, paged, 40 per page.
//!   Its price field is `pricingInfoOverride`, which is *marketing prose*, not
//!   data: 764 of 1,418 entries carry none at all and the rest range from
//!   `"Your request will cost $0.05 per image."` to conditional tables written
//!   as sentences. See [`fal`] for why the parser refuses most of it.
//! - **Vercel AI Gateway** — `GET https://ai-gateway.vercel.sh/v1/models`, one
//!   request, genuinely structured `pricing` objects. See [`vaig`].
//!
//! # The bot checkpoint
//!
//! The plan warned that fal sits behind a checkpoint that 429s plain curl. That
//! is true of the **HTML** site and false of the **JSON** API, and the fix is
//! not the one the plan assumed. Measured on 2026-08-05:
//!
//! | Request | `halation/0.1.0` UA | Chrome UA |
//! |---|---|---|
//! | `https://fal.ai/models` (HTML) | 429 challenge | 429 challenge |
//! | `https://fal.ai/api/models` (JSON) | 200 | 200 |
//!
//! So a browser-shaped User-Agent does **not** defeat the checkpoint — it wants
//! a JS challenge solved, which a desktop app will not do. What works is asking
//! the JSON API instead of scraping the page. We still send browser-shaped
//! headers as cheap insurance in case the WAF rule is later widened to
//! `/api/*`, but the load-bearing defence is [`FeedError::BotCheckpoint`]:
//! a challenge response is *recognised* and reported, so the fetch degrades to
//! the bundled snapshot instead of surfacing as a JSON parse error — or worse,
//! as an absent price that some caller renders as free.
//!
//! # Never zero
//!
//! Every parser in this module rejects a non-finite or non-positive figure, and
//! a route the feeds do not price unambiguously simply has no [`Quote`].
//! [`PriceFeed::cost_model`] returns `None` in that case. There is no code path
//! here that can turn "we do not know" into `$0.00`.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::catalog::Modality;
use crate::cost::{Billable, CostModel};
use crate::provider::ProviderId;

/// fal's public model index. The JSON API, deliberately not the HTML page —
/// see the module docs on the checkpoint.
pub const FAL_MODELS_URL: &str = "https://fal.ai/api/models";

/// Vercel AI Gateway's OpenAI-shaped model list. Unauthenticated: it answers
/// with prices and no `Authorization` header.
pub const VAIG_MODELS_URL: &str = "https://ai-gateway.vercel.sh/v1/models";

/// A current desktop Chrome UA. Insurance only — verified on 2026-08-05 not to
/// change the outcome on either endpoint. It exists so that if fal extends its
/// WAF rule to `/api/*`, we are already shaped like the traffic it allows,
/// rather than shipping an update to add a header.
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// fal reported 36 pages on 2026-08-05. The cap stops a malformed `pages` field
/// from turning a launch fetch into an unbounded crawl of someone's connection.
const MAX_FAL_PAGES: u32 = 60;

/// Long enough for a slow page, short enough that a launch fetch cannot hang
/// the price display behind a dead network.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// The prices we shipped in the binary, so a first run with no network still
/// shows costs. Regenerated from the live feeds, never hand-edited.
const SNAPSHOT: &str = include_str!("../vendor/prices-snapshot.json");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Which upstream feed a number came from. Kept on every quote because the two
/// feeds disagree on the same logical model often enough that "which one said
/// this" is the first question anyone asks about a surprising price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feed {
    Fal,
    Vaig,
}

impl Feed {
    pub fn provider(self) -> ProviderId {
        match self {
            Feed::Fal => ProviderId::Fal,
            Feed::Vaig => ProviderId::Vaig,
        }
    }
}

/// Whether a quote was fetched in this session or read out of the bundle.
///
/// Per-quote rather than per-feed: a fetch where fal answers and Vercel does
/// not leaves a [`PriceFeed`] that is half live and half months old, and a
/// single feed-level flag would have to lie about one of the halves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    Live,
    Snapshot,
}

fn snapshot_origin() -> Origin {
    Origin::Snapshot
}

/// One row of a provider's published per-second rate table.
///
/// `None` on a field means the provider did not condition on it, which is
/// different from it not applying — an unconstrained row is a default that any
/// request can fall back to, and [`Quote::cost_model`] treats it that way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoTier {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// Provider tier name, `std` or `pro` in the feeds seen so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_control: Option<bool>,
    pub usd_per_second: f64,
}

/// One row of a per-image rate table keyed by output size (`1K`, `4K`,
/// `default`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageSizeTier {
    pub size: String,
    pub usd: f64,
}

/// What the feed said, before it is narrowed to a single [`CostModel`].
///
/// The tiered variants exist because [`CostModel::PerSecond`] holds one rate
/// and Vercel publishes six for Kling 3.0. Collapsing that at fetch time would
/// mean picking a row before knowing the request, which is how you quote the
/// 720p price for a 4K generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Priced {
    /// The settings the user can change do not move the rate.
    Fixed {
        cost: CostModel,
    },
    VideoTiers {
        tiers: Vec<VideoTier>,
    },
    ImageSizes {
        tiers: Vec<ImageSizeTier>,
    },
}

/// A fetched price for one [`crate::route::Route`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    /// Matches [`crate::route::Route::id`], i.e. `provider:slug`.
    pub route_id: String,
    pub feed: Feed,
    /// Not serialised: a quote read from the bundle is a snapshot quote by
    /// definition, and storing the word in the file would let it go stale
    /// against the file it is stored in.
    #[serde(skip, default = "snapshot_origin")]
    pub origin: Origin,
    /// Unix seconds. Drives [`PriceFeed::age`].
    pub fetched_at: u64,
    pub priced: Priced,
    /// The upstream model ids this number came from, so a wrong price is
    /// traceable to a specific row of a specific feed without a re-fetch.
    #[serde(default)]
    pub upstream: Vec<String>,
    /// A known way this quote can be too low or too high, in the user's words.
    /// Present only where the feed prices something [`Billable`] cannot
    /// express — Recraft's vector styles, Seedance's video-input discount.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caveat: Option<String>,
}

/// Why a feed did not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedError {
    /// A Vercel WAF challenge. Named separately from a plain 429 because the
    /// remedy is different: a rate limit clears by waiting, a challenge does
    /// not clear at all without solving JS, so retrying is pointless and the
    /// snapshot is the answer.
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
    Malformed {
        url: String,
        msg: String,
    },
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::BotCheckpoint { url } => write!(
                f,
                "{url} answered with a bot checkpoint, not prices — showing bundled prices instead"
            ),
            FeedError::Http { url, status } => write!(f, "{url} returned HTTP {status}"),
            FeedError::Transport { url, msg } => write!(f, "{url} was unreachable: {msg}"),
            FeedError::Malformed { url, msg } => {
                write!(f, "{url} sent something unreadable: {msg}")
            }
        }
    }
}

/// Prices for the routes the registry offers, keyed by route id.
#[derive(Debug, Clone, Default)]
pub struct PriceFeed {
    quotes: BTreeMap<String, Quote>,
    problems: Vec<FeedError>,
}

#[derive(Deserialize)]
struct SnapshotFile {
    quotes: Vec<Quote>,
}

impl PriceFeed {
    /// The prices compiled into the binary. Always available, never blocks,
    /// works offline — this is what a first run shows before [`Self::fetch`]
    /// has returned.
    ///
    /// A snapshot that will not parse yields an empty feed with the failure
    /// recorded rather than a panic: the app must still open. The test
    /// `bundled_snapshot_parses_and_is_not_empty` is what makes that
    /// unreachable in a shipped build.
    pub fn bundled() -> PriceFeed {
        match serde_json::from_str::<SnapshotFile>(SNAPSHOT) {
            Ok(f) => PriceFeed {
                quotes: f
                    .quotes
                    .into_iter()
                    .filter(|q| q.is_sane())
                    .map(|q| (q.route_id.clone(), q))
                    .collect(),
                problems: Vec::new(),
            },
            Err(e) => PriceFeed {
                quotes: BTreeMap::new(),
                problems: vec![FeedError::Malformed {
                    url: "vendor/prices-snapshot.json".into(),
                    msg: e.to_string(),
                }],
            },
        }
    }

    /// Fetch both feeds and overlay them on the bundle.
    ///
    /// Never fails. A dead network, a checkpoint or a reshaped response all
    /// leave the bundled prices in place and add an entry to
    /// [`Self::problems`], because the alternative — a generator with no prices
    /// on the Generate button — is worse than a price from last month clearly
    /// labelled with its age.
    pub fn fetch() -> PriceFeed {
        let mut feed = PriceFeed::bundled();
        let client = match reqwest::blocking::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(BROWSER_USER_AGENT)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                feed.problems.push(FeedError::Transport {
                    url: "(http client)".into(),
                    msg: e.to_string(),
                });
                return feed;
            }
        };

        let wanted = registry_routes();
        let now = unix_now();

        match fal::fetch(&client) {
            Ok(rows) => feed.overlay(fal::quotes(&rows, &wanted, now)),
            Err(e) => feed.problems.push(e),
        }
        match vaig::fetch(&client) {
            Ok(rows) => feed.overlay(vaig::quotes(&rows, &wanted, now)),
            Err(e) => feed.problems.push(e),
        }
        feed
    }

    fn overlay(&mut self, fresh: Vec<Quote>) {
        for q in fresh {
            if q.is_sane() {
                self.quotes.insert(q.route_id.clone(), q);
            }
        }
    }

    pub fn quote(&self, route_id: &str) -> Option<&Quote> {
        self.quotes.get(route_id)
    }

    pub fn quotes(&self) -> impl Iterator<Item = &Quote> {
        self.quotes.values()
    }

    /// Feeds that failed on the last [`Self::fetch`]. Non-empty means some of
    /// what is displayed is bundled rather than current; the UI should say so.
    pub fn problems(&self) -> &[FeedError] {
        &self.problems
    }

    pub fn len(&self) -> usize {
        self.quotes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.quotes.is_empty()
    }

    /// How old the *oldest* displayed price is.
    ///
    /// The oldest rather than the newest, because staleness is a property of
    /// the worst number on screen — a feed that is 90% fresh and 10% from
    /// December is a feed that can quote you a December price.
    ///
    /// `None` when there is nothing to age, or when the clock reads earlier
    /// than the fetch. That second case is a machine whose clock moved
    /// backwards; reporting `0` there would render as "just updated", which is
    /// exactly the wrong thing to tell someone about a stale price.
    pub fn age(&self) -> Option<Duration> {
        let oldest = self.quotes.values().map(|q| q.fetched_at).min()?;
        let now = unix_now();
        if now < oldest {
            return None;
        }
        Some(Duration::from_secs(now - oldest))
    }

    /// The cost model to price this route with, for these settings.
    ///
    /// `None` means the feeds do not price this route unambiguously. Callers
    /// must render that as unknown and fall back to whatever else they trust —
    /// never as free.
    pub fn cost_model(&self, route_id: &str, b: &Billable) -> Option<CostModel> {
        let q = self.quotes.get(route_id)?;
        q.cost_model(b, mode_hint(route_id))
    }
}

impl Quote {
    /// Reject anything that could put a zero or a nonsense number on the
    /// Generate button. Applied to the bundle as well as to live rows, because
    /// a hand-edit of the vendored file is exactly the kind of mistake that
    /// would otherwise ship.
    fn is_sane(&self) -> bool {
        match &self.priced {
            Priced::Fixed { cost } => cost_is_sane(cost),
            Priced::VideoTiers { tiers } => {
                !tiers.is_empty() && tiers.iter().all(|t| positive(t.usd_per_second))
            }
            Priced::ImageSizes { tiers } => {
                !tiers.is_empty() && tiers.iter().all(|t| positive(t.usd))
            }
        }
    }

    /// Narrow the feed's rate table to one [`CostModel`] for one request.
    ///
    /// `mode_hint` is the provider tier the route slug names (`std`, `pro`).
    /// It is a real signal rather than a guess: `klingai/kling-v2.6-standard`
    /// and `fal-ai/kling-video/v3/standard` both say which tier they are, and
    /// without it a two-tier table is genuinely ambiguous.
    pub fn cost_model(&self, b: &Billable, mode_hint: Option<&str>) -> Option<CostModel> {
        match &self.priced {
            Priced::Fixed { cost } => {
                // A stored `Unknown` is not a price; hand it back as `None` so
                // no caller can call `.estimate()` on it and read the zero-ish
                // shape of a missing number.
                if matches!(cost, CostModel::Unknown) || !cost_is_sane(cost) {
                    return None;
                }
                Some(cost.clone())
            }
            Priced::VideoTiers { tiers } => select_video_tier(tiers, b, mode_hint).map(|usd| {
                // 1.0, not the ratio between the audio-on and audio-off rows:
                // the row was already chosen for `b.audio`, so applying a
                // multiplier on top would bill the user twice for their audio.
                CostModel::PerSecond {
                    usd,
                    audio_multiplier: 1.0,
                }
            }),
            Priced::ImageSizes { tiers } => {
                select_image_size(tiers, b).map(|usd| CostModel::PerImage {
                    usd,
                    usd_per_extra_input: 0.0,
                })
            }
        }
    }
}

fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

fn cost_is_sane(c: &CostModel) -> bool {
    match c {
        CostModel::PerSecond {
            usd,
            audio_multiplier,
        } => positive(*usd) && positive(*audio_multiplier),
        CostModel::PerToken {
            usd_per_million,
            fps,
        } => positive(*usd_per_million) && *fps > 0,
        // Every tier must be a real price, and the thresholds must ascend or
        // the lookup picks the wrong one.
        CostModel::PerSecondTiered { tiers } => {
            !tiers.is_empty()
                && tiers.iter().all(|(h, usd)| *h > 0 && positive(*usd))
                && tiers.windows(2).all(|w| w[0].0 < w[1].0)
        }
        CostModel::PerImage {
            usd,
            usd_per_extra_input,
        } => positive(*usd) && usd_per_extra_input.is_finite() && *usd_per_extra_input >= 0.0,
        CostModel::PerMegapixel { usd, first_usd } => {
            positive(*usd) && first_usd.map(positive).unwrap_or(true)
        }
        CostModel::Flat { usd } => positive(*usd),
        // Deliberately not sane: `Unknown` is the absence of a price, so
        // storing one as a quote would be storing an empty box.
        CostModel::Unknown => false,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Matching a route to a feed row
// ---------------------------------------------------------------------------

/// A route we want a price for, with the modality that decides which of the
/// provider's endpoints are even the same product.
#[derive(Debug, Clone)]
struct Wanted {
    route_id: String,
    provider: ProviderId,
    slug: String,
    modality: Modality,
}

/// Every route the registry offers.
fn registry_routes() -> Vec<Wanted> {
    let mut out: Vec<Wanted> = crate::registry::registry()
        .values()
        .flat_map(|m| {
            m.routes.iter().map(|r| Wanted {
                route_id: r.id(),
                provider: r.provider,
                slug: r.slug.clone(),
                modality: m.modality,
            })
        })
        .collect();
    // By route id, which is `provider:slug` and therefore already unique per
    // route — `ProviderId` is deliberately not `Ord`, and a route id carries
    // the same ordering information anyway.
    out.sort_by(|a, b| a.route_id.cmp(&b.route_id));
    out.dedup_by(|a, b| a.route_id == b.route_id);
    out
}

/// Input-mode suffixes fal appends to a family root, grouped by what the
/// endpoint *produces*.
///
/// A whitelist, not "anything after a slash", because the extra segment is not
/// always a mode: `fal-ai/kling-video/o3/pro/video-to-video/edit` is $0.168/s
/// against $0.112/s for the text-to-video of the same family, and treating it
/// as a sibling makes the whole family look ambiguous and lose its price.
///
/// Grouped by output rather than pooled, because a fal family sells more than
/// one product under one root. `fal-ai/wan-25-preview` is the live case: its
/// `/text-to-image` costs **$0.05 per image** and its `/text-to-video` costs
/// **$0.10 per second at 720p**. Pooling them priced the video route — which is
/// what `wan2_5_video` routes to — at the image rate, a tenfold understatement
/// on a 5-second clip.
const FAL_VIDEO_SUFFIXES: [&str; 5] = [
    "/text-to-video",
    "/image-to-video",
    "/reference-to-video",
    "/first-last-frame-to-video",
    "/start-end-to-video",
];
const FAL_IMAGE_SUFFIXES: [&str; 3] = ["/text-to-image", "/image-to-image", "/edit"];
const FAL_AUDIO_SUFFIXES: [&str; 4] = [
    "/text-to-speech",
    "/text-to-music",
    "/text-to-audio",
    "/image-to-audio",
];

/// The endpoint suffixes that can serve a route of this modality.
fn fal_suffixes(m: Modality) -> &'static [&'static str] {
    match m {
        Modality::Video => &FAL_VIDEO_SUFFIXES,
        Modality::Image => &FAL_IMAGE_SUFFIXES,
        Modality::Audio => &FAL_AUDIO_SUFFIXES,
        // 3D and the explainer pipeline have no endpoint-naming convention we
        // have verified, so they match only an exact slug.
        Modality::ThreeD | Modality::Other => &[],
    }
}

/// Whether a billing shape can possibly describe this modality.
///
/// The backstop for the `wan-25-preview` bug above: even if matching ever
/// pairs a route with the wrong endpoint again, a per-image rate cannot reach a
/// video route and a per-second rate cannot reach an image route.
fn shape_fits(m: Modality, c: &CostModel) -> bool {
    !matches!(
        (m, c),
        (
            Modality::Video | Modality::Audio,
            CostModel::PerImage { .. } | CostModel::PerMegapixel { .. }
        ) | (Modality::Image, CostModel::PerSecond { .. })
    )
}

/// The same idea for Vercel, which uses short suffixes.
///
/// `-fast`, `-flash` and `-motion-control` are absent on purpose: they are
/// separate models at separate prices, and matching them would blend
/// `alibaba/wan-v2.6` at $0.10/s with `-i2v-flash` at $0.05/s.
const VAIG_MODE_SUFFIXES: [&str; 6] = ["-t2v", "-i2v", "-r2v", "-v2v", "-t2i", "-i2i"];

/// Feed rows that correspond to one route slug.
///
/// An exact hit wins outright. Without that rule `bytedance/seedance-2.0` would
/// also drag in `bytedance/seedance-2.0-fast`, a different model at a different
/// rate, and the disagreement would throw away a price we actually have.
fn matches<'a>(slug: &str, ids: &[&'a str], suffixes: &[&str]) -> Vec<&'a str> {
    if let Some(exact) = ids.iter().find(|id| **id == slug) {
        return vec![exact];
    }
    suffixes
        .iter()
        .filter_map(|suf| {
            let want = format!("{slug}{suf}");
            ids.iter().find(|id| **id == want).copied()
        })
        .collect()
}

/// The provider tier a route slug names, if it names one.
fn mode_hint(route_id: &str) -> Option<&'static str> {
    let l = route_id.to_ascii_lowercase();
    // "standard" is tested first so that a slug carrying both words resolves to
    // the cheaper tier. Over-quoting a std job is a bad estimate; under-quoting
    // a pro job is a bill the user did not agree to.
    if l.contains("standard") || l.ends_with("-std") || l.contains("/std") {
        Some("std")
    } else if l.contains("pro") {
        Some("pro")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tier selection
// ---------------------------------------------------------------------------

/// The label a provider would use for this output size.
///
/// Short side, which is what "720p" means for both landscape and portrait.
/// Only exact matches: a 576-line clip is not "480p rounded up", and inventing
/// a nearest tier is inventing a price.
fn resolution_label(b: &Billable) -> Option<&'static str> {
    let (w, h) = (b.width?, b.height?);
    match w.min(h) {
        360 => Some("360p"),
        480 => Some("480p"),
        540 => Some("540p"),
        576 => Some("576p"),
        720 => Some("720p"),
        1080 => Some("1080p"),
        1440 => Some("2k"),
        2160 => Some("4k"),
        _ => None,
    }
}

/// The size label a per-image table would use. Long side, which is how the
/// Gateway's `1K`/`2K`/`4K` rows are scaled.
fn image_size_label(b: &Billable) -> Option<&'static str> {
    let (w, h) = (b.width?, b.height?);
    match w.max(h) {
        512 => Some("512"),
        1024 => Some("1k"),
        2048 => Some("2k"),
        4096 => Some("4k"),
        _ => None,
    }
}

/// Keep the rows that state this attribute explicitly, if any do; otherwise
/// keep the rows that leave it unset.
///
/// Specific beats default, and a row that does not mention audio is a fallback
/// for requests the provider did not price separately — not a row that applies
/// only when audio is off.
fn narrow<T, F, G>(rows: Vec<&T>, explicit: F, wanted: G) -> Vec<&T>
where
    F: Fn(&T) -> bool,
    G: Fn(&T) -> bool,
{
    let hits: Vec<&T> = rows
        .iter()
        .copied()
        .filter(|r| explicit(r) && wanted(r))
        .collect();
    if !hits.is_empty() {
        return hits;
    }
    rows.into_iter().filter(|r| !explicit(r)).collect()
}

/// One rate, or nothing.
///
/// Returns `None` rather than a nearest-guess whenever two surviving rows carry
/// different rates. `klingai/kling-v3.0` is the live example: its table holds a
/// `std` row at $0.168/s and a `pro` row at $0.224/s and the route slug names
/// neither, so we genuinely do not know which one a generation would be billed
/// at. Quoting the cheaper one would understate a pro job by a third.
fn select_video_tier(tiers: &[VideoTier], b: &Billable, mode_hint: Option<&str>) -> Option<f64> {
    let label = resolution_label(b);
    let mut rows: Vec<&VideoTier> = tiers.iter().collect();

    rows = narrow(
        rows,
        |t| t.resolution.is_some(),
        |t| match (&t.resolution, label) {
            (Some(r), Some(l)) => r.eq_ignore_ascii_case(l),
            _ => false,
        },
    );
    rows = narrow(rows, |t| t.audio.is_some(), |t| t.audio == Some(b.audio));
    // Voice control is a Kling extra we never request and `Billable` cannot
    // express. Its rows would otherwise sit alongside the plain audio-on row
    // and make every Kling 3.0 quote look ambiguous.
    rows.retain(|t| t.voice_control != Some(true));
    if let Some(hint) = mode_hint {
        rows = narrow(
            rows,
            |t| t.mode.is_some(),
            |t| t.mode.as_deref().map(|m| m.eq_ignore_ascii_case(hint)) == Some(true),
        );
    }

    one_price(rows.into_iter().map(|t| t.usd_per_second))
}

/// Same rule for per-image tables. Falls back to the feed's own `default` row,
/// which is a value the provider published — not one we picked.
fn select_image_size(tiers: &[ImageSizeTier], b: &Billable) -> Option<f64> {
    let label = image_size_label(b);
    if let Some(l) = label {
        if let Some(t) = tiers.iter().find(|t| t.size.eq_ignore_ascii_case(l)) {
            return positive(t.usd).then_some(t.usd);
        }
    }
    let d = tiers
        .iter()
        .find(|t| t.size.eq_ignore_ascii_case("default"))?;
    positive(d.usd).then_some(d.usd)
}

/// Collapse candidate rates to one only when they agree.
///
/// Duplicate rows are common — Vercel lists 720p and 1080p at the same rate for
/// Wan 2.6 — and refusing on count alone would drop prices that are not in fact
/// ambiguous. Refusing on *disagreement* is the guard that matters.
fn one_price(rates: impl Iterator<Item = f64>) -> Option<f64> {
    let mut chosen: Option<f64> = None;
    for r in rates {
        if !positive(r) {
            return None;
        }
        match chosen {
            None => chosen = Some(r),
            Some(c) if (c - r).abs() < 1e-12 => {}
            Some(_) => return None,
        }
    }
    chosen
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// A request shaped like a browser navigation.
///
/// Verified not to change either endpoint's answer on 2026-08-05; it is here so
/// that widening fal's WAF rule to `/api/*` does not require an app update.
fn browser_get(client: &reqwest::blocking::Client, url: &str) -> reqwest::blocking::RequestBuilder {
    client
        .get(url)
        .header("User-Agent", BROWSER_USER_AGENT)
        .header(
            "Accept",
            "application/json,text/html;q=0.9,application/xhtml+xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Dest", "empty")
        .header("Sec-Fetch-Mode", "cors")
        .header("Sec-Fetch-Site", "same-origin")
}

/// True when a response is a Vercel WAF challenge rather than content.
///
/// Header-first: `x-vercel-mitigated: challenge` is what the edge actually
/// stamps on a blocked request, and it is present whatever the body turns out
/// to be. The title check is a backstop for a future challenge page served
/// without the header — losing it would mean feeding a 33 KB HTML document to
/// `serde_json` and reporting "unreadable" for something we understand exactly.
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

/// GET a URL and hand back parsed JSON, classifying a checkpoint as itself.
fn get_json(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<serde_json::Value, Box<FeedError>> {
    let resp = browser_get(client, url).send().map_err(|e| {
        Box::new(FeedError::Transport {
            url: url.to_string(),
            msg: e.to_string(),
        })
    })?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.text().map_err(|e| {
        Box::new(FeedError::Transport {
            url: url.to_string(),
            msg: e.to_string(),
        })
    })?;

    if is_bot_checkpoint(status, &headers, &body) {
        return Err(Box::new(FeedError::BotCheckpoint {
            url: url.to_string(),
        }));
    }
    if !(200..300).contains(&status) {
        return Err(Box::new(FeedError::Http {
            url: url.to_string(),
            status,
        }));
    }
    serde_json::from_str(&body).map_err(|e| {
        Box::new(FeedError::Malformed {
            url: url.to_string(),
            msg: e.to_string(),
        })
    })
}

// ---------------------------------------------------------------------------
// fal
// ---------------------------------------------------------------------------

/// fal's feed.
///
/// **fal publishes no structured prices.** `pricingInfoOverride` is prose
/// written for humans, and on 2026-08-05 only 654 of 1,418 models carried one
/// at all. The parser below accepts five sentence templates and refuses
/// everything else, which prices 5 of our 36 fal routes.
///
/// That low yield is the point. The refusals are not gaps in the parser, they
/// are prices we do not know:
///
/// - `fal-ai/nano-banana-2` says `$0.08 per image` and then that 2K and 4K cost
///   1.5x and 2x. Reading the first figure quotes half the real price of a 4K
///   image.
/// - `fal-ai/topaz/upscale/video` lists $0.01, $0.02 and $0.08 per second by
///   output resolution, and doubles for 60fps.
/// - `fal-ai/recraft/v3` is $0.04 per image or $0.08 in a vector style.
/// - `fal-ai/flux-lora-portrait-trainer` says `$0.0024 cents per step` — dollars
///   and cents in one phrase — with a 1,000-step floor.
///
/// Every one of those has a first dollar figure that a naive parser would lift,
/// and every one of those numbers would be wrong on the Generate button.
mod fal {
    use super::*;

    /// The subset of a fal model row we read.
    #[derive(Debug, Clone, Deserialize)]
    pub struct Row {
        pub id: String,
        #[serde(rename = "pricingInfoOverride")]
        pub pricing: Option<String>,
    }

    #[derive(Deserialize)]
    struct Page {
        items: Vec<Row>,
        #[serde(default)]
        pages: u32,
    }

    /// Page through the whole index. 36 pages of 40 as of 2026-08-05.
    pub fn fetch(client: &reqwest::blocking::Client) -> Result<Vec<Row>, FeedError> {
        let mut out = Vec::new();
        let mut page = 1u32;
        let mut total = 1u32;
        while page <= total && page <= MAX_FAL_PAGES {
            let url = format!("{FAL_MODELS_URL}?page={page}");
            let v = get_json(client, &url).map_err(|e| *e)?;
            let parsed: Page = serde_json::from_value(v).map_err(|e| FeedError::Malformed {
                url: url.clone(),
                msg: e.to_string(),
            })?;
            if page == 1 {
                total = parsed.pages.max(1);
            }
            if parsed.items.is_empty() {
                break;
            }
            out.extend(parsed.items);
            page += 1;
        }
        if out.is_empty() {
            return Err(FeedError::Malformed {
                url: FAL_MODELS_URL.to_string(),
                msg: "index returned no models".into(),
            });
        }
        Ok(out)
    }

    /// Turn feed rows into quotes for the routes we actually offer.
    pub fn quotes(rows: &[Row], wanted: &[Wanted], now: u64) -> Vec<super::Quote> {
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let mut out = Vec::new();
        for w in wanted {
            if w.provider != ProviderId::Fal {
                continue;
            }
            let hits = matches(&w.slug, &ids, fal_suffixes(w.modality));
            let mut parsed: Vec<(&str, CostModel)> = Vec::new();
            for id in hits {
                let Some(row) = rows.iter().find(|r| r.id == id) else {
                    continue;
                };
                if let Some(c) = parse_prose(row.pricing.as_deref()) {
                    if shape_fits(w.modality, &c) {
                        parsed.push((id, c));
                    }
                }
            }
            if parsed.is_empty() {
                continue;
            }
            // Every endpoint of a family must agree. They disagree for real:
            // `fal-ai/kling-video/v3/turbo` resolves to a `/pro` at $0.14/s and
            // a `/standard` at $0.112/s, and the route slug does not say which
            // one a job lands on.
            let first = parsed[0].1.clone();
            if parsed.iter().any(|(_, c)| *c != first) {
                continue;
            }
            out.push(super::Quote {
                route_id: w.route_id.clone(),
                feed: Feed::Fal,
                origin: Origin::Live,
                fetched_at: now,
                priced: Priced::Fixed { cost: first },
                upstream: parsed.iter().map(|(i, _)| i.to_string()).collect(),
                caveat: None,
            });
        }
        out
    }

    /// Everything from here on is illustration, not rate: a worked example
    /// (`For example, a 5s video ...`), a runs-per-dollar line (`For $1.00, you
    /// can run ...`) or a disclaimer (`Note: ...`). Cutting it first is what
    /// lets the "exactly one dollar figure" rule mean what it says.
    fn trim_illustration(s: &str) -> String {
        let plain = s.replace("**", "").replace('*', "");
        let low = plain.to_ascii_lowercase();
        let cut = ["for example", "for $1", "for $1.00", "note:"]
            .iter()
            .filter_map(|m| low.find(m))
            .min()
            .unwrap_or(plain.len());
        plain[..cut]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Dollar figures, in order.
    fn money(s: &str) -> Vec<f64> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'$' {
                let mut j = i + 1;
                while j < b.len() && b[j] == b' ' {
                    j += 1;
                }
                let start = j;
                while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                    j += 1;
                }
                // Trailing sentence period: "$0.14." is $0.14, not $0.14.<eof>
                let mut end = j;
                while end > start && b[end - 1] == b'.' {
                    end -= 1;
                }
                if end > start {
                    if let Ok(v) = s[start..end].parse::<f64>() {
                        out.push(v);
                    }
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Tokens that mean the single figure in the sentence is not the whole
    /// story. Each one is present in a real fal string that would otherwise
    /// parse into a wrong number.
    const DISQUALIFYING: [&str; 26] = [
        "double",
        "half",
        "times the",
        "x the",
        "per step",
        "minimum",
        "rounded",
        "additional",
        "depends",
        "depend on",
        "varies",
        "vary",
        "vector",
        "style",
        "discount",
        "cents",
        "frame",
        "tier",
        "sample",
        "candidate",
        "compute second",
        "training",
        "input and output",
        "web search",
        "multiplied",
        "surcharge",
    ];

    /// Output-resolution words. Their presence means the rate is conditional on
    /// something the sentence is about to qualify.
    const RESOLUTION_WORDS: [&str; 12] = [
        "360p", "480p", "540p", "576p", "720p", "1080p", "1440p", "2160p", "0.5k", "1k", "2k", "4k",
    ];

    fn contains_resolution(low: &str) -> bool {
        RESOLUTION_WORDS.iter().any(|w| low.contains(w))
    }

    /// Parse a `pricingInfoOverride` string, or refuse.
    ///
    /// Five templates, all of which state their rate and its unit outright:
    ///
    /// 1. `... $X (audio off) or $Y (audio on)` → per-second with a multiplier
    /// 2. `For every second of video ... $X` → per-second
    /// 3. `Your request will cost $X per image.` → per-image
    /// 4. `Your request will cost $X per megapixel.` → per-megapixel
    /// 5. `Your request will cost $X per generation.` → flat
    pub fn parse_prose(raw: Option<&str>) -> Option<CostModel> {
        let t = trim_illustration(raw?);
        let low = t.to_ascii_lowercase();

        // Template 1 first: it is the only one that legitimately carries two
        // figures, so it has to be recognised before the one-figure rule runs.
        if let Some(m) = audio_pair(&t, &low) {
            return Some(m);
        }

        if contains_resolution(&low) {
            return None;
        }
        if DISQUALIFYING.iter().any(|d| low.contains(d)) {
            return None;
        }

        let figures = money(&t);
        let mut distinct = figures.clone();
        distinct.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        distinct.sort_by(|a, b| a.total_cmp(b));
        distinct.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        if distinct.len() != 1 {
            return None;
        }
        let usd = distinct[0];
        if !positive(usd) {
            return None;
        }

        if per_second(&low) {
            return Some(CostModel::PerSecond {
                usd,
                audio_multiplier: 1.0,
            });
        }
        if ends_with_unit(&low, &["per image", "per output image"]) {
            return Some(CostModel::PerImage {
                usd,
                usd_per_extra_input: 0.0,
            });
        }
        if ends_with_unit(&low, &["per megapixel", "per output megapixel"]) {
            return Some(CostModel::PerMegapixel {
                usd,
                first_usd: None,
            });
        }
        if ends_with_unit(
            &low,
            &["per video generation", "per generation", "per request"],
        ) {
            return Some(CostModel::Flat { usd });
        }
        None
    }

    /// `For every second of video you generated, you will be charged $0.084
    /// (audio off) or $0.126 (audio on)`.
    ///
    /// Both rates are stated with their condition, so the multiplier is read
    /// out of the sentence rather than assumed. This template reproduces the
    /// hand-transcribed `per_second_audio(0.084, 1.5)` in `registry.rs` exactly,
    /// which is the evidence the fetcher and the transcription agree.
    fn audio_pair(t: &str, low: &str) -> Option<CostModel> {
        let off_at = low.find("(audio off)")?;
        let on_at = low.find("(audio on)")?;
        if on_at < off_at {
            return None;
        }
        let off = *money(&t[..off_at]).last()?;
        let on = *money(&t[off_at..on_at]).last()?;
        if !positive(off) || !positive(on) || on < off {
            return None;
        }
        // Kling names a third, higher rate for voice control. We never request
        // it and `Billable` cannot express it, so the clause is dropped — but
        // any *other* trailing figure means the sentence is still qualifying
        // itself and we should not be reading it.
        let tail = &low[on_at + "(audio on)".len()..];
        let tail = match tail.find("if voice control") {
            Some(i) => {
                let rest = &tail[i..];
                let end = rest.find('.').map(|e| i + e + 1).unwrap_or(tail.len());
                format!("{}{}", &tail[..i], &tail[end.min(tail.len())..])
            }
            None => tail.to_string(),
        };
        if !money(&tail).is_empty() || contains_resolution(low) {
            return None;
        }
        Some(CostModel::PerSecond {
            usd: off,
            // Ratios of two decimal literals land on values like
            // 1.4999999999999998 for $0.15/$0.10. Rounded to 4 places purely so
            // the estimate's "audio x1.5" reads as a price and not as float
            // noise; 1e-4 on a multiplier is far below a cent on any real clip.
            audio_multiplier: ((on / off) * 10_000.0).round() / 10_000.0,
        })
    }

    /// A per-second rate must be *anchored*: the sentence has to open by naming
    /// the unit, or state it as an explicit "per second of output" phrase.
    ///
    /// A merely-contains check would accept `"$0.05 per second for 480p, $0.10
    /// per second for 720p"` as a flat rate. That string is refused earlier by
    /// the resolution check; the anchor is the second line of defence.
    fn per_second(low: &str) -> bool {
        low.starts_with("for every second of")
            || low.starts_with("for each second of")
            || low.contains("per second of generated video")
            || low.contains("per second of output video")
            || low.contains("per video second")
    }

    /// The sentence must *end* on the unit. `"$0.028 per image for 1K/2K,
    /// price will be double for 4K"` contains "per image" and is not a flat
    /// per-image rate.
    fn ends_with_unit(low: &str, units: &[&str]) -> bool {
        let trimmed = low.trim_end_matches(['.', ' ']);
        units.iter().any(|u| trimmed.ends_with(u))
    }
}

// ---------------------------------------------------------------------------
// Vercel AI Gateway
// ---------------------------------------------------------------------------

/// Vercel's feed.
///
/// Genuinely structured, unlike fal's: `pricing` is an object with
/// `video_duration_pricing` rate tables, `video_token_pricing`, or a flat
/// `image` figure. On 2026-08-05 it matched 20 of our 22 Vercel routes and
/// priced 18 of them — `bfl/flux-2-pro` ships an empty `pricing` object
/// upstream and `openai/gpt-image-2` is priced per token.
///
/// Language-model token pricing (`input`/`output` per token) is read and then
/// discarded on purpose. [`Billable`] carries no token counts, so a per-token
/// image price like GPT Image 2's cannot be turned into a number for the
/// Generate button — which is the same conclusion `registry.rs` reached by
/// hand, and the honest one.
mod vaig {
    use super::*;

    #[derive(Debug, Clone, Deserialize)]
    pub struct Row {
        pub id: String,
        #[serde(default)]
        pub pricing: Option<Pricing>,
    }

    #[derive(Debug, Clone, Default, Deserialize)]
    pub struct Pricing {
        #[serde(default)]
        pub image: Option<String>,
        #[serde(default)]
        pub video_duration_pricing: Option<Vec<DurationRow>>,
        #[serde(default)]
        pub video_token_pricing: Option<TokenPricing>,
        #[serde(default)]
        pub image_dimension_quality_pricing: Option<Vec<DimensionRow>>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct DurationRow {
        #[serde(default)]
        pub resolution: Option<String>,
        #[serde(default)]
        pub mode: Option<String>,
        #[serde(default)]
        pub audio: Option<bool>,
        #[serde(default)]
        pub voice_control: Option<bool>,
        pub cost_per_second: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct TokenPricing {
        #[serde(default)]
        pub no_video_input: Option<TokenRate>,
        #[serde(default)]
        pub with_video_input: Option<TokenRate>,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct TokenRate {
        pub cost_per_million_tokens: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    pub struct DimensionRow {
        #[serde(default)]
        pub size: Option<String>,
        #[serde(default)]
        pub style: Option<String>,
        #[serde(default)]
        pub operation: Option<String>,
        pub cost: String,
    }

    #[derive(Deserialize)]
    struct ModelList {
        data: Vec<Row>,
    }

    pub fn fetch(client: &reqwest::blocking::Client) -> Result<Vec<Row>, FeedError> {
        let v = get_json(client, VAIG_MODELS_URL).map_err(|e| *e)?;
        let parsed: ModelList = serde_json::from_value(v).map_err(|e| FeedError::Malformed {
            url: VAIG_MODELS_URL.to_string(),
            msg: e.to_string(),
        })?;
        if parsed.data.is_empty() {
            return Err(FeedError::Malformed {
                url: VAIG_MODELS_URL.to_string(),
                msg: "model list was empty".into(),
            });
        }
        Ok(parsed.data)
    }

    pub fn quotes(rows: &[Row], wanted: &[Wanted], now: u64) -> Vec<super::Quote> {
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let mut out = Vec::new();
        for w in wanted {
            if w.provider != ProviderId::Vaig {
                continue;
            }
            let hits = matches(&w.slug, &ids, &VAIG_MODE_SUFFIXES);
            let mut parsed: Vec<(&str, Priced, Option<String>)> = Vec::new();
            for id in hits {
                let Some(row) = rows.iter().find(|r| r.id == id) else {
                    continue;
                };
                if let Some((p, caveat)) = parse_pricing(row.pricing.as_ref()) {
                    // Same cross-modality guard as fal. The Gateway's suffixes
                    // are input modes rather than output kinds, so this has not
                    // fired here yet — it is here so it cannot start.
                    if let Priced::Fixed { cost } = &p {
                        if !shape_fits(w.modality, cost) {
                            continue;
                        }
                    }
                    parsed.push((id, p, caveat));
                }
            }
            if parsed.is_empty() {
                continue;
            }
            let first = parsed[0].1.clone();
            if parsed.iter().any(|(_, p, _)| *p != first) {
                continue;
            }
            out.push(super::Quote {
                route_id: w.route_id.clone(),
                feed: Feed::Vaig,
                origin: Origin::Live,
                fetched_at: now,
                priced: first,
                upstream: parsed.iter().map(|(i, _, _)| i.to_string()).collect(),
                caveat: parsed.iter().find_map(|(_, _, c)| c.clone()),
            });
        }
        out
    }

    fn num(s: &str) -> Option<f64> {
        s.trim().parse::<f64>().ok().filter(|v| positive(*v))
    }

    /// Read one `pricing` object, returning the rate table and any way it is
    /// known to be wrong for a request we can describe.
    pub fn parse_pricing(p: Option<&Pricing>) -> Option<(Priced, Option<String>)> {
        let p = p?;

        if let Some(rows) = &p.video_duration_pricing {
            if rows.is_empty() {
                return None;
            }
            let mut tiers = Vec::with_capacity(rows.len());
            for r in rows {
                tiers.push(VideoTier {
                    resolution: r.resolution.clone(),
                    mode: r.mode.clone(),
                    audio: r.audio,
                    voice_control: r.voice_control,
                    usd_per_second: num(&r.cost_per_second)?,
                });
            }
            return Some((Priced::VideoTiers { tiers }, None));
        }

        if let Some(t) = &p.video_token_pricing {
            let no = num(&t.no_video_input.as_ref()?.cost_per_million_tokens)?;
            let mut caveat = None;
            if let Some(w) = &t.with_video_input {
                let with = num(&w.cost_per_million_tokens)?;
                // Both Seedance rows discount video input ($7.00 → $4.30). If a
                // future row ever inverted that, quoting the no-video rate
                // would understate the bill, so refuse rather than guess which
                // rate a request lands on.
                if with > no {
                    return None;
                }
                caveat = Some(format!(
                    "quoted at the no-video-input rate of ${no}/M tokens; a request that \
                     includes video bills at ${with}/M, so this over-states rather than \
                     under-states"
                ));
            }
            return Some((
                Priced::Fixed {
                    cost: CostModel::PerToken {
                        usd_per_million: no,
                        fps: 24,
                    },
                },
                caveat,
            ));
        }

        let dims = p.image_dimension_quality_pricing.as_deref().unwrap_or(&[]);
        let sized: Vec<&DimensionRow> = dims.iter().filter(|d| d.size.is_some()).collect();
        if !sized.is_empty() {
            let mut tiers = Vec::with_capacity(sized.len());
            for d in sized {
                tiers.push(ImageSizeTier {
                    size: d.size.clone()?,
                    usd: num(&d.cost)?,
                });
            }
            return Some((Priced::ImageSizes { tiers }, None));
        }

        if let Some(img) = &p.image {
            let usd = num(img)?;
            // Recraft prices `vector_illustration` at more than double the base
            // rate. `Billable` has no style axis, so the quote is the base rate
            // and the caveat is the only honest way to say the rest.
            let others: Vec<String> = dims
                .iter()
                .filter_map(|d| d.style.clone().or_else(|| d.operation.clone()))
                .collect();
            let caveat = (!others.is_empty()).then(|| {
                format!(
                    "base rate only; the feed prices {} separately and Billable carries no \
                     style, so those generations cost more than quoted",
                    others.join(", ")
                )
            });
            return Some((
                Priced::Fixed {
                    cost: CostModel::PerImage {
                        usd,
                        usd_per_extra_input: 0.0,
                    },
                },
                caveat,
            ));
        }

        // Only per-token language pricing left. See the module docs.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- the bundle ------------------------------------------------------

    #[test]
    fn bundled_snapshot_parses_and_is_not_empty() {
        // `bundled()` swallows a parse failure so the app still opens. This is
        // the test that makes a broken vendored file impossible to ship.
        let f = PriceFeed::bundled();
        assert!(f.problems().is_empty(), "{:?}", f.problems());
        assert!(f.len() >= 20, "snapshot only had {} quotes", f.len());
    }

    #[test]
    fn no_bundled_quote_is_free() {
        // A zero here would put "$0.00" on the Generate button for a paid
        // generation — the single worst thing this codebase can do.
        for q in PriceFeed::bundled().quotes() {
            match &q.priced {
                Priced::Fixed { cost } => {
                    assert!(cost_is_sane(cost), "{} has {:?}", q.route_id, cost)
                }
                Priced::VideoTiers { tiers } => {
                    for t in tiers {
                        assert!(t.usd_per_second > 0.0, "{} has a free tier", q.route_id);
                    }
                }
                Priced::ImageSizes { tiers } => {
                    for t in tiers {
                        assert!(t.usd > 0.0, "{} has a free size", q.route_id);
                    }
                }
            }
        }
    }

    #[test]
    fn every_bundled_route_id_still_exists_in_the_registry() {
        // Catches the drift that makes a price silently unreachable: rename a
        // route slug and its fetched price stops matching anything, which looks
        // exactly like "the provider stopped publishing a price".
        let known: std::collections::BTreeSet<String> =
            registry_routes().into_iter().map(|w| w.route_id).collect();
        for q in PriceFeed::bundled().quotes() {
            assert!(
                known.contains(&q.route_id),
                "{} is priced but no longer routed",
                q.route_id
            );
        }
    }

    #[test]
    fn a_bundled_price_reaches_a_real_usd_estimate() {
        // The end-to-end path the Generate button depends on: bundled JSON →
        // Quote → CostModel → Estimate. Asserts the shape rather than the
        // figure, so refreshing the snapshot when fal moves a price does not
        // turn a correct update into a red test.
        let f = PriceFeed::bundled();
        let b = Billable::video(8.0, 1280, 720);
        let m = f
            .cost_model("fal:fal-ai/kling-video/v3/standard", &b)
            .expect("kling v3 standard is priced in the bundle");
        let e = m.estimate(&b).expect("a per-second model always estimates");
        assert!(e.usd > 0.0, "estimate was {}", e.usd);
        assert!(e.basis.contains("8s"), "basis was {}", e.basis);
    }

    #[test]
    fn a_bundled_audio_rate_is_dearer_than_the_silent_one() {
        // fal states the Kling audio surcharge in prose and hides it in the
        // product. If the multiplier were ever dropped on the way through, the
        // two numbers would be equal and nobody would notice from the shape.
        let f = PriceFeed::bundled();
        let mut loud = Billable::video(8.0, 1280, 720);
        loud.audio = true;
        let quiet = Billable::video(8.0, 1280, 720);
        let id = "fal:fal-ai/kling-video/v3/standard";
        let a = f.cost_model(id, &loud).unwrap().estimate(&loud).unwrap();
        let s = f.cost_model(id, &quiet).unwrap().estimate(&quiet).unwrap();
        assert!(a.usd > s.usd, "audio {} vs silent {}", a.usd, s.usd);
    }

    #[test]
    fn no_bundled_quote_is_an_order_of_magnitude_from_the_hand_transcribed_one() {
        // Not an equality check: prices move, and a refreshed snapshot that
        // disagrees with `registry.rs` by 20% is the fetcher doing its job.
        // A **tenfold** gap is not a price change, it is a units bug — that is
        // exactly how the `wan-25-preview` per-image/per-second mismatch showed
        // up, and it is the shape worth failing a build over.
        let f = PriceFeed::bundled();
        let reg = crate::registry::registry();
        let video = Billable::video(5.0, 1280, 720);
        let image = Billable::image(1);
        for m in reg.values() {
            let b = if m.modality == Modality::Video {
                &video
            } else {
                &image
            };
            for r in &m.routes {
                let (Some(hard), Some(live)) = (
                    m.estimate(r, b).map(|e| e.usd),
                    f.cost_model(&r.id(), b)
                        .and_then(|c| c.estimate(b))
                        .map(|e| e.usd),
                ) else {
                    continue;
                };
                let ratio = (hard / live).max(live / hard);
                assert!(
                    ratio < 10.0,
                    "{}: registry says ${hard:.4}, the feed says ${live:.4} ({ratio:.1}x apart)",
                    r.id()
                );
            }
        }
    }

    #[test]
    fn bundled_quotes_are_marked_as_snapshot_not_live() {
        // The UI decides whether to show a staleness warning off this field.
        for q in PriceFeed::bundled().quotes() {
            assert_eq!(q.origin, Origin::Snapshot, "{}", q.route_id);
        }
    }

    // -- staleness -------------------------------------------------------

    #[test]
    fn age_reports_the_oldest_quote_not_the_newest() {
        // A feed that is 90% fresh can still quote a stale price; the warning
        // has to be driven by the worst number on screen.
        let mut f = PriceFeed::default();
        let now = unix_now();
        for (id, at) in [("a", now - 10), ("b", now - 5_000)] {
            f.quotes.insert(
                id.to_string(),
                Quote {
                    route_id: id.into(),
                    feed: Feed::Fal,
                    origin: Origin::Live,
                    fetched_at: at,
                    priced: Priced::Fixed {
                        cost: CostModel::Flat { usd: 1.0 },
                    },
                    upstream: vec![],
                    caveat: None,
                },
            );
        }
        assert!(f.age().unwrap().as_secs() >= 5_000);
    }

    #[test]
    fn a_clock_set_backwards_reports_unknown_age_not_fresh() {
        // Returning Duration::ZERO here would render as "updated just now" for
        // a price fetched in the future, i.e. the most stale case possible.
        let mut f = PriceFeed::default();
        f.quotes.insert(
            "a".into(),
            Quote {
                route_id: "a".into(),
                feed: Feed::Fal,
                origin: Origin::Live,
                fetched_at: unix_now() + 86_400,
                priced: Priced::Fixed {
                    cost: CostModel::Flat { usd: 1.0 },
                },
                upstream: vec![],
                caveat: None,
            },
        );
        assert_eq!(f.age(), None);
    }

    #[test]
    fn an_empty_feed_has_no_age() {
        assert_eq!(PriceFeed::default().age(), None);
    }

    // -- never zero ------------------------------------------------------

    #[test]
    fn an_unknown_cost_model_is_never_returned_as_a_price() {
        let q = Quote {
            route_id: "fal:x".into(),
            feed: Feed::Fal,
            origin: Origin::Live,
            fetched_at: 0,
            priced: Priced::Fixed {
                cost: CostModel::Unknown,
            },
            upstream: vec![],
            caveat: None,
        };
        assert!(q.cost_model(&Billable::image(1), None).is_none());
        assert!(!q.is_sane(), "an Unknown quote must not survive overlay");
    }

    #[test]
    fn a_zero_or_negative_rate_is_rejected_not_stored() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let q = Quote {
                route_id: "fal:x".into(),
                feed: Feed::Fal,
                origin: Origin::Live,
                fetched_at: 0,
                priced: Priced::VideoTiers {
                    tiers: vec![VideoTier {
                        resolution: None,
                        mode: None,
                        audio: None,
                        voice_control: None,
                        usd_per_second: bad,
                    }],
                },
                upstream: vec![],
                caveat: None,
            };
            assert!(!q.is_sane(), "{bad} was accepted as a price");
        }
    }

    #[test]
    fn a_route_with_no_quote_has_no_price() {
        let f = PriceFeed::bundled();
        assert!(f
            .cost_model("fal:not-a-real-route", &Billable::image(1))
            .is_none());
    }

    // -- fal prose -------------------------------------------------------

    // Every string below is a verbatim `pricingInfoOverride` captured from
    // https://fal.ai/api/models on 2026-08-05.

    #[test]
    fn the_audio_pair_template_reproduces_the_hand_transcribed_kling_price() {
        // registry.rs carries `per_second_audio(0.084, 1.5)` for this route,
        // typed in by hand from a research document. The fetcher deriving the
        // same pair from the live feed is the evidence that the two agree.
        let s = "For every second of video you generated, you will be charged **$0.084** \
                 (audio off) or **$0.126** (audio on), if voice control is used while \
                 generating audio you will be charged **$0.154**. For example, a 5s video \
                 with audio on and voice control will cost **$0.77**";
        assert_eq!(
            fal::parse_prose(Some(s)),
            Some(CostModel::PerSecond {
                usd: 0.084,
                audio_multiplier: 1.5
            })
        );
    }

    #[test]
    fn a_float_noisy_multiplier_is_not_shown_as_1_4999999999999998() {
        // $0.15/$0.10 is exactly 1.5; IEEE754 says otherwise, and cost.rs
        // formats the multiplier straight into the user-facing basis string.
        let s = "For every second of video you generated, you will be charged **$0.10** \
                 (audio off) or **$0.15** (audio on).";
        let CostModel::PerSecond {
            audio_multiplier, ..
        } = fal::parse_prose(Some(s)).unwrap()
        else {
            panic!("expected per-second")
        };
        assert_eq!(audio_multiplier, 1.5);
    }

    #[test]
    fn a_flat_per_second_rate_is_read_without_its_worked_example() {
        let s = "For every second of video you generate, you will be charged **$0.14** . \
                 For example, a 5s video will cost **$0.70**.";
        assert_eq!(
            fal::parse_prose(Some(s)),
            Some(CostModel::PerSecond {
                usd: 0.14,
                audio_multiplier: 1.0
            })
        );
    }

    #[test]
    fn a_bare_per_image_rate_is_read() {
        assert_eq!(
            fal::parse_prose(Some("Your request will cost **$0.05** per image.")),
            Some(CostModel::PerImage {
                usd: 0.05,
                usd_per_extra_input: 0.0
            })
        );
    }

    #[test]
    fn a_resolution_multiplier_makes_the_headline_price_unusable() {
        // fal-ai/nano-banana-pro. Reading the $0.15 would quote half the real
        // cost of a 4K image, on the button, before the user clicks it.
        let s = "Your request will cost **$0.15** per image. 2K and 4K outputs will be \
                 charged at **1.5** times and **2** times the standard rate, respectively.";
        assert_eq!(fal::parse_prose(Some(s)), None);
    }

    #[test]
    fn a_resolution_rate_table_written_as_a_sentence_is_refused() {
        // fal-ai/topaz/upscale/video. Three rates and an fps doubler; the first
        // figure is the cheapest of them.
        let s = "For every second a video your request will cost **$0.01** for up to \
                 **720p**, **$0.02** for **720p** to **1080p**, and **$0.08** for above \
                 **1080p** output. Price doubles for **60fps** output.";
        assert_eq!(fal::parse_prose(Some(s)), None);
    }

    #[test]
    fn a_style_conditional_per_image_rate_is_refused() {
        // fal-ai/recraft/v3: $0.04, or $0.08 in a vector style.
        let s = "Your request will cost **$0.04** per image (or **$0.08** if you are using \
                 a vector style). For $1 you can run this model approximately **25** times.";
        assert_eq!(fal::parse_prose(Some(s)), None);
    }

    #[test]
    fn dollars_and_cents_in_one_phrase_is_refused() {
        // fal-ai/flux-lora-portrait-trainer says "$0.0024 cents per step". The
        // unit is self-contradictory and there is a 1,000-step floor besides.
        let s = "Your request will cost **$0.0024 cents per step. A minimum of 1000 steps \
                 will be billed.**";
        assert_eq!(fal::parse_prose(Some(s)), None);
    }

    #[test]
    fn a_unit_we_cannot_bill_is_refused() {
        // pixelcut/video-background-removal charges per 30 frames; Billable has
        // no frame count, so there is no honest number to show.
        let s = "Your request will cost **$0.022** per **30** frames.";
        assert_eq!(fal::parse_prose(Some(s)), None);
    }

    #[test]
    fn a_trailing_conditional_after_per_image_is_refused() {
        // fal-ai/kling-image/o3: one dollar figure, and a doubling clause.
        let s = "Your request will cost **$0.028** per image for **1K/2K**, price will be \
                 double for **4K**";
        assert_eq!(fal::parse_prose(Some(s)), None);
    }

    #[test]
    fn a_model_with_no_price_prose_yields_nothing() {
        assert_eq!(fal::parse_prose(None), None);
        assert_eq!(fal::parse_prose(Some("")), None);
    }

    #[test]
    fn a_free_sounding_zero_is_refused_rather_than_believed() {
        assert_eq!(
            fal::parse_prose(Some("Your request will cost **$0.00** per image.")),
            None
        );
    }

    // -- fal family agreement --------------------------------------------

    #[test]
    fn a_family_whose_endpoints_disagree_gets_no_price() {
        // fal-ai/kling-video/v3/turbo really does resolve to /pro at $0.14/s
        // and /standard at $0.112/s. The route slug names neither, so quoting
        // either one is a coin flip with the user's money.
        let rows = vec![
            fal::Row {
                id: "fal-ai/fam/text-to-video".into(),
                pricing: Some(
                    "For every second of video you generate, you will be charged **$0.14**.".into(),
                ),
            },
            fal::Row {
                id: "fal-ai/fam/image-to-video".into(),
                pricing: Some(
                    "For every second of video you generate, you will be charged **$0.112**."
                        .into(),
                ),
            },
        ];
        let wanted = vec![Wanted {
            route_id: "fal:fal-ai/fam".into(),
            provider: ProviderId::Fal,
            slug: "fal-ai/fam".into(),
            modality: Modality::Video,
        }];
        assert!(fal::quotes(&rows, &wanted, 0).is_empty());
    }

    #[test]
    fn a_video_route_is_never_priced_from_the_image_endpoint_of_its_own_family() {
        // The `wan-25-preview` bug, found by diffing this feed against the
        // hand-transcribed registry. One fal root sells two products:
        // `/text-to-image` at $0.05 **per image** and `/text-to-video` at
        // $0.10 **per second** at 720p. Pooling the suffixes priced the video
        // route — which is what `wan2_5_video` routes to — at the image rate,
        // showing $0.05 on the button for a 5s clip that bills $0.50.
        let rows = vec![
            fal::Row {
                id: "fal-ai/wan-25-preview/text-to-image".into(),
                pricing: Some("Your request will cost **$0.05** per image.".into()),
            },
            fal::Row {
                id: "fal-ai/wan-25-preview/text-to-video".into(),
                pricing: Some(
                    "Your request will cost **$0.05** per second for **480p**, **$0.10** \
                     per second for **720p**, **$0.15** per second for **1080p**."
                        .into(),
                ),
            },
        ];
        let video = vec![Wanted {
            route_id: "fal:fal-ai/wan-25-preview".into(),
            provider: ProviderId::Fal,
            slug: "fal-ai/wan-25-preview".into(),
            modality: Modality::Video,
        }];
        assert!(
            fal::quotes(&rows, &video, 0).is_empty(),
            "a video route took the per-image rate"
        );

        // The image route of the same family is still priced, so the fix is a
        // modality filter and not a blanket refusal.
        let image = vec![Wanted {
            route_id: "fal:fal-ai/wan-25-preview".into(),
            provider: ProviderId::Fal,
            slug: "fal-ai/wan-25-preview".into(),
            modality: Modality::Image,
        }];
        assert_eq!(fal::quotes(&rows, &image, 0).len(), 1);
    }

    #[test]
    fn a_per_image_rate_can_never_describe_a_video_route() {
        // The backstop behind the modality filter: even if matching pairs a
        // route with the wrong endpoint again, the billing shape cannot fit.
        let per_image = CostModel::PerImage {
            usd: 0.05,
            usd_per_extra_input: 0.0,
        };
        assert!(!shape_fits(Modality::Video, &per_image));
        assert!(shape_fits(Modality::Image, &per_image));
        let per_second = CostModel::PerSecond {
            usd: 0.1,
            audio_multiplier: 1.0,
        };
        assert!(!shape_fits(Modality::Image, &per_second));
        assert!(shape_fits(Modality::Video, &per_second));
    }

    #[test]
    fn an_exact_slug_match_does_not_drag_in_a_sibling_model() {
        // `bytedance/seedance-2.0` and `bytedance/seedance-2.0-fast` are
        // different models at different rates. Prefix matching would blend them
        // and lose both.
        let ids = ["bytedance/seedance-2.0", "bytedance/seedance-2.0-fast"];
        let got = matches("bytedance/seedance-2.0", &ids, &VAIG_MODE_SUFFIXES);
        assert_eq!(got, vec!["bytedance/seedance-2.0"]);
    }

    #[test]
    fn a_family_root_collects_only_input_mode_siblings() {
        let ids = [
            "klingai/kling-v3.0-t2v",
            "klingai/kling-v3.0-i2v",
            // A different product, priced differently. Must not be collected.
            "klingai/kling-v3.0-motion-control",
        ];
        let got = matches("klingai/kling-v3.0", &ids, &VAIG_MODE_SUFFIXES);
        assert_eq!(
            got,
            vec!["klingai/kling-v3.0-t2v", "klingai/kling-v3.0-i2v"]
        );
    }

    // -- Vercel tier selection -------------------------------------------

    fn kling_v3_tiers() -> Vec<VideoTier> {
        // Verbatim from ai-gateway.vercel.sh/v1/models for klingai/kling-v3.0-t2v.
        let row = |mode: &str, audio: bool, vc: Option<bool>, usd: f64| VideoTier {
            resolution: None,
            mode: Some(mode.into()),
            audio: Some(audio),
            voice_control: vc,
            usd_per_second: usd,
        };
        vec![
            row("std", false, None, 0.168),
            row("std", true, None, 0.252),
            row("std", true, Some(true), 0.308),
            row("pro", false, None, 0.224),
            row("pro", true, None, 0.336),
            row("pro", true, Some(true), 0.392),
        ]
    }

    #[test]
    fn a_two_tier_table_with_no_tier_in_the_slug_is_refused() {
        // Kling 3.0 on Vercel is $0.168/s std and $0.224/s pro. `klingai/
        // kling-v3.0` says neither, so a quote would be a guess that understates
        // a pro job by a third.
        assert_eq!(
            select_video_tier(&kling_v3_tiers(), &Billable::video(5.0, 1280, 720), None),
            None
        );
    }

    #[test]
    fn a_slug_that_names_its_tier_resolves_it() {
        assert_eq!(
            select_video_tier(
                &kling_v3_tiers(),
                &Billable::video(5.0, 1280, 720),
                Some("std")
            ),
            Some(0.168)
        );
        assert_eq!(
            select_video_tier(
                &kling_v3_tiers(),
                &Billable::video(5.0, 1280, 720),
                Some("pro")
            ),
            Some(0.224)
        );
    }

    #[test]
    fn audio_selects_the_audio_row_and_does_not_also_multiply() {
        // The row is already the audio-on rate. Returning it alongside a 1.5x
        // multiplier would bill $0.378/s for a $0.252/s clip.
        let mut b = Billable::video(5.0, 1280, 720);
        b.audio = true;
        let q = Quote {
            route_id: "vaig:klingai/kling-v3.0-std".into(),
            feed: Feed::Vaig,
            origin: Origin::Live,
            fetched_at: 0,
            priced: Priced::VideoTiers {
                tiers: kling_v3_tiers(),
            },
            upstream: vec![],
            caveat: None,
        };
        assert_eq!(
            q.cost_model(&b, Some("std")),
            Some(CostModel::PerSecond {
                usd: 0.252,
                audio_multiplier: 1.0
            })
        );
    }

    #[test]
    fn a_voice_control_row_never_wins() {
        // We never request voice control and Billable cannot express it, so its
        // $0.308/s row must not be selectable.
        let mut b = Billable::video(5.0, 1280, 720);
        b.audio = true;
        assert_eq!(
            select_video_tier(&kling_v3_tiers(), &b, Some("std")),
            Some(0.252)
        );
    }

    #[test]
    fn resolution_picks_the_matching_row() {
        // Veo 3.1 Lite: 720p and 1080p are genuinely different rates.
        let tiers = vec![
            VideoTier {
                resolution: Some("720p".into()),
                mode: None,
                audio: Some(false),
                voice_control: None,
                usd_per_second: 0.03,
            },
            VideoTier {
                resolution: Some("1080p".into()),
                mode: None,
                audio: Some(false),
                voice_control: None,
                usd_per_second: 0.05,
            },
        ];
        assert_eq!(
            select_video_tier(&tiers, &Billable::video(5.0, 1280, 720), None),
            Some(0.03)
        );
        assert_eq!(
            select_video_tier(&tiers, &Billable::video(5.0, 1920, 1080), None),
            Some(0.05)
        );
    }

    #[test]
    fn a_resolution_the_provider_does_not_sell_is_not_rounded_to_one_it_does() {
        // 1280x544 is not "480p, near enough". Guessing the nearest tier is
        // guessing a price.
        let tiers = vec![VideoTier {
            resolution: Some("720p".into()),
            mode: None,
            audio: None,
            voice_control: None,
            usd_per_second: 0.1,
        }];
        assert_eq!(
            select_video_tier(&tiers, &Billable::video(5.0, 1280, 544), None),
            None
        );
    }

    #[test]
    fn portrait_video_reads_its_short_side_as_the_resolution() {
        // 1080x1920 is 1080p. Reading `height` would call it 1920 and refuse.
        let tiers = vec![VideoTier {
            resolution: Some("1080p".into()),
            mode: None,
            audio: None,
            voice_control: None,
            usd_per_second: 0.15,
        }];
        assert_eq!(
            select_video_tier(&tiers, &Billable::video(5.0, 1080, 1920), None),
            Some(0.15)
        );
    }

    #[test]
    fn duplicate_rows_at_the_same_rate_are_not_treated_as_ambiguous() {
        // minimax/minimax-h3 lists a "2k" row and an unconstrained row, both at
        // $0.13/s. Refusing on row count alone would drop a price we do know.
        let tiers = vec![
            VideoTier {
                resolution: Some("2k".into()),
                mode: None,
                audio: None,
                voice_control: None,
                usd_per_second: 0.13,
            },
            VideoTier {
                resolution: None,
                mode: None,
                audio: None,
                voice_control: None,
                usd_per_second: 0.13,
            },
        ];
        assert_eq!(
            select_video_tier(&tiers, &Billable::video(5.0, 2560, 1440), None),
            Some(0.13)
        );
    }

    #[test]
    fn an_unconstrained_row_is_a_fallback_not_an_audio_off_row() {
        let tiers = vec![VideoTier {
            resolution: Some("720p".into()),
            mode: None,
            audio: None,
            voice_control: None,
            usd_per_second: 0.1,
        }];
        let mut b = Billable::video(5.0, 1280, 720);
        b.audio = true;
        assert_eq!(select_video_tier(&tiers, &b, None), Some(0.1));
    }

    #[test]
    fn an_image_size_table_falls_back_to_the_providers_own_default_row() {
        // Billable::image(1) carries no dimensions. "default" is a value Google
        // published, not one we picked.
        let tiers = vec![
            ImageSizeTier {
                size: "1K".into(),
                usd: 0.1344,
            },
            ImageSizeTier {
                size: "4K".into(),
                usd: 0.24,
            },
            ImageSizeTier {
                size: "default".into(),
                usd: 0.1344,
            },
        ];
        assert_eq!(select_image_size(&tiers, &Billable::image(1)), Some(0.1344));
        let mut b = Billable::image(1);
        b.width = Some(4096);
        b.height = Some(4096);
        assert_eq!(select_image_size(&tiers, &b), Some(0.24));
    }

    #[test]
    fn an_image_size_table_with_no_default_and_no_dimensions_is_refused() {
        let tiers = vec![ImageSizeTier {
            size: "4K".into(),
            usd: 0.24,
        }];
        assert_eq!(select_image_size(&tiers, &Billable::image(1)), None);
    }

    // -- Vercel pricing objects ------------------------------------------

    #[test]
    fn language_token_pricing_is_not_turned_into_an_image_price() {
        // openai/gpt-image-2 bills on output tokens. Billable has no token
        // count, so registry.rs marks it Unknown by hand and this must agree —
        // an $0.00003 "per image" would be off by three orders of magnitude.
        let p: vaig::Pricing = serde_json::from_str(
            r#"{"input":"0.000005","output":"0.00003","input_cache_read":"0.00000125"}"#,
        )
        .unwrap();
        assert!(vaig::parse_pricing(Some(&p)).is_none());
    }

    #[test]
    fn an_empty_pricing_object_yields_no_quote() {
        // bfl/flux-2-pro ships `"pricing": {}` upstream.
        let p: vaig::Pricing = serde_json::from_str("{}").unwrap();
        assert!(vaig::parse_pricing(Some(&p)).is_none());
    }

    #[test]
    fn video_token_pricing_quotes_the_higher_rate_and_says_so() {
        // Seedance 2.0: $7.00/M without video input, $4.30/M with. We cannot
        // tell which applies, so we quote the dearer one and name the caveat
        // rather than quietly under-quoting.
        let p: vaig::Pricing = serde_json::from_str(
            r#"{"video_token_pricing":{"no_video_input":{"cost_per_million_tokens":"7"},
                 "with_video_input":{"cost_per_million_tokens":"4.3"}}}"#,
        )
        .unwrap();
        let (priced, caveat) = vaig::parse_pricing(Some(&p)).unwrap();
        assert_eq!(
            priced,
            Priced::Fixed {
                cost: CostModel::PerToken {
                    usd_per_million: 7.0,
                    fps: 24
                }
            }
        );
        assert!(caveat.unwrap().contains("over-states"));
    }

    #[test]
    fn a_video_input_rate_higher_than_the_base_rate_is_refused() {
        // Both live rows discount video input. If that ever inverted, quoting
        // the base rate would understate the bill, so refuse instead.
        let p: vaig::Pricing = serde_json::from_str(
            r#"{"video_token_pricing":{"no_video_input":{"cost_per_million_tokens":"4"},
                 "with_video_input":{"cost_per_million_tokens":"9"}}}"#,
        )
        .unwrap();
        assert!(vaig::parse_pricing(Some(&p)).is_none());
    }

    #[test]
    fn a_style_priced_separately_is_recorded_as_a_caveat_not_ignored() {
        // recraft/recraft-v4.1: $0.035 base, $0.08 for vector illustration.
        let p: vaig::Pricing = serde_json::from_str(
            r#"{"image":"0.035","image_dimension_quality_pricing":
                 [{"style":"vector_illustration","cost":"0.08"}]}"#,
        )
        .unwrap();
        let (priced, caveat) = vaig::parse_pricing(Some(&p)).unwrap();
        assert_eq!(
            priced,
            Priced::Fixed {
                cost: CostModel::PerImage {
                    usd: 0.035,
                    usd_per_extra_input: 0.0
                }
            }
        );
        assert!(caveat.unwrap().contains("vector_illustration"));
    }

    #[test]
    fn a_zero_cost_per_second_row_invalidates_the_whole_table() {
        let p: vaig::Pricing = serde_json::from_str(
            r#"{"video_duration_pricing":[{"resolution":"720p","cost_per_second":"0.1"},
                 {"resolution":"1080p","cost_per_second":"0"}]}"#,
        )
        .unwrap();
        assert!(vaig::parse_pricing(Some(&p)).is_none());
    }

    // -- the bot checkpoint ----------------------------------------------

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_vercel_challenge_is_recognised_not_reported_as_unreadable_json() {
        // Captured from https://fal.ai/models on 2026-08-05. Without this the
        // 33 KB challenge page reaches serde_json and the user is told the feed
        // "sent something unreadable" — hiding a condition we understand and
        // have a correct answer for.
        let body = "<!DOCTYPE html><html lang=\"en\"><head><title>Vercel Security \
                    Checkpoint</title></head><body></body></html>";
        assert!(is_bot_checkpoint(429, &headers(&[]), body));
        assert!(is_bot_checkpoint(
            429,
            &headers(&[("x-vercel-mitigated", "challenge")]),
            ""
        ));
        // Header-only: a challenge served with a 200 and an empty body still is
        // one, and the header is what the edge actually stamps.
        assert!(is_bot_checkpoint(
            200,
            &headers(&[("x-vercel-challenge-token", "2.178.60.abc")]),
            ""
        ));
    }

    #[test]
    fn a_plain_rate_limit_is_not_mistaken_for_a_challenge() {
        // A 429 clears by waiting; a challenge does not. Conflating them would
        // make us retry something that can never succeed.
        assert!(!is_bot_checkpoint(
            429,
            &headers(&[]),
            "{\"error\":\"slow down\"}"
        ));
    }

    #[test]
    fn a_normal_json_response_is_not_a_challenge() {
        assert!(!is_bot_checkpoint(200, &headers(&[]), "{\"items\":[]}"));
    }

    #[test]
    fn price_requests_are_browser_shaped() {
        // Insurance against fal widening its WAF rule to /api/*. Verified on
        // 2026-08-05 that a browser UA does NOT by itself defeat the existing
        // checkpoint on the HTML site — using the JSON API is what does.
        let c = reqwest::blocking::Client::new();
        let req = browser_get(&c, FAL_MODELS_URL).build().unwrap();
        let ua = req.headers().get("user-agent").unwrap().to_str().unwrap();
        assert!(ua.starts_with("Mozilla/5.0"), "not browser-shaped: {ua}");
        assert!(ua.contains("Chrome/"), "not browser-shaped: {ua}");
        assert!(req.headers().contains_key("accept-language"));
        assert!(req.headers().contains_key("sec-fetch-mode"));
    }

    #[test]
    fn the_fal_endpoint_is_the_json_api_never_the_html_page() {
        // https://fal.ai/models answers 429 with a challenge to every client we
        // can build; https://fal.ai/api/models answers 200. Scraping the page
        // is the mistake this constant exists to prevent.
        assert_eq!(FAL_MODELS_URL, "https://fal.ai/api/models");
        assert!(!FAL_MODELS_URL.ends_with("/models") || FAL_MODELS_URL.contains("/api/"));
    }

    // -- mode hint -------------------------------------------------------

    #[test]
    fn a_slug_naming_its_tier_produces_a_hint() {
        assert_eq!(mode_hint("fal:fal-ai/kling-video/v3/standard"), Some("std"));
        assert_eq!(mode_hint("vaig:klingai/kling-v2.6-standard"), Some("std"));
        assert_eq!(mode_hint("fal:fal-ai/kling-video/v2.6/pro"), Some("pro"));
        assert_eq!(mode_hint("vaig:klingai/kling-v3.0"), None);
    }

    // -- live feeds (network; excluded from the default run) --------------

    /// Run with `cargo test -p halation-core -- --ignored`. Kept out of the
    /// default run so `cargo test` stays offline and fast, and out of CI's
    /// required set so a provider outage cannot redden an unrelated PR.
    #[test]
    #[ignore = "hits the network"]
    fn live_feeds_answer_and_price_at_least_as_many_routes_as_the_bundle() {
        let live = PriceFeed::fetch();
        assert!(
            live.problems().is_empty(),
            "live fetch degraded: {:?}",
            live.problems()
        );
        assert!(
            live.len() >= PriceFeed::bundled().len(),
            "live fetch priced {} routes against the bundle's {}",
            live.len(),
            PriceFeed::bundled().len()
        );
        for q in live.quotes() {
            assert_eq!(
                q.origin,
                Origin::Live,
                "{} came from the bundle",
                q.route_id
            );
        }
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_fal_json_api_is_not_behind_the_checkpoint() {
        let c = reqwest::blocking::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap();
        let rows = fal::fetch(&c).expect("fal index");
        assert!(rows.len() > 500, "only {} models", rows.len());
    }

    #[test]
    #[ignore = "hits the network"]
    fn live_vaig_feed_is_unauthenticated_and_structured() {
        let c = reqwest::blocking::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .build()
            .unwrap();
        let rows = vaig::fetch(&c).expect("gateway model list");
        let priced = rows
            .iter()
            .filter(|r| vaig::parse_pricing(r.pricing.as_ref()).is_some())
            .count();
        assert!(priced > 20, "only {priced} models carried usable pricing");
    }
}
