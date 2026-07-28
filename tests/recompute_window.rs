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
        .args([
            "recompute-window",
            "--clear",
            "--window-start",
            "2026-02-01",
        ])
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
            "--state-db",
            db.to_str().unwrap(),
            "recompute-window",
            "--window-start",
            "2026-02-01",
            "--window-end",
            "2026-03-31",
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
            "--state-db",
            db.to_str().unwrap(),
            "recompute-window",
            "--window-start",
            "2026-03-01",
            "--dry-run",
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
            "--state-db",
            db.to_str().unwrap(),
            "recompute-window",
            "--window-start",
            "2026-03-01",
        ])
        .assert()
        .success();
    assert_eq!(flags(&db), vec![0, 1, 1]);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "recompute-window",
            "--clear",
        ])
        .assert()
        .success()
        .stdout(contains("changed 1"));
    assert_eq!(flags(&db), vec![1, 1, 1]);
}

#[test]
fn rejects_reversed_window_range() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "recompute-window",
            "--window-start",
            "2026-03-01",
            "--window-end",
            "2026-02-01",
        ])
        .assert()
        .failure()
        .stderr(contains("--window-start"))
        .stderr(contains("--window-end"))
        .stderr(contains("2026-03-01"))
        .stderr(contains("2026-02-01"));
    assert_eq!(flags(&db), vec![1, 1, 1], "reversed range must not write");
}

#[test]
fn refuses_missing_db() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("absent.sqlite");
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "recompute-window",
            "--clear",
        ])
        .assert()
        .failure()
        .stderr(contains("not found"));
    assert!(!db.exists());
}
