use anyhow::Result;
use tempfile::TempDir;
use uu_tiktok::state::{Claim, Store, SuccessArtifacts};

fn fresh_store_with(videos: &[(&str, &str)]) -> (TempDir, Store) {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    for (id, url) in videos {
        store.upsert_video(id, url, true).unwrap();
    }
    (tmp, store)
}

#[test]
fn claim_next_returns_none_on_empty_db() {
    let (_tmp, mut store) = fresh_store_with(&[]);
    let claim = store.claim_next("worker-1").unwrap();
    assert!(claim.is_none());
}

#[test]
fn claim_next_returns_pending_video_and_marks_in_progress() {
    let (_tmp, mut store) = fresh_store_with(&[("7234567890123456789", "url")]);

    let claim = store.claim_next("worker-1").unwrap().expect("claim");
    assert_eq!(claim.video_id, "7234567890123456789");
    assert_eq!(claim.source_url, "url");

    let row = store
        .get_video_for_test("7234567890123456789")
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "in_progress");
    assert_eq!(row.attempt_count, 1, "attempt_count incremented on claim");
}

#[test]
fn claim_next_orders_by_first_seen_at() {
    let (_tmp, mut store) = fresh_store_with(&[]);
    store
        .upsert_video("7234567890123456789", "first", true)
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    store
        .upsert_video("7234567890123456788", "second", true)
        .unwrap();

    let first_claim = store.claim_next("w").unwrap().unwrap();
    assert_eq!(first_claim.video_id, "7234567890123456789");
}

#[test]
fn mark_succeeded_writes_status_and_event_in_one_transaction() {
    let (_tmp, mut store) = fresh_store_with(&[("7234567890123456789", "url")]);
    let claim = store.claim_next("w").unwrap().unwrap();
    assert_eq!(claim.video_id, "7234567890123456789");

    let artifacts = SuccessArtifacts {
        duration_s: Some(23.4),
        language_detected: Some("en".into()),
        fetcher: "ytdlp",
        transcript_source: "whisper.cpp",
    };
    store
        .mark_succeeded(&claim.video_id, "w", artifacts)
        .unwrap();

    let row = store.get_video_for_test(&claim.video_id).unwrap().unwrap();
    assert_eq!(row.status, "succeeded");
    let events = store.get_events_for_test(&claim.video_id).unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(kinds.contains(&"claimed"), "claimed event recorded");
    assert!(kinds.contains(&"succeeded"), "succeeded event recorded");
}

#[test]
fn concurrent_claim_serializes_via_begin_immediate() {
    // Two distinct connections to the same DB. claim_next must atomically
    // pick a row so only one connection succeeds for a given pending row.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("state.sqlite");

    let mut store_a = Store::open(&path).unwrap();
    let mut store_b = Store::open(&path).unwrap();

    store_a
        .upsert_video("7234567890123456789", "url", true)
        .unwrap();

    let a = store_a.claim_next("worker-a").unwrap();
    let b = store_b.claim_next("worker-b").unwrap();

    let claimed: Vec<&Claim> = [a.as_ref(), b.as_ref()].into_iter().flatten().collect();
    assert_eq!(claimed.len(), 1, "exactly one connection wins the row");
}

#[test]
fn mark_succeeded_with_stale_claim_returns_zero_and_does_not_update() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;

    store.upsert_video("vid_a", "https://example/a", false)?;

    let claim = store.claim_next("worker-1")?.expect("first claim succeeds");
    assert_eq!(claim.video_id, "vid_a");

    // Simulate a different worker calling mark_succeeded with the wrong
    // worker_id (stale claim): the predicate rejects, returns 0.
    let artifacts = SuccessArtifacts {
        duration_s: Some(1.0),
        language_detected: Some("en".to_string()),
        fetcher: "fake",
        transcript_source: "fake",
    };
    let changed = store.mark_succeeded("vid_a", "worker-DIFFERENT", artifacts)?;
    assert_eq!(changed, 0, "stale-claim mark_succeeded should not update");

    // Row should still be in_progress, still claimed by worker-1.
    let row = store
        .get_video_for_test("vid_a")?
        .expect("row still present");
    assert_eq!(row.status, "in_progress");

    Ok(())
}

