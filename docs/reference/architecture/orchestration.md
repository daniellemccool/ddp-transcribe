# ddp-transcribe — orchestration

The orchestrator drives the pipeline at runtime: it claims work from the state machine, dispatches fetcher and transcriber workers, supervises them via tokio primitives, and coordinates shutdown. The pipelined orchestrator lives in `src/pipeline/pipelined.rs`; shared types and helpers (used by both it and the serial baseline) live in `src/pipeline/mod.rs`.

## Topology

The orchestrator runs an n=3-fetch + 1-transcribe topology over a bounded mpsc channel of capacity 2, per [ADR 0027](../../decisions/0027-orchestrator-topology-3-fetch-workers-feed-1-transcribe-worker-over-a-capacity-2-channel.md). Both counts are configurable: `download_workers` defaults to 3 and `channel_capacity` defaults to 2 (`src/config.rs:53–54`), flag-tunable via `--download-workers` / `--channel-capacity`. The choice is shaped by GPU saturation: at most one transcribe task usefully runs on the single A10 dev GPU at a time, while fetch is network-bound and benefits from concurrency. The bounded capacity supplies backpressure — when the transcribe worker is busy, fetch workers block on `send` rather than buffering work indefinitely.

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

`run_pipelined` (`src/pipeline/pipelined.rs:492`) runs a one-time stale-claim sweep (per [ADR 0024](../../decisions/0024-stale-claim-sweep-recovers-rows-blind-no-validation-no-attempt-bump.md), `:504–505`), constructs the `CancellationToken` and the bounded mpsc channel (`:511–512`), then spawns the single transcribe worker first (`:524`) followed by N fetch workers (`:537–547`). It drops its own sender clone (`:552`) and supervises by draining the `JoinSet` (`:559`).

End-to-end worker boundary: a **fetch worker** polls cancellation, claims one `pending` row via `claim_next` (`:146`), then runs `fetch_and_decode` — which acquires the audio via the fetcher and decodes the WAV to `Vec<f32>` PCM samples (`src/pipeline/mod.rs:137–162`; the decode happens in the **fetch** worker, not the transcribe worker). It packages the result as a `FetchedItem` and sends it over the channel (`:178`). The **single transcribe worker** receives the item (`:311`), runs `transcriber.transcribe` outside the store lock (`:348`), then calls `write_artifacts_durable` (`.txt` + `.json`, unlocked) followed by `mark_after_artifacts` (`mark_succeeded` under the lock) — artifacts durable *before* the DB mark, per [ADR 0008](../../decisions/0008-artifacts-are-durable-on-disk-before-mark-succeeded.md). Since Epic 4c the transcribe worker holds the store mutex only for the `mark_succeeded` call itself, not for the artifact writes and their fsyncs, and never across the ~1s transcribe call (`src/pipeline/pipelined.rs`). This matches [`data-input.md`](data-input.md) (decode in the fetch worker) and [`transcription.md`](transcription.md) (two `WhisperState`s, one transcribe worker).

Each fetch worker exits when `claim_next` returns `None` (drain semantics per [ADR 0026](../../decisions/0026-workers-drain-and-exit-on-claim-next-none-no-polling.md), `:153–159`); the orchestrator does not poll. The transcribe worker exits when the channel closes (`recv()` returns `None`, `:311–316`) — which happens once every fetch worker has dropped its sender clone *and* the orchestrator has dropped its own (`:552`).

## Supervision

Per [ADR 0025](../../decisions/0025-worker-supervision-joinset-cancellationtoken-shutdown-order-is-load-bearing.md), the orchestrator supervises workers with a `tokio::task::JoinSet<Result<()>>` (`src/pipeline/pipelined.rs:520`) plus a shared `tokio_util::sync::CancellationToken` (`:511`; `tokio-util` is in `Cargo.toml:24`). The token is cloned into every worker (`:525`, `:539`).

