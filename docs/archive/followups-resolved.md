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

---

## Resolved by Plan B Epic 2 — T20 bake closeout (2026-05-20)

Five Epic 2 entries resolved by the T20 bake closeout (state machine + pipelined orchestrator shipped, bake validated, Epic 2 fully closed). All were carried as active-scope entries in `docs/followups/epic-2.md`.

### `Store::open` records `schema_version` but never reads-and-checks it

**Found in:** T7 code quality review (opus).
**Disposition:** Deferred to Plan B (first schema change).
**Trigger to revisit:** any task that changes `state::schema::SCHEMA_SQL`.
**Resolved by:** T2 commit `0151e2e` (`Store::open` reads schema_version; typed `SchemaVersionMismatch` error — ADR 0022) + T3 commits `a9ec705` + `c13f64a` (`migrate` CLI subcommand for v1→v2 schema upgrade; UPSERT fix folded in).

`Store::open` writes the schema version to `meta` on first run via
`INSERT OR IGNORE`, but no subsequent open verifies the stored version against
the current `SCHEMA_VERSION` constant. A Plan B `Store::open` running against
a Plan A database would silently keep the old schema (CREATE IF NOT EXISTS
doesn't migrate).

The decision the project will eventually need to make is multi-alternative —
worth recording as a proper ADR before Plan B's first schema change:

- (a) Hard-fail `Store::open` on version mismatch
- (b) Auto-migrate forward via numbered migration scripts
- (c) Refuse to open older versions but allow newer (read-only)
- (d) Log warning on mismatch, proceed anyway (current behavior — silent)

Lowest-cost stopgap before Plan B: a one-line `tracing::warn!` in `Store::open`
when stored version differs from `SCHEMA_VERSION`. Converts silent drift into
a loud signal at near-zero cost.

---

### `concurrent_claim_serializes_via_begin_immediate` doesn't actually race

**Found in:** T10 code quality review (opus).
**Disposition:** Test-quality gap; defer until Plan B introduces real
concurrency (multi-instance / async pipeline).
**Trigger to revisit:** Plan B's first multi-worker design, or any change
to the `claim_next` transaction shape.
**Resolved by:** T10 commit `518fc8a` (rewrite `concurrent_claim_serializes_via_begin_immediate` with `Barrier(2)` to actually race two threads).

`tests/state_claims.rs::concurrent_claim_serializes_via_begin_immediate`
creates two `Store` handles to one DB file but invokes `claim_next` on
them sequentially on the main thread. The first call commits before the
second begins, so the second naturally finds no pending row. The
`BEGIN IMMEDIATE` write-lock path, `busy_timeout = 5000`, and the WAL
writer-exclusion contract are never exercised — a regression that
downgraded the transaction to `BEGIN DEFERRED` or removed it entirely
would still pass this test.

**Suggested fix:** rewrite using `std::thread::spawn` + `std::sync::Barrier`
so both threads enter `claim_next` simultaneously, then assert exactly
one returns `Some` and the other returns `Ok(None)` (or, with one row,
that the loser observes the row already `in_progress`). For two-worker
contention with multiple pending rows, assert each worker claims a
distinct `video_id`. Out-of-scope for Plan A's serial loop; Plan B's
multi-worker design will need this anyway.

---

### `mark_succeeded` doesn't require `status = 'in_progress'`

**Found in:** T10 code quality review (opus).
**Disposition:** Defensive-programming gap; defer to Plan B (state
machine + recovery).
**Trigger to revisit:** Plan B's stale-claim recovery / retry design, or
any task that grows additional state-transition mutators.
**Resolved by:** T5 commit `a8696e6` (`mark_succeeded` gains `WHERE status='in_progress' AND claimed_by=?` predicate; added round-trip test for non-re-claimable succeeded rows).

`Store::mark_succeeded` does an unconditional UPDATE — no `WHERE
status = 'in_progress'` predicate. A caller that invokes it on a
`pending`, already-`succeeded`, or `failed_*` row silently transitions
the row to `succeeded`. For Plan A's strictly-serial loop (claim → fetch
→ transcribe → succeed within one synchronous call) this cannot happen,
so it's accepted for now.

For Plan B this becomes a real concern: stale-claim recovery, retry
flows, and any out-of-order mutator could land here. Either:
- Add a `WHERE status = 'in_progress' AND claimed_by = ?` predicate and
  return an error (or `bool`) when 0 rows update; or
- Introduce a typed state-machine layer above `Store` that gates
  transitions before SQL emission.

The same observation applies to the future `mark_failed_terminal` /
`mark_failed_retryable` mutators that Plan B will add — bake the gate
into the convention before they're written.

---

### Plan B reassessment: `claim_next` polling semantics

**Found in:** T10 code quality review (opus).
**Disposition:** Defer to Plan B's process-loop / multi-instance design.
**Trigger to revisit:** Plan B planning session.
**Resolved by:** ADR 0026 commit `62a2eb6` — Plan B's design decision is "no polling": `run_pipelined` drains the queue on `claim_next` returning `None`, so the empty-transaction churn and the blocking-semantics question are both moot. The drain-on-none semantics are documented in ADR 0026.

Two related concerns about how `Store::claim_next` will behave under
Plan B's concurrent / multi-instance workloads, neither relevant to
Plan A's serial single-process loop:

