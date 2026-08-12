# Task 08 — Circuit breaker implementation

**Files:**
- Modify: `src/cli.rs:150-183` (Process flag), `src/commands.rs` (thread
  through + exit mapping + params_json field), `src/pipeline/mod.rs`
  (`ProcessOptions` + `ProcessStats` fields), `src/pipeline/pipelined.rs`
  (Breaker struct, wiring), `src/batch.rs:35-72` (`RunCensus` field +
  `From` + `Display`)
- Test: `tests/pipeline_fakes/pipelined_tests.rs`
- Sweep: every `ProcessOptions {` literal in tests
  (`rg 'ProcessOptions \{' tests/ src/`)

**Interfaces:**
- Consumes: the breaker ADR (Task 07). Test literals include Task 05's
  `FakeFetcher.received_urls` and `Claim.canonical` fields.
- Produces: `ProcessOptions.breaker_threshold: usize`,
  `ProcessStats.breaker_tripped: bool`, `RunCensus.breaker_tripped: bool`,
  `CommandExit::BreakerTripped` (code 4), CLI `--breaker-threshold`
  (default 50). Task 09 adds `breaker_threshold` to `params_json` if this
  task hasn't (it has — see Step 4).

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth (`src/pipeline/pipelined.rs`): cap atomic `claims_counter`
:982, worker spawn :991-1024; failure dispatch sites — fetch-side terminal
census gate :441-444, fetch-side retryable
`handle_record_fetch_failure_outcome` :470-478, transcribe-side :796;
success path `mark_after_artifacts` :690-700 with success/stale counters
:706-715; `token.cancel()` sites :1070-1072 / :1086 / :1094;
`ProcessStats` literal :1113-1126. `params_json` `src/commands.rs:155-163`;
census close + `NoClaims` return :300-316; `CommandExit` :21-34.
`RunCensus` `src/batch.rs:35-54`, `From<&ProcessStats>` :56-72, `Display`
:89-146. CLI Process variant `src/cli.rs:150-183`.

- [ ] **Step 1: Write the failing tests**

In `tests/pipeline_fakes/pipelined_tests.rs` (setup shape of
`run_pipelined_honors_max_videos_cap:248`; `FakeFetcher::always_fails()`
builder exists at `src/fetcher/mod.rs:196` — remember it now needs the
`received_urls` field from Task 05 if you touch the literal):

```rust
#[tokio::test]
async fn breaker_trips_on_consecutive_failures_and_drains_cleanly()
-> anyhow::Result<()> {
    // 100 pending rows, FakeFetcher::always_fails(), breaker_threshold: 10,
    // download_workers: 3, max_videos: None, retries: 1
    // run_pipelined must return Ok(stats) — a trip is an outcome, not an Err
    assert!(stats.breaker_tripped);
    assert!(stats.claimed >= 10, "streak must actually reach threshold");
    assert!(stats.claimed <= 10 + 3,
        "claims stop within one in-flight round of the trip: {}", stats.claimed);
    assert_eq!(stats.succeeded, 0);
    Ok(())
}

#[tokio::test]
async fn breaker_disabled_at_zero_drains_everything() -> anyhow::Result<()> {
    // 20 rows, always_fails, breaker_threshold: 0
    assert!(!stats.breaker_tripped);
    assert_eq!(stats.claimed, 20, "disabled breaker never aborts the drain");
    Ok(())
}

#[tokio::test]
async fn breaker_streak_resets_on_success() -> anyhow::Result<()> {
    // 30 rows; canned WAVs for every SECOND video_id in descending id
    // order (claim order is deterministic: video_id DESC, Task 03), the
    // others absent from the canned map (canned-miss = retryable failure).
    // breaker_threshold: 10. Max streak is 1 → must never trip.
    assert!(!stats.breaker_tripped);
    assert_eq!(stats.claimed, 30);
    assert_eq!(stats.succeeded, 15);
    Ok(())
}
```

