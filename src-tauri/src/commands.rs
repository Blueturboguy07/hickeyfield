//! The command surface exposed to the webview.
//!
//! Two rules hold everywhere in this file. A secret never crosses the bridge —
//! the UI can learn *whether* a key is set, never what it is. And an unknown
//! price is returned as `null`, never as zero: telling someone a paid
//! generation is free is the worst thing this app could do.

use crate::app::AppState;
use crate::runner::now_secs;
use crate::vault::{self, KeyState};
use hickeyfield_core::clients::{detect_local, LocalEndpoints};
use hickeyfield_core::engine::{JobSet, JobStore};
use hickeyfield_core::{registry, Billable, Estimate, ProviderId, RoutePolicy};
use serde::{Deserialize, Serialize};
use tauri::State;

// ── Keys ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn key_states() -> Vec<KeyState> {
    ProviderId::ALL.into_iter().map(vault::state).collect()
}

#[tauri::command]
pub fn set_key(provider: String, secret_half: bool, value: String) -> Result<(), String> {
    let p =
        ProviderId::from_slug(&provider).ok_or_else(|| format!("unknown provider: {provider}"))?;
    vault::set(p, secret_half, &value)
}

#[tauri::command]
pub fn configured_providers() -> Vec<String> {
    ProviderId::ALL
        .into_iter()
        .filter(|p| vault::is_configured(*p))
        .map(|p| p.slug().to_string())
        .collect()
}

/// Can the user actually generate anything right now?
///
/// Deliberately not `configured_providers().is_empty()`. `Local` needs no key,
/// so it reports as configured on a machine with nothing installed — which
/// made the app look ready on first run and skip onboarding entirely. What
/// matters is whether any provider is *reachable*: a real key, or a local
/// endpoint that is genuinely answering.
#[tauri::command]
pub fn is_ready() -> bool {
    let has_key = ProviderId::ALL
        .into_iter()
        .filter(|p| p.needs_key())
        .any(vault::is_configured);
    has_key || detect_local().any()
}

/// Which local inference endpoints are up. Costs nothing to use and needs no
/// key — only reachable at all because we are native rather than in a browser.
#[tauri::command]
pub fn local_endpoints() -> LocalEndpoints {
    detect_local()
}

/// Everything the onboarding screen needs: where to get each key, what it
/// unlocks, and which one to start with.
#[tauri::command]
pub fn provider_info() -> Vec<hickeyfield_core::ProviderInfo> {
    hickeyfield_core::all_provider_info()
}

/// Make a real authenticated read against the provider. "Key is set" and "key
/// works" are different facts, and only the second one is useful.
#[tauri::command]
pub fn validate_key(provider: String) -> Result<crate::validate::Validation, String> {
    let p =
        ProviderId::from_slug(&provider).ok_or_else(|| format!("unknown provider: {provider}"))?;
    let key = vault::get(p, false).unwrap_or_default();
    let secret = vault::get(p, true);
    Ok(crate::validate::validate(p, &key, secret.as_deref()))
}

#[derive(Serialize)]
pub struct ImportReport {
    pub imported: Vec<String>,
    pub unknown: Vec<String>,
}

/// Parse a `.env` blob and store every credential it recognises.
///
/// The text never touches disk and is not logged — it is a bag of secrets that
/// happens to arrive as one string.
#[tauri::command]
pub fn import_env(text: String) -> Result<ImportReport, String> {
    let parsed = hickeyfield_core::parse_env(&text);
    let mut imported = Vec::new();
    for k in parsed.keys {
        let Some(p) = ProviderId::from_slug(&k.provider) else {
            continue;
        };
        vault::set(p, k.secret_half, &k.value)?;
        let name = p.display_name().to_string();
        if !imported.contains(&name) {
            imported.push(name);
        }
    }
    Ok(ImportReport {
        imported,
        unknown: parsed.unknown,
    })
}

// ── Catalogue ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RouteDto {
    pub id: String,
    pub provider: String,
    pub slug: String,
    pub note: Option<String>,
    /// True only when this route can actually be executed — the user holds a
    /// key **and** a client exists for the provider.
    ///
    /// Both halves matter. Reporting key-presence alone made the picker offer
    /// routes the resolver would then refuse, which is how a user could add a
    /// Vercel AI Gateway key and be told to add a Vercel AI Gateway key.
    pub available: bool,
    /// Why not, in words the picker can render. `None` when available.
    pub unavailable_reason: Option<String>,
}

