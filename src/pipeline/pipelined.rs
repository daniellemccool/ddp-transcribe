//! Pipelined orchestrator: N fetch workers feed 1 transcribe worker via a
//! bounded mpsc channel (0027).
//!
//! T15 landed the skeleton + shared types. T16 lands the fetch worker.
//! T17 lands the transcribe worker. T18 will wire `run_pipelined`'s
//! `JoinSet` + `CancellationToken` shutdown order per 0025 and flip the
//! `--pipelined` branch in `main.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::{
    fetch_and_decode, write_artifacts_and_mark, ProcessOptions, ProcessOutcome, ProcessStats,
};
use crate::errors::TranscribeError;
use crate::fetcher::VideoFetcher;
use crate::state::{Claim, Store};
use crate::transcribe::{PerCallConfig, Transcriber};

/// Shared mutable access to the `Store` across N fetch workers + 1
/// transcribe worker.
///
/// Implemented as `Arc<tokio::sync::Mutex<Store>>` because the workers
/// contend on `claim_next` (which already serializes via SQLite
/// `BEGIN IMMEDIATE` — the Mutex is for Rust-level `&mut self` access;
/// SQLite handles inter-connection contention). The expensive work
/// (fetch, decode, transcribe, write) happens *outside* the lock; only
/// `claim_next` / `mark_*` calls hold it, and those are sub-millisecond.
///
/// Alternative considered (per 0025 brainstorm): each worker opens its
/// own `Store::open` connection. Also valid. The `Mutex<Store>` choice
/// keeps the type surface uniform (one Store handle for both serial and
/// pipelined paths). A reviewer should re-check this is fine before T16/
/// T17 commit; if the Mutex shows up as a bottleneck, the alternative is
/// one Store per worker.
pub type SharedStore = std::sync::Arc<tokio::sync::Mutex<Store>>;

/// Channel payload from fetch workers to the transcribe worker.
///
/// Per 0027: fetch workers do WAV decode in parallel so the transcribe
/// path is lean. The `PathBuf` rides through for cleanup after
/// `mark_succeeded` per 0008. `samples_len` is carried so the transcribe
/// worker can compute `duration_s = samples_len as f64 / 16_000.0`
/// without re-deriving from the moved `samples` Vec (which is consumed
/// by the engine.transcribe() call).
///
/// `fetcher_name` carries `VideoFetcher::name()` from the fetch worker
/// that produced this item, so the transcribe worker can populate the
/// artifact JSON's `fetcher` field without a hardcoded literal (T18
/// Decision B). The 'static lifetime mirrors the trait method's
/// signature; every fetcher's name is a string literal.
///
/// `pub` (not `pub(crate)`) so the integration tests in
/// `tests/pipeline_fakes.rs` can construct/inspect items. After T18
/// `run_pipelined` is wired in `main.rs`, every field is read on the
/// bin path (via destructuring inside `transcribe_worker`) — no
/// `dead_code` suppression needed.
#[derive(Debug)]
pub struct FetchedItem {
    pub claim: Claim,
    pub samples: Vec<f32>,
    pub samples_len: usize,
    pub wav_path: PathBuf,
    pub fetcher_name: &'static str,
}

