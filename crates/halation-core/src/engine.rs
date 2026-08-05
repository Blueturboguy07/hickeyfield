//! The job engine: submit, poll, persist, reattach.
//!
//! This lives in the core rather than the shell for one reason that shapes the
//! whole design — the job outlives the window. A generation started and then
//! dismissed keeps running, and is still there on relaunch. The web version of
//! this product could not have done that without a server holding the user's
//! key, which is precisely what we refuse to build.
//!
//! Transport and storage are traits so this module stays testable without a
//! network or a database. The real implementations live in the shell.

use crate::cost::Billable;
use crate::job::{JobStatus, Phase};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A submitted generation. One user action, one JobSet, possibly several
/// output media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSet {
    pub id: String,
    pub model_id: String,
    pub route_id: String,
    /// The provider's own handle for this request, used for polling.
    pub request_id: String,
    /// The endpoint we actually submitted to.
    ///
    /// Not derivable from `route_id`: fal splits a model across one endpoint
    /// per input mode and the registry stores only the family root, so submit
    /// resolves `…/mini` to `…/mini/text-to-video`. Polling rebuilt the URL
    /// from the root instead and got **405** — the job ran and we could not
    /// read it. Recording what we called is the only way the two can agree.
    #[serde(default)]
    pub endpoint: String,
    pub status: JobStatus,
    pub prompt: String,
    /// Set when the enhancer rewrote the prompt. Rendered as a second,
    /// separately copyable chip so the user can see what was actually sent.
    pub enhanced_prompt: Option<String>,
    /// Which corpus and rewriter produced `enhanced_prompt`, e.g.
    /// `enhancer.v1/video+ollama/qwen2.5:7b`.
    ///
    /// `None` when nothing rewrote the prompt — never a placeholder. A guessed
    /// pin would make two generations from different corpora look reproducible.
    #[serde(default)]
    pub enhancer_version: Option<String>,
    /// Why the rewrite did not happen, when it did not. Shown next to the
    /// toggle so "enhance was on and nothing changed" is never silent.
    #[serde(default)]
    pub enhance_note: Option<String>,
    /// Things that happened which the user should know but which did not fail
    /// the job — a setting the endpoint has no field for, a value we rewrote.
    ///
    /// Deliberately separate from `fail_reason` (the job died) and from
    /// `enhance_note` (about the rewriter). Folding an advisory into either
    /// puts it under the wrong heading in the UI, which is its own small
    /// version of telling the user something untrue.
    #[serde(default)]
    pub advisories: Vec<String>,
    pub preset_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub results: Vec<Output>,
    pub estimated_usd: Option<f64>,
    pub actual_usd: Option<f64>,
    pub fail_reason: Option<String>,
    /// The generation settings, kept verbatim so Rerun and recipe export can
    /// reconstruct the request exactly.
    pub settings: serde_json::Value,
    /// The inputs the user attached, as roles rather than wire flags.
    ///
    /// Persisted because without it Rerun restored an image-to-video job as
    /// text-to-video — a different, cheaper-looking generation that is not the
    /// one the user asked to repeat, charged at the same price. `default` so
    /// rows written before this field existed still deserialise.
    #[serde(default)]
    pub media: Vec<crate::media::MediaRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub url: String,
    pub kind: OutputKind,
    /// Set once the file has been copied into the user's library. Provider URLs
    /// expire — Higgsfield's own API deletes results after 7 days — so a job is
    /// not really finished until the bytes are local.
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputKind {
    Image,
    Video,
    Audio,
}

impl JobSet {
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// Everything the user is owed has been fetched: terminal, and if it
    /// succeeded, every output is on disk.
    pub fn is_settled(&self) -> bool {
        self.is_terminal()
            && (self.status.phase() != Phase::Completed
                || self.results.iter().all(|r| r.local_path.is_some()))
    }
}

/// What a provider adapter must do. Deliberately narrow — the engine does not
/// care how a provider spells its parameters, only that it can start work and
/// report on it.
/// What a provider gives back when it accepts a request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Submission {
    pub request_id: String,
    /// The provider's own status URL, when it supplies one.
    ///
    /// **Authoritative, and not reconstructible.** fal answers a submit to
    /// `fal-ai/bytedance/seedance-2.0/mini/text-to-video` with a status URL
    /// under `fal-ai/bytedance` — the first two path segments only. Rebuilding
    /// the URL from the submit path gives 405; rebuilding it from the family
    /// root also gives 405. Both were tried. The server knows, so ask it.
    pub status_url: Option<String>,
}

