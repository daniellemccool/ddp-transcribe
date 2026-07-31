//! Pipeline orchestration: shared types + helpers used by both the serial
//! loop ([`run_serial`]) and the Phase 2 pipelined orchestrator
//! ([`run_pipelined`]).
//!
//! Module layout (T15): split into `mod.rs` (this file) + `serial.rs` +
//! `pipelined.rs`. The single-file `pipeline.rs` crossed the 250-line
//! production-code threshold from the T15 brief once Phase 1 review
//! carry-forward (StaleAfterSuccess) landed, and `run_pipelined` will grow
//! substantially across T16/T17/T18 (worker tasks + JoinSet +
//! CancellationToken). Splitting now keeps each downstream task's diff
//! scoped to one file.
//!
//! Shared items live here so both submodules can call them without crossing
//! a `pub(crate)` boundary twice:
//! - [`ProcessOptions`], [`ProcessStats`], [`ProcessOutcome`], [`SharedStore`]
//! - [`fetch_and_decode`] — phases 1+2 (acquire + decode WAV), and its two
//!   halves [`acquire_audio`] / [`decode_fetched`], which `fetch_worker`
//!   calls separately so its cancellation `select!` covers only the half
//!   that a drop can actually cancel
//! - [`transcribe_and_write`] — phases 3+4 (transcribe + write artifacts +
//!   mark_succeeded + cleanup). 0008 invariant lives here: artifacts are
//!   durable on disk BEFORE `mark_succeeded`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::audio;
use crate::fetcher::{Acquisition, FetchOpts, VideoFetcher};
use crate::output::artifacts::{RawSignals, TranscriptMetadata};
use crate::output::{artifacts, shard};
use crate::state::{Claim, Store, SuccessArtifacts};
use crate::transcribe::{PerCallConfig, TranscribeOutput, Transcriber};

mod pipelined;
mod serial;

// T18: `run_pipelined` + `SharedStore` are now consumed by `main.rs`'s
// Process arm; the other three (`fetch_worker`, `transcribe_worker`,
// `FetchedItem`) are reached transitively via `run_pipelined` inside
// the bin and DIRECTLY from the `tests/pipeline_fakes/` test files. The
// direct test reach is the reason these stay `pub` re-exports — bin
// compilation doesn't see the direct reach, hence the
// `#[allow(unused_imports)]` stays per 0002 (suppressed-at-re-export, not
// at definition).
#[allow(unused_imports)]
pub use pipelined::{fetch_worker, run_pipelined, transcribe_worker, FetchedItem, SharedStore};
// `run_serial` is no longer on the bin's hot path after T18 (the
// Process arm calls `run_pipelined`). It stays compiled for the
// integration tests in `tests/pipeline_fakes/serial_tests.rs` which
// exercise the serial helper's behavioral contract (retryable failure
// classification, stale-after-success). 0002 placeholder until a
// follow-up either retires `run_serial` or restores a behind-a-flag
// bin caller.
#[allow(unused_imports)]
pub use serial::run_serial;

/// The fetched audio plus the attempt directory that holds it — the pipeline's
/// handle on everything one acquire put on disk (Epic 5b).
///
/// **Lifecycle.** The caller owns this from the moment `acquire` returns `Ok`
/// until exactly one of the two consumers below runs:
/// - [`FetchedAudio::cleanup_after_success`] — after `mark_succeeded` commits
///   (0008 ordering: never before the DB acknowledges the artifacts).
/// - [`FetchedAudio::discard`] — decode failure, transcribe failure, or any
///   other path where this attempt's bytes are dead. The row is either
///   requeued (the retry re-fetches into a FRESH dir) or terminal, so nothing
///   downstream can want them.
///
/// The deliberate non-consumer is `ProcessOutcome::StaleAfterSuccess`: a
/// concurrent sweep cleared the claim, the row will be re-claimed, and
/// deleting bytes the next claim might want stays forbidden.
///
/// `attempt_dir: None` (test fakes serving a wav the test staged) degrades to
/// the pre-5b behavior: success removes the wav alone, failures remove
/// nothing — a file this pipeline did not create is not its to delete.
#[derive(Debug)]
pub struct FetchedAudio {
    pub wav_path: PathBuf,
    pub attempt_dir: Option<PathBuf>,
}

impl FetchedAudio {
    /// Remove this attempt's bytes after the DB commit. Best-effort by
    /// design: the success is already durable, and a leftover dir is disk
    /// churn the startup sweep collects — never a reason to fail the video.
    fn cleanup_after_success(&self) {
        match &self.attempt_dir {
            Some(dir) => crate::fetcher::ytdlp::remove_attempt_dir(dir),
            None => {
                if let Err(e) = std::fs::remove_file(&self.wav_path) {
                    tracing::warn!(path = %self.wav_path.display(), error = %e, "could not remove wav after success");
                }
            }
        }
    }

    /// Drop this attempt's bytes on a failure path. Same best-effort
    /// contract; a `None` attempt dir leaves the wav alone (see the struct
    /// docs).
    fn discard(&self) {
        if let Some(dir) = &self.attempt_dir {
            crate::fetcher::ytdlp::remove_attempt_dir(dir);
        }
    }
}