/// Phase 2 fetch worker. Claims pending rows; fetches + decodes WAVs;
/// pushes a [`FetchedItem`] onto the channel. Exits cleanly on
/// `claim_next == None` (drain semantics per 0026 — no polling). On
/// retryable error, calls `mark_retryable_failure` and continues. On
/// Bug-class signals (channel closed, store error), returns `Err` — the
/// orchestrator reacts per 0025.
///
/// **Mutex hold-time discipline.** The shared store guard is acquired
/// briefly for `claim_next` and `mark_retryable_failure` only; it is
/// dropped before the multi-second `fetch_and_decode().await` so other
/// fetch workers can claim concurrently (SQLite's BEGIN IMMEDIATE
/// already serializes the actual claim transaction at the connection
/// level).
///
/// **Cancellation latency.** Polled at the loop top. The hottest await
/// inside the loop is `fetcher.acquire()` (multi-second yt-dlp call);
/// it is intentionally NOT wrapped in `tokio::select!` for Phase 2 —
/// yt-dlp's `kill_on_drop` already terminates the subprocess when the
/// future is dropped. If shutdown latency becomes operationally painful,
/// a follow-up can add a `select!` around `acquire`.
///
/// **Stale-after-failure handling (design amendment).** When
/// `mark_retryable_failure` returns `Ok(0)`, the row's claim was swept
/// (or re-assigned) between this worker's `claim_next` and its
/// `mark_retryable_failure` call — the predicate
/// `status='in_progress' AND claimed_by=?` no longer matches. Symmetric
/// to T9's `StaleAfterSuccess` outcome on the success side: increment
/// `stats_stale_after_failure` and continue, do NOT return Err. T18
/// merges this counter into `ProcessStats` after worker drain.
///
/// T18: `run_pipelined` is the in-bin caller; the prior
/// `#[allow(dead_code)]` placeholder is lifted as part of this wiring
/// per 0002. Integration tests in `tests/pipeline_fakes.rs` continue
/// to exercise it directly.
pub async fn fetch_worker(
    token: CancellationToken,
    store: SharedStore,
    fetcher: Arc<dyn VideoFetcher>,
    sender: mpsc::Sender<FetchedItem>,
    stats_stale_after_failure: Arc<AtomicUsize>,
    claims_counter: Arc<AtomicUsize>,
    opts: Arc<ProcessOptions>,
) -> Result<()> {
    let worker_id = opts.worker_id.clone();
    loop {
        // Respect cancellation: check before every claim. The orchestrator
        // calls token.cancel() during shutdown; we exit before claiming
        // more work.
        if token.is_cancelled() {
            tracing::info!(worker = %worker_id, "fetch_worker: cancellation observed; exiting");
            return Ok(());
        }

        // Acquire the store guard ONLY for the claim transaction; drop it
        // before the long-running fetch_and_decode below.
        //
        // Honor --max-videos cap. The check, claim_next, and counter
        // increment are all inside this Mutex<Store> guard scope, so the
        // entire check+claim+increment is atomic across concurrent fetch
        // workers (the Mutex serializes them). Zero overshoot guaranteed.
        let claim = {
            let mut guard = store.lock().await;
            let at_cap = match opts.max_videos {
                Some(max) => claims_counter.load(Ordering::Relaxed) >= max,
                None => false,
            };
            if at_cap {
                tracing::info!(
                    worker = %worker_id,
                    "fetch_worker: max_videos cap reached; exiting"
                );
                None
            } else {
                let c = guard.claim_next(&worker_id)?;
                if c.is_some() {
                    claims_counter.fetch_add(1, Ordering::Relaxed);
                }
                c
            }
        };
        let Some(claim) = claim else {
            // 0026: drain semantics. Worker exits on None.
            tracing::info!(worker = %worker_id, "fetch_worker: queue drained; exiting");
            return Ok(());
        };

        // Inline fetch + decode (T15 helper). Errors here are application
        // failures, not Bug-class — classify as retryable and continue.
        match fetch_and_decode(fetcher.as_ref(), &claim).await {
            Ok((samples, wav_path)) => {
                let samples_len = samples.len();
                let item = FetchedItem {
                    claim,
                    samples,
                    samples_len,
                    wav_path,
                    // T18 Decision B: stamp the fetcher's identifier onto
                    // the item so the transcribe worker can pass it to
                    // `write_artifacts_and_mark` (instead of a hardcoded
                    // "ytdlp" literal).
                    fetcher_name: fetcher.name(),
                };
                if sender.send(item).await.is_err() {
                    // Channel closed — transcribe_worker exited (panic or
                    // shutdown). Bug-class signal up to the orchestrator;
                    // the WAV is on disk and the claim remains in_progress
                    // until the next sweep recovers it.
                    tracing::error!(
                        worker = %worker_id,
                        "fetch_worker: channel closed; transcribe_worker has exited"
                    );
                    return Err(anyhow!(
                        "fetch→transcribe channel closed; transcribe_worker has exited"
                    ));
                }
            }
            Err(e) => {
                let video_id = claim.video_id.clone();
                let msg = format!("{e:#}");
                tracing::error!(
                    worker = %worker_id,
                    video_id = video_id.as_str(),
                    error = %e,
                    "fetch_worker: failure; classifying as retryable"
                );
                // Acquire guard briefly for the mutator; drop before the
                // next loop iteration's claim.
                let result = {
                    let mut guard = store.lock().await;
                    // Epic 2 MVP placeholder kind "Fetch" per 0023; Epic 3
                    // swaps in typed RetryableKind via classifier dispatch.
                    guard.mark_retryable_failure(&video_id, &worker_id, "Fetch", &msg)
                };
                match result {
                    Ok(0) => {
                        // Stale claim: predicate
                        // `status='in_progress' AND claimed_by=?` missed.
                        // Symmetric to StaleAfterSuccess on the success
                        // side. Counter is monotonic telemetry — Relaxed
                        // is fine (no synchronization signal).
                        stats_stale_after_failure.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            worker = %worker_id,
                            video_id = video_id.as_str(),
                            "fetch_worker: mark_retryable_failure swallowed: \
                             row no longer claimed by this worker \
                             (probably swept + re-claimed by another process)"
                        );
                    }
                    Ok(_) => { /* normal retryable flip */ }
                    Err(store_err) => {
                        // The store call itself failed — Bug-class.
                        return Err(store_err.context("fetch_worker: mark_retryable_failure"));
                    }
                }
                // continue to next iteration.
            }
        }
    }
}

