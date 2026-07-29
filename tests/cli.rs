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

#[test]
fn process_retries_rejects_negative_values() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["process", "--retries=-1"]) // = form: a bare "-1" token reads as a flag to clap
        .assert()
        .code(2); // clap range violation, not a silent zero-budget run
}

#[test]
fn process_retries_rejects_i64_max() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["process", "--retries", "9223372036854775807"])
        .assert()
        .code(2); // would overflow at retries + 1
}

#[test]
fn process_retries_accepts_bounds() {
    // Parse-only check: valid values get PAST argument parsing and fail
    // later on the missing model file, not with a usage error (exit != 2).
    for v in ["0", "1000000"] {
        let assert = Command::cargo_bin("ddp-transcribe")
            .unwrap()
            .args([
                "--state-db",
                "/nonexistent/x.sqlite",
                "process",
                "--retries",
                v,
            ])
            .assert()
            .failure();
        assert_ne!(
            assert.get_output().status.code(),
            Some(2),
            "--retries {v} must parse"
        );
    }
}

#[test]
fn process_checkpoint_every_requires_checkpoint_cmd() {
    // An interval with nothing to run is an operator typo, not a silent
    // no-op: clap's `requires` turns it into a usage error.
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["process", "--checkpoint-every", "5m"])
        .assert()
        .code(2);
}

#[test]
fn process_checkpoint_cmd_with_interval_parses() {
    // Parse-only check: the pair gets PAST argument parsing and fails later
    // on the missing state DB / model, not with a usage error (exit != 2).
    let assert = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            "/nonexistent/x.sqlite",
            "process",
            "--checkpoint-cmd",
            "/x",
            "--checkpoint-every",
            "5m",
        ])
        .assert()
        .failure();
    assert_ne!(
        assert.get_output().status.code(),
        Some(2),
        "--checkpoint-cmd with --checkpoint-every must parse"
    );
}

#[test]
fn config_echo_omits_model_path_for_non_model_subcommands() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "init"])
        .assert()
        .success()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !all.contains("whisper_model_path"),
        "init never loads a model; echoing the model path caused a production false alarm (epic-4 followup)"
    );
}

#[test]
fn config_echo_includes_model_path_for_process() {
    // process DOES consume the model — the echo must still advertise it.
    // The run fails later (missing model file); the echo happens first.
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "init"])
        .assert()
        .success();
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "process",
            "--max-videos",
            "0",
        ])
        .assert()
        .failure()
        .get_output()
        .clone();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(all.contains("whisper_model_path"));
}

#[test]
fn global_flags_accepted_after_subcommand() {
    // Parse-only checks: anything but clap's usage-error exit (2)
    // proves the flag was accepted in the post-subcommand position
    // (the run itself may then fail for other reasons, e.g. missing
    // DB — that's fine here).
    let cases: &[&[&str]] = &[
        &["status", "--profile", "dev"],
        &["status", "--state-db", "x.sqlite"],
        &["status", "--inbox", "in"],
        &["status", "--transcripts", "out"],
        &["status", "--log-format", "human"],
        &["status", "--whisper-model", "m.bin"],
        &["status", "--classification", "c.toml"],
        &["status", "--stale-claim-threshold", "30m"],
        &["status", "--download-workers", "2"],
        &["status", "--channel-capacity", "2"],
    ];
    for args in cases {
        let assert = Command::cargo_bin("ddp-transcribe")
            .unwrap()
            .args(*args)
            .assert();
        let code = assert.get_output().status.code();
        assert_ne!(code, Some(2), "clap rejected {args:?}");
    }
}