/// A one-line description for the model picker.
///
/// Built from the model's own capabilities rather than written by hand for 68
/// models, so it cannot drift from what the model actually accepts. The picker
/// previously printed the raw slug here, which tells a user nothing.
fn model_subtitle(m: &hickeyfield_core::Model) -> Option<String> {
    let caps = m.spec.capabilities();
    let mut bits: Vec<String> = Vec::new();

    if caps.supports_duration {
        match caps.durations.as_slice() {
            [] => {
                if let Some(d) = caps.default_duration {
                    bits.push(format!("{}s default", trim_num(d)));
                }
            }
            [only] => bits.push(format!("{}s", trim_num(*only))),
            many => {
                let lo = many.iter().cloned().fold(f64::INFINITY, f64::min);
                let hi = many.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                bits.push(format!("{}–{}s", trim_num(lo), trim_num(hi)));
            }
        }
    }
    if let Some(best) = caps.resolutions.last() {
        bits.push(format!("up to {best}"));
    }
    if caps.audio {
        bits.push("audio".into());
    }
    // Say what the model *needs*, since a missing required input is the single
    // most common first-submit failure and the picker is the cheapest place to
    // prevent it.
    let required: Vec<&str> = m
        .spec
        .required_flags()
        .filter(|f| f.value.is_media())
        .map(|f| f.name.as_str())
        .collect();
    if !required.is_empty() {
        bits.push(format!("needs {}", required.join(" + ").replace('_', " ")));
    } else if m.spec.media_flags().next().is_some() {
        // No required input, but it accepts one — the distinction between a
        // pure generator and something that can also edit, which is exactly
        // what a user scanning the list wants to know.
        bits.push("text or image in".into());
    } else if m.spec.takes_prompt() {
        bits.push("text in".into());
    }

    if bits.is_empty() {
        // Last resort, and still better than the raw slug the picker used to
        // print: name what kind of thing it makes.
        return Some(format!("{} model", m.modality));
    }
    Some(bits.join(" · "))
}

/// `5` not `5.0`, `5.5` kept. Rust has no `{:g}`.
fn trim_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Explain a route's state once, so the picker and the resolver cannot disagree.
fn route_state(provider: ProviderId, slug: &str, configured: &[String]) -> (bool, Option<String>) {
    // Measured absent from the provider entirely. Offering it spends a click
    // and a round trip to learn what we already know — which is how
    // `fal-ai/wan/v2.6` reached a user as `404 Path /v2.6/video-to-video`.
    if provider == ProviderId::Fal && hickeyfield_core::media::route_is_missing(slug) {
        return (
            false,
            Some("fal does not serve this model — try another route".to_string()),
        );
    }
    if !provider.has_adapter() {
        return (
            false,
            Some(format!(
                "Hickeyfield has no client for {} yet",
                provider.display_name()
            )),
        );
    }
    if provider.needs_key() && !configured.iter().any(|c| c == provider.slug()) {
        return (
            false,
            Some(format!("Needs a {} key", provider.display_name())),
        );
    }
    (true, None)
}

/// The same question, narrowed to one job.
///
/// A route can be perfectly configured and still be the wrong way to do *this*
/// piece of work: `fal-ai/veo3.1` takes no media, so under Animate Image it is
/// unusable even though the model belongs there — its Higgsfield sibling
/// `/veo3.1/image-to-video` is what serves that tab. Marking it here is what
/// stops the picker handing over a default route that fails at submit.
fn route_state_for_job(
    model: &hickeyfield_core::Model,
    route: &hickeyfield_core::route::Route,
    use_case: Option<hickeyfield_core::UseCase>,
    configured: &[String],
) -> (bool, Option<String>) {
    let (available, why) = route_state(route.provider, &route.slug, configured);
    if !available {
        return (available, why);
    }
    match use_case {
        Some(uc) if !hickeyfield_core::use_case::route_serves(model, route, uc) => (
            false,
            Some(format!(
                "{} cannot do this job — pick another route",
                route.provider.display_name()
            )),
        ),
        _ => (true, None),
    }
}

/// Per-model option sets, derived from the model's own declared flags.
///
/// `supports_*` and the list are separate facts: an empty list with support
/// true means the model takes a free value, and the UI must offer an input
/// rather than invent choices. See `catalog::Capabilities`.
#[derive(Serialize)]
pub struct CapabilitiesDto {
    pub supports_duration: bool,
    pub durations: Vec<f64>,
    pub default_duration: Option<f64>,
    pub supports_resolution: bool,
    pub resolutions: Vec<String>,
    pub default_resolution: Option<String>,
    pub supports_aspect: bool,
    pub aspects: Vec<String>,
    pub default_aspect: Option<String>,
    pub audio: bool,
    pub constraints: Vec<String>,
    /// Media roles this model cannot take **on this route**, by wire name.
    ///
    /// The slot list comes from the use case, so every image-to-video tab draws
    /// an End Frame box whether or not the chosen model has anywhere to put
    /// one. Seedance 2.0 accepted the file, ignored it, and billed in full.
    /// The submit path refuses that now, but refusing after the user has
    /// composed the whole shot is the wrong moment — this greys the slot out
    /// before it is ever filled.
    pub unsupported_roles: Vec<String>,
}

