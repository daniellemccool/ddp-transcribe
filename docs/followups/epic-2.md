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
