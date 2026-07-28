//! Pipelined orchestrator: N fetch workers feed 1 transcribe worker via a
//! bounded mpsc channel (0027).
//!
//! T15 landed the skeleton + shared types. T16 lands the fetch worker.
//! T17 lands the transcribe worker. T18 will wire `run_pipelined`'s
//! `JoinSet` + `CancellationToken` shutdown order per 0025 and flip the
//! `--pipelined` branch in `main.rs`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::sync::mpsc;
use tokio::sync::Mutex as TokioMutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::{
    classify_fetch_phase, cookie_opts_for, fetch_and_decode, write_artifacts_and_mark,
    ProcessOptions, ProcessOutcome, ProcessStats,
};
use crate::errors::TranscribeError;
use crate::failure::{classify_transcribe_error, ClassifiedFailure};
use crate::fetcher::VideoFetcher;
use crate::state::{Claim, FailureRecordOutcome, Store};
use crate::transcribe::{PerCallConfig, Transcriber};

/// Shared aggregation of a per-video terminal write-off count keyed by
/// label, folded into `ProcessStats::terminal_by_label` after the workers
/// drain. Only `fetch_worker` writes to it (the sole producer of
/// `Unavailable`).
type TerminalByLabel = Arc<TokioMutex<BTreeMap<String, usize>>>;

/// Shared outcome-dispatch for `Store::record_fetch_failure`, used by BOTH
/// `fetch_worker` and `transcribe_worker`.
///
/// The two pipelined workers reach `record_fetch_failure` from the same
/// `Retryable` verdict and aggregate through the same `Arc<AtomicUsize>`
/// counter shape, so the outcome→counter mapping lives here once rather
/// than duplicated per-worker (analogous to [`handle_mutator_result`]) —
/// duplication would invite dispatch skew. `serial.rs` keeps its own copy
/// (its stats are direct struct fields, not atomics).
///
/// - `Requeued` → `requeued_for_retry += 1` (row back to 'pending').
/// - `Exhausted` → `exhausted_retries += 1` (cap reached; parked).
/// - `ParkedForCookies` → `parked_for_cookies += 1` (requires-cookie, no
///   cookies configured this run).
/// - `StaleClaim` → `stale_after_failure += 1` (+ warn): the claim was
///   swept + re-claimed elsewhere, nothing was recorded. Symmetric to the
///   success-side stale routing; NOT a Bug.
/// - `Err` → propagated with row context (Bug-class; orchestrator reacts
///   per 0025).
fn handle_record_fetch_failure_outcome(
    outcome: anyhow::Result<FailureRecordOutcome>,
    worker_id: &str,
    video_id: &str,
    stats_requeued_for_retry: &Arc<AtomicUsize>,
    stats_exhausted_retries: &Arc<AtomicUsize>,
    stats_parked_for_cookies: &Arc<AtomicUsize>,
    stats_stale_after_failure: &Arc<AtomicUsize>,
) -> anyhow::Result<()> {
    match outcome {
        Ok(FailureRecordOutcome::Requeued) => {
            stats_requeued_for_retry.fetch_add(1, Ordering::Relaxed);
        }
        Ok(FailureRecordOutcome::Exhausted) => {
            stats_exhausted_retries.fetch_add(1, Ordering::Relaxed);
        }
        Ok(FailureRecordOutcome::ParkedForCookies) => {
            stats_parked_for_cookies.fetch_add(1, Ordering::Relaxed);
        }
        Ok(FailureRecordOutcome::StaleClaim) => {
            stats_stale_after_failure.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                worker = %worker_id,
                video_id,
                "record_fetch_failure: stale claim (swept + re-claimed elsewhere)"
            );
        }
        Err(e) => {
            return Err(e.context(format!("record_fetch_failure for {video_id}")));
        }
    }
    Ok(())
}

