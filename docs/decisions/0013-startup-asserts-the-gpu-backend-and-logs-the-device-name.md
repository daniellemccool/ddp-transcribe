---
status: accepted
date: "2026-05-12"
category: Whisper engine
applies_to:
    - src/transcribe.rs
priority: invariant
companions:
    - tests/whisper_engine_init.rs
---

# Startup asserts the GPU backend and logs the device name

## Decision

Engine construction must verify that the whisper.cpp backend actually in use
matches what the build expects (CUDA when the `cuda` feature is on), log the
device index and name for operator audit, and abort with
`WhisperInitError::BackendMismatch` on mismatch — at startup, not at first
transcribe.

## Guidance

- **The assertion is accepted but not yet implemented**: `BackendMismatch` exists unconstructed in `src/transcribe.rs` (its comment says the assertion "lands in T13", which never did) and engine init performs no backend check today. Whoever next touches engine init lands it — capture whisper.cpp's init log via the `whisper_log_set` callback, assert, and emit the `tracing::info!` backend/device line. Until then, a green run is not proof of GPU use.
- Review rejects softening the contract to a warning, and rejects deferring the check past engine construction — the operator must learn about a fallback before any batch work starts.
- `tests/whisper_engine_init.rs` is today a model-load smoke test + shutdown-deadlock guard; the assertion-path test lands with the mechanism.

## Why

whisper.cpp silently falls back to CPU at ~100× slower throughput when GPU
init fails, and a misconfigured CUDA_VISIBLE_DEVICES silently picks the wrong
GPU — a fallen-back run looks completely normal, just uselessly slow, which
is precisely the failure a human won't notice until a workspace-day is gone.

## Alternatives

- **Bake-time manual log inspection** — relies on a human noticing an *absent* log line; the silent-fallback run looks regular.
- **Trust the build flags** — fails the day upstream CUDA detection changes or the workspace toolkit drifts.