- The `JoinSet` holds every spawned worker's handle; `join_next` (`:559`) surfaces each worker's `Ok(())`, application `Err`, or panic.
- Fetch workers observe cancellation by polling `token.is_cancelled()` at the loop top (`:121`). The transcribe worker observes it through two `biased` `tokio::select!` arms — one at the loop top (`:307`) and one wrapping the in-flight transcribe future (`:344`), so cancellation can interrupt a transcription mid-inference (the `CancelOnDrop` chain fires whisper.cpp's `abort_callback`).
- On the first worker `Err` or panic, the supervisor records it as `first_error` and fires `token.cancel()` (`:572`, `:580`), cascading the wind-down to all remaining workers.
- The supervisor always drains the `JoinSet` to completion, so worker errors and panics are observable rather than silently dropped.
- **Checkpoint task (Epic 5a).** With `process --checkpoint-cmd`, one *non-worker* task joins the same `JoinSet`: a timer that runs the operator's hook every `--checkpoint-every` (`checkpoint_task`, `src/pipeline/pipelined.rs`). It holds no sender clone, no store handle, and no claim — so the `drop(tx)` drain semantics are untouched — and it **can only return `Ok(())`**, on cancellation. A hook's nonzero exit, timeout, or spawn failure warns and bumps `ProcessStats::checkpoints_failed`; returning `Err` there would trip the first-error `token.cancel()` above and kill a live campaign's batch over a broken sync script. The hook runs through `process::run`'s bounded machinery per [ADR 0021](../../decisions/0021-subprocess-output-capture-is-bounded-by-construction.md), with the interval doubling as its timeout.

`abort_all()` is deliberately *not* used; cancellation is cooperative via the token (`:479–486`). Redirect the supervision *why* — and especially the ordering rationale below — to ADR 0025.

## Shutdown order

This is the load-bearing section per ADR 0025. **Read the order from the code, not from a logical narrative** — there are two distinct teardown paths, and the clean-drain path never calls `cancel()` at all. The `engine.shutdown()` constraint (it must run *last*) is the single invariant a wrong narrative corrupts.

Source-order facts:

1. **`drop(tx)`** — the orchestrator drops its own sender clone unconditionally, immediately after the spawn loop, *before* the join loop (`src/pipeline/pipelined.rs:552`). This is what lets the channel ever close.
2. **`token.cancel()`** — fired *only inside* the join loop, on a worker `Err`/panic (`:572`, `:580`) or — only when the Epic 5a checkpoint task is running — once all `1 + download_workers` real workers have joined. It is conditional, not a guaranteed first step.

This yields two paths:

- **Clean drain (no error):** `drop(tx)` at `:552` is already done → each fetch worker exits on `claim_next == None` (`:153`) and drops its sender clone → the channel closes once the last clone is gone → the transcribe worker's `recv()` returns `None` and it exits (`:311–316`) → `join_next` (`:559`) drains every worker `Ok`. **`cancel()` is never called on this path** — *unless* `--checkpoint-cmd` is set, in which case the supervisor cancels once the worker count has joined, purely to release the checkpoint timer (which loops until cancelled and would otherwise park `join_next` forever). No worker is alive to observe that cancel.
- **Error / panic:** a worker returns `Err` (or panics) → the supervisor fires `token.cancel()` (`:572`/`:580`) → fetch workers observe it at the loop-top `is_cancelled()` poll (`:121`), the transcribe worker via the biased `cancelled()` arm (`:307`) or the in-flight transcribe select arm (`:344`) → workers exit → `join_next` drains them. The first error is re-raised after the drain (`:602–604`).
- **Engine teardown (in the caller, `src/main.rs`):** after the `run_pipelined` future resolves, `main` drops its own `Arc<dyn Transcriber>` clone (`src/main.rs:159`) — the bridge that closes the engine's request channel once the workers have already dropped theirs — and *then* calls `engine.shutdown()` last (`src/main.rs:165`), which consumes the engine by value and joins its worker thread.

The load-bearing constraint per ADR 0025 is that `engine.shutdown()` runs **last** — after `join_next` has drained the workers and after `main` has dropped its transcriber clone. Reversing this (shutting the engine down before the workers drain) wedges the transcribe worker on a dead engine; the `drop(transcriber)` at `src/main.rs:159` is the bridge between the worker drain and the engine teardown. Redirect the rationale to ADR 0025.

## Failure handling

The orchestrator turns worker outcomes into state-machine mutations via **three-arm classifier dispatch** (Epic 3 taxonomy per [ADR 0033](../../decisions/0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md); Epic 4a routing per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)):

