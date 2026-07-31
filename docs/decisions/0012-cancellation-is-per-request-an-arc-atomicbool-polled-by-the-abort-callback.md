---
status: accepted
date: "2026-05-12"
category: Whisper engine
applies_to:
    - src/transcribe.rs
priority: invariant
---

# Cancellation is per-request: an Arc<AtomicBool> polled by the abort callback

## Decision

Every `TranscribeRequest` carries its own fresh `cancel: Arc<AtomicBool>` and
`deadline: Instant`; whisper.cpp's `abort_callback` polls both (deadline
elapsed OR flag flipped) inside the encoder/decoder loop. Cancellation state
is never shared across requests and never lives on the engine struct.

## Guidance

- Review rejects any engine-level or shared cancellation flag: a late timeout from request A must be structurally unable to cancel request B, and reset-per-call does not close that race.
- Timeout enforcement is the same single callback polling the deadline — no separate timer task.
- On abort, `state.full()` returns an ordinary `Err` (no unwind), and the worker attributes it to `Err(TranscribeError::Cancelled)` only when the callback actually fired: the callback records its firing in a per-request `abort_fired: Arc<AtomicBool>`, so an unrelated inference `Err` landing just after the deadline is not misclassified as a cancel — keep the attribution on `abort_fired`, never on re-checking the clock. Dropping the request future is itself a first-class cancel: the `CancelOnDrop` guard flips the flag on drop and the worker treats a closed reply channel as an already-cancelled caller — keep the guard; don't bypass it.
- The orchestrator's CancellationToken propagates into this flag (via the select + abort-callback composition); this record owns the per-request half.

## Why

Embedded inference can't be killed like the old subprocess could — without a
cooperative abort, a pathological whisper.cpp state hangs the process and the
operator's only recourse is SIGKILL, losing in-progress claims on every
worker. The per-request shape exists because the engine-level alternative has
a real race (late timer from A cancels B mid-inference) that no reset
discipline fixes.

## Context

The single-callback deadline shape replaced an earlier spawn-a-timer design
during code review: the abort callback already fires frequently during the
encoder/decoder loop, so polling the deadline there covers timeout
enforcement with no extra task. whisper-rs 0.16's `set_abort_callback_safe`
has a type-mismatch bug, so the engine wires a raw FFI trampoline
(`abort_trampoline`, `src/transcribe.rs`) instead.

## Alternatives

- **Engine-level flag reset per call** — racy; reset and check are not atomic with respect to a late timer.
- **No cancellation** — a hung inference wedges the whole batch.
