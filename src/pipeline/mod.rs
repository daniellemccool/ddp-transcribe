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
//! - [`fetch_and_decode`] — phases 1+2 (acquire + decode WAV)
//! - [`transcribe_and_write`] — phases 3+4 (transcribe + write artifacts +
//!   mark_succeeded + cleanup). 0008 invariant lives here: artifacts are
//!   durable on disk BEFORE `mark_succeeded`.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::audio;
use crate::fetcher::{Acquisition, VideoFetcher};
use crate::output::artifacts::{RawSignals, TranscriptMetadata};
use crate::output::{artifacts, shard};
use crate::state::{Claim, Store, SuccessArtifacts};
use crate::transcribe::{PerCallConfig, TranscribeOutput, Transcriber};

mod pipelined;
mod serial;

// T18: `run_pipelined` + `SharedStore` are now consumed by `main.rs`'s
// Process arm; the other three (`fetch_worker`, `transcribe_worker`,
// `FetchedItem`) are reached transitively via `run_pipelined` inside
// the bin and DIRECTLY from `tests/pipeline_fakes.rs`. The direct test
// reach is the reason these stay `pub` re-exports — bin compilation
// doesn't see the direct reach, hence the `#[allow(unused_imports)]`
// stays per 0002 (suppressed-at-re-export, not at definition).
#[allow(unused_imports)]
pub use pipelined::{fetch_worker, run_pipelined, transcribe_worker, FetchedItem, SharedStore};
// `run_serial` is no longer on the bin's hot path after T18 (the
// Process arm calls `run_pipelined`). It stays compiled for the
// integration tests in `tests/pipeline_fakes.rs` which exercise the
// serial helper's behavioral contract (retryable failure
// classification, stale-after-success). 0002 placeholder until a
// follow-up either retires `run_serial` or restores a behind-a-flag
// bin caller.
#[allow(unused_imports)]
pub use serial::run_serial;

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
    /// T15-T18 when the pipelined orchestrator is wired; suppressed until
    /// then per 0002.
    #[allow(dead_code)]
    pub download_workers: usize,
    /// 0027: default 2; flag-tunable via --channel-capacity. Consumed by
    /// T15-T18 when the pipelined orchestrator is wired; suppressed until
    /// then per 0002.
    #[allow(dead_code)]
    pub channel_capacity: usize,
}

