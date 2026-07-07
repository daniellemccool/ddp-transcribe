//! Failure classification (Epic 3, ADR 0033). Policy layer over tool errors:
//! maps `FetchError` / `TranscribeError` to a three-arm verdict the pipeline
//! dispatches on. Patterns are evidence-derived from the 65k production run;
//! see the fixture corpus in tests/fixtures/yt_dlp_stderr/ and ADR 0033 for
//! the probe validation behind each verdict. Default-cautious: unmatched
//! input is Retryable, never Bug.

use crate::classification::{ClassificationTable, Disposition};
use crate::errors::{FetchError, TranscribeError};

/// Structural failure labels — failures that are facts about the process,
/// not yt-dlp opinions, so they stay code-mapped rather than living in the
/// operator-editable classification table. Same bare-variant spelling the
/// retired enums used; DB columns are TEXT throughout, so nothing stored
/// changes shape.
pub mod labels {
    pub const TOOL_TIMEOUT: &str = "ToolTimeout";
    pub const NETWORK_TRANSIENT: &str = "NetworkTransient";
    pub const YTDLP_OTHER: &str = "YtDlpOther";
    pub const TRANSCRIBE_OTHER: &str = "TranscribeOther";
}

// 0002: lifted in Epic 3 T07 — constructed within classify_fetch_error/
// classify_transcribe_error, both reached from `main()` via the pipeline
// dispatch this task wires.
#[derive(Debug, Clone)]
pub struct FailureContext {
    // 0002: `tool`/`exit_code`/`signal` are populated for every verdict but
    // only read by `Debug` (dead-code analysis ignores derived-trait reads,
    // per rustc's own diagnostic). Epic 3 T10 landed the triage subcommand
    // WITHOUT reading these: triage classifies the *stored* message text
    // directly via the classification table's `classify` (never
    // reconstructing a `FailureContext`), and its census aggregates on the
    // label string only — there is no raw tool/exit_code/signal display.
    // Still genuinely dead; re-tagged rather than lifted. Revisit if a
    // future task adds an operator-facing raw-context view.
    #[allow(dead_code)]
    pub tool: &'static str,
    #[allow(dead_code)]
    pub exit_code: Option<i32>,
    /// Unix signal that killed the tool, when applicable. Not read by the
    /// classifiers themselves or by T10's triage census (see note above).
    #[allow(dead_code)]
    pub signal: Option<i32>,
    pub stderr_excerpt: String,
    /// Which rule matched — audit trail for "why was this row written off".
    pub classification_reason: &'static str,
}

impl FailureContext {
    /// Message written to last_retryable_message / terminal_message. Leads
    /// with the matched rule so operators can grep verdicts, keeps the raw
    /// excerpt so nothing is lost.
    // 0002: lifted in Epic 3 T07 — dispatch calls this to build the
    // message/reason text persisted to state columns.
    pub fn message(&self) -> String {
        format!("[{}] {}", self.classification_reason, self.stderr_excerpt)
    }
}

/// Three-arm verdict the pipeline dispatches on. `label` is the tag
/// persisted to kind/reason columns (from the classification table for
/// message classes, from [`labels`] for structural ones).
/// `requires_cookie` marks rows whose retry only makes sense with cookies
/// attached (disposition `requires-cookie` in the active table) — the
/// failure-time decision in `record_fetch_failure` (T04) parks them when
/// no cookies are configured.
#[derive(Debug)]
pub enum ClassifiedFailure {
    Retryable {
        label: String,
        // 0002: populated by every Retryable arm (classify_fetch_error's
        // ToolFailed match on Disposition::RequiresCookie is the only arm
        // that sets this true) but not yet read by any dispatch site —
        // `fetch_worker`/`transcribe_worker`/`run_serial` all bind it as
        // `requires_cookie: _`. Epic 4a T04/T06 consume it via
        // `record_fetch_failure`; lift then.
        #[allow(dead_code)]
        requires_cookie: bool,
        ctx: FailureContext,
    },
    Unavailable {
        label: String,
        ctx: FailureContext,
    },
    Bug {
        ctx: FailureContext,
    },
}

