---
status: accepted
date: "2026-05-20"
category: Orchestration
applies_to:
    - src/pipeline/pipelined.rs
    - src/main.rs
priority: invariant
companions:
    - src/transcribe.rs
    - tests/pipeline_fakes/pipelined_tests.rs
---

# Worker supervision: JoinSet + CancellationToken; shutdown order is load-bearing

## Decision

The orchestrator supervises workers with a shared `tokio::task::JoinSet` plus
a `tokio_util::sync::CancellationToken`, under a load-bearing shutdown
protocol: the orchestrator drops its own fetch→transcribe `mpsc::Sender`
immediately after spawning, cancels the token on the first Bug-class `Err`
or panic, drains
`join_set.join_next()` to completion, and only after the drain does the
caller run `engine.shutdown()` — never inside `run_pipelined`.

## Guidance

- `engine.shutdown()` runs strictly after `run_pipelined` resolves AND after the caller drops its own `Arc<dyn Transcriber>` clone (`main.rs` `Process` arm) — shutting the engine while a worker may still hold an in-flight `engine.transcribe()` wedges that call on a dead engine.
- Keep the unconditional `drop(tx)` right after the spawn loop; without it the transcribe worker parks on `recv()` forever even after all fetch workers exit.
- The transcribe worker wraps `engine.transcribe()` in `tokio::select!` with `token.cancelled()` — channel-close alone cannot interrupt in-flight inference; the token propagates into the per-request `Arc<AtomicBool>` abort flag that whisper.cpp's `abort_callback` polls. Don't replace either half with flag-polling loops or per-worker signal channels.
- Supervision uses `token.cancel()` only, never `join_set.abort_all()` — cancellation is cooperative so workers can finish their current row's state write. First Bug-class `Err` or panic → cancel + drain; process exits 1 on Bug, 0 on clean drain. Workers never sequence shutdown themselves.

## Why

Two deadlocks hide in this ordering, and no happy-path test surfaces either:
skip the `drop(tx)` and the transcribe worker parks on `recv()` until process
exit; shut the engine before the drain completes and a pending `transcribe()`
wedges on a dead engine (the engine worker likewise parks on `blocking_recv`
until its request-side sender count reaches zero). The select + abort-callback
composition keeps cancellation latency at milliseconds instead of the ~1s
largest-await bound.

## Context

The pipelined orchestrator spawns N fetch workers + 1 transcribe worker as
tokio tasks over a bounded mpsc channel, with the whisper engine on a
dedicated thread behind a request channel (`src/transcribe.rs`). The engine
thread exits only when every request-side sender is gone — which is why
engine shutdown is the caller's last act, after the drain has proven no
worker still holds one. On a clean drain the token is never cancelled;
cancellation is purely the Bug-class/panic path.

## Alternatives

- **`Arc<Mutex<bool>>` shutdown flag polled in each loop** — adds latency and busywork; polling granularity bounds cancellation latency.
- **A dedicated signal channel per worker** — redundant with `CancellationToken`, which is purpose-built and select-composable.
- **Manual `JoinHandle` tracking without `JoinSet`** — loses automatic panic propagation.
