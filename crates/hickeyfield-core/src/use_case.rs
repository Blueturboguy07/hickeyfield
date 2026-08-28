//! What the user is trying to do, and which models can actually do it.
//!
//! The workspace tabs used to be *Create Video · Edit Video · Motion Control* —
//! Higgsfield's names, describing surfaces rather than jobs — and they filtered
//! nothing. Every model was offered under every tab, so you could sit in
//! "Edit Video", attach a clip, pick Gemini Omni, and be told after submitting
//! that it takes a prompt only.
//!
//! **The refusal was right and its timing was wrong.** A control that lets you
//! choose something impossible and explains afterwards has already wasted the
//! decision. The use case is the thing the user knows before anything else, so
//! it should be what narrows the roster.
//!
//! Filtering is deliberately *local* — no network. Drawing a picker must not
//! cost 68 HTTP requests, so this uses the measured mode table in
//! [`crate::media`] plus the no-media list below, and
//! [`crate::capability::for_route`] refines the single selected model
//! afterwards.

use crate::media::{self, InputMode, MediaRole};
use crate::route::Route;
use crate::{catalog::Modality, registry, Model, ProviderId};

/// fal endpoints that accept **no media at all**, measured 2026-08-05 by
/// diffing every fal route against its published schema.
///
/// These are the models the catalogue claims take image or video references
/// while fal's endpoint has no such field. Gemini Omni is the one that reached
/// a user: they attached a clip, asked for an edit, and got an unrelated
/// generation billed in full, because fal silently ignores a key it does not
/// recognise.
///
/// Kept as model ids rather than slugs because that is what a picker filters.
/// Provisional in the same way the mode table is — `capability::for_route`
/// reads fal's schema directly and is authoritative once a model is selected.
const NO_MEDIA_MODELS: [&str; 10] = [
    "flux_2",
    "gemini_omni",
    "gpt_image_2",
    "nano_banana_2",
    "nano_banana_2_lite",
    "nano_banana_flash",
    "seedream_v5_pro",
    "veo3_1",
    "veo3_1_fast",
    "veo3_1_lite",
];

/// A job someone actually wants done.
///
/// Named for the work, not for a product surface: a person arrives wanting to
/// *edit their clip*, not wanting "Motion Control".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UseCase {
    TextToVideo,
    ImageToVideo,
    EditVideo,
    TextToImage,
    EditImage,
}

impl UseCase {
    pub const ALL: [UseCase; 5] = [
        UseCase::TextToVideo,
        UseCase::ImageToVideo,
        UseCase::EditVideo,
        UseCase::TextToImage,
        UseCase::EditImage,
    ];

    /// Stable id for the bridge and for stored preferences.
    pub fn slug(self) -> &'static str {
        match self {
            UseCase::TextToVideo => "text-to-video",
            UseCase::ImageToVideo => "image-to-video",
            UseCase::EditVideo => "edit-video",
            UseCase::TextToImage => "text-to-image",
            UseCase::EditImage => "edit-image",
        }
    }

    pub fn from_slug(s: &str) -> Option<Self> {
        UseCase::ALL.into_iter().find(|u| u.slug() == s)
    }

    /// The tab label. Short, because it sits in a segmented control.
    pub fn label(self) -> &'static str {
        match self {
            UseCase::TextToVideo => "New Video",
            UseCase::ImageToVideo => "Animate Image",
            UseCase::EditVideo => "Edit Video",
            UseCase::TextToImage => "New Image",
            UseCase::EditImage => "Edit Image",
        }
    }

    /// One line under the tab, so the choice does not rely on the label alone.
    pub fn blurb(self) -> &'static str {
        match self {
            UseCase::TextToVideo => "Describe a shot and generate it.",
            UseCase::ImageToVideo => "Bring a still to life.",
            UseCase::EditVideo => "Change a clip you already have.",
            UseCase::TextToImage => "Describe an image and generate it.",
            UseCase::EditImage => "Change an image you already have.",
        }
    }

    pub fn output(self) -> Modality {
        match self {
            UseCase::TextToVideo | UseCase::ImageToVideo | UseCase::EditVideo => Modality::Video,
            UseCase::TextToImage | UseCase::EditImage => Modality::Image,
        }
    }

    /// The input mode the endpoint must serve.
    pub fn input_mode(self) -> InputMode {
        match self {
            UseCase::TextToVideo | UseCase::TextToImage => InputMode::Text,
            UseCase::ImageToVideo | UseCase::EditImage => InputMode::Image,
            UseCase::EditVideo => InputMode::Video,
        }
    }

    /// The media slots this use case shows, and whether one is required.
    ///
    /// `EditVideo` requires a clip: without one there is nothing to edit and
    /// the job silently becomes text-to-video, which is the failure that
    /// started all of this.
    pub fn slots(self) -> &'static [(MediaRole, bool)] {
        match self {
            UseCase::TextToVideo => &[],
            UseCase::ImageToVideo => &[
                (MediaRole::Start, true),
                (MediaRole::End, false),
                (MediaRole::Reference, false),
            ],
            UseCase::EditVideo => &[(MediaRole::Video, true), (MediaRole::Reference, false)],
            UseCase::TextToImage => &[(MediaRole::Reference, false)],
            UseCase::EditImage => &[(MediaRole::Start, true), (MediaRole::Reference, false)],
        }
    }

    /// Whether this use case cannot proceed without an attachment.
    pub fn requires_media(self) -> bool {
        self.slots().iter().any(|(_, required)| *required)
    }
}

