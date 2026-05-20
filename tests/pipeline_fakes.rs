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

/// In-test `Transcriber` impl that returns a scripted `TranscribeOutput`
/// regardless of the samples it receives. Lets us assert that the pipeline
/// projects the engine's output into the JSON artifact's `raw_signals`
/// sub-object correctly without needing to load a whisper.cpp model.
struct FakeTranscriber {
    scripted: TranscribeOutput,
}

#[async_trait]
impl Transcriber for FakeTranscriber {
    async fn transcribe(
        &self,
        _samples: Vec<f32>,
        _config: PerCallConfig,
        _timeout: Duration,
    ) -> Result<TranscribeOutput, TranscribeError> {
        Ok(self.scripted.clone())
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

    let transcriber = FakeTranscriber {
        scripted: TranscribeOutput {
            text: "hello fake world".into(),
            language: "en".into(),
            lang_probs: None,
            segments: vec![],
            model_id: "ggml-test.bin".into(),
        },
    };

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
    let transcriber = FakeTranscriber {
        scripted: TranscribeOutput {
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
        },
    };

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
    let transcriber = FakeTranscriber {
        scripted: TranscribeOutput {
            text: "unused".into(),
            language: "en".into(),
            lang_probs: None,
            segments: vec![],
            model_id: "unused.bin".into(),
        },
    };

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
    for vid in ["vid_a", "vid_b"] {
        let row = store.get_video_for_test(vid)?.expect("row");
        assert_eq!(row.status, "failed_retryable", "video {vid}");
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
