//! Serial orchestrator: claim → fetch+decode → transcribe+write loop.
//!
//! Single-threaded pull from `Store::claim_next` per iteration; each claim
//! flows through `process_one`, which is now a thin caller of the shared
//! [`super::fetch_and_decode`] + [`super::transcribe_and_write`] helpers
//! (T15). Stays the production default until T18 wires `--pipelined`.

use anyhow::{Context, Result};

use super::{fetch_and_decode, transcribe_and_write, ProcessOptions, ProcessOutcome, ProcessStats};
use crate::fetcher::VideoFetcher;
use crate::state::{Claim, Store};
use crate::transcribe::Transcriber;

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

/// Drive a single claim through phases 1-4. Thin caller over the shared
/// helpers (T15): `fetch_and_decode` runs phases 1+2; `transcribe_and_write`
/// runs phases 3+4 and owns the 0008 artifact-before-mark_succeeded
/// invariant (plus the StaleAfterSuccess branch).
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

    let (samples, wav_path) = fetch_and_decode(fetcher, claim).await?;
    transcribe_and_write(
        store,
        transcriber,
        claim,
        samples,
        wav_path,
        fetcher.name(),
        opts,
    )
    .await
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
    use std::time::Duration;
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
            download_workers: 3,
            channel_capacity: 2,
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
