# Task 06: `recompute-window` subcommand — explicit, one-shot, refuses bare invocation

**Files:**
- Modify: `src/state/mod.rs` (add `recompute_window` mutator + `count_window_mismatches` dry-run query)
- Modify: `src/cli.rs` (`Command::RecomputeWindow` with a required ArgGroup)
- Modify: `src/main.rs` (RecomputeWindow arm)
- Create: `tests/recompute_window.rs` (auto-discovered — uses only public API; NO Cargo.toml block)

**Interfaces:**
- Consumes (Task 05): `ingest::WindowBounds::from_dates` and its pub fields `start` / `end_exclusive` (start inclusive-midnight, end-exclusive next-midnight), `cli::parse_window_date`, schema v4.
- Produces:
  - `Store::recompute_window(&mut self, start: Option<i64>, end_exclusive: Option<i64>) -> anyhow::Result<usize>` (row-change count per 0006; plain `Option<i64>` bounds so `state` never imports from `ingest`)
  - `Store::count_window_mismatches(&self, start: Option<i64>, end_exclusive: Option<i64>) -> anyhow::Result<usize>`
  - CLI contract (Task 08's window ADR records it): one of `--window-start` / `--window-end` / `--clear` is REQUIRED (bare invocation = clap usage error, exit 2 — silently wiping the study's filter must be impossible); `--clear` conflicts with the window flags; `--dry-run` reports the would-change count without writing.

- [ ] **Step 1: Write the failing tests**

Create `tests/recompute_window.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::str::contains;

/// DB with three watch_history rows at known instants:
///   2026-02-10 12:00:00Z (in Feb), 2026-03-05 12:00:00Z (in Mar),
///   2026-04-01 00:00:00Z (Apr 1 midnight) — all seeded in_window = 1.
fn seeded_db(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    let db = tmp.path().join("state.sqlite");
    {
        let _s = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    let ts = |y, m, d, h| {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    };
    for (i, t) in [ts(2026, 2, 10, 12), ts(2026, 3, 5, 12), ts(2026, 4, 1, 0)]
        .into_iter()
        .enumerate()
    {
        let vid = format!("70000000000000001{i:02}");
        conn.execute(
            "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
             VALUES (?1, 'https://example/', 1, 'pending', 1, 1)",
            rusqlite::params![vid],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO watch_history (respondent_id, video_id, watched_at, in_window)
             VALUES ('r1', ?1, ?2, 1)",
            rusqlite::params![vid, t],
        )
        .unwrap();
    }
    db
}

fn flags(db: &std::path::Path) -> Vec<i64> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT in_window FROM watch_history ORDER BY watched_at")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<i64>, _>>()
        .unwrap()
}

#[test]
fn bare_invocation_is_a_usage_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "recompute-window"])
        .assert()
        .code(2); // clap required-group violation; DB untouched
    assert_eq!(flags(&db), vec![1, 1, 1]);
}

#[test]
fn clear_conflicts_with_window_flags() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["recompute-window", "--clear", "--window-start", "2026-02-01"])
        .assert()
        .code(2);
}

#[test]
fn recompute_applies_inclusive_window() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "recompute-window", "--window-start", "2026-02-01", "--window-end", "2026-03-31",
        ])
        .assert()
        .success()
        .stdout(contains("changed 1"));
    // Feb 10 stays in, Mar 5 stays in (end date inclusive through Mar 31),
    // Apr 1 00:00 flips out (day after end excluded).
    assert_eq!(flags(&db), vec![1, 1, 0]);
}

#[test]
fn dry_run_reports_without_writing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "recompute-window", "--window-start", "2026-03-01", "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("would change 1"));
    assert_eq!(flags(&db), vec![1, 1, 1], "dry-run must not write");
}

#[test]
fn clear_sets_everything_in_window() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    // First shrink the window, then --clear restores all-1.
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db", db.to_str().unwrap(),
            "recompute-window", "--window-start", "2026-03-01",
        ])
        .assert()
        .success();
    assert_eq!(flags(&db), vec![0, 1, 1]);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "recompute-window", "--clear"])
        .assert()
        .success()
        .stdout(contains("changed 1"));
    assert_eq!(flags(&db), vec![1, 1, 1]);
}

#[test]
fn refuses_missing_db() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("absent.sqlite");
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "recompute-window", "--clear"])
        .assert()
        .failure()
        .stderr(contains("not found"));
    assert!(!db.exists());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test recompute_window -- --test-threads=1`
Expected: all fail — `recompute-window` is not a subcommand yet.

- [ ] **Step 3: Store mutator + dry-run query**

`src/state/mod.rs`, after `close_batch_run` (same `impl Store` block). The CASE expression appears twice (SET + WHERE) so the UPDATE only touches rows whose flag actually changes — the returned count per 0006 is the honest "changed" number, and unchanged rows don't pay a write:

```rust
    /// One-shot in_window recomputation over ALL watch_history rows
    /// (Epic 4b window ADR). Bounds are unix seconds: start inclusive,
    /// end exclusive (the CLI derives them from inclusive calendar dates);
    /// both None = clear (everything in-window). Returns the number of
    /// rows whose flag actually changed, per 0006.
    pub fn recompute_window(
        &mut self,
        start: Option<i64>,
        end_exclusive: Option<i64>,
    ) -> Result<usize> {
        let changed = self
            .conn
            .execute(
                "UPDATE watch_history
                 SET in_window = CASE WHEN (?1 IS NULL OR watched_at >= ?1)
                                       AND (?2 IS NULL OR watched_at < ?2)
                                  THEN 1 ELSE 0 END
                 WHERE in_window != CASE WHEN (?1 IS NULL OR watched_at >= ?1)
                                          AND (?2 IS NULL OR watched_at < ?2)
                                     THEN 1 ELSE 0 END",
                params![start, end_exclusive],
            )
            .context("recompute watch_history.in_window")?;
        Ok(changed)
    }

    /// Dry-run companion to [`Store::recompute_window`]: how many rows
    /// WOULD change under these bounds. Read-only.
    pub fn count_window_mismatches(
        &self,
        start: Option<i64>,
        end_exclusive: Option<i64>,
    ) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM watch_history
                 WHERE in_window != CASE WHEN (?1 IS NULL OR watched_at >= ?1)
                                          AND (?2 IS NULL OR watched_at < ?2)
                                     THEN 1 ELSE 0 END",
                params![start, end_exclusive],
                |r| r.get(0),
            )
            .context("count in_window mismatches")?;
        Ok(usize::try_from(n).unwrap_or(0))
    }
```

- [ ] **Step 4: CLI + main**

`src/cli.rs` — new variant after `Status` (the ArgGroup makes bare invocation a clap usage error, exit 2, per the original spec):

```rust
    /// Recompute watch_history.in_window from explicit window flags.
    /// One-shot; does not re-read DDP files. Refuses to run bare —
    /// silently wiping the study's window filter must be impossible.
    #[command(group(clap::ArgGroup::new("window").required(true).multiple(true)))]
    RecomputeWindow {
        /// Inclusive analysis-window start (YYYY-MM-DD, UTC).
        #[arg(long, value_parser = parse_window_date, group = "window")]
        window_start: Option<chrono::NaiveDate>,
        /// Inclusive analysis-window end (YYYY-MM-DD, UTC; covers that whole day).
        #[arg(long, value_parser = parse_window_date, group = "window")]
        window_end: Option<chrono::NaiveDate>,
        /// Explicitly opt into "no filter": set in_window = 1 for ALL rows.
        #[arg(long, group = "window", conflicts_with_all = ["window_start", "window_end"])]
        clear: bool,
        /// Report how many rows would change, without writing.
        #[arg(long)]
        dry_run: bool,
    },
```

(Syntax note: clap 4 supports ArgGroups on subcommand variants; `#[command(group = ArgGroup::new(...))]` is an equivalent accepted spelling. Verify with the Step 1 tests, don't hand-roll a runtime check unless clap genuinely can't express it. If a runtime check IS needed, `anyhow::bail!` gives exit 1, and the two exit-code assertions in Step 1 must then be adjusted to `.failure()` with a disclosed deviation per 0003.)

`src/main.rs` — new arm (after Status):

```rust
        cli::Command::RecomputeWindow {
            window_start,
            window_end,
            clear,
            dry_run,
        } => {
            let path = &cfg.state_db;
            if !path.exists() {
                anyhow::bail!(
                    "recompute-window: state.sqlite not found at {}. Run `ddp-transcribe init` first.",
                    path.display()
                );
            }
            // --clear == both bounds None (everything in-window); clap
            // guarantees clear XOR window flags.
            let window = if clear {
                ingest::WindowBounds::default()
            } else {
                ingest::WindowBounds::from_dates(window_start, window_end)
            };
            let mut store = state::Store::open(path).context("opening state DB")?;
            if dry_run {
                let n = store.count_window_mismatches(window.start, window.end_exclusive)?;
                println!("recompute-window dry-run: would change {n} row(s)");
            } else {
                let n = store.recompute_window(window.start, window.end_exclusive)?;
                println!("recompute-window: changed {n} row(s)");
            }
        }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --test recompute_window -- --test-threads=1`
Expected: 6/6 pass.

- [ ] **Step 6: Full verification + commit**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green.

```bash
git add src/state/mod.rs src/cli.rs src/main.rs tests/recompute_window.rs
git commit -m "feat(state): recompute-window subcommand — explicit one-shot in_window recompute; refuses bare invocation

Required ArgGroup makes bare invocation a usage error (exit 2); --clear
is the explicit no-filter opt-in; --dry-run counts without writing;
mutator returns the actually-changed row count per 0006."
```
