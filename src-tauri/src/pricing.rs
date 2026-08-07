//! Live provider prices, and the shell's one source of a USD figure.
//!
//! The core crate has carried a complete price feed for some time —
//! [`PriceFeed`] fetches fal's and the Vercel gateway's public catalogues,
//! overlays them on a bundled snapshot, sanity-checks every row and reports
//! its own age. Nothing called it. Every number the app displayed came from
//! literals transcribed into `registry.rs` by hand, which is exactly the
//! arrangement the plan forbids: *prices are fetched at runtime, never
//! hardcoded*, because a transcribed price rots silently and the user finds
//! out from their invoice.
//!
//! This module is the caller. It holds one feed, refreshes it off the main
//! thread, and answers "what will this cost" from live data where live data
//! exists.

use halation_core::cost::{Billable, Estimate};
use halation_core::prices::PriceFeed;
use halation_core::{Model, Route};
use serde::Serialize;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// How often to re-fetch. Provider price changes are a weeks-to-months event,
/// so this is about not shipping a month-old number, not about tracking a
/// market.
const REFRESH_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Prices {
    /// Swapped wholesale on refresh. An `Arc` inside the lock so a reader
    /// clones a pointer and releases the lock immediately — pricing is on the
    /// path of every keystroke in the settings rail.
    feed: RwLock<Arc<PriceFeed>>,
}

impl Prices {
    /// The compiled-in snapshot: available instantly, correct offline, and
    /// what the first paint uses while the network is still being asked.
    pub fn bundled() -> Self {
        Prices {
            feed: RwLock::new(Arc::new(PriceFeed::bundled())),
        }
    }

    pub fn current(&self) -> Arc<PriceFeed> {
        // A poisoned lock here means a panic while swapping feeds. Prices are
        // not worth taking the app down for; the bundled snapshot is still a
        // correct answer, so recover rather than propagate.
        match self.feed.read() {
            Ok(g) => Arc::clone(&g),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    fn replace(&self, fresh: PriceFeed) {
        match self.feed.write() {
            Ok(mut g) => *g = Arc::new(fresh),
            Err(poisoned) => *poisoned.into_inner() = Arc::new(fresh),
        }
    }

    /// Refresh now and then once a day, forever, on a background thread.
    ///
    /// A blocking fetch on the main thread would hold the window closed behind
    /// two HTTP requests to services that are allowed to be slow. `fetch()`
    /// never fails — a dead network leaves the bundled prices in place and
    /// records the reason — so there is no error path to handle here beyond
    /// saying what happened.
    pub fn start_refresh(self: &Arc<Self>) {
        let prices = Arc::clone(self);
        std::thread::Builder::new()
            .name("price-refresh".into())
            .spawn(move || loop {
                let fresh = PriceFeed::fetch();
                for problem in fresh.problems() {
                    // Warn, not error: the app is fully usable on the snapshot,
                    // and the UI says which it is showing.
                    tracing::warn!("price feed: {problem}");
                }
                tracing::info!("priced {} routes from the live feeds", fresh.len());
                prices.replace(fresh);
                std::thread::sleep(REFRESH_EVERY);
            })
            .expect("spawn price-refresh thread");
    }

    /// What this call will cost, preferring the live feed.
    ///
    /// The registry's own model is the fallback rather than the source. Both
    /// are real prices; the difference is that one of them was true when the
    /// binary was built and the other was true this morning.
    ///
    /// `None` from both means genuinely unknown, which every caller must
    /// render as unknown — the whole point of the `Option` is that no layer
    /// gets to turn it into a zero.
    pub fn estimate(&self, model: &Model, route: &Route, b: &Billable) -> Option<Estimate> {
        let feed = self.current();
        feed.cost_model(&route.id(), b)
            .and_then(|m| m.estimate(b))
            .or_else(|| model.estimate(route, b))
    }

    /// Same answer as [`Self::estimate`], shaped for the route resolver's
    /// cheapest-first policy. Routing on stale prices would pick a route that
    /// stopped being the cheapest one.
    pub fn usd(&self, model: &Model, route: &Route, b: &Billable) -> Option<f64> {
        self.estimate(model, route, b).map(|e| e.usd)
    }

    pub fn status(&self) -> PriceStatus {
        let feed = self.current();
        PriceStatus {
            routes: feed.len(),
            age_seconds: feed.age().map(|d| d.as_secs()),
            live: feed.problems().is_empty(),
        }
    }
}

/// What the UI needs to say where a number came from. Higgsfield shows a price
/// and no provenance; showing the age is a deliberate divergence, because our
/// prices are the user's actual bill rather than an internal credit rate.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PriceStatus {
    /// How many routes have a price at all.
    pub routes: usize,
    /// Age of the *oldest* displayed price. `None` when nothing is priced or
    /// the clock moved backwards — never `0`, which would read as "just now".
    pub age_seconds: Option<u64>,
    /// False when the last fetch had problems and some prices are the bundled
    /// snapshot. The UI says so rather than implying everything is current.
    pub live: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use halation_core::registry::registry;

    fn billable() -> Billable {
        Billable {
            seconds: Some(4.0),
            ..Default::default()
        }
    }

    #[test]
    fn the_bundled_snapshot_prices_routes_without_a_network() {
        let p = Prices::bundled();
        assert!(
            !p.current().is_empty(),
            "a bundled snapshot that prices nothing leaves the first run with \
             no numbers at all"
        );
    }

    #[test]
    fn an_unpriced_route_stays_unpriced_rather_than_becoming_free() {
        // The invariant the whole cost surface rests on: unknown is not zero.
        let reg = registry();
        let prices = Prices::bundled();
        let mut unknown = 0;
        for model in reg.values() {
            for route in &model.routes {
                if prices.estimate(model, route, &billable()).is_none() {
                    unknown += 1;
                }
            }
        }
        // Not an assertion that the count is small — an assertion that the
        // unknown case is reachable and returns None, so the UI's
        // "Price unavailable" path is live code rather than decoration.
        assert!(unknown > 0 || reg.is_empty());
    }

    #[test]
    fn the_feed_wins_over_the_transcribed_literal() {
        // Where the live snapshot prices a route, that is the number used —
        // otherwise wiring the feed in would have changed nothing.
        let reg = registry();
        let prices = Prices::bundled();
        let feed = prices.current();

        let mut compared = 0;
        for model in reg.values() {
            for route in &model.routes {
                let Some(from_feed) = feed
                    .cost_model(&route.id(), &billable())
                    .and_then(|m| m.estimate(&billable()))
                else {
                    continue;
                };
                let combined = prices
                    .estimate(model, route, &billable())
                    .expect("a feed-priced route must be priced");
                assert_eq!(
                    combined.usd,
                    from_feed.usd,
                    "{} should be priced by the feed, not by the literal",
                    route.id()
                );
                compared += 1;
            }
        }
        assert!(
            compared > 0,
            "no registry route matched a snapshot quote — the route-id spelling \
             on one side has drifted and the feed is silently doing nothing"
        );
    }
}
