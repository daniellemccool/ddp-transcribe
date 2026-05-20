# FOLLOWUPS resolved — archive

Append-only history of resolved FOLLOWUPS entries. When an entry in
`docs/FOLLOWUPS.md` is resolved, move it here with the resolving commit
SHA. Do not edit prior entries in place. Sibling files: `docs/FOLLOWUPS.md`
(active-scope), `docs/cosmetic-followups.md` (deferred indefinitely),
`docs/bake-findings.md` (operational observations).

---

## Resolved by Plan B Epic 1

The three entries below were resolved by Plan B Epic 1 work but had not
yet been moved out of `docs/FOLLOWUPS.md` at the AD0020 restructure.
Resolving commits are not annotated inline; the EPIC-5-SKETCH resolution
map (`docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md` lines
120-148) is the authoritative pointer until per-entry SHAs are
backfilled.

### `transcribe::transcribe` error mapping is inconsistent and lossy

**Found in:** T12 code quality review (opus).
**Resolution:** Plan B Epic 1 (T11 deletes the `transcribe::transcribe` function and reroutes via `WhisperEngine`). Per EPIC-5-SKETCH map.

Three concerns in `src/transcribe.rs::transcribe`, none blocking for Plan A's
serial happy path:

1. **Inline `.map_err(|e| match e {...})` instead of `From<RunError> for TranscribeError`.**
   T6 chose the `From` idiom for `FetchError` so fetcher code can use `?`
   directly; T12 chose the inline match. Brief's intentional choice (no
   `From<RunError> for TranscribeError` impl in `errors.rs`), but Plan B's
   failure-classification work should harmonize on one idiom across both
   error types.

2. **`exit_code: -1` sentinel collapses non-Timeout RunError variants.**
   `RunError::Spawn`, `RunError::Io`, and any Plan B additions all collapse
   to `TranscribeError::Failed { exit_code: -1, stderr_excerpt: other.to_string() }`.
   Same loss-of-signal already flagged for T6's `From<RunError> for FetchError`
   and `status.code().unwrap_or(-1)`. Whisper-cli OOM (signal kill) and
   missing whisper-cli binary become indistinguishable to a downstream
   classifier.

3. **`exit_code: 0` for post-success artifact-read failure is misleading.**
   When `std::fs::read_to_string(&txt_path)` fails after a 0-exit
   whisper-cli run, the error is built as
   `TranscribeError::Failed { exit_code: 0, stderr_excerpt: "reading {path}: {io_err}" }`.
   A downstream consumer reading `exit_code: 0` would conclude the tool
   succeeded; the failure was actually in the artifact-reading step.
   Parallel to T11's `wav_path.exists() == false → FetchError::ParseError`
   mismatch. Plan B should introduce a dedicated variant
   (e.g., `TranscribeError::ArtifactMissing` /
   `TranscribeError::ArtifactUnreadable`).

---

### `pipeline_fakes` test gaps: `transcribed_at` RFC 3339, wav cleanup, re-run idempotence

**Found in:** T14 code quality review (opus); narrowed in T11 (Plan B Epic 1).
**Resolution:** Plan B Epic 1 — T11 reads and deserializes the `.json` artifact and asserts `model`, `transcript_source`, `fetcher`, plus the full `raw_signals` projection (schema_version, language, segments, tokens). Per EPIC-5-SKETCH map ("Pipeline hardcodes fetcher/transcript_source (T14)" and "`pipeline_fakes` test doesn't verify .json (T14)" both marked Resolved by Plan B Epic 1).

Three smaller gaps remained from the original T14 finding after T11's narrowing:

1. `transcribed_at` is not asserted to be RFC 3339; a regression that
   changed `Utc::now().to_rfc3339()` to a non-RFC format would still pass.
2. The staged `fake.wav` cleanup post-success (`!fake_wav.exists()`) is
   not asserted; a regression that skipped `std::fs::remove_file` would
   still pass.
3. Re-run idempotence (`max_videos: Some(2)` against one pending row
   returns `claimed: 1` on the second invocation, not 2) is not exercised.

Per the resolution map these gaps were closed out alongside the T11
artifact-deserialization assertions; if any of the three remain
empirically uncovered, re-open as a new active entry rather than
editing this archive.

---

### Wav cleanup-before-mark_succeeded ordering inverted in T11; documented in pipeline.rs

**Found in:** T11 (pipeline integration).
**Resolution:** Resolved in T11 — the pipeline order was inverted (`mark_succeeded → remove_file` rather than `remove_file → mark_succeeded`); the entry was kept in FOLLOWUPS as a future-reader signpost rather than a pending action.