/// Operator checkpoint hook (Epic 5a): run `cmd` every `every` for as long
/// as the batch is running, so a long uncapped run syncs artifacts mid-run
/// instead of only at run boundaries.
///
/// Deliberately minimal: a program path and a period. The hook takes no
/// arguments (the operator's script owns its own configuration) and the
/// pipeline never inspects its output beyond exit status — see
/// `pipelined::run_pipelined`'s checkpoint task for the
/// failures-never-abort contract.
#[derive(Debug, Clone)]
pub struct CheckpointConfig {
    pub cmd: PathBuf,
    /// Doubles as the hook's timeout: a hook that outlives its own period is
    /// an operator configuration error, surfaced as a timeout warn rather
    /// than allowed to pile up overlapping invocations.
    pub every: Duration,
}

pub struct ProcessOptions {
    pub worker_id: String,
    pub transcripts_root: PathBuf,
    /// Cap on total claimed rows. Honored by both `run_serial` (outer loop
    /// guard `stats.claimed < max`) and `run_pipelined` (shared
    /// `Arc<AtomicUsize>` counter checked inside the `Mutex<Store>` guard
    /// before each `claim_next`, so the check + claim + increment is
    /// race-free across N concurrent fetch workers; zero overshoot).
    pub max_videos: Option<usize>,
    /// Threaded from `Config::compute_lang_probs`. Consumed in `process_one`
    /// when constructing `PerCallConfig`.
    pub compute_lang_probs: bool,
    /// Threaded from `Config::transcribe_timeout`. Per-call deadline handed
    /// to `Transcriber::transcribe`; 0012's abort_callback polls it.
    pub transcribe_timeout: Duration,
    /// Threshold for `sweep_stale_claims` at the top of `run_serial` per 0024
    /// (30-min default). Constructed from `Config::stale_claim_threshold` in
    /// main.rs and consumed below.
    pub stale_claim_threshold: Duration,
    /// 0027: default 3; flag-tunable via --download-workers. Consumed by
    /// `run_pipelined` when sizing the fetch-worker pool.
    pub download_workers: usize,
    /// 0027: default 2; flag-tunable via --channel-capacity. Consumed by
    /// `run_pipelined` when sizing the fetch→transcribe channel.
    pub channel_capacity: usize,
    /// Netscape-format cookie file, flag-tunable via `--cookies-file`
    /// (Epic 3 T08). Threaded to `fetcher.acquire` ONLY on claims whose
    /// `last_retryable_kind` is `SensitiveLoginGated` — see
    /// [`cookie_opts_for`]. ADR 0035: first attempts never get cookies.
    pub cookies_file: Option<PathBuf>,
    /// Active classification policy (Epic 4a): compiled default or the
    /// operator's `--classification` file, validated at startup. Shared
    /// read-only with every worker.
    pub classification: std::sync::Arc<crate::classification::ClassificationTable>,
    /// Epic 4a: automatic retry budget. A video gets at most `retries`
    /// automatic requeues (lifetime cap = retries + 1 total attempts,
    /// compared against attempt_count which claim_next bumps at claim
    /// time). Default 1 — pilot evidence: one retry recovers the dominant
    /// recoverable class (NoDataBlocks re-fetch 10/10 OK).
    pub retries: i64,
    /// Epic 5a: operator checkpoint hook, `None` when `--checkpoint-cmd` was
    /// not supplied (feature off — the pre-Epic-5a behavior of syncing only
    /// at run boundaries). Consumed by `run_pipelined`, which spawns one
    /// timer task for it; `run_serial` ignores it.
    pub checkpoint: Option<CheckpointConfig>,
}

