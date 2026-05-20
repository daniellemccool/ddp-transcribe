use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;

use uu_tiktok::errors::TranscribeError;
use uu_tiktok::fetcher::FakeFetcher;
use uu_tiktok::pipeline::{run_serial, ProcessOptions};
use uu_tiktok::state::Store;
use uu_tiktok::transcribe::{PerCallConfig, SegmentRaw, TokenRaw, TranscribeOutput, Transcriber};

/// In-test `Transcriber` impl with two behaviors:
/// - `Scripted(output)`: returns a scripted `TranscribeOutput` regardless of
///   the samples it receives. Lets us assert that the pipeline projects the
///   engine's output into the JSON artifact's `raw_signals` sub-object
///   correctly without loading a whisper.cpp model.
/// - `AlwaysFailsRetryable`: returns `Err(TranscribeError::EmptyOutput)` — a
///   non-Cancelled, non-Bug variant that T17's `transcribe_worker`
///   classifies as a retryable failure (used for the stale-after-failure
///   counter test, symmetric to T16's `fetch_worker` test).
enum FakeBehavior {
    Scripted(TranscribeOutput),
    AlwaysFailsRetryable,
}

struct FakeTranscriber {
    behavior: FakeBehavior,
}

impl FakeTranscriber {
    /// Scripted output, mirrors the legacy constructor pattern.
    fn scripted(output: TranscribeOutput) -> Self {
        Self {
            behavior: FakeBehavior::Scripted(output),
        }
    }

    /// "Echo" transcriber: a minimal scripted output with empty text and a
    /// recognizable language tag. Used by T17's happy-path test where the
    /// transcript content isn't being asserted — only that the row reaches
    /// `succeeded` and artifacts are written per 0008.
    fn echo() -> Self {
        Self::scripted(TranscribeOutput {
            text: String::new(),
            language: "en".into(),
            lang_probs: None,
            segments: vec![],
            model_id: "fake-echo.bin".into(),
        })
    }

    /// Always fails with `TranscribeError::EmptyOutput` — a retryable-class
    /// variant (not Cancelled, not Bug). Used by T17's
    /// `transcribe_worker_increments_stale_after_failure_on_swept_claim` to
    /// drive the worker into the `mark_retryable_failure` branch.
    fn always_fails_retryable() -> Self {
        Self {
            behavior: FakeBehavior::AlwaysFailsRetryable,
        }
    }
}

#[async_trait]
impl Transcriber for FakeTranscriber {
    async fn transcribe(
        &self,
        _samples: Vec<f32>,
        _config: PerCallConfig,
        _timeout: Duration,
    ) -> Result<TranscribeOutput, TranscribeError> {
        match &self.behavior {
            FakeBehavior::Scripted(out) => Ok(out.clone()),
            FakeBehavior::AlwaysFailsRetryable => Err(TranscribeError::EmptyOutput),
        }
    }

    fn name(&self) -> &'static str {
        "fake-transcriber"
    }
}

/// Path to a known-good 16 kHz mono WAV fixture (`audio::decode_wav` requires
/// this exact format; using bytes that don't parse would fail before the
/// transcriber is called, defeating the projection assertions).
fn silence_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/silence_16khz_mono.wav")
}