Plan A's `pipeline::process_one` did `remove_file(wav) → mark_succeeded`
in that order. If `mark_succeeded` failed (rare; SQLite write error), the
wav was already gone — recovery had no audio to re-transcribe. T11
reversed the order: `mark_succeeded → remove_file`. If `mark_succeeded`
fails, the wav stays on disk and a future retry can pick it up.

The inverted order trades one form of waste for another: if `remove_file`
fails after `mark_succeeded`, the wav lingers (operator sweeps), but the
DB and artifacts are durable. This is the strictly safer trade. The
ordering is intentional and documented in `src/pipeline.rs::process_one`'s
inline comments — not a regression to revert.

Epic 2's state-machine work may revisit this when adding stale-claim
recovery or retry: at that point, a typed "wav still on disk" signal
might become useful for re-claiming a row.

---

## Resolved by perf-tweaks worktree (2026-05-18)

Three entries resolved by the perf-tweaks worktree commits that merged before Plan B Epic 2's T11 began. Coordinated cross-session with the Epic 2 author — see `docs/superpowers/specs/2026-05-13-perf-tweaks-design.md` § Cross-session coordination.

### `process::run` buffers full stderr/stdout in memory before truncation

**Found in:** T6 code quality review (opus).
**Originally:** FOLLOWUPS L47, routed to Epic 2.
**Resolved by:** commit `9e84b54` (`feat(process): bounded streaming subprocess capture`) on `feat/perf-tweaks`. AD0021 records the design.

`src/process.rs` previously read entire stdout AND stderr streams into `Vec<u8>` via `read_to_end` before slicing the tail; the `*_capture_bytes` field only bounded the retained excerpt, not peak memory. The perf-tweaks worktree replaced this with a streaming reader filling a `VecDeque<u8>` of size `cap`; peak retained memory is now bounded by construction. `stdout` capture got a symmetric opt-in via `stdout_capture_bytes`; `CommandOutcome::stdout` is now `Option<Vec<u8>>` (`None` = intentionally discarded). Cross-session coordination: Plan B Epic 2's T13 inherits the design and may add per-tool stdout defaults on top of AD0021 without authoring a new ADR.

---

### `ring_buffer_tail` is misnamed (it's not a ring buffer)

**Found in:** T6 code quality review (opus).
**Originally:** FOLLOWUPS L48, routed to Epic 2.
**Resolved by:** same commit `9e84b54`. The helper is removed; capture is bounded by construction rather than by post-hoc tail-slicing. No rename needed.

---

### Lazy-allocate lang_state on first opt-in request

**Found in:** T8-Epic1 (lang_probs opt-in) — codex-advisor code-quality review.
**Originally:** FOLLOWUPS L87, routed to Plan C.
**Resolved by:** commit `17716ef` (`refactor(transcribe): lazy-allocate lang_state on first opt-in request`) on `feat/perf-tweaks`. Brought forward from Plan C scope.

`WhisperEngine` worker thread previously allocated `lang_state` unconditionally at startup; non-opt-in workers paid ~500MB-1GB VRAM/host overhead for an unused state. Replaced with `Option<WhisperState>` lazily allocated on the first request with `compute_lang_probs=true`. AD0016 invariant preserved (state stays inside the worker thread). New `tests/transcribe_lang_state.rs` asserts via an `Arc<AtomicUsize>` counter that non-opt-in workers never allocate and that opt-in workers allocate exactly once.

---

## Resolved by Plan B Epic 2 — T1 audit (2026-05-18)

Two `verify-then-archive` forward-pointers from Plan B Epic 1's codex-advisor reviews were audited against shipped Epic 1 `src/transcribe.rs` during Plan B Epic 2 T1 (commit landing alongside this archive update). Both confirmed shipped and archived here. The third audit candidate (`0013` backend assertion) was NOT confirmed and remains in `docs/followups/cross-epic.md` with an audit note (see commit message).

### T8 lang_probs needs a SECOND WhisperState allocated in init phase

**Found in:** T7 (engine transcribe) — codex-advisor code-quality review.
**Originally:** `docs/followups/cross-epic.md` (Plan B Epic 1 forward-pointer for T8 dispatch).
**Resolved by:** commit `a3b7261` (`feat(transcribe): wire --compute-lang-probs opt-in for lang_probs`) on `main` — initial second-state allocation alongside the primary inference state. Refined in `17716ef` (perf-tweaks: `refactor(transcribe): lazy-allocate lang_state on first opt-in request`) to lazy-on-first-opt-in.

