#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::Result;
use ddp_transcribe::state::Store;
use tempfile::TempDir;

fn fresh_store() -> (Store, TempDir) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    (store, tmp)
}

fn seed_failed_retryable(
    store: &mut Store,
    video_id: &str,
    kind: &str,
    message: &str,
) -> Result<()> {
    store.upsert_video(video_id, "https://example.com/v", true)?;
    let _claim = store.claim_next("seed-worker")?;
    store.mark_retryable_failure(video_id, "seed-worker", kind, message)?;
    Ok(())
}

#[test]
fn sweep_mark_terminal_flips_only_failed_retryable() {
    use rusqlite::Connection;

    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(
        &mut store,
        "7000000000000000001",
        "Fetch",
        "ERROR: Your IP address is blocked",
    )
    .unwrap();

    let n = store
        .sweep_mark_terminal(
            "7000000000000000001",
            "IpBlockedMessage",
            "probe/message write-off",
        )
        .unwrap();
    assert_eq!(n, 1);

    // Second call: predicate misses (already terminal) → 0, no extra event.
    let n2 = store
        .sweep_mark_terminal("7000000000000000001", "IpBlockedMessage", "again")
        .unwrap();
    assert_eq!(n2, 0);

    let raw = Connection::open(&db_path).unwrap();
    let (status, reason, kept_kind): (String, String, Option<String>) = raw.query_row(
        "SELECT status, terminal_reason, last_retryable_kind FROM videos WHERE video_id='7000000000000000001'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(status, "failed_terminal");
    assert_eq!(reason, "IpBlockedMessage");
    assert_eq!(
        kept_kind.as_deref(),
        Some("Fetch"),
        "last_retryable_* preserved for audit"
    );

    let event_count: i64 = raw.query_row(
        "SELECT COUNT(*) FROM video_events WHERE video_id = '7000000000000000001' AND event_type = 'swept_terminal'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(event_count, 1);
}

#[test]
fn sweep_requeue_respects_attempt_cap_and_writes_kind_back() {
    use rusqlite::Connection;

    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed_retryable(
        &mut store,
        "7000000000000000002",
        "Fetch",
        "ERROR: Did not get any data blocks",
    )
    .unwrap();
    // seeded row has attempt_count = 1

    let n = store
        .sweep_requeue("7000000000000000002", "NoDataBlocks", 3)
        .unwrap();
    assert_eq!(n, 1);

    let raw = Connection::open(&db_path).unwrap();
    let (status, kind): (String, Option<String>) = raw
        .query_row(
            "SELECT status, last_retryable_kind FROM videos WHERE video_id='7000000000000000002'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(
        kind.as_deref(),
        Some("NoDataBlocks"),
        "requeue normalizes the kind"
    );

    let event_count: i64 = raw.query_row(
        "SELECT COUNT(*) FROM video_events WHERE video_id = '7000000000000000002' AND event_type = 'requeued'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(event_count, 1);

    // At the cap: attempt_count=1, max_attempts=1 → predicate misses.
    seed_failed_retryable(&mut store, "7000000000000000003", "Fetch", "msg").unwrap();
    let n2 = store
        .sweep_requeue("7000000000000000003", "NoDataBlocks", 1)
        .unwrap();
    assert_eq!(n2, 0);
}

#[test]
fn list_failed_retryable_returns_message_and_attempts() {
    let (mut store, _tmp) = fresh_store();
    seed_failed_retryable(
        &mut store,
        "7000000000000000004",
        "Fetch",
        "ERROR: whatever",
    )
    .unwrap();
    let rows = store.list_failed_retryable().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].video_id, "7000000000000000004");
    assert_eq!(rows[0].attempt_count, 1);
    assert!(rows[0]
        .last_retryable_message
        .as_deref()
        .unwrap()
        .contains("whatever"));
}
