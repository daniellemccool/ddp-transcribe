//! Direct `fetch_worker` tests, incl. the gated stale-race test.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tempfile::TempDir;

use ddp_transcribe::fetcher::FakeFetcher;
use ddp_transcribe::state::Store;

use crate::fakes::{run_single_fetch_worker, silence_fixture, store_with_pending};
use crate::fakes::{status_and_retryable_kind, status_and_terminal_reason};

/// A single `fetch_worker` claims every pending row, decodes audio, emits a
/// `FetchedItem` per row onto the channel, then exits cleanly when
/// `claim_next` returns `None` (drain semantics — no polling). Plain
/// `#[tokio::test]` (current_thread runtime) per the operator's
/// `TOKIO_WORKER_THREADS=1` policy; cooperative `.await` interleaves the
/// spawned worker with the channel drain.
// worker-level: candidate for run_pipelined-level replacement (audit Epic 3 T06); kept as-is
#[tokio::test]
async fn fetch_worker_drains_pending_rows_and_exits() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.upsert_video("vid_b", "https://example/b", false)?;

    // Stage real WAV fixtures for the FakeFetcher — `fetch_and_decode` calls
    // `audio::decode_wav` which requires a valid 16 kHz mono WAV.
    let fake_wav_a = tmp.path().join("vid_a.wav");
    let fake_wav_b = tmp.path().join("vid_b.wav");
    std::fs::copy(silence_fixture(), &fake_wav_a)?;
    std::fs::copy(silence_fixture(), &fake_wav_b)?;
    let map = HashMap::from([
        ("vid_a".to_string(), fake_wav_a.clone()),
        ("vid_b".to_string(), fake_wav_b.clone()),
    ]);
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
    };

    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let (tx, mut rx) = mpsc::channel::<FetchedItem>(2);
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let claims_counter = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        Arc::clone(&shared),
        Arc::new(fetcher),
        tx,
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&claims_counter),
        Arc::new(opts),
    ));

    // Drain the channel — should get 2 items, then None when the worker drops
    // its sender on clean exit (claim_next == None).
    let mut items = Vec::new();
    while let Some(item) = rx.recv().await {
        items.push(item);
    }
    assert_eq!(items.len(), 2, "two pending rows → two channel items");

    // Sanity-check the payload — claim + samples + samples_len + wav_path
    // ride together.
    for item in &items {
        assert!(item.samples_len > 0, "decoded samples must be non-empty");
        assert_eq!(item.samples.len(), item.samples_len);
        assert!(item.wav_path.exists(), "wav_path must still exist on disk");
        assert!(
            ["vid_a", "vid_b"].contains(&item.claim.video_id.as_str()),
            "claim.video_id matches the upsert set"
        );
        // fetcher_name rides through FetchedItem from VideoFetcher::name();
        // FakeFetcher reports "fake-fetcher".
        assert_eq!(
            item.fetcher_name, "fake-fetcher",
            "FetchedItem.fetcher_name reflects the actual fetcher name"
        );
    }

    let worker_result = worker_handle.await.expect("join");
    assert!(worker_result.is_ok(), "fetch_worker returns Ok on drain");
    assert_eq!(
        stats_stale_after_failure.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "happy path must not increment the stale-after-failure counter"
    );

    Ok(())
}

