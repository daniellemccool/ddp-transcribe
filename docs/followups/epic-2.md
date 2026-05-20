# FOLLOWUPS — Epic 2 active entries

Active-scope review items targeted for Plan B Epic 2. See `../FOLLOWUPS.md`
for the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

---

### `Store::open` records `schema_version` but never reads-and-checks it

**Found in:** T7 code quality review (opus).
**Disposition:** Deferred to Plan B (first schema change).
**Trigger to revisit:** any task that changes `state::schema::SCHEMA_SQL`.

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

`tests/state_claims.rs` exercises each transition independently
(`claim_next` of a pending row, `mark_succeeded` of an in_progress row)
but never composes `claim_next` → `mark_succeeded` → `claim_next` and
asserts the second claim returns `Ok(None)`. A regression that, say,
changed the SELECT predicate to `WHERE status IN ('pending',
'succeeded')` would not be caught by the current suite. T14's
end-to-end fake-fetcher tests will likely cover this incidentally;
if they don't, add a one-liner here.

---

### Worker-side closed-reply path silently swallows the error

**Found in:** T5 (engine shell) — codex-advisor code-quality review.
**Disposition:** Operational logging improvement; not blocking Epic 1.
**Trigger to revisit:** When Epic 2 wires tracing context (per-video request IDs).

T5's worker loop uses `let _ = req.reply.send(...)`, ignoring the case
where the caller dropped the receiver before the worker replied. This is
expected during caller-side cancellation (`CancelOnDrop` fires, future is
dropped) but suspicious otherwise. Once Epic 2 adds request-scoped tracing
context, replace the swallow with a `tracing::warn!` that includes the
video_id / request_id and the elapsed wallclock — so an unexplained dropped
caller is visible in logs.

---

### Mutator test parity — backport `video_events` assertions to T5/T6; no-event-on-stale across all three

**Found in:** T7 spec-compliance review (Sonnet + codex-advisor delegation per 0018).
**Disposition:** Epic 2 cleanup; resolve before Phase 2 close (Epic 2 ships).
**Trigger to revisit:** When approaching Phase 2 close, OR whenever T5/T6 happy-path tests are otherwise edited.

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

### `--max-videos` ignored by `run_pipelined` (silent regression from `run_serial`)

**Found in:** T18 supervision wiring (codex-advisor + opus review).
**Disposition:** Epic 2 cleanup; resolve before Phase 2 close.
**Trigger to revisit:** Phase 2 close cleanup; OR any task that
touches `ProcessOptions::max_videos` or the orchestrator's
fetch_worker loop.

T18 swapped `main::Process` from `run_serial` (which honors
`opts.max_videos` by checking `stats.claimed < max` in the outer
loop) to `run_pipelined` (which does not). The CLI flag still
parses, `Config::download_workers` still threads through, but
`run_pipelined` never reads the field — every pending row drains
regardless of the operator's cap.

T18 added a startup `tracing::warn!` in `main.rs` when
`max_videos.is_some()` so the operator sees the regression in the
log instead of silently. The proper fix is a shared atomic counter
in the orchestrator that fetch workers decrement-and-check before
each `claim_next`, with a coordinated cancellation when the bound
is reached. The dead-code suppression on
`ProcessOptions::max_videos` should come off in the same commit.

A bounded-take adapter on the claim stream is an alternative; either
shape works as long as the cap is honored under N concurrent fetch
workers (the obvious sum-after-the-fact "claim N, take first
max_videos" would over-claim by up to N-1 rows).
