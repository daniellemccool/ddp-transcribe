# uu-tiktok — orchestration

The orchestrator drives the pipeline at runtime: it claims work from the state machine, dispatches fetcher and transcriber workers, supervises them via tokio primitives, and coordinates shutdown. The pipelined orchestrator lives in `src/pipeline/pipelined.rs`; shared types and helpers (used by both it and the serial baseline) live in `src/pipeline/mod.rs`.

## Topology

The orchestrator runs an n=3-fetch + 1-transcribe topology over a bounded mpsc channel of capacity 2, per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md). Both counts are configurable: `download_workers` defaults to 3 and `channel_capacity` defaults to 2 (`src/config.rs:53–54`), flag-tunable via `--download-workers` / `--channel-capacity`. The choice is shaped by GPU saturation: at most one transcribe task usefully runs on the single A10 dev GPU at a time, while fetch is network-bound and benefits from concurrency. The bounded capacity supplies backpressure — when the transcribe worker is busy, fetch workers block on `send` rather than buffering work indefinitely.

Each fetch worker calls `claim_next` itself — there is no separate dispatcher task (`src/pipeline/pipelined.rs:146`). Contention is arbitrated by the state machine's `BEGIN IMMEDIATE` claim transaction (see [`state-machine.md`](state-machine.md) §Claim contention), not by the orchestrator.

A separate `run_serial` topology exists as a single-threaded baseline (no orchestrator, no mpsc, one worker) and is retained for the integration tests (`src/pipeline/serial.rs`); the sections below describe `run_pipelined` unless stated otherwise.

### Topology diagram

```
                          +-----------+
                          |   State   |
                          |  machine  |
                          +-----+-----+
                                ^
        claim_next / mark_* (each worker calls directly; SQLite
        BEGIN IMMEDIATE serializes — no dispatcher task)
                                |
        +-----------------------+------------------------------+
        |            Orchestrator (run_pipelined)              |
        |   JoinSet<Result<()>>  +  CancellationToken          |
        |   spawns 1 transcribe worker FIRST, then N fetch     |
        +--+----------+----------+-----------+-----------------+
           |          |          |           |
           v          v          v           v
      +--------+ +--------+ +--------+  +--------------+
      | Fetch  | | Fetch  | | Fetch  |  |  Transcribe  |
      |  #1    | |  #2    | |  #3    |  |   (single)   |
      +---+----+ +---+----+ +---+----+  +------+-------+
          |          |          |              ^
          | fetch_and_decode (acquire + decode WAV -> PCM)
          +----------+----------+              |
                     |                         |
                     v                         |
       mpsc::Sender<FetchedItem> (cap = 2) ----+
                     |
        FetchedItem { claim, samples: Vec<f32>,
                      samples_len, wav_path, fetcher_name }
```

The channel payload is the `FetchedItem` struct (`src/pipeline/pipelined.rs:65`), not a bare tuple. It extends the `(Claim, Vec<f32>, PathBuf)` triple named in ADR 0027 with `samples_len` (so the transcribe worker derives `duration_s` without the moved `samples` Vec) and `fetcher_name` (so the artifact JSON's `fetcher` field is sourced from the producing fetcher, not a literal).

## Control loop

`run_pipelined` (`src/pipeline/pipelined.rs:492`) runs a one-time stale-claim sweep (per [ADR 0024](../../decisions/0024-stale-claim-sweep-no-validation-no-attempt-count-bump-30-min-default-threshold.md), `:504–505`), constructs the `CancellationToken` and the bounded mpsc channel (`:511–512`), then spawns the single transcribe worker first (`:524`) followed by N fetch workers (`:537–547`). It drops its own sender clone (`:552`) and supervises by draining the `JoinSet` (`:559`).

End-to-end worker boundary: a **fetch worker** polls cancellation, claims one `pending` row via `claim_next` (`:146`), then runs `fetch_and_decode` — which acquires the audio via the fetcher and decodes the WAV to `Vec<f32>` PCM samples (`src/pipeline/mod.rs:137–162`; the decode happens in the **fetch** worker, not the transcribe worker). It packages the result as a `FetchedItem` and sends it over the channel (`:178`). The **single transcribe worker** receives the item (`:311`), runs `transcriber.transcribe` outside the store lock (`:348`), then calls `write_artifacts_and_mark` (`:366`), which writes the `.txt` + `.json` artifacts and calls `mark_succeeded` — artifacts durable *before* the DB mark, per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md). The transcribe worker holds the store mutex only for that sub-50ms write+mark critical section, never across the ~1s transcribe call (`src/pipeline/pipelined.rs:333–337`). This matches [`data-input.md`](data-input.md) (decode in the fetch worker) and [`transcription.md`](transcription.md) (two `WhisperState`s, one transcribe worker).

