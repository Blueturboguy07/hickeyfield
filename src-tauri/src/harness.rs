//! The harness: turning what the user selected into the prompt actually sent.
//!
//! `enhance.rs`, `preset.rs`, `camera.rs`, `enhancer.rs` and the `prompts/`
//! corpus were all complete, tested, and **called by nothing**. 371 green tests
//! were exercising a subsystem no user path reached, so selecting a camera
//! preset changed the id stored in SQLite and nothing else — the provider
//! received the raw typed prompt with no camera clause and no rewrite.
//!
//! This module is the missing caller. It lives in the shell rather than the
//! core because it needs the user's settings and their choice of rewriter,
//! and it is deliberately one function so the ordering hazards below are
//! decided in exactly one place.
//!
//! **Order matters and is not arbitrary:**
//!
//! 1. Resolve the preset *first* — the enhance decision needs to know whether a
//!    real one was chosen (rule 1 forces enhancement on).
//! 2. Build the camera clause into [`PromptParts`], but hand the rewriter only
//!    the **scene**. Passing the compiled prompt still "works" and quietly
//!    degrades every generation: the model rewrites our five-slot camera
//!    grammar into prose and the preset's precision is lost.
//! 3. Decide *whether* to enhance before running anything, because an end frame
//!    forbids it unconditionally.
//! 4. Recompile after the rewrite, so the camera clause survives verbatim.

use hickeyfield_core::enhance::{self, EnhanceInputs, PresetSelection, PromptParts};
use hickeyfield_core::enhancer::{
    enhance_or_original, mode_for, recipe_pin, EnhanceRequest, LocalEnhancer, RewriteStatus,
    Rewritten,
};
use hickeyfield_core::{corpus, MediaRef, Model};

/// What the harness produced, and how.
pub struct Compiled {
    /// The string to send to the provider.
    pub prompt: String,
    /// The user's own words, always preserved so the UI can show both.
    pub original: String,
    /// `Some` only when a rewriter actually changed the text.
    pub enhanced: Option<String>,
    /// Which corpus and rewriter ran. `None` when none did — never a
    /// placeholder, because a guessed pin makes two unlike generations look
    /// reproducible.
    pub version: Option<String>,
    /// Shown next to the toggle when the rewrite did not happen or failed.
    pub note: Option<String>,
}

/// Which rewriter to use, chosen by the user.
pub enum Rewriter<'a> {
    /// No rewrite. The compiled prompt still gets its camera clause.
    None,
    /// Local Ollama. Free, private, no key.
    Ollama { model: &'a str },
}

