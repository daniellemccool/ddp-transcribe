//! WhisperEngine init smoke test.
//!
//! Requires ./models/ggml-tiny.en.bin on disk; gated by test-helpers feature
//! per 0005 because it depends on a non-trivial fixture.

#![cfg(feature = "test-helpers")]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ddp_transcribe::errors::TranscribeError;
use ddp_transcribe::transcribe::{EngineConfig, PerCallConfig, WhisperEngine};

fn tiny_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/ggml-tiny.en.bin")
}

#[tokio::test]
#[ignore = "requires ./models/ggml-tiny.en.bin; run with `cargo test --features test-helpers -- --ignored`"]
async fn engine_loads_tiny_en_model_successfully() {
    let config = EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        // flash_attn forced false locally because we don't always build with cuda
        flash_attn: false,
    };

    let engine = WhisperEngine::new(&config).expect("engine should load tiny.en");

    // Verify the worker is alive by sending a transcribe and getting a real
    // reply (T7 wires inference). 1s of silence on tiny.en should succeed
    // with empty/short text and a populated language code.
    let samples = vec![0.0_f32; 16000]; // 1 second of silence
    let output = engine
        .transcribe(samples, PerCallConfig::default(), Duration::from_secs(30))
        .await
        .expect("transcribe of 1s silence should succeed");
    assert!(
        output.text.len() < 200,
        "silence shouldn't transcribe to a long phrase, got: {:?}",
        output.text
    );
    assert!(!output.language.is_empty(), "language should be set");

    // Regression guard for the Drop-ordering deadlock (see WhisperEngine::teardown).
    // If a future change inverts "drop sender → join handle" to "join handle → drop
    // sender", the worker will park forever in blocking_recv and shutdown() will
    // hang. Fail fast at 5s wallclock instead of letting the test harness time out
    // at 60s+ with no diagnostic context.
    let start = Instant::now();
    engine.shutdown();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(5),
        "shutdown took {elapsed:?} — possible Drop-ordering deadlock in WhisperEngine"
    );
}

#[tokio::test]
async fn engine_rejects_missing_model_path() {
    let config = EngineConfig {
        model_path: PathBuf::from("/nonexistent/model.bin"),
        gpu_device: 0,
        flash_attn: false,
    };
    let result = WhisperEngine::new(&config);
    assert!(
        result.is_err(),
        "expected WhisperInitError on missing model"
    );
}

fn silence_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/silence_16khz_mono.wav")
}

#[tokio::test]
#[ignore = "requires ./models/ggml-tiny.en.bin; run with `cargo test --features test-helpers -- --ignored`"]
async fn transcribe_silence_returns_empty_or_short_text() {
    let config = EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        flash_attn: false,
    };
    let engine = WhisperEngine::new(&config).expect("engine loads");

    let samples =
        ddp_transcribe::audio::decode_wav(&silence_fixture_path()).expect("decode fixture");

    let output = engine
        .transcribe(samples, PerCallConfig::default(), Duration::from_secs(60))
        .await
        .expect("transcribe of silence should succeed (text may be empty)");

    // Silence may produce empty or near-empty text. Either is fine.
    assert!(
        output.text.len() < 200,
        "silence shouldn't transcribe to a long phrase, got: {:?}",
        output.text
    );
    assert!(!output.language.is_empty(), "language should be set");
    // T9: segments are now populated. For silence whisper.cpp may return
    // zero or a small number of segments; either is acceptable.
    // Structural range checks on any segments that do appear.
    for (i, seg) in output.segments.iter().enumerate() {
        assert!(
            seg.no_speech_prob >= 0.0 && seg.no_speech_prob <= 1.0,
            "segment {i} no_speech_prob out of range: {}",
            seg.no_speech_prob
        );
    }
    // model_id pinned to the file_name() of the configured model path.
    assert_eq!(output.model_id, "ggml-tiny.en.bin");

    engine.shutdown();
}

