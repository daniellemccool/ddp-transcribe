---
status: accepted
date: "2026-05-20"
comments:
    - author: Danielle McCool
      date: "2026-05-20 13:54:51"
      text: marked decision as decided
---

# Bug-class supervision: JoinSet + CancellationToken; shutdown ORDER is load-bearing

## Context and Problem Statement

Phase 2 spawns N fetch workers + 1 transcribe worker as tokio tasks. The orchestrator must (a) supervise all workers and panic-propagate; (b) implement graceful shutdown on first Bug-class error or panic; (c) avoid deadlocking on shutdown — specifically, avoid the engine worker parking on `blocking_recv` until process exit.

## Considered Options

* `tokio::task::JoinSet` + `tokio_util::sync::CancellationToken` + `tokio::select!` at engine call
* `Arc<Mutex<bool>>` shutdown flag polled in each loop
* Channel-based cancellation (extra signal channel per worker)
* `tokio::spawn` + manual `JoinHandle` tracking (no JoinSet)

## Decision Drivers

- Composes cleanly with Epic 1's 0012 per-request cancellation (`Arc<AtomicBool>` flipped by abort_callback)
- Cancellation latency: O(milliseconds), not O(seconds)
- No deadlock on coordinated shutdown
- Panic-propagation is automatic

## Decision Outcome

Chosen option: "`tokio::task::JoinSet` + `tokio_util::sync::CancellationToken` + `tokio::select!` at engine call", because Polling Arc<Mutex<bool>> adds latency and busywork; channel-based cancellation is redundant with CancellationToken; manual JoinHandle tracking loses JoinSet panic-propagation..

## Consequences

- All fetch workers + the transcribe worker spawn into a shared `tokio::task::JoinSet`. A shared `tokio_util::sync::CancellationToken` is cloned to each worker. Main loops on `join_set.join_next()`; on first `Err(Bug)` or panic, `token.cancel()` and drains remaining tasks.
- The transcribe worker's hot inner call wraps `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => Err(Cancelled), r = engine.transcribe(...) => r }`. When `token.cancel()` fires, the select arm wins and drops the in-flight `transcribe()` future. The `CancelOnDrop` guard inside `transcribe()` (Epic 1's 0012) fires the `Arc<AtomicBool>` cancel flag, which whisper.cpp's `abort_callback` polls — inference aborts within milliseconds. This is the composition path from Epic 1's cancellation primitive to Phase 2's orchestrator.
- **Shutdown ORDER is load-bearing.** Without this exact sequence the engine worker parks on `blocking_recv` until process exit (codex-advisor identified this during brainstorm):
  1. `token.cancel()` — signals all workers to stop.
  2. Drop the fetch→transcribe `mpsc::Sender` — closes the channel so `transcribe_worker`'s `receiver.recv()` returns `None` and the worker exits its loop.
  3. `join_set.join_next()` to completion — waits for all workers to return.
  4. `engine.shutdown()` — LAST. Drops the engine's request-side `mpsc::Sender`; the engine worker thread sees the closed channel, exits its `blocking_recv` loop, and the join completes.

  Reversing 2 and 4 (calling `engine.shutdown()` before draining `transcribe_worker`) would cause `transcribe_worker`'s pending `engine.transcribe()` to wedge on a dead engine. Reversing 1 and 2 (dropping the sender before cancelling) would prevent the CancellationToken from interrupting in-flight `transcribe()` calls.
- Process exits 1 on Bug; 0 on clean drain.
- Cancellation latency is bounded by the largest single `await` in the worker loops (typically `engine.transcribe()` for the transcribe worker, ~1s on `large-v3-turbo-q5_0`; whisper.cpp's `abort_callback` polls every few ms during inference).
- The transcribe worker MUST wrap `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => ..., r = engine.transcribe(...) => r }`; otherwise cancellation only catches at await-yield points between videos.
- The orchestrator's main is responsible for the shutdown order; the workers themselves don't sequence it.
- 0012's `Arc<AtomicBool>` per-request cancellation stays unchanged; it's the *propagation mechanism* from CancellationToken into whisper.cpp's `abort_callback`.

## Comments

* **2026-05-20 13:54:51 — @Danielle McCool:** marked decision as decided
