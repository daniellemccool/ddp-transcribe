//! Subcommand dispatch — the library half of the thin-bin/fat-lib split
//! (0045). `main.rs` parses, inits tracing, and calls [`dispatch`]; every
//! subcommand's work lives here. Exit semantics travel back to main as a
//! [`CommandExit`] value: nothing in this module (or anywhere else in the
//! library) may call `std::process::exit`.

use anyhow::{Context, Result};

use crate::{
    backfill, batch, classification, cli, config, fetcher, ingest, metadata_loader, output,
    pipeline, state, status, transcribe, Cli,
};

/// The process-exit semantics a subcommand can request, carried back to
/// `main` instead of being executed in the library (0045).
///
/// The two non-zero codes are the pre-existing operator contract: `3` means
/// `process` claimed nothing (an empty batch, not an error), `1` means
/// `status --verify` found the pipeline not pause-safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandExit {
    Success,
    NoClaims,
    VerifyFailed,
    /// Breaker ADR (0050): the run-global consecutive-no-success streak
    /// reached `--breaker-threshold` and the run was aborted.
    BreakerTripped,
}

impl CommandExit {
    pub fn code(&self) -> i32 {
        match self {
            CommandExit::Success => 0,
            CommandExit::NoClaims => 3,
            CommandExit::VerifyFailed => 1,
            CommandExit::BreakerTripped => 4,
        }
    }
}

