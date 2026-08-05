//! What the prompt leaves for the model to guess.
//!
//! A generation model never says "you didn't tell me". It decides, silently and
//! plausibly, and bills you for the decision. Asked for "a red door" on a 7B
//! rewriter, it produced *"a cozy living space bathed in golden light"* — a
//! whole room nobody mentioned. The prompt was underspecified and the gap was
//! filled by whatever the prior liked.
//!
//! So: find the gaps *before* spending, and ask.
//!
//! **Deterministic, not LLM-driven.** Detection has to be instant and the same
//! every time — a question that appears on one run and not the next is worse
//! than none, and a model deciding what to ask is another thing that can
//! hallucinate. The axes here are the ones the corpus says carry a shot, and
//! each one is looked for by vocabulary the user would plausibly have used.
//!
//! **Every question is skippable and there are never more than three.** The
//! point is to catch the one detail that would otherwise be invented, not to
//! interrogate someone who knows what they want.

use crate::media::MediaRole;
use crate::use_case::UseCase;

/// One thing the prompt does not say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// Stable id, so an answer can be matched back and a skip remembered.
    pub id: &'static str,
    /// Asked in the second person, short enough to read at a glance.
    pub question: String,
    /// What the model will otherwise decide on its own. Shown small, because a
    /// question the user cannot see the cost of is just friction.
    pub consequence: &'static str,
    /// Concrete answers. Picking beats typing, and a blank field invites the
    /// same vagueness the gap came from.
    pub options: Vec<&'static str>,
    /// Ranked: the most consequential gap first, so a cap of three keeps the
    /// three that matter.
    pub weight: u8,
}

/// Words that mean the axis is already specified.
///
/// Deliberately generous. A false negative costs one unnecessary question; a
/// false positive means the gap goes unasked and gets invented, which is the
/// thing this module exists to prevent.
const LIGHT: [&str; 24] = [
    "light",
    "lit",
    "sun",
    "sunlight",
    "sunset",
    "sunrise",
    "dawn",
    "dusk",
    "night",
    "noon",
    "golden hour",
    "overcast",
    "shadow",
    "backlit",
    "neon",
    "candle",
    "lamp",
    "moonlit",
    "bright",
    "dark",
    "silhouette",
    "glow",
    "morning",
    "evening",
];

const CAMERA: [&str; 22] = [
    "camera", "shot", "close-up", "closeup", "wide", "medium", "aerial", "overhead", "pan", "tilt",
    "dolly", "zoom", "track", "orbit", "handheld", "static", "locked", "push", "pull", "crane",
    "pov", "angle",
];

const MOTION: [&str; 20] = [
    "walk", "run", "turn", "fall", "rise", "move", "spin", "drift", "flow", "jump", "dance",
    "reach", "open", "close", "drive", "fly", "swim", "burst", "collapse", "melt",
];

const SETTING: [&str; 18] = [
    " in ",
    " on ",
    " at ",
    "room",
    "street",
    "forest",
    "beach",
    "city",
    "field",
    "kitchen",
    "office",
    "desert",
    "mountain",
    "studio",
    "indoor",
    "outdoor",
    "background",
    "behind",
];

fn mentions(prompt: &str, vocab: &[&str]) -> bool {
    let low = format!(" {} ", prompt.to_lowercase());
    vocab.iter().any(|w| low.contains(w))
}

/// How many words of actual description there are.
///
/// A three-word prompt is not a style choice, it is a prompt that has not been
/// written yet, and asking about lens on it would be absurd.
fn is_sparse(prompt: &str) -> bool {
    prompt.split_whitespace().count() < 8
}

/// The most questions worth asking. Beyond this it stops being help.
pub const MAX_GAPS: usize = 3;

