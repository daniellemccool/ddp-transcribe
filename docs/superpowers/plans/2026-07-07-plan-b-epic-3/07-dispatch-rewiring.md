# Task 07: Three-arm classifier dispatch in both workers + serial; T16 cancellation wrap

**Files:**
- Modify: `src/pipeline/mod.rs` (`fetch_and_decode` return type → `FetchPhaseError`)
- Modify: `src/pipeline/pipelined.rs` (fetch_worker error arm ~lines 159–231; transcribe_worker retryable arm ~lines 409–450; fetch select-wrap)
- Modify: `src/pipeline/serial.rs` (error arm ~lines 58–77)
- Modify: `src/state/mod.rs` (lift `#[allow(dead_code)]` from `mark_terminal_failure`, ~line 420)
- Test: `tests/pipeline_fakes/fetch_worker_tests.rs`, `tests/pipeline_fakes/serial_tests.rs`

**Interfaces:**
- Consumes: Task 03's `classify_fetch_error` / `classify_transcribe_error` / `ClassifiedFailure`; Task 04's `Claim`; Task 06's module layout.
- Produces:
  - `pub enum FetchPhaseError { Fetch(FetchError), Decode(AudioDecodeError) }` (thiserror, `#[from]` both) in `src/pipeline/mod.rs`; `fetch_and_decode(…) -> Result<(Vec<f32>, PathBuf), FetchPhaseError>`.
  - `pub fn classify_fetch_phase(e: &FetchPhaseError) -> ClassifiedFailure` (thin: `Fetch` → `classify_fetch_error`; `Decode` → `Retryable { kind: RetryableKind::TranscribeOther, ctx: … reason: "wav decode failure: refetch may repair a corrupt download" }`).
  - First caller of `Store::mark_terminal_failure` (suppression lifted per 0002).
  - Placeholder kinds `"Fetch"` / `"Transcribe"` / `"FetchOrTranscribe"` are gone from `src/`.

- [ ] **Step 1: Write the failing tests**

In `tests/pipeline_fakes/fetch_worker_tests.rs` — the write-off path. Extend `FakeFetcher` (in `src/fetcher/mod.rs`, cfg-gated) with a canned-error mode first if it only supports `always_fails` `NetworkError` today:

```rust
// src/fetcher/mod.rs — FakeFetcher addition (cfg(any(test, feature = "test-helpers"))):
/// When Some, `acquire` returns this ToolFailed stderr verbatim. Lets
/// integration tests drive specific classifier verdicts through real
/// worker dispatch.
pub canned_stderr: std::sync::Mutex<Option<String>>,
```

(with a `fails_with_stderr(stderr: &str) -> Self` constructor setting `always_fails: false`, `first_call_gate: None`; in `acquire`, check `canned_stderr` before the `always_fails` branch and return `FetchError::ToolFailed { tool: "yt-dlp", exit_code: 1, signal: None, stderr_excerpt }`. Update the two existing constructors to set the new field to `Mutex::new(None)`.)

Test:

```rust
#[tokio::test]
async fn fetch_worker_writes_off_ip_blocked_as_terminal() {
    // one pending row; fetcher fails with the write-off message
    let (store, _tmp) = crate::fakes::store_with_pending(&["7000000000000000010"]);
    let fetcher = std::sync::Arc::new(
        ddp_transcribe::fetcher::FakeFetcher::fails_with_stderr(
            "ERROR: [TikTok] 7000000000000000010: Your IP address is blocked from accessing this post",
        ),
    );
    run_single_fetch_worker(store.clone(), fetcher).await; // reuse the file's existing harness helper

    let (status, reason) = crate::fakes::status_and_terminal_reason(&store, "7000000000000000010").await;
    assert_eq!(status, "failed_terminal");
    assert_eq!(reason.as_deref(), Some("IpBlockedMessage"));
}

#[tokio::test]
async fn fetch_worker_records_taxonomy_kind_for_retryable() {
    let (store, _tmp) = crate::fakes::store_with_pending(&["7000000000000000011"]);
    let fetcher = std::sync::Arc::new(
        ddp_transcribe::fetcher::FakeFetcher::fails_with_stderr(
            "ERROR: unable to download video data: HTTP Error 403: Forbidden",
        ),
    );
    run_single_fetch_worker(store.clone(), fetcher).await;

    let (status, kind) = crate::fakes::status_and_retryable_kind(&store, "7000000000000000011").await;
    assert_eq!(status, "failed_retryable");
    assert_eq!(kind.as_deref(), Some("HttpError"), "placeholder \"Fetch\" kind must be gone");
}
```

