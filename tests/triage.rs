#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::probe::ProbeVerdict;
use ddp_transcribe::state::Store;
use ddp_transcribe::triage::{run_triage, TriageOptions};
use tempfile::TempDir;

fn fresh_store() -> (Store, TempDir) {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    (store, tmp)
}

fn seed_failed(store: &mut Store, video_id: &str, kind: &str, message: &str) {
    store
        .upsert_video(video_id, "https://example.com/v", true)
        .unwrap();
    let _claim = store.claim_next("seed-worker").unwrap();
    store
        .mark_retryable_failure(video_id, "seed-worker", kind, message)
        .unwrap();
}

fn status_of(store: &Store, video_id: &str) -> String {
    store.get_video_for_test(video_id).unwrap().unwrap().status
}

fn kind_of(store: &Store, video_id: &str) -> Option<String> {
    store
        .get_video_for_test(video_id)
        .unwrap()
        .unwrap()
        .last_retryable_kind
}

struct FakeOracle {
    verdicts: std::collections::HashMap<String, ddp_transcribe::probe::ProbeVerdict>,
    probed: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl ddp_transcribe::probe::ProbeOracle for FakeOracle {
    async fn probe(&self, video_id: &str) -> ddp_transcribe::probe::ProbeVerdict {
        self.probed.lock().unwrap().push(video_id.to_string());
        self.verdicts.get(video_id).cloned().unwrap_or(
            ddp_transcribe::probe::ProbeVerdict::Unreachable("unconfigured".into()),
        )
    }
}

#[tokio::test]
async fn triage_routes_all_four_verdicts() {
    let (mut store, _tmp) = fresh_store();
    // Row A: stored message is a write-off class → terminal WITHOUT probe.
    seed_failed(
        &mut store,
        "7000000000000000021",
        "Fetch",
        "ERROR: Your IP address is blocked from accessing this post",
    );
    // Row B: retryable message, probe says Dead → terminal.
    seed_failed(
        &mut store,
        "7000000000000000022",
        "Fetch",
        "ERROR: Did not get any data blocks",
    );
    // Row C: retryable message, probe says Alive → requeued with normalized kind.
    seed_failed(
        &mut store,
        "7000000000000000023",
        "Fetch",
        "ERROR: [TikTok] x: This post may not be comfortable for some audiences. Log in for access.",
    );
    // Row D: probe Unreachable → untouched.
    seed_failed(
        &mut store,
        "7000000000000000024",
        "Fetch",
        "ERROR: Did not get any data blocks",
    );

    let oracle = FakeOracle {
        verdicts: [
            ("7000000000000000022".to_string(), ProbeVerdict::Dead),
            ("7000000000000000023".to_string(), ProbeVerdict::Alive),
            (
                "7000000000000000024".to_string(),
                ProbeVerdict::Unreachable("timeout".into()),
            ),
        ]
        .into_iter()
        .collect(),
        probed: Default::default(),
    };
    let stats = run_triage(
        &mut store,
        &oracle,
        &TriageOptions {
            dry_run: false,
            rate_per_sec: 1000.0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap();

    assert_eq!(status_of(&store, "7000000000000000021"), "failed_terminal");
    assert!(
        !oracle
            .probed
            .lock()
            .unwrap()
            .contains(&"7000000000000000021".to_string()),
        "write-off class must not be probed"
    );
    assert_eq!(status_of(&store, "7000000000000000022"), "failed_terminal");
    assert_eq!(status_of(&store, "7000000000000000023"), "pending");
    assert_eq!(
        kind_of(&store, "7000000000000000023").as_deref(),
        Some("SensitiveLoginGated"),
        "requeue must normalize the historical placeholder kind"
    );
    assert_eq!(status_of(&store, "7000000000000000024"), "failed_retryable");

    assert_eq!(stats.examined, 4);
    assert_eq!(stats.marked_terminal, 2);
    assert_eq!(stats.requeued, 1);
    assert_eq!(stats.kept_unreachable, 1);
}

#[tokio::test]
async fn triage_dry_run_mutates_nothing() {
    let (mut store, _tmp) = fresh_store();
    seed_failed(
        &mut store,
        "7000000000000000025",
        "Fetch",
        "ERROR: Your IP address is blocked",
    );
    let oracle = FakeOracle {
        verdicts: Default::default(),
        probed: Default::default(),
    };
    let stats = run_triage(
        &mut store,
        &oracle,
        &TriageOptions {
            dry_run: true,
            rate_per_sec: 1000.0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        stats.marked_terminal, 1,
        "dry run still REPORTS the verdict"
    );
    assert_eq!(
        status_of(&store, "7000000000000000025"),
        "failed_retryable",
        "…but mutates nothing"
    );
}

#[tokio::test]
async fn triage_respects_attempt_cap() {
    let (mut store, _tmp) = fresh_store();
    seed_failed(
        &mut store,
        "7000000000000000026",
        "Fetch",
        "ERROR: Did not get any data blocks",
    ); // attempt_count = 1
    let oracle = FakeOracle {
        verdicts: [("7000000000000000026".to_string(), ProbeVerdict::Alive)]
            .into_iter()
            .collect(),
        probed: Default::default(),
    };
    let stats = run_triage(
        &mut store,
        &oracle,
        &TriageOptions {
            dry_run: false,
            rate_per_sec: 1000.0,
            max_attempts: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(stats.kept_capped, 1);
    assert_eq!(status_of(&store, "7000000000000000026"), "failed_retryable");
}

/// Oracle that simulates a concurrent actor: on probe, it flips the row out
/// of failed_retryable via a second SQLite connection (WAL allows this), so
/// the subsequent triage_mark_terminal UPDATE's predicate misses. The row
/// remains in the snapshot run_triage already listed — exactly the race the
/// ADR-0006 row-change counts exist to detect.
struct FlipOnProbeOracle {
    db_path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl ddp_transcribe::probe::ProbeOracle for FlipOnProbeOracle {
    async fn probe(&self, video_id: &str) -> ddp_transcribe::probe::ProbeVerdict {
        let conn = rusqlite::Connection::open(&self.db_path).unwrap();
        conn.execute(
            "UPDATE videos SET status = 'succeeded' WHERE video_id = ?1",
            [video_id],
        )
        .unwrap();
        ProbeVerdict::Dead
    }
}

#[tokio::test]
async fn triage_predicate_miss_is_examined_but_not_counted_as_action() {
    let (mut store, tmp) = fresh_store();
    let db_path = tmp.path().join("state.sqlite");
    seed_failed(
        &mut store,
        "7000000000000000027",
        "Fetch",
        "ERROR: Did not get any data blocks",
    );
    let oracle = FlipOnProbeOracle { db_path };
    let stats = run_triage(
        &mut store,
        &oracle,
        &TriageOptions {
            dry_run: false,
            rate_per_sec: 1000.0,
            max_attempts: 3,
        },
    )
    .await
    .unwrap();

    // The row was in the listed snapshot, so it counts as examined; the
    // UPDATE predicate missed (row flipped to succeeded mid-pass), so no
    // action counter moves. The examined-vs-actions gap is the honest signal.
    assert_eq!(stats.examined, 1);
    assert_eq!(
        stats.marked_terminal, 0,
        "predicate miss must not be counted"
    );
    assert_eq!(stats.requeued, 0);
    assert_eq!(status_of(&store, "7000000000000000027"), "succeeded");
    let k = &stats.by_kind["NoDataBlocks"];
    assert_eq!(k.examined, 1);
    assert_eq!(k.marked_terminal, 0);
}
