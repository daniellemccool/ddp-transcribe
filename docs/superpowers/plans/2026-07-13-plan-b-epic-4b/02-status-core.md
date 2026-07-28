# Task 02: `status` core — counts, retryable-by-kind, in-progress ages, honest batch-run history, `--json`

**Files:**
- Create: `src/state/queries.rs` (read-only Store queries + row structs)
- Modify: `src/state/mod.rs` (declare `pub mod queries;`; make `unix_now` `pub(crate)`)
- Create: `src/status.rs` (report assembly + human rendering)
- Modify: `src/main.rs` (add `mod status;` + the `Command::Status` arm)
- Modify: `src/cli.rs` (add `Command::Status` variant)
- Create: `tests/status.rs` (binary-level integration tests; auto-discovered — NO Cargo.toml block, it uses only public API)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces (Tasks 03/04 extend these exact items):
  - `src/state/queries.rs`: `impl Store` methods `count_by_status(&self) -> anyhow::Result<BTreeMap<String, i64>>`, `count_retryable_by_kind(&self) -> anyhow::Result<BTreeMap<String, i64>>`, `list_in_progress(&self) -> anyhow::Result<Vec<InProgressRow>>`, `list_batch_runs(&self) -> anyhow::Result<Vec<BatchRunRow>>`; structs `InProgressRow { video_id: String, claimed_by: Option<String>, claimed_at: Option<i64> }`, `BatchRunRow { run_id: i64, started_at: i64, finished_at: Option<i64>, params_json: String, policy_toml: String, census_json: Option<String> }`.
  - `src/status.rs`: `pub fn build_report(store: &Store, now: i64) -> anyhow::Result<StatusReport>`, `pub fn render_report(report: &StatusReport) -> String`, `pub fn fmt_utc(ts: i64) -> String`, `pub struct StatusReport` (Serialize; fields below).
  - CLI: `Command::Status { json: bool }` (Tasks 03/04 add more flags to this variant).
  - `crate::state::unix_now()` visible as `pub(crate)`.

**Design constraints (from the epic overview — binding):**
- `status` NEVER creates a DB: the main arm bails if `--state-db` doesn't exist (mirror the Migrate arm's message shape).
- Open/interrupted `batch_runs` rows (`finished_at IS NULL`) render honestly as `INTERRUPTED`, never skipped, never a NULL-unwrap crash. The pilot snapshot's run 1 is the real test case.
- Legacy kind `"Fetch"` gets the ` (legacy placeholder kind)` annotation in human output only; JSON carries stored values untouched.
- Policy provenance per run = byte length + equality with the current binary's compiled default TOML (`classification::ClassificationTable::compiled_default()?.source_toml()`); no hashing (no new deps).

- [ ] **Step 1: Write the failing tests**

