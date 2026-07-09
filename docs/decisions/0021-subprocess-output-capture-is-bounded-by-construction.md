---
status: accepted
date: "2026-05-18"
category: Subprocess
applies_to:
    - src/process.rs
    - tests/process_bounded_capture.rs
priority: invariant
checks:
    - desc: no unbounded read of child streams in the subprocess runner
      grep: 'read_to_end'
      in: ["src/process.rs"]
      expect: absent
---

# Subprocess output capture is bounded by construction

## Decision

`process::run` captures child stdout/stderr through a streaming bounded reader
(a `VecDeque<u8>` of the configured cap that pops leading bytes when full), so
retained memory per subprocess is `stdout_capture_bytes + stderr_capture_bytes`
by construction. Unbounded reads of child streams are not allowed.

## Guidance

- Review rejects any `read_to_end` (or equivalent grow-with-output read) on child stdout/stderr; peak memory must never scale with what the child emits.
- Call sites opt in to capture explicitly on `CommandSpec`: `stdout_capture_bytes: 0` discards (`CommandOutcome::stdout` is `Option<Vec<u8>>` — `None` = intentionally discarded, distinct from captured-but-empty).
- Both streams are drained concurrently (`tokio::try_join!`) even when discarded, so a chatty child can never block on a full pipe.
- `tests/process_bounded_capture.rs` asserts the peak-memory bound directly (an `Arc<AtomicUsize>` allocation counter) — it is the executable form of this rule; change capture semantics there deliberately, not incidentally.

## Why

A misbehaving yt-dlp emitting gigabytes to stderr would otherwise be allocated
in full before truncation; under N concurrent fetch workers that is an OOM.
No fixture-based test emits enough output to trip it, so the bound must hold
by construction, not by observation.

## Context

The original `run` read both streams via `read_to_end` into unbounded
`Vec<u8>` and then sliced an 8 KiB stderr tail — `stderr_capture_bytes`
bounded the *retained excerpt*, not peak memory during the read. The bound
applies to the retained buffer; transient chunk buffers hold at most one
`read()` worth of bytes each before draining into the deque. The fetcher's
call site retains stderr (trailing 8 KiB) and discards stdout.

## Alternatives

- **Keep `read_to_end`, bound only the retained slice** — the status quo it replaced; peak memory stays unbounded.
- **Split `CommandOutcome` into an enum (`AudioFile` / `Captured`)** — tighter types but a heavier public-surface change than the problem warranted; the `Option<Vec<u8>>` shape distinguishes the cases without restructuring call sites.
