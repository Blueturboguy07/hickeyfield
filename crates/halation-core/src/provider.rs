//! Provider identity.
//!
//! Nine providers ship at launch. Eight are hosted and take a user-supplied
//! key; `Local` is an auto-detected ComfyUI or Ollama endpoint and takes none.
//!
//! There is no proxy and no server-side key: every request is made from this
//! process with a credential read out of the OS keychain at assembly time.

use crate::engine::TimeoutPolicy;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Fal,
    /// Vercel AI Gateway — a routing target, not our host.
    Vaig,
    Google,
    OpenAi,
    XAi,
    /// Black Forest Labs (FLUX).
    Bfl,
    Recraft,
    /// Higgsfield's own public API. The only literal-Soul and literal-DoP path.
    /// Auth is `Authorization: Key {key}:{secret}` — not Bearer.
    Higgsfield,
    /// Auto-detected local inference. No key, no cost.
    Local,
}

impl ProviderId {
    pub const ALL: [ProviderId; 9] = [
        ProviderId::Fal,
        ProviderId::Vaig,
        ProviderId::Google,
        ProviderId::OpenAi,
        ProviderId::XAi,
        ProviderId::Bfl,
        ProviderId::Recraft,
        ProviderId::Higgsfield,
        ProviderId::Local,
    ];

    /// Stable string used as the keychain account name and in route ids.
    pub fn slug(self) -> &'static str {
        match self {
            ProviderId::Fal => "fal",
            ProviderId::Vaig => "vaig",
            ProviderId::Google => "google",
            ProviderId::OpenAi => "openai",
            ProviderId::XAi => "xai",
            ProviderId::Bfl => "bfl",
            ProviderId::Recraft => "recraft",
            ProviderId::Higgsfield => "higgsfield",
            ProviderId::Local => "local",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Fal => "fal.ai",
            ProviderId::Vaig => "Vercel AI Gateway",
            ProviderId::Google => "Google",
            ProviderId::OpenAi => "OpenAI",
            ProviderId::XAi => "xAI",
            ProviderId::Bfl => "Black Forest Labs",
            ProviderId::Recraft => "Recraft",
            ProviderId::Higgsfield => "Higgsfield",
            ProviderId::Local => "Local",
        }
    }

    /// Whether a client exists that can actually execute a request here.
    ///
    /// **This is deliberately separate from [`Self::needs_key`].** Holding a key
    /// is necessary but not sufficient: the shell's `client_for` implements fal
    /// and Higgsfield today and returns `None` for the rest. Without this
    /// distinction the resolver happily picks the cheapest *reachable* route,
    /// which for several models is Vercel AI Gateway — and the user who has just
    /// added a Vaig key is then told "no credentials for Vercel AI Gateway — add
    /// a key in Settings". Being instructed to do the thing you just did is the
    /// worst failure shape a BYO-key product has, and it fires on the default
    /// policy.
    ///
    /// Keep this list in step with `src-tauri/src/app.rs::client_for`. The test
    /// `app::tests::has_adapter_matches_the_clients_actually_implemented` fails
    /// loudly if they drift.
    pub fn has_adapter(self) -> bool {
        matches!(
            self,
            ProviderId::Fal | ProviderId::Higgsfield | ProviderId::Local
        )
    }

    /// `Local` is the only provider that never needs a credential.
    pub fn needs_key(self) -> bool {
        !matches!(self, ProviderId::Local)
    }

    /// Higgsfield issues a key/secret pair rather than a single token.
    pub fn needs_secret(self) -> bool {
        matches!(self, ProviderId::Higgsfield)
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        ProviderId::ALL.into_iter().find(|p| p.slug() == s)
    }

    /// The operational limits of this provider, as data.
    ///
    /// These are the numbers the runner needs and does not currently have. It
    /// spawns one poll loop per job with no limiter, so a user who queues six
    /// generations against a fal account that allows two in flight makes their
    /// own jobs rate-limit each other — the app amplifies the throttling it is
    /// supposed to respect. And a single flat timeout for every provider gives
    /// a local GPU the same ten minutes as a datacenter.
    ///
    /// Kept here rather than on [`crate::engine::ProviderClient`] so the runner
    /// can consult them *before* deciding to start a job, which is the only
    /// point at which a concurrency limit can still be honoured.
    pub const fn features(self) -> Features {
        match self {
            ProviderId::Fal => Features {
                // Two in flight on a new account. Sourced from this repo's own
                // research — the same figure the module docs of `clients.rs`
                // and `runner.rs` and `docs/BRIDGE.md` §4 item 11 all state —
                // and *not* re-read from fal's docs, because their site 429s
                // automated fetches (the trap recorded in `docs/PARITY.md`
                // §3.1). Re-verify by hand before treating it as exact.
                max_concurrent: Concurrency::Provider(2),
                // fal's own SDK polls at 500ms and `FalClient` matches it;
                // `features_agree_with_the_clients_that_exist` keeps the two in
                // step so wiring the runner onto Features cannot silently
                // change the request rate.
                poll_interval: Duration::from_millis(500),
                timeout: TimeoutPolicy::HOSTED,
            },
            // One GPU, one job. A second concurrent local generation does not
            // finish sooner; it contends for the same VRAM as the first.
            ProviderId::Local => Features {
                max_concurrent: Concurrency::SelfImposed(1),
                // Localhost has no rate limit to respect and the user is
                // watching their own machine finish, so the only thing a slower
                // interval buys is dead time after the render completes.
                poll_interval: Duration::from_secs(1),
                timeout: TimeoutPolicy::LOCAL,
            },
            _ => Features {
                max_concurrent: Concurrency::SelfImposed(4),
                // The generic 3s of `ProviderClient::poll_interval`, which is
                // what `HiggsfieldClient` uses today.
                poll_interval: Duration::from_secs(3),
                timeout: TimeoutPolicy::HOSTED,
            },
        }
    }
}

