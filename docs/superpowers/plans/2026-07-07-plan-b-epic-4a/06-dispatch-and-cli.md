# Task 06: Dispatch through `record_fetch_failure`; `--retries` + `--classification` CLI; retry integration tests

**Files:**
- Modify: `src/pipeline/mod.rs` (`ProcessOptions` gains `retries: i64`; `ProcessStats` gains three counters)
- Modify: `src/pipeline/pipelined.rs` (fetch_worker + transcribe_worker Retryable arms → `record_fetch_failure`)
- Modify: `src/pipeline/serial.rs` (both Retryable arms + default-cautious arm → `record_fetch_failure`)
- Modify: `src/cli.rs` (`--retries` on Process; `--classification` global; DELETE nothing yet — triage flags die in Task 08)
- Modify: `src/config.rs` (`classification_path: Option<PathBuf>` threading)
- Modify: `src/main.rs` (load table from file when given; thread `retries`)
- Modify: `src/fetcher/mod.rs` (`FakeFetcher` gains fails-N-then-succeeds behavior)
- Modify: `tests/pipeline_fakes/fakes.rs` + `tests/pipeline_fakes/fetch_worker_tests.rs` + `tests/pipeline_fakes/serial_tests.rs` (new integration tests; ProcessOptions constructions gain `retries`)

