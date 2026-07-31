---
status: accepted
date: "2026-05-20"
category: Orchestration
applies_to:
    - src/pipeline/pipelined.rs
    - src/commands.rs
priority: invariant
companions:
    - src/transcribe.rs
    - tests/pipeline_fakes/pipelined_tests.rs
---

# Shutdown order is load-bearing: cancel, drain the JoinSet, then shut down the engine

## Decision

The orchestrator supervises workers with a shared `tokio::task::JoinSet` plus
a `tokio_util::sync::CancellationToken`, under a load-bearing shutdown
protocol: it drops its fetch→transcribe `mpsc::Sender` right after spawning,
cancels the token on the first Bug-class `Err` or panic (and, with a periodic
task in the set, on clean drain), drains `join_set.join_next()` to
completion, and only then does the caller run `engine.shutdown()` — never
inside `run_pipelined`.

## Guidance

- `engine.shutdown()` runs strictly after `run_pipelined` resolves AND after the caller drops its own `Arc<dyn Transcriber>` clone (the `Process` arm of `commands::dispatch`, `src/commands.rs`; main itself carries no pipeline code since the thin-bin restructure) — shutting the engine while a worker may still hold an in-flight `engine.transcribe()` wedges that call on a dead engine.
- Keep the unconditional `drop(tx)` right after the spawn loop; without it the transcribe worker parks on `recv()` forever even after all fetch workers exit.
- The transcribe worker wraps `engine.transcribe()` in `tokio::select!` with `token.cancelled()` — channel-close alone cannot interrupt in-flight inference; the token propagates into the per-request `Arc<AtomicBool>` abort flag that whisper.cpp's `abort_callback` polls. Don't replace either half with flag-polling loops or per-worker signal channels.
- Supervision uses `token.cancel()` only, never `join_set.abort_all()` — cancellation is cooperative so workers can finish their current row's state write. First Bug-class `Err` or panic → cancel + drain (the next bullet covers the second, error-free trigger); process exits 1 on Bug, 0 on clean drain. Workers never sequence shutdown themselves.
- The `JoinSet` may also hold a periodic non-worker task that loops until cancelled (the `--checkpoint-cmd` hook task, per the checkpoint-hook decision). Such a task never joins on its own, so the supervision loop counts joins and cancels the token once `1 + download_workers` tasks have joined — every real worker done, clean drain, no error. Keep that count-based cancel gated on the periodic task actually being spawned, and keep it `==` so it fires exactly once (on the Bug path the token is already cancelled and it is a no-op). Any future always-on task added to this `JoinSet` must update the expected worker count or the run hangs at completion.

## Why

Three distinct deadlocks hide in this ordering and no happy-path test
surfaces any of them — a wrong order produces a batch that silently never
finishes, not an error; the select + abort-callback composition additionally
keeps cancellation latency at milliseconds instead of the ~1s largest-await
bound.

## Context

The three deadlocks: skip the `drop(tx)` and the transcribe worker parks on
`recv()` until process exit; shut the engine before the drain completes and a
pending `transcribe()` wedges on a dead engine (the engine worker likewise
parks on `blocking_recv` until its request-side sender count reaches zero);
drop the count-based cancel and a checkpointing run drains every video, then
hangs forever in `join_next()` waiting on a task whose only exit is a cancel
that now never comes — a successful batch that never finishes.

The pipelined orchestrator spawns N fetch workers + 1 transcribe worker as
tokio tasks over a bounded mpsc channel, with the whisper engine on a
dedicated thread behind a request channel (`src/transcribe.rs`). The engine
thread exits only when every request-side sender is gone — which is why
engine shutdown is the caller's last act, after the drain has proven no
worker still holds one. Through Epic 4c the token was cancelled only on the
Bug-class/panic path — a clean drain never touched it. Epic 5a's checkpoint
hook put a task in the `JoinSet` that outlives the workers by construction,
which added the second, error-free trigger: cancellation now also means
"the batch is over", not only "something broke".

## Alternatives

- **`Arc<Mutex<bool>>` shutdown flag polled in each loop** — adds latency and busywork; polling granularity bounds cancellation latency.
- **A dedicated signal channel per worker** — redundant with `CancellationToken`, which is purpose-built and select-composable.
- **Manual `JoinHandle` tracking without `JoinSet`** — loses automatic panic propagation.
