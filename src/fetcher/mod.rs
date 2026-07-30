pub mod ytdlp;

use std::path::PathBuf;

use async_trait::async_trait;

use crate::errors::FetchError;

#[derive(Debug)]
pub enum Acquisition {
    /// Audio file written to disk; pipeline will hand to whisper.cpp next.
    AudioFile(PathBuf),
}

/// Fetch-format selection policy (staged experiment, ADR 0038 — see
/// `ytdlp::build_yt_dlp_args`'s doc comment for the selector strings,
/// probe evidence, and the yt-dlp-issue caveat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchPolicy {
    /// Pre-muxed audio with selection-time fallbacks (`-f
    /// "download/b[vcodec=h264]/b"`) — byte-identical to the selector the
    /// pilot ran, and the default for fresh claims and every retry kind
    /// except `NoDataBlocks`. Retained as default on pilot-scale evidence:
    /// the 2026-07-08 frugal probe is n=17, yt-dlp #16622 is an open issue
    /// against exactly the ABR formats the frugal selector prefers, and
    /// there is no evidence yet about differential rate-limiting between
    /// the two format populations.
    #[default]
    DeterministicAudio,
    /// Smallest audio-tagged combined format (`-f "b[acodec!=none]/b"`).
    /// Never selects TikTok's `download` static asset. Applied only to a
    /// retry whose prior failure classified `NoDataBlocks` — the
    /// download-advertised-but-unservable mechanism, where re-picking
    /// `download` would reproduce the mid-transfer failure. The parked
    /// pilot backlog (~2,318 rows) retried under this policy is the
    /// at-scale experiment that a future frugal-default flip is contingent
    /// on (ADR 0038 names the decision trigger).
    Frugal,
}

impl FetchPolicy {
    /// Short stable tag recorded as the `"policy"` key in the detail JSON
    /// of every event `Store::record_fetch_failure` writes (ADR 0038
    /// observability: the failure mix must be attributable to the format
    /// policy the fetch actually ran under).
    pub fn tag(&self) -> &'static str {
        match self {
            FetchPolicy::DeterministicAudio => "deterministic-audio",
            FetchPolicy::Frugal => "frugal",
        }
    }
}

/// Per-request fetch options (Epic 3 cookies; ADR 0038 format policy).
/// Cookie scope is policy: ADR 0035 pins cookies to SensitiveLoginGated
/// retries only. Format-policy scope is likewise policy: `Frugal` is keyed
/// on `NoDataBlocks` retries (see `pipeline::cookie_opts_for`). This
/// struct just carries both decisions to the tool adapter.
#[derive(Debug, Clone, Default)]
pub struct FetchOpts {
    pub cookies_file: Option<PathBuf>,
    pub format_policy: FetchPolicy,
}

/// Raw fetch-time metadata capture (Epic 4c): the versioned envelope JSON
/// stored verbatim in `video_metadata_raw`. Produced on success AND
/// tool-failure paths; absent on structural failures (timeout/spawn/io).
///
/// Both pipeline paths read the field into `Store::upsert_metadata_raw`
/// before outcome dispatch (0042).
#[derive(Debug, Clone)]
pub struct MetadataCapture {
    pub envelope_json: String,
}

#[async_trait]
pub trait VideoFetcher: Send + Sync {
    /// Acquire the video's audio. The first tuple element is the raw
    /// metadata envelope when the tool produced one — present on success
    /// AND classified-failure paths (the printed line lands before the
    /// media transfer), absent on structural failures. Callers persist it
    /// BEFORE interpreting the outcome (Epic 4c).
    async fn acquire(
        &self,
        video_id: &str,
        source_url: &str,
        opts: &FetchOpts,
    ) -> (Option<MetadataCapture>, Result<Acquisition, FetchError>);

    /// Identifier of the fetcher implementation, recorded in
    /// `TranscriptMetadata::fetcher` and `SuccessArtifacts::fetcher`.
    /// Replaces Plan A's hardcoded "ytdlp" literal so multi-fetcher
    /// provenance reflects the actual fetcher that ran (partial resolution
    /// of FOLLOWUPS T14).
    fn name(&self) -> &'static str;
}

