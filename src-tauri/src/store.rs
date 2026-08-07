//! SQLite-backed job persistence.
//!
//! Implements `hickeyfield_core::engine::JobStore`. The core defines the trait and
//! stays database-free so its tests run in milliseconds; the concrete storage
//! lives here, where it is allowed to be slow and platform-aware.
//!
//! Everything is written on submit. That is what lets a generation survive the
//! window closing and reappear on relaunch — the property that separates this
//! from the web build, which could only have offered it by putting the user's
//! key on a server.

use hickeyfield_core::engine::{JobError, JobSet, JobStore};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

fn map_err(e: impl std::fmt::Display) -> JobError {
    // Storage failures are not the provider's fault and retrying the *network*
    // will not help, so they are permanent from the engine's point of view.
    JobError::Permanent(format!("store: {e}"))
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, JobError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(map_err)?;
        }
        let conn = Connection::open(path).map_err(map_err)?;
        Self::from_conn(conn)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self, JobError> {
        Self::from_conn(Connection::open_in_memory().map_err(map_err)?)
    }

    fn from_conn(conn: Connection) -> Result<Self, JobError> {
        // WAL so a long-running poll write never blocks a UI read.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(map_err)?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(map_err)?;
        let store = SqliteStore {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Schema version lives in `user_version`. Migrations are append-only:
    /// never renumber, never edit a shipped step.
    fn migrate(&self) -> Result<(), JobError> {
        let conn = self.conn.lock().unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(map_err)?;

        if version < 1 {
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS job_sets (
                    id              TEXT PRIMARY KEY,
                    model_id        TEXT NOT NULL,
                    route_id        TEXT NOT NULL,
                    request_id      TEXT NOT NULL,
                    status          TEXT NOT NULL,
                    prompt          TEXT NOT NULL,
                    enhanced_prompt TEXT,
                    preset_id       TEXT,
                    created_at      INTEGER NOT NULL,
                    updated_at      INTEGER NOT NULL,
                    results         TEXT NOT NULL DEFAULT '[]',
                    estimated_usd   REAL,
                    actual_usd      REAL,
                    fail_reason     TEXT,
                    settings        TEXT NOT NULL DEFAULT '{}'
                );
                -- The library lists newest-first, and reattach-on-launch scans
                -- by status; both would table-scan without these.
                CREATE INDEX IF NOT EXISTS idx_jobs_created ON job_sets(created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_jobs_status  ON job_sets(status);
                PRAGMA user_version = 1;
                "#,
            )
            .map_err(map_err)?;
        }

        if version < 2 {
            // Attached inputs. Without these Rerun restored an image-to-video
            // job as text-to-video: a different generation, at the same price,
            // that the user did not ask for.
            conn.execute_batch(
                r#"
                ALTER TABLE job_sets ADD COLUMN media TEXT NOT NULL DEFAULT '[]';
                PRAGMA user_version = 2;
                "#,
            )
            .map_err(map_err)?;
        }

        if version < 3 {
            // The endpoint actually submitted to. Rebuilding it from route_id
            // gave the family root, and fal answers 405 there — the job ran and
            // could not be read back.
            conn.execute_batch(
                r#"
                ALTER TABLE job_sets ADD COLUMN endpoint TEXT NOT NULL DEFAULT '';
                PRAGMA user_version = 3;
                "#,
            )
            .map_err(map_err)?;
        }

        if version < 4 {
            // What rewrote the prompt, and why it did not. Without these a
            // generation cannot be reproduced — the same prompt through a
            // different corpus is a different generation.
            conn.execute_batch(
                r#"
                ALTER TABLE job_sets ADD COLUMN enhancer_version TEXT;
                ALTER TABLE job_sets ADD COLUMN enhance_note TEXT;
                PRAGMA user_version = 4;
                "#,
            )
            .map_err(map_err)?;
        }

        if version < 5 {
            // Advisories: things the user should know that did not fail the
            // job. A setting the endpoint silently discards is the case this
            // exists for.
            conn.execute_batch(
                r#"
                ALTER TABLE job_sets ADD COLUMN advisories TEXT NOT NULL DEFAULT '[]';
                PRAGMA user_version = 5;
                "#,
            )
            .map_err(map_err)?;
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), JobError> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM job_sets WHERE id = ?1", params![id])
            .map(|_| ())
            .map_err(map_err)
    }

    #[cfg(test)]
    pub fn count(&self) -> Result<i64, JobError> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM job_sets", [], |r| r.get(0))
            .map_err(map_err)
    }
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobSet> {
    let status: String = row.get("status")?;
    let results: String = row.get("results")?;
    let settings: String = row.get("settings")?;
    Ok(JobSet {
        id: row.get("id")?,
        model_id: row.get("model_id")?,
        route_id: row.get("route_id")?,
        request_id: row.get("request_id")?,
        // A status we cannot parse must not drop the row — the job still
        // exists and the user still has a right to see it.
        status: serde_json::from_str(&format!("\"{status}\""))
            .unwrap_or(hickeyfield_core::JobStatus::InProgress),
        prompt: row.get("prompt")?,
        enhanced_prompt: row.get("enhanced_prompt")?,
        enhancer_version: row.get("enhancer_version").unwrap_or_default(),
        enhance_note: row.get("enhance_note").unwrap_or_default(),
        advisories: row
            .get::<_, String>("advisories")
            .ok()
            .and_then(|a| serde_json::from_str(&a).ok())
            .unwrap_or_default(),
        preset_id: row.get("preset_id")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        results: serde_json::from_str(&results).unwrap_or_default(),
        estimated_usd: row.get("estimated_usd")?,
        actual_usd: row.get("actual_usd")?,
        fail_reason: row.get("fail_reason")?,
        settings: serde_json::from_str(&settings).unwrap_or(serde_json::Value::Null),
        // Pre-v3 rows have '', which the runner reads as "fall back to the
        // route slug" — correct for them, since they predate suffixing.
        endpoint: row.get("endpoint").unwrap_or_default(),
        // Rows written before schema v2 carry '[]'; a malformed blob degrades
        // to "no attachments" rather than dropping the whole job.
        media: row
            .get::<_, String>("media")
            .ok()
            .and_then(|m| serde_json::from_str(&m).ok())
            .unwrap_or_default(),
    })
}

impl JobStore for SqliteStore {
    fn upsert(&self, j: &JobSet) -> Result<(), JobError> {
        let status = serde_json::to_string(&j.status)
            .map_err(map_err)?
            .trim_matches('"')
            .to_string();
        let results = serde_json::to_string(&j.results).map_err(map_err)?;
        let settings = serde_json::to_string(&j.settings).map_err(map_err)?;
        let media = serde_json::to_string(&j.media).map_err(map_err)?;
        let endpoint = j.endpoint.clone();
        let advisories = serde_json::to_string(&j.advisories).map_err(map_err)?;

        self.conn
            .lock()
            .unwrap()
            .execute(
                r#"
                INSERT INTO job_sets (
                    id, model_id, route_id, request_id, status, prompt,
                    enhanced_prompt, preset_id, created_at, updated_at,
                    results, estimated_usd, actual_usd, fail_reason, settings,
                    media, endpoint, enhancer_version, enhance_note, advisories
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
                ON CONFLICT(id) DO UPDATE SET
                    status          = excluded.status,
                    enhanced_prompt = excluded.enhanced_prompt,
                    enhancer_version = excluded.enhancer_version,
                    enhance_note    = excluded.enhance_note,
                    advisories      = excluded.advisories,
                    updated_at      = excluded.updated_at,
                    results         = excluded.results,
                    actual_usd      = excluded.actual_usd,
                    fail_reason     = excluded.fail_reason,
                    endpoint        = excluded.endpoint
                "#,
                params![
                    j.id,
                    j.model_id,
                    j.route_id,
                    j.request_id,
                    status,
                    j.prompt,
                    j.enhanced_prompt,
                    j.preset_id,
                    j.created_at,
                    j.updated_at,
                    results,
                    j.estimated_usd,
                    j.actual_usd,
                    j.fail_reason,
                    settings,
                    media,
                    endpoint,
                    j.enhancer_version,
                    j.enhance_note,
                    advisories,
                ],
            )
            .map(|_| ())
            .map_err(map_err)
    }

    fn get(&self, id: &str) -> Result<Option<JobSet>, JobError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT * FROM job_sets WHERE id = ?1",
                params![id],
                row_to_job,
            )
            .optional()
            .map_err(map_err)
    }

    fn all(&self) -> Result<Vec<JobSet>, JobError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT * FROM job_sets ORDER BY created_at DESC")
            .map_err(map_err)?;
        let rows = stmt.query_map([], row_to_job).map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickeyfield_core::engine::{Output, OutputKind};
    use hickeyfield_core::JobStatus;

    fn job(id: &str, status: JobStatus) -> JobSet {
        JobSet {
            id: id.into(),
            model_id: "kling-3.0".into(),
            route_id: "fal:kling/v3/pro".into(),
            request_id: "req".into(),
            endpoint: String::new(),
            status,
            prompt: "a cat wearing a hat".into(),
            enhanced_prompt: None,
            enhancer_version: None,
            enhance_note: None,
            advisories: Vec::new(),
            preset_id: None,
            created_at: 1,
            updated_at: 1,
            results: vec![],
            estimated_usd: Some(0.67),
            actual_usd: None,
            fail_reason: None,
            settings: serde_json::json!({"duration": 8}),
            media: Vec::new(),
        }
    }

    #[test]
    fn round_trips_every_field() {
        let s = SqliteStore::in_memory().unwrap();
        let mut j = job("a", JobStatus::Completed);
        j.enhanced_prompt = Some("a tabby cat in a red fedora".into());
        j.results = vec![Output {
            url: "https://cdn/x.mp4".into(),
            kind: OutputKind::Video,
            local_path: Some("/lib/x.mp4".into()),
        }];
        j.actual_usd = Some(0.71);
        s.upsert(&j).unwrap();
        assert_eq!(s.get("a").unwrap().unwrap(), j);
    }

    #[test]
    fn upsert_updates_rather_than_duplicating() {
        let s = SqliteStore::in_memory().unwrap();
        let mut j = job("a", JobStatus::Queued);
        s.upsert(&j).unwrap();
        j.status = JobStatus::Completed;
        j.updated_at = 99;
        s.upsert(&j).unwrap();

        assert_eq!(s.count().unwrap(), 1);
        let got = s.get("a").unwrap().unwrap();
        assert_eq!(got.status, JobStatus::Completed);
        assert_eq!(got.updated_at, 99);
    }

    #[test]
    fn survives_reopening_the_same_file() {
        // The whole point of the store: quit mid-generation, come back, it is
        // still there and still known to be unfinished.
        let dir = std::env::temp_dir().join(format!("hickeyfield-test-{}", std::process::id()));
        let path = dir.join("jobs.sqlite");
        let _ = std::fs::remove_dir_all(&dir);

        {
            let s = SqliteStore::open(&path).unwrap();
            s.upsert(&job("running", JobStatus::InProgress)).unwrap();
        }
        {
            let s = SqliteStore::open(&path).unwrap();
            let pending = s.unfinished().unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].id, "running");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unfinished_excludes_fully_settled_work() {
        let s = SqliteStore::in_memory().unwrap();
        s.upsert(&job("running", JobStatus::InProgress)).unwrap();
        s.upsert(&job("failed", JobStatus::Failed)).unwrap();

        let mut downloaded = job("downloaded", JobStatus::Completed);
        downloaded.results = vec![Output {
            url: "u".into(),
            kind: OutputKind::Video,
            local_path: Some("/lib/u.mp4".into()),
        }];
        s.upsert(&downloaded).unwrap();

        let mut awaiting_download = job("awaiting", JobStatus::Completed);
        awaiting_download.results = vec![Output {
            url: "v".into(),
            kind: OutputKind::Video,
            local_path: None,
        }];
        s.upsert(&awaiting_download).unwrap();

        let mut ids: Vec<_> = s.unfinished().unwrap().into_iter().map(|j| j.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["awaiting", "running"]);
    }

    #[test]
    fn listing_is_newest_first() {
        let s = SqliteStore::in_memory().unwrap();
        for (id, t) in [("old", 1), ("new", 3), ("mid", 2)] {
            let mut j = job(id, JobStatus::Completed);
            j.created_at = t;
            s.upsert(&j).unwrap();
        }
        let ids: Vec<_> = s.all().unwrap().into_iter().map(|j| j.id).collect();
        assert_eq!(ids, vec!["new", "mid", "old"]);
    }

    #[test]
    fn the_enhancer_pin_survives_a_round_trip() {
        // Without it a generation cannot be reproduced: the same prompt through
        // a different corpus or a different local model is a different result,
        // and the recipe would claim otherwise.
        let s = SqliteStore::in_memory().unwrap();
        let mut j = job("a", JobStatus::Completed);
        j.enhanced_prompt = Some("a rewritten scene".into());
        j.enhancer_version = Some("enhancer.v1/video+ollama/qwen2.5:7b".into());
        j.enhance_note = None;
        s.upsert(&j).unwrap();
        let back = s.get("a").unwrap().unwrap();
        assert_eq!(back.enhanced_prompt.as_deref(), Some("a rewritten scene"));
        assert_eq!(
            back.enhancer_version.as_deref(),
            Some("enhancer.v1/video+ollama/qwen2.5:7b")
        );
    }

    #[test]
    fn a_job_that_was_not_rewritten_stores_no_pin() {
        // Never a placeholder — see the field's doc comment.
        let s = SqliteStore::in_memory().unwrap();
        let mut j = job("b", JobStatus::Completed);
        j.enhance_note = Some("enhancement is off for this model".into());
        s.upsert(&j).unwrap();
        let back = s.get("b").unwrap().unwrap();
        assert!(back.enhancer_version.is_none());
        assert!(back.enhance_note.is_some());
    }

    #[test]
    fn attached_media_survives_a_round_trip() {
        // Without this, Rerun restored an image-to-video job as text-to-video:
        // a different generation, at the same price, that the user did not ask
        // for.
        use hickeyfield_core::{MediaRef, MediaRole};
        let s = SqliteStore::in_memory().unwrap();
        let mut j = job("a", JobStatus::Queued);
        j.media = vec![
            MediaRef::url(MediaRole::Start, "https://cdn/first.png"),
            MediaRef::url(MediaRole::End, "https://cdn/last.png"),
        ];
        s.upsert(&j).unwrap();

        let back = s.get("a").unwrap().unwrap();
        assert_eq!(back.media.len(), 2);
        assert_eq!(back.media[0].role, MediaRole::Start);
        assert_eq!(back.media[1].role, MediaRole::End);
    }

    #[test]
    fn a_v1_database_upgrades_without_losing_rows() {
        // Migrations are append-only, so an existing library must survive the
        // new column rather than being rebuilt.
        let s = SqliteStore::in_memory().unwrap();
        {
            let conn = s.conn.lock().unwrap();
            conn.execute_batch("PRAGMA user_version = 1;").unwrap();
            // Every column added after v1, so the replay actually starts there.
            conn.execute_batch(
                "ALTER TABLE job_sets DROP COLUMN media; \
                 ALTER TABLE job_sets DROP COLUMN endpoint; \
                 ALTER TABLE job_sets DROP COLUMN enhancer_version; \
                 ALTER TABLE job_sets DROP COLUMN enhance_note; \
                 ALTER TABLE job_sets DROP COLUMN advisories;",
            )
            .unwrap();
        }
        {
            // Written the way v1 wrote them: no media column at all.
            let conn = s.conn.lock().unwrap();
            conn.execute(
                r#"INSERT INTO job_sets
                   (id, model_id, route_id, request_id, status, prompt,
                    created_at, updated_at, results, settings)
                   VALUES ('old','m','fal:x','r','completed','hi',1,1,'[]','{}')"#,
                [],
            )
            .unwrap();
        }

        s.migrate().unwrap();
        let back = s.get("old").unwrap().unwrap();
        assert_eq!(back.id, "old");
        // A pre-v2 row has no attachments, which is correct rather than lossy.
        assert!(back.media.is_empty());
    }

    #[test]
    fn migration_is_idempotent() {
        let s = SqliteStore::in_memory().unwrap();
        s.migrate().unwrap();
        s.migrate().unwrap();
        assert_eq!(s.count().unwrap(), 0);
    }

    #[test]
    fn delete_removes_one_row() {
        let s = SqliteStore::in_memory().unwrap();
        s.upsert(&job("a", JobStatus::Completed)).unwrap();
        s.upsert(&job("b", JobStatus::Completed)).unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.count().unwrap(), 1);
        assert!(s.get("a").unwrap().is_none());
    }
}