/// When `mark_retryable_failure` returns `Ok(0)` (the worker's claim was
/// swept mid-flight and the row is no longer in `in_progress AND
/// claimed_by=worker`), `fetch_worker` increments the
/// `stats_stale_after_failure` counter and continues — it does NOT return
/// Err. Symmetric to `process_one`'s `StaleAfterSuccess` outcome on the
/// success side.
///
/// Forces the race deterministically via `FakeFetcher::gated_then_always_fails`:
/// iteration 1's fetch awaits a Notify; the test main task acquires the
/// shared Store lock, sweeps the row back to pending with `Duration::ZERO`,
/// drops the lock, then fires `notify_one`. The fetcher returns Err and
/// the worker's `mark_retryable_failure` predicate misses (the row's
/// `claimed_by` is now NULL) → `Ok(0)` → counter++.
// worker-level: REQUIRED — deterministic interleaving via gate, unreachable from run_pipelined
#[tokio::test]
async fn fetch_worker_increments_stale_after_failure_on_swept_claim() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let (fetcher, gate) = FakeFetcher::gated_then_always_fails();
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    // capacity 1; the fetcher never produces a `FetchedItem` (always fails),
    // so capacity is irrelevant here.
    let (tx, mut rx) = mpsc::channel::<FetchedItem>(1);
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let claims_counter = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let counter_handle = Arc::clone(&stats_stale_after_failure);
    let shared_for_worker = Arc::clone(&shared);
    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        shared_for_worker,
        Arc::new(fetcher),
        tx,
        counter_handle,
        Arc::clone(&claims_counter),
        Arc::new(opts),
    ));

    // Wait until the worker has claimed the row and entered the gated
    // fetcher. unix_now() is second-resolution, so wait ≥1s past the claim
    // before sweeping (so `claimed_at < now` after the sweep cutoff).
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // Sweep on the shared store (the worker has dropped the mutex guard;
    // it's blocked on the gate inside fetcher.acquire). Duration::ZERO →
    // cutoff = now, so the row's claimed_at < cutoff flips it to pending.
    {
        let mut guard = shared.lock().await;
        let swept = guard.sweep_stale_claims(Duration::ZERO)?;
        assert_eq!(swept, 1, "row must sweep back to pending");
    }

    // Release the fetcher → returns Err → mark_retryable_failure → Ok(0).
    gate.notify_one();

    // Drain the channel. No FetchedItem should ever arrive (every fetch
    // fails); the worker exits when claim_next returns None (row is now
    // failed_retryable after iteration 2's successful flip).
    assert!(
        rx.recv().await.is_none(),
        "no successful fetch expected; FakeFetcher always fails"
    );

    let worker_result = worker_handle.await.expect("join");
    assert!(
        worker_result.is_ok(),
        "Ok(0) is not a Bug — worker must NOT return Err: {worker_result:?}"
    );
    assert_eq!(
        stats_stale_after_failure.load(Ordering::Relaxed),
        1,
        "exactly one Ok(0) → counter incremented once"
    );

    Ok(())
}

/// Epic 3 T07: a fetch failure whose stderr matches a write-off pattern
/// (ADR 0033) is dispatched through `mark_terminal_failure`, not
/// `mark_retryable_failure` — the row goes straight to `failed_terminal`
/// with the classifier's tag as `terminal_reason`, never to be retried.
#[tokio::test]
async fn fetch_worker_writes_off_ip_blocked_as_terminal() {
    let (store, _tmp) = store_with_pending(&["7000000000000000010"]);
    let fetcher = std::sync::Arc::new(FakeFetcher::fails_with_stderr(
        "ERROR: [TikTok] 7000000000000000010: Your IP address is blocked from accessing this post",
    ));
    run_single_fetch_worker(store.clone(), fetcher).await;

    let (status, reason) = status_and_terminal_reason(&store, "7000000000000000010").await;
    assert_eq!(status, "failed_terminal");
    assert_eq!(reason.as_deref(), Some("IpBlockedMessage"));
}

/// Epic 3 T07: a fetch failure whose stderr matches a retryable pattern
/// records the taxonomy kind (not the Epic 2 placeholder "Fetch") in
/// `last_retryable_kind`.
#[tokio::test]
async fn fetch_worker_records_taxonomy_kind_for_retryable() {
    let (store, _tmp) = store_with_pending(&["7000000000000000011"]);
    let fetcher = std::sync::Arc::new(FakeFetcher::fails_with_stderr(
        "ERROR: unable to download video data: HTTP Error 403: Forbidden",
    ));
    run_single_fetch_worker(store.clone(), fetcher).await;

    let (status, kind) = status_and_retryable_kind(&store, "7000000000000000011").await;
    assert_eq!(status, "failed_retryable");
    assert_eq!(
        kind.as_deref(),
        Some("HttpError"),
        "placeholder \"Fetch\" kind must be gone"
    );
}