#[tokio::test]
async fn pipeline_processes_one_video_to_succeeded_with_fake_fetcher() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    store
        .upsert_video("7234567890123456789", "fake://url", true)
        .unwrap();

    // Stage a real WAV fixture as the FakeFetcher's canned response. The
    // pipeline calls audio::decode_wav on this path; a raw "RIFF...." byte
    // string would fail format validation before the transcriber is invoked.
    let fake_wav = tmp.path().join("fake.wav");
    std::fs::copy(silence_fixture(), &fake_wav).expect("copy silence fixture");
    let map = HashMap::from([("7234567890123456789".to_string(), fake_wav.clone())]);
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
    };

    let transcriber = FakeTranscriber::scripted(TranscribeOutput {
        text: "hello fake world".into(),
        language: "en".into(),
        lang_probs: None,
        segments: vec![],
        model_id: "ggml-test.bin".into(),
    });

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(1),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(60),
        stale_claim_threshold: Duration::from_secs(30 * 60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let stats = run_serial(&mut store, &fetcher, &transcriber, opts)
        .await
        .expect("pipeline");
    assert_eq!(stats.succeeded, 1);
    assert_eq!(stats.failed, 0);

    let row = store
        .get_video_for_test("7234567890123456789")
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "succeeded");

    // Final artifacts present in the sharded directory.
    let txt = tmp.path().join("transcripts/89/7234567890123456789.txt");
    assert!(txt.exists(), "transcript file at {}", txt.display());
    let json = tmp.path().join("transcripts/89/7234567890123456789.json");
    assert!(json.exists(), "transcript metadata at {}", json.display());
    let json_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(
        json_value["model"], "ggml-test.bin",
        "model field reflects the transcriber's reported model_id (T11: \
         engine reports model per call; no more hardcoded transcript_model)"
    );
    assert_eq!(
        json_value["transcript_source"], "fake-transcriber",
        "transcript_source reflects the actual transcriber name (T11: \
         Transcriber::name() lands in metadata; no more hardcoded \"whisper.cpp\")"
    );
    assert_eq!(
        json_value["fetcher"], "fake-fetcher",
        "fetcher reflects the actual fetcher name (T11: VideoFetcher::name() \
         lands in metadata; no more hardcoded \"ytdlp\")"
    );
}

#[tokio::test]
async fn pipeline_writes_raw_signals_to_json_artifact() {
    let tmp = TempDir::new().unwrap();
    let mut store = Store::open(&tmp.path().join("state.sqlite")).unwrap();
    store
        .upsert_video("7234567890123456789", "fake://url", true)
        .unwrap();

    let fake_wav = tmp.path().join("fake.wav");
    std::fs::copy(silence_fixture(), &fake_wav).expect("copy silence fixture");
    let map = HashMap::from([("7234567890123456789".to_string(), fake_wav.clone())]);
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
    };

    // Scripted output with one realistic segment+token so the projection
    // round-trip is checkable end-to-end (token id, text, p, plog all
    // pass through to the artifact).
    let transcriber = FakeTranscriber::scripted(TranscribeOutput {
        text: "hello world".to_string(),
        language: "en".to_string(),
        lang_probs: None,
        segments: vec![SegmentRaw {
            no_speech_prob: 0.02,
            tokens: vec![TokenRaw {
                id: 50257,
                text: "\u{2581}hello".to_string(),
                p: 0.99,
                plog: -0.01,
            }],
        }],
        model_id: "fake-model.bin".to_string(),
    });

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(1),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(60),
        stale_claim_threshold: Duration::from_secs(30 * 60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let stats = run_serial(&mut store, &fetcher, &transcriber, opts)
        .await
        .expect("pipeline");
    assert_eq!(stats.succeeded, 1);

    // Find the written .json artifact.
    let json_path = tmp.path().join("transcripts/89/7234567890123456789.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json_path).expect("read artifact"))
            .expect("parse json");

    // Plan B Epic 1 (0010): raw_signals lands as a sub-object on the
    // metadata wire format, with schema_version="1".
    let rs = &parsed["raw_signals"];
    assert_eq!(rs["schema_version"], "1");
    assert_eq!(rs["language"], "en");

    // 0010: lang_probs is null (not absent) when not opted in — the
    // RawSignals struct has no skip_serializing_if on this field.
    assert!(
        rs.get("lang_probs").is_some(),
        "lang_probs key must be present even when None"
    );
    assert!(
        rs["lang_probs"].is_null(),
        "lang_probs must serialize as null when None"
    );

    // Segments + tokens round-trip the scripted values losslessly.
    let segments = rs["segments"].as_array().expect("segments array");
    assert_eq!(segments.len(), 1);
    assert!(
        (segments[0]["no_speech_prob"].as_f64().unwrap() - 0.02).abs() < 1e-6,
        "no_speech_prob round-trip"
    );

    let tokens = segments[0]["tokens"].as_array().expect("tokens array");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0]["id"], 50257);
    assert_eq!(tokens[0]["text"], "\u{2581}hello");
    assert!(
        (tokens[0]["p"].as_f64().unwrap() - 0.99).abs() < 1e-6,
        "token p round-trip"
    );
    assert!(
        (tokens[0]["plog"].as_f64().unwrap() - (-0.01)).abs() < 1e-6,
        "token plog round-trip"
    );

    // Provenance reflects the actual transcriber and fetcher (no more
    // hardcoded "whisper.cpp" / "ytdlp"; partial fix for FOLLOWUPS T14).
    assert_eq!(parsed["transcript_source"], "fake-transcriber");
    assert_eq!(parsed["fetcher"], "fake-fetcher");

    // model field reflects the transcriber's per-call model_id (no more
    // ProcessOptions::transcript_model literal).
    assert_eq!(parsed["model"], "fake-model.bin");
}