#[derive(Debug, Default)]
pub struct ProcessStats {
    /// Input-side, per-attempt (ADR-0007): every successful `claim_next`
    /// this run, INCLUDING retry re-claims. Both orchestrators count this
    /// way — a fail-once-then-recover video is `claimed: 2`.
    pub claimed: usize,
    /// Attempts whose `mark_succeeded` changed a row this run.
    pub succeeded: usize,
    /// Failure-dispatched attempts this run (one per classified failure,
    /// regardless of which arm handled it) — per-attempt, so a video that
    /// failed then recovered contributes to BOTH `failed` and `succeeded`.
    pub failed: usize,
    /// T5-review carry-forward: rows where `process_one` wrote artifacts and
    /// then `mark_succeeded` returned `Ok(0)` — meaning a concurrent sweep
    /// (or different worker) cleared the claim between `claim_next` and
    /// `mark_succeeded`. The row sits in `pending` and will be re-claimed
    /// on the next iteration; artifacts are durable per 0008. Distinct from
    /// `failed` because no failure occurred — the work succeeded but DB
    /// acknowledgment didn't land against this worker's claim.
    ///
    /// In Phase 1 (single-process serial loop with sweep at the top) this
    /// counter should stay at 0 in practice. It's surfaced for Phase 2's
    /// concurrent workers where stale-after-success is reachable.
    pub stale_after_success: usize,
    /// T18: symmetric counter for the failure path. Rows where the failure
    /// mutator missed the `status='in_progress' AND claimed_by=?` predicate
    /// (`record_fetch_failure` → `StaleClaim` outcome on the retryable
    /// path; `mark_terminal_failure` → `Ok(0)` on the write-off path)
    /// because a concurrent sweep cleared the claim between `claim_next`
    /// and the failure-flip. Both `fetch_worker` and `transcribe_worker`
    /// increment this. The row stays where the sweep left it (`pending`)
    /// and will be re-claimed on the next iteration.
    ///
    /// In Phase 1 (serial loop) this counter doesn't exist on the path
    /// because `run_serial` doesn't run a mid-loop sweep. Phase 2's
    /// concurrent workers reach it via the swept-claim race.
    pub stale_after_failure: usize,
    /// Epic 4a: rows a worker sent back to 'pending' for an in-batch retry.
    pub requeued_for_retry: usize,
    /// Epic 4a: rows whose failure exhausted the attempt cap this run.
    pub exhausted_retries: usize,
    /// Epic 4a: requires-cookie rows parked because no cookies-file was
    /// configured for this run.
    pub parked_for_cookies: usize,
    /// Epic 4a: inline write-offs this run, keyed by label — the census's
    /// run-side terminal-by-label breakdown (attrition documentation).
    pub terminal_by_label: std::collections::BTreeMap<String, usize>,
    /// Epic 5a: operator checkpoint hook invocations that exited 0 this run.
    /// Input-side like every other field here (ADR-0007): one count per
    /// firing of the timer, not per artifact synced — the pipeline has no
    /// visibility into what the operator's script moved.
    pub checkpoints_run: u64,
    /// Epic 5a: checkpoint hook firings that did NOT exit 0 — nonzero exit,
    /// timeout, or spawn failure. A nonzero value here is an operator alarm
    /// (mid-run syncing is not happening), never a run failure: the hook
    /// path deliberately has no error return (see `run_pipelined`).
    pub checkpoints_failed: u64,
}

/// Outcome of a single `process_one` call. `StaleAfterSuccess` is the
/// T5-review carry-forward path: artifacts were written, but
/// `mark_succeeded` returned 0 (predicate mismatch on
/// `status='in_progress' AND claimed_by=?`), indicating a concurrent
/// sweep cleared the claim. Per 0008 the artifacts are durable, so the
/// row is safe to re-claim.
#[derive(Debug, PartialEq, Eq)]
pub enum ProcessOutcome {
    Succeeded,
    StaleAfterSuccess,
}

