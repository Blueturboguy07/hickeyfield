//! Provider metadata and credential import.
//!
//! Getting a key into the app is the first thing a new user does and the only
//! thing standing between them and a working product, so it gets a real
//! surface rather than a settings afterthought.
//!
//! Two paths are supported deliberately. Most people already have these keys
//! in a `.env` somewhere, so pasting that blob and having it sorted out is far
//! faster than eight copy-pastes. The per-provider path exists for everyone
//! else.

use crate::provider::ProviderId;
use serde::Serialize;

/// What a user needs to know to decide whether to add a given key.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub slug: &'static str,
    pub display_name: &'static str,
    pub needs_key: bool,
    pub needs_secret: bool,
    /// Where to actually get one.
    pub key_url: &'static str,
    /// Conventional environment variable names, used both for `.env` import
    /// and as a hint in the UI.
    pub env_names: &'static [&'static str],
    /// One line on what this key unlocks. Concrete, not marketing.
    pub blurb: &'static str,
    /// fal is the backbone — it reaches most of the roster on one key, so a
    /// first-time user should start there rather than collecting eight.
    pub recommended: bool,
}

pub fn provider_info(p: ProviderId) -> ProviderInfo {
    match p {
        ProviderId::Fal => ProviderInfo {
            slug: "fal",
            display_name: "fal.ai",
            needs_key: true,
            needs_secret: false,
            key_url: "https://fal.ai/dashboard/keys",
            env_names: &["FAL_KEY", "FAL_API_KEY"],
            blurb: "Most of the roster on one key — Kling, Seedance, Wan, Veo, FLUX, upscaling.",
            recommended: true,
        },
        ProviderId::Vaig => ProviderInfo {
            slug: "vaig",
            display_name: "Vercel AI Gateway",
            needs_key: true,
            needs_secret: false,
            key_url: "https://vercel.com/dashboard/ai-gateway",
            env_names: &["AI_GATEWAY_API_KEY", "VERCEL_AI_GATEWAY_KEY"],
            blurb: "Cheaper than fal on MiniMax, Seedance 1.5 Pro and Seedream.",
            recommended: false,
        },
        ProviderId::Google => ProviderInfo {
            slug: "google",
            display_name: "Google",
            needs_key: true,
            needs_secret: false,
            key_url: "https://aistudio.google.com/apikey",
            env_names: &["GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENAI_API_KEY"],
            blurb: "Veo 3.1 and Nano Banana, direct and cheapest.",
            recommended: false,
        },
        ProviderId::OpenAi => ProviderInfo {
            slug: "openai",
            display_name: "OpenAI",
            needs_key: true,
            needs_secret: false,
            key_url: "https://platform.openai.com/api-keys",
            env_names: &["OPENAI_API_KEY"],
            blurb: "GPT Image 2.",
            recommended: false,
        },
        ProviderId::XAi => ProviderInfo {
            slug: "xai",
            display_name: "xAI",
            needs_key: true,
            needs_secret: false,
            key_url: "https://console.x.ai",
            env_names: &["XAI_API_KEY", "GROK_API_KEY"],
            blurb: "Grok Imagine — about 3x cheaper direct than through a gateway.",
            recommended: false,
        },
        ProviderId::Bfl => ProviderInfo {
            slug: "bfl",
            display_name: "Black Forest Labs",
            needs_key: true,
            needs_secret: false,
            key_url: "https://dashboard.bfl.ai",
            env_names: &["BFL_API_KEY"],
            blurb: "FLUX.2 and Kontext, direct from the people who train them.",
            recommended: false,
        },
        ProviderId::Recraft => ProviderInfo {
            slug: "recraft",
            display_name: "Recraft",
            needs_key: true,
            needs_secret: false,
            key_url: "https://www.recraft.ai/profile/api",
            env_names: &["RECRAFT_API_KEY"],
            blurb: "Vector and raster art. Note: prepaid and non-refundable.",
            recommended: false,
        },
        ProviderId::Higgsfield => ProviderInfo {
            slug: "higgsfield",
            display_name: "Higgsfield",
            needs_key: true,
            needs_secret: true,
            key_url: "https://cloud.higgsfield.ai",
            env_names: &["HF_KEY", "HIGGSFIELD_API_KEY"],
            blurb: "Optional. The only way to reach Soul and DoP, using your own account.",
            recommended: false,
        },
        ProviderId::Local => ProviderInfo {
            slug: "local",
            display_name: "Local",
            needs_key: false,
            needs_secret: false,
            key_url: "https://github.com/comfyanonymous/ComfyUI",
            env_names: &[],
            blurb: "Free. Auto-detected if ComfyUI or Ollama is running on this machine.",
            recommended: false,
        },
    }
}

