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
