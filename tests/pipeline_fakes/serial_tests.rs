//! `run_serial` / `process_one`-level tests.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tempfile::TempDir;

use ddp_transcribe::fetcher::FakeFetcher;
use ddp_transcribe::pipeline::{run_serial, ProcessOptions};
use ddp_transcribe::state::Store;
use ddp_transcribe::transcribe::{SegmentRaw, TokenRaw, TranscribeOutput};

use crate::fakes::{silence_fixture, FakeTranscriber};

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
        canned_stderr: std::sync::Mutex::new(None),
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
        canned_stderr: std::sync::Mutex::new(None),
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

    // raw_signals lands as a sub-object on the metadata wire format, with
    // schema_version="1".
    let rs = &parsed["raw_signals"];
    assert_eq!(rs["schema_version"], "1");
    assert_eq!(rs["language"], "en");

    // lang_probs is null (not absent) when not opted in — the RawSignals
    // struct has no skip_serializing_if on this field.
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
    // hardcoded "whisper.cpp" / "ytdlp").
    assert_eq!(parsed["transcript_source"], "fake-transcriber");
    assert_eq!(parsed["fetcher"], "fake-fetcher");

    // model field reflects the transcriber's per-call model_id (no more
    // ProcessOptions::transcript_model literal).
    assert_eq!(parsed["model"], "fake-model.bin");
}

/// `run_serial` no longer aborts on first failure; it classifies the
/// failure as retryable and continues. This test confirms the behavior: a
/// failing fetcher leaves the row as `failed_retryable` and run_serial
/// returns Ok(stats) with `failed >= 1`.
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

    // Both rows should be failed_retryable with the taxonomy kind
    // (Epic 3 T07: FakeFetcher::always_fails emits FetchError::NetworkError,
    // which classify_fetch_error maps to RetryableKind::NetworkTransient —
    // the Epic 2 placeholder "FetchOrTranscribe" is gone).
    for vid in ["vid_a", "vid_b"] {
        let row = store.get_video_for_test(vid)?.expect("row");
        assert_eq!(row.status, "failed_retryable", "video {vid}");
    }

    // Assert kind, message, and claim-slot cleared (retry-safety invariant)
    // via raw SQL — mirrors the Store-layer assertion in
    // state_claims::mark_retryable_failure_flips_status_and_records_columns
    // but exercised end-to-end through the pipeline.
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
            Some("NetworkTransient"),
            "video {vid}: taxonomy kind (placeholder \"FetchOrTranscribe\" must be gone)"
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

/// Symmetric to the fetch-failure test: a failing transcriber leaves both
/// rows as `failed_retryable`. Epic 3 T07 (+ review fix): `process_one`'s
/// anyhow chain for a transcribe-side failure doesn't carry a
/// `FetchPhaseError` root (only `fetch_and_decode` produces one), so the
/// `FetchPhaseError` downcast misses; the `None` arm then walks the chain
/// for a `TranscribeError` (`EmptyOutput` here, wrapped below a
/// "transcribing …" context layer) and dispatches via
/// `classify_transcribe_error` → `RetryableKind::TranscribeOther`.
/// Confirms both arms (fetch and transcribe) route through the same Err
/// branch in `run_serial`, landing on different (but both non-placeholder)
/// kinds.
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
        canned_stderr: std::sync::Mutex::new(None),
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
            Some("TranscribeOther"),
            "video {vid}: default-cautious kind for a non-FetchPhaseError anyhow chain \
             (placeholder \"FetchOrTranscribe\" must be gone)"
        );
        assert!(
            rm.as_ref().is_some_and(|m| !m.is_empty()),
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

/// Epic 3 T07 (review fix 2): a fetch failure whose stderr matches a
/// write-off pattern (ADR 0033) routes through `run_serial`'s
/// `mark_terminal_failure` arm — the row lands `failed_terminal` with the
/// classifier's tag as `terminal_reason`, and `run_serial` itself returns
/// Ok (a write-off is a per-row verdict, not a Bug).
#[tokio::test]
async fn run_serial_writes_off_ip_blocked_as_terminal() -> anyhow::Result<()> {
    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let fetcher = FakeFetcher::fails_with_stderr(
        "ERROR: [TikTok] vid_a: Your IP address is blocked from accessing this post",
    );
    let transcriber = FakeTranscriber::echo(); // never reached; fetch fails

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(1),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let stats = run_serial(&mut store, &fetcher, &transcriber, opts).await?;
    assert_eq!(stats.claimed, 1);
    assert_eq!(stats.succeeded, 0);
    assert_eq!(stats.failed, 1, "write-off counts as a failure in stats");

    let row = store.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(row.status, "failed_terminal");
    assert_eq!(
        row.terminal_reason.as_deref(),
        Some("IpBlockedMessage"),
        "terminal_reason carries the classifier's write-off tag"
    );
    Ok(())
}

/// Epic 3 T07 (review fix 1): a transcribe-side `TranscribeError::Bug`
/// escalates out of `run_serial` as `Err` — it must NOT be silently
/// downgraded to a retryable mark. The row stays `in_progress` (no mutator
/// ran; a later sweep recovers it per 0024).
#[tokio::test]
async fn run_serial_escalates_transcribe_bug_as_err() -> anyhow::Result<()> {
    let tmp = TempDir::new()?;
    let mut store = Store::open(&tmp.path().join("state.sqlite"))?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    // Fetch succeeds (real WAV staged); only the transcriber fails, with
    // the Bug-class variant.
    let fake_wav = tmp.path().join("vid_a.wav");
    std::fs::copy(silence_fixture(), &fake_wav)?;
    let map = HashMap::from([("vid_a".to_string(), fake_wav)]);
    let fetcher = FakeFetcher {
        canned: Mutex::new(map),
        always_fails: false,
        first_call_gate: tokio::sync::Mutex::new(None),
        canned_stderr: std::sync::Mutex::new(None),
    };
    let transcriber = FakeTranscriber::always_fails_bug();

    let opts = ProcessOptions {
        worker_id: "test-worker".into(),
        transcripts_root: tmp.path().join("transcripts"),
        max_videos: Some(1),
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    };

    let result = run_serial(&mut store, &fetcher, &transcriber, opts).await;
    let err = result.expect_err("Bug-class transcribe failure must escalate as Err");
    assert!(
        err.to_string().contains("transcribe Bug for vid_a"),
        "error names the Bug and the row: {err:#}"
    );

    // No mutator ran: the row is still in_progress (claimed), not
    // failed_retryable/failed_terminal.
    let row = store.get_video_for_test("vid_a")?.expect("row");
    assert_eq!(
        row.status, "in_progress",
        "Bug escalation must not flip the row to a failure status"
    );
    assert_eq!(
        row.last_retryable_kind, None,
        "no retryable mark must be recorded for a Bug"
    );
    Ok(())
}