#[derive(Debug, Default)]
pub struct ProcessStats {
    pub claimed: usize,
    pub succeeded: usize,
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
    /// T18: symmetric counter for the failure path. Rows where
    /// `mark_retryable_failure` returned `Ok(0)` — predicate
    /// `status='in_progress' AND claimed_by=?` missed because a concurrent
    /// sweep cleared the claim between `claim_next` and the failure-flip.
    /// Both `fetch_worker` and `transcribe_worker` increment this on the
    /// retryable-error path. The row stays where the sweep left it
    /// (`pending`) and will be re-claimed on the next iteration.
    ///
    /// In Phase 1 (serial loop) this counter doesn't exist on the path
    /// because `run_serial` doesn't run a mid-loop sweep. Phase 2's
    /// concurrent workers reach it via the swept-claim race.
    pub stale_after_failure: usize,
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

/// Phase 1+2: acquire the audio and decode it to f32 PCM samples.
///
/// Returns the owned samples + the WAV path on disk (needed downstream so
/// `transcribe_and_write` can remove the WAV after the DB commit).
///
/// Used by `run_serial`'s `process_one` AND (in Phase 2) by the fetch
/// workers in `pipelined::fetch_worker`.
pub(crate) async fn fetch_and_decode(
    fetcher: &dyn VideoFetcher,
    claim: &Claim,
) -> Result<(Vec<f32>, PathBuf)> {
    let acquisition = fetcher
        .acquire(&claim.video_id, &claim.source_url)
        .await
        .with_context(|| format!("fetching {}", claim.video_id))?;

    // Plan A's `Acquisition` has only one variant; Plan B will add `Unavailable`
    // and `ReadyTranscript`, at which point the `match` becomes load-bearing.
    // Keeping it now means Plan B's diff is additive arms, not a syntax flip.
    #[allow(clippy::infallible_destructuring_match)]
    let wav_path = match acquisition {
        Acquisition::AudioFile(p) => p,
    };
    tracing::info!(video_id = claim.video_id.as_str(), wav = %wav_path.display(), "audio acquired");

    // Decode WAV → owned Vec<f32> samples (0014: 16 kHz mono validated
    // inside decode_wav). Owned samples cross the worker-thread boundary
    // per 0016.
    let samples = audio::decode_wav(&wav_path)
        .with_context(|| format!("decoding wav {}", wav_path.display()))?;

    Ok((samples, wav_path))
}

/// Phase 3+4: transcribe → write artifacts → mark_succeeded → cleanup wav.
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
/// (`transcribe_worker`) delegates directly to the inner
/// [`write_artifacts_and_mark`] helper (which is the 0008 single source
/// of truth); after T18 the pipelined worker no longer routes through
/// this outer wrapper. Kept for `run_serial` (integration tests).
///
/// 0002: paired with `run_serial`'s suppression; bin doesn't reach
/// this after T18.
#[allow(dead_code)]
pub(crate) async fn transcribe_and_write(
    store: &mut Store,
    transcriber: &dyn Transcriber,
    claim: &Claim,
    samples: Vec<f32>,
    wav_path: PathBuf,
    fetcher_name: &'static str,
    opts: &ProcessOptions,
) -> Result<ProcessOutcome> {
    // T17 refactor: capture `samples_len` and `transcript_source` BEFORE
    // the transcribe move so the shared `write_artifacts_and_mark` helper
    // can be called without re-deriving them from a consumed Vec or a
    // borrowed &dyn Transcriber.
    let samples_len = samples.len();
    let transcript_source = transcriber.name();

    // Epic 1 stays auto-detect-only (PerCallConfig::default().language == None).
    // No CLI flag for language pin; if Epic 4 needs one, it adds it then.
    let per_call = PerCallConfig {
        compute_lang_probs: opts.compute_lang_probs,
        ..PerCallConfig::default()
    };

    let transcribe_output = transcriber
        .transcribe(samples, per_call, opts.transcribe_timeout)
        .await
        .with_context(|| format!("transcribing {}", claim.video_id))?;
    tracing::info!(
        video_id = claim.video_id.as_str(),
        chars = transcribe_output.text.len(),
        language = transcribe_output.language.as_str(),
        "transcribed"
    );

    // T17 refactor (Path A): the post-transcribe artifact write + DB
    // mark + wav cleanup lives in `write_artifacts_and_mark`, shared with
    // `pipelined::transcribe_worker`. Splitting the helper this way keeps
    // the 0008 ordering invariant in a single place — the pipelined worker
    // needs the same write+mark logic but can't reuse this whole function
    // because it must run the transcribe call OUTSIDE the store mutex.
    write_artifacts_and_mark(
        store,
        transcribe_output,
        claim,
        samples_len,
        wav_path,
        fetcher_name,
        transcript_source,
        opts,
    )
}

/// Phase 4 helper extracted from [`transcribe_and_write`] (T17, Path A):
/// write artifacts → mark_succeeded → cleanup wav.
///
/// **0008 invariant (load-bearing).** Artifacts (txt + json) must be
/// durable on disk BEFORE `store.mark_succeeded`. A crash between
/// artifact writes and `mark_succeeded` leaves the row in `in_progress`,
/// which the next run's `sweep_stale_claims` reclaims (per 0024); the
/// artifacts on disk are re-written on the next attempt (atomic_write
/// is idempotent).
///
/// This helper is the **single source of truth** for that ordering.
/// Both `transcribe_and_write` (serial path) and
/// `pipelined::transcribe_worker` (T17) delegate here; a regression in
/// the order would silently pass tests on the happy path but corrupt
/// invariants in a crash-mid-write scenario.
///
/// Returns [`ProcessOutcome::StaleAfterSuccess`] when `mark_succeeded`
/// updates 0 rows (concurrent sweep cleared the claim during
/// transcription). Artifacts are durable per 0008; the row sits in
/// `pending` and will be re-claimed.
///
/// `transcript_source` is passed in (instead of calling
/// `transcriber.name()` inside) because the pipelined worker holds a
/// `Arc<dyn Transcriber>` and consumes `transcribe_output` here — the
/// caller captures `transcriber.name()` before the transcribe move per
/// the same pattern that captures `samples_len`.
///
/// Sync (not async): every operation here is a blocking syscall
/// (`atomic_write`, `mark_succeeded` via rusqlite, `remove_file`). The
/// caller holds the store mutex around this call and serializes against
/// other workers — making this `async` would only add a `.await` that
/// never yields, since there's no I/O wait point.
///
/// clippy::too_many_arguments allow: 8 args; a builder/param struct
/// would add boilerplate disproportionate to the value (every arg is
/// part of the same logical "write+mark" operation; none are optional;
/// the call is internal with two callers).
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_artifacts_and_mark(
    store: &mut Store,
    transcribe_output: TranscribeOutput,
    claim: &Claim,
    samples_len: usize,
    wav_path: PathBuf,
    fetcher_name: &'static str,
    transcript_source: &'static str,
    opts: &ProcessOptions,
) -> Result<ProcessOutcome> {
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
        raw_signals: Some(RawSignals::from_transcribe_output(&transcribe_output)),
    };
    // T4 perf-tweaks: compact JSON shrinks the raw_signals payload
    // meaningfully (per-token id+text+p+plog dominates by token count;
    // pretty-print added ~3x whitespace bloat). 0008 ordering preserved;
    // 0010 schema shape unchanged (compact and pretty are equivalent
    // JSON values).
    let json_bytes = serde_json::to_vec(&metadata).context("serializing transcript metadata")?;
    let json_path = shard_dir.join(format!("{}.json", claim.video_id));
    artifacts::atomic_write(&json_path, &json_bytes)?;

    // 0008: artifacts durable, now mark the row succeeded.
    let changed = store.mark_succeeded(
        &claim.video_id,
        &opts.worker_id,
        SuccessArtifacts {
            duration_s,
            language_detected: Some(transcribe_output.language.clone()),
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
        // Skip wav cleanup: leave it for the next claim's retry path (the
        // re-claimed run will re-fetch and overwrite). Diverges from the
        // happy-path cleanup below, but symmetry isn't worth the risk of
        // deleting bytes the next claim might want.
        return Ok(ProcessOutcome::StaleAfterSuccess);
    }

    // Cleanup the wav file after the DB commit. If this fails, the success
    // is already durable; the leftover wav is just disk churn an operator
    // can sweep. (Plan A removed the wav before mark_succeeded, which left
    // a window where a crashed mark_succeeded had no audio to retry from.
    // Reversed here.)
    if let Err(e) = std::fs::remove_file(&wav_path) {
        tracing::warn!(path = %wav_path.display(), error = %e, "could not remove wav after success");
    }

    tracing::info!(video_id = claim.video_id.as_str(), "succeeded");
    Ok(ProcessOutcome::Succeeded)
}