impl CapabilitiesDto {
    /// Build from the authoritative description.
    ///
    /// `Support::No` becomes `supports_* = false`, which hides the control.
    /// `Support::Unknown` also renders nothing — the predecessor of this type
    /// filled unknowns with plausible defaults and that is precisely how the
    /// chip row came to offer 10s on a 5s-only model.
    fn from_capability(c: &hickeyfield_core::capability::ModelCapability) -> Self {
        use hickeyfield_core::capability::Axis;
        let num = |a: &Axis| -> Vec<f64> {
            a.values
                .iter()
                .filter_map(|v| v.trim().trim_end_matches('s').trim().parse::<f64>().ok())
                .collect()
        };
        CapabilitiesDto {
            supports_duration: c.duration.support.is_yes(),
            durations: num(&c.duration),
            default_duration: c
                .duration_seconds()
                .and_then(|_| c.default_duration_seconds()),
            supports_resolution: c.resolution.support.is_yes(),
            resolutions: c.resolution.values.clone(),
            default_resolution: c.resolution.default.clone(),
            supports_aspect: c.aspect.support.is_yes(),
            aspects: c.aspect.values.clone(),
            default_aspect: c.aspect.default.clone(),
            audio: c.audio_output.support.is_yes(),
            constraints: c.constraints.clone(),
            // Needs the route, which this conversion does not have. Filled by
            // `model_capabilities`.
            unsupported_roles: Vec::new(),
        }
    }
}

impl From<hickeyfield_core::catalog::Capabilities> for CapabilitiesDto {
    fn from(c: hickeyfield_core::catalog::Capabilities) -> Self {
        CapabilitiesDto {
            supports_duration: c.supports_duration,
            durations: c.durations,
            default_duration: c.default_duration,
            supports_resolution: c.supports_resolution,
            resolutions: c.resolutions,
            default_resolution: c.default_resolution,
            supports_aspect: c.supports_aspect,
            aspects: c.aspects,
            default_aspect: c.default_aspect,
            audio: c.audio,
            constraints: c.constraints,
            // Model-level view, no route chosen yet: nothing can be ruled out.
            unsupported_roles: Vec::new(),
        }
    }
}

#[derive(Serialize)]
pub struct ModelDto {
    pub id: String,
    pub display_name: String,
    pub modality: String,
    /// What this model can be asked for. Never assumed — see `CapabilitiesDto`.
    pub capabilities: CapabilitiesDto,
    /// One-line description for the picker row. Previously the picker fell back
    /// to printing the raw slug, which is not a description.
    pub subtitle: Option<String>,
    pub routes: Vec<RouteDto>,
    pub is_launch: bool,
    pub takes_prompt: bool,
}

/// The use cases the workspace tabs offer.
#[derive(Serialize)]
pub struct UseCaseDto {
    pub slug: String,
    pub label: String,
    pub blurb: String,
    /// `role` plus whether the job cannot run without it.
    pub slots: Vec<(String, bool)>,
    pub requires_media: bool,
}

#[tauri::command]
pub fn list_use_cases() -> Vec<UseCaseDto> {
    hickeyfield_core::UseCase::ALL
        .into_iter()
        .map(|u| UseCaseDto {
            slug: u.slug().to_string(),
            label: u.label().to_string(),
            blurb: u.blurb().to_string(),
            slots: u
                .slots()
                .iter()
                .map(|(role, req)| {
                    (
                        serde_json::to_value(role)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_default(),
                        *req,
                    )
                })
                .collect(),
            requires_media: u.requires_media(),
        })
        .collect()
}

#[tauri::command]
pub fn list_models() -> Vec<ModelDto> {
    models_matching(None)
}

/// Models that can do one job.
///
/// The filter lives here rather than in the UI so the picker cannot offer
/// something the submit path would then refuse — which is what happened when
/// every tab showed every model.
#[tauri::command]
pub fn models_for_use_case(use_case: String) -> Vec<ModelDto> {
    models_matching(hickeyfield_core::UseCase::from_slug(&use_case))
}

