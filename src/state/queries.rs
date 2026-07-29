//! Read-only Store queries for the operator-facing `status` subcommand
//! (Epic 4b). Reporting layer: no mutations, no transactions. Mutators
//! stay in `state/mod.rs` per 0006/0023; these return typed row structs
//! (the `list_failed_retryable` precedent).

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::Serialize;

use super::Store;

/// One `in_progress` row as the operator sees it: who claimed it and when.
/// `claimed_by`/`claimed_at` are nullable in the schema, so a malformed row
/// renders as unknown rather than crashing the report.
#[derive(Debug, Serialize)]
pub struct InProgressRow {
    pub video_id: String,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<i64>,
}

/// One `batch_runs` row, raw. `finished_at IS NULL` is the on-disk
/// fingerprint of an interrupted run (0036-era design); rendering it
/// honestly is a hard requirement of the 4b status work.
#[derive(Debug, Serialize)]
pub struct BatchRunRow {
    pub run_id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub params_json: String,
    pub policy_toml: String,
    pub census_json: Option<String>,
}

impl Store {
    /// Video counts grouped by status. Statuses absent from the table are
    /// absent from the map; `status::build_report` zero-fills the fixed
    /// five-status vocabulary.
    pub fn count_by_status(&self) -> Result<BTreeMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM videos GROUP BY status")
            .context("prepare count_by_status")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .context("query count_by_status")?
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()
            .context("collect count_by_status")?;
        Ok(rows)
    }

    /// failed_retryable counts grouped by `last_retryable_kind`. NULL kinds
    /// group under "(none)" so the sum always matches the status count.
    pub fn count_retryable_by_kind(&self) -> Result<BTreeMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COALESCE(last_retryable_kind, '(none)'), COUNT(*)
                 FROM videos WHERE status = 'failed_retryable'
                 GROUP BY COALESCE(last_retryable_kind, '(none)')",
            )
            .context("prepare count_retryable_by_kind")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .context("query count_retryable_by_kind")?
            .collect::<std::result::Result<BTreeMap<_, _>, _>>()
            .context("collect count_retryable_by_kind")?;
        Ok(rows)
    }

    /// Current claims, oldest first — the "is anything stuck / safe to
    /// pause?" surface. Cross-reference 0024: the next `process` run's
    /// stale sweep re-queues rows older than the threshold (default 30m).
    pub fn list_in_progress(&self) -> Result<Vec<InProgressRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT video_id, claimed_by, claimed_at FROM videos
                 WHERE status = 'in_progress'
                 ORDER BY claimed_at ASC, video_id ASC",
            )
            .context("prepare list_in_progress")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(InProgressRow {
                    video_id: r.get(0)?,
                    claimed_by: r.get(1)?,
                    claimed_at: r.get(2)?,
                })
            })
            .context("query list_in_progress")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_in_progress")?;
        Ok(rows)
    }

    /// Full batch-run history, oldest first. Returns raw column values —
    /// params/census parsing and policy provenance live in `status`
    /// (reporting policy), not here (storage).
    pub fn list_batch_runs(&self) -> Result<Vec<BatchRunRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT run_id, started_at, finished_at, params_json, policy_toml, census_json
                 FROM batch_runs ORDER BY run_id ASC",
            )
            .context("prepare list_batch_runs")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(BatchRunRow {
                    run_id: r.get(0)?,
                    started_at: r.get(1)?,
                    finished_at: r.get(2)?,
                    params_json: r.get(3)?,
                    policy_toml: r.get(4)?,
                    census_json: r.get(5)?,
                })
            })
            .context("query list_batch_runs")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_batch_runs")?;
        Ok(rows)
    }

    /// All succeeded video_ids — the population the 0017 done-contract
    /// checks walk. Plain Vec: the caller groups by shard.
    pub fn list_succeeded_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT video_id FROM videos WHERE status = 'succeeded' ORDER BY video_id")
            .context("prepare list_succeeded_ids")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .context("query list_succeeded_ids")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_succeeded_ids")?;
        Ok(rows)
    }
}

/// One raw metadata envelope row (Epic 4c loader input).
#[derive(Debug)]
pub struct RawMetadataRow {
    pub video_id: String,
    pub fetched_at: i64,
    pub raw_json: String,
}

fn map_raw_metadata_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawMetadataRow> {
    Ok(RawMetadataRow {
        video_id: r.get(0)?,
        fetched_at: r.get(1)?,
        raw_json: r.get(2)?,
    })
}

