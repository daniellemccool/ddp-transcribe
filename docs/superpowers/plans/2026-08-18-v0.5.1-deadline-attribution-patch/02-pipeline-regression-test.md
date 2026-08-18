# Task 02 — Pipeline regression test: all-timeout run completes with a census

**Files:**
- Test: `tests/pipeline_fakes/pipelined_tests.rs` (new test; no production
  code expected to change — this task PROVES Task 01 closed the kill path)

**Interfaces:**
- Consumes: Task 01's attribution rule (`Timeout` on deadline, `Cancelled`
  only on cancel-flag); `FakeBehavior::AlwaysFailsTimeout`
  (`tests/pipeline_fakes/fakes.rs:84-108`, returns
  `Err(TranscribeError::Timeout { .. })`).
- Produces: the regression guarantee named in the release notes — "a
  per-item transcription timeout can no longer terminate the run".

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: copy the setup shape of `run_pipelined_honors_max_videos_cap`
(TempDir, canned WAV map so every FETCH succeeds, full `ProcessOptions`
literal). The failure mode being regression-tested: before v0.5.1, a
deadline abort returned `Cancelled`, the transcribe worker exited `Ok(())`,
fetch workers died on the closed channel, and `run_pipelined` returned
`Err` with rows stranded `in_progress`. After Task 01 the equivalent
per-item outcome is `Timeout` → `classify_transcribe_error` → `Retryable`
→ `record_fetch_failure`, and the run drains normally.

**Breaker note:** every claim in this test fails, so the run WOULD trip the
breaker at the default threshold — set `breaker_threshold: 0` (disabled) in
the `ProcessOptions` literal; this test is about the timeout path, not the
breaker (which has its own tests).

- [ ] **Step 1: Write the failing-shaped test**

In `tests/pipeline_fakes/pipelined_tests.rs`:

```rust
#[tokio::test]
async fn per_item_transcribe_timeout_does_not_kill_the_run() -> anyhow::Result<()> {
    // 6 pending canonical rows; canned WAVs for ALL of them (fetch always
    // succeeds); FakeTranscriber with FakeBehavior::AlwaysFailsTimeout
    // (every transcription "hits its deadline"); breaker_threshold: 0;
    // retries: 1; download_workers: 2.
    //
    // ... seed / build ProcessOptions per run_pipelined_honors_max_videos_cap ...

    let stats = run_result?; // MUST be Ok — the old bug returned Err here

    assert_eq!(stats.succeeded, 0, "every transcription timed out");
    assert!(stats.claimed >= 6, "all rows were claimed (plus in-batch retries)");
    assert_eq!(
        stats.stale_after_failure, 0,
        "no rows were left in_progress for the sweep — timeouts were \
         recorded as ordinary retryable failures"
    );

    // The state machine must show every row parked/requeued/exhausted —
    // NOT in_progress (the old bug's signature).
    let stranded: i64 = /* SELECT COUNT(*) FROM videos WHERE status='in_progress' */;
    assert_eq!(stranded, 0, "no stranded in_progress rows after the run");
    Ok(())
}
```

Write the seeding and the `stranded` query concretely against the file's
existing helpers (the other tests in this file show the `rusqlite` /
store-handle idiom for post-run assertions — follow it; do not invent a new
harness). Choose exact counter assertions (`requeued_for_retry` /
`exhausted_retries`) from the observed values on the first green run IF the
retry arithmetic is not obvious from the brief — but `succeeded == 0`,
`Ok(..)` return, and zero `in_progress` are the load-bearing assertions and
are non-negotiable.

- [ ] **Step 2: Run it to verify it exercises the real path**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1 per_item_transcribe_timeout`
Expected: PASS against Task 01's fix (this is a regression test written
after the fix; its "failing" counterpart is history — the 2026-08-17
incident). To prove the test would have caught the bug, temporarily check
out the pre-Task-01 behavior in your head: the run would return `Err`
("channel closed") and the first assertion (`run_result?`) fails the test.
State in your report that the test's `Ok`-return assertion is the arm that
bites on regression.

- [ ] **Step 3: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "test(pipeline): regression — per-item transcribe timeout never terminates the run"`
