//! Route resolution: which provider actually serves a given model.
//!
//! A logical model (`kling-3.0`) has an ordered list of routes, each naming a
//! provider and the slug that provider knows it by. The resolver picks the
//! first route the user holds a key for, unless they pinned one or asked for
//! the cheapest.
//!
//! Higgsfield's internal model ids are byte-for-byte fal slugs minus the
//! `fal-ai/` prefix, which is why the fal column of this table reads like a
//! copy of theirs. That is the single largest saving available to us and the
//! reason fal anchors the roster.

use crate::provider::ProviderId;
use serde::{Deserialize, Serialize};

/// One way to reach a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub provider: ProviderId,
    /// The provider's own identifier for this model.
    pub slug: String,
    /// Why this route is ordered where it is. Surfaced in the UI so a user can
    /// see *why* we picked a route, rather than being told to trust us.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Route {
    pub fn new(provider: ProviderId, slug: &str) -> Self {
        Route {
            provider,
            slug: slug.to_string(),
            note: None,
        }
    }

    pub fn noted(provider: ProviderId, slug: &str, note: &str) -> Self {
        Route {
            provider,
            slug: slug.to_string(),
            note: Some(note.to_string()),
        }
    }

    pub fn id(&self) -> String {
        format!("{}:{}", self.provider.slug(), self.slug)
    }
}

/// How to choose among the routes a user can actually reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RoutePolicy {
    /// Lowest estimated USD. The default — the whole point of the product is
    /// that the user pays cost, so defaulting to anything else would be odd.
    #[default]
    Cheapest,
    /// Preserve the authored order, which is roughly quality-first.
    Preferred,
}

/// Why a model could not be reached.
///
/// Four variants rather than two, because they need four different fixes and
/// conflating them produces the "add the key you just added" dead end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// The user holds no key for any provider that serves this model, and at
    /// least one of them is something they could add.
    NoConfiguredProvider { needed: Vec<ProviderId> },
    /// The user *does* hold a key, but Halation has no client for that provider
    /// yet. Nothing the user can do — do not send them to Settings.
    NoAdapter { held: Vec<ProviderId> },
    /// The pinned route is not one this model offers.
    UnknownPin(String),
    /// The pinned route exists but cannot be executed.
    PinUnusable { id: String, why: &'static str },
}

fn names(ps: &[ProviderId]) -> String {
    ps.iter()
        .map(|p| p.display_name())
        .collect::<Vec<_>>()
        .join(", ")
}

impl std::fmt::Display for RouteError {
    /// Every string here reaches the user, so each one names the actual
    /// remedy — or says plainly that there isn't one.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::NoConfiguredProvider { needed } => {
                write!(f, "add a key for one of: {}", names(needed))
            }
            RouteError::NoAdapter { held } => write!(
                f,
                "Halation cannot reach this model through {} yet — your key is fine, the adapter is not built. Pick a model served by fal.",
                names(held)
            ),
            RouteError::UnknownPin(id) => write!(f, "no such route for this model: {id}"),
            RouteError::PinUnusable { id, why } => {
                write!(f, "the pinned route {id} cannot be used: {why}")
            }
        }
    }
}

