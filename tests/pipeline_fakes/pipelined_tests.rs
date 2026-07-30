//! `run_pipelined` orchestration tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tempfile::TempDir;

use ddp_transcribe::fetcher::FakeFetcher;
use ddp_transcribe::state::Store;
use ddp_transcribe::transcribe::Transcriber;

use crate::fakes::{silence_fixture, FakeTranscriber};

/// Operator-checkpoint shim: an absolute-path `/bin/sh` script that appends
/// one line to a sentinel file and then exits with `exit_code`. The sentinel
/// is the out-of-process witness that the hook really ran (as opposed to the
/// in-process counters, which are the thing under test); the `exit_code` knob
/// drives both the success and the failure test off one helper.
fn checkpoint_shim(dir: &TempDir, exit_code: i32) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let sentinel = dir.path().join(format!("checkpoints-{exit_code}.log"));
    let script = dir.path().join(format!("checkpoint-{exit_code}.sh"));
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho tick >> {}\nexit {exit_code}\n",
            sentinel.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, sentinel)
}

/// Lines appended to a checkpoint sentinel so far (0 when the hook never ran
/// and the file therefore doesn't exist).
fn sentinel_lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// A `FakeFetcher` that serves `map` but parks its FIRST `acquire` on the
/// returned `Notify`. The checkpoint hook is a wall-clock timer, so these
/// tests need the run held open for a controlled window — gating the single
/// fetch worker inside `acquire` does that without a sleep-in-the-fake knob
/// and without making the assertion depend on transcribe throughput.
fn gated_canned_fetcher(
    map: HashMap<String, PathBuf>,
) -> (FakeFetcher, std::sync::Arc<tokio::sync::Notify>) {
    let gate = std::sync::Arc::new(tokio::sync::Notify::new());
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(Some(gate.clone())),
        canned_stderr: std::sync::Mutex::new(None),
        received_opts: std::sync::Mutex::new(Vec::new()),
        fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
        canned_metadata: std::sync::Mutex::new(None),
    };
    (fetcher, gate)
}

/// Task 04: the periodic operator checkpoint hook fires once per
/// `--checkpoint-every` while the batch runs, and stops when the
/// orchestrator's `CancellationToken` fires at drain. Sleep-then-run: no
/// firing at t=0 (the run boundary already syncs), so a ~1 s run at a 300 ms
/// interval sees at least two.
#[tokio::test]
async fn checkpoint_hook_fires_periodically_and_stops_on_cancel() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, CheckpointConfig, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;
    let db = tmp.path().join("state.sqlite");

    let mut store = Store::open(&db)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;
    drop(store);

    let (script, sentinel) = checkpoint_shim(&tmp, 0);
    let (fetcher, gate) = gated_canned_fetcher(HashMap::from([("vid_a".to_string(), wav)]));
    let fetcher = Arc::new(fetcher);
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());
    let shared: SharedStore = Arc::new(TokioMutex::new(Store::open(&db)?));

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
        checkpoint: Some(CheckpointConfig {
            cmd: script,
            every: Duration::from_millis(300),
        }),
    };

    // Hold the one fetch worker inside `acquire` for ~1 s (three intervals),
    // then release it so the run drains normally.
    let release = async {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        gate.notify_one();
    };
    let (stats, ()) = tokio::join!(
        run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts),
        release
    );
    let stats = stats?;

    assert!(sentinel.exists(), "the hook must have run at least once");
    let ticks = sentinel_lines(&sentinel);
    assert!(
        ticks >= 2,
        "a ~1 s run at a 300 ms interval must fire the hook at least twice, saw {ticks}"
    );
    assert!(
        stats.checkpoints_run >= 2,
        "checkpoints_run must count every successful firing, got {}",
        stats.checkpoints_run
    );
    assert_eq!(
        u64::try_from(ticks)?,
        stats.checkpoints_run,
        "every counted run corresponds to one completed hook invocation"
    );
    assert_eq!(stats.checkpoints_failed, 0, "the exit-0 shim never fails");
    assert_eq!(stats.succeeded, 1, "the batch still drained its one video");

    // ...and stops: the checkpoint task exits on the orchestrator's cancel,
    // so no further ticks land after `run_pipelined` resolved.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        sentinel_lines(&sentinel),
        ticks,
        "the hook must stop firing once the run is cancelled/drained"
    );

    Ok(())
}

/// Task 04 (ADR-0025): a failing hook is a warn + a counter, never an `Err`.
/// An `Err` from the checkpoint task would trip the orchestrator's
/// first-error `token.cancel()` and kill the whole batch, so the run must
/// finish its videos normally with the failures merely counted.
#[tokio::test]
async fn checkpoint_hook_failure_never_aborts_the_run() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use ddp_transcribe::pipeline::{run_pipelined, CheckpointConfig, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;
    let db = tmp.path().join("state.sqlite");

    let mut store = Store::open(&db)?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &wav)?;
    drop(store);

    let (script, sentinel) = checkpoint_shim(&tmp, 1);
    let (fetcher, gate) = gated_canned_fetcher(HashMap::from([("vid_a".to_string(), wav)]));
    let fetcher = Arc::new(fetcher);
    let transcriber: Arc<dyn Transcriber> = Arc::new(FakeTranscriber::echo());
    let shared: SharedStore = Arc::new(TokioMutex::new(Store::open(&db)?));

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
        checkpoint: Some(CheckpointConfig {
            cmd: script,
            every: Duration::from_millis(150),
        }),
    };

    let release = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        gate.notify_one();
    };
    let (stats, ()) = tokio::join!(
        run_pipelined(Arc::clone(&shared), fetcher, transcriber, opts),
        release
    );
    let stats = stats?;

    assert!(
        sentinel_lines(&sentinel) >= 1,
        "the exit-1 shim must actually have been invoked"
    );
    assert!(
        stats.checkpoints_failed >= 1,
        "a nonzero-exit hook counts as failed, got {}",
        stats.checkpoints_failed
    );
    assert_eq!(
        stats.checkpoints_run, 0,
        "a nonzero-exit hook never counts as a successful run"
    );
    assert_eq!(
        stats.succeeded, 1,
        "hook failures must not cancel the workers — the video still succeeded"
    );
    assert_eq!(stats.failed, 0, "no video-level failure was dispatched");

    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "succeeded");

    Ok(())
}

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
        checkpoint: None,
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
        checkpoint: None,
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
        checkpoint: None,
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
        checkpoint: None,
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
