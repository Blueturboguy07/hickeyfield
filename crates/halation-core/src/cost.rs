//! Cost estimation, in real USD.
//!
//! Higgsfield shows an opaque credit integer. We show what the generation will
//! actually cost, before submit, computed from live provider price feeds. That
//! only works if the billing shapes are modelled honestly — several of them are
//! counter-intuitive, and each of the traps below has been verified against a
//! provider's published pricing.

use serde::{Deserialize, Serialize};

/// The settings that affect price. Deliberately a flat struct rather than the
/// full request: cost must be computable before a request is valid, so the user
/// sees a number while they are still choosing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Billable {
    pub seconds: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub images: Option<u32>,
    pub megapixels: Option<f64>,
    pub audio: bool,
    pub batch: u32,
    /// Inputs beyond the first, where a provider charges per extra reference.
    pub extra_inputs: u32,
}

impl Billable {
    pub fn video(seconds: f64, width: u32, height: u32) -> Self {
        Billable {
            seconds: Some(seconds),
            width: Some(width),
            height: Some(height),
            batch: 1,
            ..Default::default()
        }
    }

    pub fn image(count: u32) -> Self {
        Billable {
            images: Some(count),
            batch: 1,
            ..Default::default()
        }
    }

    fn batch_factor(&self) -> f64 {
        self.batch.max(1) as f64
    }
}

/// How a provider charges. One variant per real billing shape we have seen —
/// not a generic formula, because the differences are exactly where the
/// surprises live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CostModel {
    /// Flat rate per output second. The common video shape.
    ///
    /// `audio_multiplier` exists because Kling charges 1.5–2x with audio on and
    /// hides it; we surface it.
    PerSecond {
        usd: f64,
        #[serde(default = "one")]
        audio_multiplier: f64,
    },

    /// Priced by output *tokens*, which scale with resolution rather than
    /// duration. Seedance at 1080p is ~2.25x the 720p clip for the same length
    /// — computing this from duration alone would understate it badly.
    PerToken {
        usd_per_million: f64,
        #[serde(default = "default_fps")]
        fps: u32,
    },

    /// Flat per image.
    PerImage {
        usd: f64,
        /// Some editors charge for references after the first.
        #[serde(default)]
        usd_per_extra_input: f64,
    },

    /// Per megapixel, with the first megapixel sometimes priced differently.
    PerMegapixel {
        usd: f64,
        #[serde(default)]
        first_usd: Option<f64>,
    },

    /// Per output second, at a rate that steps with resolution.
    ///
    /// Wan's video-to-video line is the shape this exists for: fal publishes
    /// $0.08/s at 720p, $0.06/s at 580p and $0.04/s at 480p for the *same*
    /// model. Flattening that to one rate is wrong in both directions — quoting
    /// the 720p rate over-charges a 480p job on the button, and quoting the
    /// 480p rate under-charges a 720p one, which is the worse half.
    ///
    /// Tiers are `(max_height, usd_per_second)`, smallest first. A height above
    /// every tier uses the last; an **unknown** height yields no estimate at
    /// all rather than a guess.
    PerSecondTiered { tiers: Vec<(u32, f64)> },

    /// Flat per generation regardless of settings.
    Flat { usd: f64 },

    /// The provider will not tell us until we ask. The UI must render this as
    /// an explicit unknown, never as free.
    Unknown,
}

/// Format a float without trailing zeros: 8.0 -> "8", 1.5 -> "1.5".
/// Prices and durations read badly as "8.0s x $0.0840/s".
fn n(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        let s = format!("{v}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn one() -> f64 {
    1.0
}
fn default_fps() -> u32 {
    24
}

/// A priced estimate, with the reasoning kept alongside the number so the UI
/// can explain itself instead of asserting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub usd: f64,
    /// Human-readable derivation, e.g. "8s x $0.084/s, audio x1.5".
    pub basis: String,
    /// Set when a provider bills a floor we would otherwise under-report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum_applied: Option<String>,
}