fn models_matching(use_case: Option<hickeyfield_core::UseCase>) -> Vec<ModelDto> {
    let configured = configured_providers();
    let mut models: Vec<ModelDto> = registry()
        .into_values()
        .filter(|m| match use_case {
            Some(uc) => hickeyfield_core::use_case::supports(m, uc),
            None => true,
        })
        .map(|m| ModelDto {
            id: m.id.clone(),
            display_name: m.display_name.clone(),
            modality: m.modality.to_string(),
            capabilities: m.spec.capabilities().into(),
            subtitle: model_subtitle(&m),
            takes_prompt: m.spec.takes_prompt(),
            is_launch: m.launch,
            routes: m
                .routes
                .iter()
                .map(|r| {
                    let (available, unavailable_reason) =
                        route_state_for_job(&m, r, use_case, &configured);
                    RouteDto {
                        id: r.id(),
                        provider: r.provider.slug().to_string(),
                        slug: r.slug.clone(),
                        note: r.note.clone(),
                        available,
                        unavailable_reason,
                    }
                })
                .collect(),
        })
        .collect();
    // Launch models first, then alphabetical — a 57-item flat list sorted only
    // by id buries the twelve models most people want.
    models.sort_by(|a, b| {
        b.is_launch
            .cmp(&a.is_launch)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    models
}

#[derive(Serialize)]
pub struct PresetDto {
    pub id: String,
    pub display_name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub description: String,
}

/// The camera-control presets, which are the reproducible half of the catalog.
///
/// Higgsfield's other ~244 presets expand to prompt text only on their server —
/// nothing but a UUID goes over the wire — so those bodies have to be authored
/// rather than derived. The camera moves are different: their own five-slot
/// schema is public, so these are real and complete today.
#[tauri::command]
pub fn list_presets() -> Vec<PresetDto> {
    hickeyfield_core::camera::slugs()
        .filter_map(hickeyfield_core::camera::get)
        .map(|t| PresetDto {
            id: t.slug.to_string(),
            display_name: t.display_name.to_string(),
            category: "camera-control".to_string(),
            tags: vec![],
            description: t.render(),
        })
        .collect()
}

/// The authoritative capabilities for one model on one route.
///
/// Separate from `list_models` on purpose. Answering this properly means asking
/// fal for the endpoint schema, and doing that for all 68 models to draw a
/// picker would be 68 blocking HTTP requests. So the list stays
/// catalogue-sourced and fast, and this refines a single model at the moment it
/// is selected — which is exactly when the answer starts to matter.
#[tauri::command]
pub fn model_capabilities(
    model_id: String,
    route_id: Option<String>,
    #[allow(non_snake_case)] hasMedia: Option<bool>,
) -> Result<CapabilitiesDto, String> {
    use hickeyfield_core::media::InputMode;

    let reg = registry();
    let model = reg
        .get(&model_id)
        .ok_or_else(|| format!("unknown model: {model_id}"))?;

    let route = route_id
        .as_deref()
        .and_then(|id| model.routes.iter().find(|r| r.id() == id))
        .or_else(|| model.routes.first())
        .ok_or_else(|| format!("{model_id} has no routes"))?;

    // The mode changes which endpoint answers, and the endpoints differ: Kling
    // 3.0's image-to-video variant has no `duration` field while its
    // text-to-video one does.
    let mode = if hasMedia.unwrap_or(false) {
        InputMode::Image
    } else {
        InputMode::Text
    };

    let cap = hickeyfield_core::capability::for_route(
        &model.spec,
        route.provider,
        &route.slug,
        mode,
        hickeyfield_core::fal_schema::for_endpoint,
    );
    let mut dto = CapabilitiesDto::from_capability(&cap);

    // Which slots this model+route has nowhere to put. Asked of the same
    // function the submit path binds with, so the picker and the wire cannot
    // disagree — the End Frame box was drawn from the use case alone, and
    // Seedance 2.0 took the file, ignored it and charged for the render.
    let dialect = hickeyfield_core::media::dialect_for(route.provider, &route.slug);
    dto.unsupported_roles = hickeyfield_core::media::MediaRole::ALL
        .iter()
        .filter(|role| {
            !hickeyfield_core::media::can_bind(&model.spec, **role, dialect, &route.slug)
        })
        .map(|role| role.slug().to_string())
        .collect();
    Ok(dto)
}

// ── Cost ───────────────────────────────────────────────────────────────────

/// Generation settings **in the vocabulary the UI actually speaks**.
///
/// This used to be `{seconds, width, height, fps, images}` while the UI sent
/// `{duration, resolution, aspect}` — not a naming mismatch but two different
/// models of the same thing. Nothing lined up, so `seconds` was always `None`,
/// `Billable` had no duration, and **every cost estimate came out as $0.00**.
/// A confidently wrong price on the Generate button is the worst defect this
/// codebase can ship, so the wire shape now matches what the UI knows and the
/// pixel arithmetic lives here, where the model knowledge already is.
// Serialize is derived rather than hand-written: the previous manual impl
// listed the field names a second time, which is precisely how it kept
// serialising `seconds`/`width`/`height` after the struct had moved on.
#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    /// Seconds. Absent on models with no duration axis.
    pub duration: Option<f64>,
    /// The model's own vocabulary: `720p`, `1080p`, `4k`, `1k`, `2k`.
    pub resolution: Option<String>,
    /// `16:9`, `9:16`, `1:1`, …
    pub aspect: Option<String>,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub enhance: bool,
    pub seed: Option<i64>,
    pub steps: Option<u32>,
    #[serde(default = "one_u32")]
    pub batch: u32,
    #[serde(default)]
    pub extra_inputs: u32,
}

/// Pixel height for a resolution label.
///
/// Two vocabularies are in play and both are real: video models say `720p`,
/// image models say `2k`. Returning `None` for anything unrecognised is
/// deliberate — a guessed resolution becomes a guessed megapixel count becomes
/// a wrong price.
fn resolution_height(label: &str) -> Option<u32> {
    let l = label.trim().to_ascii_lowercase();
    match l.as_str() {
        "480p" => Some(480),
        "540p" => Some(540),
        "720p" => Some(720),
        "1080p" => Some(1080),
        "1440p" => Some(1440),
        "2160p" | "4k" => Some(2160),
        "1k" => Some(1024),
        "2k" => Some(1440),
        _ => None,
    }
}

/// Width from a height and an aspect label like `16:9`.
fn width_for(height: u32, aspect: Option<&str>) -> Option<u32> {
    let a = aspect?.trim();
    let (w, h) = a.split_once(':')?;
    let (w, h): (f64, f64) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    if h <= 0.0 || w <= 0.0 {
        return None;
    }
    // Rounded to an even number: encoders reject odd dimensions, and a price
    // computed on a size the provider will not accept is meaningless anyway.
    let px = (height as f64 * w / h).round() as u32;
    Some(px + (px % 2))
}

fn one_u32() -> u32 {
    1
}

impl From<&SettingsDto> for Billable {
    fn from(s: &SettingsDto) -> Self {
        let height = s.resolution.as_deref().and_then(resolution_height);
        let width = height.and_then(|h| width_for(h, s.aspect.as_deref()));
        // Only when both dimensions are known. Several models price per
        // megapixel, and half a dimension gives a number that looks right and
        // is not.
        let megapixels = match (width, height) {
            (Some(w), Some(h)) => Some((w as f64 * h as f64) / 1_000_000.0),
            _ => None,
        };
        Billable {
            seconds: s.duration,
            width,
            height,
            fps: None,
            // An image request is one image per batch slot; video models ignore
            // this field entirely.
            images: Some(s.batch.max(1)),
            megapixels,
            audio: s.audio,
            batch: s.batch.max(1),
            extra_inputs: s.extra_inputs,
        }
    }
}

/// `null` means the provider publishes no price for this call. The UI must
/// render that as "price unavailable" — never as free.
#[tauri::command]
pub fn estimate_cost(
    state: State<'_, AppState>,
    model_id: String,
    route_id: String,
    settings: SettingsDto,
) -> Option<Estimate> {
    estimate_with(&state.prices, &model_id, &route_id, &settings)
}

/// The body of [`estimate_cost`], without the Tauri handle.
///
/// Split out so the cost path stays testable without standing up an app: the
/// invariant worth guarding here — unknown is `None`, never `0.0` — should not
/// depend on a harness a future edit would delete rather than fix.
fn estimate_with(
    prices: &crate::pricing::Prices,
    model_id: &str,
    route_id: &str,
    settings: &SettingsDto,
) -> Option<Estimate> {
    let reg = registry();
    let model = reg.get(model_id)?;
    let route = model.route(route_id)?;
    prices.estimate(model, route, &Billable::from(settings))
}

/// Where the prices on screen came from and how old they are.
///
/// Deliberately exposed. A number with no provenance is the kind of thing a
/// user only checks after it has already been wrong.
#[tauri::command]
pub fn price_status(state: State<'_, AppState>) -> crate::pricing::PriceStatus {
    state.prices.status()
}

// ── Jobs ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitInput {
    pub model_id: String,
    /// Omitted means "resolve for me", which applies the cheapest-route policy.
    pub route_id: Option<String>,
    pub prompt: String,
    pub preset_id: Option<String>,
    #[serde(default)]
    pub settings: SettingsDto,
    /// Attached inputs, as roles rather than wire flags. Without this the app
    /// can only do text-to-video, which is a different and much smaller
    /// product than the one being cloned.
    #[serde(default)]
    pub media: Vec<hickeyfield_core::MediaRef>,
    /// Ollama tag to rewrite with. `None` sends the prompt as written.
    #[serde(default)]
    pub rewriter: Option<String>,
}

