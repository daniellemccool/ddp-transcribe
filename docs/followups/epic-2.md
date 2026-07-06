# FOLLOWUPS — Epic 2 active entries

Active-scope review items targeted for Plan B Epic 2. See `../FOLLOWUPS.md`
for the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

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
