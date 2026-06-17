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