#[tauri::command]
pub fn submit_job(state: State<'_, AppState>, input: SubmitInput) -> Result<String, String> {
    let reg = registry();
    let model = reg
        .get(&input.model_id)
        .ok_or_else(|| format!("unknown model: {}", input.model_id))?;

    let available: Vec<ProviderId> = configured_providers()
        .iter()
        .filter_map(|s| ProviderId::from_slug(s))
        .collect();

    // The harness. Resolves the preset, appends its camera clause, applies the
    // three enhance rules, and — when asked and able — rewrites the scene
    // through the filmmaking corpus. Runs *before* pricing and routing because
    // a refusal here must cost nothing.
    let compiled = crate::harness::compile(
        model,
        &input.prompt,
        input.preset_id.as_deref(),
        &input.media,
        input.settings.enhance,
        match input.rewriter.as_deref() {
            Some(tag) if !tag.trim().is_empty() => crate::harness::Rewriter::Ollama { model: tag },
            _ => crate::harness::Rewriter::None,
        },
    )?;

    let billable = Billable::from(&input.settings);
    let route = hickeyfield_core::route::resolve(
        &model.routes,
        &available,
        RoutePolicy::Cheapest,
        input.route_id.as_deref(),
        |r| state.prices.usd(model, r, &billable),
    )
    .map_err(|e| e.to_string())?;

    // The id is ours, not the provider's: a submit that fails still leaves a
    // visible row explaining why, rather than vanishing.
    let id = format!("{}-{}", now_secs(), model.id);
    let mut job = JobSet {
        id: id.clone(),
        model_id: model.id.clone(),
        route_id: route.id(),
        request_id: String::new(),
        endpoint: String::new(),
        status: hickeyfield_core::JobStatus::Queued,
        // The user's own words are stored as the prompt; the compiled and
        // rewritten string is what goes over the wire. Keeping both is what
        // lets the meta rail show the second chip honestly.
        prompt: compiled.original.clone(),
        enhanced_prompt: compiled.enhanced.clone(),
        enhancer_version: compiled.version.clone(),
        enhance_note: compiled.note.clone(),
        advisories: Vec::new(),
        preset_id: input.preset_id.clone(),
        created_at: now_secs(),
        updated_at: now_secs(),
        results: vec![],
        estimated_usd: state.prices.usd(model, route, &billable),
        actual_usd: None,
        fail_reason: None,
        settings: serde_json::to_value(&input.settings).unwrap_or(serde_json::Value::Null),
        // Persisted so Rerun repeats the generation the user actually ran.
        media: input.media.clone(),
    };
    state.store.upsert(&job).map_err(|e| e.to_string())?;

    match crate::app::submit_to_provider(&mut job, model, route, &input.media, &compiled.prompt) {
        Ok(request_id) => {
            job.request_id = request_id;
            job.updated_at = now_secs();
            state.store.upsert(&job).map_err(|e| e.to_string())?;
            state.runner.watch(job);
            Ok(id)
        }
        Err(e) => {
            job.status = hickeyfield_core::JobStatus::Failed;
            job.fail_reason = Some(e.to_string());
            job.updated_at = now_secs();
            let _ = state.store.upsert(&job);
            Err(e.to_string())
        }
    }
}