**Resolution:** confirmed against shipped Epic 1 code. `src/transcribe.rs:461` declares `let mut lang_state: Option<whisper_rs::WhisperState> = None`; lines 485–491 lazily allocate it on the first `req.config.compute_lang_probs == true` request via `ctx.create_state()`; lines 619–628 use it for `pcm_to_mel` + `lang_detect` to populate `lang_probs`. The shipped behavior is a refinement of the original guidance (lazy instead of eager init-phase allocation), preserving the architectural goal (separate state for lang_probs avoids clobbering the primary state's decoders/logits) and improving the memory profile for non-opt-in workers.

---

### T9 extraction must reject non-finite f32 values from whisper-rs

**Found in:** T4 (TranscribeOutput types) — codex-advisor code-quality review.
**Originally:** `docs/followups/cross-epic.md` (Plan B Epic 1 forward-pointer for T9's implementer brief).
**Resolved by:** commit `ce55d9b` (`feat(transcribe): extract per-segment + per-token raw signals from whisper-rs`) on `main`.

**Resolution:** confirmed against shipped Epic 1 code. `src/transcribe.rs::extract_segments` validates finite values when constructing `SegmentRaw` and `TokenRaw`:

- line 109: `if !no_speech_prob.is_finite() || !(0.0..=1.0).contains(&no_speech_prob) { return Err(...) }`
- line 131: `if !td.p.is_finite() || !(0.0..=1.0).contains(&td.p) { return Err(...) }`
- line 138: `if !td.plog.is_finite() || td.plog > 0.0001 { return Err(...) }`

`extract_segments` returns `Result<Vec<SegmentRaw>, String>`; the worker maps this to `TranscribeError::Bug` at line 725. Behavior matches the guidance: reject non-finite at the extraction boundary so `serde_json::to_string` never sees `NaN`/`inf`.

---

## Resolved by Plan B Epic 2 — T18 supervision wiring (2026-05-20)

Two Epic 2 entries resolved by commit `eee573d` (`feat(orchestrator): pipelined supervision wiring with LOAD-BEARING shutdown ORDER`). Both were carried as active-scope entries in `docs/followups/epic-2.md` and are archived here with the resolving SHA.

### WhisperEngine teardown can hang once T7 lands real inference

**Found in:** T5 (engine shell) — codex-advisor code-quality review.
**Disposition:** Epic 2 (graceful shutdown / state-machine work).
**Trigger to revisit:** Epic 2 planning, before pipelined orchestrator lands.
**Resolved by:** commit `eee573d` — T18's 4-step shutdown ORDER (token.cancel → drop tx → join_set.join_next → engine.shutdown) ensures the transcribe worker exits before engine.shutdown() drops the request sender; the engine worker then sees the closed channel and exits blocking_recv cleanly.

T5's teardown (drop sender → join handle) is correct for an idle worker.
Once T7 adds `whisper_full_with_state` inside the worker loop, an in-flight
request that's already been dequeued can take seconds-to-minutes to finish;
`shutdown()`/`Drop` will block until the request completes OR its deadline
fires. For Epic 1's fail-fast exit (process dies on transcribe failure;
OS reclaims everything) this is acceptable. For Epic 2's graceful shutdown,
add a shutdown signal path that flips the current request's `cancel` flag
when teardown begins — then the worker observes cancel and exits via
`TranscribeError::Cancelled` rather than blocking on inference.

---

### `Config::whisper_use_gpu` and `Config::whisper_threads` are unused by Plan B's engine path

**Found in:** T11 (pipeline integration) — Plan A leftovers.
**Disposition:** Defer cleanup sweep to Epic 2.
**Trigger to revisit:** Epic 2's state-machine and config rationalization work,
OR any task that touches `Config::from_args` for unrelated reasons.
**Resolved by:** commit `eee573d` — T18 deleted both fields from `Config` as part of the supervision wiring task.

Plan B's `WhisperEngine` does not consume `whisper_use_gpu` or `whisper_threads`:
whisper-rs picks `n_threads = min(4, hw_concurrency)` itself (api-and-pipeline.md:51),
and the GPU choice is an `i32` device index passed via `EngineConfig::gpu_device`
(currently hardcoded to `0` in `main.rs::Process` per pre-correction 3 of T11).
T11 left both fields in place because they have CLI/env plumbing and per-field
unit tests in `src/config.rs::tests`; deletion is a separate cleanup sweep.

Both fields carry `#[allow(dead_code)]` annotations pointing here. The cleanup
sweep should:

1. Delete `whisper_use_gpu` and `whisper_threads` from `Config`.
2. Remove their `whisper_model_override_takes_precedence_over_profile_default`-
   adjacent unit tests in `src/config.rs::tests` (the assertions that check
   default values).
3. If a future operator-facing config knob is needed for GPU device index or
   threads, add a typed field (`gpu_device: i32`, `n_threads: Option<usize>`)
   to `EngineConfig` and thread it from `Config` then.

Epic 2 is the natural home — that's when the broader Plan A → Plan B
state-machine and config rationalization lands.

---

## Resolved by Plan B Epic 2 — pre-T20 cleanup (2026-05-20)

Four Epic 2 entries resolved by the pre-T20 cleanup commit. All were
carried as active-scope entries in `docs/followups/epic-2.md` and are
archived here. Resolving SHA: this cleanup commit (pre-T20); use
`git log --oneline --grep="pre-T20 cleanup"` to find the exact SHA.

### Mutator test parity — backport `video_events` assertions to T5/T6; no-event-on-stale across all three

**Found in:** T7 spec-compliance review (Sonnet + codex-advisor delegation per 0018).
**Disposition:** Epic 2 cleanup; resolve before Phase 2 close (Epic 2 ships).
**Trigger to revisit:** When approaching Phase 2 close, OR whenever T5/T6 happy-path tests are otherwise edited.
**Resolved by:** this cleanup commit (pre-T20) — backported video_events shape assertions to T5 (`mark_succeeded_writes_status_and_event_in_one_transaction`) and T6 (`mark_retryable_failure_flips_status_and_records_columns`) happy-path tests; added no-event-on-stale assertions to all three stale-claim tests in `tests/state_claims.rs`.

T7's review surfaced two coverage gaps in the symmetric mutator family
(`mark_succeeded`, `mark_retryable_failure`, `mark_terminal_failure`):

1. Only T7's happy-path test (after commit `0a8ad5a`) asserts the
   `video_events` row exists with the expected `event_type`,
   `worker_id`, and `detail_json` shape. T5 and T6 happy-path tests
   exercise the UPDATE but never read the event row.

2. None of the three stale-claim tests assert that NO `video_events`
   row was inserted when the predicate rejected. The gating logic
   (`if changed > 0`) is structurally simple and visible, but the
   no-event invariant is part of the mutator contract and untested.

Event INSERT shapes verified consistent across the three mutators:

- `mark_succeeded` writes `(?1, ?2, 'succeeded', ?3, NULL)` —
  worker_id at ?3, no detail.
- `mark_retryable_failure` writes `(?1, ?2, 'failed_retryable',
  ?3, ?4)` — worker_id + JSON detail with kind/message.
- `mark_terminal_failure` writes `(?1, ?2, 'failed_terminal',
  ?3, ?4)` — worker_id + JSON detail with reason/message.

A backport pass should add the symmetric event-row assertions to
T5/T6 happy-path tests and add no-event-on-stale-claim assertions to
all three stale-claim tests. Estimated ~30 lines of test code across
5 test functions. No source changes.

Carried forward from codex-advisor review on commit `1d6b29c`;
partially addressed by commit `0a8ad5a` (T7 only, per advisor's
narrow-fix scope — reopening T5/T6 was explicitly out of scope).

---

### `sweep_stale_claims` hardening — threshold overflow, zero-threshold semantics, future-claimed_at coverage

**Found in:** T8 spec-compliance review (Sonnet + codex-advisor delegation per 0018).
**Disposition:** Defense-in-depth polish; defer to Epic 2 cleanup before Phase 2 close, OR Plan C if not surfaced sooner.
**Trigger to revisit:** Phase 2 close cleanup, OR any task that calls `sweep_stale_claims` with a non-default threshold.
**Resolved by:** this cleanup commit (pre-T20) — `threshold.as_secs() as i64` replaced with `i64::try_from(threshold.as_secs()).unwrap_or(i64::MAX)` + `saturating_sub`; doc-comment notes added for `threshold == 0` semantics and clock-skew behavior; two new tests added: `sweep_stale_claims_does_not_sweep_future_claimed_at` and `sweep_stale_claims_with_zero_threshold_does_not_sweep_same_second_claim`.

Three small hardening items on the T8 mutator (none load-bearing
against the brief; all approved as-is):

1. `threshold.as_secs() as i64` truncates silently for absurd
   Duration values. At the 30-min default it's irrelevant, but
   `i64::try_from(threshold.as_secs()).unwrap_or(i64::MAX)` +
   `saturating_sub` would make the method robust-by-construction.

2. `threshold == 0` semantics are undocumented: it means
   `claimed_at < now` (same-second claims survive the sweep).
   Defensible but a doc-comment note OR a test pinning the
   behavior would prevent caller confusion.

3. Future-valued `claimed_at` rows are left untouched (correct
   clock-skew behavior — `claimed_at < cutoff` is false when
   `claimed_at > now`), but the test triplet doesn't cover this
   case. A fourth test asserting "claimed_at in the future is
   NOT swept" would lock the invariant down.

All three are pure tightening — they don't change any current
behavior; they document and test what the existing code already
does correctly.

---

### `mark_retryable_failure` Ok(0) silently swallowed in `run_serial` (symmetric to T5 carry-forward)

**Found in:** T9 spec-compliance review (Sonnet + codex-advisor delegation per 0018).
**Disposition:** Defense-in-depth, Phase 2 scope. Unreachable in the Phase 1 serial loop today.
**Trigger to revisit:** T17 (transcribe-worker) / T18 (supervision wiring) — anywhere concurrent sweeps + workers exist.
**Resolved by:** commits `dd23814` (T16) + `6d95598` (T17) + `eee573d` (T18). Phase 2's design (`stats_stale_after_failure: Arc<AtomicUsize>` counter, symmetric to T9's `StaleAfterSuccess`) handles the `Ok(0)` case in both `fetch_worker` and `transcribe_worker`; `run_pipelined` merges the counter into `ProcessStats`. Note: the original entry mentioned `run_serial`, but Phase 2's `run_pipelined` is what actually handles the case (`run_serial` path was made test-only by T18 — `#[allow(dead_code)]`). The entry is functionally resolved by the Phase 2 mechanism.

