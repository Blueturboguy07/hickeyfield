//! Does this key actually work?
//!
//! A settings screen that only shows "key set" is close to useless: the two
//! failure modes people hit are a typo and a key that has been revoked or has
//! run out of credit, and both look identical to a stored-successfully tick.
//!
//! So "Test" makes a real authenticated request. Every endpoint here is chosen
//! to be a *read* — listing models, fetching an account — so testing a key
//! never costs the user money and never queues a generation.

use hickeyfield_core::ProviderId;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct Validation {
    pub ok: bool,
    /// Shown verbatim to the user, so it has to be worth reading.
    pub detail: String,
}

impl Validation {
    fn ok(detail: impl Into<String>) -> Self {
        Validation {
            ok: true,
            detail: detail.into(),
        }
    }
    fn bad(detail: impl Into<String>) -> Self {
        Validation {
            ok: false,
            detail: detail.into(),
        }
    }
}

/// Phrases providers use when they mean "this key is wrong". Needed because
/// the status code alone is not enough: Google and xAI both answer a bad key
/// with **400**, not 401, and a 400 could otherwise be a malformed probe.
const REJECTION_PHRASES: [&str; 6] = [
    "api key not valid",
    "incorrect api key",
    "invalid api key",
    "invalid_api_key",
    "unauthorized",
    "authentication",
];

/// Turn a response into something a person can act on. "401" is not an
/// instruction; "the key was rejected" is.
///
/// Ambiguity resolves to *not verified* rather than to success. A test that
/// green-lights a dead key is worse than no test — the user goes on to blame
/// the app when a generation fails.
fn explain(status: u16, body: &str, provider: &str) -> Validation {
    let looks_rejected = {
        let lower = body.to_ascii_lowercase();
        REJECTION_PHRASES.iter().any(|p| lower.contains(p))
    };

    match status {
        200..=299 => Validation::ok("key works"),
        401 | 403 => Validation::bad(format!(
            "{provider} rejected this key — check it was copied whole, and that it has not been revoked"
        )),
        402 => Validation::bad(format!("{provider} accepted the key but the account has no credit")),
        // Rate limited means it authenticated first.
        429 => Validation::ok("key works (rate limited right now, which still means it authenticated)"),
        400 | 422 if looks_rejected => Validation::bad(format!(
            "{provider} rejected this key — check it was copied whole, and that it has not been revoked"
        )),
        // Authenticated, then complained about the throwaway id in the probe.
        400 | 422 => Validation::ok("key works"),
        500..=599 => Validation::bad(format!("{provider} is having problems — try again shortly")),
        other => Validation::bad(format!(
            "could not verify against {provider} (HTTP {other}) — the key may still be fine"
        )),
    }
}

fn client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .ok()
}