/// Can this model do this job?
///
/// Four conditions, and every one has produced a real failure:
///
/// 1. **Output modality.** An image model cannot make a video.
/// 2. **A route we can execute** — see [`ProviderId::has_adapter`]. A key
///    without a client sent users to Settings to add a key they already had.
/// 3. **The endpoint serves this input mode.** Seedance has no
///    `/video-to-video`; Gemini Omni takes no media at all.
/// 4. **The model can bind the roles this job attaches.** Added after (1)–(3)
///    passed for models that then refused the file: `grok_edit_video` serves
///    the video input mode and takes *only* a clip, so offering it under
///    "Animate Image" produced "does not accept a start frame" at submit.
///
/// Condition 4 exists because `supports` and [`crate::media::bind`] are two
/// answers to "can this model take my file", and a test asserting they agree is
/// weaker than asking `bind` itself.
pub fn supports(model: &Model, use_case: UseCase) -> bool {
    if model.modality != use_case.output() {
        return false;
    }
    // `NO_MEDIA_MODELS` was read off fal's schemas, so it is a route-level fact
    // wearing a model-level shape. It still holds wherever fal is the only
    // judge — lifted only by a Higgsfield route their own spec is measured to
    // serve in this mode, which is a different document saying a different
    // thing about a different endpoint.
    let rescued_by_higgsfield = model.routes.iter().any(|r| {
        r.provider == ProviderId::Higgsfield
            && r.provider.has_adapter()
            && media::higgsfield_is_measured(&r.slug)
            && media::higgsfield_serves(&r.slug, use_case.input_mode())
    });
    if use_case.input_mode() != InputMode::Text
        && NO_MEDIA_MODELS.contains(&model.id.as_str())
        && !rescued_by_higgsfield
    {
        return false;
    }

    model
        .routes
        .iter()
        .any(|r| route_serves(model, r, use_case))
}

/// Can this **one route** do this one job?
///
/// [`supports`] asks whether *any* route can, which is the right question for
/// "show this model in this tab". This is the per-route form, and the picker
/// needs both: Veo 3.1 belongs under Animate Image because a Higgsfield key
/// reaches `/veo3.1/image-to-video`, while its fal route
/// (`fal-ai/veo3.1`) takes no media at all. Offering the model without marking
/// that route unusable hands the user a default route that fails at submit —
/// which is the same "refused after the decision" failure this module exists
/// to prevent, just moved one level down.
///
/// The dialect is the route's own: fal defers to the measured endpoint, and for
/// Higgsfield the catalogue *is* the spec. See [`crate::media::can_bind`].
pub fn route_serves(model: &Model, route: &Route, use_case: UseCase) -> bool {
    if !route.provider.has_adapter() {
        return false;
    }
    let (dialect, slug) = match route.provider {
        ProviderId::Fal => {
            if media::route_is_missing(&route.slug) {
                return false;
            }
            if media::resolve_endpoint(
                &route.slug,
                use_case.input_mode(),
                use_case.output() == Modality::Video,
            )
            .is_err()
            {
                return false;
            }
            (media::Dialect::Fal, route.slug.as_str())
        }
        ProviderId::Higgsfield => {
            if media::resolve_higgsfield_endpoint(
                &route.slug,
                use_case.input_mode(),
                use_case.output() == Modality::Video,
            )
            .is_err()
            {
                return false;
            }
            // Their mirror takes fal's `image_url`, so fal is the dialect and
            // their own spec — already consulted just above — is the authority
            // on the mode. Their in-house surfaces keep the catalogue, which
            // really does describe them. See `media::higgsfield_speaks_fal`.
            (
                media::dialect_for(route.provider, &route.slug),
                route.slug.as_str(),
            )
        }
        // Only fal's and Higgsfield's endpoint *shapes* are measured, so there
        // is no mode check to run here — but the catalogue still describes what
        // the model accepts, and skipping it offered `z_image` under Edit Image
        // when its spec has no start-frame flag at all.
        _ => (media::Dialect::Catalog, ""),
    };
    use_case
        .slots()
        .iter()
        .filter(|(_, required)| *required)
        .all(|(role, _)| media::can_bind(&model.spec, *role, dialect, slug))
}