/// Phase 2 transcribe worker. 1 instance per 0027. Drains
/// [`FetchedItem`]s emitted by N fetch workers and runs phases 3+4 per
/// item: transcribe (NO store lock) → write artifacts + mark_succeeded
/// (store lock, sub-50ms critical section) → cleanup wav.
///
/// **Loop structure.** A `biased` `tokio::select!` at the loop top
/// prefers the cancellation arm when both are ready — without `biased`,
/// a constant stream of items could starve cancellation. Channel close
/// (`recv() == None`) exits cleanly (fetch workers drained per 0026).
///
/// **0008 invariant (load-bearing).** Artifacts (txt + json) must be
/// durable on disk BEFORE `mark_succeeded`. The
/// [`write_artifacts_and_mark`] helper (factored from T15's
/// `transcribe_and_write` per T17 Path A) is the single source of truth
/// for that ordering — both serial and pipelined paths delegate to it.
///
/// **Mutex hold-time discipline.** `transcriber.transcribe(samples, …)`
/// is NOT inside `store.lock().await` — on `large-v3-turbo-q5_0` that
/// call is ~1s, and holding the store mutex across it would starve fetch
/// workers' `claim_next`. The store guard is acquired only for the
/// write + mark phase (sub-50ms total).
///
/// **Cancellation composition (ADR 0025 strict text).** Per ADR 0025:
/// the per-item transcribe call is wrapped in `tokio::select!` against
/// the `CancellationToken`. When `token.cancel()` fires, the select arm
/// wins, the in-flight transcribe future drops, the `CancelOnDrop` chain
/// (per ADR 0012) fires the per-request `Arc<AtomicBool>`, and
/// whisper.cpp's `abort_callback` aborts inference within milliseconds.
///
/// **Error classification.**
/// - [`TranscribeError::Cancelled`]: row stays `in_progress`, sweep
///   recovers on next startup. Worker returns `Ok(())` — coordinated
///   shutdown is not a Bug.
/// - [`TranscribeError::Bug`]: worker returns `Err` — the orchestrator
///   reacts per 0025 (cancel all + drain JoinSet).
/// - Other variants (currently `Timeout`, `Failed`, `EmptyOutput`):
///   classified as retryable via `mark_retryable_failure(kind="Transcribe")`.
///   On `Ok(0)` (claim swept mid-flight), increment
///   `stats_stale_after_failure` and continue (symmetric to T16's
///   amendment for the fetch side).
///
/// **Success-side stale-claim routing (T18 Decision A).** When
/// `write_artifacts_and_mark` returns `Ok(StaleAfterSuccess)`,
/// `mark_succeeded` updated 0 rows: artifacts are durable per 0008,
/// but the predicate `status='in_progress' AND claimed_by=?` missed
/// (a concurrent sweep cleared the claim mid-transcription). The
/// worker increments `stats_stale_after_success` and continues — this
/// is the success-side counterpart to `stats_stale_after_failure`.
///
/// T18: `run_pipelined` is the in-bin caller; the prior
/// `#[allow(dead_code)]` placeholder is lifted as part of this wiring
/// per 0002. Integration tests in `tests/pipeline_fakes.rs` continue
/// to exercise it directly.
pub async fn transcribe_worker(
    token: CancellationToken,
    mut receiver: mpsc::Receiver<FetchedItem>,
    transcriber: Arc<dyn Transcriber>,
    store: SharedStore,
    stats_stale_after_failure: Arc<AtomicUsize>,
    stats_stale_after_success: Arc<AtomicUsize>,
    opts: Arc<ProcessOptions>,
) -> Result<()> {
    let worker_id = opts.worker_id.clone();

    loop {
        // `biased` prefers the cancellation arm when both are ready —
        // prevents a constant stream of FetchedItems from starving
        // cancellation. Per 0025 this is the propagation entry point.
        let item = tokio::select! {
            biased;
            () = token.cancelled() => {
                tracing::info!(worker = %worker_id, "transcribe_worker: cancellation observed; exiting");
                return Ok(());
            }
            maybe_item = receiver.recv() => match maybe_item {
                Some(it) => it,
                None => {
                    tracing::info!(worker = %worker_id, "transcribe_worker: channel closed; exiting");
                    return Ok(());
                }
            }
        };

        let FetchedItem {
            claim,
            samples,
            samples_len,
            wav_path,
            fetcher_name,
        } = item;

        tracing::info!(
            video_id = claim.video_id.as_str(),
            attempt = claim.attempt_count,
            "transcribe_worker: processing item"
        );

        // Phase 3: transcribe OUTSIDE the store mutex. Per the
        // Mutex-hold-time discipline at the function docstring,
        // holding the mutex across this ~1s await would starve fetch
        // workers' `claim_next`.
        let per_call = PerCallConfig {
            compute_lang_probs: opts.compute_lang_probs,
            ..PerCallConfig::default()
        };
        let transcribe_result = tokio::select! {
            biased;
            () = token.cancelled() => {
                tracing::info!(worker = %worker_id, video_id = %claim.video_id.as_str(), "transcribe_worker: cancellation during transcribe; exiting");
                return Ok(());
            }
            r = transcriber.transcribe(samples, per_call, opts.transcribe_timeout) => r,
        };

        match transcribe_result {
            Ok(transcribe_output) => {
                tracing::info!(
                    video_id = claim.video_id.as_str(),
                    chars = transcribe_output.text.len(),
                    language = transcribe_output.language.as_str(),
                    "transcribed"
                );

                // Phase 4: write artifacts + mark_succeeded under the
                // store mutex. The 0008 invariant lives inside
                // `write_artifacts_and_mark` — both serial and pipelined
                // call the same helper so a future change to the
                // ordering would land in one place.
                let mut guard = store.lock().await;
                let outcome = write_artifacts_and_mark(
                    &mut guard,
                    transcribe_output,
                    &claim,
                    samples_len,
                    wav_path,
                    fetcher_name,
                    transcriber.name(),
                    &opts,
                )
                .with_context(|| format!("write_artifacts_and_mark for {}", claim.video_id))?;
                drop(guard);

                // T18 Decision A: route `StaleAfterSuccess` into a counter
                // symmetric to `stats_stale_after_failure`. Artifacts are
                // durable per 0008; the row sits in pending and will be
                // re-claimed. Counter is monotonic telemetry — Relaxed is
                // fine.
                if outcome == ProcessOutcome::StaleAfterSuccess {
                    stats_stale_after_success.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(TranscribeError::Cancelled) => {
                // Coordinated shutdown, not a row failure. Row stays
                // `in_progress`; sweep recovers on next startup. Do
                // NOT increment any failure counter.
                tracing::info!(
                    worker = %worker_id,
                    video_id = claim.video_id.as_str(),
                    "transcribe_worker: TranscribeError::Cancelled — exiting Ok"
                );
                return Ok(());
            }
            Err(e @ TranscribeError::Bug { .. }) => {
                // Bug-class: orchestrator reacts per 0025 (cancel all
                // + drain). Wrap with anyhow context for the JoinSet.
                tracing::error!(
                    worker = %worker_id,
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "transcribe_worker: TranscribeError::Bug — returning Err"
                );
                return Err(
                    anyhow::Error::new(e).context(format!("transcribe Bug for {}", claim.video_id))
                );
            }
            Err(e) => {
                // Retryable transcribe error: classify via
                // mark_retryable_failure. Symmetric to T16's
                // `stats_stale_after_failure` amendment on Ok(0).
                let video_id = claim.video_id.clone();
                let msg = format!("{e:#}");
                tracing::error!(
                    worker = %worker_id,
                    video_id = video_id.as_str(),
                    error = %e,
                    "transcribe_worker: retryable transcribe failure"
                );
                let result = {
                    let mut guard = store.lock().await;
                    // Epic 2 MVP placeholder kind "Transcribe" per 0023;
                    // Epic 3 swaps in typed RetryableKind via
                    // classifier dispatch.
                    guard.mark_retryable_failure(&video_id, &worker_id, "Transcribe", &msg)
                };
                match result {
                    Ok(0) => {
                        // Stale claim: predicate
                        // `status='in_progress' AND claimed_by=?` missed.
                        // Counter is monotonic telemetry — Relaxed is
                        // fine.
                        stats_stale_after_failure.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            worker = %worker_id,
                            video_id = video_id.as_str(),
                            "transcribe_worker: mark_retryable_failure swallowed: \
                             row no longer claimed by this worker \
                             (probably swept + re-claimed by another process)"
                        );
                    }
                    Ok(_) => { /* normal retryable flip */ }
                    Err(store_err) => {
                        // The store call itself failed — Bug-class.
                        return Err(store_err.context("transcribe_worker: mark_retryable_failure"));
                    }
                }
                // continue loop
            }
        }
    }
}

/// Phase 2 entry point: pipelined orchestrator.
///
/// Spawns N `fetch_worker` tasks + 1 `transcribe_worker` task into a
/// shared `tokio::task::JoinSet`, supervised by a shared
/// `tokio_util::sync::CancellationToken`. On first `Err`/panic from any
/// worker, fires `token.cancel()` and drains the remaining tasks. On
/// clean drain (every worker exits `Ok(())` after `claim_next == None`
/// or channel close), computes a `ProcessStats` from the DB and merges
/// the two stale-claim counters in.
///
/// **0025 shutdown ORDER — steps 1-3 happen inside this function:**
/// 1. `token.cancel()` is fired on the first `Err` from any worker.
/// 2. Original `tx` is dropped after the spawn loop, so the
///    fetch→transcribe channel closes when all fetch workers exit.
/// 3. `JoinSet::join_next()` is awaited to completion; every worker
///    has returned (Ok, Err, or panic) before this function returns.
///
/// **Step 4 (`engine.shutdown()`) is the caller's responsibility** —
/// must run after this future resolves AND after the caller has dropped
/// its own `Arc<dyn Transcriber>` clone. See `main.rs::Process` arm.
///
/// **Abort lever (T18 Consideration D).** The supervision loop uses
/// `token.cancel()` only — not `join_set.abort_all()`. Cancellation
/// latency is bounded by the largest single `await` in each worker
/// (fetch ~`ytdlp_timeout` worst case; transcribe ~1s on
/// large-v3-turbo-q5_0). `abort_all()` would drop futures immediately
/// (yt-dlp's `kill_on_drop` would fire), at the cost of losing graceful
/// cleanup. Tracked as a deferred choice; an operator can cap fetch
/// latency at the CLI via `--ytdlp-timeout`.
///
/// Sweeps stale claims at the top (0024), same as `run_serial`. The
/// inner workers also tolerate stale claims via the two
/// `stats_stale_after_*` counters; the top-of-run sweep is for
/// process-crash recovery only.
pub async fn run_pipelined(
    store: SharedStore,
    fetcher: Arc<dyn VideoFetcher>,
    transcriber: Arc<dyn Transcriber>,
    opts: ProcessOptions,
) -> Result<ProcessStats> {
    // 0024: recover rows left in_progress by a crashed earlier run.
    // Same gesture as `run_serial`; happens once per orchestrator
    // invocation before any worker starts claiming.
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
    let opts_arc = Arc::new(opts);
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    // Shared counter for --max-videos cap. Checked and incremented inside the
    // Mutex<Store> guard in fetch_worker, so the check+claim+increment is
    // race-free across all concurrent fetch workers.
    let claims_counter = Arc::new(AtomicUsize::new(0));
    let mut join_set: JoinSet<Result<()>> = JoinSet::new();

    // Spawn the transcribe worker FIRST so the channel has a consumer
    // by the time the first fetch worker tries to send.
    join_set.spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::clone(&transcriber),
        Arc::clone(&store),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::clone(&opts_arc),
    ));

    // Spawn N fetch workers. Each gets its own `tx.clone()`; the
    // original `tx` is dropped below so the channel closes when every
    // fetch_worker exits (drain semantics per 0026).
    for _ in 0..opts_arc.download_workers {
        join_set.spawn(fetch_worker(
            token.clone(),
            Arc::clone(&store),
            Arc::clone(&fetcher),
            tx.clone(),
            Arc::clone(&stats_stale_after_failure),
            Arc::clone(&claims_counter),
            Arc::clone(&opts_arc),
        ));
    }
    // 0025 step 2: drop the orchestrator's own tx so the channel closes
    // as soon as every fetch_worker drops its clone. Without this drop
    // the transcribe_worker would park on recv() forever even after all
    // fetch workers exit.
    drop(tx);

    // Supervise: on first Err or panic, cancel the token and drain.
    // Bug-class signals propagate via the `Err` join arm; non-Bug
    // outcomes from individual rows are absorbed inside the workers
    // (via `mark_retryable_failure` + the two counters).
    let mut first_error: Option<anyhow::Error> = None;
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok(Ok(())) => { /* worker exited clean */ }
            Ok(Err(e)) => {
                tracing::error!(error = %e, "worker returned Err; initiating shutdown");
                if first_error.is_none() {
                    first_error = Some(e);
                }
                // 0025 step 1: cancel the token. Workers observe at next
                // loop-top poll (fetch) or at the biased select arm
                // (transcribe). The 0008 invariant lives inside
                // `write_artifacts_and_mark`; even a cancellation
                // mid-write leaves artifacts durable.
                token.cancel();
            }
            Err(join_err) if join_err.is_panic() => {
                let msg = format!("worker panicked: {join_err}");
                tracing::error!(error = %msg, "worker panic; initiating shutdown");
                if first_error.is_none() {
                    first_error = Some(anyhow!(msg));
                }
                token.cancel();
            }
            Err(join_err) => {
                // Cancelled JoinError — only possible if a future caller
                // adds `join_set.abort_all()`. Not used today; left as a
                // defensive arm so a later abort-lever upgrade
                // (Consideration D) doesn't surprise the orchestrator.
                tracing::warn!(error = %join_err, "JoinError (cancelled)");
            }
        }
    }
    // 0025 step 3 complete: every worker has joined. Compute the final
    // stats by summing per-status DB row counts + merging the two
    // counters.

    let mut stats = {
        let guard = store.lock().await;
        compute_process_stats(&guard)?
    };
    stats.stale_after_failure = stats_stale_after_failure.load(Ordering::Relaxed);
    stats.stale_after_success = stats_stale_after_success.load(Ordering::Relaxed);

    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(stats)
}

