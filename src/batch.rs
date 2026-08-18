//! Batch lifecycle bookkeeping (Epic 4a): the start-of-batch sweep of
//! parked failures and the durable census. Policy layer — reads the
//! classification table, drives Store mutators, never touches the network
//! or the fetch path.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::classification::{ClassificationTable, Disposition};
use crate::pipeline::ProcessStats;
use crate::state::Store;

/// Input-side sweep counters (0007).
/// `examined >= swept_terminal + requeued_for_retry + parked_for_cookies + kept_capped`:
/// a Terminal-arm predicate miss (a concurrent writer moved the row off
/// `failed_retryable` between `list_failed_retryable`'s snapshot and the
/// UPDATE) is examined but lands in no action bucket — each such miss is
/// logged with `tracing::warn!` in `run_sweep` instead of incrementing a
/// counter. The requeue arm has no such gap: its predicate misses (cap hit
/// or concurrent move) land in `kept_capped`.
#[derive(Debug, Default, Serialize)]
pub struct SweepStats {
    pub examined: usize,
    pub swept_terminal: usize,
    /// Attrition breakdown: which write-off classes died in the sweep.
    pub swept_terminal_by_label: BTreeMap<String, usize>,
    pub requeued_for_retry: usize,
    pub parked_for_cookies: usize,
    pub kept_capped: usize,
}

#[derive(Debug, Serialize)]
pub struct RunCensus {
    pub claimed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub requeued_for_retry: usize,
    pub exhausted_retries: usize,
    pub parked_for_cookies: usize,
    /// Attrition breakdown: inline write-offs during the drain, by label.
    pub terminal_by_label: BTreeMap<String, usize>,
    pub stale_after_success: usize,
    pub stale_after_failure: usize,
    /// Epic 5a: operator checkpoint hook firings this run (exit 0) and
    /// firings that failed. Not attrition — run infrastructure — but they
    /// belong in the durable per-run record alongside the `checkpoint_cmd`
    /// / `checkpoint_every_secs` config in `batch_runs.params_json`: "was
    /// this run's output actually being synced mid-run?" is answerable
    /// only from the pair.
    pub checkpoints_run: u64,
    pub checkpoints_failed: u64,
    /// Breaker ADR (0050): DB-visible per the standing operator ruling —
    /// the verdict must be answerable from `batch_runs.census_json` alone.
    pub breaker_tripped: bool,
}

impl From<&ProcessStats> for RunCensus {
    fn from(s: &ProcessStats) -> Self {
        RunCensus {
            claimed: s.claimed,
            succeeded: s.succeeded,
            failed: s.failed,
            requeued_for_retry: s.requeued_for_retry,
            exhausted_retries: s.exhausted_retries,
            parked_for_cookies: s.parked_for_cookies,
            terminal_by_label: s.terminal_by_label.clone(),
            stale_after_success: s.stale_after_success,
            stale_after_failure: s.stale_after_failure,
            checkpoints_run: s.checkpoints_run,
            checkpoints_failed: s.checkpoints_failed,
            breaker_tripped: s.breaker_tripped,
        }
    }
}

/// The batch's attrition record: persisted to batch_runs.census_json AND
/// printed for the operator. The generating policy rides alongside in
/// batch_runs.policy_toml — a census without its policy is not
/// reproducible attrition documentation.
///
/// Deliberately no bug-escalation counter (spec §4 deviation, disclosed):
/// a Bug aborts the run before close_batch_run, so a census recording one
/// can never be written — the honest bug record is the batch_runs row
/// left with finished_at IS NULL.
#[derive(Debug, Serialize)]
pub struct BatchCensus {
    pub sweep: SweepStats,
    pub run: RunCensus,
}