/// Shared stale-claim routing for failure-side mutators, factored out so
/// `fetch_worker` and `transcribe_worker` don't each carry a third copy of
/// this match (T07: the classifier dispatch would otherwise duplicate it
/// per-arm on top of the two pre-existing copies).
///
/// `Ok(0)`: the mutator's `status='in_progress' AND claimed_by=?` predicate
/// missed — a concurrent sweep (or another worker) cleared the claim between
/// this worker's `claim_next` and the failure-flip. Symmetric to the
/// success-side `StaleAfterSuccess` outcome: count it and continue, do NOT
/// treat it as a Bug.
///
/// `Err`: the store call itself failed — Bug-class, propagated with context
/// identifying which mutator and which row.
///
/// Returns the mutator's row-change count (0006 shape) so call sites can
/// gate follow-up accounting on "the write actually landed" — e.g.
/// fetch_worker's `terminal_by_label` census increment must NOT run on the
/// `Ok(0)` stale path (nothing was written; counting it would inflate the
/// census — Epic 4a T06 review fix).
fn handle_mutator_result(
    result: anyhow::Result<usize>,
    worker_id: &str,
    video_id: &str,
    stale_counter: &Arc<AtomicUsize>,
    op: &'static str,
) -> anyhow::Result<usize> {
    match result {
        Ok(0) => {
            stale_counter.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                worker = %worker_id,
                video_id,
                "{op} swallowed: row no longer claimed by this worker \
                 (probably swept + re-claimed by another process)"
            );
            Ok(0)
        }
        Ok(changed) => Ok(changed),
        Err(e) => Err(e.context(format!("{op} for {video_id}"))),
    }
}

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
/// `pub` (not `pub(crate)`) so the integration tests under
/// `tests/pipeline_fakes/` can construct/inspect items. After T18
/// `run_pipelined` is wired in `main.rs`, every field is read on the
/// bin path (via destructuring inside `transcribe_worker`) — no
/// `dead_code` suppression needed.
///
/// `fetch_policy_tag` (ADR 0038) is the [`crate::fetcher::FetchPolicy::tag`]
/// of the format policy this item was actually fetched under, stamped by
/// `fetch_worker` from its `FetchOpts` at fetch time. `transcribe_worker`
/// passes it straight through to `record_fetch_failure` on a
/// transcribe-side retryable — recomputing from `claim` there would be
/// correct too (the policy is a pure function of `last_retryable_kind`,
/// per `format_policy_for`), but stamping it here avoids a second
/// classification-table-shaped lookup off the hot path.
#[derive(Debug)]
pub struct FetchedItem {
    pub claim: Claim,
    pub samples: Vec<f32>,
    pub samples_len: usize,
    pub wav_path: PathBuf,
    pub fetcher_name: &'static str,
    pub fetch_policy_tag: &'static str,
}

