//! Migration test: synthesize a v1 DB (no new Epic 2 columns; meta.schema_version='1'),
//! run the migrate function, confirm v2 columns are present and meta.schema_version='2'.
//! Then run Store::open and confirm it succeeds (round-trip with T2's check).

use anyhow::Result;
use ddp_transcribe::state::{migrate::run_migrate, Store};
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

         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL
         );

         INSERT INTO meta (key, value) VALUES ('schema_version', '1');
        ",
    )?;
    Ok(())
}

fn columns_in(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
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

    // Post-migrate: confirm v2 shape.
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
        assert_eq!(v, "2");
    }

    // Round-trip with T2's Store::open: should succeed now.
    let _store = Store::open(&path)?;
    Ok(())
}

#[test]
fn migrate_is_idempotent_on_v2() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    // Fresh DB at current SCHEMA_VERSION (v2 after T4).
    let _ = Store::open(&path)?;

    // Migrate is a no-op on v2.
    run_migrate(&path)?;
    run_migrate(&path)?; // second run also no-op

    let raw = Connection::open(&path)?;
    let v: String = raw.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v, "2");
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
        assert_eq!(v, "2");
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
