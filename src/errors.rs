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
        stderr_excerpt: String,
    },

    #[error("network error during fetch: {0}")]
    NetworkError(String),

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

    #[error("transcription bug: {detail}")]
    Bug { detail: String },
}

impl From<crate::audio::AudioDecodeError> for TranscribeError {
    fn from(e: crate::audio::AudioDecodeError) -> Self {
        TranscribeError::Bug {
            detail: format!("audio decode failure (should be classified, not Bug, in Epic 3): {e}"),
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
}
