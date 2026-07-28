#![allow(clippy::unwrap_used, clippy::expect_used, clippy::type_complexity)]

//! Migration test: synthesize a v1 DB (no new Epic 2 columns; meta.schema_version='1'),
//! run the migrate function, confirm v2 columns are present and meta.schema_version
//! lands on the current SCHEMA_VERSION (v3 as of Epic 4a — the ladder walks v1→v2→v3
//! in one call). Then run Store::open and confirm it succeeds (round-trip with T2's
//! check). Also covers the v2→v3 leg directly: a hand-built v2-shaped DB migrating
//! to v3's `batch_runs` table + attempt-aware pending index.

use anyhow::Result;
use ddp_transcribe::state::{migrate::run_migrate, Store, SCHEMA_VERSION};
use rusqlite::Connection;
use tempfile::TempDir;

/// Synthesize a Plan A v1 schema (no Epic 2 columns) at `path`.
fn synthesize_v1_db(path: &std::path::Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;

         CREATE TABLE IF NOT EXISTS videos (
             video_id            TEXT PRIMARY KEY NOT NULL,
             source_url          TEXT NOT NULL,
             canonical           INTEGER NOT NULL,
             status              TEXT NOT NULL CHECK (status IN
                                   ('pending','in_progress','succeeded','failed_terminal','failed_retryable')),
             claimed_by          TEXT,
             claimed_at          INTEGER,
             attempt_count       INTEGER NOT NULL DEFAULT 0,
             succeeded_at        INTEGER,
             duration_s          REAL,
             language_detected   TEXT,
             fetcher             TEXT,
             transcript_source   TEXT,
             first_seen_at       INTEGER NOT NULL,
             updated_at          INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS watch_history (
             respondent_id  TEXT NOT NULL,
             video_id       TEXT NOT NULL,
             watched_at     INTEGER NOT NULL,
             in_window      INTEGER NOT NULL,
             PRIMARY KEY (respondent_id, video_id, watched_at),
             FOREIGN KEY (video_id) REFERENCES videos(video_id)
         );

         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL
         );

         INSERT INTO meta (key, value) VALUES ('schema_version', '1');
        ",
    )?;
    Ok(())
}

/// Synthesize a v2-shaped schema (Epic 2 columns present, no `batch_runs`
/// table, old `idx_videos_pending` index) at `path` — the pre-Epic-4a shape.
/// Copies `synthesize_v1_db`'s hand-built-SQL style, adding the four v2
/// columns to `videos` and recording `meta.schema_version = '2'`.
fn synthesize_v2_db(path: &std::path::Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;

         CREATE TABLE IF NOT EXISTS videos (
             video_id            TEXT PRIMARY KEY NOT NULL,
             source_url          TEXT NOT NULL,
             canonical           INTEGER NOT NULL,
             status              TEXT NOT NULL CHECK (status IN
                                   ('pending','in_progress','succeeded','failed_terminal','failed_retryable')),
             claimed_by          TEXT,
             claimed_at          INTEGER,
             attempt_count       INTEGER NOT NULL DEFAULT 0,
             succeeded_at        INTEGER,
             duration_s          REAL,
             language_detected   TEXT,
             fetcher             TEXT,
             transcript_source   TEXT,
             last_retryable_kind     TEXT,
             last_retryable_message  TEXT,
             terminal_reason         TEXT,
             terminal_message        TEXT,
             first_seen_at       INTEGER NOT NULL,
             updated_at          INTEGER NOT NULL
         );

         CREATE INDEX IF NOT EXISTS idx_videos_pending
             ON videos (status, first_seen_at, video_id)
             WHERE status = 'pending';

         CREATE TABLE IF NOT EXISTS watch_history (
             respondent_id  TEXT NOT NULL,
             video_id       TEXT NOT NULL,
             watched_at     INTEGER NOT NULL,
             in_window      INTEGER NOT NULL,
             PRIMARY KEY (respondent_id, video_id, watched_at),
             FOREIGN KEY (video_id) REFERENCES videos(video_id)
         );

         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL
         );

         INSERT INTO meta (key, value) VALUES ('schema_version', '2');
        ",
    )?;
    Ok(())
}