/// Phase 2 fetch worker. Claims pending rows; fetches + decodes WAVs;
/// pushes a [`FetchedItem`] onto the channel. Exits cleanly on
/// `claim_next == None` (drain semantics per 0026 — no polling).
///
/// **Error classification (Epic 3 T07; Epic 4a T06).** `fetch_and_decode`'s
/// [`super::FetchPhaseError`] is run through [`classify_fetch_phase`]:
/// - `Bug`: returns `Err` — the orchestrator reacts per 0025 (cancel all +
///   drain).
/// - `Unavailable`: write-off class (ADR 0033) — calls
///   `mark_terminal_failure` and continues; the row never retries.
/// - `Retryable`: calls `record_fetch_failure`, which decides
///   requeue-vs-exhaust-vs-park in one transaction (Epic 4a), and
///   continues; the outcome routes to the retry counters via
///   [`handle_record_fetch_failure_outcome`].
///
/// Every failure-dispatched attempt also bumps `stats_failed` once (before
/// the classifier match), mirroring `run_serial`'s per-attempt bump —
/// ADR-0007 input-side accounting.
///
/// **Mutex hold-time discipline.** The shared store guard is acquired
/// briefly for `claim_next` and the failure mutators only; it is
/// dropped before the multi-second `fetch_and_decode().await` so other
/// fetch workers can claim concurrently (SQLite's BEGIN IMMEDIATE
/// already serializes the actual claim transaction at the connection
/// level).
///
/// **Cancellation latency (T16, closed out here).** The `fetch_and_decode`
/// future is wrapped in a `tokio::select!` against the `CancellationToken`,
/// mirroring the transcribe-side wrap (a66d38b). When `token.cancel()`
/// fires mid-fetch, the select arm wins and the in-flight `acquire` future
/// drops; `kill_on_drop` reaps the yt-dlp child immediately instead of the
/// worker waiting out the fetch. The row stays `in_progress`; the next
/// run's `sweep_stale_claims` recovers it per 0024. Resolves the
/// FOLLOWUPS T16 cancellation-latency entry (previously: polled at the
/// loop top only, relying on `kill_on_drop` firing whenever the future was
/// eventually dropped — correct but with unbounded latency during a long
/// fetch).
///
/// **Stale-after-failure handling (design amendment).** When a failure
/// mutator returns `Ok(0)`, the row's claim was swept (or re-assigned)
/// between this worker's `claim_next` and the mutator call — the predicate
/// `status='in_progress' AND claimed_by=?` no longer matches. Symmetric
/// to T9's `StaleAfterSuccess` outcome on the success side: increment
/// `stats_stale_after_failure` and continue, do NOT return Err. T18
/// merges this counter into `ProcessStats` after worker drain.
///
/// T18: `run_pipelined` is the in-bin caller; the prior
/// `#[allow(dead_code)]` placeholder is lifted as part of this wiring
/// per 0002. Integration tests in `tests/pipeline_fakes/fetch_worker_tests.rs`
/// continue to exercise it directly.
#[allow(clippy::too_many_arguments)] // one worker; each Arc counter/handle is a distinct sink
pub async fn fetch_worker(
    token: CancellationToken,
    store: SharedStore,
    fetcher: Arc<dyn VideoFetcher>,
    sender: mpsc::Sender<FetchedItem>,
    stats_stale_after_failure: Arc<AtomicUsize>,
    stats_requeued_for_retry: Arc<AtomicUsize>,
    stats_exhausted_retries: Arc<AtomicUsize>,
    stats_parked_for_cookies: Arc<AtomicUsize>,
    stats_failed: Arc<AtomicUsize>,
    terminal_by_label: TerminalByLabel,
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

        // Epic 3 T08: kind-gated cookie routing (ADR 0035). Computed at the
        // call site so the policy decision (which reads `claim` fresh each
        // iteration) never goes stale across retries.
        let fetch_opts =
            cookie_opts_for(&claim, &opts.classification, opts.cookies_file.as_deref());

        // T16: wrap the fetch in the cancellation select so a mid-fetch
        // shutdown drops the in-flight `acquire` future promptly (see the
        // docstring's Cancellation latency section) instead of relying on
        // the loop-top poll alone.
        let fetch_result = tokio::select! {
            biased;
            () = token.cancelled() => {
                tracing::info!(worker = %worker_id, "fetch_worker: cancellation during fetch; exiting");
                return Ok(());
            }
            r = fetch_and_decode(fetcher.as_ref(), &claim, &fetch_opts) => r,
        };
        let (_metadata_capture, fetch_result) = fetch_result; // Epic 4c Task 03 wires persistence

        match fetch_result {
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
                    // ADR 0038: stamp the policy this item was actually
                    // fetched under, so a transcribe-side retryable can
                    // record the accurate tag instead of recomputing.
                    fetch_policy_tag: fetch_opts.format_policy.tag(),
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
                // ADR-0007 input-side accounting: one failure-dispatched
                // attempt, bumped before the classifier match — mirrors
                // run_serial's `stats.failed += 1` at the top of its Err
                // arm. (The Bug arm returns Err below, discarding stats,
                // so the extra bump there is inert — same as serial.)
                stats_failed.fetch_add(1, Ordering::Relaxed);
                match classify_fetch_phase(&e, &opts.classification) {
                    ClassifiedFailure::Bug { ctx } => {
                        return Err(anyhow!("fetch Bug for {video_id}: {}", ctx.message()));
                    }
                    ClassifiedFailure::Unavailable { label, ctx } => {
                        tracing::warn!(
                            worker = %worker_id,
                            video_id = video_id.as_str(),
                            label = label.as_str(),
                            "fetch_worker: write-off; marking terminal"
                        );
                        let result = {
                            let mut guard = store.lock().await;
                            guard.mark_terminal_failure(
                                &video_id,
                                &worker_id,
                                &label,
                                &ctx.message(),
                            )
                        };
                        let changed = handle_mutator_result(
                            result,
                            &worker_id,
                            &video_id,
                            &stats_stale_after_failure,
                            "mark_terminal_failure",
                        )?;
                        // Epic 4a: run-side terminal-by-label census. Only
                        // fetch_worker produces Unavailable; the map is
                        // folded into ProcessStats after the join loop.
                        // Gated on the write actually landing: on the
                        // Ok(0) stale path nothing was written (the row
                        // was swept + re-claimed elsewhere) and counting
                        // it would inflate the census (T06 review fix).
                        if changed > 0 {
                            let mut m = terminal_by_label.lock().await;
                            *m.entry(label.clone()).or_insert(0) += 1;
                        }
                    }
                    ClassifiedFailure::Retryable {
                        label,
                        requires_cookie,
                        ctx,
                    } => {
                        tracing::error!(
                            worker = %worker_id,
                            video_id = video_id.as_str(),
                            label = label.as_str(),
                            "fetch_worker: retryable failure"
                        );
                        let outcome = {
                            let mut guard = store.lock().await;
                            guard.record_fetch_failure(
                                &video_id,
                                &worker_id,
                                &label,
                                &ctx.message(),
                                fetch_opts.format_policy.tag(),
                                opts.retries + 1,
                                requires_cookie,
                                opts.cookies_file.is_some(),
                            )
                        };
                        handle_record_fetch_failure_outcome(
                            outcome,
                            &worker_id,
                            &video_id,
                            &stats_requeued_for_retry,
                            &stats_exhausted_retries,
                            &stats_parked_for_cookies,
                            &stats_stale_after_failure,
                        )?;
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
/// **Error classification (Epic 3 T07; Epic 4a T06).**
/// - [`TranscribeError::Cancelled`]: row stays `in_progress`, sweep
///   recovers on next startup. Worker returns `Ok(())` — coordinated
///   shutdown is not a Bug.
/// - [`TranscribeError::Bug`]: worker returns `Err` — the orchestrator
///   reacts per 0025 (cancel all + drain JoinSet).
/// - Other variants (currently `Timeout`, `Failed`, `EmptyOutput`,
///   `AudioDecode`): run through `classify_transcribe_error`, which never
///   produces `Unavailable` and only produces `Bug` for the
///   already-excluded `TranscribeError::Bug` case — both arms are
///   `unreachable!()` in this match. The live path is `Retryable`:
///   `record_fetch_failure` decides requeue-vs-exhaust-vs-park (Epic 4a);
///   the outcome routes to the retry counters via
///   [`handle_record_fetch_failure_outcome`], whose `StaleClaim` arm
///   (claim swept mid-flight) increments `stats_stale_after_failure` and
///   continues (symmetric to T16's amendment for the fetch side). Each
///   dispatched attempt also bumps `stats_failed` once (ADR-0007
///   input-side accounting, mirroring `run_serial`).
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
/// per 0002. Integration tests in
/// `tests/pipeline_fakes/transcribe_worker_tests.rs` continue to exercise
/// it directly.
#[allow(clippy::too_many_arguments)] // one worker; each Arc counter is a distinct sink
pub async fn transcribe_worker(
    token: CancellationToken,
    mut receiver: mpsc::Receiver<FetchedItem>,
    transcriber: Arc<dyn Transcriber>,
    store: SharedStore,
    stats_stale_after_failure: Arc<AtomicUsize>,
    stats_stale_after_success: Arc<AtomicUsize>,
    stats_requeued_for_retry: Arc<AtomicUsize>,
    stats_exhausted_retries: Arc<AtomicUsize>,
    stats_parked_for_cookies: Arc<AtomicUsize>,
    stats_succeeded: Arc<AtomicUsize>,
    stats_failed: Arc<AtomicUsize>,
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
            fetch_policy_tag,
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
                // fine. Otherwise mark_succeeded changed a row: bump the
                // input-side success counter (ADR-0007; T06 review fix —
                // ProcessStats is per-attempt, not DB-status-derived).
                if outcome == ProcessOutcome::StaleAfterSuccess {
                    stats_stale_after_success.fetch_add(1, Ordering::Relaxed);
                } else {
                    stats_succeeded.fetch_add(1, Ordering::Relaxed);
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
                // Classifier dispatch (Epic 3 T07). By this point
                // `TranscribeError::Cancelled` and `TranscribeError::Bug`
                // have already been consumed by the arms above, so
                // `classify_transcribe_error` — which only ever produces
                // `Bug` for the `Bug` variant, and never produces
                // `Unavailable` — cannot reach either arm here. Both are
                // `unreachable!()`; the live path is `Retryable`.
                let video_id = claim.video_id.clone();
                // ADR-0007 input-side accounting: one failure-dispatched
                // attempt (mirrors run_serial's per-attempt bump).
                stats_failed.fetch_add(1, Ordering::Relaxed);
                match classify_transcribe_error(&e) {
                    ClassifiedFailure::Bug { .. } => {
                        unreachable!("TranscribeError::Bug is caught by the arm above this match")
                    }
                    ClassifiedFailure::Unavailable { .. } => {
                        unreachable!("classify_transcribe_error never produces Unavailable")
                    }
                    // Epic 4a T06: transcribe-side retryables route through
                    // record_fetch_failure too — `requires_cookie` is always
                    // false here (no yt-dlp stderr), but the dispatch path is
                    // identical, so pass it through.
                    ClassifiedFailure::Retryable {
                        label,
                        requires_cookie,
                        ctx,
                    } => {
                        tracing::error!(
                            worker = %worker_id,
                            video_id = video_id.as_str(),
                            label = label.as_str(),
                            "transcribe_worker: retryable transcribe failure"
                        );
                        let outcome = {
                            let mut guard = store.lock().await;
                            guard.record_fetch_failure(
                                &video_id,
                                &worker_id,
                                &label,
                                &ctx.message(),
                                fetch_policy_tag,
                                opts.retries + 1,
                                requires_cookie,
                                opts.cookies_file.is_some(),
                            )
                        };
                        handle_record_fetch_failure_outcome(
                            outcome,
                            &worker_id,
                            &video_id,
                            &stats_requeued_for_retry,
                            &stats_exhausted_retries,
                            &stats_parked_for_cookies,
                            &stats_stale_after_failure,
                        )?;
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
/// or channel close), assembles `ProcessStats` from the shared input-side
/// counters (ADR-0007): every field counts THIS RUN's attempts —
/// `claimed` per successful `claim_next` (retry re-claims included),
/// `succeeded` per row-changing `mark_succeeded`, `failed` per
/// failure-dispatched attempt — never DB-status row counts (which would
/// under-report retried attempts and absorb prior runs' rows; Epic 4a
/// T06 review fix).
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
    // Epic 4a: capped-retry outcome counters, shared across both worker
    // kinds (both dispatch retryables through record_fetch_failure). The
    // terminal-by-label census map is fetch_worker-only.
    let stats_requeued_for_retry = Arc::new(AtomicUsize::new(0));
    let stats_exhausted_retries = Arc::new(AtomicUsize::new(0));
    let stats_parked_for_cookies = Arc::new(AtomicUsize::new(0));
    let terminal_by_label: TerminalByLabel = Arc::new(TokioMutex::new(BTreeMap::new()));
    // ADR-0007 input-side attempt counters (T06 review fix): `succeeded`
    // is bumped by transcribe_worker per row-changing mark_succeeded;
    // `failed` by both workers per failure-dispatched attempt.
    let stats_succeeded = Arc::new(AtomicUsize::new(0));
    let stats_failed = Arc::new(AtomicUsize::new(0));
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
        Arc::clone(&stats_requeued_for_retry),
        Arc::clone(&stats_exhausted_retries),
        Arc::clone(&stats_parked_for_cookies),
        Arc::clone(&stats_succeeded),
        Arc::clone(&stats_failed),
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
            Arc::clone(&stats_requeued_for_retry),
            Arc::clone(&stats_exhausted_retries),
            Arc::clone(&stats_parked_for_cookies),
            Arc::clone(&stats_failed),
            Arc::clone(&terminal_by_label),
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
    // (via `record_fetch_failure` / `mark_terminal_failure` + the shared
    // counters).
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
    // 0025 step 3 complete: every worker has joined; no counter writer
    // remains. Assemble the final stats from the shared input-side
    // counters (ADR-0007; T06 review fix). The previous status-derived
    // COUNT-by-status proxy (`compute_process_stats`) is retired: with
    // in-batch retries a fail-once-then-recover video consumes 2 claims
    // but would have reported claimed=1/failed=0, contradicting serial's
    // per-attempt semantics for the identical struct fields and Task 07's
    // census expectations.
    let stats = ProcessStats {
        claimed: claims_counter.load(Ordering::Relaxed),
        succeeded: stats_succeeded.load(Ordering::Relaxed),
        failed: stats_failed.load(Ordering::Relaxed),
        stale_after_success: stats_stale_after_success.load(Ordering::Relaxed),
        stale_after_failure: stats_stale_after_failure.load(Ordering::Relaxed),
        requeued_for_retry: stats_requeued_for_retry.load(Ordering::Relaxed),
        exhausted_retries: stats_exhausted_retries.load(Ordering::Relaxed),
        parked_for_cookies: stats_parked_for_cookies.load(Ordering::Relaxed),
        terminal_by_label: terminal_by_label.lock().await.clone(),
    };

    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(stats)
}