- **Fetch-side errors** run through `classify_fetch_phase` (`src/pipeline/mod.rs`), transcribe-side errors through `classify_transcribe_error` (`src/failure.rs`); both produce a classified failure with three arms driven by the active classification table ([ADR 0037](../../decisions/0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)). **Retryable / requires-cookie** → `Store::record_fetch_failure`, which makes the requeue-vs-exhaust-vs-park decision in one transaction at failure time (Epic 4a, ADR 0036): under the lifetime cap it requeues to `pending` (the re-fetch itself is the liveness oracle — the retired probe's job); a requires-cookie class with no cookies parks; the cap-hit case exhausts into `failed_retryable`. **Terminal** (a proven-dead class per ADR 0033) → `Store::mark_terminal_failure` inline; the row never retries. **Bug** → the worker returns `Err`, which the supervisor turns into `token.cancel()` + drain. In the pipelined orchestrator both workers funnel the `record_fetch_failure` outcome through **one shared dispatch helper**, `handle_record_fetch_failure_outcome` (`src/pipeline/pipelined.rs`), which maps the typed outcome to the run-census counters; `run_serial` keeps its own copy of the same mapping (an adjudicated deviation from verbatim duplication — the serial path is a self-contained baseline). Classification is message/exit-status-driven and network-pure — **no probe on the hot path, and none off it either: the oEmbed probe and the `triage` subcommand retired in Epic 4a** (ADR 0036 supersedes [ADR 0034](../../madr-archive/0034-operator-triage-subcommand-oembed-oracle-via-curl-subprocess-message-class-fast-path-attempt-capped-requeue.md)). This reads consistently with [`data-input.md`](data-input.md) §Retry classification and [`state-machine.md`](state-machine.md) §Failure classification.
- **Fetch cancellation latency (T16 wrap).** The fetch worker's `fetch_and_decode` future is wrapped in a `biased` `tokio::select!` against the `CancellationToken` (`src/pipeline/pipelined.rs:216–223`), mirroring the transcribe-side wrap. When `token.cancel()` fires mid-fetch, the in-flight future drops and `kill_on_drop` reaps the yt-dlp child immediately — shutdown latency is no longer bounded by the largest await (previously up to the 300s yt-dlp timeout). The abandoned row stays `in_progress`; the next run's sweep recovers it per ADR 0024.
- **Cookie-scoped fetch opts** ([ADR 0035](../../decisions/0035-cookies-ride-only-sensitivelogingated-retries-with-argv-redaction.md)). Each claim's fetch options are computed by `cookie_opts_for` (`src/pipeline/mod.rs`): the operator-supplied `--cookies-file` path rides on the yt-dlp invocation **only** when the claim's `last_retryable_kind` snapshot is `SensitiveLoginGated` — i.e. only on retries of login-gated rows (requeued in-batch by `record_fetch_failure` or by the start-of-batch sweep). First attempts never send cookies; the cookie path is redacted from structured subprocess logs and scrubbed from stderr excerpts.
- **`TranscribeError::Cancelled`** is treated as coordinated shutdown, not a row failure: the worker returns `Ok(())` and the row stays `in_progress` for the next sweep to recover (`src/pipeline/pipelined.rs:469–478`).
- **`TranscribeError::Bug`** and store-call errors are Bug-class: the worker returns `Err`, which the supervisor turns into `token.cancel()` + drain.
- **Panics** surface via the `JoinSet` join-error arm; the supervisor logs, records the first error, and cancels the token — treated as a fatal run error, not a per-row retryable failure.
- **Stale-claim races** — when a failure mutator, `mark_terminal_failure` included, or `mark_succeeded` returns `Ok(0)` (the claim was swept and re-assigned mid-flight), the worker increments a monotonic `stale_after_failure` / `stale_after_success` counter (via `handle_mutator_result`, `src/pipeline/pipelined.rs:41`) and continues; it does *not* return `Err`.
- **Subprocess output** (yt-dlp's stdout/stderr) is bounded inside the fetcher per [ADR 0021](../../decisions/0021-subprocess-output-capture-is-bounded-by-construction.md) (covered in [`data-input.md`](data-input.md)); the orchestrator needs no separate handling.

## Batch lifecycle (Epic 4a)

Since Epic 4a, `main`'s `Process` arm wraps the orchestrator in a durable batch lifecycle ([ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)):

1. **Open** — build the active classification table (operator `--classification` TOML or the compiled default; validated hard-fail *before* the model loads), then `open_batch_run` inserts a `batch_runs` row snapshotting the params JSON and the full policy TOML, returning a `run_id`.
2. **Sweep** — `batch::run_sweep` (`src/batch.rs`) adjudicates every parked `failed_retryable` row through the table before the drain begins: terminal classes write off (`sweep_mark_terminal`), retryables and the cookie pool requeue under the lifetime cap (`sweep_requeue`), and requires-cookie rows with no cookies stay parked. This is where historical write-off pools and cross-batch stragglers die or re-enter on the first post-upgrade run — no operator subcommand needed.
3. **Drain** — the pipelined orchestrator runs (fresh work first, then requeued retries per the `attempt_count ASC` claim ordering), dispatching failures through `record_fetch_failure` as above.
4. **Close** — `close_batch_run` stamps the census JSON (sweep counters + run counters) and finish time onto the `batch_runs` row, and the census also prints for the operator. A census without its generating policy is not reproducible attrition documentation, so the policy TOML and the census ride in the same row.

## Batch validation contract

The full operational "done" contract per [ADR 0017](../../madr-archive/0017-operational-done-contract-for-batch-validation.md) is: every in-scope row is in a terminal status (no `pending`/`in_progress` except those skipped via `--max-videos`), every `succeeded` row has its `.txt` + `.json` artifacts on disk, every `.json`'s `raw_signals.schema_version` matches the expected constant, and the batch is pause-safe (no `in_progress` rows awaiting recovery). ADR 0017 explicitly assigns implementation of that contract to the **`status` subcommand** (Epic 4b) — it is the target, not a current orchestrator behavior.

What exists on current `main` is **partial**: the orchestrator *ends* a batch via drain (every fetch worker exits on `claim_next == None` per ADR 0026), and the run census is assembled from **input-side per-attempt counters** the workers accumulate as they land each outcome (per [ADR 0007](../../decisions/0007-stats-structs-count-the-input-side-with-verb-named-parallel-counters.md); the Epic 3 `compute_process_stats` COUNT-by-status pass was deleted in the Epic 4a T06 review). Because the counters are per-attempt, a fail-once-then-recover video contributes `claimed = 2, failed = 1, succeeded = 1` — attempts, not distinct videos. It does **not** verify artifacts on disk, does **not** check `raw_signals.schema_version`, and does **not** evaluate the pause-safe predicate. Those checks land with the Epic 4b status subcommand; ADR 0017 is the contract it must fulfill.

## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0008 | Artifact-before-`mark_succeeded` | Transcribe worker's write+mark ordering (`write_artifacts_durable` → `mark_after_artifacts`). |
| 0017 | Operational done contract | Batch-validation target; orchestrator implements a partial COUNT-by-status proxy. |
| 0021 | Bounded subprocess output capture | Inherited from fetch workers (covered in `data-input.md`). |
| 0024 | Stale-claim sweep | One-time sweep at orchestrator startup; in-flight stale-claim races. |
| 0025 | JoinSet + CancellationToken shutdown order is load-bearing | Supervision and the engine-shutdown-last ordering. |
| 0026 | Claim contention / no polling / batch drain | Drain-on-`None` worker exit and channel close. |
| 0027 | Orchestrator topology n=3 + 1, mpsc cap 2 | Topology, worker counts, channel shape. |
| 0033 | Evidence-derived failure taxonomy + inline write-off | Three-arm classifier dispatch in both workers. |
| 0034 | Operator triage subcommand (superseded by 0036) | Historical; triage + probe retired in Epic 4a. |
| 0035 | Cookies scoped to SensitiveLoginGated retries | `cookie_opts_for` kind-gated fetch opts + redaction. |
| 0036 | In-batch capped retry + end-of-queue claim ordering | `record_fetch_failure` dispatch, shared outcome helper, batch lifecycle (open→sweep→drain→close). |
| 0037 | Operator-editable TOML classification table | Classifier dispatch reads the active table; policy snapshot in `batch_runs`. |
