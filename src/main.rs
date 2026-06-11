use anyhow::{Context, Result};
use clap::Parser;

mod audio;
mod canonical;
mod cli;
mod config;
mod errors;
mod fetcher;
mod ingest;
mod output;
mod pipeline;
mod process;
mod state;
mod transcribe;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    init_tracing(cli.global.log_format);
    let cfg = config::Config::from_args(&cli.global);
    tracing::info!(
        profile = ?cfg.profile,
        state_db = ?cfg.state_db,
        whisper_model_path = ?cfg.whisper_model_path,
        "config resolved"
    );

    match cli.command {
        cli::Command::Init => {
            let path = &cfg.state_db;
            if path.exists() {
                let store = state::Store::open(path)?;
                if let Some(version) = store.read_meta("schema_version")? {
                    tracing::info!(
                        path = %path.display(),
                        version = version.as_str(),
                        "state.sqlite already initialized; nothing to do"
                    );
                    return Ok(());
                }
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).context("creating state.sqlite parent dir")?;
            }
            let _store = state::Store::open(path)?;
            tracing::info!(path = %path.display(), "state.sqlite initialized");
        }
        cli::Command::Ingest { dry_run } => {
            let mut store = state::Store::open(&cfg.state_db).context("opening state DB")?;
            if dry_run {
                tracing::info!("dry-run: not yet implemented; running real ingest");
            }
            let stats = ingest::ingest(&cfg.inbox, &mut store).context("ingest failed")?;
            tracing::info!(
                files = stats.files_processed,
                videos = stats.unique_videos_seen,
                history = stats.watch_history_rows_processed,
                duplicates = stats.watch_history_duplicates,
                short_links_skipped = stats.short_links_skipped,
                invalid_urls_skipped = stats.invalid_urls_skipped,
                date_parse_failures = stats.date_parse_failures,
                "ingest complete"
            );
        }
        cli::Command::Process { max_videos } => {
            let store = state::Store::open(&cfg.state_db).context("opening state DB")?;
            std::fs::create_dir_all(&cfg.transcripts).context("creating transcripts dir")?;
            // Tmp cleanup at startup
            let removed = output::artifacts::cleanup_tmp_files(&cfg.transcripts)?;
            if removed > 0 {
                tracing::info!(removed, "cleaned up leftover .tmp files");
            }

            let work_dir = cfg.transcripts.join(".work");
            std::fs::create_dir_all(&work_dir).context("creating work dir")?;

            // Construct WhisperEngine once at the top of Process. Loads the
            // model on the worker thread and blocks until init succeeds or
            // fails (0016: model/state never leave the worker; the engine
            // handle owns the worker JoinHandle). For Epic 1's single-GPU
            // path, `gpu_device = 0`; `flash_attn` follows the cuda feature
            // flag (on for CUDA builds, off for local CPU dev).
            //
            // The std::sync::mpsc rendezvous inside `WhisperEngine::new`
            // blocks this executor thread until init reports back. That's
            // acceptable here because Process is the startup path; we have
            // not yet entered the per-video hot loop.
            //
            // 0025 ownership: `engine` is OWNED here (not Arc'd) so
            // `engine.shutdown()` (which consumes `self`) can run as
            // step 4 of the load-bearing shutdown ORDER below.
            let engine_config = transcribe::EngineConfig {
                model_path: cfg.whisper_model_path.clone(),
                gpu_device: 0,
                flash_attn: cfg!(feature = "cuda"),
            };
            let engine = transcribe::WhisperEngine::new(&engine_config)
                .context("constructing WhisperEngine")?;

            // T18 / 0025: workers see a clone-able `Arc<dyn Transcriber>`
            // (the `WhisperEngineHandle` wraps the engine's request
            // sender). The engine itself stays owned in main so
            // `engine.shutdown()` can fire LAST below.
            let transcriber: std::sync::Arc<dyn transcribe::Transcriber> =
                engine.transcriber_handle();
            let fetcher: std::sync::Arc<dyn fetcher::VideoFetcher> = std::sync::Arc::new(
                fetcher::ytdlp::YtDlpFetcher::new(&work_dir, cfg.ytdlp_timeout),
            );
            let shared: pipeline::SharedStore = std::sync::Arc::new(tokio::sync::Mutex::new(store));

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

            // ────────────────────────────────────────────────────────────
            // 0025 SHUTDOWN ORDER (load-bearing, four steps):
            //   1. token.cancel()                  ← inside run_pipelined
            //                                       on first worker Err.
            //   2. drop fetch→transcribe sender    ← inside run_pipelined
            //                                       (after spawn loop;
            //                                       channel closes when
            //                                       fetch workers exit).
            //   3. join_set.join_next() to done    ← inside run_pipelined
            //                                       (every worker drops
            //                                       its `transcriber`
            //                                       Arc clone on exit).
            //   4. engine.shutdown()               ← HERE, AFTER (a) the
            //                                       run_pipelined future
            //                                       resolves and (b) we
            //                                       drop main's own clone
            //                                       of `transcriber`.
            //
            // Reversing steps 2 and 4 (engine.shutdown() before draining
            // workers) wedges transcribe_worker on a dead engine.
            // Reversing steps 1 and 2 (drop sender before cancel) loses
            // the cancellation path through the in-flight transcribe
            // (no select! arm wins). The "drop main's own transcriber
            // Arc clone before engine.shutdown()" gesture below is the
            // bridge between step 3 (workers drop their clones) and step
            // 4 (engine teardown): the engine's worker thread only exits
            // blocking_recv when the LAST request_tx clone goes away.
            // ────────────────────────────────────────────────────────────
            let stats_result =
                pipeline::run_pipelined(shared, fetcher, std::sync::Arc::clone(&transcriber), opts)
                    .await;

            // Drop main's own `Arc<dyn Transcriber>` clone — this is the
            // last clone in this scope (workers dropped theirs as they
            // exited inside run_pipelined). Closes the engine's request
            // channel so step 4's `engine.shutdown()` can join cleanly.
            drop(transcriber);

            // 0025 step 4: engine teardown LAST. Consumes `engine` by
            // value; drops the engine's own request_tx, the worker
            // thread sees the closed channel, blocking_recv returns
            // None, and the join completes.
            engine.shutdown();

            let stats = stats_result?;
            tracing::info!(
                claimed = stats.claimed,
                succeeded = stats.succeeded,
                failed = stats.failed,
                // T9 (T5-review carry-forward): in Phase 2 the
                // concurrent workers can reach the stale-after-success
                // / stale-after-failure paths if a row's claim is swept
                // mid-flight. Surface both counters so an operator can
                // see them in the process-complete line.
                stale_after_success = stats.stale_after_success,
                stale_after_failure = stats.stale_after_failure,
                "process complete"
            );
            if stats.claimed == 0 {
                std::process::exit(3);
            }
        }
        cli::Command::Migrate => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "migrate: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            state::migrate::run_migrate(path).context("running migrate")?;
            tracing::info!(path = %path.display(), "migrate complete");
        }
    }

    Ok(())
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "host".to_string())
}

fn init_tracing(format: cli::LogFormat) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match format {
        cli::LogFormat::Human => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
        cli::LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(filter)
                .init();
        }
    }
}