Create `tests/status.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::str::contains;

/// Build a seeded v-current DB via the public Store::open (schema apply),
/// then raw SQL — same seeding convention as src/batch.rs's tests.
fn seeded_db(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    let db = tmp.path().join("state.sqlite");
    {
        let _store = ddp_transcribe::state::Store::open(&db).expect("open store");
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    let seed_video = |id: &str, status: &str, kind: Option<&str>| {
        conn.execute(
            "INSERT INTO videos (video_id, source_url, canonical, status,
                                 last_retryable_kind, first_seen_at, updated_at,
                                 claimed_by, claimed_at)
             VALUES (?1, ?2, 1, ?3, ?4, 100, 100,
                     CASE WHEN ?3 = 'in_progress' THEN 'w1' END,
                     CASE WHEN ?3 = 'in_progress' THEN 1000 END)",
            rusqlite::params![id, format!("https://example/{id}"), status, kind],
        )
        .unwrap();
    };
    seed_video("v_ok1", "succeeded", None);
    seed_video("v_ok2", "succeeded", None);
    seed_video("v_pend", "pending", None);
    seed_video("v_prog", "in_progress", None);
    seed_video("v_retry1", "failed_retryable", Some("NoPermission"));
    seed_video("v_retry2", "failed_retryable", Some("Fetch"));
    seed_video("v_term", "failed_terminal", None);
    // batch_runs: run 1 interrupted (NULL finished_at + NULL census),
    // run 2 closed with a census.
    conn.execute(
        "INSERT INTO batch_runs (started_at, params_json, policy_toml)
         VALUES (200, '{\"retries\":1,\"download_workers\":3,\"cookies_present\":false}', 'schema = 1')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO batch_runs (started_at, finished_at, params_json, policy_toml, census_json)
         VALUES (300, 400, '{\"retries\":2,\"download_workers\":3,\"cookies_present\":true}', 'schema = 1',
                 '{\"sweep\":{\"examined\":5},\"run\":{\"claimed\":3,\"succeeded\":2,\"failed\":1}}')",
        [],
    )
    .unwrap();
    db
}

#[test]
fn status_renders_counts_kinds_claims_and_runs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(contains("succeeded"))
        .stdout(contains("failed_retryable"))
        .stdout(contains("NoPermission"))
        .stdout(contains("(legacy placeholder kind)"))
        .stdout(contains("v_prog"))
        .stdout(contains("INTERRUPTED"))
        .stdout(contains("run 2"));
}

#[test]
fn status_json_is_parseable_and_correct() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["total_videos"], 7);
    assert_eq!(v["counts"]["succeeded"], 2);
    assert_eq!(v["counts"]["pending"], 1);
    assert_eq!(v["counts"]["in_progress"], 1);
    assert_eq!(v["counts"]["failed_terminal"], 1);
    assert_eq!(v["counts"]["failed_retryable"], 2);
    // JSON carries the RAW stored kind — annotation is human-render only.
    assert_eq!(v["retryable_by_kind"]["Fetch"], 1);
    assert_eq!(v["retryable_by_kind"]["NoPermission"], 1);
    let runs = v["batch_runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["run_id"], 1);
    assert_eq!(runs[0]["interrupted"], true);
    assert!(runs[0]["census_headline"].is_null());
    assert_eq!(runs[1]["interrupted"], false);
    assert_eq!(runs[1]["census_headline"]["claimed"], 3);
    assert_eq!(runs[1]["params"]["retries"], 2);
}

#[test]
fn status_refuses_missing_db_without_creating_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("absent.sqlite");
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status"])
        .assert()
        .failure()
        .stderr(contains("not found"));
    assert!(!db.exists(), "status must not create a missing DB");
}

#[test]
fn status_empty_db_renders_zero_counts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let _store = ddp_transcribe::state::Store::open(&db).expect("open store");
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(contains("0 total"));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test status -- --test-threads=1`
Expected: COMPILE FAILURE in the binary — `status` subcommand does not exist (`Command::Status` unresolved) once wired; first run fails at `cargo_bin` because the CLI rejects `status`.

- [ ] **Step 3: Add the read-only query module**

Create `src/state/queries.rs`:

```rust
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
```

In `src/state/mod.rs`:
1. After `pub mod migrate;` add: `pub mod queries;`
2. Change `fn unix_now()` to `pub(crate) fn unix_now()` (the Status arm passes "now" in from the caller so `build_report` stays deterministic under test).

- [ ] **Step 4: Add the status module**

Create `src/status.rs`:

```rust
//! Operator-facing `status` subcommand (Plan B Epic 4b): read-only report
//! over the state DB. Bare `status` is DB-only and cheap; the archived
//! ADR-0017 done-contract checks (disk + artifact parse) live behind
//! `--verify` (Task 04). Rendering policy lives here; SQL lives in
//! `state::queries`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use serde::Serialize;

use crate::state::queries::BatchRunRow;
use crate::state::Store;

/// The fixed status vocabulary (matches the schema CHECK constraint),
/// in lifecycle order for rendering.
pub const STATUSES: [&str; 5] = [
    "pending",
    "in_progress",
    "succeeded",
    "failed_terminal",
    "failed_retryable",
];

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub total_videos: i64,
    /// Zero-filled over [`STATUSES`].
    pub counts: BTreeMap<String, i64>,
    /// Raw stored kinds — including the legacy placeholder "Fetch". The
    /// human renderer annotates it; JSON consumers get stored truth.
    pub retryable_by_kind: BTreeMap<String, i64>,
    pub in_progress: Vec<InProgressAge>,
    pub batch_runs: Vec<BatchRunSummary>,
}

#[derive(Debug, Serialize)]
pub struct InProgressAge {
    pub video_id: String,
    pub claimed_by: Option<String>,
    pub claimed_at: Option<i64>,
    /// now − claimed_at; None when claimed_at is NULL (malformed row —
    /// rendered as unknown, never a crash).
    pub age_s: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct BatchRunSummary {
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
pub struct PolicyProvenance {
    pub bytes: usize,
    /// True iff policy_toml is byte-identical to THIS binary's compiled
    /// default. A binary upgrade can flip this for historical rows; that
    /// is honest — provenance is relative to the reading binary.
    pub compiled_default: bool,
}

#[derive(Debug, Serialize)]
pub struct CensusHeadline {
    pub sweep_examined: Option<u64>,
    pub claimed: Option<u64>,
    pub succeeded: Option<u64>,
    pub failed: Option<u64>,
}

pub fn build_report(store: &Store, now: i64) -> Result<StatusReport> {
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
    })
}

fn summarize_run(row: BatchRunRow, compiled_default_toml: Option<&str>) -> BatchRunSummary {
    let params = serde_json::from_str(&row.params_json)
        .unwrap_or_else(|_| serde_json::Value::String(row.params_json.clone()));
    let census_headline = row.census_json.as_deref().and_then(|c| {
        serde_json::from_str::<serde_json::Value>(c).ok().map(|v| CensusHeadline {
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

/// Render a unix timestamp as "YYYY-MM-DD HH:MM:SSZ". Out-of-range values
/// (hand-edited DBs) render as a marker, never panic.
pub fn fmt_utc(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map_or_else(|| format!("(invalid timestamp {ts})"), |dt| {
            dt.format("%Y-%m-%d %H:%M:%SZ").to_string()
        })
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

pub fn render_report(report: &StatusReport) -> String {
    // Writing to a String is infallible; unwraps are forbidden, so route
    // through a helper closure that ignores the Ok(()) results via let _.
    let mut out = String::new();
    let _ = writeln!(out, "videos: {} total", report.total_videos);
    for s in STATUSES {
        let _ = writeln!(out, "  {:<18} {:>7}", s, report.counts.get(s).copied().unwrap_or(0));
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
            let note = if kind == "Fetch" { "  (legacy placeholder kind)" } else { "" };
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
                r.claimed_at.map_or_else(|| "(unknown)".to_string(), fmt_utc),
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
                    run.run_id, fmt_utc(run.started_at), fmt_utc(fin), params, policy
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
                        let _ = writeln!(out, "         census: unreadable (closed run with unparseable census_json)");
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
        if let Some(v) = obj.get("cookies_present").and_then(serde_json::Value::as_bool) {
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
```

