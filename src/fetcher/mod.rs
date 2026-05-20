pub mod ytdlp;

use std::path::PathBuf;

use async_trait::async_trait;

use crate::errors::FetchError;

#[derive(Debug)]
pub enum Acquisition {
    /// Audio file written to disk; pipeline will hand to whisper.cpp next.
    AudioFile(PathBuf),
}

#[async_trait]
pub trait VideoFetcher: Send + Sync {
    async fn acquire(&self, video_id: &str, source_url: &str) -> Result<Acquisition, FetchError>;

    /// Identifier of the fetcher implementation, recorded in
    /// `TranscriptMetadata::fetcher` and `SuccessArtifacts::fetcher`.
    /// Replaces Plan A's hardcoded "ytdlp" literal so multi-fetcher
    /// provenance reflects the actual fetcher that ran (partial resolution
    /// of FOLLOWUPS T14).
    fn name(&self) -> &'static str;
}

// Cfg-gated test fixture per 0005; consumed by tests/pipeline_fakes.rs.
// Bin compilation also gets the feature when --features test-helpers is
// enabled, hence the dead_code suppression.
#[cfg(any(test, feature = "test-helpers"))]
#[allow(dead_code)]
pub struct FakeFetcher {
    pub canned: std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>,
    /// When true, `acquire` always returns `FetchError::NetworkError` regardless
    /// of the canned map. Used by `run_serial` failure-classification tests
    /// (T9) to exercise the retryable-failure path.
    pub always_fails: bool,
    /// One-shot gate: when `Some`, the FIRST `acquire` call awaits
    /// `notified()` on the inner `Notify` before returning (the configured
    /// outcome via `always_fails`/`canned` then applies). Subsequent calls
    /// skip the gate. Used by T16's `fetch_worker_increments_stale_after_failure_on_swept_claim`
    /// test to deterministically interleave: worker enters
    /// `fetcher.acquire` → test main task sweeps the row back to pending
    /// on a separate connection / locks the shared store → test fires
    /// `notify_one` → fetcher returns Err → worker's
    /// `mark_retryable_failure` predicate misses (row no longer claimed)
    /// → returns `Ok(0)` → counter increments.
    pub first_call_gate: tokio::sync::Mutex<Option<std::sync::Arc<tokio::sync::Notify>>>,
}

#[cfg(any(test, feature = "test-helpers"))]
#[allow(dead_code)]
impl FakeFetcher {
    /// Construct a `FakeFetcher` that fails every `acquire` call. Used by T9's
    /// continue-on-failure test in `tests/pipeline_fakes.rs`.
    pub fn always_fails() -> Self {
        Self {
            canned: std::sync::Mutex::new(std::collections::HashMap::new()),
            always_fails: true,
            first_call_gate: tokio::sync::Mutex::new(None),
        }
    }

    /// Construct a `FakeFetcher` whose FIRST `acquire` call awaits the
    /// returned `Notify` before failing; subsequent calls fail immediately.
    /// Used to force the stale-after-failure path in T16's tests
    /// deterministically.
    pub fn gated_then_always_fails() -> (Self, std::sync::Arc<tokio::sync::Notify>) {
        let gate = std::sync::Arc::new(tokio::sync::Notify::new());
        let fetcher = Self {
            canned: std::sync::Mutex::new(std::collections::HashMap::new()),
            always_fails: true,
            first_call_gate: tokio::sync::Mutex::new(Some(gate.clone())),
        };
        (fetcher, gate)
    }
}

#[cfg(any(test, feature = "test-helpers"))]
#[async_trait]
impl VideoFetcher for FakeFetcher {
    async fn acquire(&self, video_id: &str, _source_url: &str) -> Result<Acquisition, FetchError> {
        // One-shot gate: take the Notify out of the slot (so subsequent calls
        // skip), then await `notified()` outside the slot guard so we don't
        // hold the tokio Mutex across the long await.
        let maybe_gate = {
            let mut slot = self.first_call_gate.lock().await;
            slot.take()
        };
        if let Some(gate) = maybe_gate {
            gate.notified().await;
        }

        if self.always_fails {
            return Err(FetchError::NetworkError(format!(
                "FakeFetcher::always_fails synthetic failure for {}",
                video_id
            )));
        }
        let map = self.canned.lock().expect("canned mutex");
        match map.get(video_id) {
            Some(path) => Ok(Acquisition::AudioFile(path.clone())),
            None => Err(FetchError::ParseError(format!(
                "FakeFetcher has no canned response for {}",
                video_id
            ))),
        }
    }

    fn name(&self) -> &'static str {
        "fake-fetcher"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn fake_fetcher_returns_canned_audio_file() {
        let map = HashMap::from([(
            "7234567890123456789".to_string(),
            PathBuf::from("/tmp/fake.wav"),
        )]);
        let fake = FakeFetcher {
            canned: std::sync::Mutex::new(map),
            always_fails: false,
            first_call_gate: tokio::sync::Mutex::new(None),
        };
        let result = fake.acquire("7234567890123456789", "url").await.unwrap();
        match result {
            Acquisition::AudioFile(p) => assert_eq!(p, PathBuf::from("/tmp/fake.wav")),
        }
    }
}