impl std::fmt::Display for BatchCensus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "batch census")?;
        writeln!(f, "  sweep: examined {:>6}", self.sweep.examined)?;
        writeln!(f, "    swept_terminal     {:>6}", self.sweep.swept_terminal)?;
        for (label, n) in &self.sweep.swept_terminal_by_label {
            writeln!(f, "      {label:<24} {n:>6}")?;
        }
        writeln!(
            f,
            "    requeued_for_retry {:>6}",
            self.sweep.requeued_for_retry
        )?;
        writeln!(
            f,
            "    parked_for_cookies {:>6}",
            self.sweep.parked_for_cookies
        )?;
        writeln!(f, "    kept_capped        {:>6}", self.sweep.kept_capped)?;
        writeln!(f, "  run:   claimed  {:>6}", self.run.claimed)?;
        writeln!(f, "    succeeded          {:>6}", self.run.succeeded)?;
        writeln!(f, "    failed             {:>6}", self.run.failed)?;
        writeln!(
            f,
            "    requeued_for_retry {:>6}",
            self.run.requeued_for_retry
        )?;
        writeln!(
            f,
            "    exhausted_retries  {:>6}",
            self.run.exhausted_retries
        )?;
        writeln!(
            f,
            "    parked_for_cookies {:>6}",
            self.run.parked_for_cookies
        )?;
        writeln!(
            f,
            "    terminal (inline)  {:>6}",
            self.run.terminal_by_label.values().sum::<usize>()
        )?;
        for (label, n) in &self.run.terminal_by_label {
            writeln!(f, "      {label:<24} {n:>6}")?;
        }
        writeln!(
            f,
            "    stale_after_success {:>5}",
            self.run.stale_after_success
        )?;
        writeln!(
            f,
            "    stale_after_failure {:>5}",
            self.run.stale_after_failure
        )?;
        writeln!(f, "    checkpoints_run    {:>6}", self.run.checkpoints_run)?;
        writeln!(
            f,
            "    checkpoints_failed {:>6}",
            self.run.checkpoints_failed
        )?;
        writeln!(f, "    breaker_tripped    {:>6}", self.run.breaker_tripped)
    }
}