/// Compile the prompt for one submission.
pub fn compile(
    model: &Model,
    raw_prompt: &str,
    preset_id: Option<&str>,
    media: &[MediaRef],
    enhance_toggle: bool,
    rewriter: Rewriter<'_>,
) -> Result<Compiled, String> {
    // 1. The preset, resolved to a real family rather than an opaque id.
    let family = preset_id.and_then(hickeyfield_core::preset::get);

    // 2. The camera clause. `with_camera` takes the slug and looks up the
    //    five-slot template, so the grammar stays in one place.
    let mut parts = PromptParts::scene(raw_prompt);
    if let Some(f) = family {
        if let Some(cam) = f.camera_template.as_deref() {
            parts = parts.with_camera(cam);
        }
    }

    // 3. Whether to enhance at all. Three rules, and the end-frame one is
    //    unconditional: rewriting a prompt between two fixed frames produces
    //    something that matches neither.
    let decision = enhance::build(
        &parts,
        EnhanceInputs::new(model.job_type)
            .with_preset(PresetSelection::from_family(family))
            .with_end_frame(hickeyfield_core::media::has_end_frame(media))
            .with_toggle(enhance_toggle),
        None,
    );

    let original = raw_prompt.to_string();
    let compiled_now = decision.prompt.clone();

    if decision.has_unresolved_sentinel {
        return Err(
            "this prompt still points at an attachment that has not been bound".to_string(),
        );
    }

    if !decision.enhance {
        return Ok(Compiled {
            prompt: compiled_now,
            original,
            enhanced: None,
            version: None,
            note: Some(decision.reason.explanation().to_string()),
        });
    }

    let Rewriter::Ollama { model: tag } = rewriter else {
        return Ok(Compiled {
            prompt: compiled_now,
            original,
            enhanced: None,
            version: None,
            note: Some("no rewriter selected — sent as written".to_string()),
        });
    };

    // 4. The rewrite. Note it receives `raw_prompt`, NOT the compiled string:
    //    handing it the camera clause invites the model to paraphrase our
    //    five-slot grammar into prose, which loses exactly the precision the
    //    preset exists to supply.
    let roles: Vec<_> = media.iter().map(|m| m.role).collect();
    let mut req = EnhanceRequest::new(
        raw_prompt,
        model.job_type,
        &model.display_name,
        model.modality,
    )
    .with_media(&roles);
    if let Some(f) = family {
        req = req.with_preset(&f.description);
    }

    let Some(mode) = mode_for(&req) else {
        // Audio and 3D have no overlay, and the base corpus must never be used
        // alone. Refusing beats sending shot-grammar guidance to a TTS model.
        return Ok(Compiled {
            prompt: compiled_now,
            original,
            enhanced: None,
            version: None,
            note: Some("no enhancer guidance exists for this kind of model".to_string()),
        });
    };

    let system = corpus::system_prompt_for(mode)?;
    let out: Rewritten = enhance_or_original(&LocalEnhancer::new(tag, system), &req);

    match out.status {
        RewriteStatus::Rewritten => {
            // 5. Put the rewritten scene back and recompile, so the camera
            //    clause is appended verbatim to the improved prose.
            let final_prompt = PromptParts {
                scene: out.prompt.clone(),
                ..parts
            }
            .compile();
            Ok(Compiled {
                prompt: final_prompt,
                original,
                enhanced: Some(out.prompt),
                version: Some(recipe_pin(corpus::CORPUS_ID, mode, Some(("ollama", tag)))),
                note: None,
            })
        }
        // A failed rewrite must never block a generation the user asked for,
        // and must never look like it succeeded.
        _ => Ok(Compiled {
            prompt: compiled_now,
            original,
            enhanced: None,
            version: None,
            note: out.note,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickeyfield_core::{MediaRole, ProviderId};

    fn model(id: &str) -> Model {
        hickeyfield_core::registry::registry()
            .remove(id)
            .unwrap_or_else(|| panic!("{id} missing from the registry"))
    }

    #[test]
    fn a_camera_preset_reaches_the_provider() {
        // The bug this whole module exists for: selecting a preset used to
        // change the stored id and nothing else, so the provider received the
        // raw prompt with no camera clause.
        let m = model("kling3_0");
        let slug = hickeyfield_core::camera::slugs().next().unwrap();
        let out = compile(
            &m,
            "a lighthouse in fog",
            Some(slug),
            &[],
            false,
            Rewriter::None,
        )
        .unwrap();
        assert_ne!(
            out.prompt, "a lighthouse in fog",
            "the preset contributed nothing to the prompt"
        );
        assert!(out.prompt.contains("a lighthouse in fog"), "{}", out.prompt);
    }

    #[test]
    fn an_end_frame_forbids_the_rewrite_even_with_a_preset() {
        // Rule 2 beats rule 1. Interpolating between two fixed frames must not
        // have its prompt rewritten — the result would match neither frame.
        let m = model("kling3_0");
        let slug = hickeyfield_core::camera::slugs().next().unwrap();
        let media = [
            MediaRef::url(MediaRole::Start, "https://a/1.png"),
            MediaRef::url(MediaRole::End, "https://a/2.png"),
        ];
        let out = compile(
            &m,
            "a lighthouse",
            Some(slug),
            &media,
            true,
            Rewriter::Ollama { model: "nope" },
        )
        .unwrap();
        assert!(out.enhanced.is_none(), "an end frame must forbid rewriting");
        assert!(out.version.is_none());
    }

    #[test]
    fn the_users_own_words_are_always_preserved() {
        let m = model("kling3_0");
        let out = compile(&m, "a red door", None, &[], false, Rewriter::None).unwrap();
        assert_eq!(out.original, "a red door");
    }

    #[test]
    fn a_missing_rewriter_still_produces_a_sendable_prompt() {
        // An optional improvement must never block a paid generation.
        let m = model("kling3_0");
        let out = compile(
            &m,
            "a red door",
            None,
            &[],
            true,
            Rewriter::Ollama {
                model: "definitely-not-installed",
            },
        )
        .unwrap();
        assert!(!out.prompt.is_empty());
        assert!(out.enhanced.is_none());
        // And it must say why, rather than looking like it worked.
        assert!(out.note.is_some());
    }

    #[test]
    fn a_version_pin_is_recorded_only_when_a_rewrite_happened() {
        // A guessed pin would make two unlike generations look reproducible.
        let m = model("kling3_0");
        let out = compile(&m, "x", None, &[], false, Rewriter::None).unwrap();
        assert!(out.version.is_none());
    }

    #[test]
    fn the_camera_clause_is_the_five_slot_grammar_not_a_paraphrase() {
        // The preset's value is its precision. If the compiled prompt loses the
        // slot structure, the preset has been reduced to a label.
        let m = model("kling3_0");
        let out = compile(
            &m,
            "a harbour at dusk",
            Some("push-in"),
            &[],
            false,
            Rewriter::None,
        )
        .unwrap();
        let tmpl = hickeyfield_core::camera::get("push-in").unwrap().render();
        assert!(
            out.prompt.contains(&tmpl),
            "expected the rendered template verbatim.\n  got: {}\n want: {tmpl}",
            out.prompt
        );
    }

    #[test]
    fn a_preset_forces_enhancement_on_even_with_the_toggle_off() {
        // Rule 1. The preset's aesthetic is delivered by the rewrite, so
        // selecting one overrides the toggle — that is why the toggle is not
        // simply honoured here.
        let m = model("kling3_0");
        let off = compile(&m, "a harbour", Some("push-in"), &[], false, Rewriter::None).unwrap();
        // With no rewriter the text is unchanged, but the *decision* must have
        // been to enhance — visible in the note, which explains the forcing.
        assert!(
            off.note.as_deref() != Some("enhancement is off for this model by default"),
            "a real preset must not leave the default-off reason in place: {:?}",
            off.note
        );
    }

    #[test]
    fn every_launch_model_compiles_without_panicking() {
        // job_type, preset lookup and mode selection all index by model; a gap
        // in any of them would panic on a model a new user is shown first.
        for m in hickeyfield_core::registry::launch_models() {
            let out = compile(&m, "a test", None, &[], false, Rewriter::None);
            assert!(out.is_ok(), "{} failed to compile a prompt", m.id);
        }
        let _ = ProviderId::Fal;
    }
}
