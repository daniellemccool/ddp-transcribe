#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Confirm the v2 schema: new nullable columns present on `videos`,
//! SCHEMA_VERSION constant is "2", fresh DB records "2" in meta.

use anyhow::Result;
use ddp_transcribe::state::{Store, SCHEMA_VERSION};
use rusqlite::Connection;
use tempfile::TempDir;

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
fn schema_version_constant_is_v2() {
    assert_eq!(SCHEMA_VERSION, "2");
}

#[test]
fn fresh_db_has_new_nullable_columns() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let _ = Store::open(&path)?;

    let raw = Connection::open(&path)?;
    let cols = columns_in(&raw, "videos");
    for expected in [
        "last_retryable_kind",
        "last_retryable_message",
        "terminal_reason",
        "terminal_message",
    ] {
        assert!(
            cols.contains(&expected.to_string()),
            "expected column `{expected}` in videos: have {cols:?}"
        );
    }

    let v: String = raw.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v, "2");
    Ok(())
}

#[test]
fn new_columns_default_to_null() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let raw = Connection::open(&path)?;
    let (rk, rm, tr, tm): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = raw.query_row(
        "SELECT last_retryable_kind, last_retryable_message, terminal_reason, terminal_message
             FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    assert_eq!(rk, None);
    assert_eq!(rm, None);
    assert_eq!(tr, None);
    assert_eq!(tm, None);
    Ok(())
}
