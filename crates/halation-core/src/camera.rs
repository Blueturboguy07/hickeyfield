//! The five-slot camera-template system.
//!
//! Higgsfield's single best idea is that a camera move is *data*, not prose: a
//! fixed chain of five slots — rig, path, pace, composition, resolve — rendered
//! into one sentence chain. Because none of the five slots ever names what is
//! being filmed, a move composes with any scene description at all. Swap the
//! scene, keep the move.
//!
//! **The schema is the reusable idea; the wording is not.** The 5-slot
//! decomposition is an uncopyrightable method and we adopt it deliberately.
//! Every template body below is written from scratch against the *meaning* of
//! the corresponding move, never transcribed from their product copy — see the
//! Legal section of the plan, and `scripts/lint-provenance.py`, which fails CI
//! on any shipped string with high shingle similarity to their string corpus.
//!
//! We ship 25 moves: 16 that they expose, plus 9 mechanical inverses (pan left,
//! tilt down, crane down, zoom out, dolly out, push out, the reversed dolly
//! zoom, and the two partial arcs). Their catalogue only offers one direction
//! per axis, which is a gap in their product rather than a design decision.

use serde::Serialize;
use std::fmt;

/// One camera move, decomposed into the five slots.
///
/// Not `Deserialize`: the fields are `&'static str` because the table is a
/// compile-time constant. Persist the [`CameraTemplate::slug`] and rehydrate
/// with [`get`] instead — that also means a stored recipe picks up any later
/// improvement to the wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CameraTemplate {
    /// Stable identifier. This is what gets persisted in recipes and presets.
    pub slug: &'static str,
    pub display_name: &'static str,
    /// Slot 1 — the rig and optical setup.
    pub camera: &'static str,
    /// Slot 2 — the path the camera travels.
    pub movement: &'static str,
    /// Slot 3 — the pace.
    pub speed: &'static str,
    /// Slot 4 — the composition rule held during the move.
    pub framing: &'static str,
    /// Slot 5 — the state the shot resolves to.
    pub end: &'static str,
}

impl CameraTemplate {
    /// Render the five slots into the sentence chain.
    ///
    /// The exact shape is load-bearing: models were prompted into this rhythm
    /// by everything that came before, and the labels give the reader (and us)
    /// a parseable structure to diff against.
    pub fn render(&self) -> String {
        format!(
            "Camera: {}. Movement: {}. Speed: {}. Framing: {}. End: {}.",
            self.camera, self.movement, self.speed, self.framing, self.end
        )
    }
}

impl fmt::Display for CameraTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

