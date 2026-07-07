//! Shared fakes and fixture helpers for the pipeline_fakes suite.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use ddp_transcribe::errors::TranscribeError;
use ddp_transcribe::transcribe::{PerCallConfig, TranscribeOutput, Transcriber};

/// In-test `Transcriber` impl with two behaviors:
/// - `Scripted(output)`: returns a scripted `TranscribeOutput` regardless of
///   the samples it receives. Lets us assert that the pipeline projects the
///   engine's output into the JSON artifact's `raw_signals` sub-object
///   correctly without loading a whisper.cpp model.
/// - `AlwaysFailsRetryable`: returns `Err(TranscribeError::EmptyOutput)` — a
///   non-Cancelled, non-Bug variant that `transcribe_worker` classifies as a
///   retryable failure (used for the stale-after-failure counter tests).
pub(crate) enum FakeBehavior {
    Scripted(TranscribeOutput),
    AlwaysFailsRetryable,
}

pub(crate) struct FakeTranscriber {
    behavior: FakeBehavior,
}

impl FakeTranscriber {
    /// Scripted output, mirrors the legacy constructor pattern.
    pub(crate) fn scripted(output: TranscribeOutput) -> Self {
        Self {
            behavior: FakeBehavior::Scripted(output),
        }
    }

    /// "Echo" transcriber: a minimal scripted output with empty text and a
    /// recognizable language tag. Used by tests where the transcript
    /// content isn't being asserted — only that the row reaches
    /// `succeeded` and artifacts are written (per 0008).
    pub(crate) fn echo() -> Self {
        Self::scripted(TranscribeOutput {
            text: String::new(),
            language: "en".into(),
            lang_probs: None,
            segments: vec![],
            model_id: "fake-echo.bin".into(),
        })
    }

    /// Always fails with `TranscribeError::EmptyOutput` — a retryable-class
    /// variant (not Cancelled, not Bug). Drives the worker into the
    /// `mark_retryable_failure` branch.
    pub(crate) fn always_fails_retryable() -> Self {
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
pub(crate) fn silence_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/silence_16khz_mono.wav")
}
