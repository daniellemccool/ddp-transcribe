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
  **(c)** unbounded in bytes or entries (whole-file read/decode, fsync, a
  `read_dir` or recursive delete over a tree whose size this process does not
  fix) while `run_pipelined`'s workers are live ⇒ `spawn_blocking`; **(a)**
  nothing else runnable on the runtime (startup sweep, single-task
  subcommand, `run_serial`) and **(b)** a bounded constant-syscall unit (one
  `stat`/`mkdir`/`unlink`, a `read_dir` or `remove_dir_all` over a directory
  whose entire contents this process created for one video, a store call
  already serialized by `Mutex<Store>`) ⇒ inline, with the class named in a
  comment. Review rejects a naked class-(c) call.
- The trigger is unboundedness, not the syscall's name. The same
  `remove_dir_all` that is (b) over one attempt dir — depth 1, contents fixed
  by this process, one video's worth — would be (c) over the whole `.work`
  root, which is exactly why that sweep is startup-only.
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
- A wrapped call must never sit inside a cancellation `select!`. Dropping a
  `spawn_blocking` `JoinHandle` does NOT cancel the closure — tokio runs it to
  completion detached and its panic is discarded instead of reaching the
  `JoinSet` — so a "cancelled" wrap buys nothing and loses a fault signal.
  `fetch_worker` selects over `acquire_audio` alone (dropping THAT future
  drops a `Child` and really does kill yt-dlp) and awaits `decode_fetched`
  unconditionally; `transcribe_worker` is cancelled by token only, never
  `abort_all`, so it is never dropped mid-body. If a wrap cannot be kept out
  of a `select!`, leave the call inline.

## Why

An fsync or a whole-file decode run inline holds a runtime worker thread;
with three fetch workers, a transcribe worker and the checkpoint timer
sharing that pool, one slow or networked artifacts volume stalls claim
dispatch and the checkpoint hook — and the symptom, throughput collapse at
idle CPU, reads as a whisper problem rather than a disk one.

## Context

Audit appendix (Epic 5b close-out, 2026-07-30). `main` is `#[tokio::main]`,
so *every* blocking call in this crate is under the runtime — class (a) is
"no other task to starve", not "outside the runtime". Sites are named by
function rather than line so the table does not rot.

| Site | Class | Rationale |
| --- | --- | --- |
| `audio::decode_wav` in `pipeline::decode_fetched` | **c** | whole-file read + f32 conversion, three fetch workers deep — **wrapped**, and awaited outside the cancellation `select!` |
| `write_artifacts_durable` call in `pipelined::transcribe_worker` | **c** | mkdir + two `atomic_write`s = three fsyncs, unbounded on a slow volume — **wrapped** |
| same call via `pipeline::write_artifacts_and_mark` | a | `run_serial` is one task; nothing to starve |
| `mark_after_artifacts` + the `pipelined` store calls | b | rusqlite serialized by `Mutex<Store>` |
| `read_dir` in `ytdlp::find_single_wav`; `ytdlp::remove_attempt_dir`'s `remove_dir_all` | b | an attempt dir is bounded BY CONSTRUCTION, which is why the generic "recursive delete ⇒ (c)" reading does not reach it: the dir is created fresh per acquire, exactly one yt-dlp invocation ever writes into it, and it holds that invocation's output alone — one WAV plus its intermediates. Depth is 1 and the entry count is fixed by this process, so the scan and the delete are a handful of syscalls, not a walk of unknown size. Structurally it also cannot be wrapped: removal is reachable from `mark_after_artifacts`, which is sync and store-locked per the artifact-ordering record, so a wrap there would have to detach the task — trading a bounded inline unlink for an unobservable one racing the next acquire |
| `ytdlp::acquire`'s attempt-dir `create_dir_all`; the wav `remove_file` in `FetchedAudio` | b | one mkdir; one unlink |
| `artifacts::cleanup_tmp_files` / `cleanup_work_dirs` via `commands.rs` | a | genuinely unbounded (whole transcripts tree, whole `.work` root) — the reason they are startup-only, before any worker spawns |
| `commands.rs` dir/config reads; `ingest`; `status`; `Store::open`, `batch::run_sweep`, `metadata_loader` | a | single-task subcommands |
| whisper model load + inference | — | already off-runtime: dedicated OS thread + rendezvous channel |
