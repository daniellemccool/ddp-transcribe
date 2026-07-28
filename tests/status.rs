#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
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
    // Event history for v_retry1: claim → retry_requeued → sweep requeue.
    for (at, ev, worker, detail) in [
        (110, "claimed", Some("w1"), None),
        (
            120,
            "retry_requeued",
            Some("w1"),
            Some(
                r#"{"kind":"NoPermission","message":"ERROR: You do not have permission to view this post","policy":"deterministic-audio"}"#,
            ),
        ),
        (
            130,
            "requeued",
            Some("sweep"),
            Some(r#"{"new_kind":"NoPermission"}"#),
        ),
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
    // Policy provenance: 'schema = 1' is 10 bytes and is not the compiled default.
    assert_eq!(runs[0]["policy"]["bytes"], 10);
    assert_eq!(runs[0]["policy"]["compiled_default"], false);
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

#[test]
fn status_json_flags_compiled_default_policy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    let default_toml = ddp_transcribe::classification::ClassificationTable::compiled_default()
        .unwrap()
        .source_toml()
        .to_string();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO batch_runs (started_at, finished_at, params_json, policy_toml, census_json)
         VALUES (500, 600, '{}', ?1, '{}')",
        rusqlite::params![default_toml],
    )
    .unwrap();
    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let runs = v["batch_runs"].as_array().unwrap();
    let run3 = &runs[2];
    assert_eq!(run3["policy"]["compiled_default"], true);
    assert_eq!(
        run3["policy"]["bytes"],
        u64::try_from(default_toml.len()).unwrap()
    );
}

#[test]
fn status_video_id_renders_legible_event_history() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp);
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "status",
            "--video-id",
            "v_retry1",
        ])
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
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "status",
            "--video-id",
            "nope",
        ])
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
            "--state-db",
            db.to_str().unwrap(),
            "status",
            "--respondent-id",
            "resp-a",
            "--json",
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
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "status",
            "--errors",
            "--retryable",
        ])
        .assert()
        .success()
        .stdout(contains("v_term"))
        .stdout(contains("IpBlockedMessage"))
        .stdout(contains("v_retry1"))
        .stdout(contains("v_retry2"))
        .stdout(contains("(legacy placeholder kind)"));
}

#[test]
fn status_detail_modes_conflict_at_parse_time() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["status", "--video-id", "x", "--errors"])
        .assert()
        .code(2); // clap usage error, not silent precedence
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["status", "--video-id", "x", "--respondent-id", "y"])
        .assert()
        .code(2);
}

/// Minimal VALID transcript JSON for the schema-version check —
/// TranscriptMetadata's mandatory fields + raw_signals.schema_version.
fn artifact_json(video_id: &str, schema_version: &str) -> String {
    format!(
        r#"{{"video_id":"{video_id}","source_url":"https://example/{video_id}",
"duration_s":1.0,"language_detected":"en","transcribed_at":"2026-07-13T00:00:00Z",
"fetcher":"ytdlp","transcript_source":"whisper-rs","model":"test",
"raw_signals":{{"schema_version":"{schema_version}","language":"en","lang_probs":null,"segments":[]}}}}"#
    )
}

fn write_artifacts(root: &std::path::Path, video_id: &str, txt: bool, json: Option<&str>) {
    // Shard = last two chars of the id (output::shard contract).
    let shard = &video_id[video_id.len() - 2..];
    let dir = root.join(shard);
    std::fs::create_dir_all(&dir).unwrap();
    if txt {
        std::fs::write(dir.join(format!("{video_id}.txt")), "text").unwrap();
    }
    if let Some(ver) = json {
        std::fs::write(
            dir.join(format!("{video_id}.json")),
            artifact_json(video_id, ver),
        )
        .unwrap();
    }
}

#[test]
fn verify_reports_missing_and_mismatched_artifacts_and_exits_1() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = seeded_db(&tmp); // has v_ok1 + v_ok2 succeeded, v_pend pending, v_prog in_progress
    let transcripts = tmp.path().join("transcripts");
    write_artifacts(&transcripts, "v_ok1", true, Some("1")); // complete + valid
    write_artifacts(&transcripts, "v_ok2", true, None); // .json missing

    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--transcripts",
            transcripts.to_str().unwrap(),
            "status",
            "--verify",
            "--json",
        ])
        .assert()
        .code(1) // pending + in_progress + missing artifact → NOT pause-safe
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let ver = &v["verify"];
    assert_eq!(ver["succeeded_rows"], 2);
    assert_eq!(ver["artifacts_missing"], 1);
    assert_eq!(ver["schema_version_mismatches"], 0);
    assert_eq!(ver["pause_safe"], false);
    assert_eq!(ver["sample_missing"][0], "v_ok2");
}

#[test]
fn verify_flags_schema_version_mismatch() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    {
        let _s = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('v_badver', 'https://example/v_badver', 1, 'succeeded', 100, 100)",
        [],
    )
    .unwrap();
    let transcripts = tmp.path().join("transcripts");
    write_artifacts(&transcripts, "v_badver", true, Some("999"));

    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--transcripts",
            transcripts.to_str().unwrap(),
            "status",
            "--verify",
            "--json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["verify"]["schema_version_mismatches"], 1);
    assert_eq!(v["verify"]["sample_mismatched"][0], "v_badver");
}

#[test]
fn verify_all_green_is_pause_safe_and_exits_0() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    {
        let _s = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('v_ok9', 'https://example/v_ok9', 1, 'succeeded', 100, 100)",
        [],
    )
    .unwrap();
    let transcripts = tmp.path().join("transcripts");
    write_artifacts(&transcripts, "v_ok9", true, Some("1"));

    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--transcripts",
            transcripts.to_str().unwrap(),
            "status",
            "--verify",
        ])
        .assert()
        .success()
        .stdout(contains("pause-safe: YES"));
}

#[test]
fn verify_conflicts_with_detail_modes() {
    Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["status", "--verify", "--video-id", "x"])
        .assert()
        .code(2); // clap usage error
}

#[test]
fn verify_counts_infra_fault_shards_unreadable_not_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db = tmp.path().join("state.sqlite");
    {
        let _s = ddp_transcribe::state::Store::open(&db).unwrap();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "INSERT INTO videos (video_id, source_url, canonical, status, first_seen_at, updated_at)
         VALUES ('v_ok1', 'https://example/v_ok1', 1, 'succeeded', 100, 100)",
        [],
    )
    .unwrap();
    let transcripts = tmp.path().join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    // shard('v_ok1') == "k1": plant a regular FILE at the shard path so
    // read_dir fails with a non-NotFound error (not-a-directory).
    std::fs::write(transcripts.join("k1"), b"not a directory").unwrap();

    let out = Command::cargo_bin("ddp-transcribe")
        .unwrap()
        .args([
            "--state-db",
            db.to_str().unwrap(),
            "--transcripts",
            transcripts.to_str().unwrap(),
            "status",
            "--verify",
            "--json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        v["verify"]["artifacts_missing"], 0,
        "infra fault must not read as data loss"
    );
    assert_eq!(v["verify"]["unreadable_artifacts"], 1);
    assert_eq!(v["verify"]["sample_unreadable"][0], "v_ok1");
}