1. **Empty-DB path commits an empty IMMEDIATE transaction.** When no
   pending row exists, `claim_next` calls `tx.commit()?` before
   returning `Ok(None)`. Functionally correct — committing an empty
   transaction releases the RESERVED lock the same as rollback would —
   but a hot polling loop that finds nothing on every tick churns the
   write lock. `drop(tx)` would be marginally cheaper and clearer
   about "we did nothing." Plan B should decide whether the polling
   loop short-polls (then the change matters) or sleeps between polls
   (then it doesn't).

2. **`BEGIN IMMEDIATE` + `busy_timeout = 5000` blocking semantics.**
   A worker that finds another worker mid-claim will block up to 5
   seconds inside `transaction_with_behavior` waiting for the lock.
   For Plan A (one worker) this never fires. For Plan B's
   multi-worker design, the choice between "block up to N seconds"
   and "fail fast and back off" is a design decision that should be
   explicit, not inherited from the per-connection PRAGMA.

Both concerns out of scope for T10 — flag for the Plan A → Plan B
reassessment point.

---

### Missing round-trip test: succeeded videos must not be re-claimable

**Found in:** T10 code quality review (opus).
**Disposition:** Coverage gap; defer until next edit to state_claims.rs
or T14 (process serial loop) lands a higher-level e2e fake-fetcher test.
**Trigger to revisit:** T14 implementation, or any change to
`claim_next`'s status filter.
**Resolved by:** T5 commit `a8696e6` — the round-trip test was added alongside the `WHERE status='in_progress'` predicate in the same commit.

`tests/state_claims.rs` exercises each transition independently
(`claim_next` of a pending row, `mark_succeeded` of an in_progress row)
but never composes `claim_next` → `mark_succeeded` → `claim_next` and
asserts the second claim returns `Ok(None)`. A regression that, say,
changed the SELECT predicate to `WHERE status IN ('pending',
'succeeded')` would not be caught by the current suite. T14's
end-to-end fake-fetcher tests will likely cover this incidentally;
if they don't, add a one-liner here.

---

## Resolved by Plan B Epic 2 — --max-videos cap honored by run_pipelined (2026-05-21)

One Epic 2 entry resolved by this fix commit. Carried as an active-scope
entry in `docs/followups/epic-2.md`.

### `--max-videos` ignored by `run_pipelined` (silent regression from `run_serial`)

**Found in:** T18 supervision wiring (codex-advisor + opus review).
**Disposition:** Epic 2 cleanup; resolve before Phase 2 close.
**Trigger to revisit:** Phase 2 close cleanup; OR any task that
touches `ProcessOptions::max_videos` or the orchestrator's
fetch_worker loop.
**Resolved by:** this commit — `fetch_worker` gains a shared
`Arc<AtomicUsize> claims_counter` parameter; the cap check
(`claims_counter.load(Ordering::Relaxed) >= max`), the
`claim_next` call, and the `claims_counter.fetch_add(1)` increment
all occur inside the same `Mutex<Store>` guard scope, making the
entire sequence race-free across N concurrent fetch workers (zero
overshoot). The `#[allow(dead_code)]` annotation on
`ProcessOptions::max_videos` is lifted; the T18 startup
`tracing::warn!` about the gap is removed; a new integration test
`run_pipelined_honors_max_videos_cap` (10 pending rows,
`max_videos=Some(3)`, `download_workers=3`) asserts exactly 3 rows
reach `succeeded` and 7 remain `pending`.

T18 swapped `main::Process` from `run_serial` (which honored
`opts.max_videos` by checking `stats.claimed < max` in the outer
loop) to `run_pipelined` (which did not). The CLI flag still
parsed, but `run_pipelined` never read the field — every pending
row drained regardless of the operator's cap. T18 added a startup
`tracing::warn!` so the regression was visible in logs rather than
silent.

---

## Resolved by Plan B Epic 3 — failure classification, triage, cookie-scoped retry (2026-07-07)

Nine entries resolved by Epic 3 task commits, archived with per-entry
resolving SHAs. One additional entry (`YtDlpFetcher::acquire`, four
findings) was SPLIT: findings 1–2 resolved here; finding 3 re-filed under
Epic 5 (`docs/followups/epic-5.md`), finding 4 re-filed under Plan C
(`docs/followups/plan-c.md`).

### `From<RunError> for FetchError` collapses Spawn and Io into NetworkError

**Found in:** T6 code quality review (opus).
**Resolution:** `9974d69` (`feat(errors): split RunError mapping, capture kill
signal, type audio-decode and acquire failures`). `RunError::Spawn` now maps
to `FetchError::ToolNotFound` and `RunError::Io` to `FetchError::SystemIo`;
`RunError::Timeout → ToolTimeout` unchanged. ADR 0033's classifier routes
`ToolNotFound` to Bug (a missing binary is never retried with network
backoff) and `SystemIo` to Retryable.

Original concern: both Spawn (binary missing — environmental, terminal) and
Io (pipe read failure — potentially transient) were labeled `NetworkError`,
which would have misguided retry/backoff logic.

### `status.code().unwrap_or(-1)` loses signal information

**Found in:** T6 code quality review (opus).
**Resolution:** `9974d69`. `CommandOutcome` gained a `signal: Option<i32>`
field populated via `std::os::unix::process::ExitStatusExt::signal()`
(cfg-gated to Unix); `FetchError::ToolFailed` carries it through to the
classifier's `FailureContext.signal`. OOM-kill (SIGKILL), operator interrupt
(SIGINT), and crash (SIGSEGV) are now distinguishable in stored failure
context.

### `claim_next` / `mark_succeeded` inner statements lack `with_context`

**Found in:** T10 code quality review (opus).
**Resolution:** `cc7782f` (`feat(state): Claim carries last_retryable_kind;
with_context on claim/succeed inner statements`). The inner
`tx.execute(...)` statements in `claim_next` (videos UPDATE + video_events
INSERT) and `mark_succeeded` (video_events INSERT) now carry
`with_context` naming the video_id/worker_id, so constraint failures
surface with enough context to diagnose.

### `YtDlpFetcher::acquire` error mapping and yt-dlp output-filename coupling — findings 1–2 (SPLIT)

**Found in:** T11 code quality review (opus). Original entry had four
findings; archived here are findings 1–2. Finding 3 (output-filename
coupling) re-filed under Epic 5 fetch hardening
(`docs/followups/epic-5.md`); finding 4 (`--` argv separator before
`source_url`) re-filed under Plan C (`docs/followups/plan-c.md`) — both
were NOT resolved by Epic 3.
**Resolution (findings 1–2):** `9974d69`.

1. `create_dir_all` failure → was `FetchError::NetworkError`; now
   `FetchError::WorkDirCreate { path, detail }` (ADR 0033 classifier routes
   it to Bug — environment failure, not network).
2. Post-success `wav_path.exists() == false` → was `FetchError::ParseError`;
   now `FetchError::MissingOutput { path }` (tool-contract postcondition
   violation, distinct from output parsing).

### `pipeline_fakes.rs` is 1000 lines mixing concerns; over-narrated with phase commentary

**Found in:** Operator test-suite review (2026-05-20).
**Resolution:** `0dd9707` (`test: split pipeline_fakes into per-concern
modules; strip phase narration; audit worker-level tests`). The 1047-line
`tests/pipeline_fakes.rs` became the `tests/pipeline_fakes/` directory
(`main.rs`, `fakes.rs`, `serial_tests.rs`, `fetch_worker_tests.rs`,
`transcribe_worker_tests.rs`, `pipelined_tests.rs`) — mechanical
relocation, 11 tests before == 11 after. Phase/task narration stripped from
comments (assert-message string literals retained per the commit's
disclosed ADR-0003 deviation).

### Over-reliance on worker-level entry points in `pipeline_fakes`

**Found in:** Operator test-suite review (2026-05-20).
**Resolution:** `0dd9707` (audit part). Audit verdicts are inline in the
split test files: 2 of 5 worker-level tests marked REQUIRED
(timing-dependent gate/sweep races unreachable from `run_pipelined`); the
rest marked as replacement candidates. Replacement by
`run_pipelined`-level tests remains opportunistic — pursue when a task
touches those tests anyway (not scheduled as standalone work).

### `From<AudioDecodeError> for TranscribeError` maps to Bug for Epic 1 fail-fast

**Found in:** T5 (engine shell) — codex-advisor code-quality review.
**Resolution:** `9974d69`. `TranscribeError` gained an
`AudioDecode { detail }` variant; the `From` impl now produces it instead
of `Bug`. ADR 0033's `classify_transcribe_error` routes `AudioDecode` to
Retryable (corrupt/truncated fetch output warrants a refetch attempt), never
Bug.

### `fetch_worker` cancellation latency bounded by largest await, not by `token.cancel()`

**Found in:** T16 codex review (Sonnet + codex-advisor delegation per 0018),
surfaced again in T18 Opus deep review.
**Resolution:** `50a1db0` (`feat(pipeline): three-arm classifier dispatch;
first mark_terminal_failure caller; T16 fetch cancellation wrap`) — fix
option (a) from the original entry. `fetch_and_decode` is wrapped in a
`biased` `tokio::select!` against the `CancellationToken`
(`src/pipeline/pipelined.rs`), mirroring the transcribe-side wrap
(`a66d38b`). Mid-fetch cancellation drops the in-flight future;
`kill_on_drop` reaps the yt-dlp child immediately. Worst-case shutdown
latency drops from ~300s (yt-dlp timeout) to milliseconds; the abandoned
row stays `in_progress` for the next sweep per 0024.

### Plan-brief library-API drift (T13/T19/T16 caught at implementation time)

**Found in:** T13/T19/T16 (Phase 2) — three consecutive tasks where a plan
brief's library-API claim didn't match the installed crate.
**Resolution:** `593bd5c` (`docs(plan): Plan B Epic 3 per-task implementation
plan`). The checklist was demonstrably applied at Epic 3 plan-writing time:
the plan overview's "Spec refinements (verified at plan-writing time)"
section records per-claim verification against the code on `main` (e.g. the
`fetch_and_decode` anyhow-boundary catch that became Task 07's
`FetchPhaseError`, and the pre-verified `CommandSpec.redact_arg_indices`
reuse in Task 08). Epic 3 execution surfaced no library-API drift
deviations, corroborating the checklist's effect.

### Real yt-dlp failure corpus from the PI 65k donation (Tier 5 deploy)

**Found in:** Tier 5 catalog-item validation — the PI's 65,024-video TikTok
donation run on the 2×A10 SRC workspace.
**Resolution:** `8000167` (`feat(failure): evidence-derived taxonomy +
classifiers with corpus-seeded table tests`). The harvested corpus seeded
`tests/fixtures/yt_dlp_stderr/` and the table tests behind ADR 0033's
classifier; the two hand-labeled fixtures (`7636789808341323039` → 10231,
`7038657312860491014` → "IP blocked") became the write-off classes
`VideoNotAvailable10231` and `IpBlockedMessage`. The load-bearing
classifier requirement held up: probe evidence (2026-07-06/07, n=36,
perfect separation) confirmed "IP blocked" marks dead videos.

Two stale claims in the original entry, corrected at archive time:

- "yt-dlp's stderr is dropped" — stale even when re-filed: stderr IS
  captured (trailing excerpt) since ADR 0021 landed; the 65k run's
  `last_retryable_message` column contains real yt-dlp stderr, which is
  what made message-class triage possible.
- The share-link canonicalization hypothesis is REFUTED (2026-07-07):
  56,600 of 56,620 URLs in the 65k run are share-form and succeeded at
  87.5% overall; 10/10 share-form re-fetches of probe-alive failures
  succeeded from the same egress. Share-form URLs do not inflate the
  failure rate; no canonicalization fix is needed.

---

## Resolved by Plan B Epic 4a — in-pipeline retry, config-driven classification, triage retirement (2026-07-08)

Triage code (`src/triage.rs`, `src/probe.rs`, `tests/triage.rs`) and the
`curl` runtime dependency were removed in the code-retirement commit
(`551580a`); the ADR slate + architecture-doc updates landed in the Epic 4a
close-out docs commit (same epic). The entries below were resolved by that
work.

### `requeued` event's `detail_json` lacks attempt-count context

**Found in:** Epic 3 final whole-branch review.
**Resolution:** Superseded by Epic 4a. The Epic 3 `requeued` event whose
`detail_json` carried only `{ "new_kind": ... }` is gone from the pipeline
path; the in-pipeline retry decision writes a `retry_requeued` event whose
detail carries only `{"kind","message"}` (`src/state/mod.rs::record_fetch_failure`)
— `max_attempts` was deliberately left out of the event shape for
event-shape uniformity across `retry_requeued`/`cookie_parked` (adjudicated
Task 04 decision), not an oversight. The start-of-batch sweep's
`sweep_requeue` still writes a `requeued` event. Attempt context is durably
reconstructable without a richer event: the `batch_runs` row's census
(requeued/exhausted counts per batch) plus the same row's params JSON
`retries` snapshot together tell an operator how many attempts a requeue
used up, so the original "can't tell how many attempts a requeue used up"
gap is closed at the `batch_runs` layer rather than the event layer. (A
richer per-event detail surface remains a 4b `status` concern, tracked
there.)

### Architecture-doc `uu-tiktok` naming sweep

**Found in:** Epic 3 final whole-branch review (close-out doc pass missed these).
**Resolution:** Epic 4a close-out docs commit. The four architecture-deepdive
H1 titles (`state-machine.md`, `orchestration.md`, `data-input.md`,
`transcription.md`) plus `index.md`'s H1/intro and `index.md:44`'s ingest
wording and `state-machine.md`'s `migrate` wording now read `ddp-transcribe`.
Historical docs/ADRs keep the old name by policy.

### `triage` runs mute — no progress output (papercut 1 of the triage-UX entry)

**Found in:** First production `triage --dry-run` (2026-07-07, 7,087-row DB).
**Resolution:** Moot — the `triage` subcommand retired in Epic 4a (`551580a`).
Retry is now pipeline behavior; the start-of-batch sweep and the drain emit
structured progress/`tracing` output and a durable census. (The sibling
config-echo papercut is NOT resolved — it re-targets to Epic 4b; see
`docs/followups/epic-4.md`.)

### Census prints bare taxonomy tags — annotate write-off classes (papercut 3 of the triage-UX entry)

**Found in:** First production `triage --dry-run` (2026-07-07, 7,087-row DB).
**Resolution:** Redesigned in Epic 4a. The triage census is gone; the batch
census (`src/batch.rs::BatchCensus`) breaks attrition down by label and, per
ADR 0037, the active classification policy — including each label's
disposition and its evidence comment — is snapshotted into
`batch_runs.policy_toml` alongside the census. The label meanings are now
documented in the policy file itself (the compiled default's inline evidence
comments, e.g. `IpBlockedMessage` → "video removed, NOT an IP issue"), which
is a durable and reproducible home for them.

### `CurlProber` doesn't pass `--location`; redirect responses are unhandled

**Found in:** Epic 3 final whole-branch review (was Plan-C-scoped, bundled
with the ADR-0034 oEmbed-drift re-validation trigger).
**Resolution:** Moot — `src/probe.rs` (`CurlProber` and its
`verdict_from_http_code` mapping) was deleted in Epic 4a's triage retirement
(`551580a`), and ADR 0034's oEmbed-oracle model is superseded by ADR 0036
(the re-fetch is the liveness oracle; no oEmbed HTTP-code table exists to
re-validate). No surviving code path issues the probe request.

### `run_serial` discards mutator row-change counts (half of the mutator-return-value bundle)

**Found in:** Epic 3 final whole-branch review.
**Resolution:** Epic 4a T06 (`c7c4f1b`): `run_serial`'s terminal arm gates
its census increment on `mark_terminal_failure`'s `changed > 0`, and the
retryable arms dispatch through `record_fetch_failure_serial`, whose typed
`StaleClaim` outcome is counted as `stale_after_failure` (+ warn) — an
`Ok(0)` predicate miss is no longer uncounted or unlogged on the serial
path. The bundle's surviving half (the fetch/transcribe downcast asymmetry)
remains active in `docs/followups/epic-5.md`.

## Resolved by Plan B Epic 4b — status surface, done-contract, window/timezone, CLI hardening (2026-07-28)

All five Epic 4b active-scope entries resolved by epic-4b task commits;
`docs/followups/epic-4.md` is now a closed-epic pointer stub (precedent:
`docs/followups/epic-3.md`).

### `parse_watched_at` assumes DDP `Date` strings are UTC; TikTok docs are silent

**Found in:** T13 code quality review (opus), Plan B Epic 3.
**Resolution:** `1a8bc49` (`docs(adr): DDP timestamp timezone verdict —
UTC-assumed, empirically unresolved (ADR-0039); fix parse_watched_at format
provenance comments`). Verdict recorded, not just closed: **"UTC-assumed
(documentary evidence), empirically unresolved"** — TikTok's May-2026 export
pipeline stamps its own output with a literal `" UTC"` suffix (documentary
anchor); an operator empirical spot-check against two known watch sessions
could not discriminate UTC from local time at ±1h memory precision. Recorded
in [ADR-0039](../decisions/0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md).
The hedge against the unresolved status — `watch_history.watched_at_raw`
(schema v4) preserving the verbatim string — landed in `bdc4723`
([ADR-0040](../decisions/0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md)
requires it never be dropped). Window filters built on top use day-granularity
bounds, which absorb the residual sub-day ambiguity for all but
boundary-adjacent rows — only rows within the ambiguity offset (~1h) of a
window edge can be misclassified, and the count of such rows is bounded by
the offset.

### Interrupted `process` leaves an open `batch_runs` row (NULL `finished_at`, no census)

**Found in:** First production 4a batch (2026-07-08).
**Resolution:** `d9d8125` (`feat(status): status subcommand core — counts,
retryable-by-kind, claim ages, honest batch-run history, --json`). An
interrupted `batch_runs` row (`finished_at IS NULL`) renders as `INTERRUPTED
(never closed; no census — outcomes remain reconstructable from the videos
table)` rather than being skipped or crashing on the NULL `census_json` —
both concrete asks from the original entry. Verified against
`ddp-run-export.sqlite` ground truth (run 1 INTERRUPTED, run 2 closed with
census).

### `--retries` / `max_attempts` accept unvalidated i64 ranges

**Found in:** Epic 4a T06 review (adjudicated deferral).
**Resolution:** `0d1b7a2` (`fix(cli): bound --retries to 0..=1_000_000 at
parse time; scope config echo to consumed config`). `process --retries` now
uses `clap::builder::RangedI64ValueParser::<i64>::new().range(0..=1_000_000)`,
closing both the negative-value budget-zeroing edge and the `i64::MAX`
overflow-at-`retries+1` edge at parse time, before either can reach
`record_fetch_failure`.

### Config echo logs `whisper_model_path` for subcommands that never load the model

**Found in:** First production `triage --dry-run` (2026-07-07).
**Resolution:** `0d1b7a2` (same commit as above). `log_resolved_config`
is now an exhaustive match scoped per subcommand — `init`/`ingest`/`migrate`/
`status`/`recompute-window` no longer log `whisper_model_path`; only
`process` (which loads the model) does.

### Operator interface is the tool itself — wrapper scripts are non-normative (standing premise)

**Found in:** Epic 3 close-out operations session (2026-07-07); ADR-0032 comment.
**Resolution:** Honored and now embodied: Epic 4b baked the operator surface
(`status`, `recompute-window`) into the tool itself, per
[ADR-0041](../decisions/0041-status-is-the-read-only-operator-surface-the-0017-done-contract-lives-behind-verify.md)
and [ADR-0040](../decisions/0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md).
The durable record of the standing premise remains the 0032 ADR comment —
this entry closes only the Epic 4b instance of honoring it.

## Resolved by the metadata-backfill branch / v0.3.1 (2026-07-29)

One long-standing Epic 5 CLI entry, resolved as a rider on the
`backfill-metadata` branch rather than waiting for the Epic 5 sweep.

### `--whisper-model` global flag rejected when placed after subcommand (missing `global = true`)

**Found in:** SRC bake (2026-05-06). `UU_TIKTOK_WHISPER_MODEL=... process`
works, and `--whisper-model X process ...` works, but
`process ... --whisper-model X` fails with
`error: unexpected argument '--whisper-model' found`. Scope-extended
2026-05-20 (T11 review) to every `GlobalArgs` field except
`compute_lang_probs`, which was the lone outlier already carrying the
attribute.
**Resolution:** `7dfa771` (`fix(cli): global = true on all 10 GlobalArgs
flags — accepted on either side of the subcommand (SRC-bake + T11
followup)`), shipped in v0.3.1 as a rider on the metadata-backfill branch.
Every `GlobalArgs` field now carries `global = true`, so each flag parses
on either side of the subcommand; a clap-definition consistency unit test
and a `tests/cli.rs` both-position acceptance test guard it.

**Final scope was 10 flags, not the seven this entry's table projected.**
The 2026-05-20 table predates Epic 4a's `classification` and Epic 4c's
`download_workers` / `channel_capacity`; the kickoff prompt's "six" was
staler still. The 10: `profile`, `state_db`, `inbox`, `transcripts`,
`log_format`, `whisper_model`, `classification`, `stale_claim_threshold`,
`download_workers`, `channel_capacity` (`compute_lang_probs` was already
global, for 11 total). `stale_claim_threshold`'s deliberate T11 omission —
taken to avoid a two-of-nine inconsistency — is moot now that the sweep is
uniform.

`docs/operations/src-vm.md` was corrected in the same branch: its two
"flags must go BEFORE the subcommand" claims now read as a pre-v0.3.1
version signal rather than a standing rule.

---

### Cargo package version must track release tags

**Found in:** v0.3.0 promotion (2026-07-29) — `ddp-transcribe -V` printed
`0.1.0` on both the rc1 and v0.3.0 binaries, making the runbook's
verify-after-update check useless; the operator had to fall back to comparing
`-h` subcommand lists.
**Disposition:** One-line release-checklist addition, not a code epic.
**Trigger to revisit:** the next release tag (v0.3.1 or later) — bump
`Cargo.toml` `version` to match the tag in the same commit the tag points at,
per the ADR-0043 promotion sequence. Consider adding the check to ADR-0043's
Guidance when it is next revised.
**Resolution:** `4746531` (`release: bump version to 0.3.1 in the tag
commit (ADR-0043 step 2; -V finally means something)`) — the commit the
annotated `v0.3.1` tag points at bumps `Cargo.toml` and `Cargo.lock`
0.1.0 → 0.3.1, so `ddp-transcribe -V` now reports the release it was built
from. The feature branch deliberately did not carry the bump (ADR-0043
step 2 puts it in the tag commit). Standing consideration preserved: add
the version-matches-tag check to ADR-0043's Guidance when that record is
next revised.

## Resolved by pre-production hardening — artifact write path (2026-07-28)

Two entries resolved together by commit `964e9c2` (`fix(output,pipeline):
pre-production hardening — unique atomic-write tmp names, artifact fsyncs
outside the store lock, honest tmp-cleanup count`), one carried as an
active-scope entry in `docs/followups/epic-2.md` and one in
`docs/followups/epic-5.md`.

### sync `write_artifacts_and_mark` inside `tokio::sync::Mutex` guard inside async fn can stall under `TOKIO_WORKER_THREADS=1`

**Found in:** T17 codex review.
**Disposition:** Phase 2 close scope or Epic 5 ops-hygiene work.
**Trigger to revisit:** If T20 bake or production logs show single-worker tokio stalling during write+mark phase.
**Resolved by:** `964e9c2`. The fsyncs now run OUTSIDE the store lock:
`write_artifacts_and_mark` split into `write_artifacts_durable` (no store
access) + `mark_after_artifacts` (the DB acknowledgement and wav cleanup);
`src/pipeline/pipelined.rs` ~600-624 locks the store only around
`mark_after_artifacts`. Residual: `mark_after_artifacts`
(`src/pipeline/mod.rs`) is still sync rusqlite called inside an async fn —
folded into the Epic 5 sync-IO sweep (see the `cleanup_tmp_files` entry in
`docs/followups/epic-5.md`).

`transcribe_worker` calls the sync `write_artifacts_and_mark` helper
inside a `store.lock().await` guard scope, inside an async fn. The
helper does `atomic_write` (filesystem) + rusqlite commit — both
blocking syscalls. On the operator's dev workstation under
`TOKIO_WORKER_THREADS=1`, this can stall ALL other tokio tasks during
the I/O (typically <50ms but variable).

Correct shape would be:

- Write artifacts OUTSIDE the store mutex (`atomic_write` is independent
  — no `Store` interaction needed).
- Use `tokio::task::spawn_blocking` for genuine blocking I/O (rusqlite
  `mark_succeeded` call).
- OR: split into `transcribe_outside_lock`, then brief `store.lock().await`
  for just `mark_succeeded`.

On the A10 bake (default multi-worker tokio), this is not visible. Phase 2
ships with the current shape; if T20 bake numbers don't show degradation,
revisit at Epic 5.

---

### Pipelined transcribe worker holds the Store mutex across artifact writes, not just `mark_succeeded`

**Found in:** transcript-storage assessment (Epic 4a close-out, format-selector
worktree). Related to, but narrower than, the T17 entry above (that entry is
about stall risk under `TOKIO_WORKER_THREADS=1`; this one is about lock-scope
minimization specifically).
**Disposition:** Deferred; today's per-video artifact-write cost (~5-20ms, 4
fsyncs, ~10-25KB) is small next to the ~1-2s transcription call it follows, so
the extra mutex hold time is not currently a measured bottleneck.
**Trigger to revisit:** Epic 5 perf sweep, or if a future bake shows fetch
workers starved on `claim_next` during the write+mark phase.
**Resolved by:** `964e9c2` (same commit as the T17 entry above). The fsyncs
now run OUTSIDE the store lock — see that entry's Resolved-by note for the
mechanism (`write_artifacts_durable` / `mark_after_artifacts` split).

