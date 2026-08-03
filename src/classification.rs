//! Operator-editable classification policy for yt-dlp stderr (Epic 4a).
//!
//! An ordered first-match table maps stderr substrings to a label (stored in
//! the DB kind/reason columns) and a disposition. The compiled-in default is
//! evidence-derived from the 65k pilot + the 2026-07-07 probe census; a
//! `--classification <path>` TOML file replaces it wholesale. Validation
//! hard-fails at startup (0022 philosophy): a batch never runs under a
//! half-understood policy. The active table's TOML text is snapshotted into
//! `batch_runs` for attrition provenance.
//!
//! Boundary: this table interprets TOOL OUTPUT (yt-dlp stderr) only.
//! Structural errors (tool missing, timeout, decode) stay code-mapped in
//! `src/failure.rs`.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// What the pipeline does with a matched message class.
// 0002: lifted in Epic 4a T03 — consumed by `classify_fetch_error`'s
// `ClassifiedFailure` mapping and `cookie_opts_for`'s gate, both reached
// from `main()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    Retryable,
    Terminal,
    RequiresCookie,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    pattern: String,
    label: String,
    disposition: Disposition,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFallback {
    label: String,
    disposition: Disposition,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    schema: u32,
    fallback: RawFallback,
    #[serde(default, rename = "rule")]
    rules: Vec<RawRule>,
}

/// Immutable, validated classification policy. Shared with workers via Arc.
// Constructed by the Process dispatch arm and consumed throughout
// `failure::classify_fetch_error` / `pipeline::cookie_opts_for`.
#[derive(Debug)]
pub struct ClassificationTable {
    rules: Vec<RawRule>,
    fallback: RawFallback,
    by_label: HashMap<String, Disposition>,
    // Populated for every table; read by `source_toml()`, which the status
    // renderer and the `batch_runs.policy_toml` snapshot both call.
    source: String,
}

/// One classification outcome. `label` borrows from the table (labels are
/// table-owned strings; callers `.to_string()` at persistence boundaries).
// 0002: lifted in Epic 4a T03 — `classify_fetch_error`'s `ToolFailed` arm
// matches on `disposition` and stores `label`.
#[derive(Debug, Clone, Copy)]
pub struct MessageMatch<'a> {
    pub label: &'a str,
    pub disposition: Disposition,
    /// True when an actual rule matched; false when this outcome is the
    /// table's fallback. Epic 4a T07 review fix: the sweep must not
    /// overwrite a real stored kind (e.g. `ToolTimeout`) with the fallback
    /// label — a fallback hit carries no positive evidence about the
    /// message class, only "nothing matched".
    pub matched_rule: bool,
}

impl ClassificationTable {
    /// Parse + validate. Hard-fail semantics: any error here must abort
    /// startup before a single row is claimed (0022 philosophy).
    pub fn from_toml_str(text: &str) -> Result<ClassificationTable> {
        let raw: RawTable = toml::from_str(text).context("parsing classification TOML")?;
        if raw.schema != 1 {
            bail!(
                "classification: unsupported schema {} (expected 1)",
                raw.schema
            );
        }
        if raw.rules.is_empty() {
            bail!("classification: rule list is empty — refusing to run without policy");
        }
        if raw.fallback.disposition == Disposition::RequiresCookie {
            bail!("classification: fallback disposition must be retryable or terminal, not requires-cookie");
        }
        if raw.fallback.label.trim().is_empty() {
            bail!("classification: fallback label is empty");
        }
        let mut by_label: HashMap<String, Disposition> = HashMap::new();
        for (i, r) in raw.rules.iter().enumerate() {
            if r.pattern.trim().is_empty() {
                bail!("classification: rule {} has an empty pattern", i + 1);
            }
            if r.label.trim().is_empty() {
                bail!("classification: rule {} has an empty label", i + 1);
            }
            if let Some(prev) = by_label.insert(r.label.clone(), r.disposition) {
                if prev != r.disposition {
                    bail!(
                        "classification: label {:?} maps to two dispositions ({prev:?} vs {:?})",
                        r.label,
                        r.disposition
                    );
                }
            }
        }
        if let Some(&prev) = by_label.get(&raw.fallback.label) {
            if prev != raw.fallback.disposition {
                bail!(
                    "classification: fallback label {:?} maps to two dispositions ({prev:?} vs {:?})",
                    raw.fallback.label,
                    raw.fallback.disposition
                );
            }
        }
        by_label
            .entry(raw.fallback.label.clone())
            .or_insert(raw.fallback.disposition);
        Ok(ClassificationTable {
            by_label,
            source: text.to_string(),
            fallback: raw.fallback,
            rules: raw.rules,
        })
    }

