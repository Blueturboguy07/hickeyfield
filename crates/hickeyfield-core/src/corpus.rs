//! The filmmaking prompt corpus, compiled into the binary.
//!
//! `prompts/*.md` is the harness's knowledge — shot grammar, camera motion,
//! lens language, per-mode discipline. It is the difference between a rewriter
//! that pads a prompt with adjectives and one that reasons about a shot.
//!
//! **Embedded rather than shipped as a resource file.** A Tauri resource can be
//! missing from a bundle, and the failure is silent and late: the base file
//! alone reads as a complete instruction set, so a missing overlay produces
//! confident output with no mode discipline and nothing downstream can tell.
//! `include_str!` makes that a compile error instead.
//!
//! Versioned deliberately. A generation records which corpus rewrote it — see
//! [`crate::enhancer::recipe_pin`] — so a prompt improvement never silently
//! changes what an old recipe reproduces.

use crate::enhancer::Mode;

/// The **reference** corpus: 6,000 words explaining why each rule exists.
///
/// Not sent to a model. It is documentation for whoever edits the operative
/// prompts below, and the place the doctrine is argued rather than asserted.
pub const REFERENCE_BASE: &str = include_str!("../../../prompts/enhancer.v1.md");
const REFERENCE_VIDEO: &str = include_str!("../../../prompts/enhancer.video.v1.md");
const REFERENCE_IMAGE: &str = include_str!("../../../prompts/enhancer.image.v1.md");
const REFERENCE_EDIT: &str = include_str!("../../../prompts/enhancer.edit.v1.md");

/// What is actually sent.
///
/// **Measured 2026-08-05, same model and prompt, three system prompts:**
///
/// | system | time | result |
/// |---|---|---|
/// | 70 tokens | 4.8s | dropped the subject entirely — returned a parameter list |
/// | **~420 tokens** | **2.0s** | kept the subject, named a shot size, chose a locked-off frame |
/// | 13,050 tokens | 56s | good prose, but hallucinated *columns on a lighthouse* |
///
/// The long corpus is well written and it is the wrong artifact to send: it is
/// teaching material, and the model pays attention tax on all of it while the
/// operative rules are a few hundred tokens. 28x faster and better, on the
/// evidence — so the reference stays, and this is what crosses the wire.
pub const BASE: &str = include_str!("../../../prompts/operative/base.v1.md");
const VIDEO: &str = include_str!("../../../prompts/operative/video.v1.md");
const IMAGE: &str = include_str!("../../../prompts/operative/image.v1.md");
const EDIT: &str = include_str!("../../../prompts/operative/edit.v1.md");

/// Identifies the corpus in a recipe. Bump with the files.
pub const CORPUS_ID: &str = "enhancer.v1";

/// The reference text for one mode. Never sent; see [`REFERENCE_BASE`].
pub fn reference(mode: Mode) -> &'static str {
    match mode {
        Mode::Video => REFERENCE_VIDEO,
        Mode::Image => REFERENCE_IMAGE,
        Mode::Edit => REFERENCE_EDIT,
    }
}

/// The operative overlay for one mode.
pub fn overlay(mode: Mode) -> &'static str {
    match mode {
        Mode::Video => VIDEO,
        Mode::Image => IMAGE,
        Mode::Edit => EDIT,
    }
}

/// Base plus exactly one overlay, ready to send as a system prompt.
pub fn system_prompt_for(mode: Mode) -> Result<String, String> {
    crate::enhancer::assemble_system_prompt(BASE, overlay(mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_has_a_real_overlay() {
        // A missing overlay is the failure `assemble_system_prompt` refuses,
        // and embedding turns "missing at runtime" into "will not compile".
        for mode in [Mode::Video, Mode::Image, Mode::Edit] {
            assert!(overlay(mode).len() > 300, "{mode:?} overlay is too thin");
            assert!(system_prompt_for(mode).is_ok());
        }
    }

    #[test]
    fn the_sent_prompt_stays_within_its_token_budget() {
        // The regression this exists for: v1 sent 13,050 tokens to rewrite one
        // sentence. It took 56s, hallucinated detail, and did not fit a small
        // local model's context at all. A ~420-token prompt was measured faster
        // AND better on the same model.
        //
        // Bytes/4 is a rough token estimate; the ceiling is generous enough
        // that the approximation cannot matter.
        for mode in [Mode::Video, Mode::Image, Mode::Edit] {
            let est = system_prompt_for(mode).unwrap().len() / 4;
            assert!(
                est < 900,
                "{mode:?} system prompt is ~{est} tokens — the long-form corpus \
                 belongs in reference(), not on the wire"
            );
        }
    }

    #[test]
    fn the_reference_is_kept_and_is_not_what_gets_sent() {
        // Both halves matter. The reference is where the doctrine is argued,
        // and losing it would make the operative rules unmaintainable folklore.
        for mode in [Mode::Video, Mode::Image, Mode::Edit] {
            assert!(reference(mode).len() > 8_000, "the reference was gutted");
            assert!(!system_prompt_for(mode).unwrap().contains(reference(mode)));
        }
    }

    #[test]
    fn the_three_overlays_are_actually_different() {
        // Guards a copy-paste that would give an image edit video guidance.
        let v = overlay(Mode::Video);
        let i = overlay(Mode::Image);
        let e = overlay(Mode::Edit);
        assert_ne!(v, i);
        assert_ne!(i, e);
        assert_ne!(v, e);
    }

    #[test]
    fn the_operative_prompt_keeps_the_rules_that_cost_money_when_broken() {
        // Distilling must not quietly drop an invariant. These four are the
        // ones with a failure attached: a swapped subject is not the user's
        // generation, invented detail is the "columns on a lighthouse" bug,
        // two actions average into mush, and re-describing an attached frame
        // is what makes an edit change someone's face.
        let all = format!(
            "{}{}{}{}",
            BASE,
            overlay(Mode::Video),
            overlay(Mode::Image),
            overlay(Mode::Edit)
        )
        .to_lowercase();
        for rule in ["invent", "one subject action", "preserve", "re-describe"] {
            assert!(all.contains(rule), "the operative prompts lost: {rule}");
        }
    }

    #[test]
    fn the_base_carries_the_doctrine_the_harness_depends_on() {
        // enhance.rs forces enhancement on when a preset is selected precisely
        // because the preset's aesthetic is delivered by the rewrite. If the
        // corpus stops teaching shot discipline, that rule starts doing harm.
        let low = format!("{BASE}{}", overlay(Mode::Video)).to_lowercase();
        for topic in ["camera", "shot", "lens", "light"] {
            assert!(
                low.contains(topic),
                "the base corpus never mentions {topic}"
            );
        }
    }

    #[test]
    fn an_assembled_prompt_contains_both_halves() {
        let p = system_prompt_for(Mode::Video).unwrap();
        assert!(p.contains(BASE.trim_end()));
        assert!(p.contains(VIDEO.trim_start()));
    }
}