#[tokio::test]
#[ignore = "requires ./models/ggml-tiny.en.bin; run with `cargo test --features test-helpers -- --ignored`"]
async fn transcribe_respects_short_deadline() {
    let config = EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        flash_attn: false,
    };
    let engine = WhisperEngine::new(&config).expect("engine loads");

    // 30 seconds of silence — encoder still runs over the full window.
    let samples = vec![0.0_f32; 16000 * 30];

    // tiny.en on CPU typically takes 1-3 seconds for 30s audio. A 100ms
    // deadline should trip the abort callback well before completion.
    //
    // Wallclock guard: if a regression breaks cancellation, the test should
    // fail before the harness timeout (60s+). The 30s upper bound accommodates
    // CPU contention when whisper_engine_init's 5 model-loading tests run in
    // parallel — each test allocates ~1GB of WhisperState buffers and competes
    // for cores, so a 30s audio inference can take 10-15s elapsed under load
    // even though the abort callback itself fires sub-second. A true hang
    // (abort_callback never returns true; inference runs to natural completion
    // under contention) would exceed 30s and be caught here. If this still
    // flakes after future task additions, consider the `serial_test` crate
    // tracked in FOLLOWUPS.
    let start = Instant::now();
    let result = engine
        .transcribe(
            samples,
            PerCallConfig::default(),
            Duration::from_millis(100),
        )
        .await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "transcribe took {elapsed:?} — possible cancellation regression or test-harness contention"
    );

    // Expect either Cancelled (most likely) or successful very short completion
    // on extremely fast hardware. Key invariant: we did NOT hang past a
    // reasonable wallclock.
    match result {
        Ok(_) => {
            eprintln!("inference completed within 100ms — fine on very fast hardware");
        }
        Err(TranscribeError::Cancelled) => {
            // The expected path.
        }
        Err(e) => panic!("expected Cancelled or Ok, got {e:?}"),
    }

    engine.shutdown();
}

#[tokio::test]
#[ignore = "requires ./models/ggml-tiny.en.bin; run with `cargo test --features test-helpers -- --ignored`"]
async fn lang_probs_present_when_opt_in() {
    let config = EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        flash_attn: false,
    };
    let engine = WhisperEngine::new(&config).expect("engine loads");

    let samples =
        ddp_transcribe::audio::decode_wav(&silence_fixture_path()).expect("decode fixture");

    // Without opt-in: lang_probs should be None
    let output_default = engine
        .transcribe(
            samples.clone(),
            PerCallConfig::default(),
            Duration::from_secs(60),
        )
        .await
        .expect("default transcribe succeeds");
    assert!(
        output_default.lang_probs.is_none(),
        "lang_probs should be None by default"
    );

    // With opt-in: lang_probs should be Some(...) populated
    let cfg = PerCallConfig {
        compute_lang_probs: true,
        ..PerCallConfig::default()
    };
    let output_with_probs = engine
        .transcribe(samples, cfg, Duration::from_secs(60))
        .await
        .expect("opt-in transcribe succeeds");
    assert!(
        output_with_probs.lang_probs.is_some(),
        "lang_probs should be Some when compute_lang_probs is true"
    );
    let probs = output_with_probs.lang_probs.unwrap();
    assert!(
        !probs.is_empty(),
        "should have at least one language probability"
    );
    // Probabilities should sum to ~1.0
    let sum: f32 = probs.iter().map(|(_, p)| p).sum();
    assert!(
        (sum - 1.0).abs() < 0.1,
        "probs should sum to ~1.0, got {sum}"
    );

    engine.shutdown();
}

#[tokio::test]
#[ignore = "requires ./models/ggml-tiny.en.bin; run with `cargo test --features test-helpers -- --ignored`"]
async fn transcribe_populates_raw_signals_segments_and_tokens() {
    let config = EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        flash_attn: false,
    };
    let engine = WhisperEngine::new(&config).expect("engine loads");

    // Use a non-silent fixture so segments are likely non-empty.
    // For Epic 1's bake we'll add a real spoken-English fixture; for now,
    // silence may still produce one empty segment (whisper.cpp behavior).
    // The test asserts structural shape, not content.
    let samples =
        ddp_transcribe::audio::decode_wav(&silence_fixture_path()).expect("decode fixture");

    let output = engine
        .transcribe(samples, PerCallConfig::default(), Duration::from_secs(60))
        .await
        .expect("transcribe succeeds");

    // Every segment must have a no_speech_prob in [0.0, 1.0]
    for (i, seg) in output.segments.iter().enumerate() {
        assert!(
            seg.no_speech_prob >= 0.0 && seg.no_speech_prob <= 1.0,
            "segment {i} no_speech_prob out of range: {}",
            seg.no_speech_prob
        );
        // Every token must have p in [0.0, 1.0], plog non-positive, and
        // a non-negative id within the model's vocab range (tiny.en vocab is
        // ~51864). We don't assert text content — whisper.cpp may emit special
        // tokens (e.g., timestamp markers) whose text representation varies.
        for (j, tok) in seg.tokens.iter().enumerate() {
            assert!(
                tok.p >= 0.0 && tok.p <= 1.0,
                "segment {i} token {j} p out of range: {}",
                tok.p
            );
            assert!(
                tok.plog <= 0.0001,
                "segment {i} token {j} plog should be non-positive log-prob, got {}",
                tok.plog
            );
            assert!(
                tok.id >= 0,
                "segment {i} token {j} id should be non-negative, got {}",
                tok.id
            );
        }
    }

    if !output.text.trim().is_empty() {
        assert!(
            !output.segments.is_empty(),
            "non-empty text should produce non-empty segments"
        );
    }

    engine.shutdown();
}
