# FOLLOWUPS — Epic 3 active entries

Active-scope review items targeted for Plan B Epic 3. See `../FOLLOWUPS.md`
for the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

---

### `From<RunError> for FetchError` collapses Spawn and Io into NetworkError

**Found in:** T6 code quality review (opus).
**Disposition:** Deferred to Plan B (failure classification work).
**Trigger to revisit:** Plan B introduces `RetryableKind` /
`UnavailableReason` / `ClassifiedFailure`.

The current mapping in `src/process.rs`:

- `RunError::Spawn` → `FetchError::NetworkError` (binary missing or fork
  failure — environmental/configuration, terminal)
- `RunError::Io` → `FetchError::NetworkError` (pipe read failure — system,
  potentially transient)
- `RunError::Timeout` → `FetchError::ToolTimeout` (correct as-is)

Both Spawn and Io being labeled "NetworkError" will misguide Plan B's
retry/backoff logic: a missing binary should not be retried with network
backoff (the binary will still be missing). Plan B should split these into
dedicated variants (e.g., `FetchError::ToolNotFound`, `FetchError::ConfigError`,
`FetchError::SystemIo`) and classify them appropriately.

A one-line note above the `From` impl in `src/process.rs` points here.

---

### `status.code().unwrap_or(-1)` loses signal information

**Found in:** T6 code quality review (opus).
**Disposition:** Deferred to Plan B (failure classification work).
**Trigger to revisit:** Plan B's classification needs to distinguish OOM-kill
(SIGKILL by oom-killer), user cancel (SIGINT), and crash (SIGSEGV).

When a child is killed by a signal, `status.code()` returns `None`, and the
current code collapses that to the sentinel `-1`. Recovering the signal number
requires `std::os::unix::process::ExitStatusExt::signal()`.

For Plan A this is fine: in-scope timeouts go through the `Timeout` arm before
`code()` is read; out-of-scope kills are rare.

For Plan B's failure classification, distinguishing OOM-kill from
user-cancelled from segfault matters for retry decisions. Plan B should expand
`CommandOutcome` with a `signal: Option<i32>` field (Unix-only via cfg), or
introduce a richer `CompletionStatus` enum.

---

### `claim_next` / `mark_succeeded` inner statements lack `with_context`

**Found in:** T10 code quality review (opus).
**Disposition:** Cosmetic; bundle with the next real edit to these
functions.
**Trigger to revisit:** Plan B (failure classification will likely
restructure error mapping anyway), or whenever a real bug surfaces
without enough context to diagnose.

`Store::claim_next` wraps the `transaction_with_behavior` and `commit`
with `.context(...)` but its inner `tx.execute(...)` calls (UPDATE
videos and INSERT video_events) bare-`?` raw `rusqlite::Error`. Same in
`Store::mark_succeeded` for the INSERT video_events statement (the
videos UPDATE is correctly contextualized via `with_context`).

A FK violation or other constraint failure on those statements surfaces
without `worker_id` / `video_id` context. Operationally fine for Plan
A's single-row happy path; worth tightening when failure classification
lands in Plan B.

---

### `YtDlpFetcher::acquire` error mapping and yt-dlp output-filename coupling

**Found in:** T11 code quality review (opus).
**Disposition:** Deferred. Findings 1–2 fold into Plan B's failure-classification
work; finding 3 is hardening; finding 4 is Plan C scope.
**Trigger to revisit:** Plan B's `RetryableKind` / `UnavailableReason` design
(findings 1, 2); Plan B's fetch-orchestrator hardening (finding 3); Plan C's
short-link resolution (finding 4).

Four concerns in `src/fetcher/ytdlp.rs::acquire`, none blocking for Plan A's
serial happy path:

1. **`create_dir_all` failure → `FetchError::NetworkError`.** Filesystem
   ENOSPC / EACCES is not a network condition. Will misclassify into Plan B's
   network-backoff path. Extends the existing T6 follow-up on
   `From<RunError>`'s coarse mappings — same root cause (`FetchError`
   variants too coarse), additional symptom (the mismapping now happens inside
   `acquire` itself, not just at the `From` boundary).

2. **Post-success `wav_path.exists() == false` → `FetchError::ParseError`.**
   `ParseError` means "couldn't parse tool output." This case is "tool
   succeeded but artifact convention was violated" — closer to a tool-contract
   postcondition error. Same Plan B classification work catches this. (The
   `FakeFetcher` missing-fixture error reuses `ParseError` similarly; that one
   is test-only and cosmetic.)

3. **Tight coupling to yt-dlp's `{video_id}.wav` output filename.** The
   `wav_path.exists()` check assumes yt-dlp's `--audio-format wav` +
   `%(ext)s` template always produces exactly `{video_id}.wav`. If yt-dlp
   emits a sanitized variant, intermediate partial files, or a suffix for
   collisions, the check fails despite a successful exit. A robustness
   improvement: scan `video_dir` for any `.wav` after success, or glob
   `{video_id}.*.wav`. Defer to Plan B's fetch-orchestrator hardening.