// Cfg-gated test fixture per 0005; consumed by the tests/pipeline_fakes/
// test files.
#[cfg(any(test, feature = "test-helpers"))]
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
    /// When Some, `acquire` returns a `FetchError::ToolFailed` carrying this
    /// stderr text verbatim (exit_code=1, signal=None), checked before the
    /// `always_fails` branch. Lets Epic 3 integration tests drive specific
    /// classifier verdicts (`classify_fetch_error`'s message table) through
    /// real worker dispatch rather than calling the classifier directly.
    pub canned_stderr: std::sync::Mutex<Option<String>>,
    /// Records every `FetchOpts` passed to `acquire`, in call order. Lets
    /// Epic 3 T08's cookie-routing tests assert what the worker actually
    /// threaded through, rather than re-deriving the policy decision.
    pub received_opts: std::sync::Mutex<Vec<FetchOpts>>,
    /// Epic 4a test hook: per-video count of failures to emit BEFORE
    /// succeeding (each failed acquire decrements). 0/absent = the canned
    /// behavior applies immediately. Failure text comes from `canned_stderr`.
    pub fail_first_n: std::sync::Mutex<std::collections::HashMap<String, u32>>,
    /// Epic 4c: when Some, every acquire returns this envelope string as
    /// its MetadataCapture (alongside whatever outcome the other knobs
    /// configure). Lets integration tests drive raw-row persistence
    /// through real worker dispatch.
    pub canned_metadata: std::sync::Mutex<Option<String>>,
}

#[cfg(any(test, feature = "test-helpers"))]
impl FakeFetcher {
    /// Construct a `FakeFetcher` that fails every `acquire` call. Used by T9's
    /// continue-on-failure test in `tests/pipeline_fakes/serial_tests.rs`.
    pub fn always_fails() -> Self {
        Self {
            canned: std::sync::Mutex::new(std::collections::HashMap::new()),
            always_fails: true,
            first_call_gate: tokio::sync::Mutex::new(None),
            canned_stderr: std::sync::Mutex::new(None),
            received_opts: std::sync::Mutex::new(Vec::new()),
            fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
            canned_metadata: std::sync::Mutex::new(None),
        }
    }

    /// Construct a `FakeFetcher` whose `acquire` always fails with
    /// `FetchError::ToolFailed { stderr_excerpt: stderr.to_string(), .. }`
    /// (exit_code=1, signal=None). Used by Epic 3 integration tests to drive
    /// specific classifier verdicts (write-off vs. taxonomy-kind retryable)
    /// through real `fetch_worker`/`run_serial` dispatch.
    pub fn fails_with_stderr(stderr: &str) -> Self {
        Self {
            canned: std::sync::Mutex::new(std::collections::HashMap::new()),
            always_fails: false,
            first_call_gate: tokio::sync::Mutex::new(None),
            canned_stderr: std::sync::Mutex::new(Some(stderr.to_string())),
            received_opts: std::sync::Mutex::new(Vec::new()),
            fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
            canned_metadata: std::sync::Mutex::new(None),
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
            canned_stderr: std::sync::Mutex::new(None),
            received_opts: std::sync::Mutex::new(Vec::new()),
            fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
            canned_metadata: std::sync::Mutex::new(None),
        };
        (fetcher, gate)
    }
}

