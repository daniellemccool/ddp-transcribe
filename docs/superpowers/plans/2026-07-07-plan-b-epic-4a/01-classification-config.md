# Task 01: Classification config module (`src/classification.rs`)

**Files:**
- Create: `src/classification.rs`
- Create: `tests/fixtures/yt_dlp_stderr/video_not_available_10240.txt`
- Modify: `Cargo.toml` (add `toml = "0.8"` dependency)
- Modify: `src/lib.rs` (module registration, between `pub mod canonical;` and `pub mod cli;`)
- Modify: `src/main.rs` (module registration, between `mod canonical;` and `mod cli;`)
- Test: in-module `#[cfg(test)]` tests in `src/classification.rs`

**Interfaces:**
- Consumes: nothing from other tasks (first task).
- Produces (later tasks rely on these EXACT signatures):
  - `pub enum Disposition { Retryable, Terminal, RequiresCookie }` (Copy, Eq)
  - `pub struct ClassificationTable` with:
    - `pub fn from_toml_str(text: &str) -> anyhow::Result<ClassificationTable>`
    - `pub fn compiled_default() -> anyhow::Result<ClassificationTable>`
    - `pub fn classify(&self, message: &str) -> MessageMatch<'_>`
    - `pub fn disposition_of(&self, label: &str) -> Option<Disposition>`
    - `pub fn source_toml(&self) -> &str` (the exact TOML text this table was parsed from — batch provenance)
    - `pub fn rule_count(&self) -> usize`
  - `pub struct MessageMatch<'a> { pub label: &'a str, pub disposition: Disposition }`
  - `pub const DEFAULT_TABLE_TOML: &str`

All new public items will have no caller until Task 03 — add `#[allow(dead_code)]` per ADR-0002 two-part convention with inline comments `// 0002: consumed by Epic 4a T03 (classifier rewire) / T06 (CLI); lift when they land.` and a "0002 dead-code note:" paragraph in the commit message.

- [ ] **Step 1: Add the `toml` dependency**

In `Cargo.toml` `[dependencies]`, after the `serde_json = "1"` line, add:

```toml
toml = "0.8"
```

- [ ] **Step 2: Create the 10240 fixture from the production DB**

```bash
sqlite3 -readonly /home/dmm/src/uu-tiktok/ddp-run-export.sqlite \
  "SELECT last_retryable_message FROM videos
   WHERE status='failed_retryable' AND last_retryable_message LIKE '%status code 10240%'
   LIMIT 1;" > tests/fixtures/yt_dlp_stderr/video_not_available_10240.txt
cat tests/fixtures/yt_dlp_stderr/video_not_available_10240.txt
```

Expected: one line containing `Video not available, status code 10240`. If the DB is unavailable, write the file with this literal content instead (disclose per 0003):

```
fetching 7607818696786103574: subprocess `yt-dlp` exited with status 1: ERROR: [TikTok] 7607818696786103574: Video not available, status code 10240; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues
```

Also append one line to `tests/fixtures/yt_dlp_stderr/README.md`: `- video_not_available_10240.txt — census 2026-07-07: 606/606 probe-dead, single exact message (former YtDlpOther population).`

- [ ] **Step 3: Write the failing test file skeleton inside the new module**

Create `src/classification.rs` with ONLY the test module first (module registration in Step 4 makes it compile-visible):

```rust
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
            (fixture!("ip_blocked"), "IpBlockedMessage", Disposition::Terminal),
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
            (fixture!("no_data_blocks"), "NoDataBlocks", Disposition::Retryable),
            // Census 2026-07-07: 25/452 alive — impure class, MUST stay retryable.
            (fixture!("no_permission"), "NoPermission", Disposition::Retryable),
            (
                fixture!("sensitive_login_gated"),
                "SensitiveLoginGated",
                Disposition::RequiresCookie,
            ),
            (fixture!("no_video_formats"), "NoVideoFormats", Disposition::Retryable),
            (
                fixture!("ffprobe_postprocess"),
                "FfprobePostprocess",
                Disposition::Retryable,
            ),
            (fixture!("http_error_403"), "HttpError", Disposition::Retryable),
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
        }
    }

    #[test]
    fn unmatched_message_hits_the_fallback() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        let m = t.classify("ERROR: some yt-dlp message we have never seen");
        assert_eq!(m.label, "YtDlpOther");
        assert_eq!(m.disposition, Disposition::Retryable);
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

    #[test]
    fn validation_hard_fails() {
        let base = |body: &str| format!(
            "schema = 1\nfallback = {{ label = \"F\", disposition = \"retryable\" }}\n{body}"
        );
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
    }

    #[test]
    fn disposition_of_covers_rules_and_fallback() {
        let t = ClassificationTable::compiled_default().expect("default table must parse");
        assert_eq!(
            t.disposition_of("SensitiveLoginGated"),
            Some(Disposition::RequiresCookie)
        );
        assert_eq!(t.disposition_of("IpBlockedMessage"), Some(Disposition::Terminal));
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
```

