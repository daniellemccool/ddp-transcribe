# Task 03: Taxonomy module + classifiers + corpus-seeded table tests

**Files:**
- Create: `src/failure.rs`
- Modify: `src/lib.rs` and/or `src/main.rs` (add `pub mod failure;` — check how existing modules are declared; the crate has both lib and bin surfaces)
- Create: `tests/fixtures/yt_dlp_stderr/` — nine fixture files (real corpus messages)

**Interfaces:**
- Consumes: Task 02's `FetchError` (incl. `ToolNotFound`, `SystemIo`, `WorkDirCreate`, `MissingOutput`, `ToolFailed { signal, .. }`) and `TranscribeError` (incl. `AudioDecode`).
- Produces (Tasks 07 and 10 depend on these exact names):
  - `pub enum RetryableKind { NoDataBlocks, NoPermission, SensitiveLoginGated, NoVideoFormats, FfprobePostprocess, NetworkTransient, HttpError, ToolTimeout, YtDlpOther, TranscribeOther }` with `pub fn tag(&self) -> &'static str` (returns the variant name verbatim, e.g. `"SensitiveLoginGated"`)
  - `pub enum UnavailableReason { IpBlockedMessage, VideoNotAvailable10231 }` with `pub fn tag(&self) -> &'static str`
  - `pub struct FailureContext { pub tool: &'static str, pub exit_code: Option<i32>, pub signal: Option<i32>, pub stderr_excerpt: String, pub classification_reason: &'static str }` with `pub fn message(&self) -> String`
  - `pub enum ClassifiedFailure { Retryable { kind: RetryableKind, ctx: FailureContext }, Unavailable { reason: UnavailableReason, ctx: FailureContext }, Bug { ctx: FailureContext } }`
  - `pub fn classify_fetch_error(e: &FetchError) -> ClassifiedFailure`
  - `pub fn classify_transcribe_error(e: &TranscribeError) -> ClassifiedFailure`
  - `pub fn classify_message(stderr: &str) -> MessageVerdict` and `pub enum MessageVerdict { Unavailable(UnavailableReason), Retryable(RetryableKind) }` — the shared message matcher; triage (Task 10) calls it directly on stored `last_retryable_message` values.

- [ ] **Step 1: Extract the fixture corpus from the production export**

`ddp-run-export.sqlite` sits untracked in the repo root. Extract one real message per class (full message, including the `fetching <id>: subprocess…` prefix — the classifier must work on exactly what the DB stores):

```bash
cd /home/dmm/src/uu-tiktok && mkdir -p tests/fixtures/yt_dlp_stderr
q() { sqlite3 ddp-run-export.sqlite "SELECT last_retryable_message FROM videos WHERE status='failed_retryable' AND last_retryable_message LIKE '%$1%' LIMIT 1" > "tests/fixtures/yt_dlp_stderr/$2.txt"; }
q "Your IP address is blocked" ip_blocked
q "status code 10231" video_not_available_10231
q "Did not get any data blocks" no_data_blocks
q "do not have permission" no_permission
q "not be comfortable for some audiences" sensitive_login_gated
q "No video formats found" no_video_formats
q "unable to obtain file audio codec with ffprobe" ffprobe_postprocess
q "HTTP Error 403" http_error_403
q "HTTPSConnectionPool" network_transient
wc -l tests/fixtures/yt_dlp_stderr/*.txt   # every file must be non-empty
```

Add a `tests/fixtures/yt_dlp_stderr/README.md` (3 lines): provenance = 65k production run 2026-06-16→2026-07-03, yt-dlp 2026.03.17, extracted 2026-07-07; these are real donor-run error messages — do not synthesize replacements.

- [ ] **Step 2: Write the failing table-driven tests**

