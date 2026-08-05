//! The on-disk media library.
//!
//! Provider result URLs expire — Higgsfield's own API deletes after seven days,
//! and most others are similarly temporary — so a generation is not really
//! finished until the bytes are local. This module owns that download, and the
//! folder the user picked to keep them in.
//!
//! Files land in a real directory the user chose, not an opaque app cache.
//! They paid for these; they should be able to find them in Finder.

use halation_core::engine::{JobError, Output, OutputKind};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Default library location. Visible, obvious, and outside the app bundle so
/// an uninstall never takes someone's work with it.
/// Where generated media goes until the user picks somewhere else.
///
/// Under the OS's own media folder — `~/Movies` on macOS, `~/Videos` on
/// Windows — rather than straight in the home directory. Two reasons, and the
/// second is not hypothetical:
///
/// 1. It is where a person already looks for video, and it is backed up and
///    indexed like the rest of their media.
/// 2. **`~/Halation` collides.** macOS filesystems are case-insensitive by
///    default, so it resolves to any existing `~/halation` — which on a
///    developer's machine is the source checkout. Observed live on 2026-08-05:
///    a generated clip was written into the repo root next to `Cargo.toml`.
///    Writing user media into an unrelated existing directory is bad on its own
///    terms; doing it to a git worktree is worse.
pub fn default_root() -> PathBuf {
    let Some(home) = dirs_home() else {
        return PathBuf::from("Halation");
    };
    let media = if cfg!(target_os = "windows") {
        home.join("Videos")
    } else {
        home.join("Movies")
    };
    // Only if the media folder actually exists — a stripped-down or headless
    // account may not have one, and creating it silently is presumptuous.
    if media.is_dir() {
        media.join("Halation")
    } else {
        home.join("Halation")
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Strip anything that would be illegal or surprising in a filename on either
/// platform. Windows is the stricter of the two, so we apply its rules
/// everywhere rather than producing files that only work on one OS.
fn sanitize(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    // Windows refuses these regardless of extension.
    const RESERVED: [&str; 9] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "LPT1", "LPT2", "LPT3",
    ];
    let safe = if RESERVED.iter().any(|r| trimmed.eq_ignore_ascii_case(r)) {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    };
    let safe = if safe.is_empty() {
        "untitled".to_string()
    } else {
        safe
    };
    // Leave room for a disambiguating suffix and an extension.
    safe.chars().take(80).collect()
}

fn extension_for(kind: OutputKind, url: &str) -> &'static str {
    // Trust the URL when it carries a familiar extension; fall back to the
    // media kind. Guessing wrong means the OS opens it with the wrong app.
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    for ext in [
        "mp4", "webm", "mov", "png", "jpg", "jpeg", "webp", "mp3", "wav",
    ] {
        if path.ends_with(&format!(".{ext}")) {
            return match ext {
                "jpeg" => "jpg",
                other => Box::leak(other.to_string().into_boxed_str()),
            };
        }
    }
    match kind {
        OutputKind::Video => "mp4",
        OutputKind::Image => "png",
        OutputKind::Audio => "mp3",
    }
}

/// Choose a path that does not already exist, appending `-2`, `-3` and so on.
/// Two generations from the same prompt are common and must not overwrite.
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-{}.{ext}", std::process::id()))
}

pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: PathBuf) -> Self {
        Library { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a job's outputs go. One folder per day keeps a heavy month from
    /// producing an unusable directory listing.
    fn dir_for(&self, created_at: i64) -> PathBuf {
        let days = created_at.max(0) / 86_400;
        // Deliberately not a calendar date: no timezone library, no ambiguity,
        // and the ordering is identical.
        self.root.join(format!("day-{days}"))
    }

    /// Download one output. Returns the local path.
    ///
    /// Writes to a temporary file and renames on success, so an interrupted
    /// download never leaves a half-file that looks complete.
    pub fn fetch(
        &self,
        out: &Output,
        prompt: &str,
        created_at: i64,
        client: &reqwest::blocking::Client,
    ) -> Result<PathBuf, JobError> {
        let dir = self.dir_for(created_at);
        std::fs::create_dir_all(&dir)
            .map_err(|e| JobError::Permanent(format!("cannot create {}: {e}", dir.display())))?;

        let stem = sanitize(prompt);
        let ext = extension_for(out.kind, &out.url);
        let final_path = unique_path(&dir, &stem, ext);
        let tmp = final_path.with_extension(format!("{ext}.part"));

        let mut resp = client
            .get(&out.url)
            .send()
            .map_err(|e| JobError::Transient(format!("download failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(JobError::from_status(resp.status().as_u16(), "download"));
        }

        {
            let mut f = std::fs::File::create(&tmp)
                .map_err(|e| JobError::Permanent(format!("cannot write {}: {e}", tmp.display())))?;
            resp.copy_to(&mut f)
                .map_err(|e| JobError::Transient(format!("download interrupted: {e}")))?;
            f.flush()
                .map_err(|e| JobError::Permanent(format!("flush failed: {e}")))?;
        }

        std::fs::rename(&tmp, &final_path)
            .map_err(|e| JobError::Permanent(format!("cannot finalize download: {e}")))?;
        Ok(final_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_path_separators() {
        // A prompt is user text and can contain anything at all.
        assert_eq!(sanitize("a/b\\c"), "a-b-c");
        assert_eq!(sanitize("what: a prompt?"), "what- a prompt-");
        assert!(!sanitize("../../etc/passwd").contains('/'));
    }

    #[test]
    fn sanitize_handles_windows_reserved_names() {
        // "CON.mp4" is unopenable on Windows regardless of extension.
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul"), "_nul");
        assert_eq!(sanitize("console"), "console", "only exact matches");
    }

    #[test]
    fn sanitize_never_yields_an_empty_name() {
        assert_eq!(sanitize(""), "untitled");
        assert_eq!(sanitize("   "), "untitled");
        assert_eq!(sanitize("..."), "untitled");
    }

    #[test]
    fn sanitize_bounds_length() {
        let long = "a".repeat(500);
        assert!(sanitize(&long).chars().count() <= 80);
    }

    #[test]
    fn extension_comes_from_the_url_when_it_has_one() {
        assert_eq!(extension_for(OutputKind::Video, "https://a/x.webm"), "webm");
        assert_eq!(extension_for(OutputKind::Image, "https://a/x.jpeg"), "jpg");
        // Query strings must not confuse the match.
        assert_eq!(
            extension_for(OutputKind::Video, "https://a/x.mp4?sig=abc&t=1"),
            "mp4"
        );
    }

    #[test]
    fn extension_falls_back_to_the_media_kind() {
        assert_eq!(extension_for(OutputKind::Video, "https://a/opaque"), "mp4");
        assert_eq!(extension_for(OutputKind::Image, "https://a/opaque"), "png");
        assert_eq!(extension_for(OutputKind::Audio, "https://a/opaque"), "mp3");
    }

    #[test]
    fn repeat_generations_do_not_overwrite_each_other() {
        let dir = std::env::temp_dir().join(format!("hal-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("cat.mp4"));

        let a = unique_path(&dir, "cat", "mp4");
        assert_eq!(a.file_name().unwrap(), "cat.mp4");
        std::fs::write(&a, b"x").unwrap();

        let b = unique_path(&dir, "cat", "mp4");
        assert_eq!(b.file_name().unwrap(), "cat-2.mp4");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outputs_are_grouped_by_day() {
        let lib = Library::new(PathBuf::from("/tmp/lib"));
        let same_day_a = lib.dir_for(86_400 * 5 + 10);
        let same_day_b = lib.dir_for(86_400 * 5 + 86_000);
        let next_day = lib.dir_for(86_400 * 6 + 10);
        assert_eq!(same_day_a, same_day_b);
        assert_ne!(same_day_a, next_day);
    }

    #[test]
    fn default_root_is_visible_not_hidden_in_app_data() {
        let root = default_root();
        assert!(root.ends_with("Halation"), "got {}", root.display());
        assert!(
            !root.to_string_lossy().contains("Application Support"),
            "the library must be somewhere a person can find it"
        );
    }

    #[test]
    fn the_default_root_is_not_the_bare_home_directory() {
        // The collision this prevents: macOS is case-insensitive, so ~/Halation
        // resolves to an existing ~/halation — on this machine, the source
        // checkout. A generated clip was written next to Cargo.toml before this
        // changed.
        let Some(home) = dirs_home() else { return };
        let root = default_root();
        assert_ne!(root, home.join("Halation").clone());
        assert!(
            root.starts_with(home.join("Movies")) || root.starts_with(home.join("Videos")),
            "expected a media folder, got {}",
            root.display()
        );
    }
}
