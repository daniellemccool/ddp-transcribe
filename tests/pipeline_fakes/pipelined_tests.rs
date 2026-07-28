//! `run_pipelined` orchestration tests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tempfile::TempDir;

use ddp_transcribe::fetcher::FakeFetcher;
use ddp_transcribe::state::Store;
use ddp_transcribe::transcribe::Transcriber;

use crate::fakes::{silence_fixture, FakeTranscriber};

/// `run_pipelined` honors `--max-videos`: with 10 pending rows and
/// `max_videos=Some(3)`, exactly 3 rows are claimed and reach `succeeded`;
/// the remaining 7 rows stay `pending`. The cap check happens inside the
/// `Mutex<Store>` guard before `claim_next`, making the check + claim +
/// counter increment race-free across all fetch workers.
#[tokio::test]
async fn run_pipelined_honors_max_videos_cap() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    let mut map = HashMap::new();
    for i in 0..10 {
        let vid = format!("vid_{i}");
        store.upsert_video(&vid, &format!("https://example/{i}"), false)?;
        let wav = tmp.path().join(format!("{vid}.wav"));
        std::fs::copy(silence_fixture(), &wav)?;
        map.insert(vid, wav);
    }
    drop(store);

    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let fetcher = Arc::new(FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
    });
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(3),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
    };

    let stats = run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts).await?;
    assert_eq!(stats.succeeded, 3, "exactly 3 rows should succeed");
    assert_eq!(stats.failed, 0);

    // 7 rows must remain pending
    let guard = shared.lock().await;
    let mut pending_count = 0usize;
    let mut succeeded_count = 0usize;
    for i in 0..10 {
        let row = guard
            .get_video_for_test(&format!("vid_{i}"))?
            .expect("row present");
        match row.status.as_str() {
            "pending" => pending_count += 1,
            "succeeded" => succeeded_count += 1,
            other => panic!("unexpected status {other} for vid_{i}"),
        }
    }
    assert_eq!(succeeded_count, 3, "3 rows must be succeeded");
    assert_eq!(pending_count, 7, "7 rows must remain pending");

    Ok(())
}

/// End-to-end `run_pipelined` happy-path drain. 6 pending rows +
/// FakeFetcher::happy + FakeTranscriber::echo + N=3 fetch workers → all
/// 6 rows reach `succeeded`; `ProcessStats { claimed: 6, succeeded: 6,
/// failed: 0, stale_after_success: 0, stale_after_failure: 0 }`.
///
/// This is the supervision wiring smoke test: spawns the full `JoinSet` +
/// `CancellationToken` topology and asserts clean drain on `claim_next ==
/// None` for every worker.
#[tokio::test]
async fn run_pipelined_drains_all_rows_and_returns_stats() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    // Stage WAV fixtures + upsert 6 pending rows. FakeFetcher needs a
    // canned WAV per video_id (the helper's HashMap lookup); decode_wav
    // needs a real 16 kHz mono WAV.
    let mut map = HashMap::new();
    for i in 0..6 {
        let vid = format!("vid_{i}");
        store.upsert_video(&vid, &format!("https://example/{i}"), false)?;
        let wav = tmp.path().join(format!("{vid}.wav"));
        std::fs::copy(silence_fixture(), &wav)?;
        map.insert(vid, wav);
    }
    drop(store);

    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let fetcher = Arc::new(FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
    });
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
    };

    let stats = run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts).await?;
    assert_eq!(stats.claimed, 6);
    assert_eq!(stats.succeeded, 6);
    assert_eq!(stats.failed, 0);
    assert_eq!(stats.stale_after_success, 0);
    assert_eq!(stats.stale_after_failure, 0);

    let guard = shared.lock().await;
    for i in 0..6 {
        let row = guard.get_video_for_test(&format!("vid_{i}"))?.expect("row");
        assert_eq!(row.status, "succeeded", "video vid_{i} reached succeeded");
    }
    Ok(())
}