    /// The evidence-derived default, compiled into the binary so the tool is
    /// self-contained. Returns Result (not a lazy static) so main can
    /// hard-fail with context; a unit test pins that it always parses.
    pub fn compiled_default() -> Result<ClassificationTable> {
        Self::from_toml_str(DEFAULT_TABLE_TOML).context("compiled default classification table")
    }

    /// Ordered first-match over exact case-sensitive substrings; fallback
    /// otherwise. Mirrors the retired hardcoded chain's contract exactly.
    pub fn classify(&self, message: &str) -> MessageMatch<'_> {
        for r in &self.rules {
            if message.contains(&r.pattern) {
                return MessageMatch {
                    label: &r.label,
                    disposition: r.disposition,
                    matched_rule: true,
                };
            }
        }
        MessageMatch {
            label: &self.fallback.label,
            disposition: self.fallback.disposition,
            matched_rule: false,
        }
    }

    /// Disposition for a stored label (claim-time cookie gate, sweep of
    /// rows whose kind was written by an earlier run). None for labels the
    /// active table doesn't know (e.g. historical placeholder "Fetch").
    pub fn disposition_of(&self, label: &str) -> Option<Disposition> {
        self.by_label.get(label).copied()
    }

    // Consumed by `status.rs`'s policy render and the
    // `batch_runs.policy_toml` snapshot (`tests/batch_census.rs`).
    pub fn source_toml(&self) -> &str {
        &self.source
    }

    // The Process dispatch arm logs `rule_count()` on the "classification
    // policy active" line.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// The compiled-in default policy. Every rule carries its evidence citation;
/// this text is also what lands in `batch_runs.policy_toml` when no
/// `--classification` override is given.
// `compiled_default()` parses this const.
pub const DEFAULT_TABLE_TOML: &str = r#"# ddp-transcribe classification policy (compiled default)
# Ordered, first-match-wins, exact case-sensitive substrings. Evidence:
# 65k pilot corpus + oEmbed probe census 2026-07-07 (n=7,087); ADR 0033
# and its comments. Edit via --classification <file>, not here.
schema = 1
# Default-cautious: unknown messages retry once and let the re-fetch
# adjudicate (fetch-as-oracle).
fallback = { label = "YtDlpOther", disposition = "retryable" }

[[rule]]
# yt-dlp MISFIRE: TikTok returns this text for DELETED content. It is NOT
# an IP issue (probe 10/10 dead; same-egress re-fetches cleared the IP —
# ADR-0033 comment 2026-07-07). Census: 3,241 rows. Re-verified 2026-08-03:
# 12/12 dead by manual browser probe from a second (residential NL) egress.
pattern = "Your IP address is blocked"
label = "IpBlockedMessage"
disposition = "terminal"

[[rule]]
# Probe-validated 5/5 dead (2026-07-06); census: 68 rows. Semantics refined
# by manual browser probe 2026-08-03 (n=4, residential NL egress): all render
# "not available in your region" — 10231 is a REGION LOCK, not removal.
# Terminal remains correct for this study: the campaign VM and the operator
# share the NL vantage, so no egress we own can reach these.
pattern = "status code 10231"
label = "VideoNotAvailable10231"
disposition = "terminal"

[[rule]]
# Census 2026-07-07: 606/606 probe-dead, single exact message — the entire
# former YtDlpOther population. Match the SPECIFIC code: unknown future
# codes must fall through to the retryable fallback and earn terminal
# status the way this one did. Re-verified 2026-08-03: 4/4 dead by manual
# browser probe from a second (residential NL) egress.
pattern = "status code 10240"
label = "VideoNotAvailable10240"
disposition = "terminal"