impl Store {
    /// Keyset page over `video_metadata_raw` ordered by video_id. Streaming
    /// input for `load_metadata` — never materializes the whole table
    /// (6–12 GB at production scale). `after_video_id` is the last id of
    /// the previous page; `None` starts at the beginning.
    ///
    /// Split into two cached statements chosen by the cursor rather than one
    /// `WHERE (?1 IS NULL OR video_id > ?1)` statement: `EXPLAIN QUERY PLAN`
    /// showed SQLite planning the single OR-NULL shape as a full ordered
    /// index scan (the plan has to stay valid for the NULL case), so every
    /// page rescanned from the start of the table — O(n²) total over a
    /// 3M-row table. Each branch below plans as an index seek instead.
    pub fn metadata_raw_page(
        &self,
        after_video_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RawMetadataRow>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = match after_video_id {
            None => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT video_id, fetched_at, raw_json FROM video_metadata_raw
                         ORDER BY video_id
                         LIMIT ?1",
                    )
                    .context("prepare metadata_raw_page (first page)")?;
                let rows = stmt
                    .query_map(rusqlite::params![limit], map_raw_metadata_row)
                    .context("query metadata_raw_page (first page)")?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .context("collect metadata_raw_page (first page)")?;
                rows
            }
            Some(after) => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT video_id, fetched_at, raw_json FROM video_metadata_raw
                         WHERE video_id > ?1
                         ORDER BY video_id
                         LIMIT ?2",
                    )
                    .context("prepare metadata_raw_page (subsequent page)")?;
                let rows = stmt
                    .query_map(rusqlite::params![after, limit], map_raw_metadata_row)
                    .context("query metadata_raw_page (subsequent page)")?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .context("collect metadata_raw_page (subsequent page)")?;
                rows
            }
        };
        Ok(rows)
    }
}

/// One succeeded video missing its raw metadata envelope — input row for
/// the backfill-metadata cohort walk.
#[derive(Debug)]
pub struct MissingMetadataVideo {
    pub video_id: String,
    pub source_url: String,
}

impl Store {
    /// Size of the backfill cohort: succeeded videos with no
    /// video_metadata_raw row (the rc1-era gap). Read-only.
    pub fn count_succeeded_missing_metadata(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .prepare_cached(
                "SELECT COUNT(*) FROM videos v
                 WHERE v.status = 'succeeded'
                   AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m
                                   WHERE m.video_id = v.video_id)",
            )
            .context("prepare count_succeeded_missing_metadata")?
            .query_row([], |r| r.get(0))
            .context("query count_succeeded_missing_metadata")?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Keyset page over the backfill cohort, ordered by video_id.
    ///
    /// Two cached statements chosen by the cursor, not one OR-NULL
    /// statement — see `metadata_raw_page`'s comment for the O(n²) plan
    /// the single-statement shape produces. The page seeks on the
    /// videos PK and filters status per row (the only secondary index
    /// is pending-only); fine at cohort scale (~10K of ~3M rows).
    /// Rows leave the cohort as envelopes land, which is safe: the
    /// cursor only moves forward, and vanished rows are all behind it.
    /// Videos succeeding behind the cursor during a live run are caught
    /// by a re-run (rerun-to-convergence semantics).
    pub fn succeeded_missing_metadata_page(
        &self,
        after_video_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MissingMetadataVideo>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let map = |r: &rusqlite::Row<'_>| {
            Ok(MissingMetadataVideo {
                video_id: r.get(0)?,
                source_url: r.get(1)?,
            })
        };
        let rows = match after_video_id {
            None => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT v.video_id, v.source_url FROM videos v
                         WHERE v.status = 'succeeded'
                           AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m
                                           WHERE m.video_id = v.video_id)
                         ORDER BY v.video_id
                         LIMIT ?1",
                    )
                    .context("prepare succeeded_missing_metadata_page (first page)")?;
                let rows = stmt
                    .query_map(rusqlite::params![limit], map)
                    .context("query succeeded_missing_metadata_page (first page)")?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .context("collect succeeded_missing_metadata_page (first page)")?;
                rows
            }
            Some(after) => {
                let mut stmt = self
                    .conn
                    .prepare_cached(
                        "SELECT v.video_id, v.source_url FROM videos v
                         WHERE v.status = 'succeeded'
                           AND v.video_id > ?1
                           AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m
                                           WHERE m.video_id = v.video_id)
                         ORDER BY v.video_id
                         LIMIT ?2",
                    )
                    .context("prepare succeeded_missing_metadata_page (subsequent page)")?;
                let rows = stmt
                    .query_map(rusqlite::params![after, limit], map)
                    .context("query succeeded_missing_metadata_page (subsequent page)")?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .context("collect succeeded_missing_metadata_page (subsequent page)")?;
                rows
            }
        };
        Ok(rows)
    }
}