`src/pipeline/pipelined.rs` (`transcribe_worker`, ~lines 562-574) acquires the
shared `Store` mutex once and holds it across the whole
`write_artifacts_and_mark` call — both the artifact writes/fsyncs (which need
no `Store` access) and the `mark_succeeded` DB write (which does). Per 0008,
the ordering (artifacts durable before `mark_succeeded`) is load-bearing and
must not change; only the *lock scope* is the finding. A future perf pass
could split `write_artifacts_and_mark` into a lock-free artifact-write phase
followed by a narrowly-scoped `store.lock().await` around just
`mark_succeeded`, preserving the 0008 ordering while shrinking the window
other fetch workers are blocked from `claim_next`. Any such change is
0008-ordering-sensitive and should land as its own reviewed change, not
bundled into an unrelated task.

---

## Resolved by ingest per-file hardening (2026-07-28)

One entry resolved by commit `022da45` (`fix(ingest)!: per-file skip-and-count
+ file-level ledger`), carried as an active-scope entry in
`docs/followups/production-run.md`.

### Ingest should name skipped inbox files

**Found in:** production ingest 2026-07-28 — `files=141` of 142 inbox
entries consumed; the summary line gives no hint which file was skipped or
why. If the odd one out is a valid donor DDP that failed to parse, this is a
data-loss bug rather than a logging nit — identifying it is the first step.
**Disposition:** log skipped files by name + reason at ingest.
**Trigger to revisit:** next ingest-touching epic; sooner if donor counts
ever look short.
**Resolution:** `022da45`. `skip_unparseable` (`src/ingest.rs:189-196`)
WARN-logs `file=<path>` plus the full `{e:#}` context chain, and increments
parallel `files_skipped_*` counters (`files_skipped_unparseable`,
`files_skipped_already_ingested`); shipped in v0.3.0. `022da45`'s own doc
comment cites this exact 142-file incident as the motivating case.

