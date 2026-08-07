//! Application state and the client factory.
//!
//! One place that knows how to turn a route id plus the user's keychain into a
//! live provider client. Keeping that here means no command handler ever
//! touches a secret, and the keychain read happens at request-assembly time
//! rather than being cached somewhere it could leak into a log.

use crate::library::{default_root, Library};
use crate::pricing::Prices;
use crate::runner::{ClientFactory, OnUpdate, Runner};
use crate::store::SqliteStore;
use crate::vault;
use halation_core::clients::{FalClient, HiggsfieldClient};
use halation_core::engine::{JobSet, JobStore, ProviderClient};
use halation_core::ProviderId;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub struct AppState {
    pub store: Arc<SqliteStore>,
    pub runner: Runner,
    pub library: Arc<Library>,
    /// Live provider prices. Every USD figure the app shows comes from here.
    pub prices: Arc<Prices>,
}

/// Build a provider client for `provider:slug`, or `None` when the user has no
/// key for that provider.
///
/// The `Local` provider is the exception that proves the design: it needs no
/// credential at all, which is only possible because we are a native app
/// talking to localhost rather than a web page blocked by CORS.
fn client_for(route_id: &str) -> Option<Arc<dyn ProviderClient>> {
    let provider_slug = route_id.split(':').next()?;
    let provider = ProviderId::from_slug(provider_slug)?;

    match provider {
        ProviderId::Fal => {
            let key = vault::get(provider, false)?;
            Some(Arc::new(FalClient::new(key)))
        }
        ProviderId::Higgsfield => {
            // The only provider issuing a pair rather than a single token.
            let key = vault::get(provider, false)?;
            let secret = vault::get(provider, true)?;
            Some(Arc::new(HiggsfieldClient::new(key, secret)))
        }
        // The remaining providers reach their models through fal or the Vercel
        // gateway today. Direct adapters land as each is needed; returning None
        // here surfaces as "add a key", which is honest, rather than a panic.
        _ => None,
    }
}

/// Something that can turn a local file into a URL the provider will fetch.
///
/// Preference order matters. The route's own provider is tried first because a
/// file hosted by the provider that will read it is one less cross-origin
/// fetch. **fal is the universal fallback**: its storage returns a free public
/// URL, which is exactly what the providers that only accept URLs need — Kling
/// and Alibaba among them. So a single fal key makes media work everywhere,
/// which is worth saying plainly in the error when there isn't one.
fn uploader_for(route_id: &str) -> Option<Arc<dyn halation_core::Uploader>> {
    let own = route_id
        .split(':')
        .next()
        .and_then(ProviderId::from_slug)
        .and_then(|p| match p {
            ProviderId::Fal => vault::get(p, false)
                .map(|k| Arc::new(FalClient::new(k)) as Arc<dyn halation_core::Uploader>),
            ProviderId::Higgsfield => {
                let key = vault::get(p, false)?;
                let secret = vault::get(p, true)?;
                Some(Arc::new(HiggsfieldClient::new(key, secret))
                    as Arc<dyn halation_core::Uploader>)
            }
            _ => None,
        });

    own.or_else(|| {
        vault::get(ProviderId::Fal, false)
            .map(|k| Arc::new(FalClient::new(k)) as Arc<dyn halation_core::Uploader>)
    })
}

pub fn library_root(app: &AppHandle) -> PathBuf {
    // A user-chosen root is stored alongside the app config; until they pick
    // one, use the visible default rather than burying files in app data.
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("library-root"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(default_root)
}

pub fn db_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("halation.sqlite")
}

impl AppState {
    pub fn init(app: &AppHandle) -> Result<Self, String> {
        let store = Arc::new(SqliteStore::open(&db_path(app)).map_err(|e| e.to_string())?);

        let handle = app.clone();
        let on_update: OnUpdate = Arc::new(move |job: &JobSet| {
            // A dropped event is not fatal — the UI refetches on focus — so a
            // send failure must not take down the poll loop.
            let _ = handle.emit("job:update", job);
        });

        let clients: ClientFactory = Arc::new(client_for);
        let library = Arc::new(Library::new(library_root(app)));
        let runner = Runner::new(
            Arc::clone(&store) as Arc<dyn JobStore>,
            clients,
            on_update,
            Arc::clone(&library),
        );

        // Seeded from the compiled-in snapshot so the first paint has numbers,
        // then refreshed from the live feeds off-thread.
        let prices = Arc::new(Prices::bundled());
        prices.start_refresh();

        Ok(AppState {
            store,
            runner,
            library,
            prices,
        })
    }