// 0002: lifted in Epic 3 T07 — called by `classify_fetch_phase`
// (`src/pipeline/mod.rs`), reached from `main()` via `fetch_worker`/
// `run_serial`'s dispatch. Epic 4a T03: the `ToolFailed` arm's message
// classification now consults the operator-editable `ClassificationTable`
// instead of a hardcoded `classify_message` chain.
pub fn classify_fetch_error(e: &FetchError, table: &ClassificationTable) -> ClassifiedFailure {
    let ctx = |exit_code: Option<i32>, signal: Option<i32>, excerpt: &str, reason: &'static str| {
        FailureContext {
            tool: "yt-dlp",
            exit_code,
            signal,
            stderr_excerpt: excerpt.to_string(),
            classification_reason: reason,
        }
    };
    let retryable = |label: &str, ctx: FailureContext| ClassifiedFailure::Retryable {
        label: label.to_string(),
        requires_cookie: false,
        ctx,
    };
    match e {
        FetchError::ToolTimeout { duration, .. } => retryable(
            labels::TOOL_TIMEOUT,
            ctx(
                None,
                None,
                &format!("timed out after {duration:?}"),
                "tool timeout",
            ),
        ),
        FetchError::ToolNotFound { detail, .. } => ClassifiedFailure::Bug {
            ctx: ctx(
                None,
                None,
                detail,
                "tool binary missing: configuration broken",
            ),
        },
        FetchError::WorkDirCreate { path, detail } => ClassifiedFailure::Bug {
            ctx: ctx(
                None,
                None,
                &format!("{}: {detail}", path.display()),
                "work dir creation failed: environment broken",
            ),
        },
        FetchError::SystemIo { detail, .. } => retryable(
            labels::NETWORK_TRANSIENT,
            ctx(None, None, detail, "system io reading subprocess output"),
        ),
        FetchError::MissingOutput { path } => retryable(
            labels::YTDLP_OTHER,
            ctx(
                Some(0),
                None,
                &format!("{} missing after exit 0", path.display()),
                "yt-dlp exit 0 but expected wav missing",
            ),
        ),
        FetchError::NetworkError(detail) => retryable(
            labels::NETWORK_TRANSIENT,
            ctx(None, None, detail, "network error"),
        ),
        FetchError::ParseError(detail) => retryable(
            labels::YTDLP_OTHER,
            ctx(None, None, detail, "fetcher output parse failure"),
        ),
        FetchError::ToolFailed {
            exit_code,
            signal,
            stderr_excerpt,
            ..
        } => {
            let base = FailureContext {
                tool: "yt-dlp",
                exit_code: Some(*exit_code),
                signal: *signal,
                stderr_excerpt: stderr_excerpt.clone(),
                classification_reason: "stderr message class",
            };
            let m = table.classify(stderr_excerpt);
            match m.disposition {
                Disposition::Terminal => ClassifiedFailure::Unavailable {
                    label: m.label.to_string(),
                    ctx: base,
                },
                Disposition::Retryable => ClassifiedFailure::Retryable {
                    label: m.label.to_string(),
                    requires_cookie: false,
                    ctx: base,
                },
                Disposition::RequiresCookie => ClassifiedFailure::Retryable {
                    label: m.label.to_string(),
                    requires_cookie: true,
                    ctx: base,
                },
            }
        }
    }
}