**Verification pointer:** to identify the specific 2026-07-28 file, grep
that run's log for `file skipped`. If no matching WARN exists there, the
file was filtered by a path that doesn't log (a walker filter) — reopen as
a new entry with that evidence.

---

## Resolved by Epic 5a — campaign-safety slice / v0.3.2 (2026-07-30)

Three entries resolved by the campaign-safety slice
(`docs/superpowers/plans/2026-07-29-epic-5a-campaign-safety/`): two carried
as active-scope entries in `docs/followups/epic-5.md`, one in
`docs/followups/production-run.md`. The slice's fourth change (per-row
`swept_stale` events, `31c18df`) deliberately resolves nothing — it
instruments the two-writer anomaly cluster, which stays active in
`docs/followups/production-run.md` until the next occurrence is adjudicated.

### Startup `cleanup_tmp_files` sweep can delete a concurrent process's in-flight tmp

**Found in:** Epic 4c Task 05 review (restated by codex-advisor while reviewing
the unique-tmp-name change). Pre-existing behavior, not introduced by 4c.
**Disposition:** Blast radius is limited to multi-process deployments, which is
exactly the SRC two-GPU setup — but the failure is self-healing (the losing
write fails, the row stays `in_progress`, the stale sweep reclaims it and the
next attempt re-writes the artifact idempotently per ADR-0008). Not worth a
rushed fix inside 4c. The real blast radius is bigger than "one video loses its
write": a concurrent instance's startup sweep unlinking an in-flight tmp makes
`atomic_write`'s rename fail, `write_artifacts_durable` propagates that as an
error, and the transcribe worker's error cancels the orchestrator — so the
*whole batch run* aborts, not just the one video. It is recoverable (restart
picks the DB state back up, nothing is corrupted) but it is a run abort under
the two-instance SRC deployment, not a one-video loss.
**Trigger to revisit:** Epic 5, bundled with the other
`output::cleanup_tmp_files` polish entry above.
**Resolution:** `fd54fea` (`fix(output): tmp sweep only collects tmps older
than the stale-claim threshold — never a live sibling's in-flight write`),
shipped in v0.3.2. `cleanup_tmp_files` took an `older_than: Duration`
parameter and now deletes a matching tmp only when its mtime age exceeds it;
the sole caller (`src/main.rs`, Process arm) passes
`cfg.stale_claim_threshold`, so a tmp young enough to be live cannot belong to
a claim the sweep would have recovered. Unreadable mtime ⇒ skip + warn (never
destroy on uncertainty); a future mtime counts as fresh. Fresh crash orphans
survive one startup and are collected on the next — the deliberate cost of
closing the abort window.

**Residual (accepted, recorded separately):** the mtime read and the
`remove_file` are not atomic — see the TOCTOU one-liner now carried in
`docs/followups/epic-5.md`.

`cleanup_tmp_files` (`src/output/artifacts.rs`) sweeps every file whose name
contains `.tmp` under the transcripts root at startup. Epic 4c made each
in-flight tmp name unique per writing process (`{name}.tmp-{pid}-{seq}`), which
removed the *collision* — two processes can no longer write the same tmp path —
but the startup sweep still matches on the substring, so a second instance
starting up while the first is mid-write will happily unlink the first's
in-flight tmp file. The fix is to make the sweep skip tmp files belonging to a
live pid (or to any pid other than its own), not to narrow the glob — a crashed
run's leftovers must still be reclaimable.