pub trait ProviderClient: Send + Sync {
    /// Start a generation.
    fn submit(&self, model_id: &str, body: &serde_json::Value) -> Result<Submission, JobError>;

    /// Poll one request.
    ///
    /// `poll_ref` is the provider's own status URL when it gave one, and
    /// otherwise the model path to build one from. fal needs the former: its
    /// queue is scoped under a prefix that is neither the submit path nor the
    /// family root, and every reconstruction attempt answered **405**.
    fn poll(&self, poll_ref: &str, request_id: &str) -> Result<PollResult, JobError>;

    /// Best-effort. Most providers only allow cancellation while queued.
    fn cancel(&self, _poll_ref: &str, _request_id: &str) -> Result<(), JobError> {
        Err(JobError::Unsupported)
    }
    /// How often to poll this provider. fal's own SDK uses 500ms; most others
    /// are happier at a few seconds.
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(3)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    pub status: JobStatus,
    pub outputs: Vec<Output>,
    pub fail_reason: Option<String>,
    pub actual_usd: Option<f64>,
}

/// Persistence. Implemented over SQLite in the shell; an in-memory version
/// backs the tests here.
pub trait JobStore: Send + Sync {
    fn upsert(&self, job: &JobSet) -> Result<(), JobError>;
    fn get(&self, id: &str) -> Result<Option<JobSet>, JobError>;
    fn all(&self) -> Result<Vec<JobSet>, JobError>;
    /// Jobs that were still running when we last exited. Re-attached on launch.
    fn unfinished(&self) -> Result<Vec<JobSet>, JobError> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|j| !j.is_settled())
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobError {
    /// Worth trying again: timeouts, rate limits, provider 5xx.
    Transient(String),
    /// Will not improve by retrying: bad request, auth failure, moderation.
    Permanent(String),
    Unsupported,
}

impl JobError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, JobError::Transient(_))
    }

    /// Map an HTTP status to the right kind. The retryable set is the same one
    /// Higgsfield's own SDK uses, which is a reasonable signal that it matches
    /// how these providers actually fail.
    pub fn from_status(code: u16, body: &str) -> Self {
        match code {
            408 | 429 | 500 | 502 | 503 | 504 => {
                JobError::Transient(format!("HTTP {code}: {body}"))
            }
            _ => JobError::Permanent(format!("HTTP {code}: {body}")),
        }
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobError::Transient(m) | JobError::Permanent(m) => f.write_str(m),
            JobError::Unsupported => f.write_str("not supported by this provider"),
        }
    }
}

/// Retry/backoff policy. Ported from Higgsfield's own client: 5 attempts,
/// exponential from 1s, capped at 30s.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub max_attempts: u32,
    pub initial: Duration,
    pub cap: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff {
            max_attempts: 5,
            initial: Duration::from_secs(1),
            cap: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// Delay before attempt `n` (1-based). `None` once attempts are exhausted.
    pub fn delay(&self, attempt: u32) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }
        let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
        Some(std::cmp::min(self.initial * factor, self.cap))
    }
}

/// Guards against polling forever when a provider simply never answers.
///
/// This is the **base** term of [`TimeoutPolicy`], not a whole-job budget: it
/// is the patience a job gets before anything is added for how much output was
/// actually asked for. Used flat, as the only deadline, it hands the same ten
/// minutes to a single 480p still and a 10s 4K clip, and reports "gave up after
/// 10 minutes" for work the provider may still be running — and still billing.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// A 720p frame, in megapixels. The unit [`TimeoutPolicy`] scales resolution
/// against.
const REFERENCE_MEGAPIXELS: f64 = 1280.0 * 720.0 / 1_000_000.0;

/// Beyond this, more work cannot change the answer — every policy's `cap` is
/// far below `per_work_unit * MAX_WORK_UNITS`. It exists only so that a corrupt
/// or hostile `seconds` cannot overflow [`Duration::mul_f64`], which panics,
/// inside the poll loop.
const MAX_WORK_UNITS: f64 = 100_000.0;