/// Pick a route.
///
/// `available` is the set of providers the user has configured. `pinned` is an
/// explicit `provider:slug` override. `cost_of` returns an estimate for a route
/// when one can be computed; routes with no known price sort last rather than
/// being treated as free, because "unknown" beating "$0.03" would be a nasty
/// way to spend someone's money.
pub fn resolve<'a, F>(
    routes: &'a [Route],
    available: &[ProviderId],
    policy: RoutePolicy,
    pinned: Option<&str>,
    cost_of: F,
) -> Result<&'a Route, RouteError>
where
    F: Fn(&Route) -> Option<f64>,
{
    if let Some(pin) = pinned {
        let route = routes
            .iter()
            .find(|r| r.id() == pin)
            .ok_or_else(|| RouteError::UnknownPin(pin.to_string()))?;
        // A pin is an explicit user choice, so it is honoured over the cheapest
        // policy — but it is still checked, or pinning becomes a way to reach
        // the same dead end deliberately.
        if !route.provider.has_adapter() {
            return Err(RouteError::PinUnusable {
                id: pin.to_string(),
                why: "Halation has no client for that provider yet",
            });
        }
        if route.provider.needs_key() && !available.contains(&route.provider) {
            return Err(RouteError::PinUnusable {
                id: pin.to_string(),
                why: "no key is set for that provider",
            });
        }
        return Ok(route);
    }

    // Two conditions, not one. A key without an adapter is unreachable, and
    // saying so honestly is what keeps the cheapest-route policy from picking
    // a route it cannot execute.
    let reachable: Vec<&Route> = routes
        .iter()
        .filter(|r| available.contains(&r.provider) && r.provider.has_adapter())
        .collect();

    if reachable.is_empty() {
        // Distinguish "you have a key we can't use" from "you have no key".
        // Same symptom, opposite remedies.
        let held: Vec<ProviderId> = routes
            .iter()
            .map(|r| r.provider)
            .filter(|p| available.contains(p) && !p.has_adapter())
            .collect();
        if !held.is_empty() {
            let mut held = held;
            held.dedup();
            return Err(RouteError::NoAdapter { held });
        }
        let mut needed: Vec<ProviderId> = routes
            .iter()
            .map(|r| r.provider)
            .filter(|p| p.has_adapter())
            .collect();
        needed.dedup();
        if needed.is_empty() {
            // Every route for this model goes through a provider we cannot yet
            // execute. Telling the user to "add a key for one of: " with an
            // empty list would be worse than useless.
            let mut all: Vec<ProviderId> = routes.iter().map(|r| r.provider).collect();
            all.dedup();
            return Err(RouteError::NoAdapter { held: all });
        }
        return Err(RouteError::NoConfiguredProvider { needed });
    }

    match policy {
        RoutePolicy::Preferred => Ok(reachable[0]),
        RoutePolicy::Cheapest => Ok(reachable
            .iter()
            .copied()
            .min_by(|a, b| match (cost_of(a), cost_of(b)) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                // A known price always beats an unknown one.
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .expect("reachable is non-empty")),
    }
}

/// The cheaper alternative to whatever was chosen, if one exists. Drives the
/// "you could save $x on {provider}" nudge — Higgsfield gives users no route
/// choice at all, so surfacing it is a real differentiator rather than clutter.
pub fn cheaper_alternative<'a, F>(
    routes: &'a [Route],
    available: &[ProviderId],
    chosen: &Route,
    cost_of: F,
) -> Option<(&'a Route, f64)>
where
    F: Fn(&Route) -> Option<f64>,
{
    let chosen_cost = cost_of(chosen)?;
    routes
        .iter()
        // Same two conditions as resolve(). Nudging someone toward a cheaper
        // route we cannot execute would be worse than not nudging at all.
        .filter(|r| {
            available.contains(&r.provider) && r.provider.has_adapter() && r.id() != chosen.id()
        })
        .filter_map(|r| cost_of(r).map(|c| (r, c)))
        .filter(|(_, c)| *c < chosen_cost)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(r, c)| (r, chosen_cost - c))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately mixes providers we can execute (fal, Higgsfield, Local)
    /// with one we cannot (Vaig), because the interesting bug lives exactly
    /// there: Vaig is the *cheapest* and the *unreachable* one, which is also
    /// true in the real registry.
    fn routes() -> Vec<Route> {
        vec![
            Route::new(ProviderId::Fal, "fal-ai/kling-video/v3/pro"),
            Route::new(ProviderId::Higgsfield, "higgsfield-ai/dop/standard"),
            Route::new(ProviderId::Vaig, "klingai/kling-v3.0-pro"),
            Route::new(ProviderId::Local, "comfy/kling"),
        ]
    }

    fn priced(r: &Route) -> Option<f64> {
        match r.provider {
            ProviderId::Fal => Some(0.67),
            ProviderId::Higgsfield => Some(0.42),
            // Cheapest, and unreachable. If this ever wins, the resolver is
            // picking routes it cannot execute.
            ProviderId::Vaig => Some(0.30),
            ProviderId::Local => Some(0.0),
            _ => None,
        }
    }

    #[test]
    fn cheapest_wins_by_default() {
        let rs = routes();
        let got = resolve(
            &rs,
            &[ProviderId::Fal, ProviderId::Higgsfield],
            RoutePolicy::Cheapest,
            None,
            priced,
        )
        .unwrap();
        assert_eq!(got.provider, ProviderId::Higgsfield);
    }

    #[test]
    fn a_cheaper_route_we_cannot_execute_never_wins() {
        // THE regression test for A1. Vaig is $0.30 against fal's $0.67 and the
        // user holds a Vaig key — but no client exists for it, so choosing it
        // produced "no credentials for Vercel AI Gateway — add a key in
        // Settings" for a user who had just added exactly that key.
        let rs = routes();
        let got = resolve(
            &rs,
            &[ProviderId::Fal, ProviderId::Vaig],
            RoutePolicy::Cheapest,
            None,
            priced,
        )
        .unwrap();
        assert_eq!(got.provider, ProviderId::Fal);
    }

    #[test]
    fn a_key_we_cannot_use_is_not_reported_as_a_missing_key() {
        // The user holds only Vaig. Sending them to Settings would be telling
        // them to do the thing they already did; the honest answer is that the
        // adapter does not exist.
        let rs = routes();
        let err = resolve(
            &rs,
            &[ProviderId::Vaig],
            RoutePolicy::Cheapest,
            None,
            priced,
        )
        .unwrap_err();
        assert!(
            matches!(err, RouteError::NoAdapter { .. }),
            "expected NoAdapter, got {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("your key is fine"), "got: {msg}");
        assert!(
            !msg.contains("add a key"),
            "must not send them to Settings: {msg}"
        );
    }

    #[test]
    fn holding_no_key_at_all_still_asks_for_one() {
        // And it must only offer providers we could actually use.
        let rs = routes();
        let err = resolve(&rs, &[], RoutePolicy::Cheapest, None, priced).unwrap_err();
        match err {
            RouteError::NoConfiguredProvider { needed } => {
                assert!(needed.contains(&ProviderId::Fal));
                assert!(
                    !needed.contains(&ProviderId::Vaig),
                    "must not suggest a key we cannot use: {needed:?}"
                );
            }
            other => panic!("expected NoConfiguredProvider, got {other:?}"),
        }
    }

    #[test]
    fn pinning_an_unusable_route_explains_itself() {
        // Pinning is an explicit choice and is honoured over policy — but it is
        // still checked, or it becomes a deliberate route to the same dead end.
        let rs = routes();
        let err = resolve(
            &rs,
            &[ProviderId::Fal, ProviderId::Vaig],
            RoutePolicy::Cheapest,
            Some("vaig:klingai/kling-v3.0-pro"),
            priced,
        )
        .unwrap_err();
        assert!(matches!(err, RouteError::PinUnusable { .. }), "got {err:?}");
        assert!(err.to_string().contains("no client"), "{err}");
    }

    #[test]
    fn pinning_a_route_with_no_key_says_so_specifically() {
        let rs = routes();
        let err = resolve(
            &rs,
            &[ProviderId::Fal],
            RoutePolicy::Cheapest,
            Some("higgsfield:higgsfield-ai/dop/standard"),
            priced,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no key is set"), "{err}");
    }

    #[test]
    fn preferred_keeps_authored_order() {
        let rs = routes();
        let got = resolve(
            &rs,
            &[ProviderId::Fal, ProviderId::Vaig],
            RoutePolicy::Preferred,
            None,
            priced,
        )
        .unwrap();
        assert_eq!(got.provider, ProviderId::Fal);
    }

    #[test]
    fn unconfigured_providers_are_skipped() {
        let rs = routes();
        let got = resolve(&rs, &[ProviderId::Fal], RoutePolicy::Cheapest, None, priced).unwrap();
        assert_eq!(got.provider, ProviderId::Fal);
    }

    #[test]
    fn no_key_reports_what_would_help() {
        let rs = routes();
        let err = resolve(&rs, &[], RoutePolicy::Cheapest, None, priced).unwrap_err();
        match err {
            RouteError::NoConfiguredProvider { ref needed } => {
                assert!(needed.contains(&ProviderId::Fal));
            }
            _ => panic!("wrong error: {err:?}"),
        }
        // The message must name providers, not scold the user.
        assert!(err.to_string().contains("fal.ai"));
    }

    #[test]
    fn a_pin_overrides_policy() {
        let rs = routes();
        let got = resolve(
            &rs,
            &[ProviderId::Fal, ProviderId::Vaig],
            RoutePolicy::Cheapest,
            Some("fal:fal-ai/kling-video/v3/pro"),
            priced,
        )
        .unwrap();
        assert_eq!(got.provider, ProviderId::Fal);
    }

    #[test]
    fn a_bad_pin_is_an_error_not_a_silent_fallback() {
        let rs = routes();
        assert!(matches!(
            resolve(
                &rs,
                &[ProviderId::Fal],
                RoutePolicy::Cheapest,
                Some("nope:nope"),
                priced
            ),
            Err(RouteError::UnknownPin(_))
        ));
    }

    #[test]
    fn unknown_price_never_beats_a_known_one() {
        // fal has no price here; Higgsfield does. Cheapest must not pick the
        // unknown one — "unknown" beating "$9.99" is a nasty way to spend
        // someone's money.
        let rs = routes();
        let got = resolve(
            &rs,
            &[ProviderId::Fal, ProviderId::Higgsfield],
            RoutePolicy::Cheapest,
            None,
            |r| match r.provider {
                ProviderId::Higgsfield => Some(9.99),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(
            got.provider,
            ProviderId::Higgsfield,
            "unknown price must sort last"
        );
    }

    #[test]
    fn suggests_a_cheaper_route_with_the_saving() {
        let rs = routes();
        let chosen = &rs[0]; // fal, $0.67
        let (alt, saving) = cheaper_alternative(
            &rs,
            &[ProviderId::Fal, ProviderId::Higgsfield],
            chosen,
            priced,
        )
        .unwrap();
        assert_eq!(alt.provider, ProviderId::Higgsfield);
        assert!((saving - 0.25).abs() < 1e-9, "saving was {saving}");
    }

    #[test]
    fn no_suggestion_when_already_cheapest() {
        let rs = routes();
        let chosen = &rs[1]; // Higgsfield, $0.42 — cheapest of the configured two
        assert!(cheaper_alternative(
            &rs,
            &[ProviderId::Fal, ProviderId::Higgsfield],
            chosen,
            priced
        )
        .is_none());
    }
}
