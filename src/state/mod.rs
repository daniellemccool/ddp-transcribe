pub mod migrate;
pub mod queries;
mod schema;

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub use schema::SCHEMA_VERSION;

pub(crate) fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // Saturating, infallible conversion: seconds-since-epoch fit i64 for ~292
        // billion years, but try_from avoids the lossy-cast lint and any wrap.
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
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
    /// Epic 3 T07 addition: lets integration tests assert the
    /// classifier-dispatched write-off reason (`mark_terminal_failure`'s
    /// `reason` column) without a hand-rolled raw `rusqlite::Connection`
    /// query per test (the pre-existing convention in `serial_tests.rs`).
    pub terminal_reason: Option<String>,
    /// Epic 3 T07 addition: same rationale as `terminal_reason`, for the
    /// retryable-side taxonomy kind (`mark_retryable_failure`'s `kind`
    /// column).
    pub last_retryable_kind: Option<String>,
}

/// One failed_retryable row, as the sweep sees it: a snapshot of rows
/// awaiting sweep adjudication. Message included because the sweep
/// classifies stored messages directly through the active
/// `ClassificationTable` (no probe step post-Epic-4a).
#[derive(Debug, Serialize)]
pub struct ParkedRow {
    pub video_id: String,
    /// The previously-stored retryable kind. `batch::run_sweep` reads it on a
    /// FALLBACK classification hit (no rule matched): a fallback carries no
    /// positive evidence about the message class, so a real stored kind (e.g.
    /// `ToolTimeout` — a non-fetch failure the fetch-stderr table never
    /// matches) is preserved rather than relabelled. Empty/NULL kinds and the
    /// legacy placeholder `"Fetch"` still take the fallback label so they
    /// normalize before the row becomes claimable. (0002 note removed: the
    /// preserve-kind-on-fallback fix made this field a live read.)
    pub last_retryable_kind: Option<String>,
    pub last_retryable_message: Option<String>,
    // 0002: `#[allow(dead_code)]` lifted (Epic 4b Task 03) — the
    // `status --retryable` renderer is the first bin consumer since Epic 4a
    // T08 deleted the triage subcommand (its `run_triage` compared this
    // against `--max-attempts`).
    pub attempt_count: i64,
}

/// Typed errors surfaced by `state::Store` mutators and accessors.
///
/// Per 0022, `Store::open` returns `SchemaVersionMismatch` when the on-disk
/// `meta.schema_version` doesn't match the binary's `SCHEMA_VERSION`. The
/// `Display` impl carries the operator-readable instruction directing them
/// to `ddp-transcribe migrate`.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(
        "schema version mismatch: state.sqlite is at v{found}, this binary requires v{expected}. \
         Run `ddp-transcribe migrate` to upgrade the database, then retry."
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
        // newer columns and must run `ddp-transcribe migrate` (0022) before
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
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .with_context(|| format!("reading PRAGMA {name}"))?;
        Ok(value)
    }

    /// Borrow the underlying connection for advanced operations. Internal use
    /// for now; the public API will grow as Tasks 9+ add methods.
    ///
    /// T18 (pipelined orchestrator's `compute_process_stats`) was the first
    /// in-bin consumer; the Epic 4a T06 review fix retired that fn
    /// (`ProcessStats` is assembled from input-side counters per 0007), so
    /// no bin caller remains. In-module `#[cfg(test)]` schema tests still
    /// call it — suppressed per 0002; lift when the next raw-connection
    /// consumer lands.
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
    ///
    /// Single-statement convenience wrapper retained for the integration tests
    /// (`tests/state_ingest.rs`); the production `ingest` walk uses the batched
    /// transaction path ([`Store::transaction`] + [`upsert_video_tx`]). The tests
    /// link the lib, but the bin no longer calls this, hence `dead_code` per 0002.
    #[allow(dead_code)]
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
                UPSERT_VIDEO_SQL,
                params![video_id, source_url, i64::from(canonical), now],
            )
            .with_context(|| format!("upserting video {video_id}"))?;
        Ok(changed)
    }

    /// Convenience sibling of [`Store::upsert_video`]; see its note re: 0002.
    #[allow(dead_code)]
    pub fn upsert_watch_history(
        &mut self,
        respondent_id: &str,
        video_id: &str,
        watched_at: i64,
        watched_at_raw: &str,
        in_window: bool,
    ) -> Result<usize> {
        let changed = self
            .conn
            .execute(
                UPSERT_WATCH_HISTORY_SQL,
                params![
                    respondent_id,
                    video_id,
                    watched_at,
                    i64::from(in_window),
                    watched_at_raw
                ],
            )
            .with_context(|| {
                format!(
                    "upserting watch_history (respondent={respondent_id}, video={video_id}, watched_at={watched_at})"
                )
            })?;
        Ok(changed)
    }

    /// Open a transaction for the batch ingest path. `ingest` opens one transaction
    /// per input file (after that file is read and parsed) and commits it before the
    /// next file, instead of paying a per-row commit. Pair with [`upsert_video_tx`] /
    /// [`upsert_watch_history_tx`], which reuse `prepare_cached` statements across
    /// transactions on the same connection.
    pub(crate) fn transaction(&mut self) -> Result<rusqlite::Transaction<'_>> {
        self.conn.transaction().context("begin ingest transaction")
    }

    /// The `ingested_files` fingerprint recorded for `file_name` (basename),
    /// as `(size_bytes, mtime)`; `None` = never fully ingested. Read on the
    /// connection rather than inside the per-file transaction because it
    /// decides whether that transaction is opened at all. `prepare_cached`
    /// keeps it to one prepare across the whole walk.
    pub(crate) fn ingested_file_fingerprint(&self, file_name: &str) -> Result<Option<(i64, i64)>> {
        self.conn
            .prepare_cached(SELECT_INGESTED_FILE_SQL)
            .context("preparing ingested_file_fingerprint")?
            .query_row(params![file_name], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .with_context(|| format!("reading ingest ledger row for {file_name}"))
    }
}