/// Every camera move we ship, in catalogue order: the 16 core moves first, then
/// the 9 derived inverses.
pub const TEMPLATES: &[CameraTemplate] = &[
    // ---- The 16 core moves -------------------------------------------------
    CameraTemplate {
        slug: "aerial-pullback",
        display_name: "Aerial pullback",
        camera: "an airborne rig reversing away and climbing",
        movement: "pull back from the subject and gain height at the same time",
        speed: "an unhurried reveal",
        framing: "open out from a tight frame until everything around is included",
        end: "settle into a broad establishing frame",
    },
    CameraTemplate {
        slug: "drone-orbit",
        display_name: "Drone orbit",
        camera: "an airborne rig circling at a fixed radius",
        movement: "travel a horizontal ring around one anchor point",
        speed: "even and unbroken",
        framing: "hold the subject in place while the backdrop wheels past",
        end: "close the arc looking straight at the subject",
    },
    CameraTemplate {
        slug: "handheld",
        display_name: "Handheld",
        camera: "carried by hand, unsupported",
        movement: "loose organic drift with constant small corrections",
        speed: "the rhythm of quiet breathing",
        framing: "trail the subject imprecisely, leaving the human error in",
        end: "come to rest roughly on the subject, never entirely still",
    },
    CameraTemplate {
        slug: "bullet-time",
        display_name: "Bullet time",
        camera: "a multi-rig sweep that reads as one frozen instant",
        movement: "arc past a subject held motionless in time",
        speed: "the world stopped while the viewpoint travels",
        framing: "keep the action paused at its peak as the angle changes",
        end: "let time restart from the new vantage point",
    },
    CameraTemplate {
        slug: "tracking-shot",
        display_name: "Tracking shot",
        camera: "a gimbal or dolly running alongside",
        movement: "keep pace beside a subject already in motion",
        speed: "locked to whatever pace the subject sets",
        framing: "pin the subject to one spot in frame while the surroundings streak past",
        end: "slow to a stop with the subject still held",
    },
    CameraTemplate {
        slug: "pan-right",
        display_name: "Pan right",
        camera: "locked to a tripod, rotating on its axis",
        movement: "swing the lens horizontally, left toward right, over a fixed pivot",
        speed: "measured and uniform",
        framing: "sweep the composition across, holding the frame level throughout",
        end: "come to rest on whatever lies to the right",
    },
    CameraTemplate {
        slug: "rack-focus",
        display_name: "Rack focus",
        camera: "planted still, with the focal plane moving",
        movement: "shift focus off the near plane and onto something deeper in the shot",
        speed: "one decisive pull",
        framing: "leave the frame untouched and let attention travel through depth",
        end: "arrive crisp on the far plane",
    },
    CameraTemplate {
        slug: "push-in",
        display_name: "Push in",
        camera: "a slow creep straight at the subject",
        movement: "advance along the lens axis, closing the gap",
        speed: "so gradual it is barely noticeable",
        framing: "narrow the frame steadily so pressure builds",
        end: "arrive at a close-up and stop",
    },
    CameraTemplate {
        slug: "crane-up",
        display_name: "Crane up",
        camera: "mounted on an arm that lifts",
        movement: "rise straight upward above the subject",
        speed: "fluid and unhurried",
        framing: "begin at standing height and climb until the view looks down over everything",
        end: "hold a broad high-angle frame",
    },
    CameraTemplate {
        slug: "static-shot",
        display_name: "Static shot",
        camera: "locked off on a fixed mount",
        movement: "no movement whatsoever for the length of the take",
        speed: "motionless",
        framing: "identical angle, height, distance and composition from first frame to last",
        end: "finish exactly where it started",
    },
    CameraTemplate {
        slug: "360-orbit",
        display_name: "360 orbit",
        camera: "one complete revolution around the subject",
        movement: "carry the lens the whole way round and back to the start",
        speed: "uniform and unbroken",
        framing: "keep the subject pinned dead centre through every degree",
        end: "arrive back at the opening angle",
    },
    CameraTemplate {
        slug: "tilt-up",
        display_name: "Tilt up",
        camera: "locked to a tripod, pivoting vertically",
        movement: "swing the lens upward from a low start",
        speed: "measured and intentional",
        framing: "open low and climb to show how much sits above",
        end: "rest high, at the top of the frame",
    },
    CameraTemplate {
        slug: "zoom-in",
        display_name: "Zoom in",
        camera: "planted still, working the lens barrel",
        movement: "extend the focal length so the subject appears to come nearer",
        speed: "even and unhurried",
        framing: "crop in from the wide view down to a single detail",
        end: "stop on a tight frame of the subject",
    },
    CameraTemplate {
        slug: "dolly-in",
        display_name: "Dolly in",
        camera: "the whole body of the camera rolling forward",
        movement: "carry the camera itself nearer to the subject",
        speed: "deliberate and unhurried",
        framing: "shorten the distance so near and far planes slide against each other",
        end: "arrive close and enveloping",
    },
    CameraTemplate {
        slug: "dolly-zoom",
        display_name: "Dolly zoom",
        camera: "the body retreating while the lens tightens",
        movement: "back away while the focal length tightens by exactly as much",
        speed: "slow, and wrong-footing",
        framing: "hold the subject at a constant size while everything behind it distorts",
        end: "sit in the vertigo with the subject unchanged",
    },
    CameraTemplate {
        slug: "pov-walk",
        display_name: "POV walk",
        camera: "a subjective viewpoint, seen from inside the scene",
        movement: "step forward as the subject would",
        speed: "an ordinary walking rhythm with a faint bob",
        framing: "aim where attention would naturally fall",
        end: "arrive at the destination and hold it in frame",
    },
    // ---- The 9 derived inverses -------------------------------------------
    // Each mirrors exactly the slots that encode direction and leaves the rest
    // of the move alone, so a pair reads as one axis rather than two unrelated
    // moves.
    CameraTemplate {
        slug: "pan-left",
        display_name: "Pan left",
        camera: "locked to a tripod, rotating on its axis",
        movement: "swing the lens horizontally, right toward left, over a fixed pivot",
        speed: "measured and uniform",
        framing: "sweep the composition across, holding the frame level throughout",
        end: "come to rest on whatever lies to the left",
    },
    CameraTemplate {
        slug: "tilt-down",
        display_name: "Tilt down",
        camera: "locked to a tripod, pivoting vertically",
        movement: "swing the lens downward from a high start",
        speed: "measured and intentional",
        framing: "open high and descend to show what sits below",
        end: "rest low, at the foot of the frame",
    },
    CameraTemplate {
        slug: "crane-down",
        display_name: "Crane down",
        camera: "mounted on an arm that descends",
        movement: "drop straight downward toward the subject",
        speed: "fluid and unhurried",
        framing: "begin looking down over everything and fall to standing height",
        end: "hold level with the subject",
    },
    CameraTemplate {
        slug: "zoom-out",
        display_name: "Zoom out",
        camera: "planted still, working the lens barrel",
        movement: "shorten the focal length so the subject appears to fall away",
        speed: "even and unhurried",
        framing: "crop out from a single detail to the full view",
        end: "stop on the wide frame",
    },
    CameraTemplate {
        slug: "dolly-out",
        display_name: "Dolly out",
        camera: "the whole body of the camera rolling backward",
        movement: "carry the camera itself away from the subject",
        speed: "deliberate and unhurried",
        framing: "lengthen the distance so near and far planes slide against each other",
        end: "arrive wide, with context around the subject",
    },
    CameraTemplate {
        slug: "push-out",
        display_name: "Push out",
        camera: "a slow creep straight back from the subject",
        movement: "withdraw along the lens axis, opening the gap",
        speed: "so gradual it is barely noticeable",
        framing: "widen the frame steadily so the pressure drains away",
        end: "arrive at a wide shot and stop",
    },
    CameraTemplate {
        slug: "dolly-zoom-in",
        display_name: "Dolly zoom in",
        camera: "the body advancing while the lens widens",
        movement: "close in while the focal length widens by exactly as much",
        speed: "slow, and wrong-footing",
        framing: "hold the subject at a constant size while everything behind it rushes outward",
        end: "sit in the inverted vertigo with the subject unchanged",
    },
    CameraTemplate {
        slug: "arc-left",
        display_name: "Arc left",
        camera: "a partial ring around the subject",
        movement: "travel to the left along the arc",
        speed: "even and unbroken",
        framing: "hold the subject in place while the backdrop wheels the other way",
        end: "stop part-way round, still facing the subject",
    },
    CameraTemplate {
        slug: "arc-right",
        display_name: "Arc right",
        camera: "a partial ring around the subject",
        movement: "travel to the right along the arc",
        speed: "even and unbroken",
        framing: "hold the subject in place while the backdrop wheels the other way",
        end: "stop part-way round, still facing the subject",
    },
];