/// Epic 4c: the raw metadata envelope is persisted on the success path.
/// A video whose fetch produced a `MetadataCapture` leaves exactly one
/// `video_metadata_raw` row, and the metadata write does not disturb the
/// pipeline outcome (the row still reaches `succeeded`).
#[tokio::test]
async fn fetch_persists_metadata_raw_row_on_success() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;
    let db = tmp.path().join("state.sqlite");

    let mut store = Store::open(&db)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;
    let map = HashMap::from([("vid_a".to_string(), wav)]);
    drop(store);

    let store = Store::open(&db)?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let fetcher = Arc::new(FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(Some(
            r#"{"schema":1,"printed":"{\"id\":\"vid_a\"}"}"#.to_string(),
        )),
    });
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
    };

    let stats = run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts).await?;
    assert_eq!(stats.succeeded, 1);
    assert_eq!(stats.failed, 0);

    {
        let guard = shared.lock().await;
        let row = guard.get_video_for_test("vid_a")?.expect("row");
        assert_eq!(
            row.status, "succeeded",
            "metadata persistence must not disturb the outcome"
        );
    }

    let conn = rusqlite::Connection::open(&db)?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_metadata_raw WHERE video_id='vid_a'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(count, 1, "exactly one raw row for the video");
    let raw: String = conn.query_row(
        "SELECT raw_json FROM video_metadata_raw WHERE video_id='vid_a'",
        [],
        |r| r.get(0),
    )?;
    assert!(raw.contains("schema"), "envelope stored verbatim: {raw}");

    Ok(())
}

/// Epic 4c: the raw metadata envelope is persisted on the FAILURE path too
/// — the insert happens before outcome dispatch, so a video whose fetch
/// produced an envelope and then failed still leaves a row. The failure
/// outcome itself is exactly what this stderr produced before the epic:
/// the `HttpError` retryable fixture requeues once (retries=1 → two
/// attempts) and then exhausts to `failed_retryable`.
#[tokio::test]
async fn fetch_persists_metadata_raw_row_on_classified_failure() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;
    let db = tmp.path().join("state.sqlite");

    let mut store = Store::open(&db)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    drop(store);

    let store = Store::open(&db)?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let fetcher = FakeFetcher::fails_with_stderr(
        "ERROR: unable to download video data: HTTP Error 403: Forbidden",
    );
    *fetcher
        .canned_metadata
        .lock()
        .expect("canned_metadata mutex") =
        Some(r#"{"schema":1,"printed":"{\"id\":\"vid_a\"}"}"#.to_string());
    let fetcher = Arc::new(fetcher);
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());

    let opts = ProcessOptions {
        worker_id: "orchestrator".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 1,
        channel_capacity: 2,
        cookies_file: None,
        classification: std::sync::Arc::new(
            ddp_transcribe::classification::ClassificationTable::compiled_default()
                .expect("default table"),
        ),
        retries: 1,
    };

    let stats = run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts).await?;
    assert_eq!(stats.succeeded, 0);
    assert!(stats.failed > 0, "the fetch failures were dispatched");

    {
        let guard = shared.lock().await;
        let row = guard.get_video_for_test("vid_a")?.expect("row");
        assert_eq!(
            row.status, "failed_retryable",
            "retryable stderr exhausts to failed_retryable, unchanged by this epic"
        );
    }

    let conn = rusqlite::Connection::open(&db)?;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM video_metadata_raw WHERE video_id='vid_a'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(
        count, 1,
        "failure-path capture: the raw row exists (keyed upsert across both attempts)"
    );
    let raw: String = conn.query_row(
        "SELECT raw_json FROM video_metadata_raw WHERE video_id='vid_a'",
        [],
        |r| r.get(0),
    )?;
    assert!(raw.contains("schema"), "envelope stored verbatim: {raw}");

    Ok(())
}