pub fn all_provider_info() -> Vec<ProviderInfo> {
    ProviderId::ALL.into_iter().map(provider_info).collect()
}

/// One credential recovered from a `.env` blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedKey {
    pub provider: String,
    /// True when this line is the *secret* half of a key/secret pair.
    pub secret_half: bool,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportResult {
    pub keys: Vec<ParsedKey>,
    /// Variable names we saw but do not map to any provider. Reported so the
    /// user can tell "nothing happened" apart from "that one is not supported".
    pub unknown: Vec<String>,
}

/// Higgsfield issues a pair. Its `HF_KEY` convention packs both into one value
/// separated by a colon, which is also the wire format for the auth header.
const HF_SECRET_NAMES: [&str; 2] = ["HF_API_SECRET", "HIGGSFIELD_API_SECRET"];

fn provider_for_env(name: &str) -> Option<(ProviderId, bool)> {
    let upper = name.trim().to_ascii_uppercase();
    if HF_SECRET_NAMES.contains(&upper.as_str()) {
        return Some((ProviderId::Higgsfield, true));
    }
    for p in ProviderId::ALL {
        if provider_info(p).env_names.iter().any(|n| *n == upper) {
            return Some((p, false));
        }
    }
    None
}

/// Strip surrounding quotes, an `export ` prefix and trailing comments the way
/// a shell would, so a file that works with `source` works here.
fn clean_value(raw: &str) -> String {
    let v = raw.trim();
    let v = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        &v[1..v.len() - 1]
    } else {
        // Only strip a trailing comment on an unquoted value — inside quotes a
        // `#` is part of the secret, and some keys really do contain one.
        v.split(" #").next().unwrap_or(v).trim()
    };
    v.to_string()
}

