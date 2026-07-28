#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn help_lists_plan_a_subcommands() {
    let mut cmd = Command::cargo_bin("ddp-transcribe").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("init"))
        .stdout(contains("ingest"))
        .stdout(contains("process"));
}

#[test]
fn init_subcommand_help_works() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["init", "--help"])
        .assert()
        .success();
}

#[test]
fn init_creates_state_sqlite() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");

    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "init"])
        .assert()
        .success();

    assert!(db.exists());
}

#[test]
fn init_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");

    for _ in 0..2 {
        Command::cargo_bin("ddp-transcribe")
            .unwrap()
            .args(["--state-db", db.to_str().unwrap(), "init"])
            .assert()
            .success();
    }
}

#[test]
fn ingest_rejects_reversed_window_range() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let inbox = tmp.path().join("inbox");

    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--inbox",
            inbox.to_str().unwrap(),
            "ingest",
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
}

#[test]
fn ingest_accepts_equal_window_start_and_end() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let inbox = tmp.path().join("inbox");

    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--inbox",
            inbox.to_str().unwrap(),
            "ingest",
            "--window-start",
            "2026-02-15",
            "--window-end",
            "2026-02-15",
        ])
        .assert()
        .success();
}