/// Full videos-row projection for `status --video-id`. Every nullable
/// column stays Option — the renderer decides what to show.
#[derive(Debug, Serialize)]
pub struct VideoDetailRow {
    pub video_id: String,
    pub source_url: String,
    pub status: String,
    pub attempt_count: i64,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<i64>,
    pub succeeded_at: Option<i64>,
    pub duration_s: Option<f64>,
    pub language_detected: Option<String>,
    pub fetcher: Option<String>,
    pub transcript_source: Option<String>,
    pub last_retryable_kind: Option<String>,
    pub last_retryable_message: Option<String>,
    pub terminal_reason: Option<String>,
    pub terminal_message: Option<String>,
    pub first_seen_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct VideoEventRow {
    pub at: i64,
    pub event_type: String,
    pub worker_id: Option<String>,
    pub detail_json: Option<String>,
}

/// Per-respondent summary per the original spec § status: counts only;
/// itemized inspection goes through --video-id. (The spec's
/// unresolved_short_links field is omitted: pending_resolutions never
/// shipped — short links are skipped at ingest.)
#[derive(Debug, Serialize)]
pub struct RespondentSummary {
    pub respondent_id: String,
    pub watch_events: i64,
    pub videos_seen: i64,
    pub videos_in_window: i64,
    pub videos_succeeded: i64,
    pub videos_failed_terminal: i64,
    pub videos_failed_retryable: i64,
    pub videos_pending: i64,
    pub videos_in_progress: i64,
}

#[derive(Debug, Serialize)]
pub struct TerminalRow {
    pub video_id: String,
    pub terminal_reason: Option<String>,
    pub terminal_message: Option<String>,
    pub updated_at: i64,
}

impl Store {
    pub fn get_video_detail(&self, video_id: &str) -> Result<Option<VideoDetailRow>> {
        self.conn
            .query_row(
                "SELECT video_id, source_url, status, attempt_count, claimed_by,
                        claimed_at, succeeded_at, duration_s, language_detected,
                        fetcher, transcript_source, last_retryable_kind,
                        last_retryable_message, terminal_reason, terminal_message,
                        first_seen_at, updated_at
                 FROM videos WHERE video_id = ?1",
                rusqlite::params![video_id],
                |r| {
                    Ok(VideoDetailRow {
                        video_id: r.get(0)?,
                        source_url: r.get(1)?,
                        status: r.get(2)?,
                        attempt_count: r.get(3)?,
                        claimed_by: r.get(4)?,
                        claimed_at: r.get(5)?,
                        succeeded_at: r.get(6)?,
                        duration_s: r.get(7)?,
                        language_detected: r.get(8)?,
                        fetcher: r.get(9)?,
                        transcript_source: r.get(10)?,
                        last_retryable_kind: r.get(11)?,
                        last_retryable_message: r.get(12)?,
                        terminal_reason: r.get(13)?,
                        terminal_message: r.get(14)?,
                        first_seen_at: r.get(15)?,
                        updated_at: r.get(16)?,
                    })
                },
            )
            .optional()
            .context("get_video_detail")
    }

    pub fn list_video_events(&self, video_id: &str) -> Result<Vec<VideoEventRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT at, event_type, worker_id, detail_json
                 FROM video_events WHERE video_id = ?1 ORDER BY at ASC, id ASC",
            )
            .context("prepare list_video_events")?;
        let rows = stmt
            .query_map(rusqlite::params![video_id], |r| {
                Ok(VideoEventRow {
                    at: r.get(0)?,
                    event_type: r.get(1)?,
                    worker_id: r.get(2)?,
                    detail_json: r.get(3)?,
                })
            })
            .context("query list_video_events")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_video_events")?;
        Ok(rows)
    }

    pub fn respondent_summary(&self, respondent_id: &str) -> Result<RespondentSummary> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                        COUNT(DISTINCT wh.video_id),
                        COUNT(DISTINCT CASE WHEN wh.in_window = 1 THEN wh.video_id END),
                        COUNT(DISTINCT CASE WHEN v.status = 'succeeded' THEN wh.video_id END),
                        COUNT(DISTINCT CASE WHEN v.status = 'failed_terminal' THEN wh.video_id END),
                        COUNT(DISTINCT CASE WHEN v.status = 'failed_retryable' THEN wh.video_id END),
                        COUNT(DISTINCT CASE WHEN v.status = 'pending' THEN wh.video_id END),
                        COUNT(DISTINCT CASE WHEN v.status = 'in_progress' THEN wh.video_id END)
                 FROM watch_history wh JOIN videos v ON v.video_id = wh.video_id
                 WHERE wh.respondent_id = ?1",
                rusqlite::params![respondent_id],
                |r| {
                    Ok(RespondentSummary {
                        respondent_id: respondent_id.to_string(),
                        watch_events: r.get(0)?,
                        videos_seen: r.get(1)?,
                        videos_in_window: r.get(2)?,
                        videos_succeeded: r.get(3)?,
                        videos_failed_terminal: r.get(4)?,
                        videos_failed_retryable: r.get(5)?,
                        videos_pending: r.get(6)?,
                        videos_in_progress: r.get(7)?,
                    })
                },
            )
            .context("respondent_summary")
    }

    /// failed_terminal rows for `status --errors`, most recently updated
    /// first (fresh write-offs are what the operator is usually chasing).
    pub fn list_terminal_failures(&self) -> Result<Vec<TerminalRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT video_id, terminal_reason, terminal_message, updated_at
                 FROM videos WHERE status = 'failed_terminal'
                 ORDER BY updated_at DESC, video_id ASC",
            )
            .context("prepare list_terminal_failures")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TerminalRow {
                    video_id: r.get(0)?,
                    terminal_reason: r.get(1)?,
                    terminal_message: r.get(2)?,
                    updated_at: r.get(3)?,
                })
            })
            .context("query list_terminal_failures")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("collect list_terminal_failures")?;
        Ok(rows)
    }
}
