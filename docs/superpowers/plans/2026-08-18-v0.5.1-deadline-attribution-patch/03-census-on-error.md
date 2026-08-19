# Task 03 — Close the batch row with an aborted census when the run errors

**Files:**
- Modify: `src/batch.rs` (new helper + unit test), `src/commands.rs:~290`
  (the `let stats = stats_result?;` line in the `Process` arm)
- Test: `src/batch.rs` `#[cfg(test)]` (helper shape) and
  `tests/batch_census.rs` (store-level: aborted close sets `finished_at`)

**Interfaces:**
- Consumes: `Store::open_batch_run` / `close_batch_run(run_id, census_json)
  -> Result<usize>` (`src/state/mod.rs:1536`, ADR-0006 row-count return);
  `batch::BatchCensus { sweep, run }` and its `Display`/`Serialize`
  (`src/batch.rs`); the sweep stats value already in scope in the `Process`
  arm (`sweep_stats`, same value the success path serializes).
- Produces: `pub(crate) fn aborted_census_json(sweep: &SweepStats, error: &str)
  -> serde_json::Result<String>` in `src/batch.rs` (adjust the sweep
  parameter's type name to the actual one used by `BatchCensus.sweep` —
  read the struct; the plan refers to it as `SweepStats`). A `batch_runs`
  row whose census JSON carries `"aborted": true` is the DB-visible marker
  that a run died mid-flight — greppable/queryable, mirroring the
  `breaker_tripped` visibility posture of ADR-0050.

**Report (ADR-0019):** ≤250 words, STATUS / SUMMARY / CHANGED FILES / DEVIATIONS.

Ground truth: on the worker-`Err` path, `run_pipelined` returns `Err` after
its drain; `commands.rs:290` (`let stats = stats_result?;`) propagates
immediately, so `close_batch_run` (:322) never runs — `finished_at` stays
NULL and the run's census is lost entirely (confirmed live: rowid 20 after
the 2026-08-17 kill). Run counters are not recoverable on this path (they
live inside `run_pipelined`); the aborted census therefore records the
sweep half (already computed before workers started) plus the error string
— honest partial documentation beats a dangling row.

- [ ] **Step 1: Write the failing tests**

In `src/batch.rs` `#[cfg(test)]` (beside the existing census serialization
tests):

```rust
#[test]
fn aborted_census_json_carries_marker_error_and_sweep() {
    let sweep = /* construct the same SweepStats value the existing
                   serialization test uses — copy its literal */;
    let json = aborted_census_json(&sweep, "fetch→transcribe channel closed").unwrap();
    assert!(json.contains("\"aborted\":true"));
    assert!(json.contains("channel closed"));
    assert!(json.contains("\"sweep\""));
    // The success-path census must NOT gain the marker:
    // (guard against the marker leaking into BatchCensus itself)
    assert!(!serde_json::to_string(&BatchCensus {
        sweep: /* same literal */,
        run: RunCensus::default_or_zeroed_equivalent(), /* use whatever
            zero-value construction the existing tests use; if RunCensus
            has no Default, build it from a zeroed ProcessStats via
            RunCensus::from(&ProcessStats { ... }) copying an existing
            test literal */
    }).unwrap().contains("aborted"));
}
```

In `tests/batch_census.rs` (store-level, following that file's
open/close idiom):

```rust
#[test]
fn aborted_close_stamps_finished_at_and_marker() -> anyhow::Result<()> {
    // fresh store; open_batch_run(...) -> run_id;
    // close_batch_run(run_id, &aborted_census_json(&sweep, "boom")?)?;
    // SELECT finished_at IS NOT NULL, census_json FROM batch_runs WHERE ...
    // assert finished_at set; assert census_json contains "aborted":true
    Ok(())
}
```

(Write both bodies concretely against the existing literals/idioms in those
files — the shapes above name every assertion required.)

- [ ] **Step 2: Run to verify they fail for the real reason**

Run: `cargo test --features test-helpers --lib batch -- --test-threads=1 aborted && cargo test --features test-helpers --test batch_census -- --test-threads=1 aborted`
Expected: compile failure on `aborted_census_json` — the helper is the
deliverable.

- [ ] **Step 3: Implement the helper**

`src/batch.rs`:

```rust
/// Census JSON for a run that died before its stats existed (worker Err
/// path). Sweep counters are real (computed pre-workers); run counters are
/// unrecoverable — the `aborted` marker plus the error string is the
/// DB-visible record that this row's absence of run counters is a crash,
/// not a zero-work run. Consumed by `commands::dispatch`'s Process arm.
pub(crate) fn aborted_census_json(
    sweep: &SweepStats,
    error: &str,
) -> serde_json::Result<String> {
    serde_json::to_string(&serde_json::json!({
        "aborted": true,
        "error": error,
        "sweep": sweep,
    }))
}
```

(Adjust `SweepStats` to the actual type name; it is already `Serialize` —
the success path serializes it inside `BatchCensus`.)

- [ ] **Step 4: Wire the Process arm**

`src/commands.rs` — replace `let stats = stats_result?;` with:

```rust
    let stats = match stats_result {
        Ok(stats) => stats,
        Err(run_err) => {
            // The run died (worker Err path). Close the batch row with an
            // aborted census so `finished_at` is stamped and the crash is
            // DB-visible (mirrors ADR-0050's census-visibility posture),
            // then re-raise. Store lock: workers have exited (run_pipelined
            // only returns after its drain), same reasoning as the success
            // path's try_lock below.
            let json = batch::aborted_census_json(&sweep_stats, &format!("{run_err:#}"))
                .context("serializing aborted census")?;
            let mut guard = shared
                .try_lock()
                .context("store lock free after run_pipelined resolved — workers have exited")?;
            let closed = guard.close_batch_run(run_id, &json)?;
            if closed == 0 {
                tracing::warn!(run_id, "aborted-census close matched no open row");
            }
            tracing::error!(run_id, "run aborted; batch row closed with aborted census");
            return Err(run_err);
        }
    };
```

(Match the surrounding code's actual variable names — `shared`, `run_id`,
`sweep_stats` are the names visible in the success path at :314-330; verify
`sweep_stats` is still in scope at :290 and not moved; if it was moved into
the census construction, borrow or clone it for the error arm first.)

- [ ] **Step 5: Run the touched suites, then the full gate**

Run: `cargo test --features test-helpers --lib batch -- --test-threads=1 && cargo test --features test-helpers --test batch_census -- --test-threads=1`
Expected: PASS.
Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Commit: `git commit -am "fix(commands): close the batch row with an aborted census before re-raising a run error"`