impl CostModel {
    /// Estimate in USD. Returns `None` only for [`CostModel::Unknown`], so the
    /// caller is forced to handle the unknown case rather than seeing a zero.
    pub fn estimate(&self, b: &Billable) -> Option<Estimate> {
        let batch = b.batch_factor();
        match self {
            CostModel::Unknown => None,

            CostModel::Flat { usd } => Some(Estimate {
                usd: usd * batch,
                basis: format!("${usd:.4} per generation"),
                minimum_applied: None,
            }),

            CostModel::PerSecondTiered { tiers } => {
                // Both facts are required. Without a height we cannot know
                // which tier applies, and guessing the cheapest would
                // under-quote a 720p job by 2x on the Generate button.
                let (Some(seconds), Some(height)) = (b.seconds, b.height) else {
                    return None;
                };
                let rate = tiers
                    .iter()
                    .find(|(max_h, _)| height <= *max_h)
                    .or_else(|| tiers.last())
                    .map(|(_, usd)| *usd)?;
                Some(Estimate {
                    usd: rate * seconds * batch,
                    basis: format!("{}s x ${rate:.4}/s at {height}p", n(seconds)),
                    minimum_applied: None,
                })
            }

            CostModel::PerSecond {
                usd,
                audio_multiplier,
            } => {
                // Providers bill wall-clock *output* seconds: a clip you dislike
                // still costs its full length. Nothing to do but be honest.
                let secs = b.seconds.unwrap_or(0.0);
                let mult = if b.audio { *audio_multiplier } else { 1.0 };
                let total = secs * usd * mult * batch;
                let mut basis = format!("{}s x ${usd:.4}/s", n(secs));
                if b.audio && (mult - 1.0).abs() > f64::EPSILON {
                    basis.push_str(&format!(", audio x{}", n(mult)));
                }
                if batch > 1.0 {
                    basis.push_str(&format!(", batch x{}", n(batch)));
                }
                Some(Estimate {
                    usd: total,
                    basis,
                    minimum_applied: None,
                })
            }

            CostModel::PerToken {
                usd_per_million,
                fps,
            } => {
                let secs = b.seconds.unwrap_or(0.0);
                let w = b.width.unwrap_or(0) as f64;
                let h = b.height.unwrap_or(0) as f64;
                let f = b.fps.unwrap_or(*fps) as f64;
                let tokens = (h * w * f * secs) / 1024.0;
                let total = (tokens / 1_000_000.0) * usd_per_million * batch;
                Some(Estimate {
                    usd: total,
                    basis: format!(
                        "{}x{} @{}fps for {}s = {:.0} tokens x ${usd_per_million}/M",
                        n(w),
                        n(h),
                        n(f),
                        n(secs),
                        tokens
                    ),
                    minimum_applied: None,
                })
            }

            CostModel::PerImage {
                usd,
                usd_per_extra_input,
            } => {
                let count = b.images.unwrap_or(1).max(1) as f64;
                let extras = b.extra_inputs as f64 * usd_per_extra_input;
                let total = (count * usd + extras) * batch;
                let mut basis = format!("{} x ${usd:.4}", n(count));
                if extras > 0.0 {
                    basis.push_str(&format!(" + {} extra inputs", b.extra_inputs));
                }
                Some(Estimate {
                    usd: total,
                    basis,
                    minimum_applied: None,
                })
            }

            CostModel::PerMegapixel { usd, first_usd } => {
                let mp = b.megapixels.or_else(|| match (b.width, b.height) {
                    (Some(w), Some(h)) => Some((w as f64 * h as f64) / 1_000_000.0),
                    _ => None,
                })?;
                let total = match first_usd {
                    Some(first) if mp > 1.0 => first + (mp - 1.0) * usd,
                    Some(first) => *first,
                    None => mp * usd,
                };
                Some(Estimate {
                    usd: total * batch,
                    basis: format!("{mp:.2} MP x ${usd:.4}/MP"),
                    minimum_applied: None,
                })
            }
        }
    }

    /// Apply a provider's billing floor. fal charges a 15-second minimum on
    /// Hummingbird regardless of clip length, which silently triples the cost
    /// of a 5-second test.
    pub fn with_minimum_seconds(
        est: Estimate,
        b: &Billable,
        min_secs: f64,
        usd_per_sec: f64,
    ) -> Estimate {
        let secs = b.seconds.unwrap_or(0.0);
        if secs >= min_secs {
            return est;
        }
        let floored = min_secs * usd_per_sec * b.batch_factor();
        Estimate {
            usd: floored,
            basis: est.basis,
            minimum_applied: Some(format!(
                "billed at the {}s provider minimum, not {}s",
                n(min_secs),
                n(secs)
            )),
        }
    }
}

