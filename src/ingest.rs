use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;

use rusqlite::Transaction;

use crate::canonical::{canonicalize_url, Canonical};
use crate::state::{backfill_watch_raw_tx, upsert_video_tx, upsert_watch_history_tx, Store};

#[derive(Debug, Default, Clone)]
pub struct IngestStats {
    pub files_processed: usize,
    pub unique_videos_seen: usize,
    pub watch_history_rows_processed: usize,
    pub watch_history_duplicates: usize,
    pub short_links_skipped: usize,
    pub invalid_urls_skipped: usize,
    pub date_parse_failures: usize,
    /// Entries this pass computed as outside the supplied window, per the
    /// input-side counting convention (ADR-0007): incremented whenever a
    /// row's freshly computed `in_window` is false, regardless of whether
    /// that value was actually written. Duplicate-PK rows are computed here
    /// but their stored `in_window` is deliberately left untouched (only
    /// `recompute-window` may change it after ingest) — so this counter can
    /// legitimately exceed the number of rows whose `in_window` flag
    /// actually changed.
    pub computed_out_of_window: usize,
    /// Existing rows whose NULL watched_at_raw this pass backfilled.
    pub backfilled_raw_dates: usize,
}

/// Analysis-window bounds in unix seconds, derived from inclusive UTC
/// calendar dates (Epic 4b window ADR). `Default` = no filter (everything
/// in-window) — matches pre-4b behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowBounds {
    /// Inclusive: 00:00:00Z of --window-start.
    pub start: Option<i64>,
    /// Exclusive: 00:00:00Z of the day AFTER --window-end (an inclusive
    /// calendar end date covers its whole day).
    pub end_exclusive: Option<i64>,
}

impl WindowBounds {
    pub fn from_dates(start: Option<chrono::NaiveDate>, end: Option<chrono::NaiveDate>) -> Self {
        let to_ts = |d: chrono::NaiveDate| {
            Utc.from_utc_datetime(&d.and_time(chrono::NaiveTime::MIN))
                .timestamp()
        };
        WindowBounds {
            start: start.map(to_ts),
            // succ_opt is None only at NaiveDate::MAX — saturate to "no
            // upper bound reachable" rather than wrap.
            end_exclusive: end.map(|d| d.succ_opt().map_or(i64::MAX, to_ts)),
        }
    }

    pub fn contains(&self, ts: i64) -> bool {
        self.start.is_none_or(|s| ts >= s) && self.end_exclusive.is_none_or(|e| ts < e)
    }
}

/// Walk `inbox`, parse each `*.json` file, and upsert resolvable rows into the
/// store. Plan A skips short links with a WARN log; Plan C writes them to a
/// pending_resolutions table.
///
/// Counters are input-side: `unique_videos_seen` and
/// `watch_history_rows_processed` reflect what the ingest pass observed in the
/// input, not what the database newly accepted. `watch_history_duplicates` is
/// the subset of processed rows where the upsert was a no-op (existing PK).
pub fn ingest(inbox: &Path, store: &mut Store, window: WindowBounds) -> Result<IngestStats> {
    let mut stats = IngestStats::default();
    let mut unique_videos: HashSet<String> = HashSet::new();

    // Batch writes per file: the read + JSON parse happen OUTSIDE the write
    // transaction, then one transaction wraps that file's row upserts and commits
    // before the next file. This keeps the SQLite write-lock window off filesystem
    // I/O, lets partial progress survive a later malformed file, and still amortizes
    // the per-row commit cost. `prepare_cached` lives on the connection, so the two
    // statements stay prepared across files. INSERT-OR-IGNORE keeps re-runs idempotent.
    for path in walk_json_files(inbox)? {
        let respondent_id = parse_respondent_id_from_filename(&path)
            .with_context(|| format!("parsing respondent_id from {}", path.display()))?;

        let raw = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let sections: Vec<Section> = serde_json::from_slice(&raw)
            .with_context(|| format!("parsing JSON from {}", path.display()))?;

        let tx = store.transaction()?;
        for section in sections {
            if let Some(rows) = section.tiktok_watch_history {
                for entry in rows {
                    process_watch_entry(
                        &tx,
                        &respondent_id,
                        &entry,
                        &mut stats,
                        &mut unique_videos,
                        window,
                    )?;
                }
            }
        }
        tx.commit()
            .with_context(|| format!("committing ingest transaction for {}", path.display()))?;

        stats.files_processed += 1;
    }

    stats.unique_videos_seen = unique_videos.len();
    Ok(stats)
}