/// `run_serial` no longer aborts on first failure (Plan A behavior); it
/// classifies the failure as retryable and continues. This test confirms
/// the new behavior: a failing fetcher leaves the row as `failed_retryable`
/// and run_serial returns Ok(stats) with `failed >= 1`.
#[tokio::test]
async fn run_serial_classifies_fetch_failure_as_retryable_and_continues() -> anyhow::Result<()> {
    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.upsert_video("vid_b", "https://example/b", false)?;

    let fetcher = FakeFetcher::always_fails();
    let transcriber = FakeTranscriber::scripted(TranscribeOutput {
        text: "unused".into(),
        language: "en".into(),
        lang_probs: None,
        segments: vec![],
        model_id: "unused.bin".into(),
    });

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(2),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let stats = run_serial(&mut store, &fetcher, &transcriber, opts).await?;
    assert_eq!(stats.claimed, 2);
    assert_eq!(stats.succeeded, 0);
    assert_eq!(stats.failed, 2);

    // Both rows should be failed_retryable with the placeholder kind.
    // 1C enrichment: assert column values + retry-safety invariant at pipeline layer.
    for vid in ["vid_a", "vid_b"] {
        let row = store.get_video_for_test(vid)?.expect("row");
        assert_eq!(row.status, "failed_retryable", "video {vid}");
    }

    // 1C enrichment: assert kind, message, and claim-slot cleared (retry-safety
    // invariant) via raw SQL — mirrors the Store-layer assertion in
    // state_claims::mark_retryable_failure_flips_status_and_records_columns
    // but now exercised end-to-end through the pipeline.
    let raw = rusqlite::Connection::open(tmp.path().join("state.sqlite"))?;
    for vid in ["vid_a", "vid_b"] {
        let (rk, rm, cb, ca): (Option<String>, Option<String>, Option<String>, Option<i64>) = raw
            .query_row(
            "SELECT last_retryable_kind, last_retryable_message, claimed_by, claimed_at
                 FROM videos WHERE video_id = ?1",
            [vid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        assert_eq!(
            rk.as_deref(),
            Some("FetchOrTranscribe"),
            "video {vid}: placeholder kind"
        );
        let msg = rm.expect("last_retryable_message populated");
        assert!(
            !msg.is_empty(),
            "video {vid}: last_retryable_message must carry the error chain"
        );
        assert_eq!(
            cb, None,
            "video {vid}: claimed_by must be NULL after retryable flip (retry-safety)"
        );
        assert_eq!(
            ca, None,
            "video {vid}: claimed_at must be NULL after retryable flip (retry-safety)"
        );
    }
    Ok(())
}

/// 1C (symmetric): a failing transcriber leaves both rows as `failed_retryable`
/// with the same placeholder kind as the fetch-failure variant. Confirms both
/// arms (fetch and transcribe) route through the same Err branch in `run_serial`.
#[tokio::test]
async fn run_serial_classifies_transcribe_failure_as_retryable_and_continues() -> anyhow::Result<()>
{
    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.upsert_video("vid_b", "https://example/b", false)?;

    // Stage real WAVs so fetch succeeds; only the transcriber fails.
    let fake_wav_a = tmp.path().join("vid_a.wav");
    let fake_wav_b = tmp.path().join("vid_b.wav");
    std::fs::copy(silence_fixture(), &fake_wav_a)?;
    std::fs::copy(silence_fixture(), &fake_wav_b)?;
    let map = HashMap::from([
        ("vid_a".to_string(), fake_wav_a.clone()),
        ("vid_b".to_string(), fake_wav_b.clone()),
    ]);
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
    };
    let transcriber = FakeTranscriber::always_fails_retryable();

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(2),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let stats = run_serial(&mut store, &fetcher, &transcriber, opts).await?;
    assert_eq!(stats.claimed, 2);
    assert_eq!(stats.succeeded, 0);
    assert_eq!(stats.failed, 2);

    let raw = rusqlite::Connection::open(tmp.path().join("state.sqlite"))?;
    for vid in ["vid_a", "vid_b"] {
        let (status, rk, rm, cb, ca): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        ) = raw.query_row(
            "SELECT status, last_retryable_kind, last_retryable_message, claimed_by, claimed_at
             FROM videos WHERE video_id = ?1",
            [vid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        assert_eq!(status, "failed_retryable", "video {vid}");
        assert_eq!(
            rk.as_deref(),
            Some("FetchOrTranscribe"),
            "video {vid}: same placeholder kind regardless of which arm failed"
        );
        assert!(
            rm.as_ref().map_or(false, |m| !m.is_empty()),
            "video {vid}: last_retryable_message populated"
        );
        assert_eq!(
            cb, None,
            "video {vid}: claimed_by cleared after retryable flip"
        );
        assert_eq!(
            ca, None,
            "video {vid}: claimed_at cleared after retryable flip"
        );
    }
    Ok(())
}

/// T16: a single `fetch_worker` claims every pending row, decodes audio,
/// emits a `FetchedItem` per row onto the channel, then exits cleanly when
/// `claim_next` returns `None` (drain semantics per 0026 — no polling).
/// Plain `#[tokio::test]` (current_thread runtime) per the operator's
/// `TOKIO_WORKER_THREADS=1` policy; cooperative `.await` interleaves the
/// spawned worker with the channel drain.
#[tokio::test]
async fn fetch_worker_drains_pending_rows_and_exits() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    store.upsert_video("vid_b", "https://example/b", false)?;

    // Stage real WAV fixtures for the FakeFetcher — `fetch_and_decode` calls
    // `audio::decode_wav` which requires a valid 16 kHz mono WAV.
    let fake_wav_a = tmp.path().join("vid_a.wav");
    let fake_wav_b = tmp.path().join("vid_b.wav");
    std::fs::copy(silence_fixture(), &fake_wav_a)?;
    std::fs::copy(silence_fixture(), &fake_wav_b)?;
    let map = HashMap::from([
        ("vid_a".to_string(), fake_wav_a.clone()),
        ("vid_b".to_string(), fake_wav_b.clone()),
    ]);
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
    };

    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let (tx, mut rx) = mpsc::channel::<FetchedItem>(2);
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        Arc::clone(&shared),
        Arc::new(fetcher),
        tx,
        Arc::clone(&stats_stale_after_failure),
        Arc::new(opts),
    ));

    // Drain the channel — should get 2 items, then None when the worker drops
    // its sender on clean exit (claim_next == None per 0026).
    let mut items = Vec::new();
    while let Some(item) = rx.recv().await {
        items.push(item);
    }
    assert_eq!(items.len(), 2, "two pending rows → two channel items");

    // Sanity-check the payload — claim + samples + samples_len + wav_path
    // ride together (T15 helper output).
    for item in &items {
        assert!(item.samples_len > 0, "decoded samples must be non-empty");
        assert_eq!(item.samples.len(), item.samples_len);
        assert!(item.wav_path.exists(), "wav_path must still exist on disk");
        assert!(
            ["vid_a", "vid_b"].contains(&item.claim.video_id.as_str()),
            "claim.video_id matches the upsert set"
        );
        // T18 Decision B: fetcher_name rides through FetchedItem from
        // VideoFetcher::name(); FakeFetcher reports "fake-fetcher".
        assert_eq!(
            item.fetcher_name, "fake-fetcher",
            "FetchedItem.fetcher_name reflects the actual fetcher name"
        );
    }

    let worker_result = worker_handle.await.expect("join");
    assert!(worker_result.is_ok(), "fetch_worker returns Ok on drain");
    assert_eq!(
        stats_stale_after_failure.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "happy path must not increment the stale-after-failure counter"
    );

    Ok(())
}

