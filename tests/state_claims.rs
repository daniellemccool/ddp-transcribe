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
