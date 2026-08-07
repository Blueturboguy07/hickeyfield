//! Who decides the shape of the output.
//!
//! The aspect ratio was being decided in three places that could disagree: the
//! chip row showed one, the request body sometimes carried another, and the
//! provider's own default silently supplied a third whenever neither of the
//! first two applied. The result is the failure a user actually sees — a
//! generation that comes back in a shape they did not pick, with nothing in the
//! interface having warned them, and no way to tell whether they mis-clicked.
//!
//! So the decision is made **once**, here, before submit, and everything else
//! reports what this returned. There are exactly three ways a generation's
//! shape can be determined, and the important property of this enum is that it
//! makes the two we do not control impossible to render as if we did.

use serde::{Deserialize, Serialize};

/// What will determine the output's aspect ratio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum AspectPlan {
    /// We put this value on the wire, so the output will have it. The only
    /// outcome under our control, and therefore the only one the UI may state
    /// as a fact.
    Sent(String),
    /// The endpoint publishes no aspect field, but the user attached media —
    /// so the output takes the shape of that input. Not a failure: for
    /// image-to-video this is usually the desired behaviour, and it is the
    /// reason animating a portrait photo should not produce a landscape clip.
    FollowsInput,
    /// No aspect control and no input to inherit from. The provider's own
    /// default decides and we genuinely do not know what it is.
    ProviderDefault,
}

impl AspectPlan {
    /// The single decision.
    ///
    /// `requested` is what the user picked, `fallback` the endpoint's own
    /// default. When the endpoint has the control we always send something
    /// explicit — never omitting the field and hoping — because an omitted
    /// field is how the provider's default became a third opinion in the first
    /// place.
    pub fn decide(
        endpoint_takes_aspect: bool,
        requested: Option<&str>,
        fallback: Option<&str>,
        has_input_media: bool,
    ) -> AspectPlan {
        if endpoint_takes_aspect {
            // Each candidate is filtered *before* the fallback is considered.
            // Filtering after the `or` lets a blank request swallow a perfectly
            // good default and drop through to "we don't control this", which
            // is the opposite of what a blank value should mean.
            fn usable(v: Option<&str>) -> Option<&str> {
                v.map(str::trim).filter(|v| !v.is_empty())
            }
            if let Some(v) = usable(requested).or_else(|| usable(fallback)) {
                return AspectPlan::Sent(v.to_string());
            }
            // The control exists but nobody has a value for it. Inheriting the
            // input beats inventing a number.
            return if has_input_media {
                AspectPlan::FollowsInput
            } else {
                AspectPlan::ProviderDefault
            };
        }
        if has_input_media {
            AspectPlan::FollowsInput
        } else {
            AspectPlan::ProviderDefault
        }
    }

    /// True only when the output's shape is a fact we control.
    pub fn is_locked(&self) -> bool {
        matches!(self, AspectPlan::Sent(_))
    }

    /// The value going on the wire, if any.
    pub fn wire_value(&self) -> Option<&str> {
        match self {
            AspectPlan::Sent(v) => Some(v),
            _ => None,
        }
    }

    /// What to tell the user when their choice is not what decides the shape.
    ///
    /// `None` when we control it, because a note in that case is noise. The
    /// two remaining cases get *different* sentences on purpose: one is benign
    /// and expected, the other is a genuine unknown, and collapsing them into
    /// "aspect ratio ignored" taught the user to ignore the notice.
    pub fn note(&self, model: &str, requested: Option<&str>) -> Option<String> {
        match self {
            AspectPlan::Sent(_) => None,
            AspectPlan::FollowsInput => Some(match requested {
                Some(r) => format!(
                    "{model} has no aspect control on this endpoint — the result keeps the shape \
                     of what you attached rather than {r}"
                ),
                None => format!("{model} keeps the shape of what you attached"),
            }),
            AspectPlan::ProviderDefault => Some(format!(
                "{model} has no aspect control on this endpoint — the provider's own default \
                 decides the shape"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_supported_choice_goes_on_the_wire() {
        let p = AspectPlan::decide(true, Some("9:16"), Some("16:9"), false);
        assert_eq!(p, AspectPlan::Sent("9:16".into()));
        assert!(p.is_locked());
        assert_eq!(p.note("Kling", Some("9:16")), None);
    }

    #[test]
    fn an_untouched_chip_still_sends_something_explicit() {
        // The bug this closes: with no value the field was omitted, the
        // provider default filled it in, and two runs of the "same" settings
        // could come back in different shapes.
        assert_eq!(
            AspectPlan::decide(true, None, Some("16:9"), false),
            AspectPlan::Sent("16:9".into())
        );
    }

    #[test]
    fn an_empty_string_is_not_a_choice() {
        assert_eq!(
            AspectPlan::decide(true, Some("  "), Some("1:1"), false),
            AspectPlan::Sent("1:1".into())
        );
    }

    #[test]
    fn with_no_control_and_an_attachment_the_input_decides() {
        let p = AspectPlan::decide(false, Some("16:9"), None, true);
        assert_eq!(p, AspectPlan::FollowsInput);
        assert!(!p.is_locked());
        // The note must say what will happen, not merely that something was
        // ignored: animating a portrait photo giving a portrait clip is the
        // right outcome, and a user told only "ignored" would assume it broke.
        let note = p.note("Grok Imagine Edit", Some("16:9")).unwrap();
        assert!(note.contains("keeps the shape"), "{note}");
        assert!(note.contains("16:9"), "{note}");
    }

    #[test]
    fn with_no_control_and_no_input_we_admit_we_do_not_know() {
        let p = AspectPlan::decide(false, Some("16:9"), None, false);
        assert_eq!(p, AspectPlan::ProviderDefault);
        let note = p.note("Some Model", Some("16:9")).unwrap();
        assert!(note.contains("provider"), "{note}");
    }

    #[test]
    fn the_two_uncontrolled_cases_read_differently() {
        // If these ever collapse into one sentence, the benign case starts
        // training the user to dismiss the real one.
        let follows = AspectPlan::FollowsInput.note("M", Some("1:1")).unwrap();
        let default = AspectPlan::ProviderDefault.note("M", Some("1:1")).unwrap();
        assert_ne!(follows, default);
    }

    #[test]
    fn only_a_sent_plan_yields_a_wire_value() {
        assert_eq!(AspectPlan::Sent("21:9".into()).wire_value(), Some("21:9"));
        assert_eq!(AspectPlan::FollowsInput.wire_value(), None);
        assert_eq!(AspectPlan::ProviderDefault.wire_value(), None);
    }
}