// 0002: lifted in Epic 3 T07 — called by `transcribe_worker`'s error arm,
// reached from `main()` via `run_pipelined`. Transcribe errors are
// structural (no yt-dlp stderr to classify), so this keeps its unchanged
// signature — no `ClassificationTable` argument (Epic 4a T03 brief).
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
            label: labels::TRANSCRIBE_OTHER.to_string(),
            requires_cookie: false,
            ctx: ctx(
                detail.clone(),
                "wav decode failure: refetch may repair a corrupt download",
            ),
        },
        TranscribeError::Timeout { duration } => ClassifiedFailure::Retryable {
            label: labels::TOOL_TIMEOUT.to_string(),
            requires_cookie: false,
            ctx: ctx(
                format!("timed out after {duration:?}"),
                "transcribe timeout",
            ),
        },
        // Cancelled is handled by the worker before classification (worker
        // exits Ok); classifying it defensively as retryable keeps the fn
        // total without inventing a verdict the dispatch will ever act on.
        other => ClassifiedFailure::Retryable {
            label: labels::TRANSCRIBE_OTHER.to_string(),
            requires_cookie: false,
            ctx: ctx(
                other.to_string(),
                "unmatched transcribe error: default-cautious",
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../tests/fixtures/yt_dlp_stderr/", $name, ".txt"))
        };
    }

    #[test]
    #[allow(clippy::type_complexity)] // table-driven test; see tests/state_migrate.rs precedent
    fn fetch_error_arms_route_correctly() {
        use crate::errors::FetchError;
        use std::time::Duration;

        let table = ClassificationTable::compiled_default().unwrap();
        let cases: &[(FetchError, fn(&ClassifiedFailure) -> bool)] = &[
            (
                FetchError::ToolTimeout {
                    tool: "yt-dlp",
                    duration: Duration::from_secs(300),
                },
                |c| {
                    matches!(
                        c,
                        ClassifiedFailure::Retryable { label, .. } if label == labels::TOOL_TIMEOUT
                    )
                },
            ),
            (
                FetchError::ToolNotFound {
                    tool: "yt-dlp",
                    detail: "ENOENT".into(),
                },
                |c| matches!(c, ClassifiedFailure::Bug { .. }),
            ),
            (
                FetchError::WorkDirCreate {
                    path: "/nope".into(),
                    detail: "EACCES".into(),
                },
                |c| matches!(c, ClassifiedFailure::Bug { .. }),
            ),
            (
                FetchError::SystemIo {
                    tool: "yt-dlp",
                    detail: "pipe".into(),
                },
                |c| {
                    matches!(
                        c,
                        ClassifiedFailure::Retryable { label, .. } if label == labels::NETWORK_TRANSIENT
                    )
                },
            ),
            (
                FetchError::MissingOutput {
                    path: "/tmp/x.wav".into(),
                },
                |c| {
                    matches!(
                        c,
                        ClassifiedFailure::Retryable { label, .. } if label == labels::YTDLP_OTHER
                    )
                },
            ),
            (FetchError::NetworkError("dns".into()), |c| {
                matches!(
                    c,
                    ClassifiedFailure::Retryable { label, .. } if label == labels::NETWORK_TRANSIENT
                )
            }),
        ];
        for (err, check) in cases {
            let got = classify_fetch_error(err, &table);
            assert!(check(&got), "wrong classification for {err:?}: {got:?}");
        }
    }

    #[test]
    fn tool_failed_with_write_off_message_is_unavailable() {
        use crate::errors::FetchError;
        let table = ClassificationTable::compiled_default().unwrap();
        let e = FetchError::ToolFailed {
            tool: "yt-dlp",
            exit_code: 1,
            signal: None,
            stderr_excerpt: fixture!("ip_blocked").to_string(),
        };
        match classify_fetch_error(&e, &table) {
            ClassifiedFailure::Unavailable { label, ctx } => {
                assert_eq!(label, "IpBlockedMessage");
                assert_eq!(ctx.exit_code, Some(1));
                assert!(!ctx.classification_reason.is_empty());
            }
            other => panic!("expected Unavailable(IpBlockedMessage), got {other:?}"),
        }
    }

    #[test]
    fn status_code_10240_is_terminal_now() {
        use crate::errors::FetchError;
        let table = ClassificationTable::compiled_default().unwrap();
        let e = FetchError::ToolFailed {
            tool: "yt-dlp",
            exit_code: 1,
            signal: None,
            stderr_excerpt: fixture!("video_not_available_10240").to_string(),
        };
        match classify_fetch_error(&e, &table) {
            ClassifiedFailure::Unavailable { label, .. } => {
                assert_eq!(label, "VideoNotAvailable10240");
            }
            other => panic!("10240 must be terminal (census n=606, 100% dead), got {other:?}"),
        }
    }

    /// A `requires-cookie`-dispositioned message class must surface as
    /// `Retryable { requires_cookie: true, .. }` — Tasks 04/06 park these
    /// rows via `record_fetch_failure` when no cookies are configured. A
    /// plain-retryable class must keep `requires_cookie == false`.
    #[test]
    fn requires_cookie_disposition_sets_the_retryable_flag() {
        use crate::errors::FetchError;
        let table = ClassificationTable::compiled_default().unwrap();
        let mk = |stderr: &str| FetchError::ToolFailed {
            tool: "yt-dlp",
            exit_code: 1,
            signal: None,
            stderr_excerpt: stderr.to_string(),
        };
        match classify_fetch_error(&mk(fixture!("sensitive_login_gated")), &table) {
            ClassifiedFailure::Retryable {
                label,
                requires_cookie,
                ..
            } => {
                assert_eq!(label, "SensitiveLoginGated");
                assert!(
                    requires_cookie,
                    "requires-cookie disposition must set requires_cookie"
                );
            }
            other => panic!("expected Retryable(SensitiveLoginGated), got {other:?}"),
        }
        match classify_fetch_error(&mk(fixture!("no_data_blocks")), &table) {
            ClassifiedFailure::Retryable {
                label,
                requires_cookie,
                ..
            } => {
                assert_eq!(label, "NoDataBlocks");
                assert!(
                    !requires_cookie,
                    "plain retryable disposition must not set requires_cookie"
                );
            }
            other => panic!("expected Retryable(NoDataBlocks), got {other:?}"),
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
            classify_transcribe_error(&TranscribeError::AudioDecode {
                detail: "truncated".into()
            }),
            ClassifiedFailure::Retryable { label, .. } if label == labels::TRANSCRIBE_OTHER
        ));
    }
}
