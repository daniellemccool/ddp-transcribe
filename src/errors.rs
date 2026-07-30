use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("subprocess `{tool}` timed out after {duration:?}")]
    ToolTimeout { tool: String, duration: Duration },

    // Final-review fix (Epic 3 close): the verbatim Task 02 brief template
    // omitted `signal`, so a signal-killed child (exit_code == -1) displayed
    // with no indication of *why* — defeating the field's stated purpose of
    // distinguishing OOM-kill (SIGKILL) / segfault (SIGSEGV) / operator
    // interrupt (SIGINT). thiserror can't conditionally format, so `signal`
    // is always shown (`None` for a normal exit) rather than adding a custom
    // Display impl for one field. Deviation disclosed per ADR-0003.
    #[error(
        "subprocess `{tool}` exited with status {exit_code} (signal {signal:?}): {stderr_excerpt}"
    )]
    ToolFailed {
        tool: String,
        exit_code: i32,
        /// Unix signal that killed the child, when it did not exit normally
        /// (`ExitStatus::code() == None`). Distinguishes OOM-kill (SIGKILL)
        /// from segfault (SIGSEGV) from operator interrupt (SIGINT).
        signal: Option<i32>,
        stderr_excerpt: String,
    },

    #[error("tool not found or not executable: {tool}: {detail}")]
    ToolNotFound { tool: String, detail: String },

    #[error("system io error running {tool}: {detail}")]
    SystemIo { tool: String, detail: String },

    #[error("failed to create work dir {path}: {detail}")]
    WorkDirCreate {
        path: std::path::PathBuf,
        detail: String,
    },

    #[error("tool succeeded but expected output {path} is missing")]
    MissingOutput { path: std::path::PathBuf },

    /// Epic 5b: the attempt directory holds MORE than one `*.wav` after a
    /// clean exit. Distinct from [`FetchError::MissingOutput`] on purpose —
    /// picking one of them would transcribe an arbitrary file and stamp it as
    /// this video's transcript, so ambiguity fails instead of guessing.
    #[error("tool succeeded but attempt dir {dir} holds {count} wav files (expected exactly 1)")]
    AmbiguousOutput {
        dir: std::path::PathBuf,
        count: usize,
    },

    // No production construction sites remain after Task 02's error split
    // (WorkDirCreate/MissingOutput/ToolNotFound/SystemIo took over the
    // acquire-path uses), but both variants are still constructed by
    // `FakeFetcher::acquire` (src/fetcher/mod.rs, gated
    // `cfg(any(test, feature = "test-helpers"))`) — NetworkError for
    // `always_fails`, ParseError for a missing canned response — backing the
    // T9 continue-on-failure and T16 stale-after-failure pipeline tests, and
    // both are classified in `failure.rs`. Check FakeFetcher before reshaping
    // these.
    #[error("network error during fetch: {0}")]
    NetworkError(String),

    #[error("failed to parse fetcher output: {0}")]
    ParseError(String),
}

#[derive(Debug, Error)]
pub enum TranscribeError {
    // Plan A's whisper-cli subprocess constructed these (T11 deleted the
    // legacy `transcribe()` fn). Epic 3 (ADR 0033) closed without rebuilding
    // this enum; as of Epic 4a the failure taxonomy is label strings driven by
    // the classification table (`src/classification.rs`), with structural
    // errors code-mapped in `src/failure.rs` — Epic 1's whisper-rs path
    // surfaces deadline-elapse via `Cancelled` and internal failures via
    // `Bug`, so `Timeout`, `Failed`, `EmptyOutput` are unconstructed by the
    // embedded engine. `Timeout` is matched by
    // `failure::classify_transcribe_error`; `Failed` is constructed by this
    // file's unit test and `EmptyOutput` by `tests/pipeline_fakes/fakes.rs`.
    // Revisit if a subprocess engine returns.
    #[error("whisper.cpp timed out after {duration:?}")]
    Timeout { duration: Duration },

    #[error("whisper.cpp exited with status {exit_code}: {stderr_excerpt}")]
    Failed {
        exit_code: i32,
        stderr_excerpt: String,
    },

    #[error("whisper.cpp produced no transcript")]
    EmptyOutput,

    #[error("transcription cancelled (deadline elapsed or operator-initiated)")]
    Cancelled,

    #[error("audio decode failure: {detail}")]
    AudioDecode { detail: String },

    #[error("transcription bug: {detail}")]
    Bug { detail: String },
}

impl From<crate::audio::AudioDecodeError> for TranscribeError {
    fn from(e: crate::audio::AudioDecodeError) -> Self {
        TranscribeError::AudioDecode {
            detail: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_error_displays_with_context() {
        let err = FetchError::ToolTimeout {
            tool: "yt-dlp".to_string(),
            duration: Duration::from_secs(300),
        };
        let msg = format!("{err}");
        assert!(msg.contains("yt-dlp"));
        assert!(msg.contains("300"));
    }

    #[test]
    fn fetch_tool_failed_display_surfaces_signal() {
        let killed = FetchError::ToolFailed {
            tool: "yt-dlp".to_string(),
            exit_code: -1,
            signal: Some(9),
            stderr_excerpt: "killed".into(),
        };
        let msg = format!("{killed}");
        assert!(
            msg.contains("signal Some(9)"),
            "signal-killed Display must surface the signal, got: {msg}"
        );

        let normal = FetchError::ToolFailed {
            tool: "yt-dlp".to_string(),
            exit_code: 1,
            signal: None,
            stderr_excerpt: "some error".into(),
        };
        let msg = format!("{normal}");
        assert!(msg.contains("signal None"));
        assert!(msg.contains("status 1"));
    }

    #[test]
    fn transcribe_error_failed_carries_exit_code() {
        let err = TranscribeError::Failed {
            exit_code: 1,
            stderr_excerpt: "out of memory".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("status 1"));
        assert!(msg.contains("out of memory"));
    }

    #[test]
    fn audio_decode_error_maps_to_audio_decode_not_bug() {
        // Create a 0-byte temp file via decode_wav to exercise the Empty variant
        use hound::{SampleFormat, WavSpec, WavWriter};
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().expect("create tempfile");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let _writer = WavWriter::create(tmp.path(), spec).expect("create wav writer");
        // Don't write any samples, so the file is valid but empty

        let e = crate::audio::decode_wav(tmp.path()).expect_err("empty WAV should error");
        match TranscribeError::from(e) {
            TranscribeError::AudioDecode { .. } => {}
            other => panic!("AudioDecodeError must map to AudioDecode, got {other:?}"),
        }
    }
}
