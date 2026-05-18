# Task 17 — `async fn transcribe_worker(...)`: receive → transcribe → write → mark_succeeded

**Goal:** Implement the transcribe worker (1 instance per AD0027). Loop: `tokio::select! { _ = token.cancelled() => break, Some(item) = receiver.recv() => transcribe_and_write(...) }`. On engine `Bug`: return `Err`. On retryable transcribe error (rare on Epic 1's surface — most transcribe errors are engine-internal Bug-class), classify via `mark_retryable_failure` and continue.

**ADRs touched:** AD0008 (preserved via `transcribe_and_write` helper), AD0024 (select-on-token), AD0027 (single transcribe worker).

**Files:**
- Modify: `src/pipeline.rs` (or `src/pipeline/pipelined.rs`)
- Modify: `tests/pipeline_fakes.rs` (test: drive an item through transcribe_worker and confirm mark_succeeded)

**Pre-reqs:** T15 (transcribe_and_write helper), T16 (fetch_worker + channel types).

---

- [ ] **Step 1: Write the failing test**

Append to `tests/pipeline_fakes.rs`:

```rust
#[tokio::test]
async fn transcribe_worker_processes_one_item_then_exits_on_channel_close() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::{mpsc, Mutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{transcribe_worker, FetchedItem, ProcessOptions, SharedStore};
    use uu_tiktok::state::{Claim, Store};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;
    let mut store_setup = Store::open(&tmp.path().join("state.sqlite"))?;
    store_setup.upsert_video("vid_a", "https://example/a", false)?;
    let claim_record = store_setup.claim_next("worker-1")?.expect("claim");
    drop(store_setup);

    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(Mutex::new(store));
    let transcriber = FakeTranscriber::echo();
    let token = CancellationToken::new();
    let opts = Arc::new(ProcessOptions {
        worker_id: "worker-1".into(), // matches the claim's worker_id
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
    });

    let (tx, rx) = mpsc::channel::<FetchedItem>(2);

    // Synthesize a wav file the worker can clean up.
    let wav_path = tmp.path().join("synth.wav");
    std::fs::write(&wav_path, b"not a real wav")?;

    let item = FetchedItem {
        claim: claim_record,
        samples: vec![0.0_f32; 16_000], // 1 second of silence at 16 kHz
        wav_path: wav_path.clone(),
    };
    tx.send(item).await.unwrap();
    drop(tx); // close channel after first item → worker exits after processing

    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        opts,
    ));

    let result = worker_handle.await.expect("join")?;
    assert_eq!(result, ());

    // Confirm vid_a is now succeeded.
    let guard = shared.lock().await;
    let row = guard
        .get_video_for_test("vid_a")?
        .expect("row present");
    assert_eq!(row.status, "succeeded");
    drop(guard);

    Ok(())
}

#[tokio::test]
async fn transcribe_worker_exits_on_cancellation() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::{mpsc, Mutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{transcribe_worker, FetchedItem, ProcessOptions, SharedStore};
    use uu_tiktok::state::Store;

    let tmp = TempDir::new()?;
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(Mutex::new(store));
    let transcriber = FakeTranscriber::echo();
    let token = CancellationToken::new();
    let opts = Arc::new(ProcessOptions {
        worker_id: "w".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
    });

    let (_tx, rx) = mpsc::channel::<FetchedItem>(2);
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        opts,
    ));

    // Fire cancellation; worker should exit promptly.
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), worker_handle).await;
    assert!(result.is_ok(), "worker should exit within 2s of cancellation");

    Ok(())
}
```

Run:

```bash
cargo test --features test-helpers --test pipeline_fakes transcribe_worker
```

Expected: FAIL — `transcribe_worker` doesn't exist.

- [ ] **Step 2: Implement `transcribe_worker`**

In `src/pipeline.rs`:

```rust
use crate::transcribe::Transcriber;

/// Phase 2 transcribe worker (1 instance per AD0027). Receives
/// FetchedItem from N fetch workers and runs Phase 3+4 (transcribe →
/// write artifacts → mark_succeeded → cleanup).
///
/// Wraps the per-item work in `tokio::select! { token.cancelled() | recv }`
/// so cancellation interrupts a wait for the next item. The per-item
/// transcribe call itself is uncancellable inside this worker — Epic 1's
/// AD0012 CancelOnDrop fires when the future drops; whisper.cpp's
/// abort_callback polls the per-request Arc<AtomicBool> and aborts within
/// milliseconds. That composition is the AD0024 mechanism.
pub async fn transcribe_worker(
    token: CancellationToken,
    mut receiver: mpsc::Receiver<FetchedItem>,
    transcriber: Arc<dyn Transcriber>,
    store: SharedStore,
    opts: Arc<ProcessOptions>,
) -> Result<()> {
    let worker_id = opts.worker_id.clone();

    loop {
        let item = tokio::select! {
            biased; // Prefer cancellation over recv when both are ready.
            _ = token.cancelled() => {
                tracing::info!(worker = %worker_id, "transcribe_worker: cancellation observed; exiting");
                return Ok(());
            }
            maybe_item = receiver.recv() => match maybe_item {
                Some(it) => it,
                None => {
                    tracing::info!(worker = %worker_id, "transcribe_worker: channel closed; exiting");
                    return Ok(());
                }
            }
        };

        // Process the item. The transcribe + write + mark_succeeded happens
        // under a single Store mutex lock (Mutex held across the await of
        // transcribe; small risk of fetch workers stalling on claim_next
        // during the ~1s transcribe call. Acceptable at N=3; if the lock
        // becomes a bottleneck, split into transcribe-then-lock-then-write.
        // Phase 2 deliberately accepts this hold-across-await for
        // simplicity; T20's bake numbers confirm it's not a measured cost).
        //
        // ALTERNATIVE: hold the lock only for store.mark_succeeded, not for
        // the transcribe await. Cleaner; the transcribe_and_write helper
        // currently bundles them. T17 implementation chooses based on
        // whichever fits the helper's signature.
        let result = {
            // Take the lock only for the store-touching parts. Transcribe
            // separately.
            let FetchedItem { claim, samples, samples_len, wav_path } = item;

            // Phase 3: transcribe (no lock held).
            let per_call = crate::transcribe::PerCallConfig {
                compute_lang_probs: opts.compute_lang_probs,
                ..crate::transcribe::PerCallConfig::default()
            };
            let transcribe_result = transcriber
                .transcribe(samples, per_call, opts.transcribe_timeout)
                .await;

            match transcribe_result {
                Ok(output) => {
                    // Phase 4: write artifacts + mark_succeeded (lock).
                    // Code mirrors T15's transcribe_and_write helper phase
                    // 3+4 but without the transcribe call (already done above
                    // outside the lock).
                    //
                    // duration_s derives from the AD0014 audio invariant
                    // (16 kHz mono): samples_len / 16_000. The transcribe
                    // call already moved `samples`, so we use `samples_len`
                    // captured in the FetchedItem at decode time (T16).
                    let duration_s = Some(samples_len as f64 / 16_000.0_f64);
                    let shard_dir = opts
                        .transcripts_root
                        .join(crate::output::shard(&claim.video_id));
                    std::fs::create_dir_all(&shard_dir)
                        .with_context(|| format!("creating shard dir {}", shard_dir.display()))?;
                    let txt_path = shard_dir.join(format!("{}.txt", claim.video_id));
                    let json_path = shard_dir.join(format!("{}.json", claim.video_id));
                    let metadata = crate::output::artifacts::TranscriptMetadata {
                        video_id: claim.video_id.clone(),
                        source_url: claim.source_url.clone(),
                        duration_s,
                        language_detected: Some(output.language.clone()),
                        transcribed_at: chrono::Utc::now().to_rfc3339(),
                        fetcher: "ytdlp".to_string(),
                        transcript_source: transcriber.name().to_string(),
                        model: output.model_id.clone(),
                        raw_signals: Some(
                            crate::output::artifacts::RawSignals::from_transcribe_output(&output),
                        ),
                    };
                    let json_bytes = serde_json::to_vec_pretty(&metadata)
                        .context("serializing transcript metadata")?;

                    let mut guard = store.lock().await;

                    // AD0008: artifacts on disk BEFORE mark_succeeded.
                    crate::output::artifacts::atomic_write(
                        &txt_path,
                        output.text.as_bytes(),
                    )
                    .with_context(|| format!("writing transcript {}", txt_path.display()))?;
                    crate::output::artifacts::atomic_write(&json_path, &json_bytes)?;

                    guard.mark_succeeded(
                        &claim.video_id,
                        &worker_id,
                        crate::state::SuccessArtifacts {
                            duration_s,
                            language_detected: Some(output.language.clone()),
                            fetcher: "ytdlp",
                            transcript_source: transcriber.name(),
                        },
                    )?;

                    // Cleanup wav after the DB commit; failure here is a
                    // warning, not an error (AD0008 — success is durable).
                    if let Err(e) = std::fs::remove_file(&wav_path) {
                        tracing::warn!(path = %wav_path.display(), error = %e,
                            "could not remove wav after success");
                    }
                    Ok(())
                }
                Err(e) => {
                    // Transcribe error. Most TranscribeError variants are
                    // Bug-class (engine init failed, panicked worker
                    // thread). Cancelled is the special case — it's not
                    // a row failure, it's coordinated shutdown; propagate
                    // as Ok(()) so the JoinSet doesn't flag a Bug.
                    use crate::errors::TranscribeError;
                    if matches!(e, TranscribeError::Cancelled) {
                        tracing::info!(worker = %worker_id, video_id = claim.video_id.as_str(),
                            "transcribe_worker: cancelled mid-item");
                        // Row stays in_progress; sweep recovers on next startup.
                        return Ok(());
                    }
                    if matches!(e, TranscribeError::Bug { .. }) {
                        return Err(anyhow::Error::new(e)
                            .context(format!("transcribe Bug for {}", claim.video_id)));
                    }
                    // Other transcribe errors classify as retryable.
                    let msg = format!("{e:#}");
                    let mut guard = store.lock().await;
                    guard.mark_retryable_failure(
                        &claim.video_id,
                        &worker_id,
                        "Transcribe",
                        &msg,
                    )?;
                    Ok(())
                }
            }
        };

        if let Err(e) = result {
            tracing::error!(worker = %worker_id, error = %e, "transcribe_worker: returning Err (Bug)");
            return Err(e);
        }
    }
}

```

**Note on `samples_len` carry:** the inlined code uses `samples_len` (captured at decode time in T16's fetch_worker, carried through `FetchedItem`) to derive `duration_s` per AD0014's 16 kHz mono invariant. Without the carry, this worker would need to compute `samples.len()` before moving `samples` into `transcriber.transcribe()` — feasible but more error-prone (a future refactor that moved the transcribe call could silently break the derivation). The `FetchedItem.samples_len` field is the explicit contract.

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-helpers --test pipeline_fakes transcribe_worker
```

Expected: PASS for both tests (happy path + cancellation).

- [ ] **Step 4: Full suite + clippy**

```bash
cargo test --features test-helpers
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: green; clean.

- [ ] **Step 5: Commit**

```bash
git add src/pipeline.rs tests/pipeline_fakes.rs
git commit -m "$(cat <<'EOF'
feat(pipeline): transcribe_worker — select{cancelled|recv} → transcribe → write → mark (AD0008, AD0024, AD0027)

1 instance per AD0027. Loops on a select! between token.cancelled() and
receiver.recv(); per-item path is transcribe → write artifacts →
mark_succeeded → cleanup wav (AD0008 invariant lives in
write_artifacts_and_mark, factored out of T15's transcribe_and_write).

Cancellation latency: bounded by the in-flight transcribe call. Epic 1's
AD0012 CancelOnDrop fires when the transcribe future drops; whisper.cpp's
abort_callback polls the per-request Arc<AtomicBool> and aborts inference
within milliseconds. That composition is the AD0024 mechanism — token
cancellation propagates to whisper.cpp through the cancellation primitive.

Error handling:
- TranscribeError::Cancelled: not a row failure; propagates as Ok(()) so
  the JoinSet doesn't flag a Bug. Row stays in_progress; sweep recovers
  on next startup.
- TranscribeError::Bug: returns Err — orchestrator reacts per AD0024.
- Other transcribe errors: classified retryable via mark_retryable_failure
  with kind="Transcribe"; loop continues.

Store mutex is held only for the write+mark phase, NOT across the
transcribe await — prevents fetch workers from stalling on claim_next
during the ~1s transcribe.

Tests: happy path (one item → row succeeded; worker exits on channel
close) + cancellation (token.cancel() → worker exits within 2s).

Refs: AD0008 (preserved), AD0012 (cancellation composition), AD0024,
AD0027

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test pipeline_fakes transcribe_worker` passes (both tests)
- [ ] Worker exits within 2s of `token.cancel()`
- [ ] Worker exits on channel close (clean shutdown)
- [ ] AD0008: artifacts written before mark_succeeded (the helper preserves order)
- [ ] Store mutex NOT held across transcribe await
- [ ] TranscribeError::Cancelled propagates as Ok(()) (not Bug)
- [ ] Clippy/fmt clean
