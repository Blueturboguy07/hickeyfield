//! Audit the registry against fal's published schemas.
//!
//! Every bug found on 2026-08-05 was the same shape: the vendored catalogue
//! describes *Higgsfield's* API, we send to fal, and the two disagree. Each was
//! found by a user hitting it. This finds the rest mechanically.
//!
//! Four checks, in descending order of how much they cost when wrong:
//!
//! 1. **Silently ignored media** — we let the user attach a file to an endpoint
//!    with no field for it. fal drops it and bills for a generation that did
//!    not do what was asked. This is the one that cost real money.
//! 2. **Missing required fields** — the endpoint demands something we never
//!    send, so the request 422s after a round trip.
//! 3. **Dropped settings** — a control the user changed that the endpoint does
//!    not declare, so it silently has no effect.
//! 4. **Wrong option sets** — our chip row offers values the endpoint rejects.
//!
//! Run: `cargo run -p hickeyfield-core --example audit_fal`. No key needed; fal's
//! schema endpoint is unauthenticated.

use hickeyfield_core::catalog::ValueSpec;
use hickeyfield_core::fal_schema;
use hickeyfield_core::media::{self, InputMode};
use hickeyfield_core::registry;
use std::collections::BTreeSet;

fn main() {
    let reg = registry::registry();
    let mut ignored_media = Vec::new();
    let mut missing_required = Vec::new();
    let mut dropped_settings = Vec::new();
    let mut wrong_options = Vec::new();
    let mut unreachable = Vec::new();
    let mut checked = 0usize;

    for m in reg.values() {
        let Some(route) = m
            .routes
            .iter()
            .find(|r| r.provider == hickeyfield_core::ProviderId::Fal)
        else {
            continue;
        };
        let produces_video = format!("{}", m.modality) == "video";

        // Audit the modes the model actually offers. A t2v-only endpoint and an
        // i2v endpoint are different contracts and both can be wrong.
        for mode in [InputMode::Text, InputMode::Image, InputMode::Video] {
            let Ok(endpoint) = media::resolve_endpoint(&route.slug, mode, produces_video) else {
                continue;
            };
            // Text and Image can resolve to the same endpoint for exact slugs;
            // do not audit it twice.
            if mode != InputMode::Text
                && media::resolve_endpoint(&route.slug, InputMode::Text, produces_video).as_deref()
                    == Ok(endpoint.as_str())
            {
                continue;
            }

            let Some(schema) = fal_schema::for_endpoint(&endpoint) else {
                unreachable.push(format!("{} -> {endpoint}", m.id));
                continue;
            };
            checked += 1;

            // 1. Does the catalogue promise media NO mode of this route can
            //    take? A text-to-video endpoint rejecting a start frame is by
            //    design — the user attaching one resolves to image-to-video
            //    instead. Only flag a route where every mode refuses media,
            //    because that is the case where the UI lets you attach a file
            //    that will be silently dropped.
            let any_mode_takes_media = [InputMode::Text, InputMode::Image, InputMode::Video]
                .into_iter()
                .filter_map(|md| media::resolve_endpoint(&route.slug, md, produces_video).ok())
                .filter_map(|ep| fal_schema::for_endpoint(&ep))
                .any(|sc| sc.takes_media());
            let catalogue_takes_media = m.spec.media_flags().next().is_some();
            if catalogue_takes_media && !any_mode_takes_media && mode == InputMode::Text {
                ignored_media.push(format!(
                    "{:<26} {endpoint}\n      catalogue offers [{}] but the endpoint takes none",
                    m.id,
                    m.spec
                        .media_flags()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }

            // 2. Required fields we would never send.
            //
            // `prompt` is always supplied by the user, and a media key is
            // supplied when they attach something — so only flag the rest,
            // which is where a silent 422 comes from.
            let never_sent: Vec<&str> = schema
                .required
                .iter()
                .map(String::as_str)
                .filter(|k| *k != "prompt" && !schema.media_keys().contains(k))
                .collect();
            if !never_sent.is_empty() {
                missing_required.push(format!(
                    "{:<26} {endpoint}\n      requires [{}] which nothing in the app supplies",
                    m.id,
                    never_sent.join(", ")
                ));
            }

            // 3. Settings the UI can change that this endpoint ignores.
            for (ours, theirs) in [
                ("duration", "duration"),
                ("resolution", "resolution"),
                ("aspect", "aspect_ratio"),
                ("audio", "generate_audio"),
            ] {
                let we_offer = match ours {
                    "duration" => m.spec.capabilities().supports_duration,
                    "resolution" => m.spec.capabilities().supports_resolution,
                    "aspect" => m.spec.capabilities().supports_aspect,
                    _ => m.spec.capabilities().audio,
                };
                if we_offer && !schema.accepts(theirs) {
                    dropped_settings.push(format!(
                        "{:<26} {endpoint}\n      UI offers `{ours}` but the endpoint has no `{theirs}`",
                        m.id
                    ));
                }
            }

            // 4. Option sets the endpoint would reject. Only checkable where
            //    both sides enumerate.
            for flag in ["resolution", "aspect_ratio", "duration"] {
                let (Some(ours), Some(theirs)) = (
                    m.spec.flag(flag).and_then(|f| match &f.value {
                        ValueSpec::Enum(v) => Some(v.iter().cloned().collect::<BTreeSet<_>>()),
                        _ => None,
                    }),
                    schema_enum(&endpoint, flag),
                ) else {
                    continue;
                };
                let rejected: Vec<String> = ours.difference(&theirs).cloned().collect();
                if !rejected.is_empty() {
                    wrong_options.push(format!(
                        "{:<26} {endpoint}\n      offers {flag} [{}] which the endpoint rejects (it takes [{}])",
                        m.id,
                        rejected.join(", "),
                        theirs.iter().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
            }
        }
    }

    section(
        "SILENTLY IGNORED MEDIA — bills for the wrong generation",
        &ignored_media,
    );
    section(
        "MISSING REQUIRED FIELDS — 422 after a round trip",
        &missing_required,
    );
    section(
        "DROPPED SETTINGS — the control has no effect",
        &dropped_settings,
    );
    section(
        "REJECTED OPTIONS — the chip row offers invalid values",
        &wrong_options,
    );

    println!("\n{checked} endpoint(s) audited against fal's own schema.");
}

/// The enumerated values fal declares for one field, if it enumerates them.
fn schema_enum(endpoint: &str, field: &str) -> Option<BTreeSet<String>> {
    let doc: serde_json::Value = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("hickeyfield-audit")
        .build()
        .ok()?
        .get(format!(
            "https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={endpoint}"
        ))
        .send()
        .ok()?
        .json()
        .ok()?;
    let schemas = doc.get("components")?.get("schemas")?.as_object()?;
    let (_, input) = schemas.iter().find(|(n, _)| n.ends_with("Input"))?;
    let prop = input.get("properties")?.get(field)?;
    let values = prop.get("enum")?.as_array()?;
    Some(
        values
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
    )
}

fn section(title: &str, rows: &[String]) {
    println!("\n=== {title} ({}) ===", rows.len());
    for r in rows {
        println!("  {r}");
    }
    if rows.is_empty() {
        println!("  none");
    }
}
