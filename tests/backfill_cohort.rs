#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Cohort queries + insert-if-missing for backfill-metadata: succeeded
//! videos with no video_metadata_raw row. Public API only (Store::open +
//! pub upserts) plus raw rusqlite status flips, so this file needs no
//! `[[test]]` block per 0005.

use ddp_transcribe::state::Store;

/// Five videos: v1 succeeded+envelope, v2 succeeded (no envelope),
/// v3 pending, v4 succeeded (no envelope), v5 failed_terminal.
/// Cohort is exactly {v2, v4}.
fn seeded_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let db = dir.path().join("state.sqlite");
    {
        let mut store = Store::open(&db).unwrap();
        for (id, url) in [
            ("v1", "https://example/1"),
            ("v2", "https://example/2"),
            ("v3", "https://example/3"),
            ("v4", "https://example/4"),
            ("v5", "https://example/5"),
        ] {
            store.upsert_video(id, url, false).unwrap();
        }
        store
            .upsert_metadata_raw("v1", r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#)
            .unwrap();
    }
    // Flip statuses with raw rusqlite — no public mutator sets
    // `succeeded` without a claim, and tests must not grow one.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE videos SET status = 'succeeded' WHERE video_id IN ('v1','v2','v4')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE videos SET status = 'failed_terminal' WHERE video_id = 'v5'",
        [],
    )
    .unwrap();
    db
}

#[test]
fn cohort_is_succeeded_without_envelope_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let store = Store::open(&db).unwrap();

    assert_eq!(store.count_succeeded_missing_metadata().unwrap(), 2);
    let page = store.succeeded_missing_metadata_page(None, 100).unwrap();
    let ids: Vec<&str> = page.iter().map(|v| v.video_id.as_str()).collect();
    assert_eq!(
        ids,
        ["v2", "v4"],
        "ordered by video_id; excludes enveloped/pending/terminal"
    );
    assert_eq!(page[0].source_url, "https://example/2");
}

#[test]
fn cohort_page_walks_all_rows_exactly_once_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let store = Store::open(&db).unwrap();

    // Page size 1 forces the cursor across both cached statements.
    let mut after: Option<String> = None;
    let mut walked = Vec::new();
    loop {
        let page = store
            .succeeded_missing_metadata_page(after.as_deref(), 1)
            .unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.video_id.clone());
        walked.extend(page.into_iter().map(|v| v.video_id));
    }
    assert_eq!(walked, ["v2", "v4"]);
}

#[test]
fn insert_if_missing_never_overwrites_and_shrinks_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let mut store = Store::open(&db).unwrap();

    // Fresh insert: 1 row changed, cohort shrinks.
    let changed = store
        .insert_metadata_raw_if_missing("v2", r#"{"schema":1,"printed":"{\"id\":\"v2\"}"}"#)
        .unwrap();
    assert_eq!(changed, 1);
    assert_eq!(store.count_succeeded_missing_metadata().unwrap(), 1);
    let page = store.succeeded_missing_metadata_page(None, 100).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].video_id, "v4");

    // Conflict: 0 rows changed, existing envelope untouched.
    let changed = store
        .insert_metadata_raw_if_missing("v1", r#"{"schema":1,"printed":"{\"id\":\"OVERWRITE\"}"}"#)
        .unwrap();
    assert_eq!(changed, 0, "existing row wins; backfill never overwrites");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.contains(r#"\"id\":\"v1\""#), "raw was: {raw}");
}
