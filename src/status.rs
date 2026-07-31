//! Operator-facing `status` subcommand (Plan B Epic 4b): read-only report
//! over the state DB. Bare `status` is DB-only and cheap; the archived
//! ADR-0017 done-contract checks (disk + artifact parse) live behind
//! `--verify` (Task 04). Rendering policy lives here; SQL lives in
//! `state::queries`.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use serde::Serialize;

use crate::state::queries::{
    BatchRunRow, RespondentSummary, TerminalRow, VideoDetailRow, VideoEventRow,
};
use crate::state::{ParkedRow, Store};

/// The fixed status vocabulary (matches the schema CHECK constraint),
/// in lifecycle order for rendering.
pub(crate) const STATUSES: [&str; 5] = [
    "pending",
    "in_progress",
    "succeeded",
    "failed_terminal",
    "failed_retryable",
];

#[derive(Debug, Serialize)]
pub(crate) struct StatusReport {
    pub total_videos: i64,
    /// Zero-filled over [`STATUSES`].
    pub counts: BTreeMap<String, i64>,
    /// Raw stored kinds — including the legacy placeholder "Fetch". The
    /// human renderer annotates it; JSON consumers get stored truth.
    pub retryable_by_kind: BTreeMap<String, i64>,
    pub in_progress: Vec<InProgressAge>,
    pub batch_runs: Vec<BatchRunSummary>,
    /// Present only under --verify (Task 04): the 0017 done-contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyReport>,
}

