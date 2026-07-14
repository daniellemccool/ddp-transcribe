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
