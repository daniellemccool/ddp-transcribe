# Task 03: `status` detail surfaces — `--video-id`, `--respondent-id`, `--errors`, `--retryable`

**Files:**
- Modify: `src/state/queries.rs` (add `get_video_detail`, `list_video_events`, `respondent_summary`, `list_terminal_failures` + row structs)
- Modify: `src/state/mod.rs` (derive `Serialize` on `ParkedRow`; lift `attempt_count`'s `#[allow(dead_code)]`)
- Modify: `src/batch.rs` (make `truncate_to_char_boundary` `pub(crate)`)
- Modify: `src/status.rs` (detail renderers)
- Modify: `src/cli.rs` (extend `Command::Status` with the four flags)
- Modify: `src/main.rs` (dispatch by mode)
- Modify: `tests/status.rs` (new tests)

**Interfaces:**
- Consumes (from Task 02): `Command::Status { json }`, `src/state/queries.rs` module, `status::fmt_utc`, the Status arm's missing-DB bail.
- Produces:
  - `Store::get_video_detail(&self, video_id: &str) -> anyhow::Result<Option<VideoDetailRow>>`
  - `Store::list_video_events(&self, video_id: &str) -> anyhow::Result<Vec<VideoEventRow>>` (`VideoEventRow { at: i64, event_type: String, worker_id: Option<String>, detail_json: Option<String> }`)
  - `Store::respondent_summary(&self, respondent_id: &str) -> anyhow::Result<RespondentSummary>`
  - `Store::list_terminal_failures(&self) -> anyhow::Result<Vec<TerminalRow>>`
  - `batch::truncate_to_char_boundary` as `pub(crate)` (Task 04 does not use it; only this task's renderer does)
  - CLI: `Command::Status { json, video_id: Option<String>, respondent_id: Option<String>, errors: bool, retryable: bool }`

**Design constraints:**
- `--video-id` renders event `detail_json` payloads **legibly** (the archived Epic 3 followup lands here): known keys as `key=value` pairs, `message` on its own indented line truncated to 200 bytes char-boundary-safe. Raw JSON blobs appear only for unknown shapes. Every event shape in the vocabulary must render: `claimed`/`succeeded` (NULL detail), `{"kind","message"}`, `{"kind","message","policy"}`, `{"reason","message"}`, `{"new_kind"}`.
- Unknown `--video-id` → error exit (non-zero) with a "not found" message; NOT an empty report.
- `--errors` and `--retryable` may be combined; each prints its own section. With `--json`, the modes emit a single JSON object with the requested sections.
- Legacy kind `"Fetch"` annotation rule applies to the `--retryable` list (human only).

- [ ] **Step 1: Write the failing tests**

Append to `tests/status.rs` (reuse `seeded_db`; extend it first):

In `seeded_db`, after the `batch_runs` inserts, add:

```rust
    // Event history for v_retry1: claim → retry_requeued → sweep requeue.
    for (at, ev, worker, detail) in [
        (110, "claimed", Some("w1"), None),
        (
            120,
            "retry_requeued",
            Some("w1"),
            Some(r#"{"kind":"NoPermission","message":"ERROR: You do not have permission to view this post","policy":"deterministic-audio"}"#),
        ),
        (130, "requeued", Some("sweep"), Some(r#"{"new_kind":"NoPermission"}"#)),
    ] {
        conn.execute(
            "INSERT INTO video_events (video_id, at, event_type, worker_id, detail_json)
             VALUES ('v_retry1', ?1, ?2, ?3, ?4)",
            rusqlite::params![at, ev, worker, detail],
        )
        .unwrap();
    }
    // watch_history for the respondent summary (in_window NOT NULL in v3).
    for (vid, watched_at) in [("v_ok1", 500), ("v_ok2", 600), ("v_retry1", 700)] {
        conn.execute(
            "INSERT INTO watch_history (respondent_id, video_id, watched_at, in_window)
             VALUES ('resp-a', ?1, ?2, 1)",
            rusqlite::params![vid, watched_at],
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE videos SET terminal_reason='IpBlockedMessage',
                           terminal_message='ERROR: Your IP address is blocked'
         WHERE video_id='v_term'",
        [],
    )
    .unwrap();
```

New tests:

```rust
#[test]
fn status_video_id_renders_legible_event_history() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status", "--video-id", "v_retry1"])
        .assert()
        .success()
        .stdout(contains("v_retry1"))
        .stdout(contains("failed_retryable"))
        .stdout(contains("claimed"))
        .stdout(contains("retry_requeued"))
        .stdout(contains("kind=NoPermission"))
        .stdout(contains("policy=deterministic-audio"))
        .stdout(contains("new_kind=NoPermission"))
        .stdout(contains("You do not have permission"))
        // Legibility contract: no raw JSON blobs for known shapes.
        .stdout(contains(r#"{"kind""#).not());
}

#[test]
fn status_video_id_unknown_is_a_failure() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status", "--video-id", "nope"])
        .assert()
        .failure()
        .stderr(contains("not found"));
}

#[test]
fn status_respondent_summary_counts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "status", "--respondent-id", "resp-a", "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let s = &v["respondent"];
    assert_eq!(s["videos_seen"], 3);
    assert_eq!(s["videos_in_window"], 3);
    assert_eq!(s["videos_succeeded"], 2);
    assert_eq!(s["videos_failed_retryable"], 1);
    assert_eq!(s["videos_failed_terminal"], 0);
    assert_eq!(s["watch_events"], 3);
}

#[test]
fn status_errors_and_retryable_lists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status", "--errors", "--retryable"])
        .assert()
        .success()
        .stdout(contains("v_term"))
        .stdout(contains("IpBlockedMessage"))
        .stdout(contains("v_retry1"))
        .stdout(contains("v_retry2"))
        .stdout(contains("(legacy placeholder kind)"));
}
```

Add `use predicates::prelude::PredicateBooleanExt;` to the test header (for `.not()`).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test status -- --test-threads=1`
Expected: the four new tests fail — the CLI rejects `--video-id` (unexpected argument).

- [ ] **Step 3: Add the queries**

Append to `src/state/queries.rs` (`use rusqlite::OptionalExtension;` joins the imports):

```rust
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
```

In `src/state/mod.rs`:
1. `ParkedRow` gains `serde::Serialize` (add `use serde::Serialize;` to the imports and `#[derive(Debug, Serialize)]`) — `status --retryable --json` serializes it directly.
2. Lift `attempt_count`'s `#[allow(dead_code)]` + its 0002 comment: the `--retryable` renderer is the new bin consumer (note the lift in the commit message per 0002).

In `src/batch.rs`: change `fn truncate_to_char_boundary` to `pub(crate) fn truncate_to_char_boundary` and extend its doc comment: "Also used by `status`'s event renderer to cap message excerpts."

- [ ] **Step 4: Add the renderers + CLI flags + dispatch**

`src/cli.rs` — the Status variant becomes:

```rust
    /// Report pipeline state: counts by status, failure breakdowns,
    /// current claims, and batch-run history. Read-only.
    Status {
        /// Full event history for one video.
        #[arg(long)]
        video_id: Option<String>,
        /// Per-respondent summary counts.
        #[arg(long)]
        respondent_id: Option<String>,
        /// List failed_terminal videos with terminal_reason / terminal_message.
        #[arg(long)]
        errors: bool,
        /// List failed_retryable videos with last_retryable_kind / _message.
        #[arg(long)]
        retryable: bool,
        /// Emit machine-readable JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
```

Append to `src/status.rs`:

```rust
use crate::state::queries::{RespondentSummary, TerminalRow, VideoDetailRow, VideoEventRow};
use crate::state::ParkedRow;

#[derive(Debug, Serialize)]
pub struct VideoDetailReport {
    pub video: VideoDetailRow,
    pub events: Vec<VideoEventRow>,
}

pub fn build_video_detail(store: &Store, video_id: &str) -> Result<VideoDetailReport> {
    let video = store
        .get_video_detail(video_id)
        .context("loading video row")?
        .with_context(|| format!("video {video_id} not found in the state DB"))?;
    let events = store
        .list_video_events(video_id)
        .context("loading video events")?;
    Ok(VideoDetailReport { video, events })
}

pub fn render_video_detail(r: &VideoDetailReport) -> String {
    let mut out = String::new();
    let v = &r.video;
    let _ = writeln!(out, "video {}", v.video_id);
    let _ = writeln!(out, "  url        {}", v.source_url);
    let _ = writeln!(out, "  status     {}  attempts {}", v.status, v.attempt_count);
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
            v.duration_s.map_or_else(|| "?".into(), |d| format!("{d:.1}")),
            v.language_detected.as_deref().unwrap_or("?"),
            v.fetcher.as_deref().unwrap_or("?"),
            v.transcript_source.as_deref().unwrap_or("?"),
        );
    }
    if let (Some(by), Some(at)) = (&v.claimed_by, v.claimed_at) {
        let _ = writeln!(out, "  claimed_by {by}  claimed_at {}", fmt_utc(at));
    }
    if let Some(kind) = &v.last_retryable_kind {
        let note = if kind == "Fetch" { "  (legacy placeholder kind)" } else { "" };
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
        let _ = writeln!(out, "{}", render_event_detail_inline(e.detail_json.as_deref()));
        if let Some(msg) = detail_message(e.detail_json.as_deref()) {
            let _ = writeln!(out, "        message: {}", excerpt(&msg));
        }
    }
    out
}

/// Inline key=value rendering of the known detail_json shapes
/// ({"kind","message"[,"policy"]}, {"reason","message"}, {"new_kind"}).
/// `message` is excluded here (rendered on its own line). Unknown shapes
/// fall back to the raw JSON so nothing is hidden.
fn render_event_detail_inline(detail: Option<&str>) -> String {
    let Some(raw) = detail else { return String::new() };
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return format!("  detail: {raw}");
    };
    let known = ["kind", "policy", "new_kind", "reason"];
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
pub struct RespondentReport {
    pub respondent: RespondentSummary,
}

pub fn render_respondent(r: &RespondentReport) -> String {
    let s = &r.respondent;
    let mut out = String::new();
    let _ = writeln!(out, "respondent {}", s.respondent_id);
    let _ = writeln!(out, "  watch_events            {:>7}", s.watch_events);
    let _ = writeln!(out, "  videos_seen             {:>7}", s.videos_seen);
    let _ = writeln!(out, "  videos_in_window        {:>7}", s.videos_in_window);
    let _ = writeln!(out, "  videos_succeeded        {:>7}", s.videos_succeeded);
    let _ = writeln!(out, "  videos_failed_terminal  {:>7}", s.videos_failed_terminal);
    let _ = writeln!(out, "  videos_failed_retryable {:>7}", s.videos_failed_retryable);
    let _ = writeln!(out, "  videos_pending          {:>7}", s.videos_pending);
    let _ = writeln!(out, "  videos_in_progress      {:>7}", s.videos_in_progress);
    out
}

#[derive(Debug, Serialize)]
pub struct FailureLists {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<TerminalRow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<Vec<ParkedRow>>,
}

pub fn render_failure_lists(l: &FailureLists) -> String {
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
            let note = if kind == "Fetch" { "  (legacy placeholder kind)" } else { "" };
            let _ = writeln!(out, "  {}  {kind}  attempts {}{note}", r.video_id, r.attempt_count);
            if let Some(msg) = &r.last_retryable_message {
                let _ = writeln!(out, "      message: {}", excerpt(msg));
            }
        }
    }
    out
}
```

`src/main.rs` — the Status arm becomes a mode dispatch (missing-DB bail stays first):

```rust
        cli::Command::Status {
            video_id,
            respondent_id,
            errors,
            retryable,
            json,
        } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "status: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            let store = state::Store::open(path).context("opening state DB")?;
            if let Some(id) = video_id {
                let report = status::build_video_detail(&store, &id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_video_detail(&report));
                }
            } else if let Some(id) = respondent_id {
                let report = status::RespondentReport {
                    respondent: store.respondent_summary(&id).context("respondent summary")?,
                };
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_respondent(&report));
                }
            } else if errors || retryable {
                let lists = status::FailureLists {
                    errors: if errors {
                        Some(store.list_terminal_failures().context("listing terminal failures")?)
                    } else {
                        None
                    },
                    retryable: if retryable {
                        Some(store.list_failed_retryable().context("listing retryable failures")?)
                    } else {
                        None
                    },
                };
                if json {
                    println!("{}", serde_json::to_string_pretty(&lists)?);
                } else {
                    print!("{}", status::render_failure_lists(&lists));
                }
            } else {
                let report = status::build_report(&store, state::unix_now())?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print!("{}", status::render_report(&report));
                }
            }
        }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --test status -- --test-threads=1`
Expected: 8/8 pass (Task 02's four + this task's four).

- [ ] **Step 6: Ground-truth spot-checks against the pilot snapshot (v3, read-only)**

```bash
cargo run --quiet -- --state-db ddp-run-export.sqlite status --respondent-id preview --json
```
Expected: `watch_events` 64931, `videos_in_window` == `videos_seen` (all v3 rows are in_window=1).

```bash
cargo run --quiet -- --state-db ddp-run-export.sqlite status --retryable | head -20
cargo run --quiet -- --state-db ddp-run-export.sqlite status --retryable | grep -c '^  7'
```
Expected: header `failed_retryable (789):`; the grep count is 789 (one indented id line per row).

Pick any video id from the `--retryable` output and run `status --video-id <id>` — expect a legible event trail (claimed / retry_requeued / requeued / cookie_parked vocabulary), messages excerpted, no raw `{"kind"` blobs. Paste one example into the task report.

- [ ] **Step 7: Full verification + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/queries.rs src/state/mod.rs src/batch.rs src/status.rs src/cli.rs src/main.rs tests/status.rs
git commit -m "feat(status): detail surfaces — --video-id event history, --respondent-id summary, --errors/--retryable lists

Event detail_json renders legibly (kind/policy/new_kind/reason inline,
message excerpted char-boundary-safe) — lands the archived Epic 3
followup's per-event rendering thread.

0002 dead-code note: ParkedRow.attempt_count allow lifted — the
--retryable renderer is the first bin consumer since triage retired."
```