/// Parse `.env`-style text into credentials.
///
/// Tolerant on purpose: people paste fragments of shell scripts, files with
/// CRLF line endings, and blocks copied out of a dashboard.
pub fn parse_env(text: &str) -> ImportResult {
    let mut out = ImportResult::default();

    for line in text.lines() {
        let line = line.trim().trim_start_matches("export ").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, raw)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = clean_value(raw);
        if value.is_empty() {
            continue;
        }

        match provider_for_env(name) {
            Some((ProviderId::Higgsfield, false)) if value.contains(':') => {
                // `HF_KEY=key:secret` — split it into the pair we store.
                let (k, s) = value.split_once(':').unwrap();
                out.keys.push(ParsedKey {
                    provider: "higgsfield".into(),
                    secret_half: false,
                    value: k.trim().to_string(),
                });
                out.keys.push(ParsedKey {
                    provider: "higgsfield".into(),
                    secret_half: true,
                    value: s.trim().to_string(),
                });
            }
            Some((p, secret_half)) => out.keys.push(ParsedKey {
                provider: p.slug().to_string(),
                secret_half,
                value,
            }),
            None => {
                if !out.unknown.iter().any(|u| u == name) {
                    out.unknown.push(name.to_string());
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_env_file() {
        let r = parse_env("FAL_KEY=abc123\nOPENAI_API_KEY=sk-xyz\n");
        assert_eq!(r.keys.len(), 2);
        assert_eq!(r.keys[0].provider, "fal");
        assert_eq!(r.keys[0].value, "abc123");
        assert_eq!(r.keys[1].provider, "openai");
        assert!(r.unknown.is_empty());
    }

    #[test]
    fn tolerates_the_shapes_people_actually_paste() {
        let r = parse_env(
            "# my keys\r\n\
             export FAL_KEY=\"quoted-value\"\r\n\
             \r\n\
             OPENAI_API_KEY='single'   \r\n\
             XAI_API_KEY=plain # trailing comment\r\n",
        );
        let by = |p: &str| r.keys.iter().find(|k| k.provider == p).unwrap();
        assert_eq!(by("fal").value, "quoted-value");
        assert_eq!(by("openai").value, "single");
        assert_eq!(by("xai").value, "plain");
    }

    #[test]
    fn a_hash_inside_a_quoted_secret_is_kept() {
        // Real keys contain '#'. Stripping it would corrupt the credential and
        // present as a mystifying 401.
        let r = parse_env("FAL_KEY=\"abc#def\"");
        assert_eq!(r.keys[0].value, "abc#def");
    }

    #[test]
    fn higgsfield_key_colon_secret_is_split_into_the_pair() {
        // Their own convention packs both halves into HF_KEY.
        let r = parse_env("HF_KEY=mykey:mysecret");
        assert_eq!(r.keys.len(), 2);
        assert!(!r.keys[0].secret_half);
        assert_eq!(r.keys[0].value, "mykey");
        assert!(r.keys[1].secret_half);
        assert_eq!(r.keys[1].value, "mysecret");
    }

    #[test]
    fn a_separate_secret_variable_also_works() {
        let r = parse_env("HIGGSFIELD_API_KEY=k\nHF_API_SECRET=s");
        assert_eq!(r.keys.len(), 2);
        assert!(r.keys.iter().any(|k| k.secret_half && k.value == "s"));
    }

    #[test]
    fn unknown_variables_are_reported_not_silently_dropped() {
        // "I pasted my file and nothing happened" is a terrible failure mode.
        let r = parse_env("SOME_OTHER_TOKEN=x\nDATABASE_URL=y\nFAL_KEY=z");
        assert_eq!(r.keys.len(), 1);
        assert_eq!(r.unknown, vec!["SOME_OTHER_TOKEN", "DATABASE_URL"]);
    }

    #[test]
    fn blank_values_and_comments_are_skipped() {
        let r = parse_env("# comment\nFAL_KEY=\nOPENAI_API_KEY=   \n\n");
        assert!(r.keys.is_empty());
        assert!(r.unknown.is_empty());
    }

    #[test]
    fn env_names_are_case_insensitive() {
        assert_eq!(parse_env("fal_key=v").keys.len(), 1);
    }

    #[test]
    fn every_provider_that_needs_a_key_says_where_to_get_one() {
        for info in all_provider_info() {
            if info.needs_key {
                assert!(
                    info.key_url.starts_with("https://"),
                    "{} has no key_url",
                    info.slug
                );
                assert!(!info.env_names.is_empty(), "{} has no env names", info.slug);
            }
            assert!(!info.blurb.is_empty(), "{} has no blurb", info.slug);
        }
    }

    #[test]
    fn exactly_one_provider_is_recommended() {
        // Recommending several defeats the point of recommending at all.
        let rec: Vec<_> = all_provider_info()
            .into_iter()
            .filter(|i| i.recommended)
            .map(|i| i.slug)
            .collect();
        assert_eq!(rec, vec!["fal"]);
    }

    #[test]
    fn env_names_do_not_collide_between_providers() {
        let mut seen = std::collections::HashMap::new();
        for info in all_provider_info() {
            for n in info.env_names {
                if let Some(prev) = seen.insert(*n, info.slug) {
                    panic!("{n} claimed by both {prev} and {}", info.slug);
                }
            }
        }
    }
}