/// Truncate `s` to at most `max_bytes` BYTES without splitting a character:
/// floor to the nearest char boundary at or below the cap.
/// (`str::floor_char_boundary` is nightly-only on our toolchain; a boundary
/// is at most 3 bytes below any index, so the loop is bounded.)
/// Also used by `status`'s event renderer to cap message excerpts.
pub(crate) fn truncate_to_char_boundary(s: &mut String, max_bytes: usize) {
    if s.len() <= max_bytes {
        return;
    }
    let mut cut = max_bytes;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

/// Census JSON for a run that died before its stats existed (worker Err
/// path). Sweep counters are real (computed pre-workers); run counters are
/// unrecoverable — the `aborted` marker plus the error string is the
/// DB-visible record that this row's absence of run counters is a crash,
/// not a zero-work run. Consumed by `commands::dispatch`'s Process arm.
pub fn aborted_census_json(sweep: &SweepStats, error: &str) -> serde_json::Result<String> {
    serde_json::to_string(&serde_json::json!({
        "aborted": true,
        "error": error,
        "sweep": sweep,
    }))
}

/// Start-of-batch sweep (Epic 4a, spec §3): adjudicate every parked
/// failed_retryable row through the active table. Terminal dispositions
/// terminalize (this is where historical write-off classes die on the
/// first post-upgrade run); retryables requeue under the cap; the cookie
/// pool moves only when cookies are configured — no attempt bump either
/// way (sweeping isn't fetching). Idempotent: a concurrent second sweep's
/// predicates all miss.
///
/// Note on a `terminal` fallback: an operator table whose fallback is
/// `terminal` write-offs even non-fetch rows (e.g. a stored `ToolTimeout`)
/// under the fallback label, since a fallback hit relabels on the terminal
/// arm. The audit trail survives — `last_retryable_kind`/`last_retryable_message`
/// still carry the row's real pre-sweep classification.
pub fn run_sweep(
    store: &mut Store,
    table: &ClassificationTable,
    retries: i64,
    cookies_configured: bool,
) -> Result<SweepStats> {
    let mut stats = SweepStats::default();
    let rows = store
        .list_failed_retryable()
        .context("sweep: list parked rows")?;
    for row in rows {
        stats.examined += 1;
        let message = row.last_retryable_message.as_deref().unwrap_or("");
        let m = table.classify(message);
        match m.disposition {
            Disposition::Terminal => {
                let mut msg = format!("[sweep] {message}");
                // Char-boundary-safe: stored yt-dlp/TikTok text can be
                // localized; a plain truncate(500) panics mid-character
                // (T07 review fix). The full text stays in
                // last_retryable_message, which the mutator preserves.
                truncate_to_char_boundary(&mut msg, 500);
                let changed = store
                    .sweep_mark_terminal(&row.video_id, m.label, &msg)
                    .with_context(|| format!("sweep terminal for {}", row.video_id))?;
                if changed > 0 {
                    stats.swept_terminal += 1;
                    *stats
                        .swept_terminal_by_label
                        .entry(m.label.to_string())
                        .or_insert(0) += 1;
                } else {
                    // Predicate miss: a concurrent writer moved the row off
                    // failed_retryable after our snapshot. Examined but in
                    // no action bucket — warned, not counted (see the
                    // SweepStats doc; same convention as the run census's
                    // mutator-miss handling).
                    tracing::warn!(
                        video_id = row.video_id.as_str(),
                        action = "sweep_mark_terminal",
                        "sweep: predicate miss; row no longer failed_retryable — not counted"
                    );
                }
            }
            Disposition::RequiresCookie if !cookies_configured => {
                stats.parked_for_cookies += 1;
            }
            Disposition::Retryable | Disposition::RequiresCookie => {
                // T07 review fix (adjudicated, preserve-kind-on-fallback): a
                // fallback hit carries no positive evidence about the message
                // class, so it must not overwrite a real stored kind (e.g.
                // ToolTimeout/TranscribeOther — non-fetch failures this
                // fetch-stderr table never matches). Empty/NULL kinds and the
                // legacy placeholder "Fetch" still take the fallback label so
                // they normalize before becoming claimable (the cookie gate
                // reads the kind at claim time).
                let kind = if m.matched_rule {
                    m.label
                } else {
                    match row.last_retryable_kind.as_deref() {
                        Some(k) if !k.is_empty() && k != "Fetch" => k,
                        _ => m.label,
                    }
                };
                let changed = store
                    .sweep_requeue(&row.video_id, kind, retries + 1)
                    .with_context(|| format!("sweep requeue for {}", row.video_id))?;
                if changed > 0 {
                    stats.requeued_for_retry += 1;
                } else {
                    stats.kept_capped += 1;
                }
            }
        }
    }
    tracing::info!(
        examined = stats.examined,
        swept_terminal = stats.swept_terminal,
        requeued_for_retry = stats.requeued_for_retry,
        parked_for_cookies = stats.parked_for_cookies,
        kept_capped = stats.kept_capped,
        "start-of-batch sweep complete"
    );
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classification::ClassificationTable;
    use crate::state::Store;
    use tempfile::TempDir;

    fn seed_parked(
        store: &mut Store,
        tmp: &TempDir,
        id: &str,
        kind: &str,
        msg: &str,
        attempts: i64,
    ) {
        store
            .upsert_video(id, &format!("https://example/{id}"), false)
            .unwrap();
        let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
        conn.execute(
            "UPDATE videos SET status='failed_retryable', last_retryable_kind=?2,
             last_retryable_message=?3, attempt_count=?4 WHERE video_id=?1",
            rusqlite::params![id, kind, msg, attempts],
        )
        .unwrap();
    }

    #[test]
    fn sweep_splits_by_disposition_and_cap() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();

        // Historical placeholder kind "Fetch" + write-off message → terminal.
        seed_parked(
            &mut store,
            &tmp,
            "v_dead",
            "Fetch",
            "ERROR: Your IP address is blocked, sad",
            1,
        );
        // 10240 (former YtDlpOther) → terminal.
        seed_parked(
            &mut store,
            &tmp,
            "v_10240",
            "Fetch",
            "ERROR: Video not available, status code 10240; report",
            1,
        );
        // Retryable under cap → pending, kind normalized.
        seed_parked(
            &mut store,
            &tmp,
            "v_alive",
            "Fetch",
            "ERROR: Did not get any data blocks",
            1,
        );
        // Retryable at cap → kept.
        seed_parked(
            &mut store,
            &tmp,
            "v_capped",
            "NoDataBlocks",
            "ERROR: Did not get any data blocks",
            2,
        );
        // requires-cookie, no cookies → parked untouched.
        seed_parked(
            &mut store,
            &tmp,
            "v_cookie",
            "Fetch",
            "This post may not be comfortable for some audiences",
            1,
        );

        let stats = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(stats.examined, 5);
        assert_eq!(stats.swept_terminal, 2);
        assert_eq!(
            stats.swept_terminal_by_label.get("IpBlockedMessage"),
            Some(&1)
        );
        assert_eq!(
            stats.swept_terminal_by_label.get("VideoNotAvailable10240"),
            Some(&1)
        );
        assert_eq!(stats.requeued_for_retry, 1);
        assert_eq!(stats.kept_capped, 1);
        assert_eq!(stats.parked_for_cookies, 1);

        let dead = store.get_video_for_test("v_dead").unwrap().unwrap();
        assert_eq!(dead.status, "failed_terminal");
        assert_eq!(dead.terminal_reason.as_deref(), Some("IpBlockedMessage"));
        let alive = store.get_video_for_test("v_alive").unwrap().unwrap();
        assert_eq!(alive.status, "pending");
        assert_eq!(alive.last_retryable_kind.as_deref(), Some("NoDataBlocks"));
        let cookie = store.get_video_for_test("v_cookie").unwrap().unwrap();
        assert_eq!(
            cookie.status, "failed_retryable",
            "no attempt bump, no move"
        );
    }

    #[test]
    fn sweep_with_cookies_requeues_the_cookie_pool() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();
        seed_parked(
            &mut store,
            &tmp,
            "v_cookie",
            "SensitiveLoginGated",
            "This post may not be comfortable for some audiences",
            1,
        );
        let stats = run_sweep(&mut store, &table, 1, true).unwrap();
        assert_eq!(stats.requeued_for_retry, 1);
        assert_eq!(stats.parked_for_cookies, 0);
        assert_eq!(
            store
                .get_video_for_test("v_cookie")
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
    }

    #[test]
    fn second_sweep_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();
        seed_parked(
            &mut store,
            &tmp,
            "v_alive",
            "Fetch",
            "ERROR: Did not get any data blocks",
            1,
        );
        run_sweep(&mut store, &table, 1, false).unwrap();
        let second = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(
            second.examined, 0,
            "requeued rows left failed_retryable — nothing to sweep"
        );
    }

    /// Review fix (T07): `String::truncate` panics off a char boundary. A
    /// localized (multi-byte) stored message whose 500th byte falls inside
    /// a character must truncate safely, not panic the sweep.
    #[test]
    fn sweep_truncates_terminal_message_on_char_boundary() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();

        // "[sweep] " (8 bytes) + the 35-byte ASCII head = 43 bytes, then
        // 2-byte 'é's: char boundaries sit at odd offsets, so byte 500
        // lands MID-CHARACTER. Precondition asserted below.
        let msg = format!("ERROR: Your IP address is blocked, {}", "é".repeat(300));
        let full = format!("[sweep] {msg}");
        assert!(
            !full.is_char_boundary(500),
            "fixture must straddle the truncation boundary"
        );
        seed_parked(&mut store, &tmp, "v_wide", "Fetch", &msg, 1);

        let stats = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(stats.swept_terminal, 1);

        let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
        let terminal_message: String = conn
            .query_row(
                "SELECT terminal_message FROM videos WHERE video_id='v_wide'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(terminal_message.len() <= 500);
        assert!(terminal_message.starts_with("[sweep] ERROR: Your IP address is blocked"));
    }

    /// Review fix (T07): a fallback classification must not overwrite a
    /// real stored kind (e.g. ToolTimeout — a non-fetch failure whose
    /// message the fetch-stderr table never matches).
    #[test]
    fn sweep_preserves_real_kind_on_fallback_classification() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();
        seed_parked(
            &mut store,
            &tmp,
            "v_timeout",
            "ToolTimeout",
            "some tool explosion the table has never seen",
            1,
        );
        let stats = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(stats.requeued_for_retry, 1);
        let row = store.get_video_for_test("v_timeout").unwrap().unwrap();
        assert_eq!(row.status, "pending");
        assert_eq!(
            row.last_retryable_kind.as_deref(),
            Some("ToolTimeout"),
            "fallback hit must preserve the real stored kind"
        );
    }

    /// Review fix (T07): the legacy placeholder kind "Fetch" (and empty
    /// kinds) still normalize to the fallback label on a fallback hit —
    /// the cookie gate reads the kind at claim time and must never see
    /// the placeholder survive a sweep requeue.
    #[test]
    fn sweep_normalizes_placeholder_kind_on_fallback() {
        let tmp = TempDir::new().unwrap();
        let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
        let table = ClassificationTable::compiled_default().unwrap();
        seed_parked(
            &mut store,
            &tmp,
            "v_legacy",
            "Fetch",
            "some tool explosion the table has never seen",
            1,
        );
        seed_parked(
            &mut store,
            &tmp,
            "v_emptykind",
            "",
            "another never-seen message",
            1,
        );
        let stats = run_sweep(&mut store, &table, 1, false).unwrap();
        assert_eq!(stats.requeued_for_retry, 2);
        for id in ["v_legacy", "v_emptykind"] {
            let row = store.get_video_for_test(id).unwrap().unwrap();
            assert_eq!(row.status, "pending");
            assert_eq!(
                row.last_retryable_kind.as_deref(),
                Some("YtDlpOther"),
                "placeholder/empty kind must normalize to the fallback label ({id})"
            );
        }
    }

    #[test]
    fn census_serializes_and_displays() {
        let by_label = std::collections::BTreeMap::from([("IpBlockedMessage".to_string(), 2usize)]);
        let census = BatchCensus {
            sweep: SweepStats {
                examined: 5,
                swept_terminal: 2,
                swept_terminal_by_label: by_label.clone(),
                requeued_for_retry: 1,
                parked_for_cookies: 1,
                kept_capped: 1,
            },
            run: RunCensus {
                claimed: 3,
                succeeded: 2,
                failed: 1,
                requeued_for_retry: 1,
                exhausted_retries: 0,
                parked_for_cookies: 0,
                terminal_by_label: std::collections::BTreeMap::new(),
                stale_after_success: 0,
                stale_after_failure: 0,
                checkpoints_run: 0,
                checkpoints_failed: 0,
                breaker_tripped: false,
            },
        };
        let json = serde_json::to_string(&census).unwrap();
        assert!(json.contains("\"swept_terminal\":2"));
        assert!(json.contains("\"IpBlockedMessage\":2"));
        // Final-review Finding 3 (ADR-0050 DB-visibility requirement): pin
        // that `breaker_tripped` actually lands in persisted census_json,
        // not just the in-memory struct.
        assert!(json.contains("\"breaker_tripped\":false"));
        let text = census.to_string();
        assert!(text.contains("swept_terminal"));
        assert!(text.contains("IpBlockedMessage"));
        assert!(text.contains("examined"));
    }

    #[test]
    fn aborted_census_json_carries_marker_error_and_sweep() {
        let by_label = std::collections::BTreeMap::from([("IpBlockedMessage".to_string(), 2usize)]);
        let sweep = SweepStats {
            examined: 5,
            swept_terminal: 2,
            swept_terminal_by_label: by_label.clone(),
            requeued_for_retry: 1,
            parked_for_cookies: 1,
            kept_capped: 1,
        };
        let json = aborted_census_json(&sweep, "fetch→transcribe channel closed").unwrap();
        assert!(json.contains("\"aborted\":true"));
        assert!(json.contains("channel closed"));
        assert!(json.contains("\"sweep\""));

        // The success-path census must NOT gain the marker (guard against
        // the marker leaking into BatchCensus itself).
        let sweep_for_census = SweepStats {
            examined: 5,
            swept_terminal: 2,
            swept_terminal_by_label: by_label,
            requeued_for_retry: 1,
            parked_for_cookies: 1,
            kept_capped: 1,
        };
        let census = BatchCensus {
            sweep: sweep_for_census,
            run: RunCensus {
                claimed: 3,
                succeeded: 2,
                failed: 1,
                requeued_for_retry: 1,
                exhausted_retries: 0,
                parked_for_cookies: 0,
                terminal_by_label: std::collections::BTreeMap::new(),
                stale_after_success: 0,
                stale_after_failure: 0,
                checkpoints_run: 0,
                checkpoints_failed: 0,
                breaker_tripped: false,
            },
        };
        assert!(!serde_json::to_string(&census).unwrap().contains("aborted"));
    }
}