/// Shared INSERT-OR-IGNORE SQL so the `&mut self` convenience methods and the
/// transaction-scoped batch helpers below cannot drift.
const UPSERT_VIDEO_SQL: &str = "INSERT OR IGNORE INTO videos
                 (video_id, source_url, canonical, status,
                  first_seen_at, updated_at)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?4)";
const UPSERT_WATCH_HISTORY_SQL: &str = "INSERT OR IGNORE INTO watch_history
                 (respondent_id, video_id, watched_at, in_window, watched_at_raw)
                 VALUES (?1, ?2, ?3, ?4, ?5)";
/// Backfill the raw DDP Date string onto a pre-v4 row. Deliberately does
/// NOT touch in_window: recompute-window is the only path that changes
/// flags after ingest (Epic 4b window ADR).
const BACKFILL_WATCH_RAW_SQL: &str = "UPDATE watch_history
                 SET watched_at_raw = ?4
                 WHERE respondent_id = ?1 AND video_id = ?2 AND watched_at = ?3
                   AND watched_at_raw IS NULL";

/// Transaction-scoped upsert for the batch ingest path. `prepare_cached` prepares
/// the statement once and reuses it for every row in the walk. Shares
/// [`UPSERT_VIDEO_SQL`] with [`Store::upsert_video`].
pub(crate) fn upsert_video_tx(
    tx: &rusqlite::Transaction<'_>,
    video_id: &str,
    source_url: &str,
    canonical: bool,
) -> Result<usize> {
    let now = unix_now();
    let changed = tx
        .prepare_cached(UPSERT_VIDEO_SQL)
        .context("preparing upsert_video")?
        .execute(params![video_id, source_url, i64::from(canonical), now])
        .with_context(|| format!("upserting video {video_id}"))?;
    Ok(changed)
}

/// Transaction-scoped sibling of [`upsert_video_tx`]; shares
/// [`UPSERT_WATCH_HISTORY_SQL`] with [`Store::upsert_watch_history`].
pub(crate) fn upsert_watch_history_tx(
    tx: &rusqlite::Transaction<'_>,
    respondent_id: &str,
    video_id: &str,
    watched_at: i64,
    watched_at_raw: &str,
    in_window: bool,
) -> Result<usize> {
    let changed = tx
        .prepare_cached(UPSERT_WATCH_HISTORY_SQL)
        .context("preparing upsert_watch_history")?
        .execute(params![
            respondent_id,
            video_id,
            watched_at,
            i64::from(in_window),
            watched_at_raw
        ])
        .with_context(|| {
            format!(
                "upserting watch_history (respondent={respondent_id}, video={video_id}, watched_at={watched_at})"
            )
        })?;
    Ok(changed)
}

/// Ingest ledger read + write (schema v6). The upsert is last-write-wins on
/// `file_name` so a changed file's fingerprint is refreshed in place.
const SELECT_INGESTED_FILE_SQL: &str =
    "SELECT size_bytes, mtime FROM ingested_files WHERE file_name = ?1";
