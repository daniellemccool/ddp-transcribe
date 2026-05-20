//! Pipelined orchestrator: N fetch workers feed 1 transcribe worker via a
//! bounded mpsc channel (0027).
//!
//! T15 landed the skeleton + shared types. T16 lands the fetch worker.
//! T17 will land the transcribe worker; T18 will wire `run_pipelined`'s
//! `JoinSet` + `CancellationToken` shutdown order per 0025 and flip the
//! `--pipelined` branch in `main.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{fetch_and_decode, ProcessOptions, ProcessStats};
use crate::fetcher::VideoFetcher;
use crate::state::{Claim, Store};
use crate::transcribe::WhisperEngine;

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
