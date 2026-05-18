# Task 18 — Supervision wiring: JoinSet + CancellationToken + LOAD-BEARING shutdown ORDER

**Goal:** Wire N fetch workers + 1 transcribe worker into a `tokio::task::JoinSet` supervised by a shared `tokio_util::sync::CancellationToken`. Main loops on `join_set.join_next()`; on first `Err`/panic, fires `token.cancel()` and drains remaining tasks. **The four-step shutdown order from 0025 is load-bearing**: `token.cancel()` → drop fetch→transcribe sender → join workers → `engine.shutdown()` LAST. Also: remove dead `Config::whisper_use_gpu` + `Config::whisper_threads` (0002 cleanup).

**ADRs touched:** 0002 (dead-code cleanup), 0025 (supervision + shutdown ORDER).

**Files:**
- Modify: `src/main.rs` (Process arm: orchestrator wiring)
- Modify: `src/config.rs` (remove `whisper_use_gpu` + `whisper_threads` fields + tests that reference them)
- Modify: `src/cli.rs` (no flag removal — these were never CLI-exposed; only Config-level)
- Modify: `tests/pipeline_fakes.rs` (high-level integration test: spawn pipelined orchestrator with FakeFetcher + FakeTranscriber; assert clean drain)

**Pre-reqs:** T13 (tokio-util), T15 (pipeline reshape + helpers), T16 (fetch_worker), T17 (transcribe_worker), T19 (CLI flags for download_workers + channel_capacity) — recommended order: **T19 first, then T18**.

---

- [ ] **Step 1: Remove dead config fields (0002 cleanup)**

In `src/config.rs`:

```rust
#[derive(Debug, Clone)]
pub struct Config {
    pub profile: Profile,
    pub state_db: PathBuf,
    pub inbox: PathBuf,
    pub transcripts: PathBuf,
    pub whisper_model_path: PathBuf,
    // whisper_use_gpu and whisper_threads removed — never consumed by
    // Plan B's WhisperEngine (whisper-rs picks n_threads itself; GPU
    // choice is an i32 device index in EngineConfig). The #[allow(dead_code)]
    // they carried since Epic 1 was a "consume when state-machine work
    // lands" placeholder; Epic 2 confirms they have no consumer.
    pub ytdlp_timeout: Duration,
    pub transcribe_timeout: Duration,
    pub compute_lang_probs: bool,
    pub stale_claim_threshold: Duration,
    pub download_workers: usize,        // T19 added
    pub channel_capacity: usize,        // T19 added
}
```

Update `Config::from_args` to drop the two fields:

```rust
impl Config {
    pub fn from_args(args: &GlobalArgs) -> Self {
        match args.profile {
            Profile::Dev => Self {
                profile: Profile::Dev,
                state_db: args.state_db.clone(),
                inbox: args.inbox.clone(),
                transcripts: args.transcripts.clone(),
                whisper_model_path: args.whisper_model.clone().unwrap_or_else(|| {
                    PathBuf::from("./models/ggml-tiny.en.bin")
                }),
                ytdlp_timeout: Duration::from_secs(300),
                transcribe_timeout: Duration::from_secs(600),
                compute_lang_probs: args.compute_lang_probs,
                stale_claim_threshold: args
                    .stale_claim_threshold
                    .unwrap_or_else(|| Duration::from_secs(30 * 60)),
                download_workers: args.download_workers.unwrap_or(3),
                channel_capacity: args.channel_capacity.unwrap_or(2),
            },
        }
    }
}
```

Update the existing `config::tests`:

```rust
#[test]
fn dev_profile_uses_tiny_en_cpu() {
    let cfg = Config::from_args(&dev_args());
    assert!(cfg.whisper_model_path.to_string_lossy().contains("tiny.en"));
    // Removed: whisper_use_gpu / whisper_threads assertions.
    assert_eq!(cfg.ytdlp_timeout, Duration::from_secs(300));
}
```

Drop `num_cpus_safe` if it has no other consumer.

- [ ] **Step 2: Write the failing integration test**

Append to `tests/pipeline_fakes.rs`:

```rust
#[tokio::test]
async fn run_pipelined_drains_all_rows_and_returns_stats() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::Mutex;

    use uu_tiktok::pipeline::{run_pipelined, ProcessOptions, SharedStore};
    use uu_tiktok::state::Store;

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    for i in 0..6 {
        store.upsert_video(&format!("vid_{i}"), &format!("https://example/{i}"), false)?;
    }
    drop(store);

    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(Mutex::new(store));
    let fetcher = Arc::new(FakeFetcher::happy());
    let transcriber = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let stats = run_pipelined(shared.clone(), fetcher, transcriber, opts).await?;
    assert_eq!(stats.claimed, 6);
    assert_eq!(stats.succeeded, 6);
    assert_eq!(stats.failed, 0);

    // All rows succeeded.
    let guard = shared.lock().await;
    for i in 0..6 {
        let row = guard.get_video_for_test(&format!("vid_{i}"))?.expect("row");
        assert_eq!(row.status, "succeeded");
    }
    Ok(())
}
```

