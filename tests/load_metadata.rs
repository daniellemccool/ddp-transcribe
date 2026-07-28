#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `load-metadata` end-to-end: seeded raw envelopes → typed columns.
//! Public API only (Store::open + the pub upserts) plus raw rusqlite
//! readback, so this file needs no `[[test]]` block per 0005.

use assert_cmd::Command as AssertCommand;

/// The typed-column readback tuple: description, uploader, created-at,
/// view count, metadata snapshot time.
type LoadedColumns = (
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn seeded_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let db = dir.path().join("state.sqlite");
    let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
    store
        .upsert_video("vid_a", "https://example/a", false)
        .unwrap();
    store
        .upsert_video("vid_b", "https://example/b", false)
        .unwrap();
    store
        .upsert_metadata_raw(
            "vid_a",
            r#"{"schema":1,"printed":"{\"id\":\"vid_a\",\"description\":\"desc A\",\"uploader\":\"acct\",\"timestamp\":1768924271,\"view_count\":42}"}"#,
        )
        .unwrap();
    store
        .upsert_metadata_raw("vid_b", "definitely not json")
        .unwrap();
    db
}

#[test]
fn load_metadata_populates_columns_and_reports_stats() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("examined 2"), "stdout was: {out}");
    assert!(out.contains("loaded 1") && out.contains("skipped-unparseable 1"));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let (desc, uploader, created, views, fetched): LoadedColumns = conn
        .query_row(
            "SELECT video_description, uploader, video_created_at, view_count, metadata_fetched_at
             FROM videos WHERE video_id='vid_a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(desc.as_deref(), Some("desc A"));
    assert_eq!(uploader.as_deref(), Some("acct"));
    assert_eq!(created, Some(1_768_924_271));
    assert_eq!(views, Some(42));
    assert!(
        fetched.is_some(),
        "metadata_fetched_at stamped from raw row"
    );

    // vid_b (unparseable) untouched.
    let b_desc: Option<String> = conn
        .query_row(
            "SELECT video_description FROM videos WHERE video_id='vid_b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(b_desc.is_none());
}

#[test]
fn load_metadata_is_idempotent_and_replayable() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    for _ in 0..2 {
        AssertCommand::cargo_bin("ddp-transcribe")
            .unwrap()
            .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
            .assert()
            .success();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    let desc: Option<String> = conn
        .query_row(
            "SELECT video_description FROM videos WHERE video_id='vid_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        desc.as_deref(),
        Some("desc A"),
        "second run reproduces, not corrupts"
    );
}

#[test]
fn load_metadata_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "load-metadata",
            "--dry-run",
        ])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("dry-run"), "stdout was: {out}");
    // The dry-run pass still reports real counts.
    assert!(out.contains("examined 2") && out.contains("loaded 1"));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let desc: Option<String> = conn
        .query_row(
            "SELECT video_description FROM videos WHERE video_id='vid_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(desc.is_none(), "--dry-run must not write");
}

/// A parsed blob whose `videos` row is gone matches 0 rows: counted as
/// `rows_without_video`, never an error. Only reachable with FK
/// enforcement off (the raw table has FK→videos), so the orphan is seeded
/// on a separate connection with the pragma disabled.
#[test]
fn load_metadata_counts_orphan_raw_rows_without_failing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    {
        let _store = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let seed = rusqlite::Connection::open(&db).unwrap();
    seed.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    seed.execute(
        "INSERT INTO video_metadata_raw (video_id, fetched_at, raw_json) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            "vid_orphan",
            1_700_000_000_i64,
            r#"{"schema":1,"printed":"{\"id\":\"vid_orphan\",\"description\":\"orphan\"}"}"#
        ],
    )
    .unwrap();
    drop(seed);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        out.contains("examined 1") && out.contains("loaded 0") && out.contains("without-video 1"),
        "stdout was: {out}"
    );
}

/// `Store::metadata_raw_page` cursor semantics span two statements (the
/// first-page/no-cursor branch and the subsequent-page/`WHERE video_id >
/// ?1` branch) — walking every page must still visit each row exactly
/// once, in ascending video_id order, proving both branches agree.
#[test]
fn metadata_raw_page_walks_all_rows_exactly_once_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let mut store = ddp_transcribe::state::Store::open(&db).unwrap();

    let ids = ["vid_a", "vid_b", "vid_c", "vid_d", "vid_e"];
    for id in ids {
        store.upsert_video(id, "https://example/v", false).unwrap();
        store
            .upsert_metadata_raw(id, r#"{"schema":1,"printed":"{}"}"#)
            .unwrap();
    }

    let mut seen = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = store.metadata_raw_page(after.as_deref(), 2).unwrap();
        if page.is_empty() {
            break;
        }
        after = Some(page.last().unwrap().video_id.clone());
        seen.extend(page.into_iter().map(|r| r.video_id));
    }

    let mut expected = ids.to_vec();
    expected.sort_unstable();
    assert_eq!(
        seen, expected,
        "union of pages == all rows, ascending, no dupes"
    );
}

#[test]
fn load_metadata_refuses_missing_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("nope.sqlite");
    AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
        .assert()
        .failure();
    assert!(!db.exists(), "must not create an empty DB");
}