pub async fn dispatch(cli: Cli) -> Result<CommandExit> {
    let cfg = config::Config::from_args(&cli.global);
    log_resolved_config(&cfg, &cli.command);

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
                    return Ok(CommandExit::Success);
                }
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).context("creating state.sqlite parent dir")?;
            }
            let _store = state::Store::open(path)?;
            tracing::info!(path = %path.display(), "state.sqlite initialized");
        }
        cli::Command::Ingest {
            dry_run,
            window_start,
            window_end,
        } => {
            cli::validate_window_order("ingest", window_start, window_end)?;
            let mut store = state::Store::open(&cfg.state_db).context("opening state DB")?;
            let window = ingest::WindowBounds::from_dates(window_start, window_end);
            let stats =
                ingest::ingest(&cfg.inbox, &mut store, window, dry_run).context("ingest failed")?;
            tracing::info!(
                dry_run,
                files = stats.files_processed,
                files_skipped_unparseable = stats.files_skipped_unparseable,
                files_skipped_already_ingested = stats.files_skipped_already_ingested,
                videos = stats.unique_videos_seen,
                history = stats.watch_history_rows_processed,
                duplicates = stats.watch_history_duplicates,
                short_links_skipped = stats.short_links_skipped,
                invalid_urls_skipped = stats.invalid_urls_skipped,
                date_parse_failures = stats.date_parse_failures,
                computed_out_of_window = stats.computed_out_of_window,
                backfilled_raw_dates = stats.backfilled_raw_dates,
                "ingest complete"
            );
            println!("ingest: {stats}{}", if dry_run { " (dry-run)" } else { "" });
        }
        cli::Command::Process {
            max_videos,
            cookies_file,
            retries,
            checkpoint_cmd,
            checkpoint_every,
            breaker_threshold,
        } => {
            let mut store = state::Store::open(&cfg.state_db).context("opening state DB")?;
            std::fs::create_dir_all(&cfg.transcripts).context("creating transcripts dir")?;
            // Tmp cleanup at startup
            let removed =
                output::artifacts::cleanup_tmp_files(&cfg.transcripts, cfg.stale_claim_threshold)?;
            if removed > 0 {
                tracing::info!(removed, "cleaned up leftover .tmp files");
            }

            let work_dir = cfg.transcripts.join(".work");
            std::fs::create_dir_all(&work_dir).context("creating work dir")?;

            // Epic 5b: same startup hygiene for the fetcher's per-acquire
            // attempt dirs. Age-gated on the same threshold — a fresh dir may
            // belong to a live sibling process's in-flight fetch.
            let swept = output::artifacts::cleanup_work_dirs(&work_dir, cfg.stale_claim_threshold)?;
            if swept > 0 {
                tracing::info!(swept, "cleaned up leftover fetch work dirs");
            }

            // Epic 4a: the active classification policy — the operator's
            // `--classification` TOML (validated, hard-fail) or the
            // evidence-derived compiled default. Built before the sweep
            // (which consumes it) and before the engine (fail fast on
            // policy before paying model load).
            let table = match &cfg.classification_path {
                Some(p) => {
                    let text = std::fs::read_to_string(p)
                        .with_context(|| format!("reading classification file {}", p.display()))?;
                    classification::ClassificationTable::from_toml_str(&text).with_context(
                        || format!("validating classification file {}", p.display()),
                    )?
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

            // Epic 4a: open the batch_runs row (policy snapshot + params) and
            // run the start-of-batch sweep of parked failures BEFORE the
            // engine loads its model — fail fast on policy before paying
            // that cost.
            // Epic 5a: the checkpoint hook's configuration rides the
            // batch_runs params so a census can attribute a run's mid-run
            // syncing (or lack of it). `null`/`null` when the feature is
            // off — the path is recorded verbatim; it is an operator script
            // path, not a credential (unlike --cookies-file, which stays a
            // boolean presence flag per 0035).
            let checkpoint = checkpoint_cmd.map(|cmd| pipeline::CheckpointConfig {
                cmd,
                every: checkpoint_every,
            });
            let params_json = serde_json::json!({
                "retries": retries,
                "max_videos": max_videos,
                "cookies_present": cookies_file.is_some(),
                "download_workers": cfg.download_workers,
                "worker_host": hostname_or_default(),
                "checkpoint_cmd": checkpoint.as_ref().map(|c| c.cmd.display().to_string()),
                "checkpoint_every_secs": checkpoint.as_ref().map(|c| c.every.as_secs()),
                "breaker_threshold": breaker_threshold,
            })
            .to_string();
            let run_id = store.open_batch_run(&params_json, classification.source_toml())?;
            let sweep_stats =
                batch::run_sweep(&mut store, &classification, retries, cookies_file.is_some())?;

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
            // `store` moves into `shared` here — AFTER the sweep + open_batch_run
            // block above, which needed `&mut store` directly.
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
                cookies_file,
                classification: std::sync::Arc::clone(&classification),
                retries,
                checkpoint,
                breaker_threshold,
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
            let stats_result = pipeline::run_pipelined(
                // 0003 disclosed deviation from the brief's literal snippet:
                // clone the Arc rather than moving `shared` into
                // run_pipelined, so this binding survives past `.await` for
                // the census-close block below (which needs `shared.try_lock()`).
                std::sync::Arc::clone(&shared),
                fetcher,
                std::sync::Arc::clone(&transcriber),
                opts,
            )
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
                // Epic 4a: capped-retry outcome counters.
                requeued_for_retry = stats.requeued_for_retry,
                exhausted_retries = stats.exhausted_retries,
                parked_for_cookies = stats.parked_for_cookies,
                // Epic 5a: the operator checkpoint hook's outcome. A
                // nonzero checkpoints_failed means mid-run syncing did not
                // happen — an alarm, but never a run failure.
                checkpoints_run = stats.checkpoints_run,
                checkpoints_failed = stats.checkpoints_failed,
                "process complete"
            );

            let census = batch::BatchCensus {
                sweep: sweep_stats,
                run: batch::RunCensus::from(&stats),
            };
            {
                let mut guard = shared.try_lock().context(
                    "store lock free after run_pipelined resolved — workers have exited",
                )?;
                let closed = guard.close_batch_run(
                    run_id,
                    &serde_json::to_string(&census).context("serializing census")?,
                )?;
                if closed == 0 {
                    tracing::warn!(run_id, "close_batch_run matched no open row");
                }
            }
            print!("{census}");

            if stats.breaker_tripped {
                return Ok(CommandExit::BreakerTripped);
            }

            if stats.claimed == 0 {
                return Ok(CommandExit::NoClaims);
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
        cli::Command::Status {
            video_id,
            respondent_id,
            errors,
            retryable,
            verify,
            json,
        } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "status: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            let store = state::Store::open(path).context("opening state DB")?;
            if let Some(id) = video_id {
                let report = status::build_video_detail(&store, &id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_video_detail(&report));
                }
            } else if let Some(id) = respondent_id {
                let report = status::build_respondent_report(&store, &id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_respondent(&report));
                }
            } else if errors || retryable {
                let lists = status::FailureLists {
                    errors: if errors {
                        Some(
                            store
                                .list_terminal_failures()
                                .context("listing terminal failures")?,
                        )
                    } else {
                        None
                    },
                    retryable: if retryable {
                        Some(
                            store
                                .list_failed_retryable()
                                .context("listing retryable failures")?,
                        )
                    } else {
                        None
                    },
                };
                if json {
                    println!("{}", serde_json::to_string_pretty(&lists)?);
                } else {
                    print!("{}", status::render_failure_lists(&lists));
                }
            } else {
                let mut report = status::build_report(&store, state::unix_now())?;
                if verify {
                    report.verify = Some(
                        status::run_verify(&store, &cfg.transcripts, &report.counts)
                            .context("running --verify checks")?,
                    );
                }
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_report(&report));
                }
                if let Some(v) = &report.verify {
                    if !v.pause_safe {
                        return Ok(CommandExit::VerifyFailed);
                    }
                }
            }
        }
        cli::Command::RecomputeWindow {
            window_start,
            window_end,
            clear,
            dry_run,
        } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "recompute-window: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            // --clear == both bounds None (everything in-window); clap
            // guarantees clear XOR window flags.
            let window = if clear {
                ingest::WindowBounds::default()
            } else {
                cli::validate_window_order("recompute-window", window_start, window_end)?;
                ingest::WindowBounds::from_dates(window_start, window_end)
            };
            let mut store = state::Store::open(path).context("opening state DB")?;
            if dry_run {
                let n = store.count_window_mismatches(window.start, window.end_exclusive)?;
                println!("recompute-window dry-run: would change {n} row(s)");
            } else {
                let n = store.recompute_window(window.start, window.end_exclusive)?;
                println!("recompute-window: changed {n} row(s)");
            }
        }
        cli::Command::LoadMetadata { dry_run } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "load-metadata: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            let mut store = state::Store::open(path).context("opening state DB")?;
            let stats = metadata_loader::load_metadata(&mut store, dry_run)?;
            tracing::info!(%stats, dry_run, "load-metadata complete");
            println!(
                "load-metadata: {stats}{}",
                if dry_run { " (dry-run)" } else { "" }
            );
        }
        cli::Command::RequeueFailures {
            error_kind,
            max_attempts,
            older_than,
            include_terminal,
            all,
            max,
            dry_run,
        } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "requeue-failures: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            let mut store = state::Store::open(path).context("opening state DB")?;
            let filter = state::RequeueFilter {
                error_kinds: error_kind,
                max_attempts,
                older_than,
                include_terminal,
                all,
                max,
            };
            // 0046 attribution: distinct from the sweep's literal
            // `worker_id = 'sweep'`, so an audit can tell an operator
            // override from an automatic transition at a glance.
            let actor = format!("operator:{}-{}", hostname_or_default(), std::process::id());
            let outcome = store
                .requeue_failures(&filter, &actor, dry_run)
                .context("requeue-failures failed")?;
            tracing::info!(
                matched = outcome.matched,
                requeued = outcome.requeued,
                dry_run,
                actor,
                "requeue-failures complete"
            );
            if outcome.matched == 0 {
                println!("requeue-failures: 0 rows matched");
                return Ok(CommandExit::Success);
            }
            for (kind, count) in &outcome.by_kind {
                println!("requeue-failures:   {kind}: {count}");
            }
            println!(
                "requeue-failures: {} row(s) matched, {} requeued{}",
                outcome.matched,
                outcome.requeued,
                if dry_run { " (dry-run)" } else { "" }
            );
        }
        cli::Command::BackfillMetadata { limit, dry_run } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "backfill-metadata: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            let mut store = state::Store::open(path).context("opening state DB")?;
            // Cohort size prints in both modes: the operator's orientation
            // for a run whose per-video cost is a network round trip.
            let cohort = store.count_succeeded_missing_metadata()?;
            if dry_run {
                tracing::info!(cohort, "backfill-metadata dry-run");
                println!(
                    "backfill-metadata: cohort {cohort} succeeded videos missing metadata (dry-run)"
                );
            } else {
                println!("backfill-metadata: cohort {cohort} succeeded videos missing metadata");
                let stats =
                    backfill::backfill_metadata(&mut store, cfg.ytdlp_timeout, limit).await?;
                tracing::info!(%stats, "backfill-metadata complete");
                println!("backfill-metadata: {stats}");
            }
        }
    }

    Ok(CommandExit::Success)
}