/// How long to keep an unanswered request open before giving up on it.
///
/// **This is a patience budget, not a prediction.** We hold no measured
/// per-model latency data, and inventing one would be a guess of exactly the
/// kind this codebase refuses to make about prices. What the policy does
/// promise is narrower and checkable: asking for more output never shortens
/// the deadline.
///
/// The asymmetry is what sets the numbers. Waiting too long costs one HTTP
/// request every few seconds. Giving up too early tells the user their
/// generation failed while the provider is still running it and still charging
/// for it — and because the job is then marked terminal, `JobStore::unfinished`
/// stops returning it, so nothing ever polls that request id again and the
/// output they paid for is never collected. Every term here therefore errs
/// long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeoutPolicy {
    /// Patience before any scaling. Covers queue wait, which is unbounded and
    /// has nothing to do with how large the job is.
    pub base: Duration,
    /// Added per work unit — see [`TimeoutPolicy::work_units`].
    pub per_work_unit: Duration,
    /// Hard ceiling. A job still unanswered at this point is far more likely to
    /// be a request id we have lost track of than a generation still running.
    pub cap: Duration,
}

impl TimeoutPolicy {
    /// Hosted providers: someone else's datacenter, someone else's queue.
    pub const HOSTED: TimeoutPolicy = TimeoutPolicy {
        base: DEFAULT_TIMEOUT,
        per_work_unit: Duration::from_secs(30),
        cap: Duration::from_secs(3600),
    };

    /// Local inference on the user's own GPU.
    ///
    /// Ten times the hosted per-unit budget and a six-hour ceiling, because a
    /// consumer GPU is not a datacenter one and because nothing is being billed
    /// while it runs — so giving up early here is pure loss with no meter to
    /// stop. The cap exists only to stop a zombie loop outliving the job.
    pub const LOCAL: TimeoutPolicy = TimeoutPolicy {
        base: DEFAULT_TIMEOUT,
        per_work_unit: Duration::from_secs(300),
        cap: Duration::from_secs(6 * 3600),
    };

    /// The requested work, in units of "one 720p output second, or one 720p
    /// still".
    ///
    /// A second of video and a still image are not comparable amounts of
    /// compute and this does not pretend they are. The policy needs one
    /// property only: the number must never shrink when the user asks for
    /// something larger, so a 4K job can never be given less patience than a
    /// 480p one.
    ///
    /// Resolution scales as pixel count relative to 720p and is floored at
    /// 1.0 — 1080p is 2.25 units per second, 4K is 9. Dimensions are read in the
    /// same precedence [`CostModel::PerMegapixel`] uses (`megapixels` first,
    /// then `width` × `height`), so the deadline and the bill are derived from
    /// the same view of the job.
    ///
    /// [`CostModel::PerMegapixel`]: crate::cost::CostModel::PerMegapixel
    pub fn work_units(work: &Billable) -> f64 {
        let batch = work.batch.max(1) as f64;
        let res = Self::resolution_factor(work);
        match work.seconds {
            Some(s) if s > 0.0 => s * res * batch,
            // Stills. One image at 720p is one unit; the same image at 4K is
            // nine, which is the whole point of scaling by pixels rather than
            // counting outputs.
            _ => work.images.unwrap_or(1).max(1) as f64 * res * batch,
        }
    }

    /// Output pixels relative to a 720p frame, never below 1.0.
    fn resolution_factor(work: &Billable) -> f64 {
        let mp = work.megapixels.or_else(|| match (work.width, work.height) {
            (Some(w), Some(h)) => Some((w as f64 * h as f64) / 1_000_000.0),
            _ => None,
        });
        match mp {
            Some(mp) if mp > 0.0 => (mp / REFERENCE_MEGAPIXELS).max(1.0),
            // A job whose dimensions we cannot read is scored as 720p rather
            // than as no work at all, so it still earns a deadline that grows
            // with its duration.
            _ => 1.0,
        }
    }

