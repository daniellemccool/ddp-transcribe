# Task 03: `failure.rs` table rewire — label strings replace message-class enums; behavior identical

**Files:**
- Modify: `src/failure.rs` (major rewrite of the type surface; behavior-preserving)
- Modify: `src/pipeline/mod.rs` (`classify_fetch_phase` + `cookie_opts_for` signatures; `ProcessOptions` gains the table)
- Modify: `src/pipeline/pipelined.rs` (dispatch call sites: `tag()` → label, pass table)
- Modify: `src/pipeline/serial.rs` (same)
- Modify: `src/main.rs` (construct the compiled-default table in the Process arm; thread into `ProcessOptions`)
- Modify: `tests/pipeline_fakes/fakes.rs` + the four `*_tests.rs` files (ProcessOptions constructions gain the new field)
- Test: existing tests updated in place; no new test files

**Interfaces:**
- Consumes (Task 01): `classification::{ClassificationTable, Disposition, MessageMatch}`.
- Produces (Tasks 04/06/07 rely on these EXACT shapes):
  - `pub mod labels` in `src/failure.rs` with `pub const TOOL_TIMEOUT: &str = "ToolTimeout"; pub const NETWORK_TRANSIENT: &str = "NetworkTransient"; pub const YTDLP_OTHER: &str = "YtDlpOther"; pub const TRANSCRIBE_OTHER: &str = "TranscribeOther";`
  - `pub enum ClassifiedFailure { Retryable { label: String, requires_cookie: bool, ctx: FailureContext }, Unavailable { label: String, ctx: FailureContext }, Bug { ctx: FailureContext } }`
  - `pub fn classify_fetch_error(e: &FetchError, table: &ClassificationTable) -> ClassifiedFailure`
  - `pub fn classify_transcribe_error(e: &TranscribeError) -> ClassifiedFailure` (unchanged signature — transcribe errors are structural, no table)
  - `pub(crate) fn classify_fetch_phase(e: &FetchPhaseError, table: &ClassificationTable) -> ClassifiedFailure` (pipeline/mod.rs)
  - `pub(crate) fn cookie_opts_for(claim: &Claim, table: &ClassificationTable, cookies_file: Option<&Path>) -> FetchOpts`
  - `ProcessOptions.classification: std::sync::Arc<crate::classification::ClassificationTable>` (new field)