/// Compute `ProcessStats` from the DB after `run_pipelined` drains.
/// Counts rows by status (succeeded / failed_retryable / failed_terminal)
/// under the assumption that the run started on a clean state or that
/// the operator interprets `claimed` as "rows in a terminal status by
/// the end of this run". Pending/in_progress rows are not counted —
/// `claimed = succeeded + failed_retryable + failed_terminal`.
///
/// Stale-claim counters (`stale_after_success`, `stale_after_failure`)
/// are written by the caller after this returns; both are workers' own
/// telemetry rather than DB-derivable.
///
/// For run-to-run accuracy in mid-stream invocations, a richer metric
/// (claim-count delta tracked in an `Arc<Mutex<ProcessStats>>` shared
/// across workers) would be preferable — left for Epic 5's ops-hygiene
/// work; Plan B Epic 2 ships this COUNT-by-status proxy because the
/// happy-path test and the 0027 bake validate only per-row status.
fn compute_process_stats(store: &Store) -> Result<ProcessStats> {
    let mut succeeded: usize = 0;
    let mut failed_retryable: usize = 0;
    let mut failed_terminal: usize = 0;

    let mut stmt = store
        .conn()
        .prepare("SELECT status, COUNT(*) FROM videos GROUP BY status")
        .context("preparing status-count query")?;
    // COUNT(*) is non-negative and far below usize::MAX.
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))
        })
        .context("executing status-count query")?;
    for row in rows {
        let (status, count) = row?;
        match status.as_str() {
            "succeeded" => succeeded = count,
            "failed_retryable" => failed_retryable = count,
            "failed_terminal" => failed_terminal = count,
            _ => { /* pending / in_progress not counted */ }
        }
    }

    let failed = failed_retryable + failed_terminal;
    Ok(ProcessStats {
        claimed: succeeded + failed,
        succeeded,
        failed,
        // Counter-derived fields are filled by the caller after this
        // function returns; leave them at default here.
        stale_after_success: 0,
        stale_after_failure: 0,
    })
}