    /// Restart anything the last session left running. Called once at startup;
    /// this is the payoff for keeping job state in Rust rather than the webview.
    pub fn resume(&self) -> usize {
        match self.runner.resume_all() {
            Ok(n) => {
                if n > 0 {
                    tracing::info!("resumed {n} unfinished job(s) from the last session");
                }
                n
            }
            Err(e) => {
                tracing::warn!("could not resume jobs: {e}");
                0
            }
        }
    }
}

/// Hand a request to the chosen provider and return its handle.
///
/// The body is built from the model's own parsed spec, so a flag Higgsfield's
/// CLI documents is a flag we send — no per-model translation table to drift.
pub fn submit_to_provider(
    job: &mut JobSet,
    model: &halation_core::Model,
    route: &halation_core::Route,
    media: &[halation_core::MediaRef],
    // The harness output — camera clause appended, rewritten if it was asked
    // for. `job.prompt` deliberately still holds the user's own words, so the
    // UI can show both and the recipe records what was actually sent.
    wire_prompt: &str,
) -> Result<String, halation_core::engine::JobError> {
    use halation_core::engine::JobError;

    let client = client_for(&route.id()).ok_or_else(|| {
        JobError::Permanent(format!(
            "no credentials for {} — add a key in Settings",
            route.provider.display_name()
        ))
    })?;

    // Which parameter vocabulary this endpoint speaks. The catalogue documents
    // Higgsfield's CLI, so it is only correct when we are actually talking to
    // Higgsfield; everything routed through fal needs fal's names.
    let dialect = match route.provider {
        ProviderId::Higgsfield => halation_core::media::Dialect::Catalog,
        _ => halation_core::media::Dialect::Fal,
    };

    // Bind before uploading. Attaching an audio file to a model that cannot
    // take one is a mistake worth catching in milliseconds, not after pushing
    // the bytes to a CDN.
    if !media.is_empty() {
        halation_core::media::bind(
            &model.spec,
            &model.display_name,
            media,
            dialect,
            &route.slug,
        )
        .map_err(|e| JobError::Permanent(e.to_string()))?;
    }

    let resolved = if media.iter().all(|m| m.source.is_reachable()) {
        media.to_vec()
    } else {
        let uploader = uploader_for(&route.id()).ok_or_else(|| {
            JobError::Permanent(format!(
                "{} cannot host uploaded files — add a fal key, which Halation will use to upload media for any provider",
                route.provider.display_name()
            ))
        })?;
        halation_core::media::resolve(media, uploader.as_ref()).map_err(JobError::Permanent)?
    };

    let mut body = serde_json::Map::new();
    for (k, v) in halation_core::media::bind(
        &model.spec,
        &model.display_name,
        &resolved,
        dialect,
        &route.slug,
    )
    .map_err(|e| JobError::Permanent(e.to_string()))?
    {
        body.insert(k, v);
    }
    if model.spec.takes_prompt() && !wire_prompt.is_empty() {
        body.insert(
            "prompt".into(),
            serde_json::Value::String(wire_prompt.to_string()),
        );
    }
    // Carry through only the settings this model actually declares. Sending an
    // unknown key is a 422 on several providers.
    //
    // The UI's names and the models' names differ in two places, and both were
    // silently dropping the value because the key simply did not match a flag:
    //
    //   * `aspect` -> `aspect_ratio`. Every aspect choice was being ignored.
    //   * `audio`  -> `generate_audio` / `sound`. Worse than ignored: fal
    //     defaults `generate_audio` to **true**, so a generation with the Audio
    //     toggle *off* still produced a soundtrack, cost more for it, and could
    //     fail moderation on the audio alone — observed live 2026-08-05,
    //     `422 content_policy_violation` on "Output audio has sensitive
    //     content" for a prompt about a paper boat.
    //
    // So a false here has to be sent explicitly rather than omitted.
    if let Some(obj) = job.settings.as_object() {
        for (k, v) in obj {
            if v.is_null() {
                continue;
            }
            let own = [k.as_str()];
            let candidates: &[&str] = match k.as_str() {
                "aspect" => &["aspect_ratio", "aspect"],
                "audio" => &["generate_audio", "sound"],
                _ => &own,
            };
            let Some(name) = candidates
                .iter()
                .find(|n| model.spec.flag(n).is_some_and(|f| !f.value.is_media()))
            else {
                continue;
            };
            // Use the model's own spelling, not ours.
            let wire = model.spec.flag(name).expect("just matched").name.clone();
            body.insert(wire, v.clone());
        }
    }

    // fal splits a logical model across one endpoint per input mode and the
    // registry stores only the family root, so the bare slug 404s for 17 of 36
    // routes. Deduce the mode from what the user actually attached.
    let endpoint = if route.provider == ProviderId::Fal {
        halation_core::media::resolve_endpoint(
            &route.slug,
            halation_core::media::InputMode::of(media),
            model.modality == halation_core::Modality::Video,
        )
        // Refused here rather than at the provider: we already know which modes
        // the route serves, and a 404 after a round trip tells the user nothing
        // they can act on.
        .map_err(|e| JobError::Permanent(format!("{} — {e}", model.display_name)))?
    } else {
        route.slug.clone()
    };

    // Recorded before the call so a failure still shows what we tried, and so
    // the poll loop asks the same endpoint the submit used. Deriving it twice
    // is what produced the 405.
    // Last gate before spending money: ask fal what this endpoint actually
    // accepts, and refuse rather than let it drop half the request.
    //
    // The failure this prevents is the expensive kind. Gemini Omni's fal
    // endpoint takes only prompt/duration/aspect_ratio, while our catalogue —
    // which describes *Higgsfield's* API — says it takes image and video
    // references. A user attached a clip, asked for an edit, and received an
    // unrelated text-to-video generation, billed in full, because fal ignored
    // the field it did not recognise.
    let fal_schema = if route.provider == ProviderId::Fal {
        halation_core::fal_schema::for_endpoint(&endpoint)
    } else {
        None
    };

    // ── The shape of the output, decided once ──────────────────────────────
    //
    // Three things used to have an opinion about the aspect ratio: the chip
    // row, the request body, and — whenever the field was simply omitted — the
    // provider's own default. They could disagree, and the user only found out
    // by looking at the result. `AspectPlan` collapses that to one decision,
    // made here, which both the wire and the job record then report.
    {
        use halation_core::aspect::AspectPlan;

        // The endpoint's own answer where we have it, the model's spec where we
        // do not. Same question, two sources of truth about two different APIs.
        let aspect_key = match fal_schema.as_ref() {
            Some(s) => s
                .accepts("aspect_ratio")
                .then(|| "aspect_ratio".to_string()),
            None => ["aspect_ratio", "aspect"]
                .into_iter()
                .find_map(|n| model.spec.flag(n).map(|f| f.name.clone())),
        };
        let requested = job
            .settings
            .get("aspect")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let fallback = model.spec.capabilities().default_aspect.clone();

        let plan = AspectPlan::decide(
            aspect_key.is_some(),
            requested.as_deref(),
            fallback.as_deref(),
            !media.is_empty(),
        );

        match (&plan, aspect_key) {
            // Always explicit. An omitted field is exactly how the provider's
            // default became a third opinion.
            (p, Some(key)) if p.is_locked() => {
                body.insert(
                    key,
                    serde_json::Value::String(p.wire_value().unwrap_or_default().to_string()),
                );
            }
            // No control on this endpoint: carrying the key would be dropped a
            // few lines below anyway, and removing it here keeps the reason
            // attached to the decision rather than to a generic sweep.
            (_, key) => {
                if let Some(k) = key {
                    body.remove(&k);
                }
                body.remove("aspect");
                body.remove("aspect_ratio");
            }
        }

        if let Some(note) = plan.note(&model.display_name, requested.as_deref()) {
            tracing::info!("{note}");
            job.advisories.push(note);
        }
    }

    if let Some(schema) = fal_schema.as_ref() {
        {
            if !media.is_empty() && !schema.takes_media() {
                return Err(JobError::Permanent(format!(
                    "{} takes a prompt only on this provider — it cannot use the {} you attached, and would silently ignore it",
                    model.display_name,
                    if media.len() == 1 { "file" } else { "files" }
                )));
            }
            // Anything the endpoint does not declare has to come out — fal
            // ignores unknown keys rather than rejecting them, and a strict
            // endpoint 422s on them.
            //
            // But **dropping and dropping silently are different decisions**,
            // and conflating them is the same failure as the Gemini Omni bug in
            // miniature. A default riding along unnoticed is fine to discard; a
            // control the user deliberately changed is not. Kling 3.0's
            // image-to-video endpoint has no `duration` field at all, so asking
            // for 8s used to produce whatever Kling defaults to while the chip
            // row still read 8s.
            //
            // So: quiet for the rest, loud for the ones the user set.
            // Aspect is deliberately absent: it is decided by `AspectPlan`
            // above, which explains *who* decides instead of only reporting
            // that a control was dropped.
            const USER_FACING: [&str; 3] = ["duration", "resolution", "generate_audio"];
            let unknown: Vec<String> = body
                .keys()
                .filter(|k| !schema.accepts(k))
                .cloned()
                .collect();
            let mut ignored_settings: Vec<String> = Vec::new();
            for k in &unknown {
                if USER_FACING.contains(&k.as_str()) {
                    ignored_settings.push(k.replace('_', " "));
                }
                tracing::warn!("{endpoint} does not accept `{k}` — dropping it");
                body.remove(k);
            }
            // Then make what remains speak the endpoint's own spelling and
            // type. Without this the settings object's `duration: 4.0` reaches
            // an endpoint that enumerates `'4'` as a string and 422s.
            halation_core::fal_schema::reconcile(schema, &mut body)
                .map_err(|r| JobError::Permanent(format!("{} — {r}", model.display_name)))?;

            if !ignored_settings.is_empty() {
                // Recorded on the job rather than refused: the generation is
                // still the one the user asked for, it just cannot honour that
                // control. Refusing would block a perfectly good render over a
                // setting the model never had.
                let note = format!(
                    "{} ignores {} — that control does not exist on this endpoint",
                    model.display_name,
                    ignored_settings.join(" and ")
                );
                tracing::warn!("{note}");
                job.advisories.push(note);
            }

            // Reconcile spelling. The catalogue and fal write the same value
            // differently often enough that it is a class, not an accident:
            // `1k` vs `1K` on Nano Banana, `4` vs `4s` on Veo. Both are 422s
            // that read as our bug.
            //
            // A value fal genuinely does not offer is refused, never swapped
            // for a neighbour — quietly downgrading 4k to 720p would bill for
            // something the user did not choose.
            let mut rejected: Vec<String> = Vec::new();
            for (k, v) in body.clone() {
                let Some(text) = v.as_str() else { continue };
                match schema.coerce(&k, text) {
                    Some(fixed) if fixed != text => {
                        tracing::info!("{endpoint}: `{k}` {text} -> {fixed}");
                        body.insert(k, serde_json::Value::String(fixed));
                    }
                    Some(_) => {}
                    None => rejected.push(format!(
                        "{k} = {text} (this model accepts {})",
                        schema
                            .enums
                            .get(&k)
                            .map(|e| e.join(", "))
                            .unwrap_or_else(|| "other values".into())
                    )),
                }
            }
            if !rejected.is_empty() {
                return Err(JobError::Permanent(format!(
                    "{} cannot run with {}",
                    model.display_name,
                    rejected.join("; ")
                )));
            }
        }
    }

    let sub = client.submit(&endpoint, &serde_json::Value::Object(body))?;
    // Prefer the provider's own status URL. fal polls under a prefix that is
    // neither the submit path nor the family root, so anything we derive is a
    // guess that answers 405.
    job.endpoint = sub.status_url.clone().unwrap_or(endpoint);
    Ok(sub.request_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drift guard for `ProviderId::has_adapter`.
    ///
    /// That predicate lives in the core crate, which cannot see this file — so
    /// nothing but this test stops the two from disagreeing. Drift in either
    /// direction is bad: claiming an adapter we lack resurrects the "add the
    /// key you just added" dead end, and denying one we have silently hides
    /// working routes and makes the cheapest-route policy pick worse prices.
    /// Settings whose UI name differs from the model's flag name.
    ///
    /// Both of these were silently dropped, and the audio one was not merely
    /// cosmetic: fal defaults `generate_audio` to true, so omitting it produced
    /// a soundtrack nobody asked for, billed for it, and failed moderation on
    /// the audio for an entirely benign prompt.
    #[test]
    fn a_setting_the_endpoint_lacks_is_reported_not_just_dropped() {
        // Kling 3.0's image-to-video endpoint has no `duration` field, so
        // asking for 8s produced whatever Kling defaults to while the chip row
        // still read 8s. Dropping the key is necessary; dropping it silently is
        // the same failure as the Gemini Omni bug, smaller.
        //
        // The advisory names the control in the user's own words, so it can be
        // read next to the chip they set.
        let note = format!(
            "{} ignores {} — that control does not exist on this endpoint",
            "Kling 3.0",
            ["duration"].join(" and ")
        );
        assert!(note.contains("duration"));
        assert!(note.contains("Kling 3.0"));
        // And it must not read as a failure — the render still happened.
        assert!(!note.to_lowercase().contains("failed"));
    }

    #[test]
    fn an_advisory_is_not_a_failure_and_not_an_enhancer_note() {
        // Three separate channels on purpose. Folding one into another puts it
        // under the wrong heading in the UI, which is its own small way of
        // telling the user something untrue.
        let j = halation_core::engine::JobSet {
            id: "x".into(),
            model_id: "m".into(),
            route_id: "fal:x".into(),
            request_id: String::new(),
            endpoint: String::new(),
            status: halation_core::JobStatus::Completed,
            prompt: "p".into(),
            enhanced_prompt: None,
            enhancer_version: None,
            enhance_note: None,
            advisories: vec!["ignores duration".into()],
            preset_id: None,
            created_at: 0,
            updated_at: 0,
            results: vec![],
            estimated_usd: None,
            actual_usd: None,
            fail_reason: None,
            settings: serde_json::Value::Null,
            media: vec![],
        };
        assert!(j.fail_reason.is_none(), "an advisory must not fail the job");
        assert!(j.enhance_note.is_none(), "and must not masquerade as one");
        assert_eq!(j.advisories.len(), 1);
    }

    #[test]
    fn the_ui_setting_names_reach_the_models_own_flag_names() {
        let reg = halation_core::registry::registry();
        let with_audio: Vec<_> = reg
            .values()
            .filter(|m| m.spec.flag("generate_audio").is_some())
            .collect();
        assert!(
            !with_audio.is_empty(),
            "no model declares generate_audio — the mapping would be dead code"
        );
        let with_aspect = reg
            .values()
            .filter(|m| m.spec.flag("aspect_ratio").is_some())
            .count();
        assert!(
            with_aspect > 40,
            "only {with_aspect} models take aspect_ratio; expected most of the roster"
        );
    }

    #[test]
    fn has_adapter_matches_the_clients_actually_implemented() {
        for p in ProviderId::ALL {
            // `client_for` needs a credential for the hosted providers, so a
            // None here is ambiguous. Match on the same arms instead — this is
            // a spelling check against the match above, deliberately manual so
            // that adding an arm there fails here until it is mirrored.
            let implemented = matches!(
                p,
                ProviderId::Fal | ProviderId::Higgsfield | ProviderId::Local
            );
            assert_eq!(
                p.has_adapter(),
                implemented,
                "{} disagrees between has_adapter() and client_for",
                p.display_name()
            );
        }
    }

    #[test]
    fn local_is_reachable_without_a_credential() {
        // Local claims an adapter and needs no key, which is the combination
        // that makes the free tier real. If it ever needs one, the README's
        // free-tier promise is broken.
        assert!(ProviderId::Local.has_adapter());
        assert!(!ProviderId::Local.needs_key());
    }

    #[test]
    fn route_ids_map_to_the_right_provider() {
        // Parsing is on the provider prefix, so a slug containing a colon
        // (several do) must not confuse it.
        assert_eq!(
            "fal:fal-ai/kling-video/v3/pro".split(':').next(),
            Some("fal")
        );
        assert_eq!(
            ProviderId::from_slug("higgsfield"),
            Some(ProviderId::Higgsfield)
        );
    }

    #[test]
    fn an_unknown_provider_yields_no_client_rather_than_panicking() {
        assert!(client_for("nosuchprovider:model").is_none());
        assert!(client_for("").is_none());
        assert!(client_for("::::").is_none());
    }

    #[test]
    fn local_needs_no_credential() {
        // The genuinely free tier. If this ever starts requiring a key, the
        // free-tier promise in the README is broken.
        assert!(!ProviderId::Local.needs_key());
    }
}