Each fetch worker exits when `claim_next` returns `None` (drain semantics per [ADR 0026](../../decisions/0026-claim-contention-no-polling-for-plan-b-batch-drain-on-claim-next-none.md), `:153–159`); the orchestrator does not poll. The transcribe worker exits when the channel closes (`recv()` returns `None`, `:311–316`) — which happens once every fetch worker has dropped its sender clone *and* the orchestrator has dropped its own (`:552`).

## Supervision

Per [ADR 0025](../../decisions/0025-bug-class-supervision-joinset-cancellationtoken-shutdown-order-is-load-bearing.md), the orchestrator supervises workers with a `tokio::task::JoinSet<Result<()>>` (`src/pipeline/pipelined.rs:520`) plus a shared `tokio_util::sync::CancellationToken` (`:511`; `tokio-util` is in `Cargo.toml:24`). The token is cloned into every worker (`:525`, `:539`).

- The `JoinSet` holds every spawned worker's handle; `join_next` (`:559`) surfaces each worker's `Ok(())`, application `Err`, or panic.
- Fetch workers observe cancellation by polling `token.is_cancelled()` at the loop top (`:121`). The transcribe worker observes it through two `biased` `tokio::select!` arms — one at the loop top (`:307`) and one wrapping the in-flight transcribe future (`:344`), so cancellation can interrupt a transcription mid-inference (the `CancelOnDrop` chain fires whisper.cpp's `abort_callback`).
- On the first worker `Err` or panic, the supervisor records it as `first_error` and fires `token.cancel()` (`:572`, `:580`), cascading the wind-down to all remaining workers.
- The supervisor always drains the `JoinSet` to completion, so worker errors and panics are observable rather than silently dropped.

`abort_all()` is deliberately *not* used; cancellation is cooperative via the token (`:479–486`). Redirect the supervision *why* — and especially the ordering rationale below — to ADR 0025.

## Shutdown order

This is the load-bearing section per ADR 0025. **Read the order from the code, not from a logical narrative** — there are two distinct teardown paths, and the clean-drain path never calls `cancel()` at all. The `engine.shutdown()` constraint (it must run *last*) is the single invariant a wrong narrative corrupts.

Source-order facts:

1. **`drop(tx)`** — the orchestrator drops its own sender clone unconditionally, immediately after the spawn loop, *before* the join loop (`src/pipeline/pipelined.rs:552`). This is what lets the channel ever close.
2. **`token.cancel()`** — fired *only inside* the join loop, and *only* on a worker `Err`/panic (`:572`, `:580`). It is conditional, not a guaranteed first step.

This yields two paths:

- **Clean drain (no error):** `drop(tx)` at `:552` is already done → each fetch worker exits on `claim_next == None` (`:153`) and drops its sender clone → the channel closes once the last clone is gone → the transcribe worker's `recv()` returns `None` and it exits (`:311–316`) → `join_next` (`:559`) drains every worker `Ok`. **`cancel()` is never called on this path.**
- **Error / panic:** a worker returns `Err` (or panics) → the supervisor fires `token.cancel()` (`:572`/`:580`) → fetch workers observe it at the loop-top `is_cancelled()` poll (`:121`), the transcribe worker via the biased `cancelled()` arm (`:307`) or the in-flight transcribe select arm (`:344`) → workers exit → `join_next` drains them. The first error is re-raised after the drain (`:602–604`).
- **Engine teardown (in the caller, `src/main.rs`):** after the `run_pipelined` future resolves, `main` drops its own `Arc<dyn Transcriber>` clone (`src/main.rs:159`) — the bridge that closes the engine's request channel once the workers have already dropped theirs — and *then* calls `engine.shutdown()` last (`src/main.rs:165`), which consumes the engine by value and joins its worker thread.

The load-bearing constraint per ADR 0025 is that `engine.shutdown()` runs **last** — after `join_next` has drained the workers and after `main` has dropped its transcriber clone. Reversing this (shutting the engine down before the workers drain) wedges the transcribe worker on a dead engine; the `drop(transcriber)` at `src/main.rs:159` is the bridge between the worker drain and the engine teardown. Redirect the rationale to ADR 0025.

## Failure handling

The orchestrator turns worker outcomes into state-machine mutations:

- **Retryable failures** call `Store::mark_retryable_failure(video_id, worker_id, kind, message)`. On current `main` (post-Epic 2, which has closed) the classifier is **string-kind only**: every fetch-path error is passed with the literal placeholder kind `"Fetch"` (`src/pipeline/pipelined.rs:207`) and every transcription error with `"Transcribe"` (`:429`) — two separate strings, not a merged kind.
- **`mark_terminal_failure` has no caller on current main** (`#[allow(dead_code)]`, `src/state/mod.rs:420`). So today *both* fetch and transcribe errors route to `mark_retryable_failure`; nothing reaches terminal classification. The typed `RetryableKind` / `UnavailableReason` / `ClassifiedFailure` taxonomy and variant-driven terminal dispatch are the Epic 3 charter. This reads consistently with [`data-input.md`](data-input.md) §Retry classification and [`state-machine.md`](state-machine.md) §Failure classification.
- **`TranscribeError::Cancelled`** is treated as coordinated shutdown, not a row failure: the worker returns `Ok(())` and the row stays `in_progress` for the next sweep to recover (`src/pipeline/pipelined.rs:388–397`).
- **`TranscribeError::Bug`** and store-call errors are Bug-class: the worker returns `Err`, which the supervisor turns into `token.cancel()` + drain (`:399–411`, `:447–449`).
- **Panics** surface via the `JoinSet` join-error arm; the supervisor logs, records the first error, and cancels the token (`:574–581`) — treated as a fatal run error, not a per-row retryable failure.
- **Stale-claim races** — when `mark_retryable_failure` or `mark_succeeded` returns `Ok(0)` (the claim was swept and re-assigned mid-flight), the worker increments a monotonic `stale_after_failure` / `stale_after_success` counter and continues; it does *not* return `Err` (`:210–224`, `:384–386`, `:432–445`).
- **Subprocess output** (yt-dlp's stdout/stderr) is bounded inside the fetcher per [ADR 0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md) (covered in [`data-input.md`](data-input.md)); the orchestrator needs no separate handling.

## Batch validation contract

The full operational "done" contract per [ADR 0017](../../decisions/0017-operational-done-contract-for-batch-validation.md) is: every in-scope row is in a terminal status (no `pending`/`in_progress` except those skipped via `--max-videos`), every `succeeded` row has its `.txt` + `.json` artifacts on disk, every `.json`'s `raw_signals.schema_version` matches the expected constant, and the batch is pause-safe (no `in_progress` rows awaiting recovery). ADR 0017 explicitly assigns implementation of that contract to **Epic 4's `status` subcommand** — it is the target, not a current orchestrator behavior.

What exists on current `main` is **partial**: the orchestrator *ends* a batch via drain (every fetch worker exits on `claim_next == None` per ADR 0026), and after the drain `compute_process_stats` (`src/pipeline/pipelined.rs:624`) computes a `ProcessStats` by `COUNT(*) GROUP BY status` — `claimed = succeeded + failed_retryable + failed_terminal` — plus the two stale-claim counters merged in from the workers (`:599–600`). It does **not** verify artifacts on disk, does **not** check `raw_signals.schema_version`, and does **not** evaluate the pause-safe predicate. Those checks land with Epic 4's status subcommand; ADR 0017 is the contract it must fulfill.

## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0008 | Artifact-before-`mark_succeeded` | Transcribe worker's write+mark ordering (via `write_artifacts_and_mark`). |
| 0017 | Operational done contract | Batch-validation target; orchestrator implements a partial COUNT-by-status proxy. |
| 0021 | Bounded subprocess output capture | Inherited from fetch workers (covered in `data-input.md`). |
| 0024 | Stale-claim sweep | One-time sweep at orchestrator startup; in-flight stale-claim races. |
| 0025 | JoinSet + CancellationToken shutdown order is load-bearing | Supervision and the engine-shutdown-last ordering. |
| 0026 | Claim contention / no polling / batch drain | Drain-on-`None` worker exit and channel close. |
| 0027 | Orchestrator topology n=3 + 1, mpsc cap 2 | Topology, worker counts, channel shape. |