fn process_watch_entry(
    tx: &Transaction<'_>,
    respondent_id: &str,
    entry: &WatchEntry,
    stats: &mut IngestStats,
    unique_videos: &mut HashSet<String>,
    window: WindowBounds,
) -> Result<()> {
    let canon = canonicalize_url(&entry.link);
    let video_id = match canon {
        Canonical::VideoId(id) => id,
        Canonical::NeedsResolution(_) => {
            tracing::warn!(
                respondent = respondent_id,
                url = entry.link.as_str(),
                "short link skipped (Plan C will resolve)"
            );
            stats.short_links_skipped += 1;
            return Ok(());
        }
        Canonical::Invalid(_) => {
            tracing::warn!(
                respondent = respondent_id,
                url = entry.link.as_str(),
                "invalid URL skipped"
            );
            stats.invalid_urls_skipped += 1;
            return Ok(());
        }
    };

    let Some(watched_at) = parse_watched_at(&entry.date) else {
        tracing::warn!(
            respondent = respondent_id,
            date = entry.date.as_str(),
            "could not parse Date; skipping row"
        );
        stats.date_parse_failures += 1;
        return Ok(());
    };

    unique_videos.insert(video_id.clone());
    upsert_video_tx(tx, &video_id, &entry.link, true)?;

    let in_window = window.contains(watched_at);
    if !in_window {
        stats.computed_out_of_window += 1;
    }
    let inserted = upsert_watch_history_tx(
        tx,
        respondent_id,
        &video_id,
        watched_at,
        &entry.date,
        in_window,
    )?;
    stats.watch_history_rows_processed += 1;
    if inserted == 0 {
        stats.watch_history_duplicates += 1;
        // Pre-v4 rows carry NULL watched_at_raw; re-ingest is the designed
        // backfill path. in_window is deliberately untouched here.
        stats.backfilled_raw_dates +=
            backfill_watch_raw_tx(tx, respondent_id, &video_id, watched_at, &entry.date)?;
    }
    Ok(())
}

fn walk_json_files(inbox: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_recursive(inbox, &mut out)?;
    Ok(out)
}

fn walk_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_recursive(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

/// Filename convention: `assignment={N}_task={N}_participant={ID}_source=tiktok_key={N}-tiktok.json`
/// Returns the value of `participant=`.
pub fn parse_respondent_id_from_filename(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("path {} has no filename", path.display()))?;

    for segment in stem.split('_') {
        if let Some(rest) = segment.strip_prefix("participant=") {
            return Ok(rest.to_string());
        }
    }

    anyhow::bail!("filename {stem} does not contain a `participant=` segment")
}

#[derive(Debug, Deserialize)]
struct Section {
    #[serde(default)]
    tiktok_watch_history: Option<Vec<WatchEntry>>,
}

#[derive(Debug, Deserialize)]
struct WatchEntry {
    #[serde(rename = "Date")]
    date: String,
    #[serde(rename = "Link")]
    link: String,
}

/// Parse a DDP `Date` string into a unix timestamp, interpreting the naive
/// value as UTC per ADR-0039 — a documentary-evidence verdict, not an
/// empirically confirmed one. TikTok's May-2026 export pipeline labels these
/// strings with a literal " UTC" suffix; an operator spot-check against known
/// watch moments could not discriminate UTC from local time at ±1h precision,
/// so the convention is recorded as UTC-assumed, empirically unresolved.
/// July-2026 exports dropped the suffix but — by pipeline continuity, not
/// independent evidence — are assumed to keep the same convention. The raw
/// string is preserved in watch_history.watched_at_raw (schema v4) so a
/// future reinterpretation never requires re-ingest.
fn parse_watched_at(s: &str) -> Option<i64> {
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S", // production TikTok DDP (July 2026 real-donor exports, unlabeled) + synthetic fixtures
        "%Y-%m-%d %H:%M:%S UTC", // production TikTok DDP (May 2026 PI bake, UTC-labeled)
    ];
    for fmt in FORMATS {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Utc.from_utc_datetime(&naive).timestamp());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_respondent_from_realistic_filename() {
        let path = PathBuf::from(
            "/x/assignment=500_task=1221_participant=preview_source=tiktok_key=1776350251592-tiktok.json",
        );
        let id = parse_respondent_id_from_filename(&path).unwrap();
        assert_eq!(id, "preview");
    }

    #[test]
    fn parse_respondent_errors_when_segment_missing() {
        let path = PathBuf::from("/x/no-segments.json");
        assert!(parse_respondent_id_from_filename(&path).is_err());
    }

    #[test]
    fn parse_watched_at_handles_standard_format() {
        assert!(parse_watched_at("2026-02-03 13:20:15").is_some());
    }

    #[test]
    fn parse_watched_at_returns_none_on_garbage() {
        assert!(parse_watched_at("not a date").is_none());
    }

    #[test]
    fn parse_watched_at_handles_utc_suffix() {
        assert!(parse_watched_at("2024-01-01 12:00:00 UTC").is_some());
    }

    #[test]
    fn parse_watched_at_returns_none_on_garbage_with_partial_utc() {
        assert!(parse_watched_at("not a date UTC").is_none());
        assert!(parse_watched_at("UTC").is_none());
    }

    #[test]
    fn window_bounds_inclusive_dates() {
        let d = |y, m, dd| chrono::NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        let w = WindowBounds::from_dates(Some(d(2026, 2, 1)), Some(d(2026, 2, 28)));
        let ts = |s: &str| parse_watched_at(s).unwrap();
        assert!(
            w.contains(ts("2026-02-01 00:00:00")),
            "start midnight inclusive"
        );
        assert!(
            w.contains(ts("2026-02-28 23:59:59")),
            "end date inclusive through its last second"
        );
        assert!(!w.contains(ts("2026-01-31 23:59:59")));
        assert!(
            !w.contains(ts("2026-03-01 00:00:00")),
            "day after end excluded"
        );
        assert!(
            WindowBounds::default().contains(ts("1999-01-01 00:00:00")),
            "no flags = everything in window"
        );
        let start_only = WindowBounds::from_dates(Some(d(2026, 2, 1)), None);
        assert!(start_only.contains(ts("2030-01-01 00:00:00")));
        assert!(!start_only.contains(ts("2026-01-31 23:59:59")));
    }
}
