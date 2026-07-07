//! Operator triage pass (ADR 0034): the retry executor. Classifies stored
//! failure messages, probes ambiguous rows via the oEmbed oracle, drains
//! dead rows to failed_terminal, requeues recoverable rows under an attempt
//! cap. The census it prints doubles as the study's attrition documentation.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use anyhow::Result;

use crate::classification::{ClassificationTable, Disposition};
use crate::probe::{ProbeOracle, ProbeVerdict};
use crate::state::Store;

pub struct TriageOptions {
    pub dry_run: bool,
    pub rate_per_sec: f64,
    pub max_attempts: i64,
}

#[derive(Debug, Default)]
pub struct KindCounts {
    pub examined: usize,
    pub marked_terminal: usize,
    pub requeued: usize,
    pub kept_unreachable: usize,
    pub kept_capped: usize,
}

/// 0007: input-side counters, verb-named. `examined >= marked_terminal +
/// requeued + kept_unreachable + kept_capped`; the two sides are not always
/// equal since commit 5c837bb (0006 row-change-count gating). The gap counts
/// predicate misses — a row changed state between `list_failed_retryable`'s
/// snapshot and the mutating UPDATE — each of which is logged with a
/// `tracing::warn!` at the call site instead of incrementing an action
/// counter.
#[derive(Debug, Default)]
pub struct TriageStats {
    pub examined: usize,
    pub marked_terminal: usize,
    pub requeued: usize,
    pub kept_unreachable: usize,
    pub kept_capped: usize,
    /// Census keyed by classification label (write-off rows keyed by the
    /// terminal-dispositioned label). Attrition table for the paper.
    pub by_kind: BTreeMap<String, KindCounts>,
}

// Epic 4a T03 (disclosed shim, pending T08's deletion of this module): the
// retired `classify_message` chain is gone, so triage now classifies via
// the same `ClassificationTable` the pipeline uses. `Disposition::Terminal`
// takes the old `MessageVerdict::Unavailable` write-off path; both
// `Retryable` and `RequiresCookie` take the old `MessageVerdict::Retryable`
// probe path (triage predates the cookie-gate concept — RequiresCookie
// rows still get probed here, same as any other retryable class, until
// Task 08 deletes this module entirely).
pub async fn run_triage(
    store: &mut Store,
    oracle: &dyn ProbeOracle,
    opts: &TriageOptions,
) -> Result<TriageStats> {
    let table = ClassificationTable::compiled_default()?;
    let rows = store.list_failed_retryable()?;
    let mut stats = TriageStats::default();
    let probe_gap = Duration::from_secs_f64(1.0 / opts.rate_per_sec.max(0.001));

    for row in rows {
        stats.examined += 1;
        let message = row.last_retryable_message.as_deref().unwrap_or("");
        let m = table.classify(message);
        match m.disposition {
            Disposition::Terminal => {
                let label = m.label.to_string();
                let k = stats.by_kind.entry(label.clone()).or_default();
                k.examined += 1;
                // 0006: census increments are gated on the mutator's returned
                // row-change count. A predicate miss (row left failed_retryable
                // between list_failed_retryable's SELECT and this UPDATE) must
                // not be recorded as an action taken — the census doubles as
                // the study's attrition documentation. Dry-run reports the
                // verdict unconditionally (plan-mandated: zero mutations, same
                // counters).
                let changed = if opts.dry_run {
                    1
                } else {
                    store.triage_mark_terminal(
                        &row.video_id,
                        &label,
                        "triage: message-class write-off",
                    )?
                };
                if changed > 0 {
                    stats.marked_terminal += 1;
                    k.marked_terminal += 1;
                } else {
                    tracing::warn!(
                        video_id = row.video_id.as_str(),
                        action = "triage_mark_terminal (message-class write-off)",
                        "triage: predicate miss; row no longer failed_retryable — not counted"
                    );
                }
            }
            Disposition::Retryable | Disposition::RequiresCookie => {
                let label = m.label.to_string();
                let verdict = oracle.probe(&row.video_id).await;
                tokio::time::sleep(probe_gap).await;
                let k = stats.by_kind.entry(label.clone()).or_default();
                k.examined += 1;
                match verdict {
                    ProbeVerdict::Dead => {
                        // 0006 row-count gating; see the Unavailable arm.
                        let changed = if opts.dry_run {
                            1
                        } else {
                            store.triage_mark_terminal(
                                &row.video_id,
                                "ProbeDead",
                                "triage: oEmbed probe returned dead",
                            )?
                        };
                        if changed > 0 {
                            stats.marked_terminal += 1;
                            k.marked_terminal += 1;
                        } else {
                            tracing::warn!(
                                video_id = row.video_id.as_str(),
                                action = "triage_mark_terminal (ProbeDead)",
                                "triage: predicate miss; row no longer failed_retryable — not counted"
                            );
                        }
                    }
                    ProbeVerdict::Alive => {
                        if row.attempt_count < opts.max_attempts {
                            // 0006 row-count gating; see the Unavailable arm.
                            let changed = if opts.dry_run {
                                1
                            } else {
                                store.requeue_retryable(&row.video_id, &label, opts.max_attempts)?
                            };
                            if changed > 0 {
                                stats.requeued += 1;
                                k.requeued += 1;
                            } else {
                                tracing::warn!(
                                    video_id = row.video_id.as_str(),
                                    action = "requeue_retryable",
                                    "triage: predicate miss; row no longer failed_retryable — not counted"
                                );
                            }
                        } else {
                            stats.kept_capped += 1;
                            k.kept_capped += 1;
                        }
                    }
                    ProbeVerdict::Unreachable(why) => {
                        tracing::warn!(
                            video_id = row.video_id.as_str(),
                            why,
                            "triage: probe unreachable; row untouched"
                        );
                        stats.kept_unreachable += 1;
                        k.kept_unreachable += 1;
                    }
                }
            }
        }
    }
    Ok(stats)
}

impl fmt::Display for TriageStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "kind                       examined  terminal  requeued  unreach  capped"
        )?;
        for (kind, c) in &self.by_kind {
            writeln!(
                f,
                "{kind:<26} {:>8} {:>9} {:>9} {:>8} {:>7}",
                c.examined, c.marked_terminal, c.requeued, c.kept_unreachable, c.kept_capped
            )?;
        }
        writeln!(
            f,
            "TOTAL                      {:>8} {:>9} {:>9} {:>8} {:>7}",
            self.examined,
            self.marked_terminal,
            self.requeued,
            self.kept_unreachable,
            self.kept_capped
        )
    }
}
