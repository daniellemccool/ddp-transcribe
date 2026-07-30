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

/// Claims `video_id` for `worker_id`, then backdates `claimed_at` through a
/// raw connection so the row reads as stale to `sweep_stale_claims`.
fn seed_stale_claim(store: &mut Store, db_path: &std::path::Path, video_id: &str, worker_id: &str) {
    use rusqlite::Connection;

    store
        .upsert_video(video_id, "https://example.com/v", true)
        .unwrap();
    let claim = store.claim_next(worker_id).unwrap().expect("claim");
    assert_eq!(claim.video_id, video_id, "claim_next took the seeded row");

    let raw = Connection::open(db_path).unwrap();
    raw.execute(
        "UPDATE videos SET claimed_at = ?1 WHERE video_id = ?2",
        rusqlite::params![1_000_i64, video_id], // 1970 — definitely stale
    )
    .unwrap();
}

#[test]
fn sweep_writes_one_swept_stale_event_per_recovered_row() {
    use rusqlite::Connection;
    use std::time::Duration;

    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");

    seed_stale_claim(&mut store, &db_path, "7000000000000000010", "host-999");
    seed_stale_claim(&mut store, &db_path, "7000000000000000011", "host-999");

    // A freshly claimed row the sweep must not touch.
    store
        .upsert_video("7000000000000000012", "https://example.com/v", true)
        .unwrap();
    store.claim_next("host-fresh").unwrap().expect("claim");

    let recovered = store.sweep_stale_claims(Duration::from_secs(1800)).unwrap();
    assert_eq!(recovered, 2, "sweep return contract unchanged");

    let raw = Connection::open(&db_path).unwrap();
    let mut stmt = raw
        .prepare(
            "SELECT video_id, detail_json FROM video_events
             WHERE event_type = 'swept_stale' AND worker_id = 'sweep'
             ORDER BY video_id ASC",
        )
        .unwrap();
    let events: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        events.len(),
        2,
        "one swept_stale event per recovered row: {events:?}"
    );

    for (video_id, detail_json) in &events {
        assert!(
            video_id == "7000000000000000010" || video_id == "7000000000000000011",
            "unexpected swept_stale subject {video_id}"
        );
        let detail: serde_json::Value =
            serde_json::from_str(detail_json.as_deref().expect("detail_json present")).unwrap();
        assert_eq!(
            detail["was_claimed_by"], "host-999",
            "stale claim provenance is preserved"
        );
        assert!(
            detail["claimed_at"].is_number(),
            "claimed_at recorded numerically: {detail}"
        );
        assert_eq!(detail["threshold_secs"], 1800);
    }

    let fresh_events: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM video_events
             WHERE video_id = '7000000000000000012' AND event_type = 'swept_stale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fresh_events, 0, "fresh claim gets no swept_stale event");
}

#[test]
fn sweep_with_nothing_stale_writes_no_events() {
    use rusqlite::Connection;
    use std::time::Duration;

    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");

    store
        .upsert_video("7000000000000000013", "https://example.com/v", true)
        .unwrap();
    store.claim_next("host-fresh").unwrap().expect("claim");

    let recovered = store.sweep_stale_claims(Duration::from_secs(1800)).unwrap();
    assert_eq!(recovered, 0, "fresh claim is not stale");

    let raw = Connection::open(&db_path).unwrap();
    let event_count: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM video_events WHERE event_type = 'swept_stale'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 0, "no recovered rows → no events");
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
