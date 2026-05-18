# Task 9 — Wire sweep + classifier into `run_serial`

**Goal:** Make `run_serial` operationally recoverable: (a) call `store.sweep_stale_claims(opts.stale_claim_threshold)` at the top before the claim loop; (b) replace the current `return Err(e)` on failure with `store.mark_retryable_failure(video_id, worker_id, "FetchOrTranscribe", &format!("{e}"))` and `continue`. The string `"FetchOrTranscribe"` is Epic 2's placeholder kind; Epic 3 replaces it with classifier dispatch.

**ADRs touched:** AD0008 (artifact-write ordering preserved), AD0022 (string-kind mutator).

**Files:**
- Modify: `src/pipeline.rs` (the `run_serial` body)
- Modify: `tests/pipeline_fakes.rs` (test that a failing `FakeFetcher` results in `failed_retryable` rather than an error propagated to the caller)

**Pre-reqs:** T6 (`mark_retryable_failure` available), T8 (`sweep_stale_claims` available), T11 (`stale_claim_threshold` plumbed through `ProcessOptions`) — **T11 should land before T9**, or T9 can hard-code `Duration::from_secs(30 * 60)` and T11 plumbs the flag in afterwards. Recommended order: T11 first, then T9 references `opts.stale_claim_threshold` cleanly.

---

