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
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
        received_urls: std::sync::Mutex::new(Vec::new()),
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
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        Arc::clone(&shared),
        Arc::new(fetcher),
        tx,
        Arc::clone(&stats_stale_after_failure),
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::new())), // terminal_by_label
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

    // Sanity-check the payload — claim + samples + samples_len + the audio
    // handle ride together.
    for item in &items {
        assert!(item.samples_len > 0, "decoded samples must be non-empty");
        assert_eq!(item.samples.len(), item.samples_len);
        assert!(
            item.audio.wav_path.exists(),
            "wav_path must still exist on disk"
        );
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
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let counter_handle = Arc::clone(&stats_stale_after_failure);
    let shared_for_worker = Arc::clone(&shared);
    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        shared_for_worker,
        Arc::new(fetcher),
        tx,
        counter_handle,
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::new())), // terminal_by_label
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

/// Epic 3 T08 / ADR 0035: cookies ride ONLY on a retry whose
/// `last_retryable_kind` is `SensitiveLoginGated`. Drives the full path
/// through real worker dispatch: seed a pending row → fail it with the
/// sensitive-login-gated fixture message (recording the taxonomy kind) →
/// requeue back to pending (`sweep_requeue`) → re-claim
/// through a fresh `fetch_worker` with `cookies_file` set in
/// `ProcessOptions`. The second `acquire` call must have carried the
/// cookie path; a first-attempt claim never would (kind gate).
#[tokio::test]
async fn fetch_worker_threads_cookies_on_sensitive_login_gated_retry() -> anyhow::Result<()> {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::fetcher::VideoFetcher;
    use ddp_transcribe::pipeline::{fetch_worker, FetchedItem, ProcessOptions};

    let video_id = "7000000000000000012";
    let (store, tmp) = store_with_pending(&[video_id]);

    // Phase 1: first attempt fails with the sensitive-login-gated message —
    // the classification table maps it to the "SensitiveLoginGated" label.
    // `run_single_fetch_worker` uses `ProcessOptions { cookies_file: None,
    // .. }`, so this first `acquire` call carries no cookies regardless
    // (kind is `None` at claim time — the gate would reject them anyway).
    let failing_fetcher = std::sync::Arc::new(FakeFetcher::fails_with_stderr(
        "ERROR: [TikTok] 7000000000000000012: This post may not be comfortable for some \
         audiences. Log in for access. Use --cookies-from-browser or --cookies for the \
         authentication.",
    ));
    run_single_fetch_worker(store.clone(), failing_fetcher).await;

    let (status, kind) = status_and_retryable_kind(&store, video_id).await;
    assert_eq!(status, "failed_retryable");
    assert_eq!(kind.as_deref(), Some("SensitiveLoginGated"));

    // Requeue back to pending (sweep mutator) — preserves the kind so
    // the next claim's `last_retryable_kind` still reads
    // "SensitiveLoginGated".
    {
        let mut guard = store.lock().await;
        let requeued = guard.sweep_requeue(video_id, "SensitiveLoginGated", 5)?;
        assert_eq!(requeued, 1, "row must requeue to pending");
    }

    // Phase 2: re-claim through a fresh fetch_worker with `cookies_file`
    // set. The recorder captures every FetchOpts passed to `acquire`.
    let cookie_path = PathBuf::from("/secret/tiktok-cookies.txt");
    let wav = tmp.path().join(format!("{video_id}.wav"));
    std::fs::copy(silence_fixture(), &wav)?;
    let map = HashMap::from([(video_id.to_string(), wav)]);
    let fetcher = Arc::new(FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
        received_urls: std::sync::Mutex::new(Vec::new()),
    });

    let (tx, mut rx) = mpsc::channel::<FetchedItem>(2);
    let opts = Arc::new(ProcessOptions {
        worker_id: "fetcher-2".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: Some(cookie_path.clone()),
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    });

    let worker = tokio::spawn(fetch_worker(
        CancellationToken::new(),
        store.clone(),
        Arc::clone(&fetcher) as Arc<dyn VideoFetcher>,
        tx,
        Arc::new(AtomicUsize::new(0)), // stale_after_failure
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::new())), // terminal_by_label
        Arc::new(AtomicUsize::new(0)), // claims_counter
        opts,
    ));

    while rx.recv().await.is_some() {}
    worker.await.expect("join fetch_worker")?;

    let recorded = fetcher.received_opts.lock().expect("received_opts mutex");
    assert_eq!(recorded.len(), 1, "exactly one acquire call in phase 2");
    assert_eq!(
        recorded[0].cookies_file.as_deref(),
        Some(cookie_path.as_path()),
        "cookies must ride on the SensitiveLoginGated retry"
    );

    Ok(())
}

