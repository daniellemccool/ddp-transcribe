//! Direct `transcribe_worker` tests, incl. its stale-race test.

use std::time::Duration;

use tempfile::TempDir;

use ddp_transcribe::state::Store;

use crate::fakes::FakeTranscriber;

/// A single `transcribe_worker` drains one `FetchedItem` from the channel,
/// transcribes → writes artifacts → marks the row succeeded (artifacts land
/// on disk before mark_succeeded), then exits cleanly when the sender is
/// dropped (channel closed). Plain `#[tokio::test]` (current_thread
/// runtime) per the operator's `TOKIO_WORKER_THREADS=1` policy.
// worker-level: candidate for run_pipelined-level replacement (audit Epic 3 T06); kept as-is
#[tokio::test]
async fn transcribe_worker_processes_one_item_then_exits_on_channel_close() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{
        transcribe_worker, FetchedAudio, FetchedItem, ProcessOptions, SharedStore,
    };

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    // First open: upsert + claim the row so the worker's mark_succeeded
    // predicate (status='in_progress' AND claimed_by='worker-1') matches.
    let mut store_setup = Store::open(&tmp.path().join("state.sqlite"))?;
    store_setup.upsert_video("vid_a", "https://example/a", false)?;
    let claim_record = store_setup.claim_next("worker-1")?.expect("claim");
    drop(store_setup);

    // Re-open the same DB for the SharedStore handed to the worker.
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let transcriber = FakeTranscriber::echo();
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "worker-1".into(), // matches the claim's worker_id
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let (tx, rx) = mpsc::channel::<FetchedItem>(2);

    // Synthesize an attempt dir + wav the worker can clean up.
    // `audio::decode_wav` is NOT called by the transcribe worker — the
    // samples are already decoded inside the FetchedItem — so the wav
    // contents don't have to be a real WAV; just bytes in a directory the
    // worker owns (Epic 5b: the whole dir goes after the DB commit).
    let attempt_dir = tmp.path().join(".work/ytdlp-vid_a.4242-0");
    std::fs::create_dir_all(&attempt_dir)?;
    let wav_path = attempt_dir.join("vid_a.wav");
    std::fs::write(&wav_path, b"not a real wav")?;

    let samples = vec![0.0_f32; 16_000]; // 1 second of silence at 16 kHz
    let samples_len = samples.len();
    let item = FetchedItem {
        claim: claim_record,
        samples,
        samples_len,
        audio: FetchedAudio {
            wav_path: wav_path.clone(),
            attempt_dir: Some(attempt_dir.clone()),
        },
        fetcher_name: "fake-fetcher",
        fetch_policy_tag: "deterministic-audio",
    };
    tx.send(item).await.unwrap();
    drop(tx); // close channel after first item → worker exits after processing

    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // succeeded
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(opts),
    ));

    worker_handle.await.expect("join")?;

    // Confirm vid_a is now succeeded.
    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row present");
    assert_eq!(row.status, "succeeded");
    drop(guard);

    // Artifacts on disk (written before mark_succeeded) — shard("vid_a") →
    // last two chars "_a" per src/output::shard.
    let txt = tmp.path().join("transcripts/_a/vid_a.txt");
    let json = tmp.path().join("transcripts/_a/vid_a.json");
    assert!(txt.exists(), "transcript .txt at {}", txt.display());
    assert!(json.exists(), "transcript .json at {}", json.display());

    // The whole attempt dir was cleaned up after the DB commit (Epic 5b:
    // previously only the wav went, leaving the dir behind).
    assert!(
        !wav_path.exists(),
        "wav must be removed after mark_succeeded"
    );
    assert!(
        !attempt_dir.exists(),
        "the attempt dir goes with it — cleanup is per-attempt, after the commit"
    );

    // Happy path: stale-after-failure counter must stay at zero.
    assert_eq!(
        stats_stale_after_failure.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "happy path must not increment the stale-after-failure counter"
    );
    // Happy path: stale-after-success counter must stay at zero too.
    assert_eq!(
        stats_stale_after_success.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "happy path must not increment the stale-after-success counter"
    );

    Ok(())
}