- [ ] **Step 1: Update `ProcessOptions` (if T11 hasn't landed yet)**

If T11 lands first, `ProcessOptions` already has `stale_claim_threshold: Duration`. Skip to Step 2.

If T9 lands first, add to `ProcessOptions` in `src/pipeline.rs`:

```rust
pub struct ProcessOptions {
    pub worker_id: String,
    pub transcripts_root: PathBuf,
    pub max_videos: Option<usize>,
    pub compute_lang_probs: bool,
    pub transcribe_timeout: Duration,
    /// Threshold for `sweep_stale_claims` at the top of `run_serial`.
    /// Default 30 min per AD0023; CLI flag is `--stale-claim-threshold`
    /// (T11). Constructed from `Config::stale_claim_threshold`.
    pub stale_claim_threshold: Duration,
}
```

Update the `ProcessOptions { … }` literal in `src/main.rs::Process` to pass the new field (T11 plumbs it from `Config`; if T11 hasn't landed, hard-code `Duration::from_secs(30 * 60)` here with a `// TODO(T11)` comment that T11 removes).

- [ ] **Step 2: Write the failing test**

Append to `tests/pipeline_fakes.rs`:

```rust
/// `run_serial` no longer aborts on first failure (Plan A behavior); it
/// classifies the failure as retryable and continues. This test confirms
/// the new behavior: a failing fetcher leaves the row as `failed_retryable`
/// and run_serial returns Ok(stats) with `failed >= 1`.
#[tokio::test]
async fn run_serial_classifies_fetch_failure_as_retryable_and_continues() -> anyhow::Result<()> {
    use std::time::Duration;
    use tempfile::TempDir;
    use uu_tiktok::pipeline::{run_serial, ProcessOptions};
    use uu_tiktok::state::Store;

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.upsert_video("vid_b", "https://example/b", false)?;

    // FakeFetcher: returns an error on every call. Adjust based on the
    // existing fake's API surface — if there's a constructor for an
    // always-fails variant use it; otherwise extend the fake here.
    let fetcher = FakeFetcher::always_fails();
    let transcriber = FakeTranscriber::echo();

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(2),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
    };

    let stats = run_serial(&mut store, &fetcher, &transcriber, opts).await?;
    assert_eq!(stats.claimed, 2);
    assert_eq!(stats.succeeded, 0);
    assert_eq!(stats.failed, 2);

    // Both rows should be failed_retryable with the placeholder kind.
    for vid in ["vid_a", "vid_b"] {
        let row = store.get_video_for_test(vid)?.expect("row");
        assert_eq!(row.status, "failed_retryable", "video {vid}");
    }
    Ok(())
}
```

If `FakeFetcher::always_fails()` doesn't exist yet, add a small constructor on the existing fake to produce one. The test should compile only after that lands.

Run:

```bash
cargo test --features test-helpers --test pipeline_fakes
```

Expected: FAIL (test asserts continue-on-failure, but `run_serial` still aborts).

- [ ] **Step 3: Update `run_serial`**

In `src/pipeline.rs`, replace the current loop body:

```rust
pub async fn run_serial(
    store: &mut Store,
    fetcher: &dyn VideoFetcher,
    transcriber: &dyn Transcriber,
    opts: ProcessOptions,
) -> Result<ProcessStats> {
    let mut stats = ProcessStats::default();
    let max = opts.max_videos.unwrap_or(usize::MAX);

    // AD0023: recover any rows left in_progress by a crashed earlier run.
    // No-op if there are none; threshold defaults to 30 min per AD0023.
    let recovered = store
        .sweep_stale_claims(opts.stale_claim_threshold)
        .context("sweep_stale_claims at run_serial start")?;
    if recovered > 0 {
        tracing::info!(recovered, "sweep_stale_claims recovered abandoned rows");
    }

    while stats.claimed + stats.failed < max {
        let claim = match store.claim_next(&opts.worker_id)? {
            Some(c) => c,
            None => break,
        };
        stats.claimed += 1;

        match process_one(store, fetcher, transcriber, &claim, &opts).await {
            Ok(()) => stats.succeeded += 1,
            Err(e) => {
                stats.failed += 1;
                let msg = format!("{e:#}"); // chain-aware via anyhow
                tracing::error!(
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "video failed; classifying as failed_retryable"
                );
                // AD0022: Epic 2 emits a single string-kind for all
                // pipeline-noticed failures. Epic 3's classifier replaces
                // this single arm with typed dispatch (RetryableKind /
                // TerminalReason / VideoUnavailable).
                store
                    .mark_retryable_failure(
                        &claim.video_id,
                        &opts.worker_id,
                        "FetchOrTranscribe",
                        &msg,
                    )
                    .with_context(|| format!("mark_retryable_failure for {}", claim.video_id))?;
                // continue — do NOT return early.
            }
        }
    }

    Ok(stats)
}
```

The `#[allow(unused_assignments)]` attribute on the function can now come off — `stats.failed += 1` is no longer dead under fail-fast (it's load-bearing in the continue-on-failure path).

```rust
// Remove this attribute:
// #[allow(unused_assignments)]
pub async fn run_serial(...
```

- [ ] **Step 4: Run the new test**

```bash
cargo test --features test-helpers --test pipeline_fakes run_serial_classifies_fetch_failure_as_retryable_and_continues
```

Expected: PASS.

- [ ] **Step 5: Run all pipeline_fakes tests + the full suite**

```bash
cargo test --features test-helpers --test pipeline_fakes
cargo test --features test-helpers
```

Expected: all green. Pre-existing tests that exercised "happy path" still pass — the change is additive on the error arm.

- [ ] **Step 6: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean. AD0002 cleanup: confirm `#[allow(unused_assignments)]` was removed (clippy would now error on the unchanged attribute since the increment is no longer dead).

- [ ] **Step 7: Commit**

```bash
git add src/pipeline.rs tests/pipeline_fakes.rs
git commit -m "$(cat <<'EOF'
feat(pipeline): run_serial sweeps stale claims + classifies failures as retryable (AD0008, AD0022, AD0023)

Plan A's run_serial aborted on first failure, leaving the row stuck in
status='in_progress'. Plan B Epic 2's MVP behavior: classify the failure
as failed_retryable (string-kind 'FetchOrTranscribe' per AD0022) and
continue. Epic 3 replaces the single string-kind arm with typed
classifier dispatch.

Two changes:
1. Call store.sweep_stale_claims(opts.stale_claim_threshold) at the top
   of run_serial, before the claim loop. AD0023: 30-min default.
   Recovers rows abandoned by a crashed earlier process.
2. Replace the early `return Err(e)` with mark_retryable_failure() +
   continue. opts.worker_id is the predicate match per AD0022.

#[allow(unused_assignments)] on run_serial removed — `stats.failed += 1`
is now load-bearing in the continue-on-failure path (per AD0002 cleanup
discipline).

Test (--features test-helpers): FakeFetcher::always_fails() + 2 pending
videos → run_serial returns Ok(stats) with claimed=2, succeeded=0,
failed=2; both rows are status='failed_retryable'.

Refs: AD0008, AD0022, AD0023

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] New pipeline test passes
- [ ] Existing pipeline tests still pass (happy path unaffected)
- [ ] `run_serial` no longer returns `Err` on per-video failure (only on infrastructure failure: sweep error, claim_next error, mark_retryable_failure error)
- [ ] `sweep_stale_claims` is called BEFORE the claim loop, not inside it
- [ ] `#[allow(unused_assignments)]` removed
- [ ] Clippy/fmt clean