In `src/failure.rs` (in-module `#[cfg(test)]`; fixtures via `include_str!` so no runtime path issues):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                "../tests/fixtures/yt_dlp_stderr/",
                $name,
                ".txt"
            ))
        };
    }

    #[test]
    fn message_table_drives_classification() {
        // (fixture, expected verdict) — the load-bearing table. Real corpus
        // messages; the probe evidence behind each verdict is in ADR 0033.
        let unavailable: &[(&str, UnavailableReason)] = &[
            (fixture!("ip_blocked"), UnavailableReason::IpBlockedMessage),
            (fixture!("video_not_available_10231"), UnavailableReason::VideoNotAvailable10231),
        ];
        let retryable: &[(&str, RetryableKind)] = &[
            (fixture!("no_data_blocks"), RetryableKind::NoDataBlocks),
            (fixture!("no_permission"), RetryableKind::NoPermission),
            (fixture!("sensitive_login_gated"), RetryableKind::SensitiveLoginGated),
            (fixture!("no_video_formats"), RetryableKind::NoVideoFormats),
            (fixture!("ffprobe_postprocess"), RetryableKind::FfprobePostprocess),
            (fixture!("http_error_403"), RetryableKind::HttpError),
            (fixture!("network_transient"), RetryableKind::NetworkTransient),
        ];
        for (msg, want) in unavailable {
            match classify_message(msg) {
                MessageVerdict::Unavailable(r) => assert_eq!(&r, want, "msg: {msg}"),
                other => panic!("expected Unavailable({want:?}), got {other:?} for: {msg}"),
            }
        }
        for (msg, want) in retryable {
            match classify_message(msg) {
                MessageVerdict::Retryable(k) => assert_eq!(&k, want, "msg: {msg}"),
                other => panic!("expected Retryable({want:?}), got {other:?} for: {msg}"),
            }
        }
    }

    #[test]
    fn unknown_message_is_default_cautious_retryable() {
        match classify_message("ERROR: some yt-dlp message we have never seen") {
            MessageVerdict::Retryable(RetryableKind::YtDlpOther) => {}
            other => panic!("unknown stderr must be YtDlpOther, got {other:?}"),
        }
    }

    #[test]
    fn fetch_error_arms_route_correctly() {
        use crate::errors::FetchError;
        use std::time::Duration;

        let cases: &[(FetchError, fn(&ClassifiedFailure) -> bool)] = &[
            (
                FetchError::ToolTimeout { tool: "yt-dlp", duration: Duration::from_secs(300) },
                |c| matches!(c, ClassifiedFailure::Retryable { kind: RetryableKind::ToolTimeout, .. }),
            ),
            (
                FetchError::ToolNotFound { tool: "yt-dlp", detail: "ENOENT".into() },
                |c| matches!(c, ClassifiedFailure::Bug { .. }),
            ),
            (
                FetchError::WorkDirCreate { path: "/nope".into(), detail: "EACCES".into() },
                |c| matches!(c, ClassifiedFailure::Bug { .. }),
            ),
            (
                FetchError::SystemIo { tool: "yt-dlp", detail: "pipe".into() },
                |c| matches!(c, ClassifiedFailure::Retryable { kind: RetryableKind::NetworkTransient, .. }),
            ),
            (
                FetchError::MissingOutput { path: "/tmp/x.wav".into() },
                |c| matches!(c, ClassifiedFailure::Retryable { kind: RetryableKind::YtDlpOther, .. }),
            ),
            (
                FetchError::NetworkError("dns".into()),
                |c| matches!(c, ClassifiedFailure::Retryable { kind: RetryableKind::NetworkTransient, .. }),
            ),
        ];
        for (err, check) in cases {
            let got = classify_fetch_error(err);
            assert!(check(&got), "wrong classification for {err:?}: {got:?}");
        }
    }

    #[test]
    fn tool_failed_with_write_off_message_is_unavailable() {
        use crate::errors::FetchError;
        let e = FetchError::ToolFailed {
            tool: "yt-dlp",
            exit_code: 1,
            signal: None,
            stderr_excerpt: fixture!("ip_blocked").to_string(),
        };
        match classify_fetch_error(&e) {
            ClassifiedFailure::Unavailable { reason: UnavailableReason::IpBlockedMessage, ctx } => {
                assert_eq!(ctx.exit_code, Some(1));
                assert!(!ctx.classification_reason.is_empty());
            }
            other => panic!("expected Unavailable(IpBlockedMessage), got {other:?}"),
        }
    }

    #[test]
    fn transcribe_bug_stays_bug_and_decode_is_retryable() {
        use crate::errors::TranscribeError;
        assert!(matches!(
            classify_transcribe_error(&TranscribeError::Bug { detail: "x".into() }),
            ClassifiedFailure::Bug { .. }
        ));
        assert!(matches!(
            classify_transcribe_error(&TranscribeError::AudioDecode { detail: "truncated".into() }),
            ClassifiedFailure::Retryable { kind: RetryableKind::TranscribeOther, .. }
        ));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test --features test-helpers failure:: --lib -- --test-threads=1`
Expected: compile failure — module doesn't exist.

- [ ] **Step 4: Implement `src/failure.rs`**

```rust
//! Failure classification (Epic 3, ADR 0033). Policy layer over tool errors:
//! maps `FetchError` / `TranscribeError` to a three-arm verdict the pipeline
//! dispatches on. Patterns are evidence-derived from the 65k production run;
//! see the fixture corpus in tests/fixtures/yt_dlp_stderr/ and ADR 0033 for
//! the probe validation behind each verdict. Default-cautious: unmatched
//! input is Retryable, never Bug.

use crate::errors::{FetchError, TranscribeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryableKind {
    NoDataBlocks,
    NoPermission,
    SensitiveLoginGated,
    NoVideoFormats,
    FfprobePostprocess,
    NetworkTransient,
    HttpError,
    ToolTimeout,
    YtDlpOther,
    TranscribeOther,
}

impl RetryableKind {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::NoDataBlocks => "NoDataBlocks",
            Self::NoPermission => "NoPermission",
            Self::SensitiveLoginGated => "SensitiveLoginGated",
            Self::NoVideoFormats => "NoVideoFormats",
            Self::FfprobePostprocess => "FfprobePostprocess",
            Self::NetworkTransient => "NetworkTransient",
            Self::HttpError => "HttpError",
            Self::ToolTimeout => "ToolTimeout",
            Self::YtDlpOther => "YtDlpOther",
            Self::TranscribeOther => "TranscribeOther",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableReason {
    /// "Your IP address is blocked" — probe-validated 10/10 dead (2026-07-06).
    /// TikTok returns this message for deleted content; it is NOT an IP issue.
    IpBlockedMessage,
    /// "Video not available, status code 10231" — probe-validated 5/5 dead.
    VideoNotAvailable10231,
}

impl UnavailableReason {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::IpBlockedMessage => "IpBlockedMessage",
            Self::VideoNotAvailable10231 => "VideoNotAvailable10231",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FailureContext {
    pub tool: &'static str,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub stderr_excerpt: String,
    /// Which rule matched — audit trail for "why was this row written off".
    pub classification_reason: &'static str,
}

impl FailureContext {
    /// Message written to last_retryable_message / terminal_message. Leads
    /// with the matched rule so operators can grep verdicts, keeps the raw
    /// excerpt so nothing is lost.
    pub fn message(&self) -> String {
        format!("[{}] {}", self.classification_reason, self.stderr_excerpt)
    }
}

#[derive(Debug)]
pub enum ClassifiedFailure {
    Retryable { kind: RetryableKind, ctx: FailureContext },
    Unavailable { reason: UnavailableReason, ctx: FailureContext },
    Bug { ctx: FailureContext },
}

#[derive(Debug, PartialEq, Eq)]
pub enum MessageVerdict {
    Unavailable(UnavailableReason),
    Retryable(RetryableKind),
}

/// Shared message matcher. Order is load-bearing: write-off classes first,
/// then specific retryable classes, then network markers, then the
/// default-cautious catch-all. Substring matching on the raw stored message
/// (which includes our own "fetching <id>: subprocess…" prefix).
pub fn classify_message(stderr: &str) -> MessageVerdict {
    if stderr.contains("Your IP address is blocked") {
        return MessageVerdict::Unavailable(UnavailableReason::IpBlockedMessage);
    }
    if stderr.contains("status code 10231") {
        return MessageVerdict::Unavailable(UnavailableReason::VideoNotAvailable10231);
    }
    if stderr.contains("Did not get any data blocks") {
        return MessageVerdict::Retryable(RetryableKind::NoDataBlocks);
    }
    if stderr.contains("do not have permission to view this post") {
        return MessageVerdict::Retryable(RetryableKind::NoPermission);
    }
    if stderr.contains("not be comfortable for some audiences") {
        return MessageVerdict::Retryable(RetryableKind::SensitiveLoginGated);
    }
    if stderr.contains("No video formats found") {
        return MessageVerdict::Retryable(RetryableKind::NoVideoFormats);
    }
    if stderr.contains("unable to obtain file audio codec with ffprobe") {
        return MessageVerdict::Retryable(RetryableKind::FfprobePostprocess);
    }
    if stderr.contains("HTTP Error") {
        return MessageVerdict::Retryable(RetryableKind::HttpError);
    }
    const NETWORK_MARKERS: &[&str] = &[
        "Unable to download webpage",
        "HTTPSConnectionPool",
        "Connection aborted",
        "ConnectionResetError",
        "RemoteDisconnected",
        "curl: (28)",
        "SSL",
        "Too Many Requests",
    ];
    if NETWORK_MARKERS.iter().any(|m| stderr.contains(m)) {
        return MessageVerdict::Retryable(RetryableKind::NetworkTransient);
    }
    MessageVerdict::Retryable(RetryableKind::YtDlpOther)
}

pub fn classify_fetch_error(e: &FetchError) -> ClassifiedFailure {
    let ctx = |exit_code: Option<i32>, signal: Option<i32>, excerpt: &str, reason: &'static str| {
        FailureContext {
            tool: "yt-dlp",
            exit_code,
            signal,
            stderr_excerpt: excerpt.to_string(),
            classification_reason: reason,
        }
    };
    match e {
        FetchError::ToolTimeout { duration, .. } => ClassifiedFailure::Retryable {
            kind: RetryableKind::ToolTimeout,
            ctx: ctx(None, None, &format!("timed out after {duration:?}"), "tool timeout"),
        },
        FetchError::ToolNotFound { detail, .. } => ClassifiedFailure::Bug {
            ctx: ctx(None, None, detail, "tool binary missing: configuration broken"),
        },
        FetchError::WorkDirCreate { path, detail } => ClassifiedFailure::Bug {
            ctx: ctx(None, None, &format!("{}: {detail}", path.display()), "work dir creation failed: environment broken"),
        },
        FetchError::SystemIo { detail, .. } => ClassifiedFailure::Retryable {
            kind: RetryableKind::NetworkTransient,
            ctx: ctx(None, None, detail, "system io reading subprocess output"),
        },
        FetchError::MissingOutput { path } => ClassifiedFailure::Retryable {
            kind: RetryableKind::YtDlpOther,
            ctx: ctx(Some(0), None, &format!("{} missing after exit 0", path.display()), "yt-dlp exit 0 but expected wav missing"),
        },
        FetchError::NetworkError(detail) => ClassifiedFailure::Retryable {
            kind: RetryableKind::NetworkTransient,
            ctx: ctx(None, None, detail, "network error"),
        },
        FetchError::ParseError(detail) => ClassifiedFailure::Retryable {
            kind: RetryableKind::YtDlpOther,
            ctx: ctx(None, None, detail, "fetcher output parse failure"),
        },
        FetchError::ToolFailed { exit_code, signal, stderr_excerpt, .. } => {
            let base = FailureContext {
                tool: "yt-dlp",
                exit_code: Some(*exit_code),
                signal: *signal,
                stderr_excerpt: stderr_excerpt.clone(),
                classification_reason: "stderr message class",
            };
            match classify_message(stderr_excerpt) {
                MessageVerdict::Unavailable(reason) => {
                    ClassifiedFailure::Unavailable { reason, ctx: base }
                }
                MessageVerdict::Retryable(kind) => {
                    ClassifiedFailure::Retryable { kind, ctx: base }
                }
            }
        }
    }
}

pub fn classify_transcribe_error(e: &TranscribeError) -> ClassifiedFailure {
    let ctx = |excerpt: String, reason: &'static str| FailureContext {
        tool: "whisper-rs",
        exit_code: None,
        signal: None,
        stderr_excerpt: excerpt,
        classification_reason: reason,
    };
    match e {
        TranscribeError::Bug { detail } => ClassifiedFailure::Bug {
            ctx: ctx(detail.clone(), "transcribe internal invariant"),
        },
        TranscribeError::AudioDecode { detail } => ClassifiedFailure::Retryable {
            kind: RetryableKind::TranscribeOther,
            ctx: ctx(detail.clone(), "wav decode failure: refetch may repair a corrupt download"),
        },
        TranscribeError::Timeout { duration } => ClassifiedFailure::Retryable {
            kind: RetryableKind::ToolTimeout,
            ctx: ctx(format!("timed out after {duration:?}"), "transcribe timeout"),
        },
        // Cancelled is handled by the worker before classification (worker
        // exits Ok); classifying it defensively as retryable keeps the fn
        // total without inventing a verdict the dispatch will ever act on.
        other => ClassifiedFailure::Retryable {
            kind: RetryableKind::TranscribeOther,
            ctx: ctx(other.to_string(), "unmatched transcribe error: default-cautious"),
        },
    }
}
```

Register the module where the crate declares modules (match existing style — likely `pub mod failure;` in both `src/lib.rs` and `src/main.rs` module lists if the crate uses the dual-declaration pattern; check `grep -n "mod errors" src/lib.rs src/main.rs` and mirror exactly).

Dead-code note (0002): `FailureContext.tool` / `.signal` and `MessageVerdict` are consumed by Tasks 07/10; if clippy flags them before those tasks land, add `#[allow(dead_code)]` at the item with a `// consumed by Epic 3 T07/T10` note and lift it there.

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test --features test-helpers failure:: --lib -- --test-threads=1` then the full suite.
Expected: PASS. The write-off table test proves both write-off classes and all seven retryable classes classify from real corpus messages.

- [ ] **Step 6: Commit**

```bash
git add src/failure.rs src/lib.rs src/main.rs tests/fixtures/yt_dlp_stderr/ Cargo.toml
git commit -m "feat(failure): evidence-derived taxonomy + classifiers with corpus-seeded table tests (ADR 0033)"
```