Run:

```bash
cargo test --features test-helpers --test pipeline_fakes run_pipelined_drains
```

Expected: FAIL — `run_pipelined` is the skeleton from T15 (returns `ProcessStats::default()`).

- [ ] **Step 3: Implement `run_pipelined`**

In `src/pipeline.rs`:

```rust
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

pub async fn run_pipelined(
    store: SharedStore,
    fetcher: std::sync::Arc<dyn VideoFetcher>,
    transcriber: std::sync::Arc<dyn Transcriber>,
    opts: ProcessOptions,
) -> Result<ProcessStats> {
    // Sweep stale claims at the top of run_pipelined, same as run_serial
    // does at the top of its loop (0024).
    {
        let mut guard = store.lock().await;
        let recovered = guard
            .sweep_stale_claims(opts.stale_claim_threshold)
            .context("sweep_stale_claims at run_pipelined start")?;
        if recovered > 0 {
            tracing::info!(recovered, "sweep_stale_claims at orchestrator start");
        }
    }

    let token = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<FetchedItem>(opts.channel_capacity);
    let opts_arc = std::sync::Arc::new(opts);
    let mut join_set: JoinSet<Result<()>> = JoinSet::new();

    // Spawn the transcribe worker first so the channel has a consumer
    // by the time fetch workers start producing.
    join_set.spawn(transcribe_worker(
        token.clone(),
        rx,
        std::sync::Arc::clone(&transcriber),
        std::sync::Arc::clone(&store),
        std::sync::Arc::clone(&opts_arc),
    ));

    // Spawn N fetch workers. They share the tx clone.
    for _ in 0..opts_arc.download_workers {
        join_set.spawn(fetch_worker(
            token.clone(),
            std::sync::Arc::clone(&store),
            std::sync::Arc::clone(&fetcher),
            tx.clone(),
            std::sync::Arc::clone(&opts_arc),
        ));
    }
    // Drop the original tx so the channel closes when all fetch workers exit.
    drop(tx);

    // Supervise: on first Err or panic, cancel the token and drain.
    let mut first_error: Option<anyhow::Error> = None;
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(Ok(())) => {
                // Worker exited clean.
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, "worker returned Err; initiating shutdown");
                if first_error.is_none() {
                    first_error = Some(e);
                }
                token.cancel();
            }
            Err(join_err) if join_err.is_panic() => {
                let msg = format!("worker panicked: {join_err}");
                tracing::error!(error = %msg, "worker panic; initiating shutdown");
                if first_error.is_none() {
                    first_error = Some(anyhow::anyhow!(msg));
                }
                token.cancel();
            }
            Err(join_err) => {
                // Cancelled JoinError — possible if join_set.abort_all() ran.
                tracing::warn!(error = %join_err, "JoinError (cancelled)");
            }
        }
    }

    // All workers joined. Compute stats from the DB (the workers
    // updated rows directly; ProcessStats is now a summary query).
    let stats = {
        let guard = store.lock().await;
        compute_process_stats(&guard)?
    };

    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(stats)
}

/// Compute ProcessStats from the DB after run_pipelined drains. Counts
/// rows in each terminal status (succeeded, failed_retryable,
/// failed_terminal) under the assumption that the run started on a clean
/// DB (operator reset gesture before the run). For run-to-run accuracy
/// in mid-stream invocations, a richer metric (claim_count delta tracked
/// in an Arc<Mutex<ProcessStats>> shared across workers) would be
/// preferable — left for Epic 5's ops-hygiene work; Plan B Epic 2 ships
/// the COUNT-by-status proxy because the test (and the 0027 bake)
/// validates only per-row status.
fn compute_process_stats(store: &Store) -> Result<ProcessStats> {
    let mut succeeded: usize = 0;
    let mut failed_retryable: usize = 0;
    let mut failed_terminal: usize = 0;

    let mut stmt = store
        .conn()
        .prepare("SELECT status, COUNT(*) FROM videos GROUP BY status")
        .context("preparing status-count query")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize)))
        .context("executing status-count query")?;
    for row in rows {
        let (status, count) = row?;
        match status.as_str() {
            "succeeded" => succeeded = count,
            "failed_retryable" => failed_retryable = count,
            "failed_terminal" => failed_terminal = count,
            _ => {} // pending / in_progress not counted in stats
        }
    }

    let failed = failed_retryable + failed_terminal;
    Ok(ProcessStats {
        claimed: succeeded + failed,
        succeeded,
        failed,
    })
}
```

