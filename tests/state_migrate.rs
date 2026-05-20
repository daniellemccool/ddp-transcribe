//! Migration test: synthesize a v1 DB (no new Epic 2 columns; meta.schema_version='1'),
//! run the migrate function, confirm v2 columns are present and meta.schema_version='2'.
//! Then run Store::open and confirm it succeeds (round-trip with T2's check).

use anyhow::Result;
use rusqlite::Connection;
use tempfile::TempDir;
use uu_tiktok::state::{migrate::run_migrate, Store};

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
