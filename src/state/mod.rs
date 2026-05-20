pub mod migrate;
mod schema;

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub use schema::SCHEMA_VERSION;

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Test-only helper for verifying row state. Not part of the public API; gated
/// to test compilation only.
// Cfg-gated to `any(test, feature = "test-helpers")`. When clippy/clippy-style
// tests run with `--features test-helpers`, the bin compilation also gets the
// feature and includes this struct, but never references it — hence dead_code.
#[cfg(any(test, feature = "test-helpers"))]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct VideoRow {
    pub video_id: String,
    pub status: String,
    pub canonical: bool,
    pub source_url: String,
    pub first_seen_at: i64,
    pub attempt_count: i64,
}

/// Typed errors surfaced by `state::Store` mutators and accessors.
///
/// Per 0022, `Store::open` returns `SchemaVersionMismatch` when the on-disk
/// `meta.schema_version` doesn't match the binary's `SCHEMA_VERSION`. The
/// `Display` impl carries the operator-readable instruction directing them
/// to `uu-tiktok migrate`.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(
        "schema version mismatch: state.sqlite is at v{found}, this binary requires v{expected}. \
         Run `uu-tiktok migrate` to upgrade the database, then retry."
    )]
    SchemaVersionMismatch { expected: String, found: String },
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening SQLite database at {}", path.display()))?;

        // Pragmas applied at every open.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
        .context("setting connection pragmas")?;

        // Schema (idempotent — uses CREATE IF NOT EXISTS). The column set
        // declared here is the CURRENT schema version; older DBs miss the
        // newer columns and must run `uu-tiktok migrate` (0022) before
        // they can be opened.
        conn.execute_batch(schema::SCHEMA_SQL)
            .context("applying schema")?;

        // Schema-version check (0022). Three cases:
        //   - fresh DB (no meta row): record the current version.
        //   - existing DB at current version: continue.
        //   - mismatch: return typed StateError::SchemaVersionMismatch.
        let found: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .context("reading schema_version from meta")?;

        match found {
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    params![SCHEMA_VERSION],
                )
                .context("recording schema_version on fresh DB")?;
            }
            Some(v) if v == SCHEMA_VERSION => {}
            Some(v) => {
                return Err(StateError::SchemaVersionMismatch {
                    expected: SCHEMA_VERSION.to_string(),
                    found: v,
                }
                .into());
            }
        }

        Ok(Self { conn })
    }

    pub fn read_meta(&self, key: &str) -> Result<Option<String>> {
        let result = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .map_or_else(
                |e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                },
                |v| Ok(Some(v)),
            )?;
        Ok(result)
    }

    // No bin consumer; only the cfg(test) `pragma_journal_mode_is_wal`
    // integration test calls this. Visibility/API decision deferred per
    // FOLLOWUPS (`Store::pragma_string` visibility) and 0002.
    #[allow(dead_code)]
    pub fn pragma_string(&self, name: &str) -> Result<String> {
        let value: String = self
            .conn
            .query_row(&format!("PRAGMA {}", name), [], |row| row.get(0))
            .with_context(|| format!("reading PRAGMA {}", name))?;
        Ok(value)
    }

    /// Borrow the underlying connection for advanced operations. Internal use
    /// for now; the public API will grow as Tasks 9+ add methods.
    // T9 (store-ingest) and T10 (store-claims) are the first consumers.
    #[allow(dead_code)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    // T9 (store-ingest) and T10 (store-claims) are the first consumers.
    #[allow(dead_code)]
    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Returns the number of rows actually inserted (1 for new, 0 for an
    /// idempotent re-upsert of an existing row). Symmetric with
    /// `upsert_watch_history`.
    pub fn upsert_video(
        &mut self,
        video_id: &str,
        source_url: &str,
        canonical: bool,
    ) -> Result<usize> {
        let now = unix_now();
        let changed = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO videos
                 (video_id, source_url, canonical, status,
                  first_seen_at, updated_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?4)",
                params![video_id, source_url, canonical as i64, now],
            )
            .with_context(|| format!("upserting video {}", video_id))?;
        Ok(changed)
    }

    pub fn upsert_watch_history(
        &mut self,
        respondent_id: &str,
        video_id: &str,
        watched_at: i64,
        in_window: bool,
    ) -> Result<usize> {
        let changed = self
            .conn
            .execute(
                "INSERT OR IGNORE INTO watch_history
                 (respondent_id, video_id, watched_at, in_window)
                 VALUES (?1, ?2, ?3, ?4)",
                params![respondent_id, video_id, watched_at, in_window as i64],
            )
            .with_context(|| {
                format!(
                    "upserting watch_history (respondent={}, video={}, watched_at={})",
                    respondent_id, video_id, watched_at
                )
            })?;
        Ok(changed)
    }
}

/// Represents a successfully claimed video row, returned by `claim_next`.
#[derive(Debug, Clone)]
pub struct Claim {
    pub video_id: String,
    pub source_url: String,
    pub attempt_count: i64,
}

/// Artifacts written to the database upon successful transcription.
#[derive(Debug, Clone)]
pub struct SuccessArtifacts {
    pub duration_s: Option<f64>,
    pub language_detected: Option<String>,
    pub fetcher: &'static str,
    pub transcript_source: &'static str,
}

impl Store {
    /// Atomically claim the oldest pending video for processing.
    ///
    /// Uses `BEGIN IMMEDIATE` to serialize concurrent claim attempts across
    /// multiple connections to the same SQLite file.
    pub fn claim_next(&mut self, worker_id: &str) -> Result<Option<Claim>> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for claim_next")?;