const UPSERT_INGESTED_FILE_SQL: &str = "INSERT INTO ingested_files
                 (file_name, size_bytes, mtime, ingested_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(file_name) DO UPDATE SET
                     size_bytes  = excluded.size_bytes,
                     mtime       = excluded.mtime,
                     ingested_at = excluded.ingested_at";

/// Record that `file_name` (basename only — the inbox directory may move
/// between hosts) was fully ingested at the given fingerprint. Called INSIDE
/// the same transaction that commits that file's rows, so the ledger row
/// exists iff the data is committed. Returns the row-change count per 0006.
pub(crate) fn upsert_ingested_file_tx(
    tx: &rusqlite::Transaction<'_>,
    file_name: &str,
    size_bytes: i64,
    mtime: i64,
) -> Result<usize> {
    let changed = tx
        .prepare_cached(UPSERT_INGESTED_FILE_SQL)
        .context("preparing upsert_ingested_file")?
        .execute(params![file_name, size_bytes, mtime, unix_now()])
        .with_context(|| format!("recording ingest ledger row for {file_name}"))?;
    Ok(changed)
}

/// Transaction-scoped backfill of watched_at_raw for an existing row
/// (INSERT OR IGNORE hit). Returns the row-change count per 0006:
/// 1 = backfilled a NULL, 0 = row already carried its raw string.
pub(crate) fn backfill_watch_raw_tx(
    tx: &rusqlite::Transaction<'_>,
    respondent_id: &str,
    video_id: &str,
    watched_at: i64,
    watched_at_raw: &str,
) -> Result<usize> {
    let changed = tx
        .prepare_cached(BACKFILL_WATCH_RAW_SQL)
        .context("preparing backfill_watch_raw")?
        .execute(params![respondent_id, video_id, watched_at, watched_at_raw])
        .with_context(|| {
            format!("backfilling watched_at_raw (respondent={respondent_id}, video={video_id})")
        })?;
    Ok(changed)
}

/// Represents a successfully claimed video row, returned by `claim_next`.
#[derive(Debug, Clone)]
pub struct Claim {
    pub video_id: String,
    pub source_url: String,
    pub attempt_count: i64,
    /// Kind tag recorded by the most recent retryable failure, if any.
    /// None on first attempt. Epic 3 cookie routing keys on this being
    /// "SensitiveLoginGated" (ADR 0035); the start-of-batch sweep's requeue
    /// normalizes historical placeholder kinds before the row becomes
    /// claimable again.
    // 0002: `#[allow(dead_code)]` lifted here (Task 08) — now read by
    // `pipeline::cookie_opts_for`'s kind-gated cookie routing.
    pub last_retryable_kind: Option<String>,
}

/// Outcome of `record_fetch_failure`'s one-transaction decision (Epic 4a):
/// where did the failed row land, and did anything change at all.
// 0002: `#[allow(dead_code)]` lifted in Epic 4a T06 — the pipelined workers
// (via the shared record-failure helper) and `record_fetch_failure_serial`
// match on every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureRecordOutcome {
    /// Row went back to 'pending' (end of queue via T05 ordering).
    Requeued,
    /// Attempt cap reached — row parked in 'failed_retryable' (exhausted pool).
    Exhausted,
    /// requires-cookie failure with no cookies configured this run — parked
    /// in 'failed_retryable' without consuming the remaining retry budget.
    ParkedForCookies,
    /// Claim predicate missed (concurrent sweep re-claimed the row) — no
    /// mutation happened; caller counts it as stale_after_failure.
    StaleClaim,
}

/// Artifacts written to the database upon successful transcription.
#[derive(Debug, Clone)]
pub struct SuccessArtifacts {
    pub duration_s: Option<f64>,
    pub language_detected: Option<String>,
    pub fetcher: &'static str,
    pub transcript_source: &'static str,
}

/// Typed column values for one video, produced by the metadata loader
/// (Epic 4c). All-nullable except the snapshot timestamp, which is the
/// `video_metadata_raw.fetched_at` of the blob these values came from.
#[derive(Debug, Clone)]
pub struct MetadataColumns {
    pub video_id: String,
    pub video_description: Option<String>,
    pub uploader: Option<String>,
    pub uploader_id: Option<String>,
    pub video_created_at: Option<i64>,
    pub view_count: Option<i64>,
    pub like_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub metadata_fetched_at: i64,
}

impl Store {
    /// Atomically claim the next pending video: fresh work first
    /// (`attempt_count ASC` — Epic 4a end-of-queue retries), FIFO by
    /// first_seen_at within each attempt tier. Matches
    /// idx_videos_pending_v3's column order.
    ///
    /// Uses `BEGIN IMMEDIATE` to serialize concurrent claim attempts across
    /// multiple connections to the same SQLite file.
    pub fn claim_next(&mut self, worker_id: &str) -> Result<Option<Claim>> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for claim_next")?;

        let candidate: Option<(String, String, i64, Option<String>)> = tx
            .query_row(
                "SELECT video_id, source_url, attempt_count, last_retryable_kind
                 FROM videos
                 WHERE status = 'pending'
                 ORDER BY attempt_count ASC, first_seen_at ASC, video_id ASC
                 LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .context("claim_next: select oldest pending row")?;

        let Some((video_id, source_url, prev_attempts, last_retryable_kind)) = candidate else {
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
        )
        .with_context(|| format!("claim_next: flip {video_id} to in_progress for {worker_id}"))?;

        tx.execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES (?1, ?2, 'claimed', ?3, NULL)",
            params![video_id, now, worker_id],
        )
        .with_context(|| format!("claim_next: insert claimed event for {video_id}"))?;

