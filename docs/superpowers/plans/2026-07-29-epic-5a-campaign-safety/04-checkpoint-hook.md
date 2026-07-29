# Task 04: periodic in-run checkpoint hook

**Files:**
- Modify: `src/process.rs` (`CommandSpec.program: &'static str` → `String`; mechanical call-site sweep)
- Modify: `src/cli.rs` (`Process` subcommand gains `checkpoint_cmd`, `checkpoint_every`)
- Modify: `src/main.rs` (thread into `ProcessOptions`; params_json records checkpoint config)
- Modify: `src/pipeline/mod.rs` (`ProcessOptions` + `ProcessStats` fields)
- Modify: `src/pipeline/pipelined.rs` (supervised periodic task)
- Test: `tests/pipeline_fakes/pipelined_tests.rs` (shim-script checkpoint tests)

**Interfaces:**
- Consumes (existing): `run_pipelined` structure (`src/pipeline/pipelined.rs:767+`): JoinSet spawns at :811/:829-843, load-bearing `drop(tx)` at :848, supervision loop :855-887 (first `Err` → `token.cancel()`), per ADR-0025. `process::run(CommandSpec)` bounded runner (ADR-0021). Process-arg precedent: `cookies_file`/`retries`/`max_videos` (`src/cli.rs:141-158` → `ProcessOptions`, main.rs:174-187). `humantime::parse_duration` value-parser precedent: `--stale-claim-threshold`. `params_json` assembly at main.rs:126-133.
- Produces:
  - CLI: `process --checkpoint-cmd <PATH> [--checkpoint-every <DUR>]` — `checkpoint_every` defaults to `15m`, and `requires = "checkpoint_cmd"` makes `--checkpoint-every` without `--checkpoint-cmd` a parse error. No cmd ⇒ feature off (today's behavior).
  - `ProcessOptions` gains `pub checkpoint: Option<CheckpointConfig>` where `pub struct CheckpointConfig { pub cmd: PathBuf, pub every: Duration }` (in `src/pipeline/mod.rs`).
  - `ProcessStats` gains `pub checkpoints_run: u64, pub checkpoints_failed: u64` (ADR-0007 verb-named, input-side; surfaced wherever ProcessStats already prints/serializes).
  - `CommandSpec.program: String` (was `&'static str`) — every existing call site updates mechanically (`"yt-dlp".to_string()` etc.; `rg 'program:' src/`).

**Semantics (binding):**
- The checkpoint task is one more `join_set.spawn(...)` inserted BEFORE the `drop(tx)` at :848 — wait, it must NOT hold a `tx` clone at all (it never sends fetch items); spawn it alongside the workers but give it only `token.clone()` and its config. The load-bearing `drop(tx)` semantics are untouched.
- Loop shape: `tokio::select! { _ = token.cancelled() => return Ok(()), _ = tokio::time::sleep(cfg.every) => { ...run hook... } }` — sleep-then-run (first firing after one full interval; no run at t=0 — the run boundary already syncs).
- The hook runs via `process::run(CommandSpec { program: cfg.cmd.display().to_string(), args: vec![], timeout: cfg.every, stderr_capture_bytes: 8*1024, stdout_capture_bytes: 4*1024, redact_arg_indices: &[] })` — bounded by construction per ADR-0021. Timeout = the interval: a hook slower than its own period is a config error surfaced as a timeout warn.
- **A hook failure NEVER fails the task**: nonzero exit / timeout / spawn error ⇒ `tracing::warn!` (with bounded stderr excerpt) + `checkpoints_failed` increment + continue looping. The task function's only `return` paths are `Ok(())` on cancellation. Review rejects any `?`/`return Err` on the hook path — an `Err` here would trip ADR-0025's first-error shutdown and kill the run.
- The task must not touch the Store, claims, or any pipeline state — it is a timer + subprocess + two counters. Overlap with the external flock-serialized `sync-to-storage.sh` is safe by that script's own design (runbook: flock).
- `params_json` gains `"checkpoint_cmd": <string|null>, "checkpoint_every_secs": <n|null>` so `batch_runs` records the config (census/attribution).

- [ ] **Step 1: Widen `CommandSpec.program` to `String`**

Mechanical: `src/process.rs:10` field type; `rg 'CommandSpec' src/ tests/` and update every construction site (`"yt-dlp".to_string()`, etc.) and the `tracing` field (`tool = spec.program.as_str()` if needed). Run `cargo test --features test-helpers -- --test-threads=1` for the affected suites (`process_bounded_capture`, fetcher tests) — behavior identical.

- [ ] **Step 2: Write the failing checkpoint tests**

In `tests/pipeline_fakes/pipelined_tests.rs` (this file drives `run_pipelined` with `FakeFetcher` + fake transcriber — follow its existing harness; it runs under `required-features = ["test-helpers"]` already):

```rust
// Shim: absolute-path script that appends a line to a sentinel file.
fn checkpoint_shim(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let sentinel = dir.path().join("checkpoints.log");
    let script = dir.path().join("checkpoint.sh");
    std::fs::write(&script, format!("#!/bin/sh\necho tick >> {}\n", sentinel.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, sentinel)
}

#[tokio::test]
async fn checkpoint_hook_fires_periodically_and_stops_on_cancel() {
    // Build the file's standard pipelined harness with enough queued fake
    // videos to keep the run alive ~2s; ProcessOptions.checkpoint =
    // Some(CheckpointConfig { cmd: script, every: Duration::from_millis(300) }).
    // After the run completes: sentinel exists with >= 2 lines;
    // stats.checkpoints_run >= 2; checkpoints_failed == 0.
}

#[tokio::test]
async fn checkpoint_hook_failure_never_aborts_the_run() {
    // cmd = a script that exits 1. Run a normal small batch to completion.
    // Assert: run completes with its videos succeeded (hook failures did
    // not cancel workers); stats.checkpoints_failed >= 1.
}
```

(Write real tests against the harness's actual constructors — the comments are the binding assertions. Timing: 300ms interval against a ≥1s run keeps it deterministic-enough single-threaded; if the harness completes too fast, use the file's existing slow-fake knobs.)

- [ ] **Step 3: Run to confirm failure** — COMPILE FAIL (`CheckpointConfig` absent).

- [ ] **Step 4: Implement**

1. `src/pipeline/mod.rs`: `CheckpointConfig` struct; `ProcessOptions.checkpoint: Option<CheckpointConfig>`; `ProcessStats { checkpoints_run, checkpoints_failed }` wired into its existing Display/serialize surface.
2. `src/pipeline/pipelined.rs`: counters as `Arc<AtomicU64>` beside the existing stale counters (:788-802); spawn:

```rust
        if let Some(cp) = opts.checkpoint.clone() {
            let token = token.clone();
            let runs = Arc::clone(&checkpoints_run);
            let fails = Arc::clone(&checkpoints_failed);
            join_set.spawn(async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(cp.every) => {
                            match crate::process::run(crate::process::CommandSpec {
                                program: cp.cmd.display().to_string(),
                                args: vec![],
                                timeout: cp.every,
                                stderr_capture_bytes: 8 * 1024,
                                stdout_capture_bytes: 4 * 1024,
                                redact_arg_indices: &[],
                            })
                            .await
                            {
                                Ok(o) if o.exit_code == 0 => {
                                    runs.fetch_add(1, Ordering::Relaxed);
                                    tracing::info!("checkpoint hook ran");
                                }
                                Ok(o) => {
                                    fails.fetch_add(1, Ordering::Relaxed);
                                    tracing::warn!(exit_code = o.exit_code,
                                        stderr = o.stderr_excerpt.as_str(),
                                        "checkpoint hook failed; continuing");
                                }
                                Err(error) => {
                                    fails.fetch_add(1, Ordering::Relaxed);
                                    tracing::warn!(%error, "checkpoint hook did not run; continuing");
                                }
                            }
                        }
                    }
                }
            });
        }
```

   Fold the two counters into `ProcessStats` where the stale counters are consumed (:633-636 region).
3. `src/cli.rs` `Process` variant:

```rust
        /// Operator checkpoint hook: run this command every
        /// --checkpoint-every while the batch is running (e.g.
        /// ~/sync-to-storage.sh). Failures warn and count; they never
        /// stop the run.
        #[arg(long)]
        checkpoint_cmd: Option<std::path::PathBuf>,
        /// Interval between checkpoint hook runs. Requires --checkpoint-cmd.
        #[arg(long, default_value = "15m", value_parser = humantime::parse_duration, requires = "checkpoint_cmd")]
        checkpoint_every: std::time::Duration,
```

4. `src/main.rs`: build `Option<CheckpointConfig>` from the two args into `ProcessOptions` (:174-187 region); add the two params_json fields (:126-133); `log_resolved_config`'s Process arm — extend only if it names other Process args today (check; match its shape).

- [ ] **Step 5: Run tests to verify they pass** — the two new pipelined tests + `cargo test --test cli -- --test-threads=1` for a new parse case: `["process","--checkpoint-every","5m"]` exits 2 (requires), `["process","--checkpoint-cmd","/x","--checkpoint-every","5m"]` parses (code != 2) — add both to `tests/cli.rs`.

- [ ] **Step 6: Full verification**

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` — green.

- [ ] **Step 7: Commit**

```bash
git add src/process.rs src/cli.rs src/main.rs src/pipeline/mod.rs src/pipeline/pipelined.rs tests/pipeline_fakes/pipelined_tests.rs tests/cli.rs src/fetcher/ytdlp.rs src/backfill.rs
git commit -m "feat(pipeline): --checkpoint-cmd/--checkpoint-every — supervised periodic operator hook via the bounded runner; failures warn and count, never abort"
```
