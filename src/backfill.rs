//! Post-hoc metadata backfill (production ops, post-v0.3.0): recovers
//! `video_metadata_raw` envelopes for succeeded videos that predate
//! fetch-time capture (the rc1-era cohort). One metadata-only yt-dlp
//! invocation per video — no media, no GPU, and by construction no
//! writes to video status/lifecycle. Best-effort per video; re-running
//! converges (rows leave the cohort as envelopes land, and only
//! `insert_metadata_raw_if_missing` is ever called, so a fetch-path
//! envelope is never overwritten).

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::fetcher::ytdlp::{build_metadata_envelope, build_metadata_only_args, STDOUT_CAP};
use crate::process::{run, CommandSpec};
use crate::state::Store;

/// Cohort page size. Small next to the loader's 10k: every row costs a
/// network invocation, so paging overhead is noise, and a shorter page
/// keeps the cursor fresh against the shrinking cohort.
const PAGE_SIZE: usize = 1_000;

/// Backfill stats: input-side counters, verb-named (ADR-0007). Every
/// examined video increments exactly one outcome counter.
#[derive(Debug, Default, Serialize)]
pub(crate) struct BackfillStats {
    /// Cohort videos attempted this run.
    pub videos_examined: u64,
    /// Envelopes captured and inserted.
    pub envelopes_captured: u64,
    /// yt-dlp failed (nonzero exit, timeout, spawn/io) or printed no
    /// usable line — logged and skipped, never fatal.
    pub captures_failed: u64,
    /// Envelope built but a row already existed (fetch path landed one
    /// concurrently; theirs wins).
    pub rows_already_filled: u64,
    /// Envelope built but the DB insert failed (best-effort, counted).
    pub inserts_failed: u64,
}

impl std::fmt::Display for BackfillStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "examined {} / captured {} / capture-failed {} / already-filled {} / insert-failed {}",
            self.videos_examined,
            self.envelopes_captured,
            self.captures_failed,
            self.rows_already_filled,
            self.inserts_failed
        )
    }
}

/// One serial best-effort pass over the backfill cohort. `limit` caps
/// attempted videos (smoke runs). Returns `Err` only for Store/query
/// failures; per-video capture problems count and continue.
///
/// Deliberately serial — one yt-dlp invocation at a time is the rate
/// limit toward TikTok for a ~10K-row cohort; a worker pool here would
/// buy nothing but a block.
pub(crate) async fn backfill_metadata(
    store: &mut Store,
    ytdlp_timeout: Duration,
    limit: Option<u64>,
) -> Result<BackfillStats> {
    let mut stats = BackfillStats::default();
    let mut after: Option<String> = None;

    'pages: loop {
        let page = store.succeeded_missing_metadata_page(after.as_deref(), PAGE_SIZE)?;
        let Some(last) = page.last() else { break };
        after = Some(last.video_id.clone());

        for video in &page {
            if limit.is_some_and(|cap| stats.videos_examined >= cap) {
                break 'pages;
            }
            stats.videos_examined += 1;

            let envelope = match run(CommandSpec {
                program: "yt-dlp".to_string(),
                args: build_metadata_only_args(&video.source_url),
                timeout: ytdlp_timeout,
                stderr_capture_bytes: 8 * 1024,
                stdout_capture_bytes: STDOUT_CAP,
                redact_arg_indices: &[],
            })
            .await
            {
                Ok(outcome) => {
                    // Envelope-first, exit code second (ADR-0042): a
                    // printed line that landed before a late failure is
                    // still data.
                    let envelope = build_metadata_envelope(outcome.stdout.as_deref(), STDOUT_CAP);
                    if envelope.is_none() {
                        tracing::warn!(
                            video_id = video.video_id.as_str(),
                            exit_code = outcome.exit_code,
                            stderr = outcome.stderr_excerpt.as_str(),
                            "backfill: no usable metadata line; skipped"
                        );
                    }
                    envelope
                }
                Err(err) => {
                    tracing::warn!(
                        video_id = video.video_id.as_str(),
                        error = %err,
                        "backfill: yt-dlp did not run to completion; skipped"
                    );
                    None
                }
            };

            let Some(envelope_json) = envelope else {
                stats.captures_failed += 1;
                continue;
            };

            match store.insert_metadata_raw_if_missing(&video.video_id, &envelope_json) {
                Ok(1) => stats.envelopes_captured += 1,
                Ok(_) => stats.rows_already_filled += 1,
                Err(err) => {
                    tracing::warn!(
                        video_id = video.video_id.as_str(),
                        error = %err,
                        "backfill: metadata insert failed; continuing"
                    );
                    stats.inserts_failed += 1;
                }
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_display_is_operator_legible() {
        let stats = BackfillStats {
            videos_examined: 10,
            envelopes_captured: 7,
            captures_failed: 2,
            rows_already_filled: 1,
            inserts_failed: 0,
        };
        assert_eq!(
            stats.to_string(),
            "examined 10 / captured 7 / capture-failed 2 / already-filled 1 / insert-failed 0"
        );
    }
}