#[tauri::command]
pub fn list_jobs(state: State<'_, AppState>) -> Result<Vec<JobSet>, String> {
    state.store.all().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job_set_id: String) -> Result<(), String> {
    state.runner.cancel(&job_set_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_job(
    state: State<'_, AppState>,
    job_set_id: String,
    #[allow(non_snake_case)] deleteFiles: Option<bool>,
) -> Result<(), String> {
    // Tombstone *first*. Deleting the row while a poll loop is live let the
    // next tick write it straight back, so the row reappeared seconds later.
    state.runner.forget(&job_set_id);

    // Only remove files if asked. A generation costs real money, and a row
    // disappearing from a list is a much smaller act than shredding the asset —
    // so the destructive half is opt-in and the caller has to say so.
    if deleteFiles.unwrap_or(false) {
        if let Ok(Some(job)) = state.store.get(&job_set_id) {
            for out in &job.results {
                if let Some(p) = &out.local_path {
                    // Best effort: a file the user already moved or deleted
                    // must not block removing the row.
                    if let Err(e) = std::fs::remove_file(p) {
                        tracing::warn!("could not delete {p}: {e}");
                    }
                }
            }
        }
    }

    state.store.delete(&job_set_id).map_err(|e| e.to_string())
}

/// Show a generated file in Finder or Explorer.
///
/// Scoped to the library root so a crafted path from the webview cannot ask
/// the OS to reveal something elsewhere on disk.
#[tauri::command]
pub fn reveal_result(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let root = state.library.root().to_path_buf();
    let target = std::path::PathBuf::from(&path);
    let canonical = target
        .canonicalize()
        .map_err(|e| format!("could not find {path}: {e}"))?;
    if !canonical.starts_with(&root) {
        return Err("that file is not in the Hickeyfield library".into());
    }
    tauri_plugin_opener::reveal_item_in_dir(&canonical).map_err(|e| e.to_string())
}

/// Let the webview display files the user just picked in the native dialog.
///
/// The asset protocol is scoped to the library, deliberately — the webview has
/// no business reading arbitrary disk. But an attachment lives wherever the
/// user keeps it, so without a grant the settings rail can only show a filename
/// where a thumbnail belongs, and nothing in the app can measure the shape of
/// an input it is about to animate.
///
/// The grant is per file and only ever for a path that came back from the OS
/// file dialog, which is the user pointing at it as directly as the platform
/// allows. Nothing here accepts a path the webview invented: a caller passing a
/// path the user did not choose is granting access to something it already had
/// the path to, and the dialog is the only thing that produces these.
#[tauri::command]
pub fn allow_media_preview(app: tauri::AppHandle, paths: Vec<String>) -> Result<(), String> {
    use tauri::Manager;
    for p in &paths {
        let path = std::path::Path::new(p);
        // Canonicalise first: a grant for a path with `..` in it is a grant for
        // wherever that resolves, which is not what the caller asked for and
        // not what the user picked.
        let real = path
            .canonicalize()
            .map_err(|e| format!("could not resolve {p}: {e}"))?;
        if !real.is_file() {
            return Err(format!("{p} is not a file"));
        }
        app.asset_protocol_scope()
            .allow_file(&real)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Ids the runner is actively polling right now.
///
/// Distinct from "status is in_progress": a row can be non-terminal in the
/// database while nothing is watching it, which is exactly what a crash leaves
/// behind. The UI needs to tell those apart to offer a resume.
#[tauri::command]
pub fn watching_jobs(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state
        .store
        .all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|j| state.runner.is_watching(&j.id))
        .map(|j| j.id)
        .collect())
}

#[tauri::command]
pub fn library_root(state: State<'_, AppState>) -> String {
    state.library.root().display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_exposed_and_launch_models_come_first() {
        let models = list_models();
        assert!(models.len() >= 55, "got {}", models.len());

        let first_non_launch = models
            .iter()
            .position(|m| !m.is_launch)
            .unwrap_or(models.len());
        assert!(
            models[..first_non_launch].iter().all(|m| m.is_launch),
            "launch models must be contiguous at the top"
        );
        assert!(first_non_launch >= 12, "expected at least 12 launch models");
    }

    #[test]
    fn every_model_offers_at_least_one_route() {
        for m in list_models() {
            assert!(!m.routes.is_empty(), "{} has no route", m.id);
        }
    }

    #[test]
    fn an_unknown_price_is_null_not_zero() {
        // The single most dangerous possible bug in this file.
        let got = estimate_with(
            &crate::pricing::Prices::bundled(),
            "definitely-not-a-model",
            "fal:whatever",
            &SettingsDto::default(),
        );
        assert!(got.is_none());
    }

    #[test]
    fn a_known_model_and_route_quotes_a_positive_price() {
        let models = list_models();
        let m = models
            .iter()
            .find(|m| m.is_launch && m.modality == "video")
            .expect("a launch video model");
        let route = &m.routes[0];

        let est = estimate_with(
            &crate::pricing::Prices::bundled(),
            &m.id,
            &route.id,
            &SettingsDto {
                duration: Some(8.0),
                resolution: Some("720p".into()),
                aspect: Some("16:9".into()),
                batch: 1,
                ..Default::default()
            },
        );
        if let Some(e) = est {
            assert!(e.usd > 0.0, "{} quoted {}", m.id, e.usd);
            assert!(!e.basis.is_empty(), "an estimate must explain itself");
        }
    }

    #[test]
    fn the_ui_vocabulary_becomes_real_pixels() {
        // The defect this replaces: SettingsDto spoke {seconds,width,height}
        // while the UI sent {duration,resolution,aspect}, so nothing
        // deserialised, `seconds` was always None, and every estimate came out
        // as $0.00 — a confidently wrong price on the Generate button.
        let s = SettingsDto {
            duration: Some(8.0),
            resolution: Some("720p".into()),
            aspect: Some("16:9".into()),
            batch: 1,
            ..Default::default()
        };
        let b = Billable::from(&s);
        assert_eq!(b.seconds, Some(8.0));
        assert_eq!(b.height, Some(720));
        assert_eq!(b.width, Some(1280));
        assert!(b.megapixels.is_some());
    }

    #[test]
    fn the_wire_shape_matches_what_the_ui_actually_sends() {
        // Guards the camelCase rename and the field names together. If either
        // drifts, this fails instead of silently producing a zero-cost job.
        let json = r#"{
            "modelId": "kling3_0",
            "prompt": "hi",
            "presetId": null,
            "settings": {"duration": 5, "resolution": "1080p", "aspect": "9:16",
                         "audio": false, "enhance": true},
            "media": []
        }"#;
        let parsed: SubmitInput = serde_json::from_str(json).expect("UI payload must deserialise");
        assert_eq!(parsed.model_id, "kling3_0");
        assert_eq!(parsed.settings.duration, Some(5.0));
        let b = Billable::from(&parsed.settings);
        assert_eq!(b.height, Some(1080));
        // 9:16 portrait — width is the short edge.
        assert_eq!(b.width, Some(608));
    }

    #[test]
    fn an_unrecognised_resolution_yields_no_pixels_rather_than_a_guess() {
        // A guessed resolution becomes a guessed megapixel count becomes a
        // wrong price.
        assert_eq!(resolution_height("ultra"), None);
        let s = SettingsDto {
            resolution: Some("ultra".into()),
            aspect: Some("16:9".into()),
            ..Default::default()
        };
        let b = Billable::from(&s);
        assert!(b.width.is_none() && b.height.is_none() && b.megapixels.is_none());
    }

    #[test]
    fn both_resolution_vocabularies_are_understood() {
        // Video models say 720p; image models say 2k. Both are real.
        assert_eq!(resolution_height("1080p"), Some(1080));
        assert_eq!(resolution_height("4K"), Some(2160));
        assert_eq!(resolution_height("2k"), Some(1440));
    }

    #[test]
    fn derived_widths_are_even() {
        // Encoders reject odd dimensions, so a price computed on one is
        // meaningless anyway.
        for aspect in ["16:9", "9:16", "1:1", "21:9", "4:3"] {
            for h in [480u32, 720, 1080, 2160] {
                let w = width_for(h, Some(aspect)).unwrap();
                assert_eq!(w % 2, 0, "{aspect} at {h} gave odd width {w}");
            }
        }
    }

    #[test]
    fn presets_are_exposed_with_their_categories() {
        let presets = list_presets();
        assert!(!presets.is_empty());
        assert!(presets.iter().all(|p| !p.category.is_empty()));
    }

    #[test]
    fn readiness_ignores_providers_that_need_no_key() {
        // The first-run bug this exists to prevent: `Local` reports as
        // configured because it needs no credential, so a naive
        // "any provider configured?" check was always true and onboarding
        // never opened on a fresh machine.
        let by_key = ProviderId::ALL
            .into_iter()
            .filter(|p| p.needs_key())
            .any(vault::is_configured);
        let naive = !configured_providers().is_empty();
        assert!(
            naive,
            "Local alone should still make the naive check true — that is the trap"
        );
        // is_ready may be true via a detected local endpoint, but it must never
        // be true *purely* because Local exists as a concept.
        if !by_key && !detect_local().any() {
            assert!(
                !is_ready(),
                "a machine with no keys and nothing local is not ready"
            );
        }
    }

    #[test]
    fn key_states_never_carry_a_secret() {
        // Serialised and inspected, because the guarantee is about the wire
        // shape rather than the Rust type.
        let json = serde_json::to_string(&key_states()).unwrap();
        for leak in ["password", "secret_value", "api_key\":\""] {
            assert!(!json.contains(leak), "possible key leak: {leak}");
        }
        assert!(json.contains("has_key"));
    }

    #[test]
    fn routes_report_availability_so_the_ui_can_explain_itself() {
        // A route the user cannot reach is shown and marked, not hidden — the
        // point is to tell them which key would unlock it.
        let models = list_models();
        let any_route = models.iter().flat_map(|m| &m.routes).next().unwrap();
        assert!(!any_route.provider.is_empty());
        assert!(any_route.id.contains(':'));
    }

    #[test]
    fn a_subtitle_describes_the_model_rather_than_repeating_its_slug() {
        let reg = hickeyfield_core::registry::registry();
        let sd = model_subtitle(&reg["seedance_2_0"]).unwrap();
        assert!(sd.contains("up to 4k"), "got: {sd}");
        assert!(!sd.contains("fal-ai"), "subtitle must not be a slug: {sd}");
    }

    #[test]
    fn most_models_get_a_subtitle() {
        // If this collapses, the picker is back to showing raw slugs.
        let reg = hickeyfield_core::registry::registry();
        let with = reg.values().filter(|m| model_subtitle(m).is_some()).count();
        assert_eq!(with, reg.len(), "every model must describe itself");
        // And none of them may fall back to printing a slug.
        for m in reg.values() {
            let s = model_subtitle(m).unwrap();
            assert!(!s.contains('/'), "{} subtitle looks like a slug: {s}", m.id);
        }
    }

    #[test]
    fn a_model_that_needs_an_image_says_so() {
        // The single most common first-submit failure is a missing required
        // input, and the picker is where it is cheapest to prevent.
        let reg = hickeyfield_core::registry::registry();
        let needy: Vec<_> = reg
            .values()
            .filter(|m| model_subtitle(m).is_some_and(|s| s.contains("needs ")))
            .collect();
        assert!(!needy.is_empty(), "no model advertises a required input");
    }

    #[test]
    fn durations_render_without_a_trailing_zero() {
        assert_eq!(trim_num(5.0), "5");
        assert_eq!(trim_num(5.5), "5.5");
    }

    #[test]
    fn a_key_without_a_client_is_not_reported_as_available() {
        // The picker must not offer what the resolver will refuse.
        let configured = vec!["vaig".to_string()];
        let (ok, why) = route_state(ProviderId::Vaig, "whatever", &configured);
        assert!(!ok);
        let why = why.unwrap();
        assert!(why.contains("no client"), "got: {why}");
        // And it must not tell them to add the key they just added.
        assert!(!why.contains("Needs a"), "got: {why}");
    }

    #[test]
    fn a_model_the_provider_does_not_serve_is_greyed_out_not_offered() {
        // The bug this exists to prevent, reported by a user: Wan 2.6 was
        // selectable, priced at $0.50, and produced
        // `404 Path /v2.6/video-to-video not found` — a click and a round trip
        // to learn something the probe already knew.
        let (ok, why) = route_state(ProviderId::Fal, "fal-ai/wan/v2.6", &["fal".to_string()]);
        assert!(!ok);
        let why = why.unwrap();
        assert!(why.contains("does not serve"), "got: {why}");
        // Not a key problem — do not send them to Settings.
        assert!(!why.contains("Needs a"), "got: {why}");
    }

    #[test]
    fn the_measured_missing_routes_are_all_real_registry_routes() {
        // A stale entry here silently greys out a model that works.
        let reg = registry();
        let known: std::collections::HashSet<&str> = reg
            .values()
            .flat_map(|m| m.routes.iter())
            .map(|r| r.slug.as_str())
            .collect();
        for slug in hickeyfield_core::media::FAL_MISSING_ROUTES {
            assert!(
                known.contains(slug),
                "{slug} is not a route in the registry"
            );
        }
    }

    #[test]
    fn a_missing_key_says_which_key() {
        let (ok, why) = route_state(ProviderId::Fal, "fal-ai/nano-banana-2", &[]);
        assert!(!ok);
        assert_eq!(why.unwrap(), "Needs a fal.ai key");
    }

    #[test]
    fn a_usable_route_carries_no_reason() {
        let (ok, why) = route_state(
            ProviderId::Fal,
            "fal-ai/nano-banana-2",
            &["fal".to_string()],
        );
        assert!(ok);
        assert!(why.is_none());
    }

    #[test]
    fn local_is_available_with_no_key_at_all() {
        // The free tier. Local needs no credential and has a client, so it must
        // never be greyed out for want of a key.
        let (ok, why) = route_state(ProviderId::Local, "comfy/x", &[]);
        assert!(ok, "local should be usable with no keys: {why:?}");
    }
}
