//! Read-only Store queries for the operator-facing `status` subcommand
//! (Epic 4b). Reporting layer: no mutations, no transactions. Mutators
//! stay in `state/mod.rs` per 0006/0023; these return typed row structs
//! (the `list_failed_retryable` precedent).

use std::collections::BTreeMap;

use anyhow::{Context, Result};
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
}
