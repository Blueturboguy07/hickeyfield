//! What a fal endpoint actually accepts, from fal.
//!
//! Three bugs in one day came from the same mistake: trusting the vendored
//! catalogue about a *different provider's* API.
//!
//! 1. Parameter **names** differ — the catalogue says `image`, fal wants
//!    `image_url`. Patched with a dialect table.
//! 2. Input **modes** differ — the catalogue has one model, fal splits it into
//!    `/text-to-video` and `/image-to-video`. Patched with a mode table.
//! 3. Accepted **inputs** differ, and this one cost real money: the catalogue
//!    says Gemini Omni takes `image_references` and `video_references`; fal's
//!    endpoint takes only `prompt`, `duration` and `aspect_ratio`. A user
//!    attached a clip, asked for it to be edited, and got an unrelated
//!    text-to-video generation — because fal ignored the field it did not
//!    recognise and billed for the result.
//!
//! The catalogue describes Higgsfield's API. It is not wrong; it is about
//! something else. fal publishes its own per-endpoint schema, unauthenticated,
//! so the durable fix is to stop guessing and read it.
//!
//! **Silently dropping a user's input is the failure mode to design against.**
//! A rejected request costs a round trip; a request that quietly ignores half
//! of what was asked costs money and trust.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// The input contract for one endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointSchema {
    /// Every field the endpoint declares.
    pub accepted: BTreeSet<String>,
    pub required: BTreeSet<String>,
    /// Enumerated values, where the endpoint publishes them.
    ///
    /// Needed because the catalogue and fal spell the *same* value
    /// differently: we offer `1k`, fal wants `1K`; we offer `4`, Veo wants
    /// `4s`. Both are 422s that look like our bug to the user.
    pub enums: BTreeMap<String, Vec<String>>,
}

impl EndpointSchema {
    pub fn accepts(&self, key: &str) -> bool {
        self.accepted.contains(key)
    }

    /// Any field that looks like it carries media.
    ///
    /// Matched on the name because fal is consistent about it — `image_url`,
    /// `image_urls`, `video_url`, `audio_url`, `*_frame_url` — and because the
    /// OpenAPI type is just `string` either way.
    pub fn media_keys(&self) -> Vec<&str> {
        self.accepted
            .iter()
            .map(String::as_str)
            .filter(|k| {
                k.ends_with("_url")
                    || k.ends_with("_urls")
                    || k.ends_with("_image")
                    || k.ends_with("_video")
            })
            .collect()
    }

    /// Whether this endpoint can take attached media at all.
    pub fn takes_media(&self) -> bool {
        !self.media_keys().is_empty()
    }

    /// Rewrite a value into the endpoint's own spelling, when it is obviously
    /// the same value written differently.
    ///
    /// Only unambiguous rewrites: case (`1k` -> `1K`) and a unit suffix
    /// (`4` -> `4s`). Anything else returns `None`, because a value the
    /// endpoint genuinely does not offer must surface as a refusal rather than
    /// be silently swapped for a neighbour the user did not pick — that would
    /// be the same silent-substitution failure in a new place.
    pub fn coerce(&self, field: &str, value: &str) -> Option<String> {
        let Some(allowed) = self.enums.get(field) else {
            return Some(value.to_string());
        };
        if allowed.iter().any(|a| a == value) {
            return Some(value.to_string());
        }
        if let Some(hit) = allowed.iter().find(|a| a.eq_ignore_ascii_case(value)) {
            return Some(hit.clone());
        }
        // `4` against `4s`, and the reverse.
        let bare = value.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        if let Some(hit) = allowed.iter().find(|a| {
            a.trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .eq_ignore_ascii_case(bare)
                && !bare.is_empty()
        }) {
            return Some(hit.clone());
        }
        None
    }
}