/// Sum a set of estimates, propagating "unknown" rather than dropping it — a
/// pipeline with one unpriced step has an unknown total, not a smaller one.
pub fn total(parts: &[Option<Estimate>]) -> Option<f64> {
    parts
        .iter()
        .try_fold(0.0, |acc, p| Some(acc + p.as_ref()?.usd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_second_is_linear_in_duration() {
        let m = CostModel::PerSecond {
            usd: 0.084,
            audio_multiplier: 1.5,
        };
        let e = m.estimate(&Billable::video(8.0, 1280, 720)).unwrap();
        assert!((e.usd - 0.672).abs() < 1e-9, "{}", e.usd);
        assert!(e.basis.contains("8s"));
    }

    #[test]
    fn audio_multiplier_is_applied_and_explained() {
        let m = CostModel::PerSecond {
            usd: 0.084,
            audio_multiplier: 1.5,
        };
        let mut b = Billable::video(8.0, 1280, 720);
        b.audio = true;
        let e = m.estimate(&b).unwrap();
        assert!((e.usd - 1.008).abs() < 1e-9, "{}", e.usd);
        // Kling hides this multiplier; ours has to say it out loud.
        assert!(e.basis.contains("audio x1.5"), "basis was {}", e.basis);
    }

    #[test]
    fn token_pricing_scales_with_resolution_not_just_duration() {
        // The trap: same duration, higher resolution, materially higher cost.
        let m = CostModel::PerToken {
            usd_per_million: 7.0,
            fps: 24,
        };
        let sd = m.estimate(&Billable::video(8.0, 1280, 720)).unwrap();
        let hd = m.estimate(&Billable::video(8.0, 1920, 1080)).unwrap();
        let ratio = hd.usd / sd.usd;
        assert!(
            (ratio - 2.25).abs() < 0.01,
            "1080p should be ~2.25x 720p, got {ratio}"
        );
    }

    #[test]
    fn unknown_price_is_none_never_zero() {
        // A zero here would tell the user a paid generation is free.
        assert!(CostModel::Unknown.estimate(&Billable::image(1)).is_none());
    }

    #[test]
    fn a_pipeline_with_one_unknown_step_has_an_unknown_total() {
        let known = CostModel::Flat { usd: 0.10 }.estimate(&Billable::image(1));
        let unknown = CostModel::Unknown.estimate(&Billable::image(1));
        assert_eq!(total(&[known.clone(), known.clone()]), Some(0.20));
        assert_eq!(total(&[known, unknown]), None);
    }

    #[test]
    fn provider_minimum_overrides_a_short_clip() {
        // fal bills 15s minimum on Hummingbird; a 5s test is not 1/3 the price.
        let m = CostModel::PerSecond {
            usd: 0.035,
            audio_multiplier: 1.0,
        };
        let b = Billable::video(5.0, 1280, 720);
        let raw = m.estimate(&b).unwrap();
        assert!((raw.usd - 0.175).abs() < 1e-9);

        let floored = CostModel::with_minimum_seconds(raw, &b, 15.0, 0.035);
        assert!((floored.usd - 0.525).abs() < 1e-9, "{}", floored.usd);
        assert!(floored.minimum_applied.is_some());
    }

    #[test]
    fn minimum_does_not_penalise_a_long_enough_clip() {
        let m = CostModel::PerSecond {
            usd: 0.035,
            audio_multiplier: 1.0,
        };
        let b = Billable::video(20.0, 1280, 720);
        let raw = m.estimate(&b).unwrap();
        let same = CostModel::with_minimum_seconds(raw.clone(), &b, 15.0, 0.035);
        assert_eq!(same.usd, raw.usd);
        assert!(same.minimum_applied.is_none());
    }

    #[test]
    fn extra_reference_images_are_charged() {
        // Seedream edit: first input free, +$0.0045 each after.
        let m = CostModel::PerImage {
            usd: 0.035,
            usd_per_extra_input: 0.0045,
        };
        let mut b = Billable::image(1);
        b.extra_inputs = 3;
        let e = m.estimate(&b).unwrap();
        assert!((e.usd - 0.0485).abs() < 1e-9, "{}", e.usd);
    }

    #[test]
    fn first_megapixel_can_be_priced_differently() {
        // FLUX.2 pro: $0.03 first MP, $0.015 each after.
        let m = CostModel::PerMegapixel {
            usd: 0.015,
            first_usd: Some(0.03),
        };
        let mut b = Billable::image(1);
        b.megapixels = Some(3.0);
        let e = m.estimate(&b).unwrap();
        assert!((e.usd - 0.06).abs() < 1e-9, "{}", e.usd);
    }

    #[test]
    fn batch_multiplies_everything() {
        let m = CostModel::PerSecond {
            usd: 0.10,
            audio_multiplier: 1.0,
        };
        let mut b = Billable::video(5.0, 1280, 720);
        b.batch = 4;
        let e = m.estimate(&b).unwrap();
        assert!((e.usd - 2.0).abs() < 1e-9, "{}", e.usd);
        assert!(e.basis.contains("batch x4"));
    }
}