[[rule]]
# 2,311/2,318 alive at census; re-fetch recovers (10/10 in the evidence
# session).
pattern = "Did not get any data blocks"
label = "NoDataBlocks"
disposition = "retryable"

[[rule]]
# IMPURE class: 427 dead / 25 ALIVE at census (5.5%). A terminal
# disposition here would silently discard recoverable videos — the
# re-fetch adjudicates per row.
pattern = "do not have permission to view this post"
label = "NoPermission"
disposition = "retryable"

[[rule]]
# Login-gated sensitive content; 301 rows, 5/5 alive at probe. Retries only
# make sense with cookies attached (ADR 0035 scope).
pattern = "not be comfortable for some audiences"
label = "SensitiveLoginGated"
disposition = "requires-cookie"

[[rule]]
pattern = "No video formats found"
label = "NoVideoFormats"
disposition = "retryable"

[[rule]]
pattern = "unable to obtain file audio codec with ffprobe"
label = "FfprobePostprocess"
disposition = "retryable"

[[rule]]
pattern = "HTTP Error"
label = "HttpError"
disposition = "retryable"

# Network markers (one rule each, shared label). Transient by definition.
[[rule]]
pattern = "Unable to download webpage"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "HTTPSConnectionPool"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "Connection aborted"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "ConnectionResetError"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "RemoteDisconnected"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "curl: (28)"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "SSL"
label = "NetworkTransient"
disposition = "retryable"