fn columns_in(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn migrate_v1_to_v2_adds_columns_and_bumps_version() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    synthesize_v1_db(&path)?;

    // Pre-migrate: confirm v1 shape.
    {
        let raw = Connection::open(&path)?;
        let cols = columns_in(&raw, "videos");
        assert!(
            !cols.contains(&"last_retryable_kind".to_string()),
            "v1 lacks new column"
        );
        let v: String = raw.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(v, "1");
    }

    run_migrate(&path)?;

    // Post-migrate: confirm v2 columns landed and the ladder walked all the
    // way to the current version (v3 as of Epic 4a).
    {
        let raw = Connection::open(&path)?;
        let cols = columns_in(&raw, "videos");
        assert!(cols.contains(&"last_retryable_kind".to_string()));
        assert!(cols.contains(&"last_retryable_message".to_string()));
        assert!(cols.contains(&"terminal_reason".to_string()));
        assert!(cols.contains(&"terminal_message".to_string()));
        let v: String = raw.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(v, SCHEMA_VERSION);
    }

    // Round-trip with T2's Store::open: should succeed now.
    let _store = Store::open(&path)?;
    Ok(())
}

#[test]
fn migrate_is_idempotent_on_v2() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    // Fresh DB at current SCHEMA_VERSION.
    let _ = Store::open(&path)?;

    // Migrate is a no-op when already at the current version.
    run_migrate(&path)?;
    run_migrate(&path)?; // second run also no-op

    let raw = Connection::open(&path)?;
    let v: String = raw.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v, SCHEMA_VERSION);
    Ok(())
}

/// Epic 4a: the v2→v3 leg in isolation. A v2-shaped DB (Epic 2 columns
/// present, no `batch_runs`, old `idx_videos_pending`) migrates to v3:
/// `batch_runs` exists, the old index is gone, the new attempt-aware index
/// exists under its new name, and a second `run_migrate` call is idempotent.
#[test]
fn migrate_v2_to_v3_adds_batch_runs_and_attempt_aware_index() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    synthesize_v2_db(&path)?;

    run_migrate(&path)?;

    {
        let raw = Connection::open(&path)?;

        let v: String = raw.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(v, SCHEMA_VERSION);

        let batch_run_count: i64 =
            raw.query_row("SELECT count(*) FROM batch_runs", [], |r| r.get(0))?;
        assert_eq!(batch_run_count, 0, "batch_runs exists and is empty");

        let mut stmt = raw.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_videos_pending%'",
        )?;
        let index_names = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        assert_eq!(
            index_names,
            vec!["idx_videos_pending_v3".to_string()],
            "old idx_videos_pending must be dropped; only the v3 index remains"
        );
    }

    // Idempotence: a second call is a no-op and version stays put.
    run_migrate(&path)?;
    let raw = Connection::open(&path)?;
    let v: String = raw.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v, SCHEMA_VERSION);

    Ok(())
}

#[test]
fn migrate_pre_plan_a_db_without_meta_row_records_current_version() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");

    // Synthesize a pre-Plan-A DB: v1 schema but no meta row at all.
    {
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS videos (
                 video_id            TEXT PRIMARY KEY NOT NULL,
                 source_url          TEXT NOT NULL,
                 canonical           INTEGER NOT NULL,
                 status              TEXT NOT NULL CHECK (status IN
                                       ('pending','in_progress','succeeded','failed_terminal','failed_retryable')),
                 claimed_by          TEXT,
                 claimed_at          INTEGER,
                 attempt_count       INTEGER NOT NULL DEFAULT 0,
                 succeeded_at        INTEGER,
                 duration_s          REAL,
                 language_detected   TEXT,
                 fetcher             TEXT,
                 transcript_source   TEXT,
                 first_seen_at       INTEGER NOT NULL,
                 updated_at          INTEGER NOT NULL
             );

             CREATE TABLE IF NOT EXISTS watch_history (
                 respondent_id  TEXT NOT NULL,
                 video_id       TEXT NOT NULL,
                 watched_at     INTEGER NOT NULL,
                 in_window      INTEGER NOT NULL,
                 PRIMARY KEY (respondent_id, video_id, watched_at),
                 FOREIGN KEY (video_id) REFERENCES videos(video_id)
             );

             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY NOT NULL,
                 value TEXT NOT NULL
             );
             -- no INSERT INTO meta — pre-Plan-A
            ",
        )?;
    }

    // Pre-migrate: confirm meta has no schema_version row.
    {
        let raw = Connection::open(&path)?;
        let count: i64 = raw.query_row(
            "SELECT COUNT(*) FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(count, 0, "pre-condition: no meta.schema_version row");
    }

    run_migrate(&path)?;

    // Post-migrate: confirm columns AND a recorded schema_version row exist.
    {
        let raw = Connection::open(&path)?;
        let cols = columns_in(&raw, "videos");
        assert!(cols.contains(&"last_retryable_kind".to_string()));
        let v: String = raw.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(v, SCHEMA_VERSION);
    }

    // Round-trip with Store::open should succeed (the whole point of the migrate
    // contract for this case).
    let _store = Store::open(&path)?;
    Ok(())
}

