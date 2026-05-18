# Task 16 — `async fn fetch_worker(...)`: claim_next → fetch → decode → send

**Goal:** Implement the fetch worker that runs in parallel (N=3 default per 0027). Loop: `claim_next` → `fetcher.acquire` → `audio::decode_wav` → `sender.send((claim, samples, wav_path))`. Exits on `claim_next == None` (no polling per 0026). On retryable error: `mark_retryable_failure` and continue. On Bug-class error or `send` failure (channel closed): return `Err`.

**ADRs touched:** 0025 (Bug → return Err), 0026 (no polling), 0027 (payload + worker count).

**Files:**
- Modify: `src/pipeline.rs` (or `src/pipeline/pipelined.rs` per T15's split decision)
- Modify: `tests/pipeline_fakes.rs` (test that a fetch worker drains pending rows and reports clean exit)

**Pre-reqs:** T13 (tokio-util), T15 (skeleton + helpers).

---

- [ ] **Step 1: Define the payload type**

In `src/pipeline.rs`:

```rust
/// Channel payload from fetch workers to the transcribe worker.
/// Per 0027: fetch workers do WAV decode in parallel so the
/// transcribe path is lean. The PathBuf rides through for cleanup
/// after mark_succeeded per 0008. `samples_len` is carried so the
/// transcribe worker can compute `duration_s = samples_len as f64 /
/// 16_000.0` without re-deriving from the moved `samples` Vec (which
/// is consumed by the engine.transcribe() call).
#[derive(Debug)]
pub(crate) struct FetchedItem {
    pub claim: Claim,
    pub samples: Vec<f32>,
    pub samples_len: usize,
    pub wav_path: PathBuf,
}
```

- [ ] **Step 2: Write the failing test**

Append to `tests/pipeline_fakes.rs`:

```rust
#[tokio::test]
async fn fetch_worker_drains_pending_rows_and_exits() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::{mpsc, Mutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};
    use uu_tiktok::state::Store;

    let tmp = TempDir::new()?;
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let mut store_setup = Store::open(&tmp.path().join("state.sqlite"))?;
    store_setup.upsert_video("vid_a", "https://example/a", false)?;
    store_setup.upsert_video("vid_b", "https://example/b", false)?;
    drop(store_setup);

    let shared: SharedStore = Arc::new(Mutex::new(store));
    let fetcher = FakeFetcher::happy(); // existing fake or new variant
    let (tx, mut rx) = mpsc::channel::<FetchedItem>(2);
    let token = CancellationToken::new();
    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        // T19 will add download_workers and channel_capacity here; T16
        // doesn't need them — they live on the orchestrator side.
    };

    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        Arc::clone(&shared),
        Arc::new(fetcher),
        tx,
        Arc::new(opts),
    ));

    // Drain the channel — should get 2 items, then None when worker exits.
    let mut items = Vec::new();
    while let Some(item) = rx.recv().await {
        items.push(item);
    }
    assert_eq!(items.len(), 2);

    let worker_result = worker_handle.await.expect("join")?;
    assert_eq!(worker_result, ()); // fetch_worker returns Result<()>; Ok means clean exit

    Ok(())
}
```

The test exercises: (a) two pending rows are claimed; (b) two items appear on the channel; (c) the worker exits cleanly when claim_next returns None; (d) the channel closes after the worker drops its sender.

Run:

```bash
cargo test --features test-helpers --test pipeline_fakes fetch_worker_drains
```

Expected: FAIL (compile error — `fetch_worker` and `FetchedItem` and `SharedStore` exports + the new signature).

- [ ] **Step 3: Implement `fetch_worker`**

In `src/pipeline.rs` (or `src/pipeline/pipelined.rs`):

```rust
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Phase 2 fetch worker. Claims pending rows; fetches + decodes WAVs;
/// pushes (claim, samples, wav_path) onto the channel. Exits cleanly on
/// claim_next == None (no polling per 0026). On retryable error,
/// classifies via mark_retryable_failure and continues. On Bug-class
/// (channel closed, store error), returns Err — the orchestrator
/// reacts per 0025.
pub async fn fetch_worker(
    token: CancellationToken,
    store: SharedStore,
    fetcher: Arc<dyn VideoFetcher>,
    sender: mpsc::Sender<FetchedItem>,
    opts: Arc<ProcessOptions>,
) -> Result<()> {
    let worker_id = opts.worker_id.clone();
    loop {
        // Respect cancellation: check before every claim. The orchestrator
        // calls token.cancel() during shutdown; we exit before claiming
        // more work.
        if token.is_cancelled() {
            tracing::info!(worker = %worker_id, "fetch_worker: cancellation observed; exiting");
            return Ok(());
        }

        let claim = {
            let mut guard = store.lock().await;
            guard.claim_next(&worker_id)?
        };
        let claim = match claim {
            Some(c) => c,
            None => {
                tracing::info!(worker = %worker_id, "fetch_worker: queue drained; exiting");
                return Ok(());
            }
        };

        // Inline fetch + decode (using the helper from T15). Errors here
        // are application failures, not Bug-class — classify as retryable
        // and continue.
        match fetch_and_decode(fetcher.as_ref(), &claim).await {
            Ok((samples, wav_path)) => {
                let samples_len = samples.len();
                let item = FetchedItem {
                    claim,
                    samples,
                    samples_len,
                    wav_path,
                };
                if sender.send(item).await.is_err() {
                    // Channel closed — transcribe_worker exited (panic or
                    // shutdown). Bug-class signal up to the orchestrator;
                    // the WAV is on disk and will be picked up by a future
                    // sweep + retry once requeue-retryables lands (Epic 5).
                    tracing::error!(worker = %worker_id, "fetch_worker: channel closed; exiting Err");
                    return Err(anyhow::anyhow!(
                        "fetch→transcribe channel closed; transcribe_worker has exited"
                    ));
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                tracing::error!(
                    worker = %worker_id,
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "fetch_worker: failure; classifying as retryable"
                );
                let result = {
                    let mut guard = store.lock().await;
                    guard.mark_retryable_failure(
                        &claim.video_id,
                        &worker_id,
                        "Fetch",
                        &msg,
                    )
                };
                if let Err(store_err) = result {
                    // The store call itself failed — Bug-class.
                    return Err(store_err.context("fetch_worker: mark_retryable_failure"));
                }
                // continue to next iteration.
            }
        }
    }
}
```

Notes on the design:

- **Cancellation polled at loop top.** The fetch worker's hottest await is `fetcher.acquire()` (multi-second). Polling cancellation between claims is the conservative choice for Phase 2; a `tokio::select!` wrapping `fetcher.acquire()` could shave seconds off shutdown but adds complexity for marginal benefit (yt-dlp's kill_on_drop already terminates the subprocess on future drop). Phase 2 ships with poll-between-claims; if shutdown latency becomes a complaint, an issue tracks adding the select! to the acquire call.

- **Mutex<Store> contention.** Each `claim_next` holds the lock for the duration of the BEGIN IMMEDIATE transaction (~milliseconds). Multiple fetch workers serialize on this lock, but they were going to serialize on SQLite's BEGIN IMMEDIATE anyway. No regression vs the run_serial path.

- **Failed mutator call.** If `mark_retryable_failure` itself fails (e.g., DB corruption), surface as Bug — the orchestrator should not silently swallow store errors.

- **No polling.** 0026: drain semantics. Worker exits on `None`.

- [ ] **Step 4: Make `VideoFetcher` Arc-able for spawning across workers**

`Arc<dyn VideoFetcher>` requires `VideoFetcher: Send + Sync + ?Sized` — confirm the trait already has those bounds (`grep 'trait VideoFetcher' src/fetcher/mod.rs`; per the earlier inspection, yes: `pub trait VideoFetcher: Send + Sync`). No trait change needed.

T18 will construct the `Arc<dyn VideoFetcher>` in `main::Process`.

- [ ] **Step 5: Run the test**

```bash
cargo test --features test-helpers --test pipeline_fakes fetch_worker_drains
```

Expected: PASS.

- [ ] **Step 6: Full suite + clippy**

```bash
cargo test --features test-helpers
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: green; clean.

- [ ] **Step 7: Commit**

```bash
git add src/pipeline.rs tests/pipeline_fakes.rs
git commit -m "$(cat <<'EOF'
feat(pipeline): fetch_worker — claim/fetch/decode/send loop with retryable classification (0025, 0026, 0027)

N=3 of these per 0027. Each worker:
- Loops claim_next (Mutex<Store>; SQLite serializes via BEGIN IMMEDIATE)
- Calls fetch_and_decode (T15 helper)
- Sends (claim, samples, wav_path) on the mpsc channel
- Exits clean on claim_next == None (no polling per 0026)
- Polls CancellationToken between claims (0025)
- Classifies retryable errors via mark_retryable_failure + continue
  (placeholder kind "Fetch"; Epic 3 swaps in typed RetryableKind)
- Returns Err on Bug-class: channel closed (transcribe_worker exited)
  or store mutator failed

Cancellation latency: bounded by the largest single await inside the
loop (typically fetcher.acquire(), multi-second). yt-dlp's kill_on_drop
already terminates the subprocess on future drop. If shutdown latency
becomes an operational complaint, add tokio::select! around acquire —
deferred to follow-up if observed.

Test: 2 pending rows + FakeFetcher::happy + drain channel → both items
appear, worker exits Ok.

Refs: 0025, 0026, 0027

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test pipeline_fakes fetch_worker_drains` passes
- [ ] Full suite green
- [ ] Worker exits clean on claim_next == None (no polling)
- [ ] Worker returns Err on channel-closed (Bug-class signal)
- [ ] Worker classifies retryable errors via mark_retryable_failure (not Err return)
- [ ] CancellationToken polled at loop top
- [ ] Clippy/fmt clean