fn cache() -> &'static Mutex<BTreeMap<String, Option<EndpointSchema>>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Option<EndpointSchema>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Parse fal's OpenAPI document into the input contract.
///
/// Separated from the fetch so the parser is testable against a captured
/// document, which is the only part likely to break when fal restructures.
pub fn parse(doc: &serde_json::Value) -> Option<EndpointSchema> {
    let schemas = doc.get("components")?.get("schemas")?.as_object()?;
    // fal names it `<Model>Input`. Take the first such schema: an endpoint
    // document describes exactly one input.
    let (_, input) = schemas.iter().find(|(name, _)| name.ends_with("Input"))?;

    let accepted = input
        .get("properties")?
        .as_object()?
        .keys()
        .cloned()
        .collect();
    let mut enums = BTreeMap::new();
    if let Some(props) = input.get("properties").and_then(|p| p.as_object()) {
        for (name, prop) in props {
            if let Some(vals) = prop.get("enum").and_then(|e| e.as_array()) {
                let vs: Vec<String> = vals
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                if !vs.is_empty() {
                    enums.insert(name.clone(), vs);
                }
            }
        }
    }

    let required = input
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Some(EndpointSchema {
        accepted,
        required,
        enums,
    })
}

/// Fetch and cache the schema for one endpoint.
///
/// `None` means we could not find out — no network, or fal restructured the
/// document. Callers must treat that as *unknown*, never as *nothing accepted*:
/// refusing a valid request because we are offline would be a worse failure
/// than the one this module exists to prevent.
///
/// Negative results are cached too, so a model that genuinely has no published
/// schema does not cost a request per generation.
pub fn for_endpoint(endpoint: &str) -> Option<EndpointSchema> {
    if let Some(hit) = cache().lock().ok()?.get(endpoint) {
        return hit.clone();
    }

    let fetched = fetch(endpoint);
    if let Ok(mut c) = cache().lock() {
        c.insert(endpoint.to_string(), fetched.clone());
    }
    fetched
}