Adapt helper names to what `fakes.rs` actually exposes after Task 06 (`store_with_pending`, worker harness, and status readers exist in some form — the current suite already seeds rows and runs `fetch_worker` directly; add the two small status-reader helpers to `fakes.rs` if missing). In `serial_tests.rs`, update the existing `run_serial_classifies_*` assertions from kind `"FetchOrTranscribe"` to the taxonomy kind the fake produces (`FakeFetcher::always_fails` emits `NetworkError` → expect `"NetworkTransient"`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1`
Expected: new tests fail to compile (no `fails_with_stderr`) or fail on kind assertions.

- [ ] **Step 3: Implement**

`src/pipeline/mod.rs`:

```rust
/// Typed error for phases 1+2 so the worker can classify without downcasting
/// through anyhow (spec refinement #1 in the Epic 3 plan overview).
#[derive(Debug, thiserror::Error)]
pub enum FetchPhaseError {
    #[error(transparent)]
    Fetch(#[from] crate::errors::FetchError),
    #[error("decoding fetched wav: {0}")]
    Decode(#[from] crate::audio::AudioDecodeError),
}

pub fn classify_fetch_phase(e: &FetchPhaseError) -> crate::failure::ClassifiedFailure {
    use crate::failure::{ClassifiedFailure, FailureContext, RetryableKind};
    match e {
        FetchPhaseError::Fetch(fe) => crate::failure::classify_fetch_error(fe),
        FetchPhaseError::Decode(de) => ClassifiedFailure::Retryable {
            kind: RetryableKind::TranscribeOther,
            ctx: FailureContext {
                tool: "hound",
                exit_code: None,
                signal: None,
                stderr_excerpt: de.to_string(),
                classification_reason: "wav decode failure: refetch may repair a corrupt download",
            },
        },
    }
}
```

Change `fetch_and_decode`'s signature and internals to return `FetchPhaseError` (replace `.context(…)?` conversions with `?` via the `#[from]` impls; keep the tracing lines). Call sites that need anyhow (e.g. `process_one`) adapt at the boundary.

`src/pipeline/pipelined.rs` — fetch worker: wrap the fetch in the T16 cancellation select and dispatch three ways:

```rust
let fetch_result = tokio::select! {
    biased;
    () = token.cancelled() => {
        // T16: drop the in-flight acquire future so kill_on_drop reaps the
        // yt-dlp child instead of waiting out its timeout. Row stays
        // in_progress; sweep recovers per 0024. Mirrors the transcribe-side
        // wrap (a66d38b).
        tracing::info!(worker = %worker_id, "fetch_worker: cancellation during fetch; exiting");
        return Ok(());
    }
    r = fetch_and_decode(fetcher.as_ref(), &claim) => r,
};
match fetch_result {
    Ok((samples, wav_path)) => { /* unchanged send path */ }
    Err(e) => {
        let video_id = claim.video_id.clone();
        match classify_fetch_phase(&e) {
            ClassifiedFailure::Bug { ctx } => {
                return Err(anyhow!("fetch Bug for {video_id}: {}", ctx.message()));
            }
            ClassifiedFailure::Unavailable { reason, ctx } => {
                tracing::warn!(worker = %worker_id, video_id = video_id.as_str(),
                    reason = reason.tag(), "fetch_worker: write-off; marking terminal");
                let result = {
                    let mut guard = store.lock().await;
                    guard.mark_terminal_failure(&video_id, &worker_id, reason.tag(), &ctx.message())
                };
                handle_mutator_result(result, &worker_id, &video_id, &stats_stale_after_failure, "mark_terminal_failure")?;
            }
            ClassifiedFailure::Retryable { kind, ctx } => {
                tracing::error!(worker = %worker_id, video_id = video_id.as_str(),
                    kind = kind.tag(), "fetch_worker: retryable failure");
                let result = {
                    let mut guard = store.lock().await;
                    guard.mark_retryable_failure(&video_id, &worker_id, kind.tag(), &ctx.message())
                };
                handle_mutator_result(result, &worker_id, &video_id, &stats_stale_after_failure, "mark_retryable_failure")?;
            }
        }
    }
}
```

Factor the existing `Ok(0)`/`Ok(_)`/`Err` match (currently duplicated verbatim in both workers) into one helper in `pipelined.rs`, since this task would otherwise create a third copy:

```rust
/// Shared stale-claim routing for failure-side mutators (Ok(0) = predicate
/// missed → count + warn; Err = Bug-class store failure → propagate).
fn handle_mutator_result(
    result: anyhow::Result<usize>,
    worker_id: &str,
    video_id: &str,
    stale_counter: &Arc<AtomicUsize>,
    op: &'static str,
) -> anyhow::Result<()> {
    match result {
        Ok(0) => {
            stale_counter.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(worker = %worker_id, video_id, "{op} swallowed: row no longer claimed by this worker");
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(e) => Err(e.context(format!("{op} for {video_id}"))),
    }
}
```

Transcribe worker retryable arm: replace the placeholder call with

```rust
Err(e) => {
    match classify_transcribe_error(&e) {
        ClassifiedFailure::Bug { ctx } => { /* keep existing Bug behavior: return Err */ }
        ClassifiedFailure::Unavailable { .. } => unreachable!("transcribe errors never classify Unavailable"),
        ClassifiedFailure::Retryable { kind, ctx } => {
            let result = {
                let mut guard = store.lock().await;
                guard.mark_retryable_failure(&video_id, &worker_id, kind.tag(), &ctx.message())
            };
            handle_mutator_result(result, &worker_id, &video_id, &stats_stale_after_failure, "mark_retryable_failure")?;
        }
    }
}
```

Wait — the existing `Err(e @ TranscribeError::Bug { .. })` arm above it already handles Bug and `Cancelled` exits earlier; keep those arms as-is and only rewire the final catch-all arm through `classify_transcribe_error`, asserting the Bug/Unavailable arms unreachable there (they were consumed earlier). Replace the `unreachable!` with a `debug_assert!` + retryable fallback if clippy objects to unreachable in that position — disclose per 0003 either way.

`src/pipeline/serial.rs` error arm: `process_one` errors are anyhow; classify by downcast, defaulting cautiously:

```rust
Err(e) => {
    stats.failed += 1;
    let verdict = e
        .downcast_ref::<FetchPhaseError>()
        .map(classify_fetch_phase);
    match verdict {
        Some(ClassifiedFailure::Unavailable { reason, ctx }) => {
            store
                .mark_terminal_failure(&claim.video_id, &opts.worker_id, reason.tag(), &ctx.message())
                .with_context(|| format!("mark_terminal_failure for {}", claim.video_id))?;
        }
        Some(ClassifiedFailure::Bug { ctx }) => {
            return Err(anyhow!("fetch Bug for {}: {}", claim.video_id, ctx.message()));
        }
        Some(ClassifiedFailure::Retryable { kind, ctx }) => {
            store
                .mark_retryable_failure(&claim.video_id, &opts.worker_id, kind.tag(), &ctx.message())
                .with_context(|| format!("mark_retryable_failure for {}", claim.video_id))?;
        }
        None => {
            // Not a fetch-phase error (transcribe-side anyhow) — default-cautious.
            store
                .mark_retryable_failure(&claim.video_id, &opts.worker_id, RetryableKind::TranscribeOther.tag(), &format!("{e:#}"))
                .with_context(|| format!("mark_retryable_failure for {}", claim.video_id))?;
        }
    }
}
```

(`process_one` must propagate `FetchPhaseError` inside its anyhow chain un-contexted for the downcast to hit — pass it through with `anyhow::Error::new(e)` rather than `.context(…)` wrapping at that one boundary; verify with the serial write-off test.)

Lift `#[allow(dead_code)]` + the "SURFACE ONLY in Epic 2" comment from `mark_terminal_failure` (0002 cleanup-on-consumption).

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --features test-helpers -- --test-threads=1` and `cargo clippy --all-targets -- -D warnings`.
Expected: PASS; grep check `grep -rn '"Fetch"\|"Transcribe"\|"FetchOrTranscribe"' src/` returns no mutator-kind literals.

- [ ] **Step 5: Commit**

```bash
git add src/pipeline/ src/state/mod.rs src/fetcher/mod.rs tests/pipeline_fakes/
git commit -m "feat(pipeline): three-arm classifier dispatch; first mark_terminal_failure caller; T16 fetch cancellation wrap

Write-off classes (ADR 0033) route to failed_terminal at failure time.
Placeholder kinds removed. fetch_and_decode returns typed FetchPhaseError
(spec refinement #1). Resolves FOLLOWUPS T16 cancellation latency."
```