    /// The deadline for a job whose requested work is `work`.
    ///
    /// `None` means the caller could not size the job — a real case, since
    /// [`JobSet::settings`] is an opaque `serde_json::Value` and nothing
    /// guarantees a persisted job carries readable dimensions. That job gets
    /// [`Self::base`], never zero: a zero deadline fails every job on its first
    /// poll tick.
    pub fn timeout(&self, work: Option<&Billable>) -> Duration {
        let units = work.map(Self::work_units).unwrap_or(0.0);
        // A non-finite or absurd `seconds` reaching here off a deserialised
        // settings blob would panic `Duration::mul_f64` on the runner thread and
        // take every other job's poll loop down with it.
        let units = if units.is_finite() {
            units.clamp(0.0, MAX_WORK_UNITS)
        } else {
            0.0
        };
        std::cmp::min(
            self.base.saturating_add(self.per_work_unit.mul_f64(units)),
            self.cap,
        )
    }
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        TimeoutPolicy::HOSTED
    }
}

/// Apply a poll result to a job, returning whether anything changed.
///
/// Kept separate from the polling loop so the state transition is testable on
/// its own — this is where a mistake would silently lose someone's output.
pub fn apply_poll(job: &mut JobSet, poll: PollResult, now: i64) -> bool {
    let changed = job.status != poll.status
        || job.results != poll.outputs
        || job.fail_reason != poll.fail_reason;
    if !changed {
        return false;
    }
    job.status = poll.status;
    job.fail_reason = poll.fail_reason;
    if !poll.outputs.is_empty() {
        // Preserve any local_path we already resolved; a later poll returning
        // the same URLs must not undo a completed download.
        for out in poll.outputs {
            match job.results.iter_mut().find(|r| r.url == out.url) {
                Some(existing) => existing.kind = out.kind,
                None => job.results.push(out),
            }
        }
    }
    if poll.actual_usd.is_some() {
        job.actual_usd = poll.actual_usd;
    }
    job.updated_at = now;
    true
}