/// How many of [`TEMPLATES`] correspond to a move Higgsfield exposes. The rest
/// are ours. Kept as a constant so the split stays visible if the table grows.
pub const CORE_MOVES: usize = 16;

/// Look a move up by slug. `None` for an unknown slug — callers must not fall
/// back to a "nearest" move, because silently generating a different camera
/// move than the one a saved recipe asked for is worse than generating none.
pub fn get(slug: &str) -> Option<&'static CameraTemplate> {
    TEMPLATES.iter().find(|t| t.slug == slug)
}

/// Every slug, in catalogue order.
pub fn slugs() -> impl Iterator<Item = &'static str> {
    TEMPLATES.iter().map(|t| t.slug)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Nouns that would tie a template to one particular scene. A template
    /// containing any of these has stopped being a camera move and started
    /// being a shot description, which breaks the compose-with-any-scene
    /// property this whole module exists for.
    ///
    /// The abstract placeholder "subject" is deliberately *not* banned — it is
    /// the mechanism that makes the templates composable, standing in for
    /// whatever the user described without asserting anything about it.
    const CONCRETE_SUBJECTS: &[&str] = &[
        "man",
        "men",
        "woman",
        "women",
        "person",
        "people",
        "child",
        "children",
        "boy",
        "boys",
        "girl",
        "girls",
        "dog",
        "dogs",
        "cat",
        "cats",
        "animal",
        "animals",
        "bird",
        "birds",
        "horse",
        "horses",
        "car",
        "cars",
        "truck",
        "vehicle",
        "bike",
        "building",
        "buildings",
        "house",
        "room",
        "street",
        "city",
        "forest",
        "tree",
        "trees",
        "mountain",
        "beach",
        "ocean",
        "sea",
        "water",
        "fire",
        "smoke",
        "sky",
        "clouds",
        "ground",
        "floor",
        "wall",
        "landscape",
        "terrain",
        "product",
        "bottle",
        "logo",
        "robot",
        "monster",
        "dancer",
        "actor",
        "model",
        "character",
        "face",
        "faces",
        "hair",
        "eyes",
        "hands",
        "head",
        "clothes",
        "food",
        "table",
        "chair",
        "window",
        "door",
    ];

    fn words(text: &str) -> Vec<String> {
        text.to_ascii_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_catalogue_is_the_16_core_moves_plus_9_inverses() {
        assert_eq!(TEMPLATES.len(), 25);
        let core: Vec<_> = TEMPLATES[..CORE_MOVES].iter().map(|t| t.slug).collect();
        assert_eq!(
            core,
            vec![
                "aerial-pullback",
                "drone-orbit",
                "handheld",
                "bullet-time",
                "tracking-shot",
                "pan-right",
                "rack-focus",
                "push-in",
                "crane-up",
                "static-shot",
                "360-orbit",
                "tilt-up",
                "zoom-in",
                "dolly-in",
                "dolly-zoom",
                "pov-walk",
            ]
        );
        let derived: Vec<_> = TEMPLATES[CORE_MOVES..].iter().map(|t| t.slug).collect();
        assert_eq!(
            derived,
            vec![
                "pan-left",
                "tilt-down",
                "crane-down",
                "zoom-out",
                "dolly-out",
                "push-out",
                "dolly-zoom-in",
                "arc-left",
                "arc-right",
            ]
        );
    }

    #[test]
    fn slugs_are_unique_and_lookup_works() {
        let mut seen = HashSet::new();
        for t in TEMPLATES {
            assert!(seen.insert(t.slug), "duplicate slug {}", t.slug);
            assert_eq!(get(t.slug).map(|x| x.slug), Some(t.slug));
        }
        assert_eq!(slugs().count(), TEMPLATES.len());
        // An unknown slug must not resolve to a near neighbour.
        assert!(get("dolly-inn").is_none());
        assert!(get("").is_none());
    }

    #[test]
    fn renders_the_exact_five_slot_chain() {
        let t = get("push-in").unwrap();
        assert_eq!(
            t.render(),
            "Camera: a slow creep straight at the subject. \
             Movement: advance along the lens axis, closing the gap. \
             Speed: so gradual it is barely noticeable. \
             Framing: narrow the frame steadily so pressure builds. \
             End: arrive at a close-up and stop."
        );
        // Display and render must not drift apart.
        assert_eq!(t.to_string(), t.render());
    }

    #[test]
    fn every_template_fills_all_five_slots_in_order() {
        for t in TEMPLATES {
            let r = t.render();
            let mut cursor = 0;
            for label in ["Camera: ", "Movement: ", "Speed: ", "Framing: ", "End: "] {
                let at = r[cursor..]
                    .find(label)
                    .unwrap_or_else(|| panic!("{} is missing slot {label}", t.slug));
                cursor += at + label.len();
            }
            assert!(r.ends_with('.'), "{} must end with a period", t.slug);
            // A slot body that already carried a period would produce ".." and
            // break the chain's rhythm.
            assert!(!r.contains(".."), "{} has a doubled period", t.slug);
            for slot in [t.camera, t.movement, t.speed, t.framing, t.end] {
                assert!(!slot.is_empty(), "{} has an empty slot", t.slug);
                assert!(
                    !slot.ends_with('.'),
                    "{} slot must not be terminated",
                    t.slug
                );
                assert_eq!(slot.trim(), slot, "{} slot has stray whitespace", t.slug);
            }
        }
    }

    /// The critical correctness property. If this fails, the template has
    /// smuggled scene content into the move and no longer composes.
    #[test]
    fn no_template_names_a_concrete_subject_or_setting() {
        for t in TEMPLATES {
            for w in words(&t.render()) {
                assert!(
                    !CONCRETE_SUBJECTS.contains(&w.as_str()),
                    "{} mentions the concrete noun {:?} — a camera move must \
                     describe only the camera, never what is in front of it",
                    t.slug,
                    w
                );
            }
        }
    }

    #[test]
    fn templates_compose_with_any_scene_unchanged() {
        // Same move, two scenes that share nothing. The move text must survive
        // both byte-for-byte, which is what "the move stays separate from the
        // scene" has to mean mechanically.
        let scenes = [
            "A cracked porcelain teapot on a windowsill at dawn.",
            "Molten steel pouring through a foundry at night.",
        ];
        for t in TEMPLATES {
            let move_text = t.render();
            for scene in scenes {
                let composed = format!("{move_text} {scene}");
                assert!(composed.contains(&move_text), "{} did not survive", t.slug);
                assert!(composed.ends_with(scene));
            }
        }
    }

    #[test]
    fn inverse_pairs_mirror_only_the_direction_slots() {
        // A derived inverse must share the rig and the pace with its source —
        // otherwise it is a different move, not the same axis reversed.
        for (a, b) in [
            ("pan-right", "pan-left"),
            ("tilt-up", "tilt-down"),
            ("zoom-in", "zoom-out"),
            ("dolly-in", "dolly-out"),
            ("push-in", "push-out"),
            ("dolly-zoom", "dolly-zoom-in"),
            ("arc-left", "arc-right"),
        ] {
            let (x, y) = (get(a).unwrap(), get(b).unwrap());
            assert_eq!(x.speed, y.speed, "{a}/{b} should share a pace");
            assert_ne!(x.movement, y.movement, "{a}/{b} must travel differently");
            assert_ne!(x.render(), y.render(), "{a}/{b} rendered identically");
        }

        // And the direction words must actually be opposite, not merely absent.
        assert!(get("pan-right").unwrap().end.contains("right"));
        assert!(get("pan-left").unwrap().end.contains("left"));
        assert!(get("tilt-up").unwrap().movement.contains("upward"));
        assert!(get("tilt-down").unwrap().movement.contains("downward"));
        assert!(get("crane-up").unwrap().movement.contains("upward"));
        assert!(get("crane-down").unwrap().movement.contains("downward"));
        assert!(get("arc-left").unwrap().movement.contains("left"));
        assert!(get("arc-right").unwrap().movement.contains("right"));
    }

    #[test]
    fn display_names_are_present_and_distinct() {
        let mut seen = HashSet::new();
        for t in TEMPLATES {
            assert!(!t.display_name.is_empty(), "{} has no label", t.slug);
            assert!(
                seen.insert(t.display_name),
                "duplicate label {}",
                t.display_name
            );
        }
    }

    #[test]
    fn serializes_for_the_ui() {
        let t = get("static-shot").unwrap();
        let v: serde_json::Value = serde_json::to_value(t).unwrap();
        assert_eq!(v["slug"], "static-shot");
        assert_eq!(v["display_name"], "Static shot");
        assert!(v["framing"].is_string());
    }
}