/// With no items in the channel, `transcribe_worker` exits within 2s of
/// `token.cancel()` (the loop-top `biased` select prefers the cancellation
/// arm — this is the propagation entry point: at the next iteration's loop
/// top after the in-flight transcribe completes, or immediately if the
/// worker was parked on `receiver.recv()`).
// worker-level: candidate for run_pipelined-level replacement (audit Epic 3 T06); kept as-is
#[tokio::test]
async fn transcribe_worker_exits_on_cancellation() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{transcribe_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let transcriber = FakeTranscriber::echo();
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "w".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let (_tx, rx) = mpsc::channel::<FetchedItem>(2);
    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // succeeded
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(opts),
    ));

    // Fire cancellation; worker should exit promptly (parked on recv()).
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), worker_handle).await;
    assert!(
        result.is_ok(),
        "worker should exit within 2s of cancellation"
    );
    // The inner future returned Ok(()) — no Bug.
    let join = result.unwrap().expect("join");
    assert!(join.is_ok(), "worker returns Ok on cancellation: {join:?}");

    Ok(())
}

/// Symmetric to the fetch-worker stale-after-failure test: when the
/// transcribe worker's claim is swept between `claim_next` (performed by
/// the test driver here, since `transcribe_worker` doesn't claim — it
/// consumes already-claimed FetchedItems) and the worker's
/// `mark_retryable_failure` call, the predicate `status='in_progress' AND
/// claimed_by=?` misses → `Ok(0)` → counter++. The worker continues; it
/// does NOT return Err.
///
/// Forcing the race: pre-claim the row with worker_id="worker-1", sleep
/// past the second-resolution timestamp boundary, sweep with
/// `Duration::ZERO` so the row flips back to pending with `claimed_by`
/// cleared. Send the (now-stale) Claim to the worker via a FetchedItem;
/// the FakeTranscriber returns `EmptyOutput` (a retryable variant);
/// `mark_retryable_failure` sees no matching row → returns Ok(0) → the
/// counter increments.
// worker-level: REQUIRED — deterministic interleaving via gate, unreachable from run_pipelined
#[tokio::test]
async fn transcribe_worker_increments_stale_after_failure_on_swept_claim() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{
        transcribe_worker, FetchedAudio, FetchedItem, ProcessOptions, SharedStore,
    };

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let claim_record = store.claim_next("worker-1")?.expect("claim");

    // unix_now() has second resolution — sleep ≥ 1s past the claim so
    // `claimed_at < cutoff` when sweep runs with Duration::ZERO.
    std::thread::sleep(Duration::from_millis(1100));
    let swept = store.sweep_stale_claims(Duration::ZERO)?;
    assert_eq!(swept, 1, "row must sweep back to pending");
    drop(store);

    // Re-open for the SharedStore handed to the worker.
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let transcriber = FakeTranscriber::always_fails_retryable();
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "worker-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let (tx, rx) = mpsc::channel::<FetchedItem>(2);
    let attempt_dir = tmp.path().join(".work/ytdlp-vid_a.4242-1");
    std::fs::create_dir_all(&attempt_dir)?;
    let wav_path = attempt_dir.join("vid_a.wav");
    std::fs::write(&wav_path, b"not a real wav")?;
    let samples = vec![0.0_f32; 16_000];
    let samples_len = samples.len();
    tx.send(FetchedItem {
        claim: claim_record,
        samples,
        samples_len,
        audio: FetchedAudio {
            wav_path,
            attempt_dir: Some(attempt_dir.clone()),
        },
        fetcher_name: "fake-fetcher",
        fetch_policy_tag: "deterministic-audio",
    })
    .await
    .unwrap();
    drop(tx); // close after item → worker exits after processing

    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // succeeded
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(opts),
    ));

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
    // Failure path: stale-after-success counter must stay at zero
    // (the worker hit the retryable arm, not the success arm).
    assert_eq!(
        stats_stale_after_success.load(Ordering::Relaxed),
        0,
        "failure path must not increment the stale-after-success counter"
    );

    // Row sits in pending (the sweep left it there; the swept-claim
    // mark_retryable_failure UPDATE updated 0 rows so the status stays
    // 'pending').
    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "pending");
    drop(guard);

    Ok(())
}