/// Config echo scoped to what the invoked subcommand actually consumes
/// (epic-4 followup: echoing whisper_model_path for `ingest` sent the
/// operator chasing a "why is it using tiny?" false alarm). Process is
/// the only model-loading arm; ingest reads the inbox; status --verify
/// reads the transcripts tree.
fn log_resolved_config(cfg: &config::Config, command: &cli::Command) {
    match command {
        cli::Command::Process { .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            transcripts = ?cfg.transcripts,
            whisper_model_path = ?cfg.whisper_model_path,
            classification = ?cfg.classification_path,
            "config resolved"
        ),
        cli::Command::Ingest { .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            inbox = ?cfg.inbox,
            "config resolved"
        ),
        cli::Command::Status { verify: true, .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            transcripts = ?cfg.transcripts,
            "config resolved"
        ),
        cli::Command::Init
        | cli::Command::Migrate
        | cli::Command::Status { .. }
        | cli::Command::RecomputeWindow { .. }
        | cli::Command::LoadMetadata { .. }
        | cli::Command::BackfillMetadata { .. }
        | cli::Command::RequeueFailures { .. } => tracing::info!(
            profile = ?cfg.profile,
            state_db = ?cfg.state_db,
            "config resolved"
        ),
    }
}

/// Real hostname for worker attribution (two-instance deployments):
/// `/proc/sys/kernel/hostname` → `$HOSTNAME` → `"host"`. Before this, both
/// SRC instances reported the literal `"host"` and A/B attribution leaned on
/// pid ranges alone.
fn hostname_or_default() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "host".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaker ADR (0050): exit code 4 joins the existing map (0 success,
    /// 1 verify-failed, 3 zero-claims) via `CommandExit`.
    #[test]
    fn breaker_tripped_maps_to_exit_4() {
        assert_eq!(CommandExit::BreakerTripped.code(), 4);
    }
}