/// Item 3 — migration row-survival: pre-existing video rows with known column
/// values must survive the v1→v2 migration with all original values intact and
/// the four new nullable v2 columns defaulting to NULL. Pins the behavioral
/// contract that `ALTER TABLE ADD COLUMN` (with NULL default) does not silently
/// overwrite, truncate, or corrupt rows whose data predates the migration.
#[test]
fn migrate_preserves_existing_video_rows_with_null_new_columns() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    synthesize_v1_db(&path)?;

    // Insert two known rows before migrating. Use raw SQL so we control every
    // column value precisely — the v1 schema lacks the Epic 2 columns, so
    // `Store::upsert_video` (which opens a v2 Store) would fail.
    {
        let raw = Connection::open(&path)?;
        // Row 1: a succeeded video with all optional fields populated.
        raw.execute(
            "INSERT INTO videos
                 (video_id, source_url, canonical, status,
                  claimed_by, claimed_at, attempt_count,
                  succeeded_at, duration_s, language_detected,
                  fetcher, transcript_source, first_seen_at, updated_at)
             VALUES (?1, ?2, 1, 'succeeded',
                     NULL, NULL, 3,
                     1716237600, 45.7, 'en',
                     'ytdlp', 'whisper.cpp', 1716230000, 1716237600)",
            rusqlite::params![
                "7234567890123456789",
                "https://www.tiktok.com/@user/video/7234567890123456789",
            ],
        )?;
        // Row 2: a pending video with minimal optional fields.
        raw.execute(
            "INSERT INTO videos
                 (video_id, source_url, canonical, status,
                  claimed_by, claimed_at, attempt_count,
                  succeeded_at, duration_s, language_detected,
                  fetcher, transcript_source, first_seen_at, updated_at)
             VALUES (?1, ?2, 0, 'pending',
                     NULL, NULL, 0,
                     NULL, NULL, NULL,
                     NULL, NULL, 1716230001, 1716230001)",
            rusqlite::params![
                "9876543210987654321",
                "https://www.tiktok.com/@other/video/9876543210987654321",
            ],
        )?;
    }

    run_migrate(&path)?;

    // Post-migrate: all v1 column values must be intact; the four new v2
    // columns must be NULL (ALTER TABLE ADD COLUMN defaults to NULL).
    let raw = Connection::open(&path)?;

    // Row 1 — succeeded video.
    let (
        source_url,
        canonical,
        status,
        claimed_by,
        claimed_at,
        attempt_count,
        succeeded_at,
        duration_s,
        language_detected,
        fetcher,
        transcript_source,
        first_seen_at,
        updated_at,
        last_retryable_kind,
        last_retryable_message,
        terminal_reason,
        terminal_message,
    ): (
        String,
        i64,
        String,
        Option<String>,
        Option<i64>,
        i64,
        Option<i64>,
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = raw.query_row(
        "SELECT source_url, canonical, status, claimed_by, claimed_at, attempt_count,
                    succeeded_at, duration_s, language_detected, fetcher, transcript_source,
                    first_seen_at, updated_at,
                    last_retryable_kind, last_retryable_message,
                    terminal_reason, terminal_message
             FROM videos WHERE video_id = ?1",
        ["7234567890123456789"],
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
                r.get(12)?,
                r.get(13)?,
                r.get(14)?,
                r.get(15)?,
                r.get(16)?,
            ))
        },
    )?;

    assert_eq!(
        source_url, "https://www.tiktok.com/@user/video/7234567890123456789",
        "source_url must survive migration"
    );
    assert_eq!(canonical, 1, "canonical must survive migration");
    assert_eq!(status, "succeeded", "status must survive migration");
    assert_eq!(claimed_by, None, "claimed_by must survive migration (NULL)");
    assert_eq!(claimed_at, None, "claimed_at must survive migration (NULL)");
    assert_eq!(attempt_count, 3, "attempt_count must survive migration");
    assert_eq!(
        succeeded_at,
        Some(1716237600),
        "succeeded_at must survive migration"
    );
    assert!(
        (duration_s.unwrap() - 45.7).abs() < 1e-6,
        "duration_s must survive migration"
    );
    assert_eq!(
        language_detected.as_deref(),
        Some("en"),
        "language_detected must survive migration"
    );
    assert_eq!(
        fetcher.as_deref(),
        Some("ytdlp"),
        "fetcher must survive migration"
    );
    assert_eq!(
        transcript_source.as_deref(),
        Some("whisper.cpp"),
        "transcript_source must survive migration"
    );
    assert_eq!(
        first_seen_at, 1716230000,
        "first_seen_at must survive migration"
    );
    assert_eq!(updated_at, 1716237600, "updated_at must survive migration");
    // v2 columns must be NULL — the ADD COLUMN default.
    assert_eq!(
        last_retryable_kind, None,
        "last_retryable_kind must be NULL after migration on pre-existing row"
    );
    assert_eq!(
        last_retryable_message, None,
        "last_retryable_message must be NULL after migration on pre-existing row"
    );
    assert_eq!(
        terminal_reason, None,
        "terminal_reason must be NULL after migration on pre-existing row"
    );
    assert_eq!(
        terminal_message, None,
        "terminal_message must be NULL after migration on pre-existing row"
    );

    // Row 2 — pending video: spot-check v1 fields + v2 NULLs.
    let (status2, attempt_count2, lrk2, tr2): (String, i64, Option<String>, Option<String>) = raw
        .query_row(
        "SELECT status, attempt_count, last_retryable_kind, terminal_reason
             FROM videos WHERE video_id = ?1",
        ["9876543210987654321"],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    assert_eq!(status2, "pending", "row 2 status must survive migration");
    assert_eq!(
        attempt_count2, 0,
        "row 2 attempt_count must survive migration"
    );
    assert_eq!(
        lrk2, None,
        "row 2 last_retryable_kind must be NULL after migration"
    );
    assert_eq!(
        tr2, None,
        "row 2 terminal_reason must be NULL after migration"
    );

    Ok(())
}