        let candidate: Option<(String, String, i64)> = tx
            .query_row(
                "SELECT video_id, source_url, attempt_count
                 FROM videos
                 WHERE status = 'pending'
                 ORDER BY first_seen_at ASC, video_id ASC
                 LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((video_id, source_url, prev_attempts)) = candidate else {
            tx.commit()?;
            return Ok(None);
        };

        let new_attempts = prev_attempts + 1;
        tx.execute(
            "UPDATE videos
             SET status = 'in_progress',
                 claimed_by = ?2,
                 claimed_at = ?3,
                 attempt_count = ?4,
                 updated_at = ?3
             WHERE video_id = ?1",
            params![video_id, worker_id, now, new_attempts],
        )?;

        tx.execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES (?1, ?2, 'claimed', ?3, NULL)",
            params![video_id, now, worker_id],
        )?;

        tx.commit().context("commit claim transaction")?;

        Ok(Some(Claim {
            video_id,
            source_url,
            attempt_count: new_attempts,
        }))
    }

    /// Mark a video as succeeded and record a `succeeded` event, atomically.
    /// Returns the row-change count from the videos UPDATE per 0006.
    ///
    /// The UPDATE is guarded by `WHERE status='in_progress' AND claimed_by = ?`
    /// (0023 symmetric with mark_retryable_failure / mark_terminal_failure):
    /// callers can detect "0 means the row was not in_progress or claimed by
    /// a different worker (stale claim)" without a separate query. The event
    /// row is inserted only when the UPDATE matches, so video_events stays
    /// faithful to what actually changed.
    pub fn mark_succeeded(
        &mut self,
        video_id: &str,
        worker_id: &str,
        artifacts: SuccessArtifacts,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for mark_succeeded")?;

        let changed = tx
            .execute(
                "UPDATE videos
             SET status = 'succeeded',
                 succeeded_at = ?2,
                 duration_s = ?3,
                 language_detected = ?4,
                 fetcher = ?5,
                 transcript_source = ?6,
                 updated_at = ?2
             WHERE video_id = ?1
               AND status = 'in_progress'
               AND claimed_by = ?7",
                params![
                    video_id,
                    now,
                    artifacts.duration_s,
                    artifacts.language_detected,
                    artifacts.fetcher,
                    artifacts.transcript_source,
                    worker_id,
                ],
            )
            .with_context(|| format!("update videos for succeeded {}", video_id))?;

        // Only insert the event row if the UPDATE matched — symmetry with the
        // mutator's row-change count. 0008 invariant: artifacts are durable
        // before this call regardless of outcome; the event row is bookkeeping
        // for "the DB acknowledged the success."
        if changed > 0 {
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'succeeded', ?3, NULL)",
                params![video_id, now, worker_id],
            )?;
        }

        tx.commit().context("commit mark_succeeded")?;
        Ok(changed)
    }
}

impl Store {
    // Cfg-gated test helper; same bin-firing dynamic as `VideoRow` above when
    // `--features test-helpers` is enabled at the workspace level.
    #[cfg(any(test, feature = "test-helpers"))]
    #[allow(dead_code)]
    pub fn get_video_for_test(&self, video_id: &str) -> Result<Option<VideoRow>> {
        let row = self
            .conn
            .query_row(
                "SELECT video_id, status, canonical, source_url, first_seen_at, attempt_count
                 FROM videos WHERE video_id = ?1",
                params![video_id],
                |r| {
                    Ok(VideoRow {
                        video_id: r.get(0)?,
                        status: r.get(1)?,
                        canonical: r.get::<_, i64>(2)? != 0,
                        source_url: r.get(3)?,
                        first_seen_at: r.get(4)?,
                        attempt_count: r.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }
}

/// A row from `video_events`, returned by `get_events_for_test`.
// Cfg-gated test helper per 0005; fires dead_code in bin compilation when --features test-helpers is enabled.
#[cfg(any(test, feature = "test-helpers"))]
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct EventRow {
    pub event_type: String,
    pub worker_id: Option<String>,
}

#[cfg(any(test, feature = "test-helpers"))]
impl Store {
    /// Retrieve all `video_events` rows for a given video_id, ordered by id.
    // Cfg-gated test helper per 0005; same bin-firing dynamic as EventRow above.
    #[allow(dead_code)]
    pub fn get_events_for_test(&self, video_id: &str) -> Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_type, worker_id FROM video_events WHERE video_id = ?1 ORDER BY id",
        )?;
        let rows: Vec<EventRow> = stmt
            .query_map(params![video_id], |r| {
                Ok(EventRow {
                    event_type: r.get(0)?,
                    worker_id: r.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Real-TDD bug-fix test (per ADR 0003). SQLite's `TEXT PRIMARY KEY` does
    /// NOT imply NOT NULL — only `INTEGER PRIMARY KEY` (rowid alias) does. The
    /// schema must declare NOT NULL explicitly. This test guards against
    /// regressing the schema to the implicit-NULL form.
    #[test]
    fn null_video_id_rejected_by_videos_schema() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let result = store.conn().execute(
            "INSERT INTO videos
             (video_id, source_url, canonical, status, first_seen_at, updated_at)
             VALUES (NULL, 'x', 0, 'pending', 0, 0)",
            [],
        );
        assert!(
            result.is_err(),
            "expected NOT NULL constraint to reject NULL video_id, but insert succeeded"
        );
    }

    /// Same SQLite quirk applies to `meta.key`. Guard it too.
    #[test]
    fn null_meta_key_rejected_by_meta_schema() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let result = store
            .conn()
            .execute("INSERT INTO meta (key, value) VALUES (NULL, 'x')", []);
        assert!(
            result.is_err(),
            "expected NOT NULL constraint to reject NULL meta.key, but insert succeeded"
        );
    }
}