[[rule]]
pattern = "Too Many Requests"
label = "NetworkTransient"
disposition = "retryable"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../tests/fixtures/yt_dlp_stderr/", $name, ".txt"))
        };
    }

    #[test]
    fn default_table_parses_and_classifies_the_corpus() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        let cases: &[(&str, &str, Disposition)] = &[
            (
                fixture!("ip_blocked"),
                "IpBlockedMessage",
                Disposition::Terminal,
            ),
            (
                fixture!("video_not_available_10231"),
                "VideoNotAvailable10231",
                Disposition::Terminal,
            ),
            (
                fixture!("video_not_available_10240"),
                "VideoNotAvailable10240",
                Disposition::Terminal,
            ),
            (
                fixture!("no_data_blocks"),
                "NoDataBlocks",
                Disposition::Retryable,
            ),
            // Census 2026-07-07: 25/452 alive — impure class, MUST stay retryable.
            (
                fixture!("no_permission"),
                "NoPermission",
                Disposition::Retryable,
            ),
            (
                fixture!("sensitive_login_gated"),
                "SensitiveLoginGated",
                Disposition::RequiresCookie,
            ),
            (
                fixture!("no_video_formats"),
                "NoVideoFormats",
                Disposition::Retryable,
            ),
            (
                fixture!("ffprobe_postprocess"),
                "FfprobePostprocess",
                Disposition::Retryable,
            ),
            (
                fixture!("http_error_403"),
                "HttpError",
                Disposition::Retryable,
            ),
            (
                fixture!("network_transient"),
                "NetworkTransient",
                Disposition::Retryable,
            ),
        ];
        for (msg, label, disposition) in cases {
            let m = t.classify(msg);
            assert_eq!(m.label, *label, "label for: {msg}");
            assert_eq!(m.disposition, *disposition, "disposition for: {msg}");
            assert!(m.matched_rule, "corpus hit must report a rule match");
        }
    }

    #[test]
    fn unmatched_message_hits_the_fallback() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        let m = t.classify("ERROR: some yt-dlp message we have never seen");
        assert_eq!(m.label, "YtDlpOther");
        assert_eq!(m.disposition, Disposition::Retryable);
        assert!(!m.matched_rule, "fallback must not report a rule match");
    }

    #[test]
    fn first_match_wins_on_overlapping_patterns() {
        let toml = r#"
schema = 1
fallback = { label = "Other", disposition = "retryable" }
[[rule]]
pattern = "status code"
label = "AnyStatusCode"
disposition = "retryable"
[[rule]]
pattern = "status code 10231"
label = "Specific10231"
disposition = "terminal"
"#;
        let t = ClassificationTable::from_toml_str(toml).expect("valid table");
        // The generic rule sits FIRST, so it wins even though the specific
        // rule also matches — order is the contract, exactly like the old
        // hardcoded chain.
        let m = t.classify("blah status code 10231 blah");
        assert_eq!(m.label, "AnyStatusCode");
    }

    /// Epic 3 close-out bundle: the compiled default's rule ORDER is
    /// load-bearing (write-off classes first, network markers last), and
    /// `classify` is `str::contains`, so a real yt-dlp blob carrying two
    /// markers at once resolves by TABLE position — never by where the
    /// markers sit in the text. `first_match_wins_on_overlapping_patterns`
    /// pins the mechanism on a synthetic table; this pins the shipped
    /// ordering that the pilot evidence bought.
    #[test]
    fn default_table_precedence_holds_when_one_blob_carries_two_markers() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        // (earlier marker, later marker, winning label, winning disposition)
        let cases: &[(&str, &str, &str, Disposition)] = &[
            // Terminal write-off (rule 1) beats a retryable class (rule 4) —
            // the ordering ADR-0033 exists to protect.
            (
                "ERROR: Your IP address is blocked",
                "ERROR: Did not get any data blocks",
                "IpBlockedMessage",
                Disposition::Terminal,
            ),
            // Terminal write-off beats a network marker (last block).
            (
                "ERROR: unable to extract: status code 10240",
                "ERROR: Unable to download webpage: Too Many Requests",
                "VideoNotAvailable10240",
                Disposition::Terminal,
            ),
            // Between two retryables, the earlier rule still wins: the
            // message class (rule 9) outranks the network markers below it.
            (
                "ERROR: HTTP Error 403: Forbidden",
                "ERROR: SSL handshake failed",
                "HttpError",
                Disposition::Retryable,
            ),
        ];
        for (earlier, later, label, disposition) in cases {
            // Both concatenation orders: table position decides, not the
            // byte offset of the marker inside the blob.
            for blob in [format!("{earlier}\n{later}"), format!("{later}\n{earlier}")] {
                let m = t.classify(&blob);
                assert_eq!(m.label, *label, "label for blob: {blob:?}");
                assert_eq!(
                    m.disposition, *disposition,
                    "disposition for blob: {blob:?}"
                );
                assert!(m.matched_rule, "a two-marker blob is never a fallback hit");
            }
        }
    }

    /// Matching is exact and case-SENSITIVE (documented on `classify`).
    /// yt-dlp/TikTok wording casing has drifted before, so pin the
    /// consequence: a case-shifted marker does not match its rule and lands
    /// on the default-cautious retryable fallback rather than being written
    /// off terminal by accident.
    #[test]
    fn classification_is_case_sensitive() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        for shifted in [
            "ERROR: your ip address is blocked",      // was terminal
            "ERROR: STATUS CODE 10240",               // was terminal
            "ERROR: did not get any data blocks",     // was NoDataBlocks
            "ERROR: http error 403: forbidden",       // was HttpError
            "ERROR: unable to download WEBPAGE: ssl", // was NetworkTransient
            "ERROR: NO VIDEO FORMATS FOUND",          // was NoVideoFormats
        ] {
            let m = t.classify(shifted);
            assert!(
                !m.matched_rule,
                "case-shifted marker must not match a rule: {shifted:?} matched {}",
                m.label
            );
            assert_eq!(m.label, "YtDlpOther", "for {shifted:?}");
            assert_eq!(m.disposition, Disposition::Retryable, "for {shifted:?}");
        }
        // Control: the exactly-cased forms of the same two markers DO match,
        // so the assertions above are about casing and nothing else.
        assert_eq!(
            t.classify("ERROR: Your IP address is blocked").label,
            "IpBlockedMessage"
        );
        assert_eq!(
            t.classify("ERROR: No video formats found").label,
            "NoVideoFormats"
        );
    }

    #[test]
    fn validation_hard_fails() {
        let base = |body: &str| {
            format!(
                "schema = 1\nfallback = {{ label = \"F\", disposition = \"retryable\" }}\n{body}"
            )
        };
        // Empty rule list.
        assert!(ClassificationTable::from_toml_str(
            "schema = 1\nfallback = { label = \"F\", disposition = \"retryable\" }\n"
        )
        .is_err());
        // Empty pattern.
        assert!(ClassificationTable::from_toml_str(&base(
            "[[rule]]\npattern = \"\"\nlabel = \"X\"\ndisposition = \"retryable\"\n"
        ))
        .is_err());
        // Empty label.
        assert!(ClassificationTable::from_toml_str(&base(
            "[[rule]]\npattern = \"p\"\nlabel = \"\"\ndisposition = \"retryable\"\n"
        ))
        .is_err());
        // Unknown disposition (serde enum rejects).
        assert!(ClassificationTable::from_toml_str(&base(
            "[[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"maybe\"\n"
        ))
        .is_err());
        // Same label, two different dispositions.
        assert!(ClassificationTable::from_toml_str(&base(
            "[[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"retryable\"\n\
             [[rule]]\npattern = \"q\"\nlabel = \"X\"\ndisposition = \"terminal\"\n"
        ))
        .is_err());
        // Wrong schema number.
        assert!(ClassificationTable::from_toml_str(
            "schema = 2\nfallback = { label = \"F\", disposition = \"retryable\" }\n\
             [[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"retryable\"\n"
        )
        .is_err());
        // requires-cookie as fallback is rejected (a blind cookie fallback
        // would park every unknown message).
        assert!(ClassificationTable::from_toml_str(
            "schema = 1\nfallback = { label = \"F\", disposition = \"requires-cookie\" }\n\
             [[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"retryable\"\n"
        )
        .is_err());
        // Whitespace-only pattern (first-match-wins poison: it would shadow
        // every later rule for any message containing a space).
        assert!(ClassificationTable::from_toml_str(&base(
            "[[rule]]\npattern = \"   \"\nlabel = \"X\"\ndisposition = \"retryable\"\n"
        ))
        .is_err());
        // Unknown field inside a [[rule]] (a misspelled key must not be
        // silently dropped).
        assert!(ClassificationTable::from_toml_str(&base(
            "[[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"retryable\"\npriority = 1\n"
        ))
        .is_err());
        // Unknown field inside the fallback block.
        assert!(ClassificationTable::from_toml_str(
            "schema = 1\nfallback = { label = \"F\", disposition = \"retryable\", extra = 1 }\n\
             [[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"retryable\"\n"
        )
        .is_err());
        // Fallback label collides with a rule label under a DIFFERENT
        // disposition — disposition_of() would disagree with what classify()
        // returns for unmatched messages.
        assert!(ClassificationTable::from_toml_str(
            "schema = 1\nfallback = { label = \"X\", disposition = \"retryable\" }\n\
             [[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"terminal\"\n"
        )
        .is_err());
    }

    #[test]
    fn fallback_label_shared_with_same_disposition_rule_is_accepted() {
        // Same label, SAME disposition: accepted — and disposition_of()
        // resolves the fallback label consistently with classify().
        let t = ClassificationTable::from_toml_str(
            "schema = 1\nfallback = { label = \"X\", disposition = \"retryable\" }\n\
             [[rule]]\npattern = \"p\"\nlabel = \"X\"\ndisposition = \"retryable\"\n",
        )
        .expect("same-disposition duplicate must be accepted");
        assert_eq!(t.disposition_of("X"), Some(Disposition::Retryable));
        assert_eq!(t.classify("no match here").label, "X");
    }

    #[test]
    fn disposition_of_covers_rules_and_fallback() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        assert_eq!(
            t.disposition_of("SensitiveLoginGated"),
            Some(Disposition::RequiresCookie)
        );
        assert_eq!(
            t.disposition_of("IpBlockedMessage"),
            Some(Disposition::Terminal)
        );
        assert_eq!(t.disposition_of("YtDlpOther"), Some(Disposition::Retryable));
        assert_eq!(t.disposition_of("NoSuchLabel"), None);
    }

    #[test]
    fn source_toml_round_trips() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        assert_eq!(t.source_toml(), DEFAULT_TABLE_TOML);
        assert!(t.rule_count() >= 17, "9 message rules + 8 network markers");
    }
}