/// Whether a terminal outcome should be refunded, for the UI to say so. Both
/// moderation refusals are refunded by every provider we route to.
pub fn refund_expected(status: JobStatus) -> bool {
    status.is_refunded()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn job(status: JobStatus) -> JobSet {
        JobSet {
            id: "j1".into(),
            model_id: "kling-3.0".into(),
            route_id: "fal:fal-ai/kling-video/v3/pro".into(),
            request_id: "req-1".into(),
            endpoint: String::new(),
            status,
            prompt: "a cat".into(),
            enhanced_prompt: None,
            enhancer_version: None,
            enhance_note: None,
            advisories: Vec::new(),
            preset_id: None,
            created_at: 0,
            updated_at: 0,
            results: vec![],
            estimated_usd: Some(0.67),
            actual_usd: None,
            fail_reason: None,
            settings: serde_json::json!({}),
            media: Vec::new(),
        }
    }

    fn out(url: &str) -> Output {
        Output {
            url: url.into(),
            kind: OutputKind::Video,
            local_path: None,
        }
    }

    #[derive(Default)]
    struct MemStore(Mutex<HashMap<String, JobSet>>);

    impl JobStore for MemStore {
        fn upsert(&self, j: &JobSet) -> Result<(), JobError> {
            self.0.lock().unwrap().insert(j.id.clone(), j.clone());
            Ok(())
        }
        fn get(&self, id: &str) -> Result<Option<JobSet>, JobError> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        fn all(&self) -> Result<Vec<JobSet>, JobError> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }
    }

    #[test]
    fn backoff_matches_the_documented_ladder() {
        let b = Backoff::default();
        let got: Vec<_> = (1..=6).map(|n| b.delay(n).map(|d| d.as_secs())).collect();
        assert_eq!(
            got,
            vec![Some(1), Some(2), Some(4), Some(8), None, None],
            "expected 1s,2s,4s,8s then exhausted after 5 attempts"
        );
    }

    #[test]
    fn backoff_is_capped() {
        let b = Backoff {
            max_attempts: 20,
            initial: Duration::from_secs(1),
            cap: Duration::from_secs(30),
        };
        assert_eq!(b.delay(19).unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn only_the_documented_codes_retry() {
        for code in [408, 429, 500, 502, 503, 504] {
            assert!(
                JobError::from_status(code, "").is_retryable(),
                "{code} should retry"
            );
        }
        // A 401 will still be a 401 in eight seconds.
        for code in [400, 401, 403, 404, 422] {
            assert!(
                !JobError::from_status(code, "").is_retryable(),
                "{code} must not retry"
            );
        }
    }

    #[test]
    fn apply_poll_reports_no_change_when_nothing_moved() {
        let mut j = job(JobStatus::Queued);
        let same = PollResult {
            status: JobStatus::Queued,
            outputs: vec![],
            fail_reason: None,
            actual_usd: None,
        };
        assert!(!apply_poll(&mut j, same, 100));
        assert_eq!(
            j.updated_at, 0,
            "an unchanged poll must not touch the clock"
        );
    }

    #[test]
    fn apply_poll_advances_status_and_collects_outputs() {
        let mut j = job(JobStatus::InProgress);
        let done = PollResult {
            status: JobStatus::Completed,
            outputs: vec![out("https://cdn/x.mp4")],
            fail_reason: None,
            actual_usd: Some(0.71),
        };
        assert!(apply_poll(&mut j, done, 100));
        assert_eq!(j.status, JobStatus::Completed);
        assert_eq!(j.results.len(), 1);
        assert_eq!(j.actual_usd, Some(0.71));
        assert_eq!(j.updated_at, 100);
    }

    #[test]
    fn a_repeat_poll_does_not_undo_a_finished_download() {
        // The bug this guards: a provider re-reports the same URL after we have
        // already saved the file, and a naive overwrite loses local_path —
        // making a settled job look unsettled forever.
        let mut j = job(JobStatus::Completed);
        j.results = vec![Output {
            url: "https://cdn/x.mp4".into(),
            kind: OutputKind::Video,
            local_path: Some("/lib/x.mp4".into()),
        }];
        let repeat = PollResult {
            status: JobStatus::Completed,
            outputs: vec![out("https://cdn/x.mp4")],
            fail_reason: Some("noop".into()),
            actual_usd: None,
        };
        apply_poll(&mut j, repeat, 200);
        assert_eq!(j.results.len(), 1, "must not duplicate the output");
        assert_eq!(j.results[0].local_path.as_deref(), Some("/lib/x.mp4"));
    }

    #[test]
    fn completed_is_not_settled_until_the_bytes_are_local() {
        // Provider URLs expire — Higgsfield's own API deletes after 7 days — so
        // "completed" alone is not good enough to stop caring about a job.
        let mut j = job(JobStatus::Completed);
        j.results = vec![out("https://cdn/x.mp4")];
        assert!(j.is_terminal());
        assert!(!j.is_settled());

        j.results[0].local_path = Some("/lib/x.mp4".into());
        assert!(j.is_settled());
    }

    #[test]
    fn a_failed_job_is_settled_with_no_outputs() {
        let j = job(JobStatus::Failed);
        assert!(j.is_settled(), "nothing to download, so nothing pending");
    }

    #[test]
    fn unfinished_is_what_gets_reattached_on_launch() {
        let store = MemStore::default();
        let mut running = job(JobStatus::InProgress);
        running.id = "running".into();

        let mut done = job(JobStatus::Completed);
        done.id = "done".into();
        done.results = vec![Output {
            url: "u".into(),
            kind: OutputKind::Video,
            local_path: Some("/lib/u.mp4".into()),
        }];

        let mut downloaded_nothing = job(JobStatus::Completed);
        downloaded_nothing.id = "pending-download".into();
        downloaded_nothing.results = vec![out("v")];

        for j in [&running, &done, &downloaded_nothing] {
            store.upsert(j).unwrap();
        }

        let mut ids: Vec<_> = store
            .unfinished()
            .unwrap()
            .into_iter()
            .map(|j| j.id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["pending-download", "running"]);
    }

    #[test]
    fn moderation_refusals_are_flagged_for_refund() {
        assert!(refund_expected(JobStatus::Nsfw));
        assert!(refund_expected(JobStatus::Failed));
        assert!(!refund_expected(JobStatus::Completed));
        assert!(!refund_expected(JobStatus::Canceled));
    }

    #[test]
    fn a_4k_job_gets_more_patience_than_a_720p_one_of_the_same_length() {
        // The bug: a flat 600s deadline for every job. A 10s 4K generation is
        // nine times the pixels of a 10s 720p one and routinely outruns ten
        // minutes, so the flat policy reported "gave up" for a generation the
        // provider was still running and still charging for.
        let p = TimeoutPolicy::HOSTED;
        let sd = p.timeout(Some(&Billable::video(10.0, 1280, 720)));
        let uhd = p.timeout(Some(&Billable::video(10.0, 3840, 2160)));
        assert!(uhd > sd, "4K {uhd:?} should outlast 720p {sd:?}");
        assert!(
            uhd > DEFAULT_TIMEOUT,
            "4K must outlast the old flat 600s, got {uhd:?}"
        );
    }

    #[test]
    fn resolution_scales_the_same_way_the_price_does() {
        // 1080p is 2.25x the pixels of 720p — the same ratio `CostModel::
        // PerToken` charges. Deadline and bill must describe the same job.
        let hd = TimeoutPolicy::work_units(&Billable::video(1.0, 1920, 1080));
        assert!((hd - 2.25).abs() < 1e-9, "got {hd}");
        let uhd = TimeoutPolicy::work_units(&Billable::video(1.0, 3840, 2160));
        assert!((uhd - 9.0).abs() < 1e-9, "got {uhd}");
    }

    #[test]
    fn asking_for_more_never_shortens_the_deadline() {
        // The single property the policy actually promises. Anything else here
        // is a patience budget, not a prediction.
        let p = TimeoutPolicy::HOSTED;
        let base = p.timeout(Some(&Billable::video(5.0, 1280, 720)));
        for bigger in [
            Billable::video(6.0, 1280, 720),
            Billable::video(5.0, 1920, 1080),
            Billable {
                batch: 4,
                ..Billable::video(5.0, 1280, 720)
            },
        ] {
            assert!(
                p.timeout(Some(&bigger)) >= base,
                "{bigger:?} got less patience than the smaller job"
            );
        }
    }

    #[test]
    fn a_sub_720p_clip_is_never_scored_below_one_unit() {
        // Floored, so a 480p job cannot be handed a shorter deadline than the
        // per-second budget it needs just to finish.
        let low = TimeoutPolicy::work_units(&Billable::video(4.0, 854, 480));
        assert!((low - 4.0).abs() < 1e-9, "got {low}");
    }

    #[test]
    fn unmeasurable_work_gets_the_base_budget_not_zero() {
        // `JobSet::settings` is an opaque blob that predates this policy, so
        // "we could not size this job" is a real state. A zero deadline would
        // fail it on its first poll tick.
        let p = TimeoutPolicy::HOSTED;
        assert_eq!(p.timeout(None), p.base);
        assert!(p.timeout(Some(&Billable::default())) > p.base);
    }

    #[test]
    fn a_nonsense_duration_cannot_panic_the_poll_loop() {
        // `Duration::mul_f64` panics on NaN and on overflow. Reaching that from
        // a deserialised settings blob would kill the runner thread and with it
        // every other job's poll loop, so it is clamped rather than trusted.
        let p = TimeoutPolicy::HOSTED;
        for secs in [f64::NAN, f64::INFINITY, -1.0, 1e300] {
            let b = Billable {
                seconds: Some(secs),
                ..Billable::video(1.0, 1280, 720)
            };
            let t = p.timeout(Some(&b));
            assert!(t >= p.base && t <= p.cap, "{secs} produced {t:?}");
        }
    }

    #[test]
    fn the_cap_is_the_last_word() {
        let p = TimeoutPolicy::HOSTED;
        let huge = Billable {
            batch: 500,
            ..Billable::video(60.0, 3840, 2160)
        };
        assert_eq!(p.timeout(Some(&huge)), p.cap);
    }

    #[test]
    fn local_inference_is_given_more_patience_than_a_datacenter() {
        // Nothing is being billed while a local GPU works, so giving up early
        // is pure loss with no meter to stop.
        let work = Billable::video(5.0, 1280, 720);
        assert!(
            TimeoutPolicy::LOCAL.timeout(Some(&work)) > TimeoutPolicy::HOSTED.timeout(Some(&work))
        );
    }

    #[test]
    fn store_round_trips_a_job() {
        let store = MemStore::default();
        let j = job(JobStatus::Queued);
        store.upsert(&j).unwrap();
        assert_eq!(store.get("j1").unwrap().unwrap(), j);
        assert!(store.get("nope").unwrap().is_none());
    }
}