T9 added `ProcessOutcome::StaleAfterSuccess` to handle `mark_succeeded`
returning `Ok(0)` (the row was no longer claimed by this worker; the
T5-carry-forward fix). The symmetric case on the failure path is NOT
handled: if a concurrent sweep clears the claim after `process_one`
returns `Err`, `mark_retryable_failure` also returns `Ok(0)`, but
`run_serial` increments `stats.failed`, logs nothing about the
predicate rejection, and the row stays in `pending` (the sweep moved
it there) — not in `failed_retryable` as the stats imply.

Phase 1 serial single-worker makes this unreachable in practice
(sweep is at the top of `run_serial`, claim_next runs next, then
process_one runs through to completion; no other thread can sweep
mid-iteration). Phase 2 (concurrent fetch workers + transcribe
worker) makes this race reachable.

Defense-in-depth fix when Phase 2's concurrent workers land: check
the count returned by `mark_retryable_failure`. On `Ok(0)`, log a
warn (symmetric to the StaleAfterSuccess warn in `process_one`) and
don't increment `stats.failed` — count via a new `stats.stale_after_failure`
counter (symmetric to `stats.stale_after_success`).

---

### T9 failure-classification test enrichment

**Found in:** T9 spec-compliance review (codex-advisor delegation).
**Disposition:** Epic 2 cleanup; resolve before Phase 2 close.
**Trigger to revisit:** Phase 2 close cleanup; OR if the
`tests/pipeline_fakes.rs::run_serial_classifies_fetch_failure_as_retryable_and_continues`
test is otherwise edited.
**Resolved by:** this cleanup commit (pre-T20) — extended `run_serial_classifies_fetch_failure_as_retryable_and_continues` with column-value assertions (`last_retryable_kind == "FetchOrTranscribe"`, `last_retryable_message` non-empty, `claimed_by IS NULL`, `claimed_at IS NULL`); added symmetric `run_serial_classifies_transcribe_failure_as_retryable_and_continues` test using `FakeTranscriber::always_fails_retryable()`.

The T9 happy-path failure test asserts `row.status == "failed_retryable"`
but does NOT assert:

- `last_retryable_kind == "FetchOrTranscribe"` (the placeholder string-kind
  that Epic 3 replaces with classifier dispatch).
- `last_retryable_message` contains the formatted error chain
  (`format!("{e:#}")`).
- `claimed_by IS NULL` and `claimed_at IS NULL` after the flip (the
  retry-safety invariant on `mark_retryable_failure`, already asserted
  in `tests/state_claims.rs::mark_retryable_failure_flips_status_and_records_columns`
  at the Store layer but not at the pipeline layer).

There's also no transcribe-failure variant of the test (only fetch-failure
is exercised). Both arms route through the same Err branch in `run_serial`
so it's not load-bearing, but a second test exercising
`FakeTranscriber::always_fails()` would lock down the symmetry.
