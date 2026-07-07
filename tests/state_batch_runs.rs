#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::state::Store;
use tempfile::TempDir;

fn fresh_store(tmp: &TempDir) -> Store {
    Store::open(&tmp.path().join("state.sqlite")).expect("open store")
}

#[test]
fn batch_run_opens_and_closes_with_census() {
    let tmp = TempDir::new().unwrap();
    let mut store = fresh_store(&tmp);

    let run_id = store
        .open_batch_run(r#"{"retries":1,"max_videos":null}"#, "schema = 1\n")
        .expect("open_batch_run");
    assert!(run_id >= 1);

    let changed = store
        .close_batch_run(run_id, r#"{"sweep":{"examined":0}}"#)
        .expect("close_batch_run");
    assert_eq!(changed, 1);

    // Raw read-back: row carries everything, finished_at is set.
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let (params, policy, census, finished): (String, String, Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT params_json, policy_toml, census_json, finished_at
             FROM batch_runs WHERE run_id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert!(params.contains("retries"));
    assert_eq!(policy, "schema = 1\n");
    assert!(census.unwrap().contains("examined"));
    assert!(finished.is_some());
}

#[test]
fn close_of_unknown_run_returns_zero() {
    let tmp = TempDir::new().unwrap();
    let mut store = fresh_store(&tmp);
    let changed = store.close_batch_run(9999, "{}").expect("close");
    assert_eq!(changed, 0, "0006: predicate miss reports 0, not an error");
}

#[test]
fn interrupted_run_leaves_finished_at_null() {
    let tmp = TempDir::new().unwrap();
    let mut store = fresh_store(&tmp);
    let run_id = store.open_batch_run("{}", "schema = 1\n").expect("open");
    // No close — simulates a crash. finished_at must be NULL (honest record).
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let finished: Option<i64> = conn
        .query_row(
            "SELECT finished_at FROM batch_runs WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(finished.is_none());
}