/// T16 design-amendment: when `mark_retryable_failure` returns `Ok(0)`
/// (the worker's claim was swept mid-flight and the row is no longer in
/// `in_progress AND claimed_by=worker`), `fetch_worker` increments the
/// `stats_stale_after_failure` counter and continues — it does NOT return
/// Err. Symmetric to `process_one`'s `StaleAfterSuccess` outcome on the
/// success side.
///
/// Forces the race deterministically via `FakeFetcher::gated_then_always_fails`:
/// iteration 1's fetch awaits a Notify; the test main task acquires the
/// shared Store lock, sweeps the row back to pending with `Duration::ZERO`,
/// drops the lock, then fires `notify_one`. The fetcher returns Err and
/// the worker's `mark_retryable_failure` predicate misses (the row's
/// `claimed_by` is now NULL) → `Ok(0)` → counter++. The brief's suggested
/// pre-claim-with-different-worker-id mechanism doesn't work (`fetch_worker`
/// only calls `mark_retryable_failure` after its OWN `claim_next` succeeds,
/// so the predicate would always match its own worker_id); see commit body
/// for the 0003 deviation note.
#[tokio::test]
async fn fetch_worker_increments_stale_after_failure_on_swept_claim() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let (fetcher, gate) = FakeFetcher::gated_then_always_fails();
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    // capacity 1; the fetcher never produces a `FetchedItem` (always fails),
    // so capacity is irrelevant here.
    let (tx, mut rx) = mpsc::channel::<FetchedItem>(1);
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "fetcher-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let counter_handle = Arc::clone(&stats_stale_after_failure);
    let shared_for_worker = Arc::clone(&shared);
    let worker_handle = tokio::spawn(fetch_worker(
        token.clone(),
        shared_for_worker,
        Arc::new(fetcher),
        tx,
        counter_handle,
        Arc::new(opts),
    ));

    // Wait until the worker has claimed the row and entered the gated
    // fetcher. unix_now() is second-resolution, so wait ≥1s past the claim
    // before sweeping (so `claimed_at < now` after the sweep cutoff).
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // Sweep on the shared store (the worker has dropped the mutex guard;
    // it's blocked on the gate inside fetcher.acquire). Duration::ZERO →
    // cutoff = now, so the row's claimed_at < cutoff flips it to pending.
    {
        let mut guard = shared.lock().await;
        let swept = guard.sweep_stale_claims(Duration::ZERO)?;
        assert_eq!(swept, 1, "row must sweep back to pending");
    }

    // Release the fetcher → returns Err → mark_retryable_failure → Ok(0).
    gate.notify_one();

    // Drain the channel. No FetchedItem should ever arrive (every fetch
    // fails); the worker exits when claim_next returns None (row is now
    // failed_retryable after iteration 2's successful flip).
    while let Some(_item) = rx.recv().await {
        panic!("no successful fetch expected; FakeFetcher always fails");
    }

    let worker_result = worker_handle.await.expect("join");
    assert!(
        worker_result.is_ok(),
        "Ok(0) is not a Bug — worker must NOT return Err: {worker_result:?}"
    );
    assert_eq!(
        stats_stale_after_failure.load(Ordering::Relaxed),
        1,
        "exactly one Ok(0) → counter incremented once"
    );

    Ok(())
}