- Deleted: `RetryableKind`, `UnavailableReason`, `MessageVerdict`, `classify_message` (the table's `classify` replaces it). This lifts every `#[allow(dead_code)]` those items carried and lifts Task 01's allows on the table types (state the lifts in the commit's 0002 note).

**Behavioral contract (binding):** after this task the pipeline behaves EXACTLY as before for every input except messages containing `"status code 10240"`, which now go terminal instead of retryable (the compiled default's new rule — evidence n=606, 100% probe-dead). Every existing integration test must pass with at most label-string/expectation edits; the 10240 change gets one new assertion.

- [ ] **Step 1: Rewrite `src/failure.rs`'s type surface**

Delete `RetryableKind`, `UnavailableReason`, `MessageVerdict`, and `classify_message` (with their `impl` blocks and doc comments). Add at the top (after the `use` lines, adding `use crate::classification::{ClassificationTable, Disposition};`):

```rust
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
```

Replace `ClassifiedFailure` with:

```rust
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
```

`FailureContext` is unchanged (keep its `#[allow(dead_code)]` field notes as they are).

- [ ] **Step 2: Rewire `classify_fetch_error`**

Same match arms, structural kinds now label constants, and the `ToolFailed` arm consults the table:

```rust
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
            ctx(None, None, &format!("timed out after {duration:?}"), "tool timeout"),
        ),
        FetchError::ToolNotFound { detail, .. } => ClassifiedFailure::Bug {
            ctx: ctx(None, None, detail, "tool binary missing: configuration broken"),
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
```

`classify_transcribe_error` keeps its arms; replace each `kind: RetryableKind::X` with `label: labels::X.to_string(), requires_cookie: false` (X ∈ TOOL_TIMEOUT for Timeout, TRANSCRIBE_OTHER elsewhere; the Bug arm unchanged).

- [ ] **Step 3: Update `src/pipeline/mod.rs`**

1. `ProcessOptions` gains (after `cookies_file`):

```rust
    /// Active classification policy (Epic 4a): compiled default or the
    /// operator's `--classification` file, validated at startup. Shared
    /// read-only with every worker.
    pub classification: std::sync::Arc<crate::classification::ClassificationTable>,
```

2. `classify_fetch_phase` becomes:

```rust
pub fn classify_fetch_phase(
    e: &FetchPhaseError,
    table: &crate::classification::ClassificationTable,
) -> crate::failure::ClassifiedFailure {
    use crate::failure::{labels, ClassifiedFailure, FailureContext};
    match e {
        FetchPhaseError::Fetch(fe) => crate::failure::classify_fetch_error(fe, table),
        FetchPhaseError::Decode(de) => ClassifiedFailure::Retryable {
            label: labels::TRANSCRIBE_OTHER.to_string(),
            requires_cookie: false,
            ctx: FailureContext {
                tool: "hound",
                exit_code: None,
                signal: None,
                stderr_excerpt: de.to_string(),
                classification_reason: "wav decode failure: refetch may repair a corrupt download",
            },
        },
    }
}
```

3. `cookie_opts_for` consults the table (policy in ONE place — the disposition, not a hardcoded label):

```rust
pub(crate) fn cookie_opts_for(
    claim: &Claim,
    table: &crate::classification::ClassificationTable,
    cookies_file: Option<&Path>,
) -> FetchOpts {
    use crate::classification::Disposition;
    let needs_cookie = claim
        .last_retryable_kind
        .as_deref()
        .and_then(|k| table.disposition_of(k))
        == Some(Disposition::RequiresCookie);
    FetchOpts {
        cookies_file: if needs_cookie {
            cookies_file.map(Path::to_path_buf)
        } else {
            None
        },
    }
}
```

4. Update the in-module test `cookies_only_for_sensitive_login_gated_retries`: build `let table = crate::classification::ClassificationTable::compiled_default().unwrap();`, pass `&table` at every call, and add one extra assertion pinning the table-driven gate: `assert_eq!(cookie_opts_for(&mk(Some("Fetch")), &table, Some(&cookie)).cookies_file, None);` (historical placeholder kind → unknown label → no cookies).

- [ ] **Step 4: Compiler-driven sweep of the dispatch call sites**

Run `cargo check --all-targets 2>&1 | head -50` and fix each site — the complete expected list:

- `src/pipeline/pipelined.rs` fetch_worker (~253): `classify_fetch_phase(&e, &opts.classification)`; arms bind `label` instead of `kind`/`reason` and pass `&label` where `.tag()` was called (`reason.tag()` → `&label`, `kind.tag()` → `&label`; the `tracing` fields become `label = label.as_str()`). The `Retryable { label, requires_cookie, ctx }` arm ignores `requires_cookie` for now with a leading underscore binding (`requires_cookie: _`) — Task 06 consumes it (leave a one-line `// Epic 4a T06 consumes requires_cookie via record_fetch_failure.` comment).
- `src/pipeline/pipelined.rs` transcribe_worker (~503): same treatment for `classify_transcribe_error` arms (no table argument).
- `src/pipeline/serial.rs` (~86-176): `classify_fetch_phase` gains `&opts.classification`; every `.tag()` becomes the bound `label`; the final default-cautious arm's `RetryableKind::TranscribeOther.tag()` becomes `crate::failure::labels::TRANSCRIBE_OTHER`; `process_one` (~204) passes the table to `cookie_opts_for(claim, &opts.classification, opts.cookies_file.as_deref())`.
- `src/main.rs` Process arm: before constructing `ProcessOptions`, add
  ```rust
  let classification = std::sync::Arc::new(
      classification::ClassificationTable::compiled_default()
          .context("loading classification policy")?,
  );
  ```
  and set `classification: std::sync::Arc::clone(&classification)` in the options. (The `--classification` file override arrives in Task 06 — compiled default only here.)
- Every `ProcessOptions { … }` literal in `src/pipeline/serial.rs` tests, `tests/pipeline_fakes/*.rs`: add `classification: std::sync::Arc::new(ddp_transcribe::classification::ClassificationTable::compiled_default().expect("default table")),` (inside `src/`, the path is `crate::classification::…`). If the same construction appears 3+ times in `tests/pipeline_fakes/`, add a `pub(crate) fn default_process_options(tmp: &TempDir, worker: &str) -> ProcessOptions` helper to `fakes.rs` ONLY if one does not already exist — otherwise edit each literal in place; do not restructure beyond the field addition.

- [ ] **Step 5: Update `src/failure.rs` tests in place**

The existing tests translate mechanically: build `let table = ClassificationTable::compiled_default().unwrap();` per test; `classify_message(msg)` assertions become `table.classify(msg)` assertions on `label`/`disposition` — BUT those exact assertions already exist in Task 01's module tests, so DELETE `message_table_drives_classification` and `unknown_message_is_default_cautious_retryable` from `failure.rs` (no duplication) and keep/translate the rest: `fetch_error_arms_route_correctly` (match on `label == labels::…` instead of enum kinds; add `&table` argument), `tool_failed_with_write_off_message_is_unavailable` (assert `label == "IpBlockedMessage"`), `transcribe_bug_stays_bug_and_decode_is_retryable` (label strings). Add ONE new case to `fetch_error_arms_route_correctly`'s spirit as a standalone test:

```rust
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
```

- [ ] **Step 6: Behavior check on the integration suites**

Run: `cargo test --features test-helpers --test pipeline_fakes -- --test-threads=1`
Expected: compile fixes from Step 4 done, all 16+ tests pass unchanged (assertion strings like `"IpBlockedMessage"` were already bare tags in the DB — labels are spelled identically).

- [ ] **Step 7: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: green. `src/triage.rs` also calls `classify_message` today — it will fail to compile. Patch it MINIMALLY to keep the tree green (it is deleted in Task 08): give `run_triage` a `let table = crate::classification::ClassificationTable::compiled_default()?;` at the top and replace its `classify_message(msg)` match with `table.classify(msg)` mapping `Disposition::Terminal → the old Unavailable branch` and both other dispositions → the old Retryable branch (probe path), using `m.label.to_string()` where kind tags were used. Disclose this shim in the commit body; `tests/triage.rs` expectations stay green because labels are spelled the same (the 10240 rows now take the write-off fast path there — if `triage_routes_all_four_verdicts` seeded a 10240-class message as probe-path, update that seed's expectation accordingly; read the failure before editing).

- [ ] **Step 8: Commit**

```bash
git add -A src/ tests/
git commit -m "refactor(failure): classification table drives message classes; label strings replace enums

Behavior identical except: 'status code 10240' is now a terminal write-off
(census 2026-07-07: 606/606 probe-dead). RetryableKind/UnavailableReason/
MessageVerdict/classify_message deleted; structural labels live in
failure::labels; cookie gate consults the table's requires-cookie
disposition instead of a hardcoded tag.

0002 dead-code note: lifts Task 01's allows on ClassificationTable/
Disposition/MessageMatch/DEFAULT_TABLE_TOML (now reached from main());
triage.rs carries a minimal table shim pending its Task 08 deletion."
```
