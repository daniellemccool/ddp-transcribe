//! Shared fakes and fixture helpers for the pipeline_fakes suite.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

use ddp_transcribe::errors::TranscribeError;
use ddp_transcribe::fetcher::VideoFetcher;
use ddp_transcribe::pipeline::{fetch_worker, FetchedItem, ProcessOptions, SharedStore};
use ddp_transcribe::state::Store;
use ddp_transcribe::transcribe::{PerCallConfig, TranscribeOutput, Transcriber};

/// In-test `Transcriber` impl with three behaviors:
/// - `Scripted(output)`: returns a scripted `TranscribeOutput` regardless of
///   the samples it receives. Lets us assert that the pipeline projects the
///   engine's output into the JSON artifact's `raw_signals` sub-object
///   correctly without loading a whisper.cpp model.
/// - `AlwaysFailsRetryable`: returns `Err(TranscribeError::EmptyOutput)` — a
///   non-Cancelled, non-Bug variant that `transcribe_worker` classifies as a
///   retryable failure (used for the stale-after-failure counter tests).
/// - `AlwaysFailsBug`: returns `Err(TranscribeError::Bug { .. })` — drives
///   the Bug-escalation dispatch arm (T07 review fix: `run_serial` must
///   return `Err` for a transcribe-side Bug, not mark the row retryable).
pub(crate) enum FakeBehavior {
    Scripted(TranscribeOutput),
    AlwaysFailsRetryable,
    AlwaysFailsBug,
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

    /// Always fails with `TranscribeError::Bug` — the Bug-class variant.
    /// Drives dispatch into the escalation arm (`return Err`) rather than
    /// any `mark_*` mutator.
    pub(crate) fn always_fails_bug() -> Self {
        Self {
            behavior: FakeBehavior::AlwaysFailsBug,
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
            FakeBehavior::AlwaysFailsBug => Err(TranscribeError::Bug {
                detail: "FakeTranscriber::always_fails_bug synthetic invariant breach".into(),
            }),
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

/// Fresh `Store` (in a scratch `TempDir`) with one `pending` row per
/// `video_id`, wrapped as a [`SharedStore`] so callers can hand it straight
/// to `fetch_worker`/`run_pipelined`. The `TempDir` must outlive the store
/// (it backs the sqlite file) — callers keep the second tuple element alive
/// for the duration of the test even if unused directly.
pub(crate) fn store_with_pending(video_ids: &[&str]) -> (SharedStore, TempDir) {
    let tmp = TempDir::new().expect("create tempdir");
    let mut store = Store::open(&tmp.path().join("state.sqlite")).expect("open store");
    for vid in video_ids {
        store
            .upsert_video(vid, &format!("https://example.com/{vid}"), false)
            .expect("upsert pending video");
    }
    (Arc::new(TokioMutex::new(store)), tmp)
}

/// Run a single `fetch_worker` to completion against `store`/`fetcher`,
/// draining (and discarding) every `FetchedItem` it emits. Used by tests
/// that only care about the DB row's post-failure state (terminal reason /
/// retryable kind), not the happy-path payload — see
/// `fetch_worker_tests.rs`'s write-off and taxonomy-kind tests.
pub(crate) async fn run_single_fetch_worker(store: SharedStore, fetcher: Arc<dyn VideoFetcher>) {
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    let (tx, mut rx) = mpsc::channel::<FetchedItem>(2);
    let opts = Arc::new(ProcessOptions {
        worker_id: "fetcher-1".into(),
        // fetch_worker never reads transcripts_root; only transcribe_worker
        // does. No need for this path to exist.
        transcripts_root: PathBuf::from("/unused/transcripts-root"),
        max_videos: None,
        compute_lang_probs: false,
        transcribe_timeout: Duration::from_secs(5),
        stale_claim_threshold: Duration::from_secs(60),
        download_workers: 3,
        channel_capacity: 2,
    });

    let worker = tokio::spawn(fetch_worker(
        CancellationToken::new(),
        store,
        fetcher,
        tx,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        opts,
    ));

    // Drain (0026 drain semantics: the worker closes the channel on exit).
    while rx.recv().await.is_some() {}

    worker
        .await
        .expect("join fetch_worker")
        .expect("fetch_worker returns Ok on this suite's failure-classification paths");
}

/// Read `(status, terminal_reason)` for `video_id` from `store`. Test-only
/// projection of [`ddp_transcribe::state::VideoRow`] — see
/// `status_and_retryable_kind` for the retryable-side counterpart.
pub(crate) async fn status_and_terminal_reason(
    store: &SharedStore,
    video_id: &str,
) -> (String, Option<String>) {
    let guard = store.lock().await;
    let row = guard
        .get_video_for_test(video_id)
        .expect("query video row")
        .expect("row present");
    (row.status, row.terminal_reason)
}

/// Read `(status, last_retryable_kind)` for `video_id` from `store`.
pub(crate) async fn status_and_retryable_kind(
    store: &SharedStore,
    video_id: &str,
) -> (String, Option<String>) {
    let guard = store.lock().await;
    let row = guard
        .get_video_for_test(video_id)
        .expect("query video row")
        .expect("row present");
    (row.status, row.last_retryable_kind)
}
