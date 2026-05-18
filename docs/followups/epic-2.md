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

### WhisperEngine teardown can hang once T7 lands real inference

**Found in:** T5 (engine shell) — codex-advisor code-quality review.
**Disposition:** Epic 2 (graceful shutdown / state-machine work).
**Trigger to revisit:** Epic 2 planning, before pipelined orchestrator lands.

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

### `Config::whisper_use_gpu` and `Config::whisper_threads` are unused by Plan B's engine path

**Found in:** T11 (pipeline integration) — Plan A leftovers.
**Disposition:** Defer cleanup sweep to Epic 2.
**Trigger to revisit:** Epic 2's state-machine and config rationalization work,
OR any task that touches `Config::from_args` for unrelated reasons.

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
