//! Failure classification (Epic 3, ADR 0033). Policy layer over tool errors:
//! maps `FetchError` / `TranscribeError` to a three-arm verdict the pipeline
//! dispatches on. Patterns are evidence-derived from the 65k production run;
//! see the fixture corpus in tests/fixtures/yt_dlp_stderr/ and ADR 0033 for
//! the evidence behind each verdict. Default-cautious: unmatched
//! input is Retryable, never Bug.

use crate::classification::{ClassificationTable, Disposition};
use crate::errors::{FetchError, TranscribeError};

/// Structural failure labels — failures that are facts about the process,
/// not yt-dlp opinions, so they stay code-mapped rather than living in the
/// operator-editable classification table. Same bare-variant spelling the
/// retired enums used; DB columns are TEXT throughout, so nothing stored
/// changes shape.
pub(crate) mod labels {
    pub(crate) const TOOL_TIMEOUT: &str = "ToolTimeout";
    pub(crate) const NETWORK_TRANSIENT: &str = "NetworkTransient";
    pub(crate) const YTDLP_OTHER: &str = "YtDlpOther";
    pub(crate) const TRANSCRIBE_OTHER: &str = "TranscribeOther";
}

// Constructed within classify_fetch_error/classify_transcribe_error, both
// reached from the pipeline dispatch.
//
// 0002 (final-review fix): this struct carried `tool`/`exit_code`/`signal`
// fields that were populated on every verdict but never read outside the
// derived `Debug`/`Clone` impls. While `failure` was `pub`, external
// reachability shielded them from `dead_code`; narrowing the module to
// `pub(crate)` (finding 2) exposed the dead fields, and they were removed
// rather than suppressed per the ADR — no named consumer was landing for
// them. If a raw tool/exit_code/signal view is ever needed (e.g. an
// operator-facing detail beyond the label string), reintroduce the fields
// alongside that reader rather than reserving them speculatively.
#[derive(Debug, Clone)]
pub(crate) struct FailureContext {
    pub(crate) stderr_excerpt: String,
    /// Which rule matched — audit trail for "why was this row written off".
    pub(crate) classification_reason: &'static str,
}

impl FailureContext {
    /// Message written to last_retryable_message / terminal_message. Leads
    /// with the matched rule so operators can grep verdicts, keeps the raw
    /// excerpt so nothing is lost.
    // Dispatch calls this to build the message/reason text persisted to
    // state columns.
    pub(crate) fn message(&self) -> String {
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
pub(crate) enum ClassifiedFailure {
    Retryable {
        label: String,
        // Populated by every Retryable arm (classify_fetch_error's ToolFailed
        // match on Disposition::RequiresCookie is the only arm that sets this
        // true) and read by the pipelined workers, which thread it into
        // `Store::record_fetch_failure` to park cookie-needing rows when no
        // cookies are configured.
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

// Called by `classify_fetch_phase` (`src/pipeline/mod.rs`), reached from the
// dispatch path via `fetch_worker`/`run_serial`. Epic 4a T03: the `ToolFailed`
// arm's message classification consults the operator-editable
// `ClassificationTable` instead of a hardcoded `classify_message` chain.
pub(crate) fn classify_fetch_error(
    e: &FetchError,
    table: &ClassificationTable,
) -> ClassifiedFailure {
    let ctx = |excerpt: &str, reason: &'static str| FailureContext {
        stderr_excerpt: excerpt.to_string(),
        classification_reason: reason,
    };
    let retryable = |label: &str, ctx: FailureContext| ClassifiedFailure::Retryable {
        label: label.to_string(),
        requires_cookie: false,
        ctx,
    };
    match e {
        FetchError::ToolTimeout { duration, .. } => retryable(
            labels::TOOL_TIMEOUT,
            ctx(&format!("timed out after {duration:?}"), "tool timeout"),
        ),
        FetchError::ToolNotFound { detail, .. } => ClassifiedFailure::Bug {
            ctx: ctx(detail, "tool binary missing: configuration broken"),
        },
        FetchError::WorkDirCreate { path, detail } => ClassifiedFailure::Bug {
            ctx: ctx(
                &format!("{}: {detail}", path.display()),
                "work dir creation failed: environment broken",
            ),
        },
        FetchError::SystemIo { detail, .. } => retryable(
            labels::NETWORK_TRANSIENT,
            ctx(detail, "system io reading subprocess output"),
        ),
        FetchError::MissingOutput { path } => retryable(
            labels::YTDLP_OTHER,
            ctx(
                &format!("{} missing after exit 0", path.display()),
                "yt-dlp exit 0 but expected wav missing",
            ),
        ),
        // Epic 5b: same class as MissingOutput — yt-dlp exited 0 but the
        // attempt dir's contents don't match the one-wav contract. Retryable
        // rather than Bug: a re-fetch lands in a FRESH dir, and a Bug verdict
        // would cancel the whole batch (0025) over one video's output shape.
        FetchError::AmbiguousOutput { dir, count } => retryable(
            labels::YTDLP_OTHER,
            ctx(
                &format!("{count} wav files in {} after exit 0", dir.display()),
                "yt-dlp exit 0 but attempt dir holds more than one wav",
            ),
        ),
        FetchError::NetworkError(detail) => {
            retryable(labels::NETWORK_TRANSIENT, ctx(detail, "network error"))
        }
        FetchError::ParseError(detail) => retryable(
            labels::YTDLP_OTHER,
            ctx(detail, "fetcher output parse failure"),
        ),
        FetchError::ToolFailed { stderr_excerpt, .. } => {
            let base = FailureContext {
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
pub(crate) fn classify_transcribe_error(e: &TranscribeError) -> ClassifiedFailure {
    let ctx = |excerpt: String, reason: &'static str| FailureContext {
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
                    tool: "yt-dlp".to_string(),
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
                    tool: "yt-dlp".to_string(),
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
                    tool: "yt-dlp".to_string(),
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
            tool: "yt-dlp".to_string(),
            exit_code: 1,
            signal: None,
            stderr_excerpt: fixture!("ip_blocked").to_string(),
        };
        match classify_fetch_error(&e, &table) {
            ClassifiedFailure::Unavailable { label, ctx } => {
                assert_eq!(label, "IpBlockedMessage");
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
            tool: "yt-dlp".to_string(),
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
            tool: "yt-dlp".to_string(),
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