/// Staged experiment (ADR 0038): a claim whose `last_retryable_kind` is
/// `NoDataBlocks` (the download-advertised-but-unservable class — the
/// parked pilot backlog) must fetch with `FetchPolicy::Frugal` and no
/// cookies, through the real `fetch_worker` dispatch path — mirrors
/// `fetch_worker_threads_cookies_on_sensitive_login_gated_retry` above, but
/// seeds the retryable kind via a raw UPDATE on a still-pending row (no
/// requeue-cycle needed since the format gate only reads
/// `last_retryable_kind`, not status).
#[tokio::test]
async fn fetch_worker_uses_frugal_on_no_data_blocks_retry() -> anyhow::Result<()> {
    use std::sync::Arc;

    use ddp_transcribe::fetcher::{FetchPolicy, VideoFetcher};

    let video_id = "7000000000000000099";
    let (store, tmp) = store_with_pending(&[video_id]);

    // Seed the still-pending row's last_retryable_kind via raw UPDATE — the
    // sweep-free flow that makes a format-blamed retry claimable (same
    // technique as the parked-row reseed in the cookie-parking test above).
    {
        let db = tmp.path().join("state.sqlite");
        let raw = rusqlite::Connection::open(&db)?;
        let changed = raw.execute(
            "UPDATE videos SET last_retryable_kind = 'NoDataBlocks' WHERE video_id = ?1",
            [video_id],
        )?;
        assert_eq!(changed, 1, "row must be seeded with NoDataBlocks kind");
    }

    let wav = tmp.path().join(format!("{video_id}.wav"));
    std::fs::copy(silence_fixture(), &wav)?;
    let map = HashMap::from([(video_id.to_string(), wav)]);
    let fetcher = Arc::new(FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
        received_urls: std::sync::Mutex::new(Vec::new()),
    });

    run_single_fetch_worker(store.clone(), Arc::clone(&fetcher) as Arc<dyn VideoFetcher>).await;

    let recorded = fetcher.received_opts.lock().expect("received_opts mutex");
    assert_eq!(recorded.len(), 1, "exactly one acquire call");
    assert_eq!(
        recorded[0].format_policy,
        FetchPolicy::Frugal,
        "a NoDataBlocks retry must not re-pick the format that died mid-transfer"
    );
    assert_eq!(
        recorded[0].cookies_file, None,
        "NoDataBlocks never carries cookies"
    );

    Ok(())
}

/// Epic 4a T06: a retryable fetch failure requeues the row to 'pending'
/// (end of queue, T05 ordering) and the SAME batch re-claims and recovers
/// it. `fails_n_times_then_succeeds(1, ..)` fails once with a NoDataBlocks
/// (retryable) stderr, then returns the canned WAV. With `retries: 1` the
/// lifetime cap is 2 attempts, so the requeue is allowed and the recovery
/// lands in-batch. Driven through `run_pipelined` (the retry loop needs the
/// full claim→fail→requeue→re-claim→succeed cycle; `download_workers: 1`
/// keeps it deterministic).
#[tokio::test]
async fn retry_requeues_then_recovers_in_same_batch() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};
    use ddp_transcribe::transcribe::Transcriber;

    use crate::fakes::{fails_n_times_then_succeeds, FakeTranscriber};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;
    drop(store);

    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let fetcher = Arc::new(fails_n_times_then_succeeds(
        1,
        "vid_a",
        wav,
        "ERROR: [TikTok] vid_a: Did not get any data blocks; please try again later.",
    ));
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 1,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let stats = run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts).await?;
    // ADR-0007 input-side, per-attempt semantics (T06 review fix): one
    // video, two attempts → claimed counts BOTH claims, and the failing
    // first attempt registers in `failed` even though the row ultimately
    // recovered.
    assert_eq!(stats.claimed, 2, "per-attempt: both claims counted");
    assert_eq!(stats.failed, 1, "per-attempt: the failing first attempt");
    assert_eq!(stats.requeued_for_retry, 1, "one in-batch requeue");
    assert_eq!(stats.succeeded, 1, "row recovered on the retry");

    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.attempt_count, 2, "two real attempts: fail then succeed");
    Ok(())
}