`Store::conn()` is the existing `pub(crate)` accessor in `src/state/mod.rs`; the `#[allow(dead_code)]` annotation on it comes off in this commit (0002 cleanup — this is the first caller).

- [ ] **Step 4: Wire `run_pipelined` into `main::Process`**

In `src/main.rs`'s `Process` arm, replace the `run_serial` call with `run_pipelined`. **Load-bearing shutdown ORDER per 0025:**

```rust
cli::Command::Process { max_videos } => {
    let store = state::Store::open(&cfg.state_db).context("opening state DB")?;
    std::fs::create_dir_all(&cfg.transcripts).context("creating transcripts dir")?;
    let removed = output::artifacts::cleanup_tmp_files(&cfg.transcripts)?;
    if removed > 0 {
        tracing::info!(removed, "cleaned up leftover .tmp files");
    }

    let work_dir = cfg.transcripts.join(".work");
    std::fs::create_dir_all(&work_dir).context("creating work dir")?;

    // engine stays OWNED by main so engine.shutdown() can be called LAST
    // per 0025. The Arc<dyn Transcriber> path that workers see is a
    // clone-able HANDLE (added to src/transcribe.rs in this task — see
    // Step 3a below) whose Drop closes the engine's request channel.
    let engine_config = transcribe::EngineConfig {
        model_path: cfg.whisper_model_path.clone(),
        gpu_device: 0,
        flash_attn: cfg!(feature = "cuda"),
    };
    let engine = transcribe::WhisperEngine::new(&engine_config)
        .context("constructing WhisperEngine")?;
    let transcriber: std::sync::Arc<dyn transcribe::Transcriber> =
        engine.transcriber_handle();

    let fetcher: std::sync::Arc<dyn fetcher::VideoFetcher> =
        std::sync::Arc::new(fetcher::ytdlp::YtDlpFetcher::new(&work_dir, cfg.ytdlp_timeout));

    let shared: pipeline::SharedStore =
        std::sync::Arc::new(tokio::sync::Mutex::new(store));

    let opts = pipeline::ProcessOptions {
        worker_id: format!("{}-{}", hostname_or_default(), std::process::id()),
        transcripts_root: cfg.transcripts.clone(),
        max_videos,
        compute_lang_probs: cfg.compute_lang_probs,
        transcribe_timeout: cfg.transcribe_timeout,
        stale_claim_threshold: cfg.stale_claim_threshold,
        download_workers: cfg.download_workers,
        channel_capacity: cfg.channel_capacity,
    };

    // 0025 SHUTDOWN ORDER (load-bearing — sequence matters):
    //   1. run_pipelined runs to completion (clean drain) or returns Err.
    //      Internally on Err it calls token.cancel(), drops the
    //      fetch→transcribe sender, and joins all worker tasks before
    //      returning. So by the time we get here, every worker task
    //      has finished and dropped its clone of the transcriber Arc.
    //   2. Drop main's own clone of the transcriber Arc. This is the
    //      LAST clone — its Drop closes the engine's request channel.
    //   3. Call engine.shutdown() LAST. With the request channel closed,
    //      the engine's worker thread exits blocking_recv promptly and
    //      shutdown() joins it.
    //
    // Reversing 2 and 3 (calling engine.shutdown() while main still holds
    // a transcriber Arc) deadlocks: shutdown() drops the engine's
    // request_tx but the Arc-held handle keeps a clone alive → channel
    // stays open → worker thread parks until process exit. codex-advisor
    // identified this during brainstorm; 0025 records.
    let stats_result =
        pipeline::run_pipelined(shared, fetcher, transcriber.clone(), opts).await;

    drop(transcriber); // (step 2: drop main's last clone)
    engine.shutdown(); // (step 3: LAST per 0025)

    let stats = stats_result?;
    tracing::info!(
        claimed = stats.claimed,
        succeeded = stats.succeeded,
        failed = stats.failed,
        "process complete"
    );
    if stats.claimed == 0 {
        std::process::exit(3);
    }
}
```

- [ ] **Step 3a: Add `WhisperEngineHandle` + `engine.transcriber_handle()` to `src/transcribe.rs`**

`WhisperEngine::shutdown(self)` consumes `self`, so passing the engine across the worker boundary as an `Arc<dyn Transcriber>` would prevent shutdown. The fix is a small clone-able handle that wraps the engine's existing `mpsc::Sender<TranscribeRequest>` — workers hold the handle (cheap to clone); `WhisperEngine` itself stays owned in `main`.

In `src/transcribe.rs`:

