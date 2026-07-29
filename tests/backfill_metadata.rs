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
/// putting it first. Child-process env only — never `std::env::set_var`.
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
        store
            .upsert_video("v1", "https://example/1", false)
            .unwrap();
        store
            .upsert_video("v2", "https://example/2", false)
            .unwrap();
        store
            .upsert_video("v3", "https://example/dead3", false)
            .unwrap();
        store
            .upsert_video("v4", "https://example/4", false)
            .unwrap();
        store
            .upsert_metadata_raw("v1", r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#)
            .unwrap();
    }
    // Flip statuses with raw rusqlite — no public mutator sets
    // `succeeded` without a claim, and tests must not grow one.
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
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "backfill-metadata",
            "--dry-run",
        ])
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
        out.contains("examined 2")
            && out.contains("captured 1")
            && out.contains("capture-failed 1"),
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
    assert!(
        out.contains("cohort 1") && out.contains("examined 1"),
        "stdout was: {out}"
    );
}

#[test]
fn limit_caps_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .env("PATH", shim_path(&dir))
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "backfill-metadata",
            "--limit",
            "1",
        ])
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

/// The URL the live test fetches. Override with `DDP_TRANSCRIBE_E2E_URL`
/// (the `tests/e2e_real_tools.rs` idiom); the compiled-in default is a
/// real public URL from the `news_orgs` bake fixture, which may age out —
/// set the variable for a dependable manual run.
fn live_url() -> String {
    std::env::var("DDP_TRANSCRIBE_E2E_URL")
        .unwrap_or_else(|_| "https://www.tiktok.com/@nosstories/video/7636781376787795232".into())
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
        store.upsert_video("live1", &live_url(), false).unwrap();
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
