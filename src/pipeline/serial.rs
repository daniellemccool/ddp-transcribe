//! Serial orchestrator: claim → fetch+decode → transcribe+write loop.
//!
//! Single-threaded pull from `Store::claim_next` per iteration; each claim
//! flows through `process_one`, which is now a thin caller of the shared
//! [`super::fetch_and_decode`] + [`super::transcribe_and_write`] helpers
//! (T15). Stays the production default until T18 wires `--pipelined`.

use anyhow::{anyhow, Context, Result};

use super::{
    classify_fetch_phase, cookie_opts_for, fetch_and_decode, format_policy_for,
    transcribe_and_write, FetchPhaseError, ProcessOptions, ProcessOutcome, ProcessStats,
};
use crate::errors::TranscribeError;
use crate::failure::{classify_transcribe_error, labels, ClassifiedFailure};
use crate::fetcher::VideoFetcher;
use crate::state::{Claim, FailureRecordOutcome, Store};
use crate::transcribe::Transcriber;

/// **Non-production reference/test path.** Retained as the reference
/// implementation of the claim → fetch+decode → transcribe+write loop,
/// exercised by the integration tests in
/// `tests/pipeline_fakes/serial_tests.rs`; production runs use the pipelined
/// orchestrator ([`super::run_pipelined`]), which T18 wired into the
/// `Process` dispatch arm. Serial's behavioral contract — retryable
/// classification + `StaleAfterSuccess` — is part of the helper-shared
/// invariants documented in `mod.rs`, and the tests reach this function
/// through `pipeline`'s re-export.
///
/// Retirement is deferred (E12 ruling, operator, 2026-07-30) until this
/// function's unique tests are mapped onto the pipelined / shared-helper
/// suites — deleting it before then would drop coverage, not just code.
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
        let Some(claim) = store.claim_next(&opts.worker_id)? else {
            break;
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
                tracing::error!(
                    video_id = claim.video_id.as_str(),
                    error = %e,
                    "video failed; classifying"
                );
                // Epic 3 T07: classify by recovering the typed
                // FetchPhaseError from the anyhow chain (E12: a chain WALK,
                // symmetric with the transcribe-side `find_map` below — see
                // `classify_fetch_phase_in_chain`). A `None` here means the
                // chain didn't originate from `fetch_and_decode` — i.e. a
                // transcribe-side failure, handled by the nested
                // TranscribeError chain-walk in the None arm below (T07
                // review fix: Bug-class transcribe errors must escalate, not
                // silently downgrade to retryable).
                let verdict = classify_fetch_phase_in_chain(&e, &opts.classification);
                match verdict {
                    Some(ClassifiedFailure::Unavailable { label, ctx }) => {
                        let changed = store
                            .mark_terminal_failure(
                                &claim.video_id,
                                &opts.worker_id,
                                &label,
                                &ctx.message(),
                            )
                            .with_context(|| {
                                format!("mark_terminal_failure for {}", claim.video_id)
                            })?;
                        // Epic 4a: run-side terminal-by-label census. Gated
                        // on the write landing — a 0-row stale-claim miss
                        // must not inflate the census (unreachable in this
                        // single-threaded loop, but the semantics must match
                        // the pipelined workers'; T06 review fix).
                        if changed > 0 {
                            *stats.terminal_by_label.entry(label.clone()).or_insert(0) += 1;
                        }
                    }
                    Some(ClassifiedFailure::Bug { ctx }) => {
                        return Err(anyhow!(
                            "fetch Bug for {}: {}",
                            claim.video_id,
                            ctx.message()
                        ));
                    }
                    Some(ClassifiedFailure::Retryable {
                        label,
                        requires_cookie,
                        ctx,
                    }) => {
                        record_fetch_failure_serial(
                            store,
                            &mut stats,
                            &claim,
                            &opts,
                            &label,
                            &ctx.message(),
                            requires_cookie,
                        )?;
                    }
                    None => {
                        // Not a fetch-phase error — transcribe-side anyhow.
                        // T07 review fix: `transcribe_and_write` wraps the
                        // engine error via `.with_context("transcribing …")`,
                        // so the TranscribeError sits BELOW a context layer —
                        // walk the chain rather than downcasting the top
                        // error. Dispatch mirrors `transcribe_worker`: Bug
                        // escalates as Err (per 0025, not silently marked
                        // retryable), Unavailable is never produced, and
                        // Retryable marks with the classified label.
                        let transcribe_verdict = e
                            .chain()
                            .find_map(|cause| cause.downcast_ref::<TranscribeError>())
                            .map(classify_transcribe_error);
                        match transcribe_verdict {
                            Some(ClassifiedFailure::Bug { ctx }) => {
                                return Err(anyhow!(
                                    "transcribe Bug for {}: {}",
                                    claim.video_id,
                                    ctx.message()
                                ));
                            }
                            Some(ClassifiedFailure::Unavailable { .. }) => {
                                unreachable!("classify_transcribe_error never produces Unavailable")
                            }
                            Some(ClassifiedFailure::Retryable {
                                label,
                                requires_cookie,
                                ctx,
                            }) => {
                                record_fetch_failure_serial(
                                    store,
                                    &mut stats,
                                    &claim,
                                    &opts,
                                    &label,
                                    &ctx.message(),
                                    requires_cookie,
                                )?;
                            }
                            None => {
                                // Genuinely unclassifiable chain (neither a
                                // FetchPhaseError nor a TranscribeError root)
                                // — default-cautious.
                                let msg = format!("{e:#}");
                                record_fetch_failure_serial(
                                    store,
                                    &mut stats,
                                    &claim,
                                    &opts,
                                    labels::TRANSCRIBE_OTHER,
                                    &msg,
                                    false,
                                )?;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(stats)
}

/// Recover a typed [`FetchPhaseError`] from an anyhow chain and classify it;
/// `None` when the chain carries none (i.e. a transcribe-side failure, which
/// `run_serial` routes through its own `TranscribeError` chain-walk).
///
/// E12 ruling (operator, 2026-07-30). This used to be a top-level
/// `e.downcast_ref::<FetchPhaseError>()`, guarded by a tripwire comment
/// warning that `.context(...)` on the fetch path would break it — an
/// asymmetry with the transcribe side's `e.chain().find_map(...)`. Two
/// facts, both established while closing the ruling:
///
/// - The `.context()` half of the worry was unfounded: `anyhow::Error`'s own
///   `downcast_ref` already searches through its `Context` wrappers, to any
///   depth. Zero, one and two context layers all resolved before this change.
/// - The asymmetry was real anyway. `downcast_ref` understands anyhow's
///   wrappers and nothing else, so a `FetchPhaseError` reachable only as some
///   other error's `#[source]` was invisible to it. `chain()` walks
///   `std::error::Error::source()` and finds both shapes.
///
/// A miss here is silent and consequential: the verdict falls through to the
/// `TranscribeOther` catch-all, turning a terminal write-off into a retryable
/// requeue. Chain-walking closes that whole class rather than one instance.
fn classify_fetch_phase_in_chain(
    e: &anyhow::Error,
    table: &crate::classification::ClassificationTable,
) -> Option<ClassifiedFailure> {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<FetchPhaseError>())
        .map(|fe| classify_fetch_phase(fe, table))
}

/// Serial-path `record_fetch_failure` dispatch + local-stats increment.
///
/// The pipelined workers share `pipelined.rs`'s atomic-counter helper, but
/// `run_serial` aggregates into direct [`ProcessStats`] fields (no atomics),
/// so it keeps its own copy per the adjudicated review decision — the two
/// mechanisms don't share a counter shape. Outcome→field mapping matches the
/// pipelined helper: `Requeued`/`Exhausted`/`ParkedForCookies` bump the named
/// counter; `StaleClaim` bumps `stale_after_failure` (+ warn) — nothing was
/// recorded, symmetric to the success-side stale routing.
///
/// ADR 0038: the serial path has no `FetchOpts` carried alongside the claim
/// the way the pipelined workers do, so it recomputes the policy tag via
/// `format_policy_for(claim).tag()` instead of threading one through.
/// `format_policy_for` is a pure function of `claim.last_retryable_kind`
/// (the immutable claim this failure was dispatched for), so this
/// recomputation is equivalent to the tag the actual fetch ran under.
fn record_fetch_failure_serial(
    store: &mut Store,
    stats: &mut ProcessStats,
    claim: &Claim,
    opts: &ProcessOptions,
    label: &str,
    message: &str,
    requires_cookie: bool,
) -> Result<()> {
    let outcome = store
        .record_fetch_failure(
            &claim.video_id,
            &opts.worker_id,
            label,
            message,
            format_policy_for(claim).tag(),
            opts.retries + 1,
            requires_cookie,
            opts.cookies_file.is_some(),
        )
        .with_context(|| format!("record_fetch_failure for {}", claim.video_id))?;
    match outcome {
        FailureRecordOutcome::Requeued => stats.requeued_for_retry += 1,
        FailureRecordOutcome::Exhausted => stats.exhausted_retries += 1,
        FailureRecordOutcome::ParkedForCookies => stats.parked_for_cookies += 1,
        FailureRecordOutcome::StaleClaim => {
            stats.stale_after_failure += 1;
            tracing::warn!(
                worker = %opts.worker_id,
                video_id = claim.video_id.as_str(),
                "record_fetch_failure: stale claim (swept + re-claimed elsewhere)"
            );
        }
    }
    Ok(())
}

/// Drive a single claim through phases 1-4. Thin caller over the shared
/// helpers (T15): `fetch_and_decode` runs phases 1+2; `transcribe_and_write`
/// runs phases 3+4 and owns the 0008 artifact-before-mark_succeeded
/// invariant (plus the StaleAfterSuccess branch).
// Reached only through `run_serial` (and this file's own tests); shares its
// retire-or-restore fate.
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

    // Epic 3 T08: kind-gated cookie routing (ADR 0035).
    let fetch_opts = cookie_opts_for(claim, &opts.classification, opts.cookies_file.as_deref());
    let (metadata_capture, fetch_result) = fetch_and_decode(fetcher, claim, &fetch_opts).await;
    // Epic 4c: raw envelope persists regardless of fetch outcome; best-effort.
    if let Some(capture) = metadata_capture {
        if let Err(e) =
            store.upsert_metadata_raw(&claim.video_id, &opts.worker_id, &capture.envelope_json)
        {
            tracing::warn!(
                video_id = claim.video_id.as_str(),
                error = %e,
                "metadata raw insert failed; continuing"
            );
        }
    }
    let (samples, audio) = fetch_result?;
    transcribe_and_write(
        store,
        transcriber,
        claim,
        samples,
        audio,
        fetcher.name(),
        opts,
    )
    .await
}

#[cfg(test)]
mod tests {
    //! Unit tests for `process_one` — placed in-module so the private
    //! function is reachable without a public re-export. The integration
    //! tests in `tests/pipeline_fakes/serial_tests.rs` exercise `run_serial`.
    use super::*;
    use crate::errors::TranscribeError;
    use crate::fetcher::{Acquisition, FakeFetcher, FetchOpts, VideoFetcher};
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

    /// A `FetchPhaseError` reachable only as another error's `source` — the
    /// case the top-level `downcast_ref` genuinely missed. `anyhow`'s own
    /// `downcast_ref` understands its `Context` wrappers (see the test
    /// below), but it knows nothing about a third-party error that carries
    /// a `FetchPhaseError` as `#[source]`; only a chain walk finds that.
    #[derive(Debug, thiserror::Error)]
    #[error("phase 1 failed for {video_id}")]
    struct SourceWrapped {
        video_id: &'static str,
        #[source]
        source: FetchPhaseError,
    }

    /// E12 ruling (operator, 2026-07-30): the fetch-side classification must
    /// walk the error chain, symmetric with the transcribe-side
    /// `e.chain().find_map(...)`, so a `FetchPhaseError` that a future edit
    /// buries under a wrapper still classifies correctly. The failure mode
    /// being closed is invisible in production: the row still fails, but
    /// with the `TranscribeOther` catch-all kind instead of its real
    /// classification — a terminal write-off silently downgraded to a
    /// retryable requeue.
    ///
    /// Zero, one and two `.context(...)` layers must all classify
    /// identically (the ruling's prescribed cases), and so must a
    /// `#[source]`-nested error.
    #[test]
    fn fetch_phase_classification_survives_context_layers() {
        use crate::errors::FetchError;

        let table =
            crate::classification::ClassificationTable::compiled_default().expect("default table");
        // A terminal write-off class: the verdict ARM differs from the
        // `TranscribeOther` retryable the miss used to produce, so a
        // regression changes the outcome, not just the label.
        let typed = || {
            FetchPhaseError::Fetch(FetchError::ToolFailed {
                tool: "yt-dlp".to_string(),
                exit_code: 1,
                signal: None,
                stderr_excerpt: "ERROR: Your IP address is blocked from accessing this post"
                    .to_string(),
            })
        };
        let mk = || anyhow::Error::from(typed());
        let cases = [
            ("no context", mk()),
            ("one context layer", mk().context("acquiring audio")),
            (
                "two context layers",
                mk().context("acquiring audio").context("processing vid_a"),
            ),
            // The discriminating case: only a chain walk reaches this one.
            (
                "source-nested under a context layer",
                anyhow::Error::from(SourceWrapped {
                    video_id: "vid_a",
                    source: typed(),
                })
                .context("processing vid_a"),
            ),
        ];
        for (label, err) in cases {
            match classify_fetch_phase_in_chain(&err, &table) {
                Some(ClassifiedFailure::Unavailable { label: got, .. }) => {
                    assert_eq!(got, "IpBlockedMessage", "{label}");
                }
                other => panic!("{label}: expected Unavailable(IpBlockedMessage), got {other:?}"),
            }
        }

        // The `None` contract is unchanged: a chain with no FetchPhaseError
        // anywhere in it still returns None, so `run_serial`'s transcribe-side
        // arm keeps receiving the errors it owns.
        let transcribe_side = anyhow::Error::from(TranscribeError::EmptyOutput)
            .context("transcribing vid_a")
            .context("processing vid_a");
        assert!(
            classify_fetch_phase_in_chain(&transcribe_side, &table).is_none(),
            "a transcribe-side chain must not be claimed by the fetch classifier"
        );
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
            first_call_gate: tokio::sync::Mutex::new(None),
            canned_stderr: Mutex::new(None),
            received_opts: Mutex::new(Vec::new()),
            fail_first_n: Mutex::new(HashMap::new()),
            canned_metadata: Mutex::new(None),
            received_urls: Mutex::new(Vec::new()),
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
        let (_capture, acq) = fetcher
            .acquire("vid_a", "https://example/a", &FetchOpts::default())
            .await;
        assert!(matches!(acq?, Acquisition::AudioFile { .. }));

        let opts = ProcessOptions {
            worker_id: "worker-1".into(),
            transcripts_root: tmp.path().join("transcripts"),
            max_videos: Some(1),
            compute_lang_probs: false,
            transcribe_timeout: Duration::from_secs(5),
            stale_claim_threshold: Duration::from_secs(60),
            download_workers: 3,
            channel_capacity: 2,
            cookies_file: None,
            classification: std::sync::Arc::new(
                crate::classification::ClassificationTable::compiled_default()
                    .expect("default table"),
            ),
            retries: 1,
            checkpoint: None,
            breaker_threshold: 0,
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