4. **`source_url` is bound as the last positional arg with no `--` separator.**
   Today this is safe because `source_url` always comes from `Canonical::Valid`
   whose regex anchors `^https?://`. Plan C will introduce short-link
   resolution that produces resolved URLs from external sources; an attacker-
   controlled or malformed URL beginning with `-` could be reinterpreted as a
   yt-dlp flag. One-line defense: insert `"--".into()` immediately before
   `source_url.to_string()` in the `args` vector. Land this when Plan C wires
   resolved URLs into the fetcher pipeline.

---

### `pipeline_fakes.rs` is 1000 lines mixing concerns; over-narrated with phase commentary

**Found in:** Operator test-suite review (2026-05-20).
**Disposition:** Epic 3+ refactor work; not Phase 2 close scope. The tests are useful as-is; the file size and narration style are the problem.
**Trigger to revisit:** Epic 3 planning kickoff; or when adding the next 200 lines to the file would push it over an unfortunate threshold.

`tests/pipeline_fakes.rs` is nearly 1000 lines and mixes: fake types (FakeFetcher,
FakeTranscriber), worker-level tests (fetch_worker_drains,
transcribe_worker_processes_one_item), serial-path tests
(pipeline_processes_one_video_to_succeeded, run_serial_classifies_*), pipelined
tests (run_pipelined_drains_all_rows), stale-race tests
(fetch_worker_increments_stale_after_failure_on_swept_claim,
transcribe_worker_increments_stale_after_failure_on_swept_claim), and artifact
assertions. The mixing makes the file hard to navigate.

The file is also over-narrated with Phase 2 task references (T16, T17, T18), ADR
citations, and "this design was added in commit X" commentary inside test bodies.
That history belongs in commit messages and git blame — not in durable test specs.
A test should read as a behavioral statement, not a Phase 2 implementation diary.

Suggested refactor:

```
tests/pipeline_fakes/
├── mod.rs            # re-exports + fixtures
├── fakes.rs          # FakeFetcher, FakeTranscriber, FetchedItem constructors
├── serial_tests.rs   # run_serial path coverage
├── fetch_worker_tests.rs
├── transcribe_worker_tests.rs
└── pipelined_tests.rs  # run_pipelined orchestration
```

When splitting, strip the T-references and phase comments from test bodies. Each
test should describe the behavior under test, not the project history that motivated
it.

---

### Over-reliance on worker-level entry points in `pipeline_fakes`

**Found in:** Operator test-suite review (2026-05-20).
**Disposition:** Epic 3+ test-quality work; not Phase 2 close scope.
**Trigger to revisit:** Epic 3 planning; or whenever a new worker-level test is being added (audit whether the same behavior could be expressed at run_pipelined level).

The current test suite calls `fetch_worker(...)` and `transcribe_worker(...)` directly
in many tests (`fetch_worker_drains_pending_rows_and_exits`,
`transcribe_worker_processes_one_item_then_exits_on_channel_close`, etc.). Direct-worker
tests are useful for failure injection (gated FakeFetcher exercising the Ok(0) race;
FakeTranscriber returning specific errors), but they're not a clean statement of
*user-visible* behavior — `uu-tiktok process` invokes `run_pipelined`, not the
individual workers.

A higher-level test using `run_pipelined` with controllable fakes (e.g., a FakeFetcher
that fails N of M videos to exercise the retryable classification path; a multi-row
fixture to exercise N=3 worker contention against the shared store mutex) could replace
some worker-level tests while expressing behavior closer to what an operator observes.

Audit suggested at Epic 3 kickoff:

1. Inventory existing worker-level tests. For each, ask: does this exercise a path
   that `run_pipelined` couldn't reach with appropriate fakes?
2. Tests that exercise only happy-path or simple classification — candidates for
   replacement by `run_pipelined`-level tests.
3. Tests that exercise specific failure injection (e.g., the `gated_then_always_fails`
   race tests) — keep at worker level because the orchestrator-level test can't
   reliably reproduce the timing.
4. Aim for a small number of focused worker-level tests + a richer set of
   `run_pipelined` orchestration tests.

This complements the `pipeline_fakes.rs` refactor entry above — when the file is
split, the audit naturally happens during the split.

---

### `From<AudioDecodeError> for TranscribeError` maps to Bug for Epic 1 fail-fast

**Found in:** T5 (engine shell) — codex-advisor code-quality review.
**Disposition:** Epic 3 (failure classification taxonomy).
**Trigger to revisit:** Epic 3 task planning.

Currently `From<AudioDecodeError>` produces `TranscribeError::Bug { detail }`
because Epic 1 lacks a failure-classification taxonomy. codex's review of
T5 noted that audio-decode failures (corrupt yt-dlp output, truncated WAVs,
unsupported sample formats) are not Bug-class — they're retryable/terminal
failures depending on cause. When Epic 3's classification ADR lands, add
`TranscribeError::AudioDecode { source }` (or whichever name fits the
taxonomy) and amend the `From` impl. The Epic 2 state-machine work should
be aware that `Bug`-from-AudioDecode is a temporary classification.