        tx.commit().context("commit claim transaction")?;

        Ok(Some(Claim {
            video_id,
            source_url,
            attempt_count: new_attempts,
            last_retryable_kind,
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
            .with_context(|| format!("update videos for succeeded {video_id}"))?;

        // Only insert the event row if the UPDATE matched — symmetry with the
        // mutator's row-change count. 0008 invariant: artifacts are durable
        // before this call regardless of outcome; the event row is bookkeeping
        // for "the DB acknowledged the success."
        if changed > 0 {
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'succeeded', ?3, NULL)",
                params![video_id, now, worker_id],
            )
            .with_context(|| format!("mark_succeeded: insert succeeded event for {video_id}"))?;
        }

        tx.commit().context("commit mark_succeeded")?;
        Ok(changed)
    }

    /// Insert or overwrite the raw fetch-time metadata envelope for a video
    /// (Epic 4c). Keyed by video_id — one row per unique video, last write
    /// wins across retries (engagement counts are point-in-time; fetched_at
    /// records the snapshot moment). Returns the row-change count per 0006.
    ///
    /// Callers treat failures as best-effort (log + continue): metadata
    /// must never change a video's pipeline outcome.
    ///
    /// Task 03 wired both pipeline paths (`fetch_worker` and
    /// `process_one`) as the first real callers, lifting the placeholder
    /// `#[allow(dead_code)]` per 0002.
    pub fn upsert_metadata_raw(&mut self, video_id: &str, envelope_json: &str) -> Result<usize> {
        let now = unix_now();
        let changed = self
            .conn
            .execute(
                "INSERT INTO video_metadata_raw (video_id, fetched_at, raw_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(video_id) DO UPDATE SET
                     fetched_at = excluded.fetched_at,
                     raw_json   = excluded.raw_json",
                params![video_id, now, envelope_json],
            )
            .with_context(|| format!("upsert_metadata_raw for {video_id}"))?;
        Ok(changed)
    }

    /// Insert a raw metadata envelope only if the video has none
    /// (backfill-metadata's write path). Unlike `upsert_metadata_raw`
    /// (fetch-path, last-write-wins), the backfill must never overwrite
    /// an envelope the fetch path captured. Returns the row-change
    /// count per 0006: 1 = inserted, 0 = a row already exists (the
    /// caller counts it; it is not an error). Best-effort contract as
    /// for `upsert_metadata_raw`: metadata writes never change a
    /// video's pipeline outcome.
    // 0002: consumed by the `backfill-metadata` subcommand (Task 03);
    // lift this allow when that loop lands.
    #[allow(dead_code)]
    pub fn insert_metadata_raw_if_missing(
        &mut self,
        video_id: &str,
        envelope_json: &str,
    ) -> Result<usize> {
        let now = unix_now();
        let changed = self
            .conn
            .execute(
                "INSERT INTO video_metadata_raw (video_id, fetched_at, raw_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(video_id) DO NOTHING",
                params![video_id, now, envelope_json],
            )
            .with_context(|| format!("inserting backfill metadata for {video_id}"))?;
        Ok(changed)
    }