#[test]
fn mark_retryable_failure_flips_status_and_records_columns() -> anyhow::Result<()> {
    use rusqlite::Connection;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let claim = store.claim_next("worker-1")?.expect("claim");

    let changed = store.mark_retryable_failure(
        &claim.video_id,
        "worker-1",
        "FetchTimeout",
        "yt-dlp exceeded 300s budget",
    )?;
    assert_eq!(changed, 1);

    let raw = Connection::open(&path)?;
    let (status, rk, rm): (String, Option<String>, Option<String>) = raw.query_row(
        "SELECT status, last_retryable_kind, last_retryable_message
         FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(status, "failed_retryable");
    assert_eq!(rk.as_deref(), Some("FetchTimeout"));
    assert_eq!(rm.as_deref(), Some("yt-dlp exceeded 300s budget"));

    // Core retry-safety invariant: claim slot must be cleared so the
    // row is re-claimable by claim_next without operator intervention.
    let (claimed_by, claimed_at): (Option<String>, Option<i64>) = raw.query_row(
        "SELECT claimed_by, claimed_at FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(
        claimed_by, None,
        "claimed_by must be NULL after retryable flip"
    );
    assert_eq!(
        claimed_at, None,
        "claimed_at must be NULL after retryable flip"
    );
    Ok(())
}

#[test]
fn mark_retryable_failure_with_stale_claim_returns_zero() -> anyhow::Result<()> {
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.claim_next("worker-1")?.expect("claim");

    // Stale: different worker tries to mark.
    let changed =
        store.mark_retryable_failure("vid_a", "worker-OTHER", "FetchTimeout", "spurious")?;
    assert_eq!(changed, 0, "stale-claim should be rejected by predicate");

    let raw = rusqlite::Connection::open(&path)?;
    let (status, rk, rm, cb): (String, Option<String>, Option<String>, Option<String>) = raw
        .query_row(
            "SELECT status, last_retryable_kind, last_retryable_message, claimed_by
         FROM videos WHERE video_id = ?1",
            ["vid_a"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    assert_eq!(
        status, "in_progress",
        "stale-claim must leave status unchanged"
    );
    assert_eq!(
        rk, None,
        "retryable kind must not be written on stale claim"
    );
    assert_eq!(
        rm, None,
        "retryable message must not be written on stale claim"
    );
    assert_eq!(
        cb.as_deref(),
        Some("worker-1"),
        "original claim must be preserved"
    );
    Ok(())
}

#[test]
fn claim_then_mark_succeeded_then_reclaim_returns_none() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let claim = store.claim_next("worker-1")?.expect("first claim");
    let artifacts = SuccessArtifacts {
        duration_s: Some(1.0),
        language_detected: Some("en".to_string()),
        fetcher: "fake",
        transcript_source: "fake",
    };
    let changed = store.mark_succeeded(&claim.video_id, "worker-1", artifacts)?;
    assert_eq!(changed, 1);

    let second = store.claim_next("worker-1")?;
    assert!(second.is_none(), "round-trip: no pending rows left");
    Ok(())
}

#[test]
fn mark_terminal_failure_flips_status_and_records_columns() -> anyhow::Result<()> {
    use rusqlite::Connection;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let _claim = store.claim_next("worker-1")?.expect("claim");

    let changed = store.mark_terminal_failure(
        "vid_a",
        "worker-1",
        "VideoUnavailable",
        "yt-dlp returned 410 Gone",
    )?;
    assert_eq!(changed, 1);

    let raw = Connection::open(&path)?;
    let (status, tr, tm): (String, Option<String>, Option<String>) = raw.query_row(
        "SELECT status, terminal_reason, terminal_message
         FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    assert_eq!(status, "failed_terminal");
    assert_eq!(tr.as_deref(), Some("VideoUnavailable"));
    assert_eq!(tm.as_deref(), Some("yt-dlp returned 410 Gone"));

    // Core retry-safety invariant: claim slot cleared so the row can't be
    // re-claimed against a stale worker_id. (Symmetric with the T6
    // mark_retryable_failure happy-path test.)
    let (claimed_by, claimed_at): (Option<String>, Option<i64>) = raw.query_row(
        "SELECT claimed_by, claimed_at FROM videos WHERE video_id = ?1",
        ["vid_a"],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(
        claimed_by, None,
        "claimed_by must be NULL after terminal flip"
    );
    assert_eq!(
        claimed_at, None,
        "claimed_at must be NULL after terminal flip"
    );

    // Verify the gated video_events INSERT fired with the expected shape.
    let (event_type, evt_worker, detail): (String, Option<String>, Option<String>) = raw
        .query_row(
            "SELECT event_type, worker_id, detail_json
         FROM video_events
         WHERE video_id = ?1 AND event_type = 'failed_terminal'",
            ["vid_a"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
    assert_eq!(event_type, "failed_terminal");
    assert_eq!(evt_worker.as_deref(), Some("worker-1"));
    let detail = detail.expect("detail_json populated");
    assert!(
        detail.contains("VideoUnavailable"),
        "detail JSON includes reason: {detail}"
    );
    assert!(
        detail.contains("yt-dlp returned 410 Gone"),
        "detail JSON includes message: {detail}"
    );

    Ok(())
}

#[test]
fn mark_terminal_failure_with_stale_claim_returns_zero() -> anyhow::Result<()> {
    use rusqlite::Connection;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.claim_next("worker-1")?.expect("claim");

    let changed =
        store.mark_terminal_failure("vid_a", "worker-OTHER", "VideoUnavailable", "spurious")?;
    assert_eq!(changed, 0);

    // Stale-claim must leave the row untouched (T6-review carry-forward).
    let raw = Connection::open(&path)?;
    let (status, tr, tm, cb): (String, Option<String>, Option<String>, Option<String>) = raw
        .query_row(
            "SELECT status, terminal_reason, terminal_message, claimed_by
             FROM videos WHERE video_id = ?1",
            ["vid_a"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    assert_eq!(
        status, "in_progress",
        "stale-claim must leave status unchanged"
    );
    assert_eq!(
        tr, None,
        "terminal_reason must not be written on stale claim"
    );
    assert_eq!(
        tm, None,
        "terminal_message must not be written on stale claim"
    );
    assert_eq!(
        cb.as_deref(),
        Some("worker-1"),
        "original claim must be preserved"
    );

    Ok(())
}

#[test]
fn sweep_stale_claims_recovers_stale_row() -> anyhow::Result<()> {
    use rusqlite::Connection;
    use std::time::Duration;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let _claim = store.claim_next("worker-crashed")?.expect("claim");

    // Backdate claimed_at via a raw UPDATE so the row appears stale.
    {
        let raw = Connection::open(&path)?;
        raw.execute(
            "UPDATE videos SET claimed_at = ?1 WHERE video_id = 'vid_a'",
            [0_i64], // 1970 — definitely stale
        )?;
    }

    let recovered = store.sweep_stale_claims(Duration::from_secs(60))?;
    assert_eq!(recovered, 1);

    // Confirm row is back to pending with cleared claim metadata.
    let row = store.get_video_for_test("vid_a")?.expect("row present");
    assert_eq!(row.status, "pending");

    let raw = Connection::open(&path)?;
    let (cb, ca): (Option<String>, Option<i64>) = raw.query_row(
        "SELECT claimed_by, claimed_at FROM videos WHERE video_id = 'vid_a'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(cb, None);
    assert_eq!(ca, None);

    // attempt_count is NOT bumped by sweep (0024).
    assert_eq!(row.attempt_count, 1, "attempt_count unchanged by sweep");

    // Carry-forward from T6/T7 review: sweep MUST NOT emit a
    // video_events row (0024: sweep is operator-recovery, not an
    // application event; tracing::info! is the only record).
    let event_count: i64 = raw.query_row(
        "SELECT COUNT(*) FROM video_events WHERE video_id = 'vid_a'",
        [],
        |r| r.get(0),
    )?;
    // claim_next emits a 'claimed' event — that's the only event we
    // expect for this row. The sweep itself must add zero.
    let sweep_event_count: i64 = raw.query_row(
        "SELECT COUNT(*) FROM video_events
         WHERE video_id = 'vid_a' AND event_type LIKE '%sweep%'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(
        sweep_event_count, 0,
        "sweep must not emit a video_events row"
    );
    // Sanity check that the underlying total is reasonable (just the
    // claim_next 'claimed' event, no more).
    assert_eq!(
        event_count, 1,
        "only the claim_next 'claimed' event should be present"
    );

    Ok(())
}

#[test]
fn sweep_stale_claims_leaves_fresh_claim_alone() -> anyhow::Result<()> {
    use std::time::Duration;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let _claim = store.claim_next("worker-1")?.expect("claim");

    // No backdating — claimed_at is `unix_now()`, well within any sane
    // threshold.
    let recovered = store.sweep_stale_claims(Duration::from_secs(60 * 60 * 24))?;
    assert_eq!(recovered, 0, "fresh claim should not be swept");

    let row = store.get_video_for_test("vid_a")?.expect("row present");
    assert_eq!(row.status, "in_progress");
    Ok(())
}

#[test]
fn sweep_stale_claims_is_idempotent() -> anyhow::Result<()> {
    use std::time::Duration;
    use uu_tiktok::state::Store;

    let tmp = tempfile::TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.claim_next("w1")?;

    // First sweep on no-stale: 0.
    assert_eq!(
        store.sweep_stale_claims(Duration::from_secs(60 * 60 * 24))?,
        0
    );
    // Second sweep on no-stale: still 0.
    assert_eq!(
        store.sweep_stale_claims(Duration::from_secs(60 * 60 * 24))?,
        0
    );
    Ok(())
}