/// T17: a single `transcribe_worker` drains one `FetchedItem` from the
/// channel, transcribes → writes artifacts → marks the row succeeded
/// (0008 invariant: artifacts on disk before mark_succeeded), then exits
/// cleanly when the sender is dropped (channel closed). Plain
/// `#[tokio::test]` (current_thread runtime) per the operator's
/// `TOKIO_WORKER_THREADS=1` policy.
#[tokio::test]
async fn transcribe_worker_processes_one_item_then_exits_on_channel_close() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{transcribe_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    // First open: upsert + claim the row so the worker's mark_succeeded
    // predicate (status='in_progress' AND claimed_by='worker-1') matches.
    let mut store_setup = Store::open(&tmp.path().join("state.sqlite"))?;
    store_setup.upsert_video("vid_a", "https://example/a", false)?;
    let claim_record = store_setup.claim_next("worker-1")?.expect("claim");
    drop(store_setup);

    // Re-open the same DB for the SharedStore handed to the worker.
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let transcriber = FakeTranscriber::echo();
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "worker-1".into(), // matches the claim's worker_id
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let (tx, rx) = mpsc::channel::<FetchedItem>(2);

    // Synthesize a wav file the worker can clean up. `audio::decode_wav`
    // is NOT called by the transcribe worker — the samples are already
    // decoded inside the FetchedItem — so the wav contents don't have to
    // be a real WAV; just a path the worker can `std::fs::remove_file`.
    let wav_path = tmp.path().join("synth.wav");
    std::fs::write(&wav_path, b"not a real wav")?;

    let samples = vec![0.0_f32; 16_000]; // 1 second of silence at 16 kHz
    let samples_len = samples.len();
    let item = FetchedItem {
        claim: claim_record,
        samples,
        samples_len,
        wav_path: wav_path.clone(),
        fetcher_name: "fake-fetcher",
    };
    tx.send(item).await.unwrap();
    drop(tx); // close channel after first item → worker exits after processing

    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::new(opts),
    ));

    let result = worker_handle.await.expect("join")?;
    assert_eq!(result, ());

    // Confirm vid_a is now succeeded.
    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row present");
    assert_eq!(row.status, "succeeded");
    drop(guard);

    // 0008 invariant: artifacts on disk (written before mark_succeeded).
    // shard("vid_a") → last two chars "_a" per src/output::shard.
    let txt = tmp.path().join("transcripts/_a/vid_a.txt");
    let json = tmp.path().join("transcripts/_a/vid_a.json");
    assert!(txt.exists(), "transcript .txt at {}", txt.display());
    assert!(json.exists(), "transcript .json at {}", json.display());

    // Wav was cleaned up after the DB commit.
    assert!(
        !wav_path.exists(),
        "wav must be removed after mark_succeeded"
    );

    // Happy path: stale-after-failure counter must stay at zero.
    assert_eq!(
        stats_stale_after_failure.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "happy path must not increment the stale-after-failure counter"
    );
    // Happy path: stale-after-success counter must stay at zero too.
    assert_eq!(
        stats_stale_after_success.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "happy path must not increment the stale-after-success counter"
    );

    Ok(())
}

