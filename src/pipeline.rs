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
    /// Threshold for `sweep_stale_claims` at the top of `run_serial` per 0024
    /// (30-min default). Constructed from `Config::stale_claim_threshold` in
    /// main.rs and consumed below.
    pub stale_claim_threshold: Duration,
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

pub async fn run_serial(
    store: &mut Store,
    fetcher: &dyn VideoFetcher,
    transcriber: &dyn Transcriber,
    opts: ProcessOptions,
) -> Result<ProcessStats> {
    let mut stats = ProcessStats::default();
    let max = opts.max_videos.unwrap_or(usize::MAX);

    // 0024: recover any rows left in_progress by a crashed earlier run.
    let recovered = store
        .sweep_stale_claims(opts.stale_claim_threshold)
        .context("sweep_stale_claims at run_serial start")?;
    if recovered > 0 {
        tracing::info!(recovered, "sweep_stale_claims recovered abandoned rows");
    }

    // Loop guard: count claimed rows against `max`, not `claimed + failed`.
    // The old form was correct only under Plan A's fail-fast (failed was
    // always 0 inside the live loop). With continue-on-failure each failure
    // would double-count, exiting early. `claimed = succeeded +
    // stale_after_success + failed` post-loop.
    while stats.claimed < max {
        let claim = match store.claim_next(&opts.worker_id)? {
            Some(c) => c,
            None => break,
        };
        stats.claimed += 1;

        match process_one(store, fetcher, transcriber, &claim, &opts).await {
            Ok(ProcessOutcome::Succeeded) => stats.succeeded += 1,
            Ok(ProcessOutcome::StaleAfterSuccess) => {
                // Artifacts durable per 0008; row sits in pending and will
                // be re-claimed. Not counted as success or failure.
                stats.stale_after_success += 1;
            }
            Err(e) => {
                stats.failed += 1;
                let msg = format!("{e:#}"); // chain-aware via anyhow
                tracing::error!(
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "video failed; classifying as failed_retryable"
                );
                // Epic 2 MVP: single placeholder kind "FetchOrTranscribe"
                // per 0023. Epic 3 replaces this with typed classifier
                // dispatch (RetryableKind enum projection).
                store
                    .mark_retryable_failure(
                        &claim.video_id,
                        &opts.worker_id,
                        "FetchOrTranscribe",
                        &msg,
                    )
                    .with_context(|| format!("mark_retryable_failure for {}", claim.video_id))?;
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
) -> Result<ProcessOutcome> {
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
    let changed = store.mark_succeeded(
        &claim.video_id,
        &opts.worker_id,
        SuccessArtifacts {
            duration_s,
            language_detected: Some(transcribe_output.language.clone()),
            fetcher: fetcher.name(),
            transcript_source: transcriber.name(),
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

#[cfg(test)]
mod tests {
    //! Unit tests for `process_one` — placed in-module so the private
    //! function is reachable without a public re-export. The integration
    //! tests in `tests/pipeline_fakes.rs` exercise `run_serial`.
    use super::*;
    use crate::errors::TranscribeError;
    use crate::fetcher::{Acquisition, FakeFetcher, VideoFetcher};
    use crate::state::Store;
    use crate::transcribe::{PerCallConfig, TranscribeOutput, Transcriber};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct ScriptedTranscriber {
        output: TranscribeOutput,
    }

    #[async_trait]
    impl Transcriber for ScriptedTranscriber {
        async fn transcribe(
            &self,
            _samples: Vec<f32>,
            _config: PerCallConfig,
            _timeout: Duration,
        ) -> Result<TranscribeOutput, TranscribeError> {
            Ok(self.output.clone())
        }
        fn name(&self) -> &'static str {
            "scripted"
        }
    }

    fn silence_wav() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/audio/silence_16khz_mono.wav")
    }

    /// T5-review carry-forward: `process_one` MUST surface a 0-row
    /// `mark_succeeded` return as `ProcessOutcome::StaleAfterSuccess`,
    /// not as silent success. Synthesize the path by sweeping the row
    /// back to pending (with `Duration::ZERO`) between `claim_next`
    /// and `process_one`'s `mark_succeeded`.
    #[tokio::test]
    async fn process_one_returns_stale_after_success_on_mark_succeeded_zero() -> anyhow::Result<()>
    {
        let tmp = TempDir::new()?;
        let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
        store.upsert_video("vid_a", "https://example/a", false)?;

        // Stage a real WAV fixture for the FakeFetcher.
        let fake_wav = tmp.path().join("fake.wav");
        std::fs::copy(silence_wav(), &fake_wav)?;
        let map = HashMap::from([("vid_a".to_string(), fake_wav.clone())]);
        let fetcher = FakeFetcher {
            canned: Mutex::new(map),
            always_fails: false,
        };
        let transcriber = ScriptedTranscriber {
            output: TranscribeOutput {
                text: "hello".into(),
                language: "en".into(),
                lang_probs: None,
                segments: vec![],
                model_id: "test.bin".into(),
            },
        };

        // Claim the row, then sweep with Duration::ZERO so claimed_at < now
        // and the row flips back to pending. mark_succeeded inside
        // process_one will then return 0 (predicate fails: status != 'in_progress').
        let claim = store.claim_next("worker-1")?.expect("first claim");
        // Sleep 1s so `claimed_at < now` after the sweep cutoff (sweep uses
        // unix_now() - threshold.as_secs() in seconds resolution; zero
        // threshold means claimed_at < now, but timestamps share the same
        // second on a fast claim. Bump to ensure inequality.)
        std::thread::sleep(Duration::from_secs(1));
        let swept = store.sweep_stale_claims(Duration::ZERO)?;
        assert_eq!(swept, 1, "row must sweep back to pending");

        // Sanity check the fetcher returns the canned audio (defensive —
        // the `Acquisition` variant could change).
        let acq = fetcher.acquire("vid_a", "https://example/a").await?;
        assert!(matches!(acq, Acquisition::AudioFile(_)));

        let opts = ProcessOptions {
            worker_id: "worker-1".into(),
            transcripts_root: tmp.path().join("transcripts"),
            max_videos: Some(1),
            compute_lang_probs: false,
            transcribe_timeout: Duration::from_secs(5),
            stale_claim_threshold: Duration::from_secs(60),
        };

        // Use the same Claim returned by claim_next — process_one needs
        // worker_id parity with the original claim for the predicate to
        // match in the happy path; here it shouldn't because the sweep
        // cleared claimed_by.
        let outcome = process_one(&mut store, &fetcher, &transcriber, &claim, &opts).await?;
        assert_eq!(
            outcome,
            ProcessOutcome::StaleAfterSuccess,
            "mark_succeeded returned 0 → StaleAfterSuccess"
        );

        // Row sits in pending (artifacts durable per 0008; will be re-claimed).
        let row = store.get_video_for_test("vid_a")?.expect("row");
        assert_eq!(row.status, "pending");

        // Artifacts on disk (0008 invariant — written before mark_succeeded).
        let txt = tmp.path().join("transcripts/_a/vid_a.txt");
        assert!(
            txt.exists(),
            "transcript artifact must exist: {}",
            txt.display()
        );

        Ok(())
    }
}