- [ ] **Step 5: Wire CLI + main**

`src/cli.rs` — append to `Command` after `Migrate`:

```rust
    /// Report pipeline state: counts by status, failure breakdowns,
    /// current claims, and batch-run history. Read-only.
    Status {
        /// Emit machine-readable JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
```

`src/main.rs`:
1. Add `mod status;` to the module list (alphabetical: after `mod state;`).
2. Add the arm after `Command::Migrate`:

```rust
        cli::Command::Status { json } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "status: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            let store = state::Store::open(path).context("opening state DB")?;
            let report = status::build_report(&store, state::unix_now())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).context("serializing status report")?
                );
            } else {
                print!("{}", status::render_report(&report));
            }
        }
```

Note: `state::unix_now()` is `pub(crate)` after Step 3 — main is the same crate as the bin's `state` module.

- [ ] **Step 6: Run the new tests**

Run: `cargo test --test status -- --test-threads=1`
Expected: 4/4 pass. If the "0 total" assertion fails on formatting, fix the renderer, not the test — "videos: 0 total" is the contract.

- [ ] **Step 7: Ground-truth acceptance against the pilot snapshot (v3, read-only)**

```bash
cargo run --quiet -- --state-db ddp-run-export.sqlite status
```

Expected output must contain EXACTLY these numbers (the epic's built-in fixture):
- `pending 0`, `in_progress 0` (rendered zero-filled), `succeeded 51903`, `failed_terminal 3928`, `failed_retryable 789`, total `56620`;
- by kind: `NoPermission 418`, `Fetch 301` with the legacy annotation, `FfprobePostprocess 36`, `NoVideoFormats 32`, `NoDataBlocks 1`, `HttpError 1`;
- `run 1` line containing `INTERRUPTED` and `started 2026-07-08 11:41:50Z`, `retries=1`;
- `run 2` line containing `finished 2026-07-08 16:32:12Z`, `retries=2`, `policy: compiled default (3065 B)` and a census line.

Also `cargo run --quiet -- --state-db ddp-run-export.sqlite status --json | python3 -m json.tool > /dev/null` — exit 0.

If any number disagrees, the QUERY is wrong (the snapshot is ground truth) — debug before proceeding. Paste the actual output block into the task report.

- [ ] **Step 8: Full verification + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/queries.rs src/state/mod.rs src/status.rs src/cli.rs src/main.rs tests/status.rs
git commit -m "feat(status): status subcommand core — counts, retryable-by-kind, claim ages, honest batch-run history, --json

Bare status is DB-only (0017 disk checks land behind --verify in a later
task). Interrupted batch_runs rows render as INTERRUPTED, never skipped
(epic-4 followup). Verified against ddp-run-export.sqlite ground truth:
51903/3928/789 and the six-kind retryable breakdown reproduce exactly."
```
