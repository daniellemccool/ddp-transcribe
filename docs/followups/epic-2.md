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

---

### `fetch_worker` cancellation latency bounded by largest await, not by `token.cancel()`

**Found in:** T16 codex review (Sonnet + codex-advisor delegation per 0018), surfaced again in T18 Opus deep review.
**Disposition:** Phase 2 close scope OR Epic 3 graceful-shutdown work.
**Trigger to revisit:** If operator-observable shutdown latency on Bug-class errors becomes a complaint, OR when Epic 3's failure-classification work touches `fetch_worker`.

`fetch_worker` polls `token.is_cancelled()` only at loop top. The hot
await is `fetcher.acquire()` (multi-second; up to `cfg.ytdlp_timeout =
300s` default). When `token.cancel()` fires, the worker continues until
`acquire()` returns naturally. `CancellationToken::cancel()` does NOT
drop the worker future; `kill_on_drop` on the yt-dlp subprocess only
fires when the future is actually dropped.

Two fix options for a future task:

- **(a)** Wrap `fetcher.acquire()` in
  `tokio::select! { _ = token.cancelled() => Err(Cancelled), r = fetcher.acquire(...) => r }`.
  Mirrors T18 fixup's transcribe-side wrap (`a66d38b`). Future-drop fires
  `kill_on_drop` on the subprocess.
- **(b)** The orchestrator's first-error path could call
  `join_set.abort_all()` after a grace period to force future-drop.
  Faster but loses graceful-cleanup chance for in-flight fetches.

Worst-case observable: ~5 min shutdown latency on Bug-class errors with
stuck fetches. Best case: <100ms.

---

### sync `write_artifacts_and_mark` inside `tokio::sync::Mutex` guard inside async fn can stall under `TOKIO_WORKER_THREADS=1`

**Found in:** T17 codex review.
**Disposition:** Phase 2 close scope or Epic 5 ops-hygiene work.
**Trigger to revisit:** If T20 bake or production logs show single-worker tokio stalling during write+mark phase.

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