#[cfg(any(test, feature = "test-helpers"))]
// Test scaffolding: a poisoned `canned` mutex only happens if a test panicked while
// holding it, so expect() (surfacing that as a panic here) is correct.
#[allow(clippy::expect_used)]
#[async_trait]
impl VideoFetcher for FakeFetcher {
    async fn acquire(
        &self,
        video_id: &str,
        _source_url: &str,
        opts: &FetchOpts,
    ) -> (Option<MetadataCapture>, Result<Acquisition, FetchError>) {
        // Recorder: pushed unconditionally, before any of the
        // fail/succeed branches below, so Epic 3's cookie-routing tests
        // can assert what the caller actually threaded through.
        self.received_opts
            .lock()
            .expect("received_opts mutex")
            .push(opts.clone());

        // Epic 4c: computed once and returned alongside EVERY outcome
        // below, mirroring the real fetcher (the envelope rides both the
        // success and the classified-failure paths).
        let capture = self
            .canned_metadata
            .lock()
            .expect("canned_metadata mutex")
            .clone()
            .map(|envelope_json| MetadataCapture { envelope_json });

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

        // Epic 4a fails-N-then-succeeds gate. For a managed video, fail the
        // first N acquires (using `canned_stderr` as the failure text, or a
        // generic fallback), then let the row graduate to the canned success
        // below. Placed BEFORE the `canned_stderr` always-fail check so an
        // exhausted managed video isn't re-failed by it — that check is then
        // guarded (`managed`) to skip managed videos.
        let managed = {
            let mut gate = self.fail_first_n.lock().expect("fail_first_n lock");
            match gate.get_mut(video_id) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    let stderr = self
                        .canned_stderr
                        .lock()
                        .expect("canned_stderr lock")
                        .clone()
                        .unwrap_or_else(|| "transient fake failure".to_string());
                    return (
                        capture,
                        Err(FetchError::ToolFailed {
                            tool: "yt-dlp".to_string(),
                            exit_code: 1,
                            signal: None,
                            stderr_excerpt: stderr,
                        }),
                    );
                }
                // Budget spent → managed; skip the always-fail paths below so
                // the canned success can run.
                Some(_) => true,
                None => false,
            }
        };

        // Checked before `always_fails` so `fails_with_stderr` doesn't need
        // to also flip `always_fails` — the two modes are mutually
        // exclusive in practice (see the constructors above). Skipped for
        // fail_first_n-managed videos so a fails-N-then-succeeds fetcher can
        // reach the canned success once its budget is spent.
        if !managed {
            let canned_err = self
                .canned_stderr
                .lock()
                .expect("canned_stderr mutex")
                .clone();
            if let Some(stderr_excerpt) = canned_err {
                return (
                    capture,
                    Err(FetchError::ToolFailed {
                        tool: "yt-dlp".to_string(),
                        exit_code: 1,
                        signal: None,
                        stderr_excerpt,
                    }),
                );
            }
        }

        if self.always_fails {
            return (
                capture,
                Err(FetchError::NetworkError(format!(
                    "FakeFetcher::always_fails synthetic failure for {video_id}"
                ))),
            );
        }
        let map = self.canned.lock().expect("canned mutex");
        let result = match map.get(video_id) {
            Some(path) => Ok(Acquisition::AudioFile(path.clone())),
            None => Err(FetchError::ParseError(format!(
                "FakeFetcher has no canned response for {video_id}"
            ))),
        };
        (capture, result)
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
            canned_stderr: std::sync::Mutex::new(None),
            received_opts: std::sync::Mutex::new(Vec::new()),
            fail_first_n: std::sync::Mutex::new(std::collections::HashMap::new()),
            canned_metadata: std::sync::Mutex::new(None),
        };
        let (capture, result) = fake
            .acquire("7234567890123456789", "url", &FetchOpts::default())
            .await;
        assert!(capture.is_none(), "no canned metadata configured");
        match result.unwrap() {
            Acquisition::AudioFile(p) => assert_eq!(p, PathBuf::from("/tmp/fake.wav")),
        }
    }

    /// Epic 4c: `canned_metadata` rides every outcome the other knobs
    /// configure — here the always-fails path, mirroring the real fetcher's
    /// "envelope survives a classified failure" contract.
    #[tokio::test]
    async fn fake_fetcher_returns_canned_metadata_alongside_failure() {
        let fake = FakeFetcher::always_fails();
        *fake.canned_metadata.lock().expect("canned_metadata mutex") =
            Some(r#"{"schema":1,"printed":"{}"}"#.to_string());
        let (capture, result) = fake.acquire("vid_a", "url", &FetchOpts::default()).await;
        assert_eq!(
            capture.expect("capture present").envelope_json,
            r#"{"schema":1,"printed":"{}"}"#
        );
        assert!(result.is_err(), "always_fails outcome unchanged by capture");
    }
}
