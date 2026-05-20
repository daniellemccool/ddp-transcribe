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
use tokio_util::sync::CancellationToken;

use super::{fetch_and_decode, write_artifacts_and_mark, ProcessOptions, ProcessStats};
use crate::errors::TranscribeError;
use crate::fetcher::VideoFetcher;
use crate::state::{Claim, Store};
use crate::transcribe::{PerCallConfig, Transcriber, WhisperEngine};

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
/// `pub` (not `pub(crate)`) so the integration tests in
/// `tests/pipeline_fakes.rs` can construct/inspect items. 0002
/// suppression on bin-compilation dead-code lives on the re-export in
/// `mod.rs`; the per-field reads are exercised by the tests.
#[derive(Debug)]
#[allow(dead_code)]
pub struct FetchedItem {
    pub claim: Claim,
    pub samples: Vec<f32>,
    pub samples_len: usize,
    pub wav_path: PathBuf,
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
/// 0002 suppression: T17 (transcribe_worker) and T18 (orchestrator
/// wiring) are the in-bin callers; until then bin compilation flags
/// this as dead. Tests in `tests/pipeline_fakes.rs` exercise it now.
#[allow(dead_code)]
pub async fn fetch_worker(
    token: CancellationToken,
    store: SharedStore,
    fetcher: Arc<dyn VideoFetcher>,
    sender: mpsc::Sender<FetchedItem>,
    stats_stale_after_failure: Arc<AtomicUsize>,
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
        let claim = {
            let mut guard = store.lock().await;
            guard.claim_next(&worker_id)?
        };
        let claim = match claim {
            Some(c) => c,
            None => {
                // 0026: drain semantics. Worker exits on None.
                tracing::info!(worker = %worker_id, "fetch_worker: queue drained; exiting");
                return Ok(());
            }
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
/// **Cancellation composition (0003 deviation from ADR 0025 strict
/// text).** ADR 0025's Consequences says the transcribe worker MUST
/// wrap `engine.transcribe()` in `tokio::select! { token.cancelled() | … }`
/// so cancellation drops the in-flight transcribe future, firing Epic 1's
/// 0012 `CancelOnDrop` and aborting whisper.cpp within milliseconds.
/// **This worker does NOT implement that wrap** — only the outer
/// `recv()` is in the select. Cancellation latency is bounded by the
/// in-flight transcribe (~1s on `large-v3-turbo-q5_0`); cancel takes
/// effect at the next loop-top iteration. Rationale: tighter
/// cancellation requires the worker to receive a cancellable handle
/// from `WhisperEngine` (the trait's `transcribe` returns a non-`select!`-
/// friendly future today); restructuring that surface is Epic 3+ scope.
/// Acknowledged trade-off: ~1s shutdown latency for the transcribe
/// worker vs. immediate-ish for fetch workers' parked
/// `claim_next`/`recv()` arms. Tracked for FOLLOWUPS at Phase 2 close
/// pending T18's measured shutdown wall-clock.
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
/// 0002 suppression: T18 wires this into `run_pipelined`; until then bin
/// compilation marks it dead. Tests in `tests/pipeline_fakes.rs`
/// exercise it now.
#[allow(dead_code)]
pub async fn transcribe_worker(
    token: CancellationToken,
    mut receiver: mpsc::Receiver<FetchedItem>,
    transcriber: Arc<dyn Transcriber>,
    store: SharedStore,
    stats_stale_after_failure: Arc<AtomicUsize>,
    opts: Arc<ProcessOptions>,
) -> Result<()> {
    let worker_id = opts.worker_id.clone();

    loop {
        // `biased` prefers the cancellation arm when both are ready —
        // prevents a constant stream of FetchedItems from starving
        // cancellation. Per 0025 this is the propagation entry point.
        let item = tokio::select! {
            biased;
            _ = token.cancelled() => {
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
        let transcribe_result = transcriber
            .transcribe(samples, per_call, opts.transcribe_timeout)
            .await;

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
                    "ytdlp",
                    transcriber.name(),
                    &opts,
                )
                .with_context(|| format!("write_artifacts_and_mark for {}", claim.video_id))?;
                drop(guard);

                // `StaleAfterSuccess`: the helper already logged the warn;
                // T18 will route this into a counter on `ProcessStats`.
                // For now just continue — artifacts are durable per 0008
                // and the row sits in pending.
                let _ = outcome;
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
/// **SKELETON ONLY in T15** — returns `Ok(ProcessStats::default())`
/// without doing real work. T16 fills in `fetch_worker`; T17 fills in
/// `transcribe_worker`; T18 wires `JoinSet` + `CancellationToken` +
/// shutdown ordering per 0025 and changes `main.rs`'s `--pipelined`
/// branch to call this.
///
/// Signature note: the `&WhisperEngine` parameter matches the T15 brief.
/// T18 may amend to `Arc<dyn Transcriber>` (the `WhisperEngineHandle`
/// pattern) if the worker structure needs an owned shared handle for
/// `tokio::spawn`.
#[allow(clippy::needless_pass_by_value, dead_code)]
pub async fn run_pipelined(
    _store: SharedStore,
    _fetcher: &dyn VideoFetcher,
    _engine: &WhisperEngine,
    _opts: ProcessOptions,
) -> Result<ProcessStats> {
    // T16/T17/T18 fill this in. Returning empty stats keeps the type
    // signature stable so callers compile.
    Ok(ProcessStats::default())
}