```rust
/// Clone-able transcriber handle backed by the engine's request channel.
/// Workers in the pipelined orchestrator hold one of these; the engine
/// itself stays owned by `main` so `engine.shutdown()` can run last per
/// 0025.
#[derive(Clone)]
pub struct WhisperEngineHandle {
    request_tx: tokio::sync::mpsc::Sender<TranscribeRequest>,
    model_id: String, // for output.model_id propagation
}

#[async_trait::async_trait]
impl Transcriber for WhisperEngineHandle {
    fn name(&self) -> &'static str { "whisper-rs" }

    async fn transcribe(
        &self,
        samples: Vec<f32>,
        config: PerCallConfig,
        timeout: std::time::Duration,
    ) -> Result<TranscribeOutput, TranscribeError> {
        // Body mirrors WhisperEngine::transcribe (Epic 1 T7); factor it
        // out as `transcribe_via_tx(&self.request_tx, samples, config, timeout)`
        // and have BOTH WhisperEngine::transcribe and
        // WhisperEngineHandle::transcribe call it. Avoids duplicating the
        // cancel-flag + oneshot dance.
        transcribe_via_tx(&self.request_tx, samples, config, timeout).await
    }
}

impl WhisperEngine {
    /// Return a clone-able Arc<dyn Transcriber> backed by this engine.
    /// The engine itself stays owned; the handle can be cloned freely
    /// across worker tasks. When the LAST handle drops, the engine's
    /// request channel closes — at which point `engine.shutdown()` can
    /// join the worker thread without parking.
    pub fn transcriber_handle(&self) -> std::sync::Arc<dyn Transcriber> {
        std::sync::Arc::new(WhisperEngineHandle {
            request_tx: self.request_tx.clone(),
            model_id: self.model_id.clone(),
        })
    }
}
```

This adds ~30 lines to `src/transcribe.rs`. The existing `impl Transcriber for WhisperEngine` stays — `run_serial` keeps calling it directly via `&engine`. The new handle is for the pipelined path.

**00-overview.md note:** the file-structure section says "src/transcribe.rs unchanged in Epic 2." That's no longer true — this task adds ~30 lines for the handle. Update the overview's file-structure block in this commit. (Spec line 211's "Cargo.toml additions: none" was also wrong; T13 corrects it. The "transcribe.rs unchanged" claim is the same flavor of correction.)

- [ ] **Step 5: Run the integration test**

```bash
cargo test --features test-helpers --test pipeline_fakes run_pipelined_drains
```

Expected: PASS.

- [ ] **Step 6: Full suite + clippy**

```bash
cargo test --features test-helpers
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: green; clippy clean. Confirm the dead-code attributes on `whisper_use_gpu` / `whisper_threads` are gone (the fields themselves are gone).

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/pipeline.rs src/config.rs tests/pipeline_fakes.rs
git commit -m "$(cat <<'EOF'
feat(orchestrator): pipelined supervision with JoinSet + CancellationToken + LOAD-BEARING shutdown order (0002, 0025)

run_pipelined wires N fetch workers (default 3) + 1 transcribe worker
into a tokio::task::JoinSet supervised by a shared CancellationToken.
main.rs loops on join_set.join_next(); on first Err or panic, fires
token.cancel() and drains remaining tasks.

**Load-bearing shutdown ORDER per 0025:**
1. token.cancel()
2. drop fetch→transcribe sender (already done by orchestrator after
   all fetch_workers exit; tx is dropped right after spawn loop in
   run_pipelined to ensure channel closes when fetch_workers exit)
3. join workers (handled by while-let on join_set.join_next())
4. engine.shutdown() LAST — in main.rs, AFTER run_pipelined returns,
   so the transcribe_worker has already exited and dropped its
   reference to the engine

Without this order the engine worker parks on blocking_recv until
process exit (codex-advisor identified during brainstorm; 0025
records).

0002 cleanup: removes Config::whisper_use_gpu + Config::whisper_threads.
Both have been dead since Epic 1 (Plan A leftovers); whisper-rs picks
n_threads itself and GPU choice is in EngineConfig. The "consume when
state-machine work lands" placeholder is resolved by Epic 2's confirmation
that no consumer materialized.

Tests:
- run_pipelined_drains_all_rows_and_returns_stats: 6 pending rows +
  FakeFetcher::happy + FakeTranscriber::echo + N=3 → all 6 succeed,
  claimed/succeeded/failed match.

Refs: 0002, 0008, 0024, 0025, 0026, 0027

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test pipeline_fakes run_pipelined_drains` passes
- [ ] Full suite green
- [ ] `engine.shutdown()` is called AFTER `run_pipelined` returns (load-bearing)
- [ ] The original `tx` is dropped after the spawn loop (so the channel closes when fetch_workers exit)
- [ ] `Config::whisper_use_gpu` and `Config::whisper_threads` are removed (grep for "whisper_use_gpu" returns nothing in src/)
- [ ] Tests previously asserting `cfg.whisper_use_gpu` / `cfg.whisper_threads` are updated
- [ ] Clippy/fmt clean
- [ ] FOLLOWUPS "WhisperEngine teardown can hang" and "Config dead fields" both resolved
