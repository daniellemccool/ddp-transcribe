#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `Store::upsert_metadata_raw` — Epic 4c raw envelope storage.
//! Public-API only (Store::open + raw rusqlite): auto-discovered, no
//! Cargo.toml [[test]] block per ADR-0005.

use ddp_transcribe::state::Store;

/// Seeds one video and claims it for `worker-1`, which is the state the
/// fetch path is in when it writes the raw envelope (the write is
/// claim-guarded per 0023 since the Epic 5b hygiene bundle).
fn store_with_claimed_video(dir: &tempfile::TempDir) -> (Store, std::path::PathBuf) {
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store
        .upsert_video("vid_a", "https://example/a", false)
        .unwrap();
    let claim = store.claim_next("worker-1").unwrap().expect("claim");
    assert_eq!(claim.video_id, "vid_a");
    (store, db)
}

#[test]
fn upsert_metadata_raw_inserts_and_returns_one() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, db) = store_with_claimed_video(&dir);

    let n = store
        .upsert_metadata_raw("vid_a", "worker-1", r#"{"schema":1,"printed":"{}"}"#)
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
    let (mut store, db) = store_with_claimed_video(&dir);

    store
        .upsert_metadata_raw("vid_a", "worker-1", r#"{"schema":1,"printed":"first"}"#)
        .unwrap();
    let n = store
        .upsert_metadata_raw("vid_a", "worker-1", r#"{"schema":1,"printed":"second"}"#)
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

/// Epic 5b hygiene bundle: the fetch-path envelope write carries the same
/// `claimed_by` predicate as every other in-flight mutator (0023). A worker
/// whose claim was swept out from under it and re-taken by someone else must
/// not overwrite the newer envelope with its older one.
#[test]
fn upsert_metadata_raw_rejects_a_stale_claim() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, db) = store_with_claimed_video(&dir);

    // worker-1 holds the claim: its write lands.
    let n = store
        .upsert_metadata_raw("vid_a", "worker-1", r#"{"schema":1,"printed":"older"}"#)
        .unwrap();
    assert_eq!(n, 1);

    // The claim goes stale, is swept, and is re-taken by worker-2, whose
    // fresher envelope lands.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE videos SET claimed_at = 1000 WHERE video_id = 'vid_a'",
        [],
    )
    .unwrap();
    let recovered = store
        .sweep_stale_claims(std::time::Duration::from_secs(1800))
        .unwrap();
    assert_eq!(recovered, 1);
    let reclaim = store.claim_next("worker-2").unwrap().expect("reclaim");
    assert_eq!(reclaim.video_id, "vid_a");
    store
        .upsert_metadata_raw("vid_a", "worker-2", r#"{"schema":1,"printed":"newer"}"#)
        .unwrap();

    // worker-1 finishes its long fetch and tries to write: guard rejects it.
    let stale = store
        .upsert_metadata_raw("vid_a", "worker-1", r#"{"schema":1,"printed":"stale"}"#)
        .unwrap();
    assert_eq!(stale, 0, "stale claim must change no rows (0006 contract)");

    let raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id='vid_a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        raw.contains("newer"),
        "the newer envelope survives a stale worker's write: {raw}"
    );
}

/// An unclaimed row has no in-flight worker at all, so no envelope write is
/// legitimate through this (fetch-path) mutator. `backfill-metadata` writes
/// unclaimed rows through `insert_metadata_raw_if_missing` instead (0042).
#[test]
fn upsert_metadata_raw_writes_nothing_for_an_unclaimed_row() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store
        .upsert_video("vid_a", "https://example/a", false)
        .unwrap();

    let n = store
        .upsert_metadata_raw("vid_a", "worker-1", r#"{"schema":1,"printed":"{}"}"#)
        .unwrap();
    assert_eq!(n, 0);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_metadata_raw", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "no claim, no envelope");
}