#[derive(Debug, Serialize)]
pub(crate) struct InProgressAge {
    pub video_id: String,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<i64>,
    /// now − claimed_at; None when claimed_at is NULL (malformed row —
    /// rendered as unknown, never a crash).
    pub age_s: Option<i64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct BatchRunSummary {
    pub run_id: i64,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// `finished_at IS NULL`: the run crashed or was interrupted before
    /// close. Its census is permanently unrecorded; outcomes remain
    /// reconstructable from the videos table (kind survives recovery).
    pub interrupted: bool,
    /// Parsed params_json, or the raw string wrapped in a JSON string if
    /// unparseable (render something, never fail the report).
    pub params: serde_json::Value,
    pub policy: PolicyProvenance,
    /// Headline numbers pulled from census_json; None when the run is
    /// interrupted (no census) or the JSON is unreadable.
    pub census_headline: Option<CensusHeadline>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PolicyProvenance {
    pub bytes: usize,
    /// True iff policy_toml is byte-identical to THIS binary's compiled
    /// default. A binary upgrade can flip this for historical rows; that
    /// is honest — provenance is relative to the reading binary.
    pub compiled_default: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct CensusHeadline {
    pub sweep_examined: Option<u64>,
    pub claimed: Option<u64>,
    pub succeeded: Option<u64>,
    pub failed: Option<u64>,
}

pub(crate) fn build_report(store: &Store, now: i64) -> Result<StatusReport> {
    let raw_counts = store.count_by_status().context("counting by status")?;
    let mut counts = BTreeMap::new();
    for s in STATUSES {
        counts.insert(s.to_string(), raw_counts.get(s).copied().unwrap_or(0));
    }
    // A value outside the CHECK vocabulary can't normally exist; if one
    // does (hand-edited DB), surface it rather than hiding it.
    for (k, v) in &raw_counts {
        counts.entry(k.clone()).or_insert(*v);
    }
    let total_videos = counts.values().sum();

    let retryable_by_kind = store
        .count_retryable_by_kind()
        .context("counting retryable by kind")?;

    let in_progress = store
        .list_in_progress()
        .context("listing in_progress rows")?
        .into_iter()
        .map(|r| InProgressAge {
            age_s: r.claimed_at.map(|c| now.saturating_sub(c)),
            video_id: r.video_id,
            claimed_by: r.claimed_by,
            claimed_at: r.claimed_at,
        })
        .collect();

    let compiled_default_toml = crate::classification::ClassificationTable::compiled_default()
        .ok()
        .map(|t| t.source_toml().to_string());
    let batch_runs = store
        .list_batch_runs()
        .context("listing batch runs")?
        .into_iter()
        .map(|r| summarize_run(r, compiled_default_toml.as_deref()))
        .collect();

    Ok(StatusReport {
        total_videos,
        counts,
        retryable_by_kind,
        in_progress,
        batch_runs,
        verify: None,
    })
}

fn summarize_run(row: BatchRunRow, compiled_default_toml: Option<&str>) -> BatchRunSummary {
    let params = serde_json::from_str(&row.params_json)
        .unwrap_or_else(|_| serde_json::Value::String(row.params_json.clone()));
    let census_headline = row.census_json.as_deref().and_then(|c| {
        serde_json::from_str::<serde_json::Value>(c)
            .ok()
            .map(|v| CensusHeadline {
                sweep_examined: v["sweep"]["examined"].as_u64(),
                claimed: v["run"]["claimed"].as_u64(),
                succeeded: v["run"]["succeeded"].as_u64(),
                failed: v["run"]["failed"].as_u64(),
            })
    });
    BatchRunSummary {
        run_id: row.run_id,
        started_at: row.started_at,
        interrupted: row.finished_at.is_none(),
        finished_at: row.finished_at,
        params,
        policy: PolicyProvenance {
            bytes: row.policy_toml.len(),
            compiled_default: compiled_default_toml == Some(row.policy_toml.as_str()),
        },
        census_headline,
    }
}

/// The archived ADR-0017 done-contract, mechanised. Sample vectors cap at
/// [`VERIFY_SAMPLE_CAP`] ids so a catastrophically wrong tree doesn't blow
/// up the report; counts are always complete.
pub(crate) const VERIFY_SAMPLE_CAP: usize = 20;

#[derive(Debug, Serialize)]
pub(crate) struct VerifyReport {
    pub succeeded_rows: usize,
    /// Rows missing `.txt` or `.json` at the sharded path.
    pub artifacts_missing: usize,
    /// Rows whose `.json` parsed but `raw_signals.schema_version` differs
    /// from EXPECTED_RAW_SIGNALS_SCHEMA_VERSION (or raw_signals is absent).
    pub schema_version_mismatches: usize,
    /// Rows whose `.json` could not be read/parsed, or whose shard's
    /// `read_dir` hit an infra fault (permissions, broken mount, ...) so
    /// artifact existence is unknowable — could not be verified due to an
    /// I/O or parse fault, distinct from `artifacts_missing` (definitively
    /// absent).
    pub unreadable_artifacts: usize,
    pub pending: i64,
    pub in_progress: i64,
    /// 0017 + 0011: everything terminal, all artifacts present and
    /// schema-valid, nothing awaiting recovery → safe to spin down.
    pub pause_safe: bool,
    pub sample_missing: Vec<String>,
    pub sample_mismatched: Vec<String>,
    pub sample_unreadable: Vec<String>,
}

pub(crate) fn run_verify(
    store: &Store,
    transcripts_root: &Path,
    counts: &BTreeMap<String, i64>,
) -> Result<VerifyReport> {
    let ids = store
        .list_succeeded_ids()
        .context("listing succeeded ids")?;

    // Brainstorm-note batching: group ids per shard, ONE read_dir per
    // shard into a filename set, then set lookups. Never per-row stat.
    let mut by_shard: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in &ids {
        by_shard
            .entry(crate::output::shard(id))
            .or_default()
            .push(id);
    }

    let mut report = VerifyReport {
        succeeded_rows: ids.len(),
        artifacts_missing: 0,
        schema_version_mismatches: 0,
        unreadable_artifacts: 0,
        pending: counts.get("pending").copied().unwrap_or(0),
        in_progress: counts.get("in_progress").copied().unwrap_or(0),
        pause_safe: false,
        sample_missing: Vec::new(),
        sample_mismatched: Vec::new(),
        sample_unreadable: Vec::new(),
    };

    for (shard, shard_ids) in by_shard {
        let dir = transcripts_root.join(shard);
        let names: HashSet<OsString> = match read_shard_filenames(&dir) {
            Ok(names) => names,
            Err(_) => {
                for id in &shard_ids {
                    report.unreadable_artifacts += 1;
                    push_capped(&mut report.sample_unreadable, id);
                }
                continue;
            }
        };
        for id in shard_ids {
            let txt = OsString::from(format!("{id}.txt"));
            let json = OsString::from(format!("{id}.json"));
            if !(names.contains(&txt) && names.contains(&json)) {
                report.artifacts_missing += 1;
                push_capped(&mut report.sample_missing, id);
                continue;
            }
            match std::fs::read(dir.join(format!("{id}.json"))) {
                Ok(bytes) => match serde_json::from_slice::<
                    crate::output::artifacts::TranscriptMetadata,
                >(&bytes)
                {
                    Ok(meta) => {
                        let ok = meta.raw_signals.as_ref().is_some_and(|rs| {
                            rs.schema_version
                                == crate::output::artifacts::EXPECTED_RAW_SIGNALS_SCHEMA_VERSION
                        });
                        if !ok {
                            report.schema_version_mismatches += 1;
                            push_capped(&mut report.sample_mismatched, id);
                        }
                    }
                    Err(_) => {
                        report.unreadable_artifacts += 1;
                        push_capped(&mut report.sample_unreadable, id);
                    }
                },
                Err(_) => {
                    report.unreadable_artifacts += 1;
                    push_capped(&mut report.sample_unreadable, id);
                }
            }
        }
    }

    report.pause_safe = report.pending == 0
        && report.in_progress == 0
        && report.artifacts_missing == 0
        && report.schema_version_mismatches == 0
        && report.unreadable_artifacts == 0;
    Ok(report)
}

/// One `read_dir` per shard into a filename set.
///
/// An absent shard dir is `Ok(empty)` → every row in it counts missing,
/// which is honest when `status` runs away from the artifacts volume. Any
/// OTHER failure (permissions, broken mount, not-a-directory) is an infra
/// fault on a present tree: counting those rows "missing" would steer the
/// operator toward re-transcription instead of the mount, so the caller
/// counts them unreadable instead.
fn read_shard_filenames(dir: &Path) -> std::io::Result<HashSet<OsString>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(e),
    };
    collect_shard_filenames(entries.map(|e| e.map(|e| e.file_name())))
}

/// The entry-collecting half of [`read_shard_filenames`], split out because
/// a failing `DirEntry` cannot be forced portably from a test: an entry
/// that exists but can't be named/stat'd is an infra fault on a present
/// tree, exactly like a failing `read_dir`, and must NOT be dropped from
/// the set (dropping it silently downgrades an unreadable artifact to a
/// missing one).
fn collect_shard_filenames(
    entries: impl IntoIterator<Item = std::io::Result<OsString>>,
) -> std::io::Result<HashSet<OsString>> {
    entries.into_iter().collect()
}

fn push_capped(v: &mut Vec<String>, id: &str) {
    if v.len() < VERIFY_SAMPLE_CAP {
        v.push(id.to_string());
    }
}

/// Render a unix timestamp as "YYYY-MM-DD HH:MM:SSZ". Out-of-range values
/// (hand-edited DBs) render as a marker, never panic.
pub(crate) fn fmt_utc(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0).single().map_or_else(
        || format!("(invalid timestamp {ts})"),
        |dt| dt.format("%Y-%m-%d %H:%M:%SZ").to_string(),
    )
}

