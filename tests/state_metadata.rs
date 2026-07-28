#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `Store::upsert_metadata_raw` — Epic 4c raw envelope storage.
//! Public-API only (Store::open + raw rusqlite): auto-discovered, no
//! Cargo.toml [[test]] block per ADR-0005.

use ddp_transcribe::state::Store;

fn store_with_video(dir: &tempfile::TempDir) -> (Store, std::path::PathBuf) {
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store
        .upsert_video("vid_a", "https://example/a", false)
        .unwrap();
    (store, db)
}

#[test]
fn upsert_metadata_raw_inserts_and_returns_one() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, db) = store_with_video(&dir);

    let n = store
        .upsert_metadata_raw("vid_a", r#"{"schema":1,"printed":"{}","captions":null}"#)
        .unwrap();
    assert_eq!(n, 1);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let (raw, fetched_at): (String, i64) = conn
        .query_row(
            "SELECT raw_json, fetched_at FROM video_metadata_raw WHERE video_id='vid_a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(raw.contains(r#""schema":1"#));
    assert!(fetched_at > 0);
}

#[test]
fn upsert_metadata_raw_overwrites_last_write_wins() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, db) = store_with_video(&dir);

    store
        .upsert_metadata_raw("vid_a", r#"{"schema":1,"printed":"first","captions":null}"#)
        .unwrap();
    let n = store
        .upsert_metadata_raw(
            "vid_a",
            r#"{"schema":1,"printed":"second","captions":null}"#,
        )
        .unwrap();
    assert_eq!(n, 1);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_metadata_raw", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "keyed upsert: one row per video");
    let raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id='vid_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.contains("second"), "last write wins");
}