*(The shipped fix took the age guard rather than the pid check this entry
proposed: an mtime comparison needs no liveness probe, no `/proc` dependency
and no cross-host assumption, and the stale-claim threshold is already the
system's definition of "no live writer can own this".)*

---

### `ingest --dry-run` is not dry

**Found in:** Epic 4b final whole-branch review. Pre-existing wart (the
`tracing::info!` warning predates 4b); raised stakes because Epic 4b gave
`ingest` window flags to preview.
**Disposition:** Not blocking 4b — `recompute-window --dry-run` (Task 06)
mitigates for rows already ingested — but the gap widens with each flag
`ingest` grows.
**Trigger to revisit:** Epic 5, or sooner if an operator is burned by it.
**Resolution:** `130c8a1` (`fix(ingest): --dry-run is actually dry — full
per-file transaction rolled back, real stats, ledger untouched`) plus
`9e61b99` (`fix(ingest): dry-run wraps the whole inbox scan in one rolled-back
transaction`), shipped in v0.3.2. `ingest` takes a `dry_run` flag and, when
set, runs the complete pass — every file read, parsed and upserted, ledger
rows included — inside a single `BEGIN IMMEDIATE` transaction spanning the
whole inbox, which is rolled back at the end. Stats are therefore a real run's
exactly, cross-file duplicates and raw-date backfills included, because each
file sees the earlier files' uncommitted rows. The follow-up commit widened the
transaction from per-file to whole-scan precisely to get that cross-file
fidelity; the honest cost is that a dry-run holds one write transaction for the
entire scan where a real ingest takes brief per-file locks — a full-inbox
dry-run beside a live `process` can hold that lock past `busy_timeout` (5s)
and abort the live batch's next claim, so the runbook and `README.md` both
say to run a dry-run only at a pause, not alongside a live `process`.

`cli::Command::Ingest`'s `dry_run` arm (`src/main.rs`, ~line 58) logs
`"dry-run: not yet implemented; running real ingest"` and then runs the real
ingest unconditionally — `--dry-run` has never actually been dry. Now that
`ingest` takes `--window-start`/`--window-end` (Epic 4b Task 05), an operator
who reaches for `--dry-run` to preview a window's effect before committing to
it instead mutates state for real. `recompute-window --dry-run` covers
re-deriving `in_window` for rows already in the DB, but not the first
ingest of a new export, where the mutation is `watch_history` inserts plus
`videos` upserts, not just the `in_window` recompute.

---

### Periodic in-run checkpoint for uncapped campaign runs

**Found in:** campaign ops 2026-07-29 — the batch-end auto-sync (hop 1)
only fires when a `process` invocation exits, so an uncapped campaign run
staled the volume (and the Yoda-pushed resume snapshot) for hours until a
manual `sync-to-storage.sh`. Documented as an operator ritual in the
researchcloud repo (`yoda-operations.md`, "Campaign checkpoint ritual"), but
the pipeline could emit a periodic checkpoint (or invoke a configurable hook)
every N videos/minutes and remove the human dependency.
**Disposition:** ops-robustness feature, small.
**Trigger to revisit:** the ritual getting missed in practice, or the next
ops-focused epic.
**Resolution:** `11a2500` (`feat(pipeline): --checkpoint-cmd/--checkpoint-every
— supervised periodic operator hook via the bounded runner; failures warn and
count, never abort`), shipped in v0.3.2 and recorded as
[ADR-0044](../decisions/0044-in-run-checkpointing-is-an-operator-supplied-hook-that-can-never-abort-the-run.md).
`process --checkpoint-cmd <path> [--checkpoint-every <dur>]` spawns a periodic
task into the existing JoinSet/CancellationToken protocol that runs the
operator's script through the bounded subprocess runner (timeout = the
interval, no arguments, no pipeline state). The hook chose wall-clock cadence
over "every N videos" — throughput swings with the failure mix, while the
operator's data-loss exposure is measured in minutes. Failures warn and bump
`checkpoints_failed` (never `Err`, which would cancel the run); counters ride
`ProcessStats` → census → `batch_runs`, and the config lands in
`params_json`. The researchcloud repo's ritual doc still needs a pointer note
saying the hook supersedes it — handed to the deploy-repo owner, not editable
from here.

**Known interaction:** `sync-to-storage.sh` currently exits 24 mid-run
(`file has vanished: …/.work/…`), so until that script excludes `.work/` and
treats exit 24 as success, `checkpoints_failed` will count benign cycles — see
the runbook's checkpoint section.

---

## Resolved by Plan B Epic 5b — close-out slice / v0.4.0 (2026-07-30)

