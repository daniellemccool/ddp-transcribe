# Task 04: CLI wiring + shim integration tests + ignored live test

**Files:**
- Modify: `src/cli.rs` (`Command::BackfillMetadata` variant, appended after `LoadMetadata`)
- Modify: `src/main.rs` (dispatch arm + `log_resolved_config` exhaustive-match arm)
- Create: `tests/backfill_metadata.rs` (binary integration tests via a yt-dlp PATH shim; auto-discovered, NO Cargo.toml block)

**Interfaces:**
- Consumes (exact, landed in Tasks 01–03):
  - `backfill::backfill_metadata(store: &mut Store, ytdlp_timeout: Duration, limit: Option<u64>) -> Result<BackfillStats>` (async), `backfill::BackfillStats` (Display prints `examined N / captured N / capture-failed N / already-filled N / insert-failed N`).
  - `Store::count_succeeded_missing_metadata(&self) -> Result<u64>`; `Store::open`; `cfg.ytdlp_timeout` (`config::Config`, default 300 s).
  - Existing arm precedent: the `LoadMetadata` dispatch (`src/main.rs:401-416`) — missing-DB bail phrasing, `tracing::info!` + `println!` split (logs → stderr, human line → stdout).
- Produces: the `backfill-metadata` subcommand surface Task 06 documents:
  - `ddp-transcribe backfill-metadata [--limit N] [--dry-run]`
  - stdout in both modes: `backfill-metadata: cohort {n} succeeded videos missing metadata…`; run mode adds `backfill-metadata: {stats}`.

**Semantics (binding):**
- `--dry-run` prints the cohort size and exits without invoking yt-dlp (kickoff contract). Cohort count prints in run mode too (operator orientation).
- Missing DB ⇒ bail BEFORE `Store::open` (never create an empty DB), mirroring the `LoadMetadata` arm's wording style verbatim (adapted to `backfill-metadata:`).
- `log_resolved_config` gains a `Command::BackfillMetadata { .. }` arm sharing the minimal `profile + state_db` shape (same as `LoadMetadata`; the exhaustive match won't compile without it — that's the tripwire working).
- Tests inject a fake `yt-dlp` onto the **child's** PATH via `assert_cmd .env()` — never process-global `std::env::set_var` (no cross-test races even beyond the mandatory `--test-threads=1`).
- One `#[ignore]`d live test (network + real yt-dlp) per the Epic 4c close precedent; it runs only when explicitly named.

- [ ] **Step 1: CLI variant**

`src/cli.rs`, append to `enum Command` after `LoadMetadata`:

```rust
    /// Backfill raw metadata (video_metadata_raw) for succeeded videos
    /// that predate fetch-time capture. Metadata-only yt-dlp per video —
    /// no media download, never touches video status. Best-effort and
    /// re-runnable; run `load-metadata` afterwards to fill the typed
    /// columns.
    BackfillMetadata {
        /// Cap the number of videos attempted (smoke runs).
        #[arg(long)]
        limit: Option<u64>,
        /// Print the cohort size and exit without invoking yt-dlp.
        #[arg(long)]
        dry_run: bool,
    },
```

- [ ] **Step 2: Dispatch + config-echo arms**

`src/main.rs`:

1. `log_resolved_config`: add `Command::BackfillMetadata { .. }` to the arm `LoadMetadata` shares (profile + state_db, no `whisper_model_path`) — read the existing match and extend the same arm pattern.

2. Dispatch arm, after the `LoadMetadata` arm — copy that arm's missing-DB bail wording exactly, substituting the subcommand name:

```rust
        Command::BackfillMetadata { limit, dry_run } => {
            if !cli.global.state_db.exists() {
                // Mirror the LoadMetadata arm's exact bail phrasing with
                // `backfill-metadata:` as the prefix.
                anyhow::bail!(
                    "backfill-metadata: state.sqlite not found at {} . Run `ddp-transcribe init` first.",
                    cli.global.state_db.display()
                );
            }
            let mut store = state::Store::open(&cli.global.state_db)?;
            let cohort = store.count_succeeded_missing_metadata()?;
            if dry_run {
                tracing::info!(cohort, "backfill-metadata dry-run");
                println!(
                    "backfill-metadata: cohort {cohort} succeeded videos missing metadata (dry-run)"
                );
            } else {
                println!("backfill-metadata: cohort {cohort} succeeded videos missing metadata");
                let stats = backfill::backfill_metadata(&mut store, cfg.ytdlp_timeout, limit).await?;
                tracing::info!(%stats, "backfill-metadata complete");
                println!("backfill-metadata: {stats}");
            }
        }
```

