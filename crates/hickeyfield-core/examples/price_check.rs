//! What the live price feeds actually price, right now.
//!
//! Run: `cargo run -p hickeyfield-core --example price_check`
//!
//! The question this answers is not "does the fetcher compile" but "how many
//! of the routes a user can pick come back with a real number, and how many
//! are still being quoted from a literal someone typed". Coverage is the whole
//! point of wiring the feed in; a fetch that succeeds and prices four routes
//! would be a fetch that changed nothing.

use hickeyfield_core::cost::Billable;
use hickeyfield_core::prices::PriceFeed;
use hickeyfield_core::registry::registry;

fn main() {
    // A *complete* request. A per-second-tiered model cannot be priced without
    // a resolution — there is no way to know which tier applies — so probing
    // with a bare duration reports routes as unpriced that are merely
    // under-specified, which would send you hunting for a bug that is in the
    // probe.
    let b = Billable {
        seconds: Some(4.0),
        width: Some(1280),
        height: Some(720),
        images: Some(1),
        batch: 1,
        ..Default::default()
    };

    let bundled = PriceFeed::bundled();
    println!("bundled snapshot: {} routes", bundled.len());

    println!("fetching live feeds…");
    let live = PriceFeed::fetch();
    println!("live feed:        {} routes", live.len());
    match live.age() {
        Some(age) => println!("oldest price:     {}s old", age.as_secs()),
        None => println!("oldest price:     unknown"),
    }
    for p in live.problems() {
        println!("  ! {p}");
    }

    let reg = registry();
    let (mut from_feed, mut from_literal, mut unpriced) = (0, 0, 0);
    let mut changed: Vec<String> = Vec::new();

    for model in reg.values() {
        for route in &model.routes {
            let feed = live
                .cost_model(&route.id(), &b)
                .and_then(|m| m.estimate(&b));
            let literal = model.estimate(route, &b);
            match (&feed, &literal) {
                (Some(f), Some(l)) => {
                    from_feed += 1;
                    // A disagreement is the interesting case: it is the exact
                    // amount by which the hardcoded number was wrong.
                    if (f.usd - l.usd).abs() > 0.0001 {
                        changed.push(format!(
                            "  {:<52} literal ${:.4} → live ${:.4}",
                            route.id(),
                            l.usd,
                            f.usd
                        ));
                    }
                }
                (Some(_), None) => from_feed += 1,
                (None, Some(_)) => from_literal += 1,
                (None, None) => unpriced += 1,
            }
        }
    }

    println!("\nroutes priced by the live feed: {from_feed}");
    println!("routes still on a literal:      {from_literal}");
    println!("routes with no price at all:    {unpriced}");

    if changed.is_empty() {
        println!("\nno price disagreements.");
    } else {
        println!("\n{} route(s) where the literal was wrong:", changed.len());
        for line in &changed {
            println!("{line}");
        }
    }
}
