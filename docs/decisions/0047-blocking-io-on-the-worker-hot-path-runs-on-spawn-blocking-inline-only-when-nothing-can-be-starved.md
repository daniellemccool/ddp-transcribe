---
status: accepted
date: "2026-07-31"
category: Orchestration
applies_to:
    - src/pipeline/mod.rs
    - src/pipeline/pipelined.rs
    - src/fetcher/ytdlp.rs
    - src/output/artifacts.rs
    - src/commands.rs
priority: default
---

# Blocking IO on the worker hot path runs on spawn_blocking; inline only when nothing can be starved

## Decision

Blocking work runs inline only where nothing else on the tokio runtime can
be starved by it; every unbounded blocking unit on the worker hot path runs
on `spawn_blocking` instead.

## Guidance

- Classify a new `std::fs`/rusqlite call in an async fn before it lands.
  **(c)** unbounded in bytes or entries (whole-file read/decode, fsync,
  recursive delete) while `run_pipelined`'s workers are live ⇒
  `spawn_blocking`; **(a)** nothing else runnable on the runtime (startup
  sweep, single-task subcommand, `run_serial`) and **(b)** a bounded
  constant-syscall unit (one `stat`/`mkdir`/`unlink`, a `read_dir` over a
  directory only this process writes, a store call already serialized by
  `Mutex<Store>`) ⇒ inline, with the class named in a comment. Review
  rejects a naked class-(c) call.
- A wrap is behavior-preserving or it is not a wrap: hand a large owned input
  back out of the closure's return value rather than cloning it (a `PathBuf`
  clone to keep a live struct intact is fine), `await` the handle
  immediately, return the closure's `Result` unchanged, and `resume_unwind` a
  panicking `JoinError` so a panic still unwinds the caller's task exactly as
  the inline call did. The non-panic `JoinError` has no producer — nothing
  holds these handles to abort them — so it is an `unreachable!`, never a new
  error kind.
- rusqlite behind `&mut Store` stays sync. It is already serialized by the
  `tokio::sync::Mutex<Store>` every worker queues on, so the blocking pool
  would buy a thread hop and no concurrency — and the lock-hold time, not
  the thread, is what needs watching.
- `write_artifacts_durable` stays a sync fn holding no store handle: the
  async caller wraps the *call*, never the function. Both artifacts still
  land before `mark_after_artifacts`, and the durable writes still run
  outside the store lock.
- A wrap adds an await point, and an await point inside a `select!` is a
  cancellation point. `fetch_worker`'s decode is now cancellable — the
  attempt dir is left to the startup `.work` sweep, the same outcome a
  dropped `acquire` future already has. `transcribe_worker` is cancelled by
  token only (never `abort_all`), so its wrap adds no new drop point.

## Why

An fsync or a whole-file decode run inline holds a runtime worker thread;
with three fetch workers, a transcribe worker and the checkpoint timer
sharing that pool, one slow or networked artifacts volume stalls claim
dispatch and the checkpoint hook — and the symptom, throughput collapse at
idle CPU, reads as a whisper problem rather than a disk one.

## Context

Audit appendix (Epic 5b close-out, 2026-07-30). `main` is `#[tokio::main]`,
so *every* blocking call in this crate is under the runtime — class (a) is
"no other task to starve", not "outside the runtime". Line numbers are of
that audit; the class is the durable part.

| Site | Class | Rationale |
| --- | --- | --- |
| `pipeline/mod.rs:419` `audio::decode_wav` | **c** | whole-file read + f32 conversion, three fetch workers deep — **wrapped** |
| `pipeline/pipelined.rs:609` `write_artifacts_durable` | **c** | mkdir + two `atomic_write`s = three fsyncs, unbounded on a slow volume — **wrapped** |
| `pipeline/mod.rs:704` same call via `write_artifacts_and_mark` | a | `run_serial` is one task; nothing to starve |
| `pipeline/mod.rs:626`, `pipelined.rs:271,323,398,437,634,729,898` store calls | b | rusqlite serialized by `Mutex<Store>` |
| `fetcher/ytdlp.rs:245` `read_dir` in `find_single_wav` | b | fresh per-acquire dir, sole writer, a few entries |
| `fetcher/ytdlp.rs:340` attempt-dir `create_dir_all` | b | one mkdir |
| `fetcher/ytdlp.rs:288` `remove_attempt_dir`; `pipeline/mod.rs:90` wav `remove_file` | b | a handful of unlinks, best-effort, warn-logged |
| `output/artifacts.rs:193`/`:260` sweeps via `commands.rs:99,110` | a | unbounded, but startup — no worker has spawned |
| `commands.rs:56,96,105,122,584`; `ingest.rs:237,249,433`; `status.rs:235,256`; `Store::open`, `batch::run_sweep`, `metadata_loader` | a | single-task subcommands |
| `transcribe.rs:795` model load; whisper inference | — | already off-runtime: dedicated OS thread + rendezvous channel |