fn fmt_age(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

pub(crate) fn render_report(report: &StatusReport) -> String {
    // Writing to a String is infallible; unwraps are forbidden, so route
    // through a helper closure that ignores the Ok(()) results via let _.
    let mut out = String::new();
    let _ = writeln!(out, "videos: {} total", report.total_videos);
    for s in STATUSES {
        let _ = writeln!(
            out,
            "  {:<18} {:>7}",
            s,
            report.counts.get(s).copied().unwrap_or(0)
        );
    }
    for (k, v) in &report.counts {
        if !STATUSES.contains(&k.as_str()) {
            let _ = writeln!(out, "  {k:<18} {v:>7}  (outside the status vocabulary!)");
        }
    }

    if !report.retryable_by_kind.is_empty() {
        let _ = writeln!(out, "failed_retryable by kind:");
        // Count DESC, then name — the operator reads the big pools first.
        let mut kinds: Vec<(&String, &i64)> = report.retryable_by_kind.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (kind, n) in kinds {
            let note = if kind == "Fetch" {
                "  (legacy placeholder kind)"
            } else {
                ""
            };
            let _ = writeln!(out, "  {kind:<24} {n:>7}{note}");
        }
    }

    if report.in_progress.is_empty() {
        let _ = writeln!(out, "in_progress claims: none");
    } else {
        let _ = writeln!(
            out,
            "in_progress claims ({}): (rows older than the stale threshold — default 30m — are re-queued by the next process run's sweep)",
            report.in_progress.len()
        );
        for r in &report.in_progress {
            let _ = writeln!(
                out,
                "  {}  claimed_by {}  age {}  (claimed {})",
                r.video_id,
                r.claimed_by.as_deref().unwrap_or("(unknown)"),
                r.age_s.map_or_else(|| "(unknown)".to_string(), fmt_age),
                r.claimed_at
                    .map_or_else(|| "(unknown)".to_string(), fmt_utc),
            );
        }
    }

    let _ = writeln!(out, "batch runs ({}):", report.batch_runs.len());
    for run in &report.batch_runs {
        let policy = if run.policy.compiled_default {
            format!("compiled default ({} B)", run.policy.bytes)
        } else {
            format!("custom ({} B)", run.policy.bytes)
        };
        let params = render_params(&run.params);
        match run.finished_at {
            Some(fin) => {
                let _ = writeln!(
                    out,
                    "  run {}  started {}  finished {}  {}  policy: {}",
                    run.run_id,
                    fmt_utc(run.started_at),
                    fmt_utc(fin),
                    params,
                    policy
                );
                match &run.census_headline {
                    Some(c) => {
                        let _ = writeln!(
                            out,
                            "         census: sweep examined {}, claimed {}, succeeded {}, failed {}",
                            c.sweep_examined.map_or_else(|| "?".into(), |v: u64| v.to_string()),
                            c.claimed.map_or_else(|| "?".into(), |v: u64| v.to_string()),
                            c.succeeded.map_or_else(|| "?".into(), |v: u64| v.to_string()),
                            c.failed.map_or_else(|| "?".into(), |v: u64| v.to_string()),
                        );
                    }
                    None => {
                        let _ = writeln!(
                            out,
                            "         census: unreadable (closed run with unparseable census_json)"
                        );
                    }
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "  run {}  started {}  INTERRUPTED (never closed; no census — outcomes remain reconstructable from the videos table)  {}  policy: {}",
                    run.run_id, fmt_utc(run.started_at), params, policy
                );
            }
        }
    }

    if let Some(v) = &report.verify {
        let _ = writeln!(out, "done-contract (0017) --verify:");
        let _ = writeln!(out, "  succeeded rows            {:>7}", v.succeeded_rows);
        let _ = writeln!(
            out,
            "  artifacts missing         {:>7}",
            v.artifacts_missing
        );
        let _ = writeln!(
            out,
            "  schema_version mismatches {:>7}",
            v.schema_version_mismatches
        );
        let _ = writeln!(
            out,
            "  unreadable artifacts      {:>7}",
            v.unreadable_artifacts
        );
        let _ = writeln!(
            out,
            "  pending {}  in_progress {}  (pending may be deliberate under --max-videos)",
            v.pending, v.in_progress
        );
        for (label, sample) in [
            ("missing", &v.sample_missing),
            ("mismatched", &v.sample_mismatched),
            ("unreadable", &v.sample_unreadable),
        ] {
            if !sample.is_empty() {
                let _ = writeln!(
                    out,
                    "  first {} {label}: {}",
                    sample.len(),
                    sample.join(", ")
                );
            }
        }
        let _ = writeln!(
            out,
            "  pause-safe: {}",
            if v.pause_safe {
                "YES — safe to spin down (0011)"
            } else {
                "NO"
            }
        );
    }

    out
}

/// Compact one-line params summary: the fields operators actually ask
/// about. Unknown/unparseable params render as "params: <raw>".
fn render_params(params: &serde_json::Value) -> String {
    if let Some(obj) = params.as_object() {
        let get = |k: &str| obj.get(k).map(std::string::ToString::to_string);
        let mut parts = Vec::new();
        if let Some(v) = get("retries") {
            parts.push(format!("retries={v}"));
        }
        if let Some(v) = get("download_workers") {
            parts.push(format!("workers={v}"));
        }
        if let Some(v) = obj
            .get("cookies_present")
            .and_then(serde_json::Value::as_bool)
        {
            parts.push(format!("cookies={}", if v { "yes" } else { "no" }));
        }
        if let Some(v) = obj.get("max_videos").filter(|v| !v.is_null()) {
            parts.push(format!("max_videos={v}"));
        }
        if parts.is_empty() {
            format!("params: {params}")
        } else {
            parts.join(" ")
        }
    } else {
        format!("params: {params}")
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct VideoDetailReport {
    pub video: VideoDetailRow,
    pub events: Vec<VideoEventRow>,
}

pub(crate) fn build_video_detail(store: &Store, video_id: &str) -> Result<VideoDetailReport> {
    let video = store
        .get_video_detail(video_id)
        .context("loading video row")?
        .with_context(|| format!("video {video_id} not found in the state DB"))?;
    let events = store
        .list_video_events(video_id)
        .context("loading video events")?;
    Ok(VideoDetailReport { video, events })
}

pub(crate) fn render_video_detail(r: &VideoDetailReport) -> String {
    let mut out = String::new();
    let v = &r.video;
    let _ = writeln!(out, "video {}", v.video_id);
    let _ = writeln!(out, "  url        {}", v.source_url);
    let _ = writeln!(
        out,
        "  status     {}  attempts {}",
        v.status, v.attempt_count
    );
    let _ = writeln!(
        out,
        "  first_seen {}  updated {}",
        fmt_utc(v.first_seen_at),
        fmt_utc(v.updated_at)
    );
    if let Some(at) = v.succeeded_at {
        let _ = writeln!(
            out,
            "  succeeded  {}  duration_s {}  language {}  fetcher {}  source {}",
            fmt_utc(at),
            v.duration_s
                .map_or_else(|| "?".into(), |d| format!("{d:.1}")),
            v.language_detected.as_deref().unwrap_or("?"),
            v.fetcher.as_deref().unwrap_or("?"),
            v.transcript_source.as_deref().unwrap_or("?"),
        );
    }
    if let (Some(by), Some(at)) = (&v.claimed_by, v.claimed_at) {
        let _ = writeln!(out, "  claimed_by {by}  claimed_at {}", fmt_utc(at));
    }
    if let Some(kind) = &v.last_retryable_kind {
        let note = if kind == "Fetch" {
            "  (legacy placeholder kind)"
        } else {
            ""
        };
        let _ = writeln!(out, "  last_retryable_kind {kind}{note}");
        if let Some(msg) = &v.last_retryable_message {
            let _ = writeln!(out, "    message: {}", excerpt(msg));
        }
    }
    if let Some(reason) = &v.terminal_reason {
        let _ = writeln!(out, "  terminal_reason {reason}");
        if let Some(msg) = &v.terminal_message {
            let _ = writeln!(out, "    message: {}", excerpt(msg));
        }
    }
    let _ = writeln!(out, "  events ({}):", r.events.len());
    for e in &r.events {
        let _ = write!(
            out,
            "    {}  {:<16} worker {}",
            fmt_utc(e.at),
            e.event_type,
            e.worker_id.as_deref().unwrap_or("-")
        );
        let _ = writeln!(
            out,
            "{}",
            render_event_detail_inline(e.detail_json.as_deref())
        );
        if let Some(msg) = detail_message(e.detail_json.as_deref()) {
            let _ = writeln!(out, "        message: {}", excerpt(&msg));
        }
    }
    out
}

/// Inline key=value rendering of the known detail_json shapes
/// ({"kind","message"[,"policy"]}, {"reason","message"}, {"new_kind"}).
/// `message` is excluded here (rendered on its own line). Unknown shapes —
/// and known keys carrying unexpected value types — fall back to the raw
/// JSON so nothing is hidden.
fn render_event_detail_inline(detail: Option<&str>) -> String {
    let Some(raw) = detail else {
        return String::new();
    };
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return format!("  detail: {raw}");
    };
    let known = ["kind", "policy", "new_kind", "reason"];
    // A known key whose value is not a string can't render as `k=v`. The
    // `as_str` filter below would silently DROP it — an unexpected writer
    // (or a hand-edited row) would render as nothing at all. Fall back to
    // the raw JSON instead: the same treatment unknown shapes already get.
    if known
        .iter()
        .any(|k| obj.get(*k).is_some_and(|v| !v.is_string()))
    {
        return format!("  detail: {raw}");
    }
    let mut parts: Vec<String> = known
        .iter()
        .filter_map(|k| {
            obj.get(*k)
                .and_then(serde_json::Value::as_str)
                .map(|v| format!("{k}={v}"))
        })
        .collect();
    let unknown: Vec<&String> = obj
        .keys()
        .filter(|k| !known.contains(&k.as_str()) && *k != "message")
        .collect();
    if !unknown.is_empty() {
        parts.push(format!("(+{} more field(s): see --json)", unknown.len()));
    }
    if parts.is_empty() && !obj.contains_key("message") {
        return format!("  detail: {raw}");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  {}", parts.join(" "))
    }
}

fn detail_message(detail: Option<&str>) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(detail?).ok()?;
    v["message"].as_str().map(std::string::ToString::to_string)
}

/// 200-byte char-boundary-safe excerpt for stored yt-dlp/TikTok text
/// (localized text panics a naive truncate — same hazard as the sweep's).
fn excerpt(s: &str) -> String {
    let mut owned = s.to_string();
    crate::batch::truncate_to_char_boundary(&mut owned, 200);
    if owned.len() < s.len() {
        owned.push('…');
    }
    owned
}

#[derive(Debug, Serialize)]
pub(crate) struct RespondentReport {
    pub respondent: RespondentSummary,
}

/// Mirrors [`build_video_detail`]: an id the DB has never seen is an
/// operator error, not a report of zeroes. Without the existence gate a
/// typo'd `--respondent-id` renders an all-zeros summary that reads as
/// "this respondent donated nothing" — the one answer the operator must
/// never get wrong.
pub(crate) fn build_respondent_report(
    store: &Store,
    respondent_id: &str,
) -> Result<RespondentReport> {
    if !store
        .respondent_is_known(respondent_id)
        .context("checking respondent existence")?
    {
        anyhow::bail!("respondent {respondent_id} not found in the state DB");
    }
    Ok(RespondentReport {
        respondent: store
            .respondent_summary(respondent_id)
            .context("respondent summary")?,
    })
}

pub(crate) fn render_respondent(r: &RespondentReport) -> String {
    let s = &r.respondent;
    let mut out = String::new();
    let _ = writeln!(out, "respondent {}", s.respondent_id);
    let _ = writeln!(out, "  watch_events            {:>7}", s.watch_events);
    let _ = writeln!(out, "  videos_seen             {:>7}", s.videos_seen);
    let _ = writeln!(out, "  videos_in_window        {:>7}", s.videos_in_window);
    let _ = writeln!(out, "  videos_succeeded        {:>7}", s.videos_succeeded);
    let _ = writeln!(
        out,
        "  videos_failed_terminal  {:>7}",
        s.videos_failed_terminal
    );
    let _ = writeln!(
        out,
        "  videos_failed_retryable {:>7}",
        s.videos_failed_retryable
    );
    let _ = writeln!(out, "  videos_pending          {:>7}", s.videos_pending);
    let _ = writeln!(out, "  videos_in_progress      {:>7}", s.videos_in_progress);
    out
}

#[derive(Debug, Serialize)]
pub(crate) struct FailureLists {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<TerminalRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<Vec<ParkedRow>>,
}

pub(crate) fn render_failure_lists(l: &FailureLists) -> String {
    let mut out = String::new();
    if let Some(errors) = &l.errors {
        let _ = writeln!(out, "failed_terminal ({}):", errors.len());
        for e in errors {
            let _ = writeln!(
                out,
                "  {}  {}  (updated {})",
                e.video_id,
                e.terminal_reason.as_deref().unwrap_or("(none)"),
                fmt_utc(e.updated_at),
            );
            if let Some(msg) = &e.terminal_message {
                let _ = writeln!(out, "      message: {}", excerpt(msg));
            }
        }
    }
    if let Some(retryable) = &l.retryable {
        let _ = writeln!(out, "failed_retryable ({}):", retryable.len());
        for r in retryable {
            let kind = r.last_retryable_kind.as_deref().unwrap_or("(none)");
            let note = if kind == "Fetch" {
                "  (legacy placeholder kind)"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  {}  {kind}  attempts {}{note}",
                r.video_id, r.attempt_count
            );
            if let Some(msg) = &r.last_retryable_message {
                let _ = writeln!(out, "      message: {}", excerpt(msg));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shard_filename_collection_surfaces_a_per_entry_error() {
        // Review finding: an entry that exists but can't be named/stat'd
        // must surface as an infra fault, not silently vanish from the
        // shard's filename set (which would miscount its row as
        // `artifacts_missing` and steer the operator at re-transcription
        // instead of the mount). This is the extracted decision point —
        // there is no portable way to make a real `DirEntry` fail.
        let entries = vec![
            Ok(OsString::from("v_ok1.txt")),
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "entry unnameable",
            )),
        ];
        assert!(
            collect_shard_filenames(entries).is_err(),
            "a per-entry error must propagate, not be dropped"
        );
    }

    #[test]
    fn shard_filename_collection_reads_every_name() {
        let entries = vec![
            Ok(OsString::from("v_ok1.txt")),
            Ok(OsString::from("v_ok1.json")),
        ];
        let names = collect_shard_filenames(entries).expect("no entry errors");
        assert_eq!(names.len(), 2);
        assert!(names.contains(&OsString::from("v_ok1.json")));
    }
}