/// Epic 4a T06: a retryable failure that never clears exhausts the lifetime
/// attempt cap and lands in `failed_retryable` (the "exhausted, adjudicate"
/// pool). `fails_n_times_then_succeeds(5, ..)` under `retries: 1` (cap = 2)
/// gets exactly two real attempts — one requeue, then the cap-exhausted
/// park — never reaching the 3rd scripted failure.
#[tokio::test]
async fn retry_exhausts_into_failed_retryable() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};
    use ddp_transcribe::transcribe::Transcriber;

    use crate::fakes::{fails_n_times_then_succeeds, FakeTranscriber};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;
    drop(store);

    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let fetcher = Arc::new(fails_n_times_then_succeeds(
        5,
        "vid_a",
        wav,
        "ERROR: [TikTok] vid_a: Did not get any data blocks; please try again later.",
    ));
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 1,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
        checkpoint: None,
    };

    let stats = run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts).await?;
    // ADR-0007 per-attempt semantics: two claims, two failure-dispatched
    // attempts (T06 review fix).
    assert_eq!(stats.claimed, 2, "per-attempt: both claims counted");
    assert_eq!(
        stats.failed, 2,
        "per-attempt: both failing attempts counted"
    );
    assert_eq!(stats.requeued_for_retry, 1, "one requeue before exhaustion");
    assert_eq!(
        stats.exhausted_retries, 1,
        "second failure exhausts the cap"
    );
    assert_eq!(stats.succeeded, 0);

    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "failed_retryable");
    assert_eq!(
        row.attempt_count, 2,
        "cap honored: exactly two attempts, never the 3rd scripted failure"
    );
    Ok(())
}