/// What one provider will tolerate, separated from how it is spelled on the
/// wire.
///
/// A struct rather than three methods on [`ProviderId`] so the whole policy for
/// a provider reads as one block and a new provider cannot be added with one of
/// the three quietly missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features {
    /// Jobs this provider may have in flight at once.
    pub max_concurrent: Concurrency,
    /// How often to poll one running job.
    pub poll_interval: Duration,
    /// How long to keep polling before giving up. Scales with the requested
    /// work; see [`TimeoutPolicy`].
    pub timeout: TimeoutPolicy,
}

/// A ceiling on simultaneous in-flight jobs, carrying **where the number came
/// from**.
///
/// The distinction is not decorative, and it is not "fixed vs adjustable" —
/// both are raisable, because account tiers move a provider's real allowance
/// and a user on a paid fal plan must not be pinned to a free account's two.
/// What it records is whether the figure is the provider's or ours, which is
/// the only thing that tells the UI what to say when the user goes to change
/// it: overshooting a provider's limit earns 429s across every running job at
/// once, while overshooting ours costs nothing but sockets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concurrency {
    /// A ceiling the provider is understood to enforce. Treat as a safe floor
    /// for the cheapest account tier, not as a universal truth.
    Provider(u32),
    /// No provider limit we have verified. The number is Halation's own and is
    /// **not** a claim about the provider's policy. It exists because
    /// "unbounded" is the wrong default for a desktop app that would otherwise
    /// open one socket per job the user clicks.
    SelfImposed(u32),
}

impl Concurrency {
    /// Jobs allowed in flight. Never zero — a limiter handed zero permits stops
    /// the queue forever, and a stuck queue looks exactly like a hung app.
    pub const fn permits(self) -> u32 {
        match self {
            Concurrency::Provider(n) | Concurrency::SelfImposed(n) => {
                if n == 0 {
                    1
                } else {
                    n
                }
            }
        }
    }