Twenty-three entries closed by the Plan B close-out epic
(`docs/superpowers/plans/2026-07-30-epic-5b-plan-b-closeout/`): twenty-one
carried in `docs/followups/epic-5.md`, two in `docs/followups/cross-epic.md`.
Every row's terminal disposition was fixed in advance by the epic's
`DISPOSITION-MATRIX.md`, which also carries the five operator rulings recorded
2026-07-30. Three rows are archived as **accepted** rather than fixed (the tmp
sweep's TOCTOU window, and items 1–2 of the ingest file-ledger bundle) — the
ruling, not a code change, is what makes them terminal. The remainder of the
`T1 codex code-quality review` entry (0009, 0016, error-variant enumeration)
was **re-routed** to `docs/followups/plan-c.md` rather than resolved, and is
not archived here.

### `Store::pragma_string` visibility is `pub`, not `pub(crate)`

**Found in:** T7 code quality review (opus).
**Disposition:** Defer to bin/lib structural reassessment (per ADR 0002).
**Trigger to revisit:** Plan A reassessment point — when bin/lib pattern is decided.
**Resolution:** `e3c9733`. Lowered to `pub(crate)`; the
`pragma_journal_mode_is_wal` integration test reaches it through the
`test-helpers` feature per ADR-0005. The blocked-on-0002 clause was discharged
earlier in the epic: ADR-0045 settled the bin/lib question (thin binary, fat
library, four-name façade), so "is this public library API?" now has an answer
— it is not.

`Store::pragma_string` builds `format!("PRAGMA {}", name)` because PRAGMA names
cannot be parameterized in SQLite. Under `pub` visibility an external library
consumer could pass an attacker-controlled or malformed name; the only caller
was ever the integration test passing the literal `"journal_mode"`.

---

### `Store::read_meta` could use `OptionalExtension::optional()`

**Found in:** T7 code quality review (opus).
**Disposition:** Style improvement; defer indefinitely.
**Trigger to revisit:** any future edit to `Store::read_meta`.
**Resolution:** `e3c9733`. The `map_or_else` translation of
`QueryReturnedNoRows` became the idiomatic
`query_row(...).optional()` with the `OptionalExtension` trait. Pure refactor,
no behavior change.

---

### `output::cleanup_tmp_files` minor cleanups: missing context, overcounted removals

**Found in:** T8 code quality review (opus).
**Disposition:** Cosmetic; bundle with the next real edit to this function.
**Trigger to revisit:** any task that touches `cleanup_tmp_files`, or T15 (init-cmd) when wiring the call site.
**Resolution:** item 2 by `964e9c2` (already recorded in the entry body below);
item 1 and the 2026-07-29 sync-IO rider by `d55f5e0`. The inner
`std::fs::read_dir(&path)` and the `entry?` / `shard_entry?` lines now carry
path context, so a permission-denied inside one shard dir names the shard. The
rider — `mark_after_artifacts` running sync rusqlite from an async fn — was
adjudicated by the sync-IO audit that produced
[ADR-0047](../decisions/0047-blocking-io-on-the-worker-hot-path-runs-on-spawn-blocking-inline-only-when-nothing-can-be-starved.md):
the store call is class (b) (bounded, and the store mutex serializes it anyway),
while the genuinely unbounded neighbour — `write_artifacts_durable`'s mkdir plus
two `atomic_write` fsyncs — moved to `spawn_blocking`. `cleanup_tmp_files`
itself is class (a): startup-only, before any worker spawns.

Two small inconsistencies in `src/output/artifacts.rs::cleanup_tmp_files`:

1. The inner `std::fs::read_dir(&path)?` and the surrounding `entry?` /
   `shard_entry?` lines bubble up raw `io::Error` without path context. The
   outer `read_dir(transcripts_root)` is contextualized via `with_context`.
   On a permission-denied inside one shard dir, the operator gets a path-less
   error.

2. ~~`let _ = std::fs::remove_file(&p); removed += 1;` increments
   unconditionally.~~ **Resolved by `964e9c2`** — `remove_file` is now
   matched; `removed` increments only on `Ok(())`, and a failure logs
   `tracing::warn!` without counting it. Test:
   `cleanup_tmp_files_counts_only_real_deletions`
   (`src/output/artifacts.rs`).

**2026-07-29 triage:** carried from archived T17 ("Resolved by pre-production
hardening — artifact write path"): `mark_after_artifacts`
(`src/pipeline/mod.rs`) is sync rusqlite called from an async fn — include it
when this function's sync-IO sweep is next touched.

---

### `output::shard_distributes_uniformly` test rationale is reversed

**Found in:** T8 code quality review (opus).
**Disposition:** Cosmetic; comment is misleading but the assertion still
catches the stated regression.
**Trigger to revisit:** any future edit to the test, or whenever a
`VideoId` newtype absorbs `shard()` and the test moves with it.
**Resolution:** `d55f5e0`. The reversed variance claim is gone: the comment now
states that monotonic `base + i` input produces exactly 100 items per bucket
(so the ±50% bound passes with zero margin) and that real Snowflake low bits
would be *looser*, not tighter — Poisson-like, ~10% std dev over 10k samples.
The load-bearing `counts.len() == 100` assertion, which is what actually catches
a high-digits implementation, is unchanged.

`src/output/mod.rs::shard_distributes_uniformly` uses monotonic counter input
(`base + i` for `i in 0..10000`), which produces exactly 100 items per
last-two-digits bucket. The ±50% assertion (`50..=150`) passes with a margin of
0%, not because the bound is "lenient for synthetic input" as the comment
claimed. The comment said "real Snowflake IDs would be tighter" — that is
reversed.

---

### `videos.updated_at` is frozen at first-seen by `upsert_video`

**Found in:** T9 code quality review (opus).
**Disposition:** Accepted for T9; re-evaluate as T10/T13 land.
**Trigger to revisit:** T10 (`claim_next` / `mark_succeeded`), T13 (ingest cmd),
or any future Store mutator that touches a `videos` row.
**Resolution:** `e3c9733`, under the operator ruling recorded in the epic's
Phase-0 disposition matrix: **`updated_at` records lifecycle-mutation time, and
a no-op ingest is clock-neutral.** `INSERT OR IGNORE` stays and the column keeps
its name — re-ingesting the same DDP export must not rewrite the column for
millions of unchanged rows. The contract is now written down in
`docs/reference/architecture/state-machine.md`, which enumerates the bumpers
(`claim_next`, `mark_succeeded`, `mark_retryable_failure`,
`record_fetch_failure` both arms, `mark_terminal_failure`, `sweep_stale_claims`,
`sweep_mark_terminal`, `sweep_requeue`) and the deliberate non-bumpers
(`requeue_failures` per ADR-0046, `apply_metadata_batch`). Task 10 pinned
re-ingest neutrality with a test and verified the lifecycle mutators bump it.

`Store::upsert_video` binds the same `now` to both `first_seen_at` and
`updated_at`; on a re-upsert neither column is written. The naming reading
("when was this row last touched") therefore only holds for rows that have
moved at all — which the ruling makes explicit rather than renaming the column.

---

### `Store::conn` / `Store::conn_mut` accessor hygiene after T10

**Found in:** T9 code quality review, re-confirmed in T10 review (opus).
**Disposition:** Cleanup commit, or fold into 0002's bin/lib reassessment.
**Trigger to revisit:** Plan A reassessment point, or any task that genuinely
needs `&Connection` / `&mut Connection` outside `Store`'s own `impl`.
**Resolution:** `88c8bc2` (deletion) + `e3c9733` (comment refresh).
`Store::conn_mut` had zero references in `src/` or `tests/` and its
justification named Epic 1 tasks that never consumed it — deleted with its
`#[allow(dead_code)]` during the Phase-1 allow purge (46 → 1). `Store::conn`
survives with a comment naming its real current consumers instead of the
factually wrong T9/T10 forward-pointer; it gained a genuine non-test consumer in
Epic 5a.

---

### `ingest::walk_recursive` minor polish: silent missing-inbox + missing inner context

**Found in:** T13 code quality review (opus).
**Disposition:** Cosmetic; bundle with the next real edit to `ingest::*`.
**Trigger to revisit:** any task that touches `walk_recursive` or `ingest`
error-handling.
**Resolution:** `d55f5e0`. Item 1: the `ingest()` root now `bail!`s when the
inbox does not exist, so a typo'd `--inbox` is an error instead of a successful
`files=0` run; deeper subdirectories vanishing mid-walk stay ignored (a race,
and the intended behavior). The surviving inner-`entry?` half of item 2 now
carries path context, matching what the same commit did to
`output::cleanup_tmp_files`.

---

### `output::shard_dir` is unused; allow comment falsely names T13/T14 as consumers

**Found in:** T15 code quality review (opus) — Plan A close-out 0002 audit.
**Disposition:** Dead helper; delete or find a real caller.
**Trigger to revisit:** Plan A → Plan B reassessment, or next edit to
`src/output/mod.rs`.
**Resolution:** `d55f5e0`. The function is deleted, along with the unit test
that was its only caller; its `#[allow(dead_code)]` had already been removed by
Task 04's purge (`88c8bc2`), so the branch's allow census is unchanged at 1.
`pipeline.rs` continues to call `opts.transcripts_root.join(shard(&…))`
directly — the join was never the duplication worth a helper, and amended
ADR-0002 no longer accepts a suppression as the alternative to deleting.

---

### `YtDlpFetcher::acquire` tight coupling to yt-dlp's `{video_id}.wav` output filename

**Found in:** T11 code quality review (opus); finding 3 of the original
four-finding `YtDlpFetcher::acquire` entry. Split out at Epic 3 close:
findings 1–2 were resolved by Epic 3 (`9974d69`, archived above), finding 4
moved to `docs/followups/plan-c.md`.
**Disposition:** Epic 5 fetch hardening.
**Trigger to revisit:** Epic 5 planning; or any yt-dlp version bump that
changes output-template behavior.
**Resolution:** `19155c1`, plus the review fix `17d290f`. The shipped contract
is stronger than the entry's "scan for any `.wav`" proposal. Each `acquire`
creates a **fresh** attempt directory `{work_dir}/ytdlp-{video_id}.{pid}-{seq}`
(nothing is reused across retries, workers, or processes), so exactly one
yt-dlp invocation ever writes into it, and the output is **discovered** by
scanning that directory: one `*.wav` is success, zero is
`FetchError::MissingOutput`, more than one is the new
`FetchError::AmbiguousOutput` — the fetcher never picks one, because guessing
would stamp an arbitrary file as this video's transcript. The path is never
parsed out of stdout, which is reserved for the Epic 4c metadata capture.
Ownership is explicit and total: `acquire` removes the directory on its own
failure returns (the caller holds no handle), and the pipeline removes it
exactly once on every path where it does hold one — after `mark_succeeded`
commits, or on a decode/transcribe/artifact-write failure; `StaleAfterSuccess`
deliberately keeps its directory. `17d290f` closed the one leak the first pass
missed: a closed fetch→transcribe channel returns the un-sent `FetchedItem`
inside `SendError`, which was being discarded along with its live attempt dir.
Crash and cancellation residue is collected by the age-gated `.work` sweep at
the next `process` startup.

---

### Epic 3 close: test-hardening bundle (signal capture, classifier precedence, kind-string end-to-end)

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Grouped opportunistic hardening; bundle into one Epic 5 pass
rather than three separate commits.
**Trigger to revisit:** Epic 5 test-sweep planning.
**Resolution:** `5082474`, all three items, each shown to fail when its subject
is broken. (1) `tests/process_bounded_capture.rs` now spawns a real child,
signals it, and asserts `FetchError::ToolFailed { signal: Some(_), .. }` comes
out of `process::run` — the `ExitStatus` Unix `signal()` extension path is no
longer only exercised by hand-constructed values. (2) `src/classification.rs`
pins `classify_message`'s match-arm precedence when two markers appear in one
stderr blob, and its case sensitivity. (3) `tests/pipeline_fakes/` asserts the
actual `RetryableKind` / `UnavailableReason` **tag string** written to
`last_retryable_kind` end-to-end through `transcribe_worker`'s three-arm
dispatch, replacing the inline worker-test audit verdicts that tracked the gap
rather than closing it.

---

### `state/mod.rs` hygiene bundle (sweep mutators — formerly Epic-3 triage mutators)

**Found in:** Epic 3 final whole-branch review. Items 3–4 re-pointed at the
Epic 4a surfaces (T08: triage retired; `triage_mark_terminal`/`requeue_retryable`
became `sweep_mark_terminal`/`sweep_requeue`, `run_triage` became
`batch::run_sweep`).
**Disposition:** Grouped cleanup; low risk, no behavior change expected.
**Trigger to revisit:** next edit to `src/state/mod.rs`, or Epic 5 sweep.
**Resolution:** `e3c9733`, all four items. `claim_next`'s empty-candidate path
now commits with a `.context(...)` like every other transaction in the file; a
test pins `attempt_count == 2` across the full claim→fail→requeue→reclaim
cycle; `sweep_mark_terminal` / `sweep_requeue` gained the defensive
`claimed_by` / `claimed_at` clear (inert under today's schema, correct if a
future change ever lets a claimed row reach them); and `batch::run_sweep`'s
`kept_capped` path now has a test asserting it writes **no** `video_events` row.
Claim and status semantics (ADR-0023 / ADR-0024) are untouched.

---

### `run_serial` fetch/transcribe downcast asymmetry

**Found in:** Epic 3 final whole-branch review. The bundle's other half —
discarded mutator row-change counts — was resolved by Epic 4a T06 (`c7c4f1b`)
and archived above.
**Disposition:** Opportunistic hardening.
**Trigger to revisit:** `run_serial` retirement decision, or Epic 5 sweep.
**Resolution:** `b61bddb`, under the operator ruling recorded 2026-07-30:
**keep `run_serial`, and fix the asymmetry** — not archive it as an accepted
defect. `src/pipeline/serial.rs`'s fetch-side classification now walks the error
chain the way the transcribe side always did, with a regression test wrapping
`FetchPhaseError` in one and in two context layers.

**Premise correction (recorded because the entry's diagnosis was wrong).** The
entry's implied mechanism — that anyhow's `.context()` wrapping defeats
`downcast_ref` — is false: `anyhow::Error::downcast_ref` already pierces
context layers and finds the underlying type. The real asymmetry is narrower
and was only found by reproducing it: a `FetchPhaseError` that is reachable
*only* as another error's `#[source]` is invisible to `downcast_ref` (which
inspects the error's own type, not its source chain) while `chain()` walks
straight to it. The fix is therefore chain-walking, and it would have been
missed by anyone testing the context-layer hypothesis alone.

**`run_serial` is retained**, documented in-file as a **non-production
reference/test path** rather than a live code path — the pipelined orchestrator
is what `process` runs. Retirement stays a future, separately-scoped task: its
unique tests must first be mapped onto the pipelined and shared-helper suites,
which is why deleting it during a behavior-preserving restructure was rejected.

---

### `FetchOpts`'s derived `Debug` does not redact `cookies_file`

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Small hardening; not exploitable today (no code path logs
`FetchOpts` via `{:?}`) but a footgun for future callers.
**Trigger to revisit:** any future logging/tracing call that formats a
`FetchOpts` value, or Epic 5 sweep.
**Resolution:** `19155c1`. `FetchOpts` carries a hand-rolled `Debug` that
redacts `cookies_file` to `[COOKIES-REDACTED]` (and prints `None` unchanged),
closing the gap between the derived form and `scrub_cookie_path`'s redaction of
the same path everywhere else it can reach an error message or argv (ADR-0035).

---

### `scrub_cookie_path` has no guard against an empty cookie path

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Small hardening; edge case, not observed in practice.
**Trigger to revisit:** Epic 5 sweep, or if `--cookies-file ""` is ever observed
in the wild.
**Resolution:** `19155c1`. An empty path returns the excerpt unchanged, with a
test pinning that `scrub_cookie_path("")` is a no-op. Without the guard,
`str::replace` with an empty pattern inserted the replacement between every
character of the stderr excerpt.

---

### Epic 4b final review: status polish + test-debt bundle

**Found in:** Epic 4b final whole-branch review.
**Disposition:** Grouped opportunistic hardening + test-debt; bundle into one
Epic 5 pass rather than five separate commits.
**Trigger to revisit:** Epic 5 hygiene sweep planning.
**Resolution:** `a8bd7ac`, all five items. `render_event_detail_inline` falls
back to the raw `detail_json` when a known key holds a non-string value; the six
missing fixtures landed in `tests/status.rs` (malformed `detail_json`, the
`{"reason","message"}` shape, a corrupt-JSON artifact, valid metadata without
`raw_signals`, a single-status zero-fill, an end-only `WindowBounds`);
`run_verify`'s per-entry `e.ok()` drop now surfaces an unreadable `DirEntry` as
an infra fault instead of counting it missing; `status --respondent-id` with an
unknown id errors and exits 1 like `--video-id` does, instead of printing an
all-zeros summary; and `src/status.rs`'s mid-file `use` statements moved into
the top-of-file block.

---

### Worker-side closed-reply path silently swallows the error

**Found in:** T5 (engine shell) — codex-advisor code-quality review. Re-routed
from `docs/followups/epic-2.md` (2026-07-29 triage): Epic 2 closed before this
fix was picked up.
**Disposition:** Operational logging improvement; ~1h fix.
**Trigger to revisit:** Epic 5 hygiene bundle.
**Resolution:** `a8bd7ac`, under the operator ruling recorded 2026-07-30:
**log without a video id, centralized.** The correct site count is **seven**,
not the six the entry named — the six error replies plus the **success** reply,
which dropped a completed `TranscribeOutput` just as silently. All seven now
route through one helper that logs a worker-local monotonic request sequence
number, elapsed wallclock since processing began, the result kind, and whether
the cancellation flag was set; transcript text is never logged.

The no-id shape is the deliberate choice, not an omission: `TranscribeRequest`
(`src/transcribe.rs`) carries `samples`, `config`, `cancel`, `deadline`,
`reply` — no video or request id. The id exists one layer up (`FetchedItem`
carries a `Claim`, hence `claim.video_id`) but `Transcriber::transcribe` and the
engine's request channel drop it, so logging it at the swallow site would have
required a trait-signature change. A worker-local sequence plus elapsed
wallclock makes an unexplained dropped caller visible with no API change; the
cancellation flag distinguishes the expected `CancelOnDrop` case from the
suspicious one.

---

### `main.rs` re-declares the library's entire module tree

**Found in:** operator review, 2026-07-28 (during Epic 4c).
**Disposition:** Real structural debt, too broad for an Epic 4c rider — it
touches every module declaration in the crate. Cites ADR-0002's deferred bin/lib
reassessment, which parked exactly this question.
**Trigger to revisit:** Epic 5 hygiene bundle, alongside `run_serial`
retirement, the `state/mod.rs` split, and the sync-IO sweep.
**Resolution:** the whole of Epic 5b Phase 1 — `935f5e1` + `369471b` (the ADR
and its correction), `6bab68e` (the restructure), `88c8bc2` (the visibility and
allow purge). `src/lib.rs` is now the crate's single module root; all 18 `mod`
declarations are gone from `src/main.rs`, which carries argument parsing,
tracing init, error rendering and the program's one `std::process::exit` and
nothing else. `main`'s `match cli.command` arms moved verbatim into
`src/commands.rs` behind `pub async fn dispatch(cli: Cli) -> Result<CommandExit>`;
exit semantics travel back as a `CommandExit` value, so the library never calls
`process::exit`. The façade is exactly four names —
`pub use cli::{Cli, LogFormat};` and `pub use commands::{dispatch, CommandExit};`
— with `#![warn(unreachable_pub)]` as the backstop and `Cli::log_format()` as
the one narrow accessor. Recorded as
[ADR-0045](../decisions/0045-the-crate-is-a-fat-library-with-a-thin-binary-behind-a-minimal-public-facade.md);
ADR-0002 was amended so visibility narrowing, not suppression, is the answer to
a `dead_code` finding.

All three named consequences are discharged. **Double compilation** is gone —
every file compiles exactly once, which removed 84 duplicated inline test
executions (`--list` census 345 → 261 runnable). **Broadened `pub` surface** is
gone — 27 items narrowed to `pub(crate)`/`pub(super)`, and the "is this public
API?" question now has a written answer, which is what unblocked the
`Store::pragma_string` entry above. **The accumulated suppressions** are gone —
the `#[allow(dead_code)]` census went 46 → 1, and 31 of the 33 real attributes
turned out to be already inert (they sat on reachable `pub` items, which
`dead_code` cannot fire on) with justifications citing exactly this double
compilation.

*(One correction to the entry's reasoning, recorded because a later reader
would otherwise inherit it: reachable `pub` items in a library are outside
`dead_code` regardless of module-root structure. The restructure removed the
second compilation context, not the exemption — which is why a narrow façade
policed by `unreachable_pub`, rather than the lint, is what keeps `pub` from
becoming a place to park unused code. `369471b` fixed the same wrong claim
where it had reached ADR-0002 and ADR-0045.)*

---

### Tmp sweep's age guard has an inherent TOCTOU window

**Found in:** Epic 5a Task 01 review (2026-07-30), as an observation on the
shipped age guard (`fd54fea`).
**Disposition:** Accepted plan-level tradeoff — recorded for completeness, not
scheduled.
**Trigger to revisit:** only if a run abort is ever traced to a swept tmp whose
writer was live — i.e. evidence that the window is reachable in practice.
**Resolution:** **accepted-archive** (operator ruling, 2026-07-30). No code
change: the tradeoff stands exactly as the entry describes it, and archiving is
the act of removing a standing tripwire that has produced no evidence. The
evidence-gated trigger above is preserved verbatim and remains the condition for
re-opening this as a **new** entry — do not append a second resolution here.

The mtime read and the `remove_file` are separate syscalls, so a writer that
stalled past the stale-claim threshold and then resumed could still have its tmp
unlinked between the two. That window is only reachable by a writer whose
*claim* would already have been swept out from under it, which is the case the
sweep exists to clean up; closing it properly means an advisory lock or a
liveness probe, both heavier than the exposure.

---

### `upsert_metadata_raw` is not claim-guarded

**Found in:** Epic 4c Task 03 review (triaged to backlog at the time).
**Disposition:** Accepted tradeoff, documented in the mutator and in ADR-0042.
**Trigger to revisit:** Epic 5, or sooner if metadata ever gains a consumer for
which snapshot freshness is load-bearing.
**Resolution:** `e3c9733` + `217f646`, under the operator ruling recorded
2026-07-30: **resolve — add the guard.** `upsert_metadata_raw` now carries the
same `status = 'in_progress' AND claimed_by = ?` predicate as every other
in-flight mutator (ADR-0023), so a worker whose claim was swept out from under it
can no longer overwrite a newer envelope. The cost the entry named was paid
rather than avoided: `worker_id` is threaded into a call site whose whole point
is that it runs unconditionally on both the success and the failure path.
ADR-0042's "last-write-wins per `video_id`" bullet is narrowed to
last-write-wins **among claim holders**, which is what the guard makes true.

---

### Ingest file-ledger hardening bundle

**Found in:** PR #23 review (production ingest hardening, 2026-07-29) — three
Minors carried out of the review by agreement.
**Disposition:** None blocks the campaign; bundle for Epic 5's ingest/sync-IO
sweep.
**Trigger to revisit:** Epic 5, or immediately if the inbox ever gains
subdirectories with same-named files.
**Resolution:** items 1 and 2 **accepted-archive**, item 3 fixed by `d55f5e0`.

1. **Basename-only ledger key — accepted** (operator ruling, 2026-07-30). The
   basename is kept: `ingested_files.file_name` is host-portable by design (the
   inbox directory moves between hosts), today's inbox is flat, and filenames
   embed participant+key so they are collision-proof in practice. Keying on an
   inbox-relative path would trade that portability for a schema migration
   against a structural risk that is not live. The re-open condition is the
   entry's own: the inbox gaining subdirectories with same-named files.
2. **(size, mtime) is a one-second-resolution change detector — accepted and
   documented** (operator ruling, 2026-07-30). A same-size rewrite inside one
   mtime tick is invisible to the ledger; row-level upserts remain the
   correctness backstop, so the cost is the freshness of that file's rows, never
   corruption. Strengthening the fingerprint (content hash, nanosecond mtime)
   would put real per-file cost on every ingest scan against a freshness-only
   exposure.
3. **Mid-transaction rollback regression test — fixed** by `d55f5e0`. The test
   `a_failed_row_upsert_rolls_back_the_file_and_its_ledger_row`
   (`src/ingest.rs`) installs a `BEFORE INSERT` trigger that `RAISE(ABORT)`s on
   the file's *second* watch-history row, so the first row is already written
   (uncommitted) when the abort lands, then asserts post-rollback state rather
   than merely that an `Err` came back: zero `watch_history` rows, zero `videos`
   rows, and **no** `ingested_files` row — a ledger row here would make the next
   run skip a file it never ingested.

---

### `global = true` both-position CLI test asserts parse acceptance only

**Found in:** metadata-backfill branch review (v0.3.1 `global = true` rider,
`7dfa771`), plus three more test-hardening candidates from that branch's final
whole-branch review, bundled here.
**Disposition:** Hardening candidate; bundle with the Epic 5 test-hardening
sweep. The shipped test is not wrong, just shallow.
**Trigger to revisit:** Epic 5 test-hardening bundle, or the next change to
`GlobalArgs`.
**Resolution:** `5082474`, all four items. `src/cli.rs` gained
`global_flag_value_propagates_from_the_post_subcommand_position` and
`duplicate_global_flag_takes_the_last_occurrence` — `try_parse_from` unit tests,
no process spawn — turning the acceptance check into a behavior check and
pinning clap's last-occurrence-wins precedence this repo had never asserted. The
existing `tests/cli.rs` both-position test and `clap_definition_is_internally_consistent`
are unchanged (they are also the archive-integrity surface for the `global = true`
entry archived under v0.3.1 — the hardening added assertions, it did not weaken
any). `backfill-metadata`'s `--dry-run` gained `conflicts_with = "limit"`, so
clap itself rejects the combination and prints the conflict instead of relying on
the operator having read the runbook — the one production change in the bundle.
`tests/backfill_metadata.rs`'s dry-run test gained a sentinel-file PATH shim, so
"invokes nothing" is a positive hermetic assertion rather than an
absence-of-evidence inference, and its `statuses()` snapshot helper widened from
`(video_id, status)` to also capture `claimed_by`, `claimed_at`, `attempt_count`
and `succeeded_at`, so a regression touching lifecycle columns
`backfill-metadata` must never write (ADR-0042's carve-out) fails a snapshot
instead of passing silently.

---

### T1 codex code-quality review — deferred ADR refinements (0011, 0017, 0013 bullets)

**Found in:** T1 (ADR drafts for Plan B Epic 1) — codex-advisor code-quality
review. Three blocking findings were resolved inline via `adg comment` at the
time (0010 schema_version-as-string; 0012 cancellation-via-abort_callback; 0016
closed-oneshot shutdown carve-out); six non-blocking items were deferred.
**Disposition:** Deferred, multi-epic.
**Trigger to revisit:** per bullet.
**Resolution:** three of the six bullets are terminal and archived here. The
other three — **0009 fallback Engine API preservation**, **0016 multi-engine GPU
memory**, and **error-variant enumeration** — are all gated on multi-engine or
CUDA-fallback work that Plan B does not do, and were **re-routed to
`docs/followups/plan-c.md`** rather than resolved. They are not archived.

- **0011 pause-safe checklist references 0017 — resolved 2026-07-29** via lean
  [ADR-0041](../decisions/0041-status-is-the-read-only-operator-surface-the-0017-done-contract-lives-behind-verify.md).
  Epic 4's `status` subcommand shipped and the 0017 done-contract now lives
  behind `status --verify`. *(Struck through in the entry body at the time but
  never migrated here; migrated by the Epic 5b close-out per this file's own
  Resolve rule, with its original 2026-07-29 provenance.)*
- **0017 splits pause-safe vs batch-complete — resolved 2026-07-29**, same
  record: 0017's done-contract became lean ADR-0041, and the pre-migration
  MADR-0017 prose is frozen in `docs/madr-archive/`. *(Same migration
  provenance as the bullet above.)*
- **0013 global log callback invariant — resolved by `ebc4ee0` + `2788483`.**
  The invariant this bullet stated is now the shipped mechanism and is written
  into ADR-0013's Guidance: `whisper_log_set` (via whisper-rs's
  `set_log_callback`) is installed exactly once behind a `Once` before any
  context init, is never replaced per engine, routes every line through one
  global bridge to `tracing`, and backend capture is phase-scoped behind a
  global init mutex so concurrent engine constructions cannot interleave.

---

### 0013 backend assertion must be cfg(feature = "cuda")-gated

**Found in:** T6 (engine init) — codex-advisor code-quality review.
**Disposition:** Forward-pointer for T13's bake-runbook implementer. Audited
2026-05-18: **NOT confirmed against shipped Epic 1 code** — which is why Epic 5b
scheduled it as an implementation task rather than an audit.
**Trigger to revisit:** Epic 5 cleanup sweep.
**Resolution:** `ebc4ee0` + `2788483`. The assertion is implemented and gated
exactly as this entry required. `EXPECTED_BACKEND` is a
`const … = if cfg!(feature = "cuda")` — `ExpectedBackend::Gpu` on a CUDA build,
`Unconstrained` otherwise — so a CPU dev build reports its backend and asserts
nothing, while a CUDA build hard-fails engine construction with
`WhisperInitError::BackendMismatch { expected, detected }` before any batch work
starts. Softening to a warning was rejected, per the ADR. The `cfg!` form was
chosen over the entry's sketched `#[cfg]`-split const pair because the latter
leaves the unselected variant unconstructed and fires `dead_code` — it would
have re-added an allow to a branch that had just purged 45 of them; `cfg!` is
the same compile-time gate with both arms type-checked by every build.

`2788483` closed a false-positive gap found in review and verified against the
pinned whisper.cpp v1.8.3: `whisper_backend_init_gpu` logs `using %s backend`
**before** it calls `ggml_backend_dev_init`, and on failure logs
`failed to initialize %s backend` and returns nullptr while the caller falls
silently through to CPU. The first implementation returned `Gpu` on the `using`
line alone, so it would have certified a CUDA build that was actually running on
CPU — precisely the failure ADR-0013 exists to catch. `detect_backend` is now an
ordered parse: `using X backend` is a *pending claim*, retracted by a following
device-matched `failed to initialize X backend`
(→ `DetectedBackend::GpuInitFailed`); `no GPU found` is its own verdict; and
`Unknown` fails closed on a CUDA build.

**CUDA-gate evidence (2026-07-30):** this workstation has no CUDA toolkit, so
the gated build was run on the paused SRC workspace at branch tip `2788483` —
`cargo build --release --features cuda` compiled in 2m 11s and all 6 ignored
`tests/whisper_engine_init.rs` tests passed on the GPU, including the assertion
test. Commits after `2788483` touch no `cfg(cuda)`-gated code; there is none in
`src/` (the feature only toggles `whisper-rs-sys`'s build).

---

## Resolved by v0.5.0 — census-completion release (2026-08-12)

### Mass-instant-failure circuit breaker for the fetch path

**Found in:** the 2026-08-06 TLS-fingerprint 403 incident
(`docs/operations/incident-2026-08-06-tiktok-tls-403.md`; at archive time
that record was on a then-unmerged branch, so this entry was archived
directly rather than moved from an active-scope line — **reconciled
2026-08-12** when the incident branches merged: the incoming active entry
was removed in the same merge, per the v0.5.0 PR's merge-ordering note; the
original body text survives in the incident branch history). The pipeline spent ~60
hours failing every claim in ~250 ms at ~8/s — 1.81M attempts burned,
sustained hammering of an endpoint that was rejecting us, and zero successes
for two and a half days with no operator-visible signal (census only writes
at run end).

**Resolution:** ADR-0050 ("Trip the breaker, never burn the pool"),
implemented in `04a457f`. `process` now aborts the run when a run-global
streak of consecutive claims resolves without a single success —
`--breaker-threshold` (default 50, `0` disables). Tripping cancels the
ADR-0025 supervision token (same drain path, no second shutdown mechanism),
the census (`batch_runs.census_json`) records `breaker_tripped`, and the
process exits with code 4 — all exactly the sizing and DB-visibility this
entry called for. The 2026-08-10 WAF incident (a second, faster-caught
instance of the same failure mode) independently promoted this to
"highest-value engineering item" before the fix landed; that incident's own
record (also on an unmerged branch,
`docs/incident-2026-08-10-waf-impersonation-block`) should likewise be
reconciled against this resolution rather than treated as still-open.

---

## Resolved by v0.5.1 — deadline-attribution patch (2026-08-18)

### yt-dlp's internal retry count is uncapped in our argv (default 10)

**Found in:** the 2026-08-13 v0.5.0 rate batches. A flaky connection to
`webapp-sg.tiktok.com` produced `Giving up after 10 retries` after ~3.5
minutes of silent grinding (10 × 20 s connect timeout) — observed 3 times
in 1,458 claims, each one a multi-minute stall of one download worker and
a `YtDlpOther` retryable (correct disposition; our claim-level retry then
re-adjudicates, so the internal 10 are largely redundant with ADR-0036's
fetch-as-oracle loop).
**Disposition:** add `--retries <small N>` (and consider `--socket-timeout`)
to `build_yt_dlp_args` in the next release — argv is code + `params_json`,
per ADR-0049's posture; do NOT do this via a yt-dlp config file
(config-by-side-effect is the incident-2 anti-pattern, runbook forbids it).
**Trigger to revisit:** next release touching the fetcher argv, or if the
stall frequency grows enough to matter for throughput.

**Resolved by:** `8b7f57a` (`fix(fetcher): cap yt-dlp internal retries at 3
in both argv builders`). `--retries 3` was added to both `build_yt_dlp_args`
(the fetch/download argv) and `build_metadata_only_args` (the
metadata-only argv), landing as an explicit argv element — code +
`params_json`, per ADR-0049's posture — never via a yt-dlp config file; the
config-file route considered in the original entry's Disposition was
explicitly not taken. `--socket-timeout` was not added; not evidenced as
needed by the observed failure mode.