/// Epic 4a T06: a `requires-cookie` (SensitiveLoginGated) failure with no
/// cookies configured PARKS the row in `failed_retryable` without burning
/// retry budget (a cookie-less retry is a guaranteed refail). A follow-up
/// run with `--cookies-file` re-claims the parked row (seeded back to
/// pending, kind preserved) and the fetcher records the cookie path attached
/// on that retry claim (ADR 0035 kind gate).
#[tokio::test]
async fn requires_cookie_parks_without_cookies_and_requeues_with() -> anyhow::Result<()> {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::fetcher::VideoFetcher;
    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};
    use ddp_transcribe::transcribe::Transcriber;

    use crate::fakes::FakeTranscriber;

    let sensitive = "ERROR: [TikTok] vid_a: This post may not be comfortable for some \
         audiences. Log in for access. Use --cookies-from-browser or --cookies for the \
         authentication.";

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;
    let db = tmp.path().join("state.sqlite");

    let mut store = Store::open(&db)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    drop(store);

    let table = || {
        std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        )
    };

    // Run 1: no cookies configured → the requires-cookie failure parks the
    // row (does NOT requeue, even though the budget is untouched).
    let shared: SharedStore = Arc::new(TokioMutex::new(Store::open(&db)?));
    let failing = Arc::new(FakeFetcher::fails_with_stderr(sensitive));
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());
    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 1,
        channel_capacity: 2,
        cookies_file: None,
        classification: table(),
        retries: 1,
        checkpoint: None,
    };
    let stats = run_pipelined(Arc::clone(&shared), failing, transcriber, opts).await?;
    assert_eq!(stats.parked_for_cookies, 1, "parked, not requeued");
    assert_eq!(stats.requeued_for_retry, 0, "budget must NOT be spent");
    // ADR-0007 per-attempt semantics: the parked attempt is one claim and
    // one failure-dispatched attempt (T06 review fix).
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.failed, 1);
    {
        let guard = shared.lock().await;
        let row = guard.get_video_for_test("vid_a")?.expect("row");
        assert_eq!(row.status, "failed_retryable");
        assert_eq!(
            row.last_retryable_kind.as_deref(),
            Some("SensitiveLoginGated")
        );
    }

    // Seed the parked row back to pending (kind preserved) via raw UPDATE —
    // the sweep-free natural flow that makes it claimable with cookies.
    {
        let raw = rusqlite::Connection::open(&db)?;
        let changed = raw.execute(
            "UPDATE videos SET status='pending', last_retryable_kind='SensitiveLoginGated' \
             WHERE video_id='vid_a'",
            [],
        )?;
        assert_eq!(changed, 1, "row re-seeded to pending");
    }

    // Run 2: cookies configured + a fetcher that succeeds → the retry claim
    // (kind SensitiveLoginGated) carries the cookie path per ADR 0035.
    let cookie_path = PathBuf::from("/secret/tiktok-cookies.txt");
    let wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;
    let succeeding = Arc::new(FakeFetcher {
        canned: StdMutex::new(HashMap::from([("vid_a".to_string(), wav)])),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: StdMutex::new(None),
        received_opts: StdMutex::new(Vec::new()),
        fail_first_n: StdMutex::new(HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
        received_urls: StdMutex::new(Vec::new()),
    });
    let transcriber2: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());
    let opts2 = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 1,
        channel_capacity: 2,
        cookies_file: Some(cookie_path.clone()),
        classification: table(),
        retries: 1,
        checkpoint: None,
    };
    let stats2 = run_pipelined(
        Arc::clone(&shared),
        Arc::clone(&succeeding) as Arc<dyn VideoFetcher>,
        transcriber2,
        opts2,
    )
    .await?;
    assert_eq!(stats2.succeeded, 1, "row recovers with cookies");
    assert_eq!(stats2.claimed, 1, "per-attempt: one claim this run");
    assert_eq!(stats2.failed, 0, "no failure-dispatched attempt this run");

    let recorded = succeeding
        .received_opts
        .lock()
        .expect("received_opts mutex");
    assert_eq!(recorded.len(), 1, "exactly one acquire in run 2");
    assert_eq!(
        recorded[0].cookies_file.as_deref(),
        Some(cookie_path.as_path()),
        "cookies must ride on the SensitiveLoginGated retry claim"
    );
    Ok(())
}

/// T06 review fix: a `mark_terminal_failure` that misses its claim predicate
/// (`Ok(0)` — the row was swept mid-fetch) must NOT be counted in the
/// `terminal_by_label` census map: nothing was written, and the follow-up
/// attempt that DOES land the write-off records the label once. Before the
/// fix the increment ran unconditionally after `handle_mutator_result`, so
/// this scenario double-counted the label (map value 2 instead of 1).
///
/// Same gated interleaving as
/// `fetch_worker_increments_stale_after_failure_on_swept_claim`, but the
/// canned stderr is a terminal class (ip-blocked) so dispatch takes the
/// `Unavailable` arm: iteration 1 goes stale (`Ok(0)`), iteration 2
/// re-claims the swept row and writes it off for real.
// worker-level: REQUIRED — deterministic interleaving via gate, unreachable from run_pipelined
#[tokio::test]
async fn fetch_worker_stale_terminal_claim_not_counted_in_census() -> anyhow::Result<()> {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    // Gated fetcher whose every failure carries a TERMINAL-class stderr.
    let gate = Arc::new(tokio::sync::Notify::new());
    let fetcher = FakeFetcher {
        canned: Mutex::new(HashMap::new()),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(Some(gate.clone())),
        canned_stderr: std::sync::Mutex::new(Some(
            "ERROR: [TikTok] vid_a: Your IP address is blocked from accessing this post"
                .to_string(),
        )),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
        received_urls: std::sync::Mutex::new(Vec::new()),
    };

    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let (tx, mut rx) = mpsc::channel::<FetchedItem>(1);
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let terminal_by_label = Arc::new(TokioMutex::new(BTreeMap::<String, usize>::new()));
    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
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

    let worker_handle = tokio::spawn(fetch_worker(
        CancellationToken::new(),
        Arc::clone(&shared),
        Arc::new(fetcher),
        tx,
        Arc::clone(&stats_stale_after_failure),
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::clone(&terminal_by_label),
        Arc::new(AtomicUsize::new(0)), // claims_counter
        Arc::new(opts),
    ));

    // Wait past the second-resolution timestamp boundary, then sweep the
    // claim out from under the worker while it is parked on the gate.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    {
        let mut guard = shared.lock().await;
        let swept = guard.sweep_stale_claims(Duration::ZERO)?;
        assert_eq!(swept, 1, "row must sweep back to pending");
    }
    gate.notify_one();

    // No successful fetch ever happens; worker exits after iteration 2's
    // real write-off leaves no pending rows.
    assert!(rx.recv().await.is_none(), "every fetch fails");
    worker_handle.await.expect("join")?;

    assert_eq!(
        stats_stale_after_failure.load(Ordering::Relaxed),
        1,
        "iteration 1's terminal mark went stale"
    );
    let census = terminal_by_label.lock().await;
    assert_eq!(
        census.get("IpBlockedMessage").copied(),
        Some(1),
        "stale Ok(0) attempt must not inflate the census: exactly one \
         counted write-off (the iteration-2 write that landed)"
    );
    assert_eq!(census.len(), 1, "no other labels recorded");
    drop(census);

    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "failed_terminal");
    Ok(())
}