    /// Whether the figure came from the provider rather than from us. Drives
    /// the copy — "fal allows 2 at a time" is a statement about fal, "up to 4
    /// at a time" is a statement about Halation, and saying the second in the
    /// voice of the first would be inventing a provider policy.
    pub const fn is_provider_stated(self) -> bool {
        matches!(self, Concurrency::Provider(_))
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_unique_and_round_trip() {
        let mut seen = std::collections::HashSet::new();
        for p in ProviderId::ALL {
            assert!(seen.insert(p.slug()), "duplicate slug {}", p.slug());
            assert_eq!(ProviderId::from_slug(p.slug()), Some(p));
        }
        assert_eq!(seen.len(), 9);
    }

    #[test]
    fn only_local_is_keyless() {
        let keyless: Vec<_> = ProviderId::ALL
            .into_iter()
            .filter(|p| !p.needs_key())
            .collect();
        assert_eq!(keyless, vec![ProviderId::Local]);
    }

    #[test]
    fn only_higgsfield_needs_a_secret() {
        let with_secret: Vec<_> = ProviderId::ALL
            .into_iter()
            .filter(|p| p.needs_secret())
            .collect();
        assert_eq!(with_secret, vec![ProviderId::Higgsfield]);
    }

    #[test]
    fn unknown_slug_is_none() {
        assert_eq!(ProviderId::from_slug("replicate"), None);
    }

    #[test]
    fn every_provider_permits_at_least_one_job() {
        // Zero permits does not throttle a queue, it stops it — and a stopped
        // queue is indistinguishable from a hung app, with no error anywhere.
        for p in ProviderId::ALL {
            assert!(
                p.features().max_concurrent.permits() >= 1,
                "{p} would deadlock the runner"
            );
        }
    }

    #[test]
    fn fal_is_capped_at_the_limit_fal_itself_enforces() {
        // The bug this bounds: the runner starts one poll loop per job with no
        // limiter, so six queued generations against a new fal account — which
        // allows two in flight — 429 each other. The app amplifies the
        // throttling it is meant to respect.
        let c = ProviderId::Fal.features().max_concurrent;
        assert_eq!(c, Concurrency::Provider(2));
        assert!(
            c.is_provider_stated(),
            "the UI has to attribute this number to fal, not to us"
        );
    }

    #[test]
    fn our_own_guesses_are_marked_as_guesses() {
        // Everything that is not fal is a number we chose, not one a provider
        // published. Losing that distinction is how a self-imposed default
        // becomes an unremovable throttle on a paying account.
        for p in ProviderId::ALL {
            if p == ProviderId::Fal {
                continue;
            }
            assert!(
                matches!(p.features().max_concurrent, Concurrency::SelfImposed(_)),
                "{p} claims a provider-stated limit we have not verified"
            );
        }
    }

    #[test]
    fn local_inference_runs_one_job_at_a_time() {
        let f = ProviderId::Local.features();
        assert_eq!(f.max_concurrent.permits(), 1);
        // Nothing is billed while a local GPU works, so the deadline is longer
        // than a hosted one for identical work.
        let work = crate::cost::Billable::video(5.0, 1280, 720);
        assert!(
            f.timeout.timeout(Some(&work))
                > ProviderId::Fal.features().timeout.timeout(Some(&work))
        );
    }

    #[test]
    fn features_agree_with_the_clients_that_exist() {
        // Two sources of truth for the poll rate. Wiring the runner onto
        // `Features` must not silently change how often we hit a provider, so
        // they are pinned together until `ProviderClient::poll_interval` goes.
        use crate::clients::{FalClient, HiggsfieldClient};
        use crate::engine::ProviderClient;

        assert_eq!(
            ProviderId::Fal.features().poll_interval,
            FalClient::new("k").poll_interval()
        );
        assert_eq!(
            ProviderId::Higgsfield.features().poll_interval,
            HiggsfieldClient::new("k", "s").poll_interval()
        );
    }

    #[test]
    fn no_provider_is_polled_faster_than_fal() {
        // fal is the only one whose own SDK documents a sub-second rate. Any
        // other provider polled that hard is us inventing a budget.
        let fal = ProviderId::Fal.features().poll_interval;
        for p in ProviderId::ALL {
            if p == ProviderId::Fal {
                continue;
            }
            assert!(
                p.features().poll_interval >= fal,
                "{p} polls faster than fal without a published rate to justify it"
            );
        }
    }
}