    /// Apply one loader batch in a single transaction (Epic 4c). Overwrites
    /// unconditionally — last-write-wins replay semantics, so re-running
    /// `load-metadata` after a parser fix needs no re-fetch. Returns the
    /// total row-change count per 0006; a row whose video_id no longer
    /// exists in `videos` contributes 0 (the loader counts it as
    /// `rows_without_video`, it is not an error).
    ///
    /// Note the deliberate absence of a status/claim guard: unlike the
    /// pipeline mutators (0023), this runs post-run against rows in any
    /// lifecycle state, and touches only descriptive columns.
    pub fn apply_metadata_batch(&mut self, rows: &[MetadataColumns]) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for apply_metadata_batch")?;
        let mut changed = 0usize;
        {
            let mut stmt = tx
                .prepare_cached(
                    "UPDATE videos SET
                         video_description = ?2, uploader = ?3, uploader_id = ?4,
                         video_created_at = ?5, view_count = ?6, like_count = ?7,
                         comment_count = ?8, metadata_fetched_at = ?9
                     WHERE video_id = ?1",
                )
                .context("prepare apply_metadata_batch")?;
            for row in rows {
                changed += stmt
                    .execute(params![
                        row.video_id,
                        row.video_description,
                        row.uploader,
                        row.uploader_id,
                        row.video_created_at,
                        row.view_count,
                        row.like_count,
                        row.comment_count,
                        row.metadata_fetched_at,
                    ])
                    .with_context(|| format!("apply_metadata_batch for {}", row.video_id))?;
            }
        }
        tx.commit().context("commit apply_metadata_batch")?;
        Ok(changed)
    }

    /// Flip a video row from `in_progress` to `failed_retryable`, recording
    /// the failure classification (kind + message) per 0023. Same
    /// stale-claim predicate as `mark_succeeded`. Returns the row-change
    /// count per 0006: 0 on stale claim, 1 on successful flip.
    ///
    /// `kind` is a stable short tag (e.g. "FetchTimeout", "TranscribeError").
    /// Epic 3's typed RetryableKind serializes via tag()/message() into the
    /// same columns; no schema change at that point — just the caller switching
    /// from string literals to enum projections.
    ///
    /// The `terminal_reason`/`terminal_message` columns are NOT cleared on
    /// this flip — they're retained as diagnostic history if the row was
    /// previously terminal (e.g., operator manually requeued). Symmetric:
    /// `mark_terminal_failure` likewise preserves prior `last_retryable_*`.
    // T9 wired this into `run_serial`'s error arm with a placeholder kind
    // ("FetchOrTranscribe" per 0023); Epic 3 T07 replaced the placeholder
    // with typed classifier dispatch (`RetryableKind::tag()`).
    //
    // 0002: Epic 4a T06 switched every pipeline caller (fetch_worker,
    // transcribe_worker, run_serial) to `record_fetch_failure`, so the bin
    // no longer reaches this mutator. Integration tests
    // (`tests/state_claims.rs`, `tests/state_sweep.rs`) still exercise it
    // directly as a failed_retryable seeding helper, hence dead_code-suppressed
    // rather than deleted. (Epic 4a T08 checked at triage retirement: retained —
    // the direct-mutator tests still depend on it.)
    #[allow(dead_code)]
    pub fn mark_retryable_failure(
        &mut self,
        video_id: &str,
        worker_id: &str,
        kind: &str,
        message: &str,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for mark_retryable_failure")?;

        let changed = tx
            .execute(
                "UPDATE videos
                 SET status = 'failed_retryable',
                     last_retryable_kind = ?2,
                     last_retryable_message = ?3,
                     claimed_by = NULL,
                     claimed_at = NULL,
                     updated_at = ?4
                 WHERE video_id = ?1
                   AND status = 'in_progress'
                   AND claimed_by = ?5",
                params![video_id, kind, message, now, worker_id],
            )
            .with_context(|| format!("update videos for failed_retryable {video_id}"))?;

        if changed > 0 {
            let detail = serde_json::json!({ "kind": kind, "message": message }).to_string();
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'failed_retryable', ?3, ?4)",
                params![video_id, now, worker_id, detail],
            )?;
        }

        tx.commit().context("commit mark_retryable_failure")?;
        Ok(changed)
    }

    /// Failure-time retry decision (Epic 4a, supersedes the Epic 3 pattern
    /// of always parking in failed_retryable). One IMMEDIATE transaction:
    ///
    /// - requires-cookie without cookies configured → park (failed_retryable);
    ///   a cookie-less retry is a guaranteed refail that would burn budget.
    /// - under the cap (`attempt_count < max_attempts`; attempt_count was
    ///   already bumped at claim time by claim_next) → back to 'pending',
    ///   unowned, rejoining the queue behind fresh work (T05 ordering).
    /// - cap exhausted → failed_retryable (the "exhausted, adjudicate" pool).
    /// - claim predicate miss everywhere → StaleClaim, nothing recorded.
    ///
    /// Always writes label+message to last_retryable_kind/_message on any
    /// row it changes. Events: 'cookie_parked' / 'retry_requeued' /
    /// 'failed_retryable' (existing vocabulary for the exhausted case), all
    /// with the uniform `{"kind": …, "message": …, "policy": …}`
    /// detail_json shape. `"policy"` (ADR 0038) carries the short stable
    /// [`crate::fetcher::FetchPolicy::tag`] of the format policy the fetch
    /// actually ran under ("deterministic-audio" / "frugal"), making the
    /// failure mix attributable per policy — the observability that lets
    /// the backlog retry batch double as the at-scale frugal experiment.
    /// This extends the Epic 4a uniform-shape contract ADDITIVELY:
    /// consumers reading `{"kind", "message"}` are unaffected.
    /// `mark_retryable_failure` (bin-dead, test-seeding only) still emits
    /// the two-key shape, so 'failed_retryable' events written by THIS
    /// mutator are a superset of that legacy schema rather than divergent.
    ///
    /// 0006 note: the `Result<usize>` row-count contract is honored
    /// internally — each UPDATE's row count drives the outcome; the typed
    /// enum IS the row-count information, made unambiguous for the caller.
    // 0002: `#[allow(dead_code)]` lifted in Epic 4a T06 — first callers:
    // fetch_worker + transcribe_worker (via the shared pipelined
    // record-failure helper) and run_serial's `record_fetch_failure_serial`.
    #[allow(clippy::too_many_arguments)] // one logical decision; every arg participates
    pub fn record_fetch_failure(
        &mut self,
        video_id: &str,
        worker_id: &str,
        label: &str,
        message: &str,
        fetch_policy: &str,
        max_attempts: i64,
        requires_cookie: bool,
        cookies_configured: bool,
    ) -> Result<FailureRecordOutcome> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for record_fetch_failure")?;

        let park = |tx: &rusqlite::Transaction<'_>, event: &str| -> Result<usize> {
            let changed = tx
                .execute(
                    "UPDATE videos
                     SET status = 'failed_retryable',
                         last_retryable_kind = ?2,
                         last_retryable_message = ?3,
                         claimed_by = NULL,
                         claimed_at = NULL,
                         updated_at = ?4
                     WHERE video_id = ?1
                       AND status = 'in_progress'
                       AND claimed_by = ?5",
                    params![video_id, label, message, now, worker_id],
                )
                .with_context(|| format!("record_fetch_failure park for {video_id}"))?;
            if changed > 0 {
                let detail =
                    serde_json::json!({ "kind": label, "message": message, "policy": fetch_policy })
                        .to_string();
                tx.execute(
                    "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![video_id, now, event, worker_id, detail],
                )
                .with_context(|| format!("record_fetch_failure {event} event for {video_id}"))?;
            }
            Ok(changed)
        };

        let outcome = if requires_cookie && !cookies_configured {
            if park(&tx, "cookie_parked")? > 0 {
                FailureRecordOutcome::ParkedForCookies
            } else {
                FailureRecordOutcome::StaleClaim
            }
        } else {
            let requeued = tx
                .execute(
                    "UPDATE videos
                     SET status = 'pending',
                         last_retryable_kind = ?2,
                         last_retryable_message = ?3,
                         claimed_by = NULL,
                         claimed_at = NULL,
                         updated_at = ?4
                     WHERE video_id = ?1
                       AND status = 'in_progress'
                       AND claimed_by = ?5
                       AND attempt_count < ?6",
                    params![video_id, label, message, now, worker_id, max_attempts],
                )
                .with_context(|| format!("record_fetch_failure requeue for {video_id}"))?;
            if requeued > 0 {
                let detail =
                    serde_json::json!({ "kind": label, "message": message, "policy": fetch_policy })
                        .to_string();
                tx.execute(
                    "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                     VALUES (?1, ?2, 'retry_requeued', ?3, ?4)",
                    params![video_id, now, worker_id, detail],
                )
                .with_context(|| {
                    format!("record_fetch_failure retry_requeued event for {video_id}")
                })?;
                FailureRecordOutcome::Requeued
            } else if park(&tx, "failed_retryable")? > 0 {
                FailureRecordOutcome::Exhausted
            } else {
                FailureRecordOutcome::StaleClaim
            }
        };

        tx.commit().context("commit record_fetch_failure")?;
        Ok(outcome)
    }

    /// Flip a video row from `in_progress` to `failed_terminal`, recording
    /// the terminal reason + message in the terminal_reason/terminal_message
    /// columns. Same stale-claim predicate as the rest of the family
    /// (0023). Returns the row-change count per 0006.
    ///
    /// **First wired in Epic 3 T07.** `fetch_worker` and `run_serial`'s
    /// error arm call this when `classify_fetch_phase`/`classify_fetch_error`
    /// returns `ClassifiedFailure::Unavailable` (ADR 0033 write-off classes:
    /// `IpBlockedMessage`, `VideoNotAvailable10231`) — a row that will never
    /// succeed on retry. Epic 2 landed the surface with no caller so Epic 3's
    /// diff would be a classifier-add task, not a mutator-add task; the
    /// `#[allow(dead_code)]` that held that placement is lifted here per 0002.
    ///
    /// The `last_retryable_kind`/`last_retryable_message` columns are NOT
    /// cleared on this flip — they're retained as diagnostic history so an
    /// operator inspecting a terminal row can see what retryable failures
    /// preceded it (e.g., "retried 3× as FetchTimeout, then gave up as
    /// VideoUnavailable"). Symmetric: `mark_retryable_failure` likewise
    /// preserves prior `terminal_*`.
    pub fn mark_terminal_failure(
        &mut self,
        video_id: &str,
        worker_id: &str,
        reason: &str,
        message: &str,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for mark_terminal_failure")?;

        let changed = tx
            .execute(
                "UPDATE videos
                 SET status = 'failed_terminal',
                     terminal_reason = ?2,
                     terminal_message = ?3,
                     claimed_by = NULL,
                     claimed_at = NULL,
                     updated_at = ?4
                 WHERE video_id = ?1
                   AND status = 'in_progress'
                   AND claimed_by = ?5",
                params![video_id, reason, message, now, worker_id],
            )
            .with_context(|| format!("update videos for failed_terminal {video_id}"))?;

        if changed > 0 {
            let detail = serde_json::json!({ "reason": reason, "message": message }).to_string();
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'failed_terminal', ?3, ?4)",
                params![video_id, now, worker_id, detail],
            )?;
        }

        tx.commit().context("commit mark_terminal_failure")?;
        Ok(changed)
    }

    /// Recover rows abandoned by a crashed process. Flips rows with
    /// `status='in_progress' AND claimed_at < (now - threshold)` back to
    /// `status='pending'`, clearing claimed_by/claimed_at. Returns the
    /// row-change count (per 0006).
    ///
    /// Per 0024: no artifact validation, no attempt_count bump. The
    /// sweep is operator-recovery semantics; application-retry semantics
    /// (and the `attempt_count` ladder) belong to Epic 3's classifier.
    ///
    /// **`threshold == 0` semantics:** cutoff == now, so the predicate is
    /// `claimed_at < now` — same-second claims survive the sweep (the
    /// timestamp has second resolution; a claim made in the same second as
    /// the sweep call is NOT considered stale). This is intentional; callers
    /// that want "all in_progress rows" must backdate claimed_at or wait one
    /// second past the claim before sweeping with `Duration::ZERO`.
    ///
    /// **Clock-skew note:** rows with `claimed_at > now` (future-valued) are
    /// never swept — the predicate `claimed_at < cutoff` is false when
    /// `claimed_at > now`. This is correct clock-skew behavior; no special
    /// handling is needed.
    // T9 wires this at the top of `run_serial` per 0024.
    pub fn sweep_stale_claims(&mut self, threshold: std::time::Duration) -> Result<usize> {
        let now = unix_now();
        // Saturating cast: absurd Duration values (e.g., u64::MAX seconds)
        // clamp to i64::MAX rather than wrapping. At the 30-min default this
        // never fires; robustness-by-construction for callers that pass
        // large thresholds (e.g., Duration::MAX in tests).
        let threshold_secs = i64::try_from(threshold.as_secs()).unwrap_or(i64::MAX);
        let cutoff = now.saturating_sub(threshold_secs);

        let changed = self
            .conn
            .execute(
                "UPDATE videos
                 SET status = 'pending',
                     claimed_by = NULL,
                     claimed_at = NULL,
                     updated_at = ?1
                 WHERE status = 'in_progress'
                   AND claimed_at IS NOT NULL
                   AND claimed_at < ?2",
                params![now, cutoff],
            )
            .context("UPDATE videos for sweep_stale_claims")?;

        if changed > 0 {
            tracing::info!(recovered = changed, threshold_secs, "sweep_stale_claims");
        }

        Ok(changed)
    }

    /// Snapshot of rows awaiting sweep adjudication, FIFO by first_seen_at.
    /// Read-only.
    pub fn list_failed_retryable(&self) -> Result<Vec<ParkedRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT video_id, last_retryable_kind, last_retryable_message, attempt_count
                 FROM videos WHERE status = 'failed_retryable'
                 ORDER BY first_seen_at ASC, video_id ASC",
            )
            .context("prepare list_failed_retryable")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ParkedRow {
                    video_id: r.get(0)?,
                    last_retryable_kind: r.get(1)?,
                    last_retryable_message: r.get(2)?,
                    attempt_count: r.get(3)?,
                })
            })
            .context("query list_failed_retryable")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_failed_retryable rows")?;
        Ok(rows)
    }

    /// Sweep verdict: dead. failed_retryable → failed_terminal. Unlike
    /// mark_terminal_failure (in_progress + claimed_by predicate, pipeline
    /// caller), this operates on unclaimed failed rows; the operator-visible
    /// audit trail is the 'swept_terminal' event. last_retryable_* columns
    /// are preserved (0023 family convention: diagnostics accumulate).
    pub fn sweep_mark_terminal(
        &mut self,
        video_id: &str,
        reason: &str,
        message: &str,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for sweep_mark_terminal")?;
        let changed = tx
            .execute(
                "UPDATE videos
                 SET status = 'failed_terminal',
                     terminal_reason = ?2,
                     terminal_message = ?3,
                     updated_at = ?4
                 WHERE video_id = ?1 AND status = 'failed_retryable'",
                params![video_id, reason, message, now],
            )
            .with_context(|| format!("sweep_mark_terminal update for {video_id}"))?;
        if changed > 0 {
            let detail = serde_json::json!({ "reason": reason, "message": message }).to_string();
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'swept_terminal', 'sweep', ?3)",
                params![video_id, now, detail],
            )
            .with_context(|| format!("sweep_mark_terminal event for {video_id}"))?;
        }
        tx.commit().context("commit sweep_mark_terminal")?;
        Ok(changed)
    }

    /// Sweep verdict: alive. failed_retryable → pending, gated by the
    /// attempt cap IN THE PREDICATE (race-free: the cap check and the flip
    /// are one statement). Writes the re-classified kind back so historical
    /// placeholder kinds ("Fetch") become taxonomy kinds before the row is
    /// claimable — cookie routing (ADR 0035) reads the kind at claim time.
    pub fn sweep_requeue(
        &mut self,
        video_id: &str,
        new_kind: &str,
        max_attempts: i64,
    ) -> Result<usize> {
        let now = unix_now();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for sweep_requeue")?;
        let changed = tx
            .execute(
                "UPDATE videos
                 SET status = 'pending',
                     last_retryable_kind = ?2,
                     updated_at = ?3
                 WHERE video_id = ?1
                   AND status = 'failed_retryable'
                   AND attempt_count < ?4",
                params![video_id, new_kind, now, max_attempts],
            )
            .with_context(|| format!("sweep_requeue update for {video_id}"))?;
        if changed > 0 {
            let detail = serde_json::json!({ "new_kind": new_kind }).to_string();
            tx.execute(
                "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
                 VALUES (?1, ?2, 'requeued', 'sweep', ?3)",
                params![video_id, now, detail],
            )
            .with_context(|| format!("sweep_requeue event for {video_id}"))?;
        }
        tx.commit().context("commit sweep_requeue")?;
        Ok(changed)
    }

    /// Open a batch-run record (Epic 4a): one row per `process` invocation,
    /// carrying the run parameters and the FULL active classification policy
    /// TOML — the census without its generating policy is not reproducible
    /// attrition documentation. Returns the new run_id.
    ///
    /// Signature note: this returns `Result<i64>` (the generated run_id),
    /// not `Result<usize>` (ADR-0006's row-count contract). 0006 governs
    /// guarded row-TRANSITION mutators, where the row count is the caller's
    /// only way to know whether a predicate matched. This is an
    /// identity-creating INSERT — there is no predicate to miss, and the
    /// product the caller needs is the generated run_id itself, not a count
    /// that would always be 1.
    // 0002: `#[allow(dead_code)]` lifted in Epic 4a T07 — main()'s Process
    // arm calls this before engine construction (fail fast on policy).
    pub fn open_batch_run(&mut self, params_json: &str, policy_toml: &str) -> Result<i64> {
        let now = unix_now();
        self.conn
            .execute(
                "INSERT INTO batch_runs (started_at, params_json, policy_toml)
                 VALUES (?1, ?2, ?3)",
                params![now, params_json, policy_toml],
            )
            .context("insert batch_runs row")?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Close a batch-run record with its census. Returns the row-change
    /// count per 0006 (0 = unknown run_id or already closed by predicate
    /// miss — callers log, never panic).
    // 0002: `#[allow(dead_code)]` lifted in Epic 4a T07 — main()'s Process
    // arm calls this after run_pipelined resolves, via `shared.try_lock()`.
    pub fn close_batch_run(&mut self, run_id: i64, census_json: &str) -> Result<usize> {
        let now = unix_now();
        self.conn
            .execute(
                "UPDATE batch_runs
                 SET finished_at = ?2, census_json = ?3
                 WHERE run_id = ?1 AND finished_at IS NULL",
                params![run_id, now, census_json],
            )
            .context("close batch_runs row")
    }

    /// One-shot in_window recomputation over ALL watch_history rows
    /// (Epic 4b window ADR). Bounds are unix seconds: start inclusive,
    /// end exclusive (the CLI derives them from inclusive calendar dates);
    /// both None = clear (everything in-window). Returns the number of
    /// rows whose flag actually changed, per 0006.
    pub fn recompute_window(
        &mut self,
        start: Option<i64>,
        end_exclusive: Option<i64>,
    ) -> Result<usize> {
        let changed = self
            .conn
            .execute(
                "UPDATE watch_history
                 SET in_window = CASE WHEN (?1 IS NULL OR watched_at >= ?1)
                                       AND (?2 IS NULL OR watched_at < ?2)
                                  THEN 1 ELSE 0 END
                 WHERE in_window != CASE WHEN (?1 IS NULL OR watched_at >= ?1)
                                          AND (?2 IS NULL OR watched_at < ?2)
                                     THEN 1 ELSE 0 END",
                params![start, end_exclusive],
            )
            .context("recompute watch_history.in_window")?;
        Ok(changed)
    }

    /// Dry-run companion to [`Store::recompute_window`]: how many rows
    /// WOULD change under these bounds. Read-only.
    pub fn count_window_mismatches(
        &self,
        start: Option<i64>,
        end_exclusive: Option<i64>,
    ) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM watch_history
                 WHERE in_window != CASE WHEN (?1 IS NULL OR watched_at >= ?1)
                                          AND (?2 IS NULL OR watched_at < ?2)
                                     THEN 1 ELSE 0 END",
                params![start, end_exclusive],
                |r| r.get(0),
            )
            .context("count in_window mismatches")?;
        Ok(usize::try_from(n).unwrap_or(0))
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
                "SELECT video_id, status, canonical, source_url, first_seen_at, attempt_count,
                        terminal_reason, last_retryable_kind
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
                        terminal_reason: r.get(6)?,
                        last_retryable_kind: r.get(7)?,
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
