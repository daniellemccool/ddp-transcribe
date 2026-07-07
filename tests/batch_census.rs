#![allow(clippy::unwrap_used, clippy::expect_used)]

//! End-to-end pin of the batch_runs provenance chain (Epic 4a T07): open a
//! run, sweep parked failures, close the run with the resulting census, and
//! read the row back — without driving the full binary.

use ddp_transcribe::batch::{self, BatchCensus, RunCensus};
use ddp_transcribe::classification::ClassificationTable;
use ddp_transcribe::state::Store;
use tempfile::TempDir;

fn seed_parked(store: &mut Store, tmp: &TempDir, id: &str, kind: &str, msg: &str, attempts: i64) {
    store
        .upsert_video(id, &format!("https://example/{id}"), false)
        .unwrap();
    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    conn.execute(
        "UPDATE videos SET status='failed_retryable', last_retryable_kind=?2,
         last_retryable_message=?3, attempt_count=?4 WHERE video_id=?1",
        rusqlite::params![id, kind, msg, attempts],
    )
    .unwrap();
}

#[test]
fn batch_lifecycle_persists_provenance_chain() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    let table = ClassificationTable::compiled_default().unwrap();

    // One write-off row, one recoverable row — same fixtures as src/batch.rs's tests.
    seed_parked(
        &mut store,
        &tmp,
        "v_dead",
        "Fetch",
        "ERROR: Your IP address is blocked, sad",
        1,
    );
    seed_parked(
        &mut store,
        &tmp,
        "v_alive",
        "Fetch",
        "ERROR: Did not get any data blocks",
        1,
    );

    // main-arm order: open_batch_run → run_sweep → (pipeline drain, elided
    // here) → close_batch_run.
    let params_json = serde_json::json!({"retries": 1, "max_videos": null}).to_string();
    let run_id = store
        .open_batch_run(&params_json, table.source_toml())
        .unwrap();

    let sweep_stats = batch::run_sweep(&mut store, &table, 1, false).unwrap();
    assert_eq!(sweep_stats.swept_terminal, 1);
    assert_eq!(sweep_stats.requeued_for_retry, 1);

    let census = BatchCensus {
        sweep: sweep_stats,
        run: RunCensus {
            claimed: 0,
            succeeded: 0,
            failed: 0,
            requeued_for_retry: 0,
            exhausted_retries: 0,
            parked_for_cookies: 0,
            terminal_by_label: std::collections::BTreeMap::new(),
            stale_after_success: 0,
            stale_after_failure: 0,
        },
    };
    let census_json = serde_json::to_string(&census).unwrap();
    let closed = store.close_batch_run(run_id, &census_json).unwrap();
    assert_eq!(closed, 1);

    let conn = rusqlite::Connection::open(tmp.path().join("state.sqlite")).unwrap();
    let (policy_toml, census_json_stored, finished_at): (String, Option<String>, Option<i64>) =
        conn.query_row(
            "SELECT policy_toml, census_json, finished_at FROM batch_runs WHERE run_id = ?1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(policy_toml, table.source_toml());
    let parsed: serde_json::Value = serde_json::from_str(&census_json_stored.unwrap()).unwrap();
    assert_eq!(parsed["sweep"]["swept_terminal"], 1);
    assert!(finished_at.is_some());
}