**Interfaces:**
- Consumes: T03's `ClassifiedFailure { Retryable { label, requires_cookie, ctx }, … }` + `ProcessOptions.classification`; T04's `record_fetch_failure` + `FailureRecordOutcome`; T05's ordering contract.
- Produces: `ProcessOptions.retries: i64` (default set in main from CLI, **default 1**); `ProcessStats { …, requeued_for_retry: usize, exhausted_retries: usize, parked_for_cookies: usize }` (input-side, verb-named per 0007 — Task 07's census consumes them). Lifts T04's allows on `record_fetch_failure`/`FailureRecordOutcome` (note in commit).

**Semantics (binding):** in every worker, the `Retryable { label, requires_cookie, ctx }` arm calls
`record_fetch_failure(&video_id, &worker_id, &label, &ctx.message(), opts.retries + 1, requires_cookie, opts.cookies_file.is_some())`
and maps the outcome: `Requeued → requeued_for_retry += 1`, `Exhausted → exhausted_retries += 1` (plus the existing `failed`-count semantics: a failure happened either way — keep incrementing `stats.failed` exactly where the old code did), `ParkedForCookies → parked_for_cookies += 1`, `StaleClaim → stale_after_failure += 1` (the existing counter; do NOT double-count via `handle_mutator_result`). `Unavailable` and `Bug` arms are untouched.

- [ ] **Step 1: Extend `ProcessOptions` + `ProcessStats`**

`src/pipeline/mod.rs` — `ProcessOptions` after `classification`:

```rust
    /// Epic 4a: automatic retry budget. A video gets at most `retries`
    /// automatic requeues (lifetime cap = retries + 1 total attempts,
    /// compared against attempt_count which claim_next bumps at claim
    /// time). Default 1 — pilot evidence: one retry recovers the dominant
    /// recoverable class (NoDataBlocks re-fetch 10/10 OK).
    pub retries: i64,
```

`ProcessStats` after `stale_after_failure`:

```rust
    /// Epic 4a: rows a worker sent back to 'pending' for an in-batch retry.
    pub requeued_for_retry: usize,
    /// Epic 4a: rows whose failure exhausted the attempt cap this run.
    pub exhausted_retries: usize,
    /// Epic 4a: requires-cookie rows parked because no cookies-file was
    /// configured for this run.
    pub parked_for_cookies: usize,
    /// Epic 4a: inline write-offs this run, keyed by label — the census's
    /// run-side terminal-by-label breakdown (attrition documentation).
    pub terminal_by_label: std::collections::BTreeMap<String, usize>,
}
```

(`#[derive(Debug, Default)]` on `ProcessStats` already covers the map.) The pipelined path aggregates `terminal_by_label` through a shared `Arc<tokio::sync::Mutex<std::collections::BTreeMap<String, usize>>>` created in `run_pipelined` alongside the atomic counters, passed to `fetch_worker` only (the sole producer of `Unavailable`), and folded into `ProcessStats` after the join loop. In fetch_worker's `Unavailable` arm, after the successful `handle_mutator_result` call, add:

```rust
                        {
                            let mut m = terminal_by_label.lock().await;
                            *m.entry(label.clone()).or_insert(0) += 1;
                        }
```

`run_serial` increments `stats.terminal_by_label` directly in its `Unavailable` arm the same way (no lock — local struct).

- [ ] **Step 2: Rewire the three Retryable dispatch sites**

The pipelined workers aggregate per-worker stats through `Arc<AtomicUsize>` counters today (`stats_stale_after_failure`); follow the same mechanism: `run_pipelined` creates three more `Arc<AtomicUsize>` (`stats_requeued_for_retry`, `stats_exhausted_retries`, `stats_parked_for_cookies`), passes them to both workers alongside the existing two, and folds all five into `ProcessStats` where the existing two are folded today (find the post-join `ProcessStats` assembly at the bottom of `run_pipelined`). Worker signature additions mirror the existing counter parameters exactly.

`src/pipeline/pipelined.rs`, fetch_worker's Retryable arm (~283-306) becomes:

```rust
                    ClassifiedFailure::Retryable { label, requires_cookie, ctx } => {
                        tracing::error!(
                            worker = %worker_id,
                            video_id = video_id.as_str(),
                            label = label.as_str(),
                            "fetch_worker: retryable failure"
                        );
                        let outcome = {
                            let mut guard = store.lock().await;
                            guard.record_fetch_failure(
                                &video_id,
                                &worker_id,
                                &label,
                                &ctx.message(),
                                opts.retries + 1,
                                requires_cookie,
                                opts.cookies_file.is_some(),
                            )
                        };
                        match outcome {
                            Ok(FailureRecordOutcome::Requeued) => {
                                stats_requeued_for_retry.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(FailureRecordOutcome::Exhausted) => {
                                stats_exhausted_retries.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(FailureRecordOutcome::ParkedForCookies) => {
                                stats_parked_for_cookies.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(FailureRecordOutcome::StaleClaim) => {
                                stats_stale_after_failure.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!(
                                    worker = %worker_id,
                                    video_id = video_id.as_str(),
                                    "record_fetch_failure: stale claim (swept + re-claimed elsewhere)"
                                );
                            }
                            Err(e) => {
                                return Err(e.context(format!(
                                    "record_fetch_failure for {video_id}"
                                )));
                            }
                        }
                    }
```

(Import `FailureRecordOutcome` at the top: `use crate::state::FailureRecordOutcome;`. The `Unavailable` arm keeps `mark_terminal_failure` + `handle_mutator_result` exactly as-is.)

transcribe_worker's Retryable arm (~510-533): same transformation verbatim (its errors are transcribe-side; `requires_cookie` is always false there but the code path is identical — pass it through).

`src/pipeline/serial.rs`: both `Some(ClassifiedFailure::Retryable { … })` arms AND the final `None` default-cautious arm switch from `mark_retryable_failure(...)?` to the same `record_fetch_failure` call + outcome match, incrementing `stats.requeued_for_retry` / `stats.exhausted_retries` / `stats.parked_for_cookies` / `stats.stale_after_failure` directly on the local `ProcessStats` (serial has no atomics). The default-cautious arm passes `label = crate::failure::labels::TRANSCRIBE_OTHER`, `requires_cookie = false`.

- [ ] **Step 3: CLI + config + main threading**

`src/cli.rs` — `GlobalArgs` gains (after `whisper_model`):

```rust
    /// Path to a classification-policy TOML (Epic 4a). Absent → the
    /// evidence-derived compiled default.
    #[arg(long, env = "DDP_TRANSCRIBE_CLASSIFICATION")]
    pub classification: Option<PathBuf>,
```

`Command::Process` gains (after `cookies_file`):

```rust
        /// Automatic in-batch retry budget per video (lifetime attempts =
        /// retries + 1). Default 1.
        #[arg(long, default_value_t = 1)]
        retries: i64,
```

`src/config.rs` — `Config` gains `pub classification_path: Option<PathBuf>,`; `from_args` sets `classification_path: args.classification.clone(),`; the `dev_args()` test helper gains `classification: None,`.

`src/main.rs` Process arm — replace Task 03's compiled-default construction with:

```rust
            let table = match &cfg.classification_path {
                Some(p) => {
                    let text = std::fs::read_to_string(p)
                        .with_context(|| format!("reading classification file {}", p.display()))?;
                    classification::ClassificationTable::from_toml_str(&text)
                        .with_context(|| format!("validating classification file {}", p.display()))?
                }
                None => classification::ClassificationTable::compiled_default()
                    .context("loading compiled-default classification policy")?,
            };
            tracing::info!(
                source = %cfg.classification_path.as_deref().map_or_else(
                    || "compiled-default".to_string(),
                    |p| p.display().to_string()
                ),
                rules = table.rule_count(),
                "classification policy active"
            );
            let classification = std::sync::Arc::new(table);
```

and add `retries,` to both the `Command::Process { max_videos, cookies_file, retries }` destructure and `ProcessOptions { …, retries, … }`.

- [ ] **Step 4: FakeFetcher fails-N-then-succeeds**

`src/fetcher/mod.rs` — add to `FakeFetcher` (following its existing cfg-gate and field style):

```rust
    /// Epic 4a test hook: per-video count of failures to emit BEFORE
    /// succeeding (each failed acquire decrements). 0/absent = the canned
    /// behavior applies immediately. Failure text comes from canned_stderr.
    pub fail_first_n: Mutex<HashMap<String, u32>>,
```

In `FakeFetcher::acquire`, before the existing `always_fails` check:

```rust
        {
            let mut gate = self.fail_first_n.lock().expect("fail_first_n lock");
            if let Some(n) = gate.get_mut(video_id) {
                if *n > 0 {
                    *n -= 1;
                    let stderr = self
                        .canned_stderr
                        .lock()
                        .expect("canned_stderr lock")
                        .clone()
                        .unwrap_or_else(|| "transient fake failure".to_string());
                    return Err(FetchError::ToolFailed {
                        tool: "yt-dlp",
                        exit_code: 1,
                        signal: None,
                        stderr_excerpt: stderr,
                    });
                }
            }
        }
```

Every existing `FakeFetcher { … }` literal (serial.rs in-module test, tests/pipeline_fakes/fakes.rs constructors) gains `fail_first_n: Mutex::new(HashMap::new()),` — compiler-driven sweep. In `tests/pipeline_fakes/fakes.rs`, add a constructor helper following the existing `always_fails_*` pattern: `pub(crate) fn fails_n_times_then_succeeds(n: u32, video_id: &str, wav: PathBuf, stderr: &str) -> FakeFetcher`.

- [ ] **Step 5: Integration tests (write them first where practical — they fail against Step 1-4 gaps until wired)**

In `tests/pipeline_fakes/fetch_worker_tests.rs` add three tests, following the file's existing seed/run/assert idiom and its `run_single_fetch_worker`-style helper:

1. `retry_requeues_then_recovers_in_same_batch`: seed one pending video; fetcher = `fails_n_times_then_succeeds(1, "vid_a", wav, "Did not get any data blocks …")` (a retryable-class stderr); run the pipelined path (or the worker loop helper the file uses for multi-iteration flows — `run_pipelined` via the existing pipelined_tests idiom is acceptable if fetch_worker helpers are single-shot; put the test where the harness fits) with `retries: 1`; assert final status `succeeded`, `attempt_count == 2`, and stats `requeued_for_retry == 1`, `succeeded == 1`.
2. `retry_exhausts_into_failed_retryable`: fetcher fails 5 times (`fails_n_times_then_succeeds(5, …)`), `retries: 1` → assert final status `failed_retryable`, `attempt_count == 2` (two real attempts, cap honored), `exhausted_retries == 1`, `requeued_for_retry == 1`.
3. `requires_cookie_parks_without_cookies_and_requeues_with`: stderr = the sensitive fixture text (`"…not be comfortable for some audiences…"`); run once with `cookies_file: None` → status `failed_retryable`, `parked_for_cookies == 1`, kind `SensitiveLoginGated`; then a second run with `cookies_file: Some(path)` and a fetcher that succeeds → row recovers and the fetcher's `received_opts` shows the cookie path attached on the retry claim (the Task-08-era test `fetch_worker_threads_cookies_on_sensitive_login_gated_retry` is the template — this replaces its manual `requeue_retryable` step with the sweep-free natural flow: seed as pending with `last_retryable_kind='SensitiveLoginGated'` via raw UPDATE, mirroring that existing test's technique).