/// Probe a provider with a cheap authenticated read.
pub fn validate(provider: ProviderId, key: &str, secret: Option<&str>) -> Validation {
    if provider == ProviderId::Local {
        let found = hickeyfield_core::clients::detect_local();
        return match (found.comfyui, found.ollama) {
            (false, false) => {
                Validation::bad("nothing detected on 127.0.0.1:8188 (ComfyUI) or :11434 (Ollama)")
            }
            (a, b) => Validation::ok(format!(
                "found {}",
                [(a, "ComfyUI"), (b, "Ollama")]
                    .iter()
                    .filter(|(on, _)| *on)
                    .map(|(_, n)| *n)
                    .collect::<Vec<_>>()
                    .join(" and ")
            )),
        };
    }

    if key.trim().is_empty() {
        return Validation::bad("no key set");
    }
    let Some(http) = client() else {
        return Validation::bad("could not create an HTTP client");
    };

    let name = provider.display_name();
    let req = match provider {
        // fal has no cheap list endpoint on the queue host; a HEAD against the
        // queue with a bad path still authenticates first, so 401 vs 404
        // distinguishes a bad key from a good one.
        ProviderId::Fal => http
            .get("https://queue.fal.run/fal-ai/flux/requests/00000000-0000-0000-0000-000000000000/status")
            .header("Authorization", format!("Key {key}")),
        ProviderId::Vaig => http
            .get("https://ai-gateway.vercel.sh/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        ProviderId::Google => http.get(format!(
            "https://generativelanguage.googleapis.com/v1beta/models?key={key}"
        )),
        ProviderId::OpenAi => http
            .get("https://api.openai.com/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        ProviderId::XAi => http
            .get("https://api.x.ai/v1/models")
            .header("Authorization", format!("Bearer {key}")),
        // /v1/get_result answers 404 whatever the key is, so it can never
        // distinguish a good key from a bad one. /v1/credits does.
        ProviderId::Bfl => http.get("https://api.bfl.ai/v1/credits").header("x-key", key),
        ProviderId::Recraft => http
            .get("https://external.api.recraft.ai/v1/users/me")
            .header("Authorization", format!("Bearer {key}")),
        ProviderId::Higgsfield => {
            let Some(secret) = secret.filter(|s| !s.trim().is_empty()) else {
                return Validation::bad("Higgsfield needs both a key and a secret");
            };
            http.get("https://platform.higgsfield.ai/requests/00000000-0000-0000-0000-000000000000/status")
                // Note the shape: `Key k:s`, not Bearer. Getting this wrong
                // produces a 401 indistinguishable from a bad key.
                .header("Authorization", format!("Key {key}:{secret}"))
        }
        ProviderId::Local => unreachable!("handled above"),
    };

    match req.send() {
        Ok(r) => {
            let status = r.status().as_u16();
            let body = r.text().unwrap_or_default();
            explain(status, &body, name)
        }
        Err(e) if e.is_timeout() => Validation::bad(format!("{name} did not respond in time")),
        Err(e) if e.is_connect() => {
            Validation::bad(format!("could not reach {name} — check your connection"))
        }
        Err(e) => Validation::bad(format!("could not reach {name}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_key_fails_without_a_network_call() {
        let v = validate(ProviderId::Fal, "   ", None);
        assert!(!v.ok);
        assert_eq!(v.detail, "no key set");
    }

    #[test]
    fn higgsfield_requires_both_halves() {
        let v = validate(ProviderId::Higgsfield, "k", None);
        assert!(!v.ok);
        assert!(v.detail.contains("secret"), "got: {}", v.detail);
    }

    #[test]
    fn rate_limiting_counts_as_a_working_key() {
        // A 429 means the request authenticated and then hit a quota. Telling
        // the user their key is broken would send them to regenerate a key
        // that was fine.
        assert!(explain(429, "", "fal.ai").ok);
    }

    #[test]
    fn a_rejected_key_says_what_to_check() {
        let v = explain(401, "", "OpenAI");
        assert!(!v.ok);
        assert!(v.detail.contains("revoked"), "got: {}", v.detail);
        assert!(v.detail.contains("OpenAI"));
    }

    #[test]
    fn no_credit_is_distinguished_from_a_bad_key() {
        // These need completely different fixes, so conflating them wastes the
        // user's time.
        let broke = explain(402, "", "Recraft");
        let rejected = explain(401, "", "Recraft");
        assert!(!broke.ok && !rejected.ok);
        assert_ne!(broke.detail, rejected.detail);
        assert!(broke.detail.contains("credit"));
    }

    #[test]
    fn a_provider_outage_is_not_blamed_on_the_key() {
        let v = explain(503, "", "fal.ai");
        assert!(!v.ok);
        assert!(v.detail.contains("try again"), "got: {}", v.detail);
        assert!(!v.detail.contains("revoked"));
    }

    /// Bodies captured live on 2026-08-02 by probing each endpoint with a
    /// bogus key. Pinned because the whole detection scheme rests on these
    /// exact phrases — if a provider rewords its error, this fails loudly
    /// instead of quietly green-lighting dead keys.
    #[test]
    fn real_provider_rejections_are_all_detected() {
        let captured = [
            (
                "Google",
                400,
                r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}"#,
            ),
            (
                "xAI",
                400,
                r#"{"code":"invalid-argument","error":"Incorrect API key provided. You can obtain an API key from https://console.x.ai."}"#,
            ),
            (
                "Black Forest Labs",
                422,
                r#"{"detail":"Invalid API key format"}"#,
            ),
        ];
        for (name, status, body) in captured {
            let v = explain(status, body, name);
            assert!(
                !v.ok,
                "{name} answers a bad key with {status} and must be caught, got: {}",
                v.detail
            );
        }
    }

    #[test]
    fn google_and_xai_reject_with_400_and_are_still_caught() {
        // Verified live: both answer a bad key with 400, not 401. Trusting the
        // status alone would report a dead key as working.
        let g = explain(
            400,
            r#"{"error":{"message":"API key not valid."}}"#,
            "Google",
        );
        assert!(!g.ok, "Google 400 must be treated as a rejection");
        let x = explain(400, r#"{"error":"Incorrect API key provided."}"#, "xAI");
        assert!(!x.ok, "xAI 400 must be treated as a rejection");
    }

    #[test]
    fn a_400_about_the_throwaway_probe_id_still_proves_the_key() {
        // Several probes deliberately reference an id that cannot exist. The
        // request had to authenticate to get as far as being told so.
        assert!(explain(400, "unknown request id", "fal.ai").ok);
    }

    #[test]
    fn an_ambiguous_status_never_claims_the_key_works() {
        // The bug this guards: 404 used to map to success, so a bogus Black
        // Forest Labs key reported as valid.
        let v = explain(404, "", "Black Forest Labs");
        assert!(!v.ok);
        assert!(v.detail.contains("could not verify"), "got: {}", v.detail);
        assert!(v.detail.contains("may still be fine"));
    }

    #[test]
    fn local_needs_no_key_and_reports_what_it_found() {
        let v = validate(ProviderId::Local, "", None);
        // Whichever way detection goes, the message must name the ports so the
        // user knows what to start.
        if v.ok {
            assert!(v.detail.contains("ComfyUI") || v.detail.contains("Ollama"));
        } else {
            assert!(v.detail.contains("8188") && v.detail.contains("11434"));
        }
    }
}