/// T17: with no items in the channel, `transcribe_worker` exits within 2s
/// of `token.cancel()` (the loop-top `biased` select prefers the
/// cancellation arm; per 0025 this is the propagation entry point — at
/// next iteration's loop top after the in-flight transcribe completes,
/// or immediately if the worker was parked on `receiver.recv()`).
#[tokio::test]
async fn transcribe_worker_exits_on_cancellation() -> anyhow::Result<()> {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{transcribe_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let transcriber = FakeTranscriber::echo();
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "w".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let (_tx, rx) = mpsc::channel::<FetchedItem>(2);
    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::new(opts),
    ));

    // Fire cancellation; worker should exit promptly (parked on recv()).
    token.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), worker_handle).await;
    assert!(
        result.is_ok(),
        "worker should exit within 2s of cancellation"
    );
    // The inner future returned Ok(()) — no Bug.
    let join = result.unwrap().expect("join");
    assert!(join.is_ok(), "worker returns Ok on cancellation: {join:?}");

    Ok(())
}

/// T17 design-amendment (symmetric to T16's stale-after-failure test):
/// when the transcribe worker's claim is swept between `claim_next`
/// (performed by the test driver here, since `transcribe_worker` doesn't
/// claim — it consumes already-claimed FetchedItems) and the worker's
/// `mark_retryable_failure` call, the predicate
/// `status='in_progress' AND claimed_by=?` misses → `Ok(0)` → counter++.
/// The worker continues; it does NOT return Err.
///
/// Forcing the race: pre-claim the row with worker_id="worker-1", sleep
/// past the second-resolution timestamp boundary, sweep with
/// `Duration::ZERO` so the row flips back to pending with `claimed_by`
/// cleared. Send the (now-stale) Claim to the worker via a FetchedItem;
/// the FakeTranscriber returns `EmptyOutput` (a retryable variant);
/// `mark_retryable_failure` sees no matching row → returns Ok(0) → the
/// counter increments.
#[tokio::test]
async fn transcribe_worker_increments_stale_after_failure_on_swept_claim() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex as TokioMutex};
    use tokio_util::sync::CancellationToken;

    use uu_tiktok::pipeline::{transcribe_worker, FetchedItem, ProcessOptions, SharedStore};

    let tmp = TempDir::new()?;
    std::fs::create_dir_all(tmp.path().join("transcripts"))?;

    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;
    let claim_record = store.claim_next("worker-1")?.expect("claim");

    // unix_now() has second resolution — sleep ≥ 1s past the claim so
    // `claimed_at < cutoff` when sweep runs with Duration::ZERO.
    std::thread::sleep(Duration::from_millis(1100));
    let swept = store.sweep_stale_claims(Duration::ZERO)?;
    assert_eq!(swept, 1, "row must sweep back to pending");
    drop(store);

    // Re-open for the SharedStore handed to the worker.
    let store = Store::open(&tmp.path().join("state.sqlite"))?;
    let shared: SharedStore = Arc::new(TokioMutex::new(store));
    let transcriber = FakeTranscriber::always_fails_retryable();
    let token = CancellationToken::new();
    let stats_stale_after_failure = Arc::new(AtomicUsize::new(0));
    let opts = ProcessOptions {
        worker_id: "worker-1".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let (tx, rx) = mpsc::channel::<FetchedItem>(2);
    let wav_path = tmp.path().join("synth.wav");
    std::fs::write(&wav_path, b"not a real wav")?;
    let samples = vec![0.0_f32; 16_000];
    let samples_len = samples.len();
    tx.send(FetchedItem {
        claim: claim_record,
        samples,
        samples_len,
        wav_path,
        fetcher_name: "fake-fetcher",
    })
    .await
    .unwrap();
    drop(tx); // close after item → worker exits after processing

    let stats_stale_after_success = Arc::new(AtomicUsize::new(0));
    let worker_handle = tokio::spawn(transcribe_worker(
        token.clone(),
        rx,
        Arc::new(transcriber),
        Arc::clone(&shared),
        Arc::clone(&stats_stale_after_failure),
        Arc::clone(&stats_stale_after_success),
        Arc::new(opts),
    ));

    let worker_result = worker_handle.await.expect("join");
    assert!(
        worker_result.is_ok(),
        "Ok(0) is not a Bug — worker must NOT return Err: {worker_result:?}"
    );
    assert_eq!(
        stats_stale_after_failure.load(Ordering::Relaxed),
        1,
        "exactly one Ok(0) → counter incremented once"
    );
    // Failure path: stale-after-success counter must stay at zero
    // (the worker hit the retryable arm, not the success arm).
    assert_eq!(
        stats_stale_after_success.load(Ordering::Relaxed),
        0,
        "failure path must not increment the stale-after-success counter"
    );

    // Row sits in pending (the sweep left it there; the swept-claim
    // mark_retryable_failure UPDATE updated 0 rows so the status stays
    // 'pending').
    let guard = shared.lock().await;
    let row = guard.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "pending");
    drop(guard);

    Ok(())
}

/// T18: end-to-end `run_pipelined` happy-path drain. 6 pending rows +
/// FakeFetcher::happy + FakeTranscriber::echo + N=3 fetch workers → all
/// 6 rows reach `succeeded`; `ProcessStats { claimed: 6, succeeded: 6,
/// failed: 0, stale_after_success: 0, stale_after_failure: 0 }`.
///
/// This is the supervision wiring smoke test: spawns the full
/// `JoinSet` + `CancellationToken` topology per 0025 and asserts clean
/// drain on `claim_next == None` for every worker (0026).
#[tokio::test]
async fn run_pipelined_drains_all_rows_and_returns_stats() -> anyhow::Result<()> {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    use uu_tiktok::pipeline::{run_pipelined, ProcessOptions, SharedStore};

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