In `tests/pipeline_fakes/serial_tests.rs` add one test `max_videos_budget_counts_retries`: seed ONE video, fetcher fails once then succeeds, `retries: 1`, `max_videos: Some(1)` → the single budget slot is consumed by the first attempt; assert `stats.claimed == 1`, final status `pending` (requeued but never re-claimed — budget honest), and a follow-up `run_serial` with `max_videos: Some(1)` completes it (`succeeded == 1`, `attempt_count == 2`).

Every `ProcessOptions { … }` literal in the four test files gains `retries: 1,` (or the test-specific value above).

- [ ] **Step 6: Run the suites**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1`
Expected: all pre-existing tests green (three former `AlwaysFailsRetryable` tests now end in `pending` after one failure instead of `failed_retryable` — because a first failure under `retries: 1` requeues. Update those assertions deliberately: with `retries: 0` they keep their old meaning; prefer setting `retries: 0` in tests whose subject is the failure-marking itself, and note it inline: `// retries: 0 → immediate exhaust, isolating the marking behavior under test`). New tests 4/4 green.

- [ ] **Step 7: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green. The `process complete` tracing line in main.rs should also gain the three new counters (`requeued_for_retry`, `exhausted_retries`, `parked_for_cookies`) next to `stale_after_failure` — add them.

- [ ] **Step 8: Commit**

```bash
git add -A src/ tests/ Cargo.toml
git commit -m "feat(pipeline): in-batch capped retry — workers dispatch through record_fetch_failure; --retries (default 1) + --classification CLI

0002 dead-code note: lifts T04's allows on record_fetch_failure/
FailureRecordOutcome (first callers: fetch_worker, transcribe_worker,
run_serial)."
```
