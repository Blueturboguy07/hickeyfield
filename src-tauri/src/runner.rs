//! The job runner: owns the poll loops and streams updates to the UI.
//!
//! One task per running job, on tokio's blocking pool. That sounds profligate
//! until you notice providers cap concurrency hard — a new fal account allows
//! *two* simultaneous requests — so the ceiling is theirs, not ours.
//!
//! The runner is why a generation survives the window closing: it lives in the
//! Rust process, writes every transition to SQLite, and is restarted from the
//! store on launch rather than from anything the webview remembers.

use crate::library::Library;
use halation_core::engine::{
    apply_poll, Backoff, JobError, JobSet, JobStore, ProviderClient, DEFAULT_TIMEOUT,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Builds a client for a route id (`provider:slug`). Returns `None` when the
/// user holds no key for that provider — a normal state, not an error.
pub type ClientFactory = Arc<dyn Fn(&str) -> Option<Arc<dyn ProviderClient>> + Send + Sync>;

/// Called on every observed change, so the UI can stream rather than poll.
pub type OnUpdate = Arc<dyn Fn(&JobSet) + Send + Sync>;

pub struct Runner {
    store: Arc<dyn JobStore>,
    clients: ClientFactory,
    on_update: OnUpdate,
    /// Outputs are downloaded here as soon as a job completes. Provider URLs
    /// expire — Higgsfield's own API deletes after seven days — so a result
    /// that only exists as a link is a result the user will eventually lose.
    library: Arc<Library>,
    /// Guards against two loops polling the same job, which would double the
    /// request rate against a provider that is already rate-limiting us.
    watching: Arc<Mutex<HashSet<String>>>,
    /// Jobs the user has deleted while a poll loop was still running.
    ///
    /// Without this, `delete_job` removed the row and the very next poll tick
    /// wrote it straight back — a destructive action that appeared to succeed
    /// and silently reverted, which is worse than one that visibly fails.
    abandoned: Arc<Mutex<HashSet<String>>>,
}

impl Runner {
    pub fn new(
        store: Arc<dyn JobStore>,
        clients: ClientFactory,
        on_update: OnUpdate,
        library: Arc<Library>,
    ) -> Self {
        Runner {
            store,
            clients,
            on_update,
            library,
            watching: Arc::new(Mutex::new(HashSet::new())),
            abandoned: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Restart polling for everything left unfinished by the last session.
    /// Returns how many were resumed.
    pub fn resume_all(&self) -> Result<usize, JobError> {
        let pending = self.store.unfinished()?;
        let n = pending.len();
        for job in pending {
            self.watch(job);
        }
        Ok(n)
    }

    /// Begin polling a job. Idempotent — calling twice starts one loop.
    pub fn watch(&self, job: JobSet) {
        {
            let mut watching = self.watching.lock().unwrap();
            if !watching.insert(job.id.clone()) {
                return;
            }
        }

        let store = Arc::clone(&self.store);
        let clients = Arc::clone(&self.clients);
        let on_update = Arc::clone(&self.on_update);
        let watching = Arc::clone(&self.watching);
        let abandoned = Arc::clone(&self.abandoned);
        let library = Arc::clone(&self.library);

        tauri::async_runtime::spawn_blocking(move || {
            let id = job.id.clone();
            let finished = poll_until_terminal(
                job,
                Arc::clone(&store),
                clients,
                Arc::clone(&on_update),
                Arc::clone(&abandoned),
            );
            // Check once more before writing the outputs: a job deleted during
            // its final poll would otherwise reappear complete.
            if !abandoned.lock().unwrap().contains(&id) {
                download_outputs(finished, store, on_update, library);
            }
            watching.lock().unwrap().remove(&id);
            abandoned.lock().unwrap().remove(&id);
        });
    }

    pub fn is_watching(&self, id: &str) -> bool {
        self.watching.lock().unwrap().contains(id)
    }

    /// Stop caring about a job, so a running poll loop cannot resurrect it.
    ///
    /// Marks the tombstone *before* the caller deletes the row: the reverse
    /// order leaves a window in which a poll tick re-inserts it.
    pub fn forget(&self, id: &str) {
        self.abandoned.lock().unwrap().insert(id.to_string());
    }

    pub fn cancel(&self, id: &str) -> Result<(), JobError> {
        let Some(job) = self.store.get(id)? else {
            return Err(JobError::Permanent(format!("no such job: {id}")));
        };
        let Some(client) = (self.clients)(&job.route_id) else {
            return Err(JobError::Permanent("no client for this route".into()));
        };
        // route_id is `provider:slug`; fal scopes its queue under the slug, so
        // strip the provider prefix rather than sending the whole id.
        let app = if job.endpoint.is_empty() {
            job.route_id.split_once(':').map(|(_, s)| s).unwrap_or("")
        } else {
            job.endpoint.as_str()
        };
        client.cancel(app, &job.request_id)
    }
}

/// Pull every output onto disk. A completed job whose bytes are still only on
/// the provider's CDN is not finished — see `JobSet::is_settled`.
///
/// A download failure is recorded on the job but never flips it back to
/// failed: the generation succeeded and the user was charged for it, so the
/// honest state is "done, but we could not save it".
fn download_outputs(
    mut job: JobSet,
    store: Arc<dyn JobStore>,
    on_update: OnUpdate,
    library: Arc<Library>,
) {
    if job.status.phase() != halation_core::Phase::Completed || job.results.is_empty() {
        return;
    }
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
    else {
        return;
    };

    let mut changed = false;
    let prompt = job.prompt.clone();
    let created = job.created_at;
    for out in job.results.iter_mut().filter(|o| o.local_path.is_none()) {
        match library.fetch(out, &prompt, created, &client) {
            Ok(path) => {
                out.local_path = Some(path.display().to_string());
                changed = true;
            }
            Err(e) => tracing::warn!("could not save {}: {e}", out.url),
        }
    }

    if changed {
        job.updated_at = now_secs();
        let _ = store.upsert(&job);
        on_update(&job);
    }
}

/// The loop itself. Blocking, so it runs on the blocking pool. Returns the
/// job in its final state so the caller can fetch its outputs.
fn poll_until_terminal(
    mut job: JobSet,
    store: Arc<dyn JobStore>,
    clients: ClientFactory,
    on_update: OnUpdate,
    abandoned: Arc<Mutex<HashSet<String>>>,
) -> JobSet {
    let is_abandoned = {
        let id = job.id.clone();
        let abandoned = Arc::clone(&abandoned);
        move || abandoned.lock().unwrap().contains(&id)
    };
    let Some(client) = clients(&job.route_id) else {
        job.status = halation_core::JobStatus::Failed;
        job.fail_reason = Some(format!(
            "no credentials for {} — add a key in Settings",
            job.route_id.split(':').next().unwrap_or("this provider")
        ));
        job.updated_at = now_secs();
        let _ = store.upsert(&job);
        on_update(&job);
        return job;
    };

    // Computed once: fal's status endpoint is scoped under the originating app,
    // and the bare form answers 405.
    // What submit actually called. Pre-v3 rows have none, and they predate
    // suffixing, so the route slug is right for them.
    let app_path = if job.endpoint.is_empty() {
        job.route_id
            .split_once(':')
            .map(|(_, slug)| slug.to_string())
            .unwrap_or_default()
    } else {
        job.endpoint.clone()
    };

    let started = Instant::now();
    let interval = client.poll_interval();
    let backoff = Backoff::default();
    let mut consecutive_errors: u32 = 0;

    loop {
        // Checked at the top of every tick rather than only at the end, so a
        // long-running generation stops costing us polls the moment the user
        // deletes it.
        if is_abandoned() {
            return job;
        }
        if started.elapsed() > DEFAULT_TIMEOUT {
            job.status = halation_core::JobStatus::Failed;
            job.fail_reason = Some(format!(
                "gave up after {} minutes with no result from the provider",
                DEFAULT_TIMEOUT.as_secs() / 60
            ));
            job.updated_at = now_secs();
            let _ = store.upsert(&job);
            on_update(&job);
            return job;
        }

        match client.poll(&app_path, &job.request_id) {
            Ok(result) => {
                consecutive_errors = 0;
                if apply_poll(&mut job, result, now_secs()) && !is_abandoned() {
                    let _ = store.upsert(&job);
                    on_update(&job);
                }
                if job.is_terminal() {
                    return job;
                }
                std::thread::sleep(interval);
            }
            Err(e) if e.is_retryable() => {
                consecutive_errors += 1;
                match backoff.delay(consecutive_errors) {
                    Some(d) => std::thread::sleep(d),
                    None => {
                        // Exhausted. The generation may well have succeeded on
                        // the provider's side, so say that rather than claiming
                        // it failed outright.
                        job.status = halation_core::JobStatus::Failed;
                        job.fail_reason = Some(format!(
                            "lost contact with the provider after {} attempts: {e}",
                            backoff.max_attempts
                        ));
                        job.updated_at = now_secs();
                        let _ = store.upsert(&job);
                        on_update(&job);
                        return job;
                    }
                }
            }
            Err(e) => {
                job.status = halation_core::JobStatus::Failed;
                job.fail_reason = Some(e.to_string());
                job.updated_at = now_secs();
                let _ = store.upsert(&job);
                on_update(&job);
                return job;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halation_core::engine::{Output, OutputKind, PollResult};
    use halation_core::JobStatus;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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

    /// Returns a scripted sequence of poll results, then repeats the last.
    struct ScriptedClient {
        script: Mutex<Vec<Result<PollResult, JobError>>>,
        calls: AtomicUsize,
    }

    impl ScriptedClient {
        fn new(script: Vec<Result<PollResult, JobError>>) -> Self {
            ScriptedClient {
                script: Mutex::new(script),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl ProviderClient for ScriptedClient {
        fn submit(
            &self,
            _m: &str,
            _b: &serde_json::Value,
        ) -> Result<halation_core::engine::Submission, JobError> {
            Ok(halation_core::engine::Submission {
                request_id: "req".into(),
                status_url: None,
            })
        }
        fn poll(&self, _model_id: &str, _id: &str) -> Result<PollResult, JobError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let s = self.script.lock().unwrap();
            s.get(n)
                .cloned()
                .unwrap_or_else(|| s.last().unwrap().clone())
        }
        fn poll_interval(&self) -> Duration {
            Duration::from_millis(1)
        }
    }

    fn job(id: &str) -> JobSet {
        JobSet {
            id: id.into(),
            model_id: "m".into(),
            route_id: "fal:m".into(),
            request_id: "req".into(),
            endpoint: String::new(),
            status: JobStatus::Queued,
            prompt: "p".into(),
            enhanced_prompt: None,
            enhancer_version: None,
            enhance_note: None,
            advisories: Vec::new(),
            preset_id: None,
            created_at: 0,
            updated_at: 0,
            results: vec![],
            estimated_usd: None,
            actual_usd: None,
            fail_reason: None,
            settings: serde_json::json!({}),
            media: Vec::new(),
        }
    }

    fn ok(status: JobStatus) -> Result<PollResult, JobError> {
        Ok(PollResult {
            status,
            outputs: vec![],
            fail_reason: None,
            actual_usd: None,
        })
    }

    fn run(script: Vec<Result<PollResult, JobError>>) -> (Arc<MemStore>, usize) {
        let store = Arc::new(MemStore::default());
        let client: Arc<dyn ProviderClient> = Arc::new(ScriptedClient::new(script));
        let updates = Arc::new(AtomicUsize::new(0));
        let u = Arc::clone(&updates);

        poll_until_terminal(
            job("j"),
            Arc::clone(&store) as Arc<dyn JobStore>,
            Arc::new(move |_| Some(Arc::clone(&client))),
            Arc::new(move |_| {
                u.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(Mutex::new(HashSet::new())),
        );
        let n = updates.load(Ordering::SeqCst);
        (store, n)
    }

    #[test]
    fn runs_to_completion_and_persists_each_transition() {
        let (store, updates) = run(vec![
            ok(JobStatus::Queued),
            ok(JobStatus::InProgress),
            Ok(PollResult {
                status: JobStatus::Completed,
                outputs: vec![Output {
                    url: "https://a/x.mp4".into(),
                    kind: OutputKind::Video,
                    local_path: None,
                }],
                fail_reason: None,
                actual_usd: Some(0.5),
            }),
        ]);
        let j = store.get("j").unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Completed);
        assert_eq!(j.results.len(), 1);
        assert_eq!(j.actual_usd, Some(0.5));
        // Queued->Queued is not a change, so only two transitions are emitted.
        assert_eq!(updates, 2, "should emit only on real changes");
    }

    #[test]
    fn a_transient_error_is_retried_rather_than_failing_the_job() {
        let (store, _) = run(vec![
            Err(JobError::Transient("502".into())),
            Err(JobError::Transient("502".into())),
            ok(JobStatus::Completed),
        ]);
        assert_eq!(
            store.get("j").unwrap().unwrap().status,
            JobStatus::Completed
        );
    }

    #[test]
    fn a_permanent_error_fails_immediately_with_the_reason() {
        let (store, _) = run(vec![Err(JobError::Permanent("401 bad key".into()))]);
        let j = store.get("j").unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        assert!(j.fail_reason.unwrap().contains("401"));
    }

    #[test]
    fn exhausted_retries_say_contact_was_lost_not_that_it_failed() {
        // The provider may well have produced the output; claiming a hard
        // failure would be a lie the user cannot check.
        let (store, _) = run(vec![Err(JobError::Transient("timeout".into()))]);
        let j = store.get("j").unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        let reason = j.fail_reason.unwrap();
        assert!(reason.contains("lost contact"), "got: {reason}");
    }

    #[test]
    fn an_abandoned_job_is_never_written_back() {
        // The delete bug: the row was removed, the next poll tick upserted it,
        // and the job reappeared seconds later. A destructive action that
        // silently reverts is worse than one that visibly fails.
        let store = Arc::new(MemStore::default());
        let client: Arc<dyn ProviderClient> = Arc::new(ScriptedClient::new(vec![Ok(PollResult {
            status: halation_core::JobStatus::InProgress,
            outputs: vec![],
            fail_reason: None,
            actual_usd: None,
        })]));
        let tomb = Arc::new(Mutex::new(HashSet::new()));
        tomb.lock().unwrap().insert("j".to_string());

        let out = poll_until_terminal(
            job("j"),
            Arc::clone(&store) as Arc<dyn JobStore>,
            Arc::new(move |_| Some(Arc::clone(&client))),
            Arc::new(|_| {}),
            Arc::clone(&tomb),
        );
        // Returned immediately without touching the store.
        assert!(
            store.all().unwrap().is_empty(),
            "an abandoned job must not be persisted"
        );
        assert_eq!(out.id, "j");
    }

    #[test]
    fn a_live_job_is_still_written() {
        // The other half: the tombstone must not suppress ordinary progress.
        let (store, updates) = run(vec![Ok(PollResult {
            status: halation_core::JobStatus::Completed,
            outputs: vec![],
            fail_reason: None,
            actual_usd: None,
        })]);
        assert!(!store.all().unwrap().is_empty());
        assert!(updates > 0);
    }

    #[test]
    fn a_missing_credential_is_explained_not_swallowed() {
        let store = Arc::new(MemStore::default());
        poll_until_terminal(
            job("j"),
            Arc::clone(&store) as Arc<dyn JobStore>,
            Arc::new(|_| None),
            Arc::new(|_| {}),
            Arc::new(Mutex::new(HashSet::new())),
        );
        let j = store.get("j").unwrap().unwrap();
        assert_eq!(j.status, JobStatus::Failed);
        let reason = j.fail_reason.unwrap();
        assert!(reason.contains("fal"), "should name the provider: {reason}");
        assert!(reason.contains("Settings"), "should say where to fix it");
    }

    #[test]
    fn nsfw_is_terminal_and_keeps_its_own_status() {
        // Collapsing this into a generic failure would hide why it happened and
        // that the provider refunds it.
        let (store, _) = run(vec![ok(JobStatus::Nsfw)]);
        assert_eq!(store.get("j").unwrap().unwrap().status, JobStatus::Nsfw);
    }

    #[test]
    fn intermediate_stages_do_not_terminate_the_loop() {
        let (store, updates) = run(vec![
            ok(JobStatus::Dna),
            ok(JobStatus::Script),
            ok(JobStatus::Visuals),
            ok(JobStatus::Completed),
        ]);
        assert_eq!(
            store.get("j").unwrap().unwrap().status,
            JobStatus::Completed
        );
        assert_eq!(updates, 4);
    }
}