fn fetch(endpoint: &str) -> Option<EndpointSchema> {
    // Short: this runs on the submit path, and a slow answer must not delay a
    // generation. Missing the schema costs correctness we already lacked.
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!("halation/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;

    let resp = client
        .get(format!(
            "https://fal.ai/api/openapi/queue/openapi.json?endpoint_id={endpoint}"
        ))
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    parse(&resp.json::<serde_json::Value>().ok()?)
}

/// Seed the cache, for tests and for a bundled snapshot.
pub fn preload(endpoint: &str, schema: Option<EndpointSchema>) {
    if let Ok(mut c) = cache().lock() {
        c.insert(endpoint.to_string(), schema);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from
    /// `fal.ai/api/openapi/queue/openapi.json?endpoint_id=google/gemini-omni-flash`
    /// on 2026-08-05 — the endpoint that silently ignored a user's video.
    const GEMINI_OMNI: &str = r#"{
      "components": { "schemas": {
        "GeminiOmniFlashOutput": { "properties": { "video": {"type": "object"} } },
        "GeminiOmniFlashInput": {
          "properties": {
            "duration": {"type": "integer"},
            "prompt": {"type": "string"},
            "aspect_ratio": {"type": "string"}
          },
          "required": ["prompt"]
        }
      } }
    }"#;

    const KLING_I2V: &str = r#"{
      "components": { "schemas": {
        "KlingInput": {
          "properties": {
            "prompt": {"type": "string"},
            "image_url": {"type": "string"},
            "tail_image_url": {"type": "string"},
            "duration": {"type": "string"}
          },
          "required": ["prompt", "image_url"]
        }
      } }
    }"#;

    #[test]
    fn published_enums_are_captured() {
        let doc = r#"{"components":{"schemas":{"XInput":{"properties":{
            "resolution":{"type":"string","enum":["1K","2K"]},
            "prompt":{"type":"string"}}}}}}"#;
        let s = parsed(doc);
        assert_eq!(s.enums.get("resolution").unwrap(), &["1K", "2K"]);
        assert!(!s.enums.contains_key("prompt"));
    }

    fn parsed(src: &str) -> EndpointSchema {
        parse(&serde_json::from_str(src).unwrap()).unwrap()
    }

    #[test]
    fn a_text_only_endpoint_reports_that_it_takes_no_media() {
        // The bug this exists to prevent: our catalogue says Gemini Omni takes
        // image_references and video_references. fal's endpoint takes neither,
        // so a user's attached clip was dropped on the floor and billed for.
        let s = parsed(GEMINI_OMNI);
        assert!(!s.takes_media(), "media keys: {:?}", s.media_keys());
        assert!(s.accepts("prompt"));
        assert!(!s.accepts("video_url"));
        assert!(!s.accepts("image_references"));
    }

    #[test]
    fn an_image_to_video_endpoint_reports_its_media_keys() {
        let s = parsed(KLING_I2V);
        assert!(s.takes_media());
        let mut keys = s.media_keys();
        keys.sort_unstable();
        assert_eq!(keys, ["image_url", "tail_image_url"]);
    }

    #[test]
    fn required_fields_are_captured() {
        assert_eq!(
            parsed(KLING_I2V)
                .required
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            ["image_url", "prompt"]
        );
    }

    #[test]
    fn the_output_schema_is_not_mistaken_for_the_input() {
        // Both live in the same document and only the suffix distinguishes
        // them; reading the output would report a video field on a text model.
        let s = parsed(GEMINI_OMNI);
        assert!(!s.accepts("video"));
    }

    #[test]
    fn a_document_we_cannot_understand_is_unknown_rather_than_empty() {
        // "Unknown" must never collapse into "accepts nothing", or being
        // offline would block every generation.
        assert!(parse(&serde_json::json!({})).is_none());
        assert!(parse(&serde_json::json!({"components": {}})).is_none());
    }

    #[test]
    fn case_only_differences_are_rewritten_not_rejected() {
        // Measured: we offer `1k`, fal's Nano Banana wants `1K`. Identical
        // value, different spelling, guaranteed 422 that reads as our bug.
        let s = EndpointSchema {
            accepted: ["resolution".to_string()].into_iter().collect(),
            required: Default::default(),
            enums: [(
                "resolution".to_string(),
                vec!["1K".into(), "2K".into(), "4K".into()],
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(s.coerce("resolution", "1k").as_deref(), Some("1K"));
        assert_eq!(s.coerce("resolution", "4K").as_deref(), Some("4K"));
    }

    #[test]
    fn a_unit_suffix_is_reconciled_in_both_directions() {
        // Measured: we offer duration `4`, Veo 3.1 wants `4s`.
        let s = EndpointSchema {
            accepted: ["duration".to_string()].into_iter().collect(),
            required: Default::default(),
            enums: [(
                "duration".to_string(),
                vec!["4s".into(), "6s".into(), "8s".into()],
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(s.coerce("duration", "4").as_deref(), Some("4s"));
        assert_eq!(s.coerce("duration", "8s").as_deref(), Some("8s"));
    }

    #[test]
    fn a_value_the_endpoint_does_not_offer_is_refused_not_substituted() {
        // Seedance Fast tops out at 720p while our catalogue offers 4k.
        // Quietly downgrading to 720p would charge for something the user did
        // not pick — the same silent-substitution failure in a new place.
        let s = EndpointSchema {
            accepted: ["resolution".to_string()].into_iter().collect(),
            required: Default::default(),
            enums: [("resolution".to_string(), vec!["480p".into(), "720p".into()])]
                .into_iter()
                .collect(),
        };
        assert_eq!(s.coerce("resolution", "4k"), None);
        assert_eq!(s.coerce("resolution", "1080p"), None);
    }

    #[test]
    fn a_field_with_no_published_enum_passes_through_untouched() {
        let s = EndpointSchema {
            accepted: ["prompt".to_string()].into_iter().collect(),
            required: Default::default(),
            enums: Default::default(),
        };
        assert_eq!(s.coerce("prompt", "anything").as_deref(), Some("anything"));
    }

    #[test]
    fn the_cache_returns_what_was_preloaded() {
        let s = parsed(KLING_I2V);
        preload("test/only", Some(s.clone()));
        assert_eq!(for_endpoint("test/only"), Some(s));
    }

    #[test]
    fn a_cached_negative_is_still_a_cache_hit() {
        // Otherwise a model with no published schema costs a network round trip
        // on every single generation.
        preload("test/negative", None);
        assert_eq!(for_endpoint("test/negative"), None);
    }
}