/// Find what the prompt leaves open, for this job and these attachments.
pub fn detect(prompt: &str, use_case: UseCase, media_roles: &[MediaRole]) -> Vec<Gap> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let editing = matches!(use_case, UseCase::EditVideo | UseCase::EditImage);
    let video = matches!(
        use_case,
        UseCase::TextToVideo | UseCase::ImageToVideo | UseCase::EditVideo
    );
    // A start frame already fixes the light, the setting and the framing. Asking
    // about them would be asking the user to describe their own picture.
    let has_source_image = media_roles
        .iter()
        .any(|r| matches!(r, MediaRole::Start | MediaRole::Video));

    let mut gaps: Vec<Gap> = Vec::new();

    // ── Editing: what must NOT change is the question that matters ─────────
    if editing {
        gaps.push(Gap {
            id: "preserve",
            question: "What should stay exactly as it is?".into(),
            consequence: "Unstated, the model may redraw faces, lighting or background \
                          while making your change.",
            options: vec![
                "The subject's face",
                "The lighting",
                "The background",
                "Everything except what I described",
            ],
            weight: 100,
        });

        if !mentions(trimmed, &MOTION) && video {
            gaps.push(Gap {
                id: "edit_extent",
                question: "Should the change happen gradually or be there from the start?".into(),
                consequence: "Otherwise the model picks a timing, and it is usually abrupt.",
                options: vec!["Gradually across the clip", "Immediately", "Near the end"],
                weight: 60,
            });
        }
    }

    // ── Generating: the axes that get invented ─────────────────────────────
    if !editing {
        if !has_source_image && !mentions(trimmed, &LIGHT) {
            gaps.push(Gap {
                id: "light",
                question: "What is the light like?".into(),
                consequence: "The model will pick a time of day and a light source for you.",
                options: vec![
                    "Daylight, soft and overcast",
                    "Hard sun, strong shadows",
                    "Golden hour",
                    "Night, artificial light",
                    "Interior, window light",
                ],
                weight: 90,
            });
        }

        if !has_source_image && !mentions(trimmed, &SETTING) && !is_sparse(trimmed) {
            gaps.push(Gap {
                id: "setting",
                question: "Where is this?".into(),
                consequence: "Without a setting the model invents a background, and it is \
                              often the wrong one.",
                options: vec![
                    "Indoors",
                    "Outdoors, urban",
                    "Outdoors, nature",
                    "Plain backdrop",
                ],
                weight: 85,
            });
        }

        if !mentions(trimmed, &CAMERA) {
            gaps.push(Gap {
                id: "framing",
                question: if video {
                    "How should the camera behave?".into()
                } else {
                    "How close should the framing be?".into()
                },
                consequence: if video {
                    "Otherwise the model chooses a move, and an arbitrary drift is the \
                     most common result."
                } else {
                    "Otherwise the model chooses a framing, usually a mid shot."
                },
                options: if video {
                    vec![
                        "Hold still",
                        "Slow push in",
                        "Slow pull out",
                        "Follow the subject",
                        "Wide establishing",
                    ]
                } else {
                    vec!["Close up", "Mid shot", "Wide", "Overhead"]
                },
                weight: 70,
            });
        }

        if video && !mentions(trimmed, &MOTION) {
            gaps.push(Gap {
                id: "action",
                question: "What actually moves?".into(),
                consequence: "A video prompt with no action tends to come back nearly still.",
                options: vec![
                    "The subject moves",
                    "Only the camera moves",
                    "Ambient motion only",
                    "Almost nothing — a held moment",
                ],
                weight: 95,
            });
        }
    }

    gaps.sort_by_key(|g| std::cmp::Reverse(g.weight));
    gaps.truncate(MAX_GAPS);
    gaps
}