/// Typed error for phases 1+2 (fetch + decode), so the worker/serial callers
/// can classify the failure without downcasting through anyhow (Epic 3 T07
/// spec refinement #1). `#[from]` on both variants means `?` inside
/// `fetch_and_decode` converts without an explicit `.map_err`; the anyhow
/// boundary (where a caller needs `anyhow::Result`) is crossed via the
/// blanket `From<E: std::error::Error + Send + Sync + 'static> for
/// anyhow::Error` impl, which does NOT attach context. `serial.rs` recovers
/// the typed error from that chain via `classify_fetch_phase_in_chain`,
/// which walks it (E12) rather than downcasting the top-level error.
#[derive(Debug, thiserror::Error)]
pub enum FetchPhaseError {
    #[error(transparent)]
    Fetch(#[from] crate::errors::FetchError),
    #[error("decoding fetched wav: {0}")]
    Decode(#[from] crate::audio::AudioDecodeError),
}

/// Classify a [`FetchPhaseError`] into the three-arm verdict. Thin
/// projection: `Fetch` delegates to [`crate::failure::classify_fetch_error`]
/// (the classification-table-driven classifier); `Decode` is always
/// `Retryable` — a corrupt/truncated WAV on disk doesn't indict the source
/// video, and a refetch may produce a decodable file.
pub fn classify_fetch_phase(
    e: &FetchPhaseError,
    table: &crate::classification::ClassificationTable,
) -> crate::failure::ClassifiedFailure {
    use crate::failure::{labels, ClassifiedFailure, FailureContext};
    match e {
        FetchPhaseError::Fetch(fe) => crate::failure::classify_fetch_error(fe, table),
        FetchPhaseError::Decode(de) => ClassifiedFailure::Retryable {
            label: labels::TRANSCRIBE_OTHER.to_string(),
            requires_cookie: false,
            ctx: FailureContext {
                tool: "hound",
                exit_code: None,
                signal: None,
                stderr_excerpt: de.to_string(),
                classification_reason: "wav decode failure: refetch may repair a corrupt download",
            },
        },
    }
}

/// Format-policy routing (staged experiment, ADR 0038 — evidence and
/// rationale live on [`crate::fetcher::FetchPolicy`] and
/// `ytdlp::build_yt_dlp_args`'s doc comment): a retry whose prior failure
/// classified `NoDataBlocks` gets [`crate::fetcher::FetchPolicy::Frugal`]
/// — that class is `download`'s advertised-but-unservable failure
/// mechanism (selection succeeds, the transfer dies with "Did not get any
/// data blocks"), and a selection-time fallback chain cannot recover
/// mid-transfer, so the retry must not re-pick `download`. Every other
/// kind and fresh claims (`last_retryable_kind == None`) stay
/// [`crate::fetcher::FetchPolicy::DeterministicAudio`], the pilot-proven
/// default.
///
/// `"NoDataBlocks"` is not one of `failure::labels`'s four structural
/// constants (`ToolTimeout`/`NetworkTransient`/`YtDlpOther`/
/// `TranscribeOther`, all code-mapped) — it's a
/// classification-table-defined label (see
/// `classification::DEFAULT_TABLE_TOML`), so it's compared here as a
/// literal, the same way [`cookie_opts_for`] would if the active table had
/// no dedicated disposition to key its own gate on.
fn format_policy_for(claim: &Claim) -> crate::fetcher::FetchPolicy {
    use crate::fetcher::FetchPolicy;
    match claim.last_retryable_kind.as_deref() {
        // "NoDataBlocks" is a PINNED (reserved) label: the override
        // contract depends on this exact string, and — unlike the cookie
        // gate, which resolves through `table.disposition_of()` — it does
        // NOT consult the active classification table. A custom
        // `--classification` table that renames the label silently
        // disables the frugal retry, so custom tables must keep it
        // verbatim (ADR 0038 Consequences records this dependency).
        Some("NoDataBlocks") => FetchPolicy::Frugal,
        _ => FetchPolicy::DeterministicAudio,
    }
}

/// Kind-gated per-claim fetch-options routing. Cookie half (Epic 3 T08, ADR
/// 0035; Epic 4a T03: the gate now consults the active
/// [`crate::classification::ClassificationTable`] instead of a hardcoded
/// tag): cookies ride on a fetch iff the claim's most recent retryable
/// failure's label resolves to disposition `requires-cookie` in the active
/// table AND the operator supplied `--cookies-file`. First attempts
/// (`last_retryable_kind == None`) never get cookies, and labels the active
/// table doesn't recognize (e.g. the historical placeholder `"Fetch"`)
/// resolve to `None` and never qualify. Format-policy half: delegates to
/// [`format_policy_for`]. The two gates key on different kinds
/// (`SensitiveLoginGated` vs. `NoDataBlocks`) so they never both apply to
/// the same claim — a `SensitiveLoginGated` retry keeps its cookie
/// behavior with the `DeterministicAudio` default format, and a
/// `NoDataBlocks` retry gets `Frugal` with `cookies_file: None`.
pub(crate) fn cookie_opts_for(
    claim: &Claim,
    table: &crate::classification::ClassificationTable,
    cookies_file: Option<&Path>,
) -> FetchOpts {
    use crate::classification::Disposition;
    let needs_cookie = claim
        .last_retryable_kind
        .as_deref()
        .and_then(|k| table.disposition_of(k))
        == Some(Disposition::RequiresCookie);
    FetchOpts {
        cookies_file: if needs_cookie {
            cookies_file.map(Path::to_path_buf)
        } else {
            None
        },
        format_policy: format_policy_for(claim),
    }
}

/// Phase 1: acquire the audio, no decode.
///
/// Split out of [`fetch_and_decode`] (Epic 5b) because it is the ONLY half a
/// caller may safely wrap in a cancellation `select!`: dropping this future
/// mid-`acquire` drops a `tokio::process::Child`, which kills the yt-dlp
/// subprocess. See [`decode_fetched`] for why its half must not be dropped.
///
/// Returns a typed [`FetchPhaseError`] (rather than `anyhow::Result`, T15's
/// original signature) so callers can classify the failure via
/// [`classify_fetch_phase`] without downcasting through an anyhow chain
/// (Epic 3 T07).
///
/// `opts` (Epic 3 T08) carries the per-claim cookie decision computed by
/// [`cookie_opts_for`] at the call site — this function does not decide
/// policy, only threads the decision to `fetcher.acquire`.
///
/// Epic 4c: the first tuple element is the raw metadata envelope the fetcher
/// captured, if any. It is returned on EVERY path so the caller can persist
/// it before interpreting the outcome.
pub(crate) async fn acquire_audio(
    fetcher: &dyn VideoFetcher,
    claim: &Claim,
    opts: &FetchOpts,
) -> (
    Option<crate::fetcher::MetadataCapture>,
    Result<FetchedAudio, FetchPhaseError>,
) {
    let (capture, acquisition) = fetcher
        .acquire(&claim.video_id, &claim.source_url, opts)
        .await;
    let acquisition = match acquisition {
        // A failed acquire hands over no attempt dir: the fetcher removed
        // whatever it created before returning (see `VideoFetcher::acquire`'s
        // ownership contract), so there is nothing to clean up here.
        Ok(a) => a,
        Err(e) => return (capture, Err(e.into())),
    };

    // Plan A's `Acquisition` has only one variant; Plan B will add `Unavailable`
    // and `ReadyTranscript`, at which point the `match` becomes load-bearing.
    // Keeping it now means Plan B's diff is additive arms, not a syntax flip.
    #[allow(clippy::infallible_destructuring_match)]
    let fetched = match acquisition {
        Acquisition::AudioFile { wav, attempt_dir } => FetchedAudio {
            wav_path: wav,
            attempt_dir,
        },
    };
    // Tracing hygiene (Epic 3 T08, ADR 0035): log ONLY whether cookies were
    // attached, never the path — the path must not reach logs or the state DB.
    tracing::info!(
        video_id = claim.video_id.as_str(),
        wav = %fetched.wav_path.display(),
        cookies = opts.cookies_file.is_some(),
        "audio acquired"
    );

    (capture, Ok(fetched))
}

/// Phase 2: decode the acquired wav to f32 PCM samples.
///
/// **This future must NEVER be dropped — do not place it inside a
/// cancellation `select!`.** Dropping a `spawn_blocking` `JoinHandle` does
/// not cancel the closure: tokio runs it to completion detached, so a drop
/// here would leave a decode reading the attempt dir with nobody to observe
/// its outcome and would silently swallow a panic that used to propagate
/// through the `JoinSet`. `fetch_worker` therefore selects over
/// [`acquire_audio`] alone and awaits this half unconditionally, which is
/// also exactly the pre-wrap behavior (an inline decode was never an await
/// point, so cancellation could never land inside it).
async fn decode_fetched(
    fetched: FetchedAudio,
) -> Result<(Vec<f32>, FetchedAudio), FetchPhaseError> {
    // Decode WAV → owned Vec<f32> samples (0014: 16 kHz mono validated
    // inside decode_wav). Owned samples cross the worker-thread boundary
    // per 0016.
    //
    // Blocking-IO policy, class (c): a whole-file read plus the f32
    // conversion is unbounded work and three fetch workers run it
    // concurrently, so it goes to the blocking pool rather than holding a
    // runtime thread. The handle is awaited immediately and the closure's
    // `Result` is returned unchanged, so the error type, the message and the
    // discard-then-return ordering below are exactly what the inline call
    // produced.
    let wav_path = fetched.wav_path.clone();
    let decoded = tokio::task::spawn_blocking(move || audio::decode_wav(&wav_path)).await;
    let samples = match decoded {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            // Epic 5b: a wav we cannot decode is dead weight — the row
            // requeues and its retry re-fetches into a fresh attempt dir.
            fetched.discard();
            return Err(e.into());
        }
        Err(join) => match join.try_into_panic() {
            // A panic inside the closure resumes unwinding here, so it
            // propagates exactly as an inline `decode_wav` panic did.
            Ok(payload) => std::panic::resume_unwind(payload),
            // This future is never dropped (see the docstring) and a blocking
            // task is never cancelled by the scheduler, so the non-panic
            // JoinError has no producer.
            Err(join) => unreachable!("spawn_blocking decode task cancelled: {join}"),
        },
    };

    Ok((samples, fetched))
}

/// Phase 1+2 composed: acquire the audio and decode it to f32 PCM samples.
///
/// Returns the owned samples + the [`FetchedAudio`] handle (the wav plus the
/// attempt dir that holds it) — downstream owns that handle and removes the
/// attempt dir after the DB commit, or discards it on a later failure.
///
/// Used by `run_serial`'s `process_one`, which has no cancellation token and
/// wants the whole phase as one call. `pipelined::fetch_worker` calls the two
/// halves directly so its cancellation `select!` covers [`acquire_audio`]
/// only — see [`decode_fetched`]'s docstring for why that split is
/// load-bearing rather than cosmetic.
///
/// Epic 4c: the metadata envelope is returned on EVERY path — including
/// decode failures — so the caller can persist it before interpreting the
/// outcome.
pub(crate) async fn fetch_and_decode(
    fetcher: &dyn VideoFetcher,
    claim: &Claim,
    opts: &FetchOpts,
) -> (
    Option<crate::fetcher::MetadataCapture>,
    Result<(Vec<f32>, FetchedAudio), FetchPhaseError>,
) {
    match acquire_audio(fetcher, claim, opts).await {
        (capture, Ok(fetched)) => (capture, decode_fetched(fetched).await),
        (capture, Err(e)) => (capture, Err(e)),
    }
}

/// Phase 3+4: transcribe → write artifacts → mark_succeeded → attempt-dir cleanup.
///
/// **0008 invariant** lives here: txt + json are durable on disk BEFORE
/// `store.mark_succeeded` is called. A crash between artifact writes and
/// `mark_succeeded` leaves the row in `in_progress`, which the next run's
/// `sweep_stale_claims` reclaims (per 0024); the artifacts on disk are
/// re-written on the next attempt (atomic_write is idempotent).
///
/// Returns [`ProcessOutcome::StaleAfterSuccess`] when `mark_succeeded`
/// updates 0 rows — i.e., a concurrent sweep (or other worker) cleared
/// the claim during transcription. Artifacts are durable per 0008; the
/// row sits in `pending` and will be re-claimed. Deviates from the T15
/// brief's `Result<()>` signature (per 0003) because that brief snippet
/// predates the T5-review carry-forward.
///
/// `fetcher_name` is passed as an argument rather than added to
/// `ProcessOptions::fetcher_name` (per 0003 brief deviation) — keeps the
/// caller's existing `fetcher.name()` source-of-truth, avoids touching
/// `Config::from_args` and three test fixture constructions.
///
/// Used by `run_serial`'s `process_one`. The pipelined path
/// (`transcribe_worker`) calls the two phase-4 halves
/// ([`write_artifacts_durable`] then [`mark_after_artifacts`] — the 0008
/// ordering owner) directly, so it can keep the store lock off the
/// durable writes; after T18 the pipelined worker no longer routes
/// through this outer wrapper. Kept for `run_serial` (integration tests),
/// which reaches it through `process_one`.
pub(crate) async fn transcribe_and_write(
    store: &mut Store,
    transcriber: &dyn Transcriber,
    claim: &Claim,
    samples: Vec<f32>,
    audio: FetchedAudio,
    fetcher_name: &'static str,
    opts: &ProcessOptions,
) -> Result<ProcessOutcome> {
    // T17 refactor: capture `samples_len` and `transcript_source` BEFORE
    // the transcribe move so `write_artifacts_and_mark` — the serial path's
    // composition of the two phase-4 halves — can be called without
    // re-deriving them from a consumed Vec or a borrowed &dyn Transcriber.
    let samples_len = samples.len();
    let transcript_source = transcriber.name();

    // Epic 1 stays auto-detect-only (PerCallConfig::default().language == None).
    // No CLI flag for language pin; if Epic 4 needs one, it adds it then.
    let per_call = PerCallConfig {
        compute_lang_probs: opts.compute_lang_probs,
        ..PerCallConfig::default()
    };

    // Epic 5b: a failed transcribe ends this attempt — drop its bytes before
    // propagating (the row requeues and re-fetches into a fresh attempt dir).
    let transcribe_output = match transcriber
        .transcribe(samples, per_call, opts.transcribe_timeout)
        .await
    {
        Ok(o) => o,
        Err(e) => {
            audio.discard();
            return Err(anyhow::Error::new(e).context(format!("transcribing {}", claim.video_id)));
        }
    };
    tracing::info!(
        video_id = claim.video_id.as_str(),
        chars = transcribe_output.text.len(),
        language = transcribe_output.language.as_str(),
        "transcribed"
    );

    // T17 refactor (Path A): the post-transcribe artifact write + DB
    // mark + attempt-dir cleanup lives in `write_artifacts_and_mark`, which composes
    // [`write_artifacts_durable`] and [`mark_after_artifacts`] — the same
    // 0008-ordering halves `pipelined::transcribe_worker` calls directly
    // (see the doc comment above). Since the Task 05 split this composed
    // wrapper serves only this serial path; the pipelined worker calls the
    // two halves itself so it can run the transcribe call OUTSIDE the store
    // mutex and keep the lock off the durable writes.
    write_artifacts_and_mark(
        store,
        transcribe_output,
        claim,
        samples_len,
        audio,
        fetcher_name,
        transcript_source,
        opts,
    )
}

/// Phase 4a — durable artifact writes (0008 first half), extracted from
/// [`write_artifacts_and_mark`] (Epic 4c).
///
/// **NO store access.** Callers run this OUTSIDE any store lock: the
/// fsyncs in `atomic_write` (two file fsyncs + a directory fsync) are the
/// slow part of phase 4 and must not serialize other workers' claim /
/// failure dispatch behind the store mutex.
///
/// Writes txt then json. If a crash happens between the two, recovery
/// sees a complete txt but missing json metadata — preferable to the
/// reverse. Returns the computed `duration_s`, which
/// [`mark_after_artifacts`] needs for the DB row.
pub(crate) fn write_artifacts_durable(
    transcribe_output: &TranscribeOutput,
    claim: &Claim,
    samples_len: usize,
    opts: &ProcessOptions,
    fetcher_name: &'static str,
    transcript_source: &'static str,
) -> Result<Option<f64>> {
    // duration_s derives from the 0014 audio invariant (16 kHz mono):
    // samples_len / 16_000. Caller captured samples_len before the
    // transcribe call moved the Vec.
    // Precision loss is acceptable: this is a reported duration metric, not a value
    // we round-trip or compare for equality.
    #[allow(clippy::cast_precision_loss)]
    let duration_s = Some(samples_len as f64 / 16_000.0);

    let shard_dir = opts.transcripts_root.join(shard(&claim.video_id));
    std::fs::create_dir_all(&shard_dir)
        .with_context(|| format!("creating shard dir {}", shard_dir.display()))?;

    // 0008: artifact write (txt + json) before mark_succeeded. Two
    // atomic_write calls: text first, JSON second. If a crash happens
    // between the two, recovery sees a complete txt but missing json
    // metadata — preferable to the reverse (operator-facing transcript
    // missing while the DB claims success).
    let txt_path = shard_dir.join(format!("{}.txt", claim.video_id));
    artifacts::atomic_write(&txt_path, transcribe_output.text.as_bytes())
        .with_context(|| format!("writing transcript {}", txt_path.display()))?;

    let metadata = TranscriptMetadata {
        video_id: claim.video_id.clone(),
        source_url: claim.source_url.clone(),
        duration_s,
        language_detected: Some(transcribe_output.language.clone()),
        transcribed_at: Utc::now().to_rfc3339(),
        fetcher: fetcher_name.to_string(),
        transcript_source: transcript_source.to_string(),
        model: transcribe_output.model_id.clone(),
        raw_signals: Some(RawSignals::from_transcribe_output(transcribe_output)),
    };
    // T4 perf-tweaks: compact JSON shrinks the raw_signals payload
    // meaningfully (per-token id+text+p+plog dominates by token count;
    // pretty-print added ~3x whitespace bloat). 0008 ordering preserved;
    // 0010 schema shape unchanged (compact and pretty are equivalent
    // JSON values).
    let json_bytes = serde_json::to_vec(&metadata).context("serializing transcript metadata")?;
    let json_path = shard_dir.join(format!("{}.json", claim.video_id));
    artifacts::atomic_write(&json_path, &json_bytes)?;

    Ok(duration_s)
}

/// Phase 4b — DB acknowledgement (0008 second half) + attempt-dir cleanup,
/// extracted from [`write_artifacts_and_mark`] (Epic 4c).
///
/// The ONLY part of phase 4 that needs the store: pipelined callers lock
/// exactly around this call. **Must never be called before
/// [`write_artifacts_durable`] has returned Ok for the same claim** — that
/// ordering IS the 0008 invariant.
///
/// Returns [`ProcessOutcome::StaleAfterSuccess`] when `mark_succeeded`
/// updates 0 rows (concurrent sweep cleared the claim during
/// transcription). Artifacts are durable per 0008; the row sits in
/// `pending` and will be re-claimed.
///
/// `transcript_source` is passed in (instead of calling
/// `transcriber.name()` inside) because the pipelined worker holds an
/// `Arc<dyn Transcriber>` — the caller captures `transcriber.name()`
/// before the transcribe move per the same pattern that captures
/// `samples_len`.
///
/// Sync (not async): every operation here is a blocking syscall
/// (`mark_succeeded` via rusqlite, the attempt-dir removal). The caller holds the
/// store mutex around this call and serializes against other workers —
/// making this `async` would only add a `.await` that never yields, since
/// there's no I/O wait point.
///
/// clippy::too_many_arguments allow: 8 args; a builder/param struct
/// would add boilerplate disproportionate to the value (every arg is
/// part of the same logical "mark" operation; none are optional; the
/// call is internal with two callers).
#[allow(clippy::too_many_arguments)]
pub(crate) fn mark_after_artifacts(
    store: &mut Store,
    claim: &Claim,
    duration_s: Option<f64>,
    language: &str,
    audio: FetchedAudio,
    fetcher_name: &'static str,
    transcript_source: &'static str,
    opts: &ProcessOptions,
) -> Result<ProcessOutcome> {
    // 0008: artifacts durable, now mark the row succeeded.
    let changed = store.mark_succeeded(
        &claim.video_id,
        &opts.worker_id,
        SuccessArtifacts {
            duration_s,
            language_detected: Some(language.to_string()),
            fetcher: fetcher_name,
            transcript_source,
        },
    )?;

    // T5-review carry-forward: a 0-row UPDATE means the claim predicate
    // (status='in_progress' AND claimed_by=?) rejected — a concurrent sweep
    // (or other worker) cleared the claim while we were transcribing.
    // Artifacts are durable per 0008; the row sits in pending and will be
    // re-claimed. Surface this as a distinct outcome rather than treating
    // it as success — locks the invariant down before Phase 2's concurrent
    // workers can regress it.
    if changed == 0 {
        tracing::warn!(
            video_id = claim.video_id.as_str(),
            worker_id = opts.worker_id.as_str(),
            "stale claim after success — row will be re-claimed; artifacts are durable per 0008"
        );
        // Skip the attempt-dir cleanup: leave it for the next claim's retry
        // path (the re-claimed run will re-fetch and overwrite). Diverges
        // from the happy-path cleanup below, but symmetry isn't worth the
        // risk of deleting bytes the next claim might want.
        return Ok(ProcessOutcome::StaleAfterSuccess);
    }

    // Cleanup after the DB commit — the WHOLE attempt dir since Epic 5b, not
    // just the wav (the leftover dir was the "disk churn" this comment used
    // to concede). Same point in the order: if it fails, the success is
    // already durable and the startup sweep collects the residue. (Plan A
    // removed the wav BEFORE mark_succeeded, which left a window where a
    // crashed mark_succeeded had no audio to retry from. Reversed here.)
    audio.cleanup_after_success();

    tracing::info!(video_id = claim.video_id.as_str(), "succeeded");
    Ok(ProcessOutcome::Succeeded)
}

/// Phase 4 helper extracted from [`transcribe_and_write`] (T17, Path A):
/// write artifacts → mark_succeeded → attempt-dir cleanup.
///
/// **0008 invariant (load-bearing), revised owner (Epic 4c).** The
/// invariant is now owned by the PAIR
/// [`write_artifacts_durable`] → [`mark_after_artifacts`]: both artifacts
/// (txt then json) must be durable on disk BEFORE
/// `store.mark_succeeded`. A crash
/// between artifact writes and `mark_succeeded` leaves the row in
/// `in_progress`, which the next run's `sweep_stale_claims` reclaims (per
/// 0024); the artifacts on disk are re-written on the next attempt
/// (atomic_write is idempotent). A regression in the order would silently
/// pass tests on the happy path but corrupt invariants in a
/// crash-mid-write scenario.
///
/// This function is the composition of that pair, kept for the serial
/// path (`transcribe_and_write`). `pipelined::transcribe_worker` calls
/// the two halves directly so its store lock covers only the DB
/// acknowledgement — the durable writes (and their fsyncs) run unlocked.
///
/// clippy::too_many_arguments allow: 8 args; a builder/param struct
/// would add boilerplate disproportionate to the value (every arg is
/// part of the same logical "write+mark" operation; none are optional;
/// the call is internal).
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_artifacts_and_mark(
    store: &mut Store,
    transcribe_output: TranscribeOutput,
    claim: &Claim,
    samples_len: usize,
    audio: FetchedAudio,
    fetcher_name: &'static str,
    transcript_source: &'static str,
    opts: &ProcessOptions,
) -> Result<ProcessOutcome> {
    let duration_s = match write_artifacts_durable(
        &transcribe_output,
        claim,
        samples_len,
        opts,
        fetcher_name,
        transcript_source,
    ) {
        Ok(d) => d,
        Err(e) => {
            // Artifact write failed (disk full / permissions): the row stays
            // `in_progress` for the next sweep to reclaim and re-fetch, so
            // this attempt's bytes are dead — and on a full disk, freeing
            // them is the friendlier failure.
            audio.discard();
            return Err(e);
        }
    };
    mark_after_artifacts(
        store,
        claim,
        duration_s,
        &transcribe_output.language,
        audio,
        fetcher_name,
        transcript_source,
        opts,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Epic 3 T08 / ADR 0035: cookies ride ONLY when the claim's
    /// `last_retryable_kind` is exactly `SensitiveLoginGated` AND a cookie
    /// file was supplied. First attempts (`None`) and every other
    /// taxonomy kind never get cookies, regardless of the flag.
    #[test]
    fn cookies_only_for_sensitive_login_gated_retries() {
        let table = crate::classification::ClassificationTable::compiled_default().unwrap();
        let cookie = PathBuf::from("/secret/c.txt");
        let mk = |kind: Option<&str>| Claim {
            video_id: "7".into(),
            source_url: "u".into(),
            attempt_count: 1,
            last_retryable_kind: kind.map(String::from),
        };
        assert_eq!(
            cookie_opts_for(&mk(None), &table, Some(&cookie)).cookies_file,
            None
        );
        assert_eq!(
            cookie_opts_for(&mk(Some("NoDataBlocks")), &table, Some(&cookie)).cookies_file,
            None
        );
        assert_eq!(
            cookie_opts_for(&mk(Some("SensitiveLoginGated")), &table, Some(&cookie)).cookies_file,
            Some(cookie.clone())
        );
        assert_eq!(
            cookie_opts_for(&mk(Some("SensitiveLoginGated")), &table, None).cookies_file,
            None
        );
        // Historical placeholder kind → unknown label → table's
        // disposition_of returns None → no cookies, regardless of the
        // cookie file being supplied.
        assert_eq!(
            cookie_opts_for(&mk(Some("Fetch")), &table, Some(&cookie)).cookies_file,
            None
        );
    }

    /// Staged experiment (ADR 0038): a `NoDataBlocks` retry gets `Frugal`
    /// and no cookies — the retry must not re-pick `download`, whose
    /// mid-transfer failure is unrecoverable by a selection-time chain. A
    /// `SensitiveLoginGated` retry keeps its cookie with the
    /// `DeterministicAudio` default; `None` and every other kind
    /// (explicitly including `FfprobePostprocess`, whose override was
    /// retired by the operator reversal) stay `DeterministicAudio`.
    #[test]
    fn format_policy_frugal_only_for_no_data_blocks_retries() {
        use crate::fetcher::FetchPolicy;

        let table = crate::classification::ClassificationTable::compiled_default().unwrap();
        let cookie = PathBuf::from("/secret/c.txt");
        let mk = |kind: Option<&str>| Claim {
            video_id: "7".into(),
            source_url: "u".into(),
            attempt_count: 1,
            last_retryable_kind: kind.map(String::from),
        };

        let opts = cookie_opts_for(&mk(Some("NoDataBlocks")), &table, Some(&cookie));
        assert_eq!(opts.format_policy, FetchPolicy::Frugal);
        assert_eq!(
            opts.cookies_file, None,
            "NoDataBlocks never carries cookies"
        );

        let opts = cookie_opts_for(&mk(Some("SensitiveLoginGated")), &table, Some(&cookie));
        assert_eq!(
            opts.format_policy,
            FetchPolicy::DeterministicAudio,
            "the cookie gate's own retry kind must not affect format policy"
        );
        assert_eq!(opts.cookies_file, Some(cookie.clone()));

        let opts = cookie_opts_for(&mk(None), &table, Some(&cookie));
        assert_eq!(
            opts.format_policy,
            FetchPolicy::DeterministicAudio,
            "fresh claims fetch under the pilot-proven default"
        );

        let opts = cookie_opts_for(&mk(Some("FfprobePostprocess")), &table, Some(&cookie));
        assert_eq!(
            opts.format_policy,
            FetchPolicy::DeterministicAudio,
            "the retired FfprobePostprocess override must not linger — \
             the default already IS the deterministic path"
        );
    }
}