(Adapt the `state::Store` path and the bail string to the file's actual conventions — open the `LoadMetadata` arm and copy its shapes literally. Remove any ADR-0002 `#[allow(dead_code)]` Task 03 left on the backfill items, per its report.)

- [ ] **Step 3: Write the failing integration tests**

Create `tests/backfill_metadata.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `backfill-metadata` end-to-end via a fake yt-dlp shim on the child's
//! PATH. Public API seeding (Store::open + pub upserts) plus raw
//! rusqlite, so this file needs no `[[test]]` block per 0005.

use assert_cmd::Command as AssertCommand;

/// Shim: prints one metadata JSON line for any URL, unless the URL
/// contains "dead" (exit 1, stderr) — the dead-link cohort stand-in.
const SHIM: &str = r#"#!/bin/sh
for last; do :; done
case "$last" in
  *dead*) echo "ERROR: video unavailable" >&2; exit 1 ;;
  *) printf '{"id":"shim","description":"backfilled by shim"}\n' ;;
esac
"#;

/// Writes the shim as `yt-dlp` in a fresh dir and returns a PATH value
/// putting it first. Child-process env only — never std::env::set_var.
fn shim_path(dir: &tempfile::TempDir) -> String {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.path().join("shim-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("yt-dlp");
    std::fs::write(&shim, SHIM).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

/// v1 succeeded+envelope (not in cohort), v2 succeeded (cohort),
/// v3 succeeded with a "dead" URL (cohort), v4 pending (not in cohort).
fn seeded_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let db = dir.path().join("state.sqlite");
    {
        let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
        store.upsert_video("v1", "https://example/1", false).unwrap();
        store.upsert_video("v2", "https://example/2", false).unwrap();
        store.upsert_video("v3", "https://example/dead3", false).unwrap();
        store.upsert_video("v4", "https://example/4", false).unwrap();
        store
            .upsert_metadata_raw("v1", r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#)
            .unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE videos SET status = 'succeeded' WHERE video_id IN ('v1','v2','v3')",
        [],
    )
    .unwrap();
    db
}

fn statuses(db: &std::path::Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT video_id, status FROM videos ORDER BY video_id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

#[test]
fn dry_run_prints_cohort_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let before = statuses(&db);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata", "--dry-run"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("cohort 2"), "stdout was: {out}");
    assert!(out.contains("dry-run"));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let raw_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_metadata_raw", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw_rows, 1, "dry-run must not write");
    assert_eq!(statuses(&db), before);
}

#[test]
fn backfill_captures_cohort_best_effort_and_never_touches_status() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let before = statuses(&db);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .success(); // dead video must NOT fail the run (best-effort)
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("cohort 2"), "stdout was: {out}");
    assert!(
        out.contains("examined 2") && out.contains("captured 1") && out.contains("capture-failed 1"),
        "stdout was: {out}"
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    // v2 gained a schema:1 envelope wrapping the shim's printed line.
    let v2_raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'v2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(v2_raw.contains(r#""schema":1"#), "raw was: {v2_raw}");
    assert!(v2_raw.contains("backfilled by shim"), "raw was: {v2_raw}");
    // v1's pre-existing envelope untouched; v3 (dead) has none.
    let v1_raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(v1_raw.contains(r#"\"id\":\"v1\""#), "raw was: {v1_raw}");
    let v3_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM video_metadata_raw WHERE video_id = 'v3'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(v3_rows, 0);

    // THE invariant: statuses and lifecycle byte-identical.
    assert_eq!(statuses(&db), before);

    // Re-run converges: only v3 (still dead) is attempted.
    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("cohort 1") && out.contains("examined 1"), "stdout was: {out}");
}

#[test]
fn limit_caps_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata", "--limit", "1"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("examined 1"), "stdout was: {out}");
}

#[test]
fn refuses_missing_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("nope.sqlite");
    AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .assert()
        .failure();
    assert!(!db.exists(), "must not create an empty DB");
}

/// Live smoke: real yt-dlp + network. Run explicitly:
/// `cargo test --test backfill_metadata -- --ignored --test-threads=1`
#[test]
#[ignore = "network + real yt-dlp; run explicitly before release"]
fn live_backfill_captures_one_real_video() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    {
        let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
        // Stable, long-lived public video (same corpus video the Epic 4c
        // live validation used — see docs/superpowers/plans/2026-07-28-plan-b-epic-4c
        // ground truth; substitute any known-alive corpus URL).
        store
            .upsert_video(
                "live1",
                "https://www.tiktok.com/@tiktok/video/7106594312292453675",
                false,
            )
            .unwrap();
    }
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("UPDATE videos SET status = 'succeeded'", [])
        .unwrap();

    AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "backfill-metadata"])
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success();

    let raw: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'live1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.contains(r#""schema":1"#), "raw was: {raw}");
}
```

- [ ] **Step 4: Run to confirm failures, then make green**

Run: `cargo test --test backfill_metadata -- --test-threads=1` — first run fails on the missing subcommand; fix wiring until all 4 non-ignored tests pass.

- [ ] **Step 5: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green, suite total = Task 03's total + 4 (the live test stays ignored).

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs src/main.rs tests/backfill_metadata.rs
git commit -m "feat(cli): backfill-metadata subcommand — cohort dry-run, best-effort capture, --limit; shim + ignored live e2e"
```