/// Fold the answers into the prompt.
///
/// Appended as a plain clause rather than merged into the user's sentence: the
/// rewriter is better at integrating prose than we are at splicing it, and the
/// user's own words stay recognisable if they read it back.
pub fn apply(prompt: &str, answers: &[(String, String)]) -> String {
    let extra: Vec<&str> = answers
        .iter()
        .map(|(_, a)| a.trim())
        .filter(|a| !a.is_empty())
        .collect();
    if extra.is_empty() {
        return prompt.to_string();
    }
    let base = prompt.trim().trim_end_matches('.');
    format!("{base}. {}.", extra.join(". "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_prompt_is_asked_about_the_things_that_get_invented() {
        // "a red door" produced "a cozy living space bathed in golden light" —
        // a whole room the user never mentioned.
        let g = detect("a red door", UseCase::TextToImage, &[]);
        let ids: Vec<_> = g.iter().map(|x| x.id).collect();
        assert!(ids.contains(&"light"), "got {ids:?}");
        assert!(ids.contains(&"framing"), "got {ids:?}");
    }

    #[test]
    fn a_prompt_that_already_says_it_is_not_asked_again() {
        // Nothing is more irritating than being asked something you just said.
        let g = detect(
            "a red door at golden hour, slow push in, on a quiet street",
            UseCase::TextToVideo,
            &[],
        );
        let ids: Vec<_> = g.iter().map(|x| x.id).collect();
        assert!(!ids.contains(&"light"), "asked about light anyway: {ids:?}");
        assert!(
            !ids.contains(&"framing"),
            "asked about camera anyway: {ids:?}"
        );
        assert!(
            !ids.contains(&"setting"),
            "asked about setting anyway: {ids:?}"
        );
    }

    #[test]
    fn an_attached_still_stops_us_asking_the_user_to_describe_their_own_picture() {
        let g = detect("make it snow", UseCase::ImageToVideo, &[MediaRole::Start]);
        let ids: Vec<_> = g.iter().map(|x| x.id).collect();
        assert!(!ids.contains(&"light"));
        assert!(!ids.contains(&"setting"));
    }

    #[test]
    fn editing_asks_what_must_not_change_first() {
        // The highest-value question in an edit, and the one whose absence
        // produces "it changed her face".
        let g = detect(
            "remove the person walking",
            UseCase::EditVideo,
            &[MediaRole::Video],
        );
        assert_eq!(g[0].id, "preserve", "got {:?}", g[0].id);
    }

    #[test]
    fn editing_never_asks_about_light_or_setting() {
        // Both are already in the attached media; asking is nonsense.
        let g = detect("make it winter", UseCase::EditImage, &[MediaRole::Start]);
        let ids: Vec<_> = g.iter().map(|x| x.id).collect();
        assert!(!ids.contains(&"light"));
        assert!(!ids.contains(&"setting"));
    }

    #[test]
    fn a_video_with_no_verb_is_asked_what_moves() {
        // The most common cause of "why is my video a still image".
        let g = detect("a harbour at dusk, wide", UseCase::TextToVideo, &[]);
        assert!(g.iter().any(|x| x.id == "action"), "{g:?}");
    }

    #[test]
    fn a_still_image_is_never_asked_what_moves() {
        let g = detect("a harbour", UseCase::TextToImage, &[]);
        assert!(!g.iter().any(|x| x.id == "action"));
    }

    #[test]
    fn there_are_never_more_than_three_questions() {
        // Past three it stops being help. The cap keeps the heaviest ones.
        for uc in UseCase::ALL {
            let g = detect("a thing", uc, &[]);
            assert!(
                g.len() <= MAX_GAPS,
                "{} asked {} questions",
                uc.slug(),
                g.len()
            );
        }
    }

    #[test]
    fn they_come_back_in_order_of_what_they_cost() {
        let g = detect("a harbour", UseCase::TextToVideo, &[]);
        for pair in g.windows(2) {
            assert!(pair[0].weight >= pair[1].weight, "not ranked: {g:?}");
        }
    }

    #[test]
    fn an_empty_prompt_is_not_interrogated() {
        // There is nothing to fill a gap in yet, and the Generate button is
        // already disabled.
        assert!(detect("   ", UseCase::TextToVideo, &[]).is_empty());
    }

    #[test]
    fn every_question_offers_real_choices_and_says_what_it_costs() {
        for uc in UseCase::ALL {
            for g in detect("a thing", uc, &[]) {
                assert!(g.options.len() >= 3, "{} has too few options", g.id);
                assert!(!g.consequence.is_empty(), "{} does not say why", g.id);
                assert!(g.question.ends_with('?'), "{} is not a question", g.id);
            }
        }
    }

    #[test]
    fn answers_extend_the_prompt_without_mangling_it() {
        let out = apply(
            "a red door.",
            &[
                ("light".into(), "Golden hour".into()),
                ("framing".into(), "Hold still".into()),
            ],
        );
        assert!(out.starts_with("a red door."), "{out}");
        assert!(
            out.contains("Golden hour") && out.contains("Hold still"),
            "{out}"
        );
        assert!(!out.contains(".."), "double punctuation: {out}");
    }

    #[test]
    fn skipping_everything_leaves_the_prompt_exactly_as_typed() {
        // Skip has to be free. A prompt that changes when you decline to answer
        // would make the feature something to avoid.
        let original = "a red door";
        assert_eq!(apply(original, &[]), original);
        assert_eq!(apply(original, &[("light".into(), "  ".into())]), original);
    }
}
