use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::audio;
use crate::fetcher::{Acquisition, VideoFetcher};
use crate::output::artifacts::{RawSignals, TranscriptMetadata};
use crate::output::{artifacts, shard};
use crate::state::{Claim, Store, SuccessArtifacts};
use crate::transcribe::{PerCallConfig, Transcriber};

pub struct ProcessOptions {
    pub worker_id: String,
    pub transcripts_root: PathBuf,
    pub max_videos: Option<usize>,
    /// Threaded from `Config::compute_lang_probs`. Consumed in `process_one`
    /// when constructing `PerCallConfig`.
    pub compute_lang_probs: bool,
    /// Threaded from `Config::transcribe_timeout`. Per-call deadline handed
    /// to `Transcriber::transcribe`; 0012's abort_callback polls it.
    pub transcribe_timeout: Duration,
    /// Threshold for `sweep_stale_claims` at the top of `run_serial`.
    /// Default 30 min per 0024; constructed from `Config::stale_claim_threshold`
    /// in main.rs. T9 (next task) reads this in `run_serial`; suppress
    /// dead_code until that task lands.
    #[allow(dead_code)]
    pub stale_claim_threshold: Duration,
}

#[derive(Debug, Default)]
pub struct ProcessStats {
    pub claimed: usize,
    pub succeeded: usize,
    pub failed: usize,
}

// `stats.failed += 1` is followed immediately by `return Err(e)` in Plan A's
// fail-fast behavior, so the increment is dead under -D warnings. Plan B will
// drop the early return (persist failure and continue), at which point the
// increment becomes load-bearing. Keeping the bookkeeping in place now means
// Plan B's diff is a one-line removal rather than a re-introduction.
#[allow(unused_assignments)]
pub async fn run_serial(
    store: &mut Store,
    fetcher: &dyn VideoFetcher,
    transcriber: &dyn Transcriber,
    opts: ProcessOptions,
) -> Result<ProcessStats> {
    let mut stats = ProcessStats::default();
    let max = opts.max_videos.unwrap_or(usize::MAX);

    while stats.claimed + stats.failed < max {
        let claim = match store.claim_next(&opts.worker_id)? {
            Some(c) => c,
            None => break,
        };
        stats.claimed += 1;

        match process_one(store, fetcher, transcriber, &claim, &opts).await {
            Ok(()) => stats.succeeded += 1,
            Err(e) => {
                stats.failed += 1;
                tracing::error!(
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "video failed (Plan A: aborting; Plan B will classify and persist)"
                );
                // Plan A behavior: leave the row in_progress; operator inspects.
                // Plan B will persist failure and continue.
                return Err(e);
            }
        }
    }

    Ok(stats)
}

async fn process_one(
    store: &mut Store,
    fetcher: &dyn VideoFetcher,
    transcriber: &dyn Transcriber,
    claim: &Claim,
    opts: &ProcessOptions,
) -> Result<()> {
    tracing::info!(
        video_id = claim.video_id.as_str(),
        attempt = claim.attempt_count,
        "claimed"
    );

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
    // per 0016. Compute duration_s from sample count once (16 kHz is the
    // 0014 invariant); avoids a second pass via ffprobe.
    let samples = audio::decode_wav(&wav_path)
        .with_context(|| format!("decoding wav {}", wav_path.display()))?;
    let duration_s = Some(samples.len() as f64 / 16_000.0);

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
        fetcher: fetcher.name().to_string(),
        transcript_source: transcriber.name().to_string(),
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
    store.mark_succeeded(
        &claim.video_id,
        &opts.worker_id,
        SuccessArtifacts {
            duration_s,
            language_detected: Some(transcribe_output.language.clone()),
            fetcher: fetcher.name(),
            transcript_source: transcriber.name(),
        },
    )?;

    // Cleanup the wav file after the DB commit. If this fails, the success
    // is already durable; the leftover wav is just disk churn an operator
    // can sweep. (Plan A removed the wav before mark_succeeded, which left
    // a window where a crashed mark_succeeded had no audio to retry from.
    // Reversed here.)
    if let Err(e) = std::fs::remove_file(&wav_path) {
        tracing::warn!(path = %wav_path.display(), error = %e, "could not remove wav after success");
    }

    tracing::info!(video_id = claim.video_id.as_str(), "succeeded");
    Ok(())
}
