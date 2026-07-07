#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::state::{FailureRecordOutcome, Store};
use tempfile::TempDir;

fn store_with_claimed_row(tmp: &TempDir, attempts_before_claim: i64) -> (Store, String) {
    let mut store = Store::open(&tmp.path().join("state.sqlite")).expect("open");
    store
        .upsert_video("vid_a", "https://example/a", false)
        .unwrap();
    if attempts_before_claim > 0 {
        let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
        conn.execute(
            "UPDATE videos SET attempt_count = ?1 WHERE video_id = 'vid_a'",
            [attempts_before_claim],
        )
        .unwrap();
    }
    let claim = store.claim_next("w1").unwrap().expect("claim");
    (store, claim.video_id)
}

fn status_of(tmp: &TempDir, id: &str) -> (String, Option<String>, i64) {
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    conn.query_row(
        "SELECT status, last_retryable_kind, attempt_count FROM videos WHERE video_id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

#[test]
fn under_cap_requeues_to_pending_unowned() {
    let tmp = TempDir::new().unwrap();
    // attempt_count is now 1 (claim bumped 0→1); cap 2 ⇒ retry budget remains.
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "w1", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Requeued));
    let (status, kind, attempts) = status_of(&tmp, &id);
    assert_eq!(status, "pending");
    assert_eq!(kind.as_deref(), Some("NoDataBlocks"));
    assert_eq!(
        attempts, 1,
        "mutator must NOT bump attempts (claim_next owns that)"
    );
    // Pending rows must be unowned.
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let claimed_by: Option<String> = conn
        .query_row(
            "SELECT claimed_by FROM videos WHERE video_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(claimed_by.is_none());
}

#[test]
fn at_cap_lands_in_exhausted_pool() {
    let tmp = TempDir::new().unwrap();
    // Seeded at 1, claim bumps to 2; cap 2 ⇒ attempt_count < 2 is false.
    let (mut store, id) = store_with_claimed_row(&tmp, 1);
    let out = store
        .record_fetch_failure(&id, "w1", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Exhausted));
    assert_eq!(status_of(&tmp, &id).0, "failed_retryable");
}

#[test]
fn requires_cookie_without_cookies_parks_regardless_of_budget() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "w1", "SensitiveLoginGated", "msg", 2, true, false)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::ParkedForCookies));
    let (status, kind, _) = status_of(&tmp, &id);
    assert_eq!(status, "failed_retryable");
    assert_eq!(kind.as_deref(), Some("SensitiveLoginGated"));
}

#[test]
fn requires_cookie_with_cookies_requeues() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(&id, "w1", "SensitiveLoginGated", "msg", 2, true, true)
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::Requeued));
    assert_eq!(status_of(&tmp, &id).0, "pending");
}

#[test]
fn stale_claim_mutates_nothing() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    let out = store
        .record_fetch_failure(
            &id,
            "DIFFERENT-WORKER",
            "NoDataBlocks",
            "msg",
            2,
            false,
            false,
        )
        .unwrap();
    assert!(matches!(out, FailureRecordOutcome::StaleClaim));
    assert_eq!(status_of(&tmp, &id).0, "in_progress", "row untouched");
}

#[test]
fn events_record_each_outcome() {
    let tmp = TempDir::new().unwrap();
    let (mut store, id) = store_with_claimed_row(&tmp, 0);
    store
        .record_fetch_failure(&id, "w1", "NoDataBlocks", "msg", 2, false, false)
        .unwrap();
    let events = store.get_events_for_test(&id).unwrap();
    assert!(
        events.iter().any(|e| e.event_type == "retry_requeued"),
        "requeue must leave a retry_requeued event; got {events:?}"
    );
}