/// Synthesize a v3-shaped schema (batch_runs + idx_videos_pending_v3
/// present, watch_history WITHOUT watched_at_raw) at `path` — the
/// pre-Epic-4b shape. Copies `synthesize_v2_db`'s hand-built-SQL style.
fn synthesize_v3_db(path: &std::path::Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;

         CREATE TABLE IF NOT EXISTS videos (
             video_id            TEXT PRIMARY KEY NOT NULL,
             source_url          TEXT NOT NULL,
             canonical           INTEGER NOT NULL,
             status              TEXT NOT NULL CHECK (status IN
                                   ('pending','in_progress','succeeded','failed_terminal','failed_retryable')),
             claimed_by          TEXT,
             claimed_at          INTEGER,
             attempt_count       INTEGER NOT NULL DEFAULT 0,
             succeeded_at        INTEGER,
             duration_s          REAL,
             language_detected   TEXT,
             fetcher             TEXT,
             transcript_source   TEXT,
             last_retryable_kind     TEXT,
             last_retryable_message  TEXT,
             terminal_reason         TEXT,
             terminal_message        TEXT,
             first_seen_at       INTEGER NOT NULL,
             updated_at          INTEGER NOT NULL
         );

         CREATE INDEX IF NOT EXISTS idx_videos_pending_v3
             ON videos (status, attempt_count, first_seen_at, video_id)
             WHERE status = 'pending';

         CREATE TABLE IF NOT EXISTS watch_history (
             respondent_id  TEXT NOT NULL,
             video_id       TEXT NOT NULL,
             watched_at     INTEGER NOT NULL,
             in_window      INTEGER NOT NULL,
             PRIMARY KEY (respondent_id, video_id, watched_at),
             FOREIGN KEY (video_id) REFERENCES videos(video_id)
         );

         CREATE TABLE IF NOT EXISTS batch_runs (
             run_id       INTEGER PRIMARY KEY,
             started_at   INTEGER NOT NULL,
             finished_at  INTEGER,
             params_json  TEXT NOT NULL,
             policy_toml  TEXT NOT NULL,
             census_json  TEXT
         );

         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL
         );

         INSERT INTO meta (key, value) VALUES ('schema_version', '3');
        ",
    )?;
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('7000000000000000111', 'https://example/7000000000000000111', 1, 'pending', 1, 1)",
        [],
    )?;
    conn.execute(
        "INSERT INTO watch_history (respondent_id, video_id, watched_at, in_window)
         VALUES ('w1', '7000000000000000111', 1000, 1)",
        [],
    )?;
    Ok(())
}

/// Epic 4b: the v3→v4 leg in isolation. A v3-shaped DB (no
/// `watched_at_raw` column, one pre-existing watch_history row) migrates
/// to v4: the column exists, pre-v4 rows carry NULL raw, and a second
/// `run_migrate` call is idempotent.
#[test]
fn migrate_upgrades_v3_to_v4_idempotently() {
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    synthesize_v3_db(&db).unwrap();

    run_migrate(&db).expect("v3→v4");
    run_migrate(&db).expect("idempotent second run");

    let conn = rusqlite::Connection::open(&db).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, ddp_transcribe::state::SCHEMA_VERSION);
    // Column exists and pre-v4 rows carry NULL raw.
    let raw: Option<String> = conn
        .query_row(
            "SELECT watched_at_raw FROM watch_history LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.is_none(), "pre-v4 rows must carry NULL watched_at_raw");
}