/// Epic 5b (task-07 review finding): when the transcribe worker has already
/// exited, `sender.send(item)` fails and hands the un-sent `FetchedItem`
/// back. That item still owns the attempt dir, so the worker must discard it
/// before escalating — otherwise every fetch worker racing the channel close
/// leaks one attempt dir until the next process start.
///
/// Deterministic without any gating: drop the receiver BEFORE the worker
/// runs, so the very first `send` lands in the closed-channel branch.
#[tokio::test]
async fn fetch_worker_discards_attempt_dir_when_channel_is_closed() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use ddp_transcribe::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    // Staged the way the real fetcher hands one over: the wav lives in an
    // `ytdlp-`-named dir, so the FakeFetcher reports it as the attempt dir.
    let attempt_dir = tmp.path().join(".work/ytdlp-vid_a.4242-0");
    std::fs::create_dir_all(&attempt_dir)?;
    let wav = attempt_dir.join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;

    let fetcher = FakeFetcher {
        canned: Mutex::new(HashMap::from([("vid_a".to_string(), wav)])),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
        received_urls: std::sync::Mutex::new(Vec::new()),
    };

    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let (tx, rx) = mpsc::channel::<FetchedItem>(2);
    drop(rx); // transcribe_worker has exited: the channel is closed.

    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
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

    let result = fetch_worker(
        CancellationToken::new(),
        Arc::clone(&shared),
        Arc::new(fetcher),
        tx,
        Arc::new(AtomicUsize::new(0)), // stale_after_failure
        Arc::new(AtomicUsize::new(0)), // requeued_for_retry
        Arc::new(AtomicUsize::new(0)), // exhausted_retries
        Arc::new(AtomicUsize::new(0)), // parked_for_cookies
        Arc::new(AtomicUsize::new(0)), // failed
        Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::new())), // terminal_by_label
        Arc::new(AtomicUsize::new(0)), // claims_counter
        Arc::new(opts),
    )
    .await;

    let err = result.expect_err("a closed channel escalates as Err");
    assert!(
        err.to_string().contains("channel closed"),
        "error names the closed channel: {err:#}"
    );
    assert!(
        !attempt_dir.exists(),
        "the un-sent item's attempt dir must be discarded, not leaked to the next \
         process start's sweep"
    );

    // The row stays `in_progress` — restart recovery re-claims and re-fetches,
    // which is why keeping those bytes has no recovery value.
    let (status, _) = status_and_retryable_kind(&shared, "vid_a").await;
    assert_eq!(status, "in_progress");
    Ok(())
}
