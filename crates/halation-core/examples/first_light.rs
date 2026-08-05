//! First light: prove the provider integration against a real fal account.
//!
//! Everything in the registry is transcribed from documents. Nothing in it has
//! ever been sent to a provider, and several route slugs carry
//! `unverified slug` notes precisely because the corpus never recorded them.
//! This binary is the first contact.
//!
//! It deliberately exercises the three things most likely to be wrong, in the
//! order they would fail during a real generation:
//!
//! 1. **Auth and reachability** — is `Key {key}` the right header shape.
//! 2. **The upload path** — `media.rs` has never moved a byte.
//! 3. **A real generation**, cheapest available, and the actual charge against
//!    the estimate the Generate button would have shown.
//!
//! The key is read from `FAL_KEY` so it never has to be written into a file or
//! passed on a command line. Run it as:
//!
//! ```sh
//! FAL_KEY=$(security find-generic-password -s ai.halation.keys -a fal -w) \
//!   cargo run -p halation-core --example first_light
//! ```

use halation_core::clients::FalClient;
use halation_core::engine::ProviderClient;
use halation_core::media::{self, Dialect, MediaRef, MediaRole, Uploader};
use halation_core::{registry, Billable};
use std::time::{Duration, Instant};

fn main() {
    let key = match std::env::var("FAL_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("FAL_KEY is not set. See the module docs for the invocation.");
            std::process::exit(2);
        }
    };
    let client = FalClient::new(key);

    // ── 1. Upload ──────────────────────────────────────────────────────────
    // A tiny real PNG rather than a fixture URL: the point is to prove our own
    // upload path, not fal's CDN.
    println!("== upload ==");
    let tmp = std::env::temp_dir().join("halation-first-light.png");
    std::fs::write(&tmp, ONE_PIXEL_PNG).expect("write temp png");
    let uploaded = match client.upload(tmp.to_str().unwrap()) {
        Ok(url) => {
            println!("  ok    {url}");
            Some(url)
        }
        Err(e) => {
            println!("  FAIL  {e}");
            None
        }
    };

    // ── 2. Binding ─────────────────────────────────────────────────────────
    // Cheap, local, and the thing the dialect fix was for: confirm the body we
    // would send uses fal's parameter names and not the catalogue's.
    println!("\n== binding ==");
    let reg = registry();
    if let (Some(model), Some(url)) = (reg.get("kling3_0"), uploaded.as_ref()) {
        let route = model
            .routes
            .iter()
            .find(|r| r.provider == halation_core::ProviderId::Fal);
        if let Some(route) = route {
            let bound = media::bind(
                &model.spec,
                &model.display_name,
                &[MediaRef::url(MediaRole::Start, url.clone())],
                Dialect::Fal,
                &route.slug,
            );
            match bound {
                Ok(body) => println!("  ok    {}", serde_json::to_string(&body).unwrap()),
                Err(e) => println!("  FAIL  {e}"),
            }
        }
    }

    // ── 3. A real generation ───────────────────────────────────────────────
    // The cheapest image model we can actually route to, so first contact costs
    // cents rather than dollars.
    println!("\n== generate ==");
    let billable = Billable::image(1);
    // Image-only by default so an unattended run costs cents; MODEL lifts it.
    let want = std::env::var("MODEL").ok();
    let mut candidates: Vec<_> = reg
        .values()
        .filter(|m| want.is_some() || format!("{}", m.modality) == "image")
        .filter_map(|m| {
            let r = m
                .routes
                .iter()
                .find(|r| r.provider == halation_core::ProviderId::Fal)?;
            let est = m.estimate(r, &billable)?;
            Some((m, r, est.usd))
        })
        .collect();
    candidates.sort_by(|a, b| a.2.total_cmp(&b.2));

    // `MODEL=<id>` overrides the cheapest-first pick. Needed because the
    // registry's slugs are *family roots* and most video routes 404 until the
    // input-mode suffix resolver exists — see docs/FIRST-LIGHT.md.
    if let Some(want) = want.as_deref() {
        candidates.retain(|(m, _, _)| m.id == want);
        if candidates.is_empty() {
            println!("  FAIL  MODEL={want} has no priced fal route");
            return;
        }
    }

    let Some((model, route, estimate)) = candidates.first() else {
        println!("  FAIL  no priced image model routes through fal");
        return;
    };
    println!("  model {} via {}", model.display_name, route.slug);
    println!("  est   ${estimate:.4}");

    // Suffix the family root exactly as the shell does, or a video route 404s.
    let endpoint = match media::resolve_endpoint(
        &route.slug,
        media::InputMode::Text,
        format!("{}", model.modality) == "video",
    ) {
        Ok(e) => e,
        Err(e) => {
            println!("  FAIL  {} — {e}", model.display_name);
            return;
        }
    };
    println!("  ep    {endpoint}");

    let body = serde_json::json!({
        "prompt": "a paper boat drifting on still water at dawn",
        "duration": 4,
        "resolution": "720p",
    });
    let started = Instant::now();
    let submission = match client.submit(&endpoint, &body) {
        Ok(s) => {
            println!("  sent  {}", s.request_id);
            s
        }
        Err(e) => {
            // The most valuable failure this run can produce: it means the slug
            // is wrong, and several are flagged unverified in the registry.
            println!("  FAIL  submit rejected: {e}");
            println!(
                "        slug `{}` may be wrong — check the registry note",
                route.slug
            );
            return;
        }
    };

    loop {
        if started.elapsed() > Duration::from_secs(180) {
            println!("  FAIL  no terminal status in 3 minutes");
            return;
        }
        std::thread::sleep(Duration::from_millis(700));
        match client.poll(
            submission.status_url.as_deref().unwrap_or(&endpoint),
            &submission.request_id,
        ) {
            Ok(p) if p.status.is_terminal() => {
                println!("  done  {:?} in {:?}", p.status, started.elapsed());
                for o in &p.outputs {
                    println!("  out   {} ({:?})", o.url, o.kind);
                }
                match p.actual_usd {
                    Some(actual) => {
                        let delta = (actual - estimate).abs();
                        println!("  cost  actual ${actual:.4} vs estimate ${estimate:.4}");
                        if delta > estimate * 0.15 {
                            println!("  WARN  estimate is off by more than 15%");
                        }
                    }
                    None => println!("  cost  provider reported no charge on this response"),
                }
                if let Some(r) = p.fail_reason {
                    println!("  why   {r}");
                }
                return;
            }
            Ok(p) => print!("\r  ...   {:?}   ", p.status),
            Err(e) => {
                println!("\n  FAIL  poll: {e}");
                return;
            }
        }
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
}

/// A 1x1 opaque PNG. Smallest thing that is unambiguously a real image.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];