And an exit-mapping unit beside the existing `CommandExit` tests (or a new
`#[cfg(test)]` block in `src/commands.rs` following the module's idiom):

```rust
#[test]
fn breaker_tripped_maps_to_exit_4() {
    assert_eq!(CommandExit::BreakerTripped.code(), 4);
}
```

- [ ] **Step 2: Run to verify they fail for the real reason**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1 breaker`
Expected: compile failures on `breaker_threshold` / `breaker_tripped` —
the fields are the deliverable.

- [ ] **Step 3: Implement the pipeline half**

`src/pipeline/mod.rs`: `ProcessOptions` gains `pub breaker_threshold: usize`
(doc comment: "0 disables; breaker ADR"); `ProcessStats` gains
`pub breaker_tripped: bool`.

`src/pipeline/pipelined.rs` — one small shared handle, defined beside the
dispatch helpers (:54 area):

```rust
/// Breaker ADR: run-global consecutive-no-success streak. Trips the
/// ADR-0025 supervision token exactly once; never touches video state.
#[derive(Clone)]
struct Breaker {
    streak: Arc<AtomicUsize>,
    tripped: Arc<AtomicBool>,
    threshold: usize,
}

impl Breaker {
    fn new(threshold: usize) -> Self {
        Self {
            streak: Arc::new(AtomicUsize::new(0)),
            tripped: Arc::new(AtomicBool::new(false)),
            threshold,
        }
    }
    fn note_failure(&self, token: &CancellationToken) {
        let streak = self.streak.fetch_add(1, Ordering::Relaxed) + 1;
        if self.threshold > 0
            && streak >= self.threshold
            && !self.tripped.swap(true, Ordering::Relaxed)
        {
            tracing::error!(
                streak,
                threshold = self.threshold,
                "circuit breaker tripped: consecutive claims without success — cancelling run"
            );
            token.cancel();
        }
    }
    fn note_success(&self) {
        self.streak.store(0, Ordering::Relaxed);
    }
    fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::Relaxed)
    }
}
```

Construct `let breaker = Breaker::new(opts_arc.breaker_threshold);` beside
`claims_counter` (:982); pass a clone into each fetch worker and the
transcribe worker (extend their parameter lists — the fetch worker already
takes 12 params; a 13th `breaker: Breaker` follows the existing pattern).

Call sites:
- fetch-side terminal write-off — after the census gate (:441-444):
  `breaker.note_failure(&token);`
- fetch-side retryable — after `handle_record_fetch_failure_outcome`
  (:470-478): `breaker.note_failure(&token);`
- transcribe-side failure — after :796: `breaker.note_failure(&token);`
- success path — beside the succeeded/stale counters (:706-715), for BOTH
  outcomes (`StaleAfterSuccess` included, per the ADR):
  `breaker.note_success();`

`ProcessStats` literal (:1113-1126): `breaker_tripped: breaker.is_tripped(),`.

**Do not** add polling, worker aborts, or a second token — the trip is
`token.cancel()` and the existing drain does the rest (0025/0026).

- [ ] **Step 4: Implement the CLI/commands/census half**

`src/cli.rs` Process variant (:150-183):
```rust
    /// Abort the run after this many consecutive claims resolve without
    /// a success (0 disables). Protects the pool and the egress from
    /// WAF-style mass-failure waves.
    #[arg(long, default_value_t = 50)]
    breaker_threshold: usize,
```

`src/commands.rs`: destructure the new field in the Process arm; add
`"breaker_threshold": breaker_threshold,` to the `params_json` `json!`
(:155-163); set `breaker_threshold` in the `ProcessOptions` literal
(:206-219). `CommandExit` (:21-34) gains `BreakerTripped` with
`code() => 4`. After `close_batch_run` + `print!("{census}")` (:300-313),
**before** the `NoClaims` return (:314-316):
```rust
    if stats.breaker_tripped {
        return Ok(CommandExit::BreakerTripped);
    }
```

`src/batch.rs`: `RunCensus` (:35-54) gains `breaker_tripped: bool`;
`From<&ProcessStats>` (:56-72) maps it; `Display` (:89-146) prints a
`breaker_tripped` line (always, true or false — greppable either way).

Sweep every `ProcessOptions {` literal in tests: add
`breaker_threshold: 0` (disabled) except the breaker tests themselves —
existing tests must not change behavior.

- [ ] **Step 5: Run the suites**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1 && cargo test --features test-helpers --test batch_census -- --test-threads=1`
Expected: PASS — three breaker tests, exit-map unit, and no regressions
in the existing pipelined/census tests.

- [ ] **Step 6: Full gate and commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "feat(pipeline): mass-failure circuit breaker — threshold 50, exit 4, census-visible"`
