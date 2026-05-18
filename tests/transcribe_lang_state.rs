//! Tier 2 test for T2 perf-tweaks: lazy lang_state allocation.
//!
//! Requires ./models/ggml-tiny.en.bin on disk; gated by test-helpers feature
//! per 0005 because it depends on a non-trivial fixture and uses the
//! test-only `WhisperEngine::lang_state_allocations()` accessor.

#![cfg(feature = "test-helpers")]

use std::path::PathBuf;
use std::time::Duration;

use uu_tiktok::transcribe::{EngineConfig, PerCallConfig, WhisperEngine};

fn tiny_model_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models/ggml-tiny.en.bin")
}

fn skip_if_no_model() -> bool {
    if !tiny_model_path().exists() {
        eprintln!("Skipping: ./models/ggml-tiny.en.bin not found");
        true
    } else {
        false
    }
}

fn engine_config() -> EngineConfig {
    EngineConfig {
        model_path: tiny_model_path(),
        gpu_device: 0,
        flash_attn: false,
    }
}

#[tokio::test]
async fn lang_state_not_allocated_when_compute_lang_probs_never_true() {
    if skip_if_no_model() {
        return;
    }
    let engine = WhisperEngine::new(&engine_config()).expect("engine loads");

    // Send three non-opt-in requests; counter must stay at 0.
    for _ in 0..3 {
        let samples = vec![0.0_f32; 16000];
        let _ = engine
            .transcribe(samples, PerCallConfig::default(), Duration::from_secs(30))
            .await
            .expect("transcribe of silence should succeed");
    }

    assert_eq!(
        engine.lang_state_allocations(),
        0,
        "non-opt-in worker must never allocate lang_state"
    );
    engine.shutdown();
}

#[tokio::test]
async fn lang_state_allocated_exactly_once_across_opt_in_requests() {
    if skip_if_no_model() {
        return;
    }
    let engine = WhisperEngine::new(&engine_config()).expect("engine loads");

    let opt_in = PerCallConfig {
        language: None,
        compute_lang_probs: true,
    };

    // Three opt-in requests; counter goes 0 -> 1 -> stays 1.
    for i in 0..3 {
        let samples = vec![0.0_f32; 16000];
        let _ = engine
            .transcribe(samples, opt_in.clone(), Duration::from_secs(60))
            .await
            .expect("transcribe of silence should succeed");
        let count = engine.lang_state_allocations();
        assert_eq!(
            count,
            1,
            "expected counter == 1 after request {}, got {}",
            i + 1,
            count
        );
    }

    engine.shutdown();
}
