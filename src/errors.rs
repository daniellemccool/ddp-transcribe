use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("subprocess `{tool}` timed out after {duration:?}")]
    ToolTimeout {
        tool: &'static str,
        duration: Duration,
    },

    #[error("subprocess `{tool}` exited with status {exit_code}: {stderr_excerpt}")]
    ToolFailed {
        tool: &'static str,
        exit_code: i32,
        /// Unix signal that killed the child, when it did not exit normally
        /// (`ExitStatus::code() == None`). Distinguishes OOM-kill (SIGKILL)
        /// from segfault (SIGSEGV) from operator interrupt (SIGINT).
        signal: Option<i32>,
        stderr_excerpt: String,
    },

    #[error("tool not found or not executable: {tool}: {detail}")]
    ToolNotFound { tool: &'static str, detail: String },

    #[error("system io error running {tool}: {detail}")]
    SystemIo { tool: &'static str, detail: String },

    #[error("failed to create work dir {path}: {detail}")]
    WorkDirCreate {
        path: std::path::PathBuf,
        detail: String,
    },

    #[error("tool succeeded but expected output {path} is missing")]
    MissingOutput { path: std::path::PathBuf },

    /// 0002: Deferred to Epic 3's failure-classification taxonomy; Task 03 will
    /// dispatch network failures through RetryableKind.
    #[allow(dead_code)]
    #[error("network error during fetch: {0}")]
    NetworkError(String),

    /// 0002: Deferred to Epic 3's failure-classification taxonomy; Task 03 will
    /// dispatch parse failures through UnavailableReason.
    #[allow(dead_code)]
    #[error("failed to parse fetcher output: {0}")]
    ParseError(String),
}

#[derive(Debug, Error)]
pub enum TranscribeError {
    // 0002: Plan A's whisper-cli subprocess constructed these (T11 deleted
    // the legacy `transcribe()` fn). Epic 3's failure-classification work will
    // rebuild this enum with a richer taxonomy (`AudioDecode`, `ModelOOM`,
    // `RetryableKind`, `UnavailableReason`, etc.). Keeping `Timeout`,
    // `Failed`, `EmptyOutput` in place as forward-pointer variants so the
    // Epic 3 diff is additive — but they're not constructed anywhere in Epic
    // 1's whisper-rs path (the engine surfaces deadline-elapse via
    // `Cancelled` and internal failures via `Bug`). The errors.rs unit test
    // keeps `Failed` alive; `Timeout` and `EmptyOutput` need the explicit
    // suppression. Remove these annotations when Epic 3 re-wires them.
    #[allow(dead_code)]
    #[error("whisper.cpp timed out after {duration:?}")]
    Timeout { duration: Duration },

    #[allow(dead_code)]
    #[error("whisper.cpp exited with status {exit_code}: {stderr_excerpt}")]
    Failed {
        exit_code: i32,
        stderr_excerpt: String,
    },

    #[allow(dead_code)]
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
            tool: "yt-dlp",
            duration: Duration::from_secs(300),
        };
        let msg = format!("{err}");
        assert!(msg.contains("yt-dlp"));
        assert!(msg.contains("300"));
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