/// Every model that can do this job, in the registry's own order.
pub fn models_for(use_case: UseCase) -> Vec<Model> {
    registry()
        .into_values()
        .filter(|m| supports(m, use_case))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_omni_is_not_offered_for_editing() {
        // The bug this module exists for. A user sat in "Edit Video", attached
        // a clip, picked Gemini Omni, and learned only after submitting that it
        // takes a prompt only.
        let reg = registry();
        let g = &reg["gemini_omni"];
        assert!(supports(g, UseCase::TextToVideo));
        assert!(!supports(g, UseCase::EditVideo));
        assert!(!supports(g, UseCase::ImageToVideo));
    }

    #[test]
    fn editing_a_clip_offers_only_models_that_take_one() {
        let editors = models_for(UseCase::EditVideo);
        assert!(!editors.is_empty(), "no model can edit a video");
        for m in &editors {
            assert!(
                !NO_MEDIA_MODELS.contains(&m.id.as_str()),
                "{} takes no media but is offered for editing",
                m.id
            );
            assert_eq!(m.modality, Modality::Video);
        }
    }

    #[test]
    fn wan_2_2_is_the_video_editor_we_verified_live() {
        // Proved end to end on 2026-08-05: upload, submit, poll, 674 KB out.
        let editors = models_for(UseCase::EditVideo);
        assert!(
            editors.iter().any(|m| m.id == "wan2_2_video"),
            "the one model verified to edit a clip is missing: {:?}",
            editors.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_model_the_provider_does_not_serve_is_never_offered() {
        // Wan 2.6 is not on fal under any suffix.
        for uc in UseCase::ALL {
            assert!(
                !models_for(uc).iter().any(|m| m.id == "wan2_6"),
                "wan2_6 offered under {}",
                uc.slug()
            );
        }
    }

    #[test]
    fn every_use_case_offers_something() {
        // An empty tab is a dead end the user cannot diagnose.
        for uc in UseCase::ALL {
            let n = models_for(uc).len();
            assert!(n > 0, "{} offers no models at all", uc.slug());
        }
    }

    #[test]
    fn image_use_cases_never_offer_video_models_and_vice_versa() {
        for uc in UseCase::ALL {
            for m in models_for(uc) {
                assert_eq!(
                    m.modality,
                    uc.output(),
                    "{} offered under {}",
                    m.id,
                    uc.slug()
                );
            }
        }
    }

    #[test]
    fn editing_requires_an_attachment_and_generating_does_not() {
        // Without this, "Edit Video" with no clip silently becomes
        // text-to-video — a different, unasked-for generation at the same price.
        assert!(UseCase::EditVideo.requires_media());
        assert!(UseCase::EditImage.requires_media());
        assert!(UseCase::ImageToVideo.requires_media());
        assert!(!UseCase::TextToVideo.requires_media());
        assert!(!UseCase::TextToImage.requires_media());
    }

    #[test]
    fn slugs_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for uc in UseCase::ALL {
            assert!(seen.insert(uc.slug()));
            assert_eq!(UseCase::from_slug(uc.slug()), Some(uc));
        }
        assert_eq!(UseCase::from_slug("motion-control"), None);
    }

    #[test]
    fn every_offered_model_can_actually_bind_what_the_use_case_attaches() {
        // THE test this module was missing, and its absence shipped three
        // broken models: `grok_edit_video`, `ray2_modify` and `wan_vace` were
        // all offered under Edit Video and all refused a clip with "does not
        // accept a video", because their hand-authored specs named fal's wire
        // keys (`video_url`) instead of the catalogue vocabulary (`video`)
        // that `media::bind` matches on.
        //
        // `supports()` and `bind()` are two different answers to "can this
        // model take my file", and nothing forced them to agree.
        use crate::media::{bind, Dialect, MediaRef, MediaSource};

        for uc in UseCase::ALL {
            // One attachment per required slot, exactly as the UI would send.
            let media: Vec<MediaRef> = uc
                .slots()
                .iter()
                .filter(|(_, required)| *required)
                .map(|(role, _)| {
                    MediaRef::new(
                        *role,
                        MediaSource::Url {
                            url: "https://example/x".into(),
                        },
                    )
                })
                .collect();
            if media.is_empty() {
                continue;
            }
            for m in models_for(uc) {
                // The same route *and* dialect the shell will use, or the test
                // proves nothing about the real path. Not "fal if present":
                // the picker marks a route that cannot do this job unusable and
                // hands over the next one, so judging by a route the user will
                // never be given would let a real failure through — and would
                // have hidden that Veo 3.1's fal route takes no media while its
                // Higgsfield route serves the tab.
                let serving = m
                    .routes
                    .iter()
                    .find(|r| route_serves(&m, r, uc))
                    .unwrap_or_else(|| {
                        panic!(
                            "{} is offered under {} with no route that serves it",
                            m.id,
                            uc.slug()
                        )
                    });
                let (dialect, slug) = match serving.provider {
                    ProviderId::Fal => (Dialect::Fal, serving.slug.clone()),
                    ProviderId::Higgsfield => (Dialect::Catalog, serving.slug.clone()),
                    _ => (Dialect::Catalog, String::new()),
                };
                assert!(
                    bind(&m.spec, &m.display_name, &media, dialect, &slug).is_ok(),
                    "{} is offered under {} but refuses the {} it requires",
                    m.id,
                    uc.slug(),
                    media
                        .iter()
                        .map(|x| x.role.label())
                        .collect::<Vec<_>>()
                        .join(" + ")
                );
            }
        }
    }

    #[test]
    fn the_no_media_list_names_real_models() {
        // A stale entry silently hides a model that works.
        let reg = registry();
        for id in NO_MEDIA_MODELS {
            assert!(reg.contains_key(id), "{id} is not in the registry");
        }
    }

    #[test]
    fn veo_is_offered_for_animate_image_only_because_of_the_higgsfield_route() {
        // `fal-ai/veo3.1` has no media field at all, which is why `veo3_1` sits
        // in NO_MEDIA_MODELS. Their platform publishes `/veo3.1/image-to-video`
        // beside it, and our catalogue agrees (`start_image`), so the model
        // belongs in the tab — with the fal route marked unusable there, or the
        // picker hands over a default that fails at submit.
        let reg = registry();
        for id in ["veo3_1", "veo3_1_fast"] {
            let m = &reg[id];
            assert!(
                supports(m, UseCase::ImageToVideo),
                "{id} should be offered under Animate Image"
            );
            let fal = m
                .routes
                .iter()
                .find(|r| r.provider == ProviderId::Fal)
                .unwrap();
            assert!(
                !route_serves(m, fal, UseCase::ImageToVideo),
                "{id}'s fal route takes no media and must be marked unusable there"
            );
            let hf = m
                .routes
                .iter()
                .find(|r| r.provider == ProviderId::Higgsfield)
                .unwrap();
            assert!(
                route_serves(m, hf, UseCase::ImageToVideo),
                "{id}'s Higgsfield route is the one that serves this tab"
            );
            // Text-to-video is the other way round: fal serves it and is priced.
            assert!(route_serves(m, fal, UseCase::TextToVideo));
        }
    }

    #[test]
    fn a_higgsfield_route_is_not_offered_for_a_mode_their_spec_lacks() {
        // None of the mirrors take a clip. The model may still be offered for
        // Edit Video by another route; this route must not be.
        let reg = registry();
        let m = &reg["kling-v2-5-turbo"];
        let hf = m
            .routes
            .iter()
            .find(|r| r.provider == ProviderId::Higgsfield)
            .unwrap();
        assert!(route_serves(m, hf, UseCase::ImageToVideo));
        assert!(!route_serves(m, hf, UseCase::EditVideo));
    }
}