- [ ] **Step 4: Register the module and run the tests to verify they fail**

In `src/lib.rs`, between `pub mod canonical;` and `pub mod cli;` insert `pub mod classification;`. In `src/main.rs`, between `mod canonical;` and `mod cli;` insert `mod classification;`.

Run: `cargo test --lib classification -- --test-threads=1`
Expected: COMPILE FAILURE — `Disposition`, `ClassificationTable`, `DEFAULT_TABLE_TOML` not found. That is the RED state.

- [ ] **Step 5: Implement the module above the test block**

```rust
use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// What the pipeline does with a matched message class.
// 0002: consumed by Epic 4a T03 (classifier rewire) / T06 (dispatch+CLI);
// lift when they land.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Disposition {
    Retryable,
    Terminal,
    RequiresCookie,
}

#[derive(Debug, Deserialize)]
struct RawRule {
    pattern: String,
    label: String,
    disposition: Disposition,
}

#[derive(Debug, Deserialize)]
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
// 0002: consumed by Epic 4a T03/T06/T07; lift when they land.
#[allow(dead_code)]
#[derive(Debug)]
pub struct ClassificationTable {
    rules: Vec<RawRule>,
    fallback: RawFallback,
    by_label: HashMap<String, Disposition>,
    source: String,
}

/// One classification outcome. `label` borrows from the table (labels are
/// table-owned strings; callers `.to_string()` at persistence boundaries).
// 0002: consumed by Epic 4a T03; lift when it lands.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct MessageMatch<'a> {
    pub label: &'a str,
    pub disposition: Disposition,
}

#[allow(dead_code)] // 0002: consumed by Epic 4a T03/T06/T07; lift when they land.
impl ClassificationTable {
    /// Parse + validate. Hard-fail semantics: any error here must abort
    /// startup before a single row is claimed (0022 philosophy).
    pub fn from_toml_str(text: &str) -> Result<ClassificationTable> {
        let raw: RawTable = toml::from_str(text).context("parsing classification TOML")?;
        if raw.schema != 1 {
            bail!("classification: unsupported schema {} (expected 1)", raw.schema);
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
            if r.pattern.is_empty() {
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
                };
            }
        }
        MessageMatch {
            label: &self.fallback.label,
            disposition: self.fallback.disposition,
        }
    }

    /// Disposition for a stored label (claim-time cookie gate, sweep of
    /// rows whose kind was written by an earlier run). None for labels the
    /// active table doesn't know (e.g. historical placeholder "Fetch").
    pub fn disposition_of(&self, label: &str) -> Option<Disposition> {
        self.by_label.get(label).copied()
    }

    pub fn source_toml(&self) -> &str {
        &self.source
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// The compiled-in default policy. Every rule carries its evidence citation;
/// this text is also what lands in `batch_runs.policy_toml` when no
/// `--classification` override is given.
// 0002: consumed by Epic 4a T06 (CLI wiring); lift when it lands.
#[allow(dead_code)]
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
# ADR-0033 comment 2026-07-07). Census: 3,241 rows.
pattern = "Your IP address is blocked"
label = "IpBlockedMessage"
disposition = "terminal"

[[rule]]
# Probe-validated 5/5 dead (2026-07-06); census: 68 rows.
pattern = "status code 10231"
label = "VideoNotAvailable10231"
disposition = "terminal"

[[rule]]
# Census 2026-07-07: 606/606 probe-dead, single exact message — the entire
# former YtDlpOther population. Match the SPECIFIC code: unknown future
# codes must fall through to the retryable fallback and earn terminal
# status the way this one did.
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
```

- [ ] **Step 6: Run the module tests to verify they pass**

Run: `cargo test --lib classification -- --test-threads=1`
Expected: all 6 tests PASS.

- [ ] **Step 7: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: clean, all green. (The new module is dead code until T03 — the `#[allow(dead_code)]` annotations added above must keep clippy quiet; if rustc flags an item the annotations miss, add the allow at that item with the same 0002 comment.)

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/classification.rs src/lib.rs src/main.rs tests/fixtures/yt_dlp_stderr/video_not_available_10240.txt tests/fixtures/yt_dlp_stderr/README.md
git commit -m "feat(classification): operator-editable TOML policy table with evidence-derived compiled default

0002 dead-code note: all public items in src/classification.rs carry
allow(dead_code) with lift points at Epic 4a T03 (classifier rewire) and
T06 (CLI wiring); nothing reaches them from main() yet."
```
