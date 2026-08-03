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

# Assert the GPU backend at startup

## Decision

Engine construction must verify that the whisper.cpp backend actually in use
matches what the build expects (CUDA when the `cuda` feature is on), log the
device index and name for operator audit, and abort with
`WhisperInitError::BackendMismatch` on mismatch — at startup, not at first
transcribe.

## Guidance

- **The hard-fail is `cfg(feature = "cuda")`-gated and fires at construction, never as a warning.** `EXPECTED_BACKEND` is `ExpectedBackend::Gpu` on a `cuda` build (a non-`Gpu` verdict aborts `WhisperEngine::new` with `WhisperInitError::BackendMismatch`) and `Unconstrained` otherwise, where the backend is reported and nothing is asserted — a CPU dev build must not fail. Every build emits the `tracing::info!` backend/device line, and the check never defers past engine construction.
- **The mechanism is one process-global log bridge, phase-scoped, spanning state creation.** `install_log_bridge` sets whisper-rs's `set_log_callback` (whisper.cpp's `whisper_log_set`, a process-global) exactly once behind a `Once`, before any context init and never replaced per engine; `InitCapture` holds a global phase lock so concurrent engine constructions cannot interleave captures. The capture must span **both** `WhisperContext::new_with_params` **and** the primary `create_state()` — at the pinned whisper.cpp v1.8.3 the `whisper_backend_init_gpu` lines land during state creation, so a capture that ends at context construction sees nothing. Any new init path that reaches a `WhisperContext` installs the bridge first.
- **`using X backend` is a pending claim, not proof.** `detect_backend` is an ordered parse, not a substring search: a `using X backend` line is retracted by a following `failed to initialize X backend` (→ `DetectedBackend::GpuInitFailed`, a CPU fallback), and `no GPU found` is its own verdict. Review rejects re-writing this as a `contains` check — that is the false-Gpu-positive the ordered parse exists to close. `GPU_BACKEND_LINE_PREFIX` keeps the `_gpu` suffix deliberately: `whisper_backend_init` (no suffix) logs identical wording for ACCEL backends such as BLAS.
- `tests/whisper_engine_init.rs` carries the assertion-path tests alongside the model-load smoke test and shutdown-deadlock guard — `test-helpers`-gated and opt-in (`cargo test --features test-helpers -- --ignored`); the assertion test runs on any build and exercises whichever arm the build's features select, so only a `cuda` build reaches the mismatch-abort arm. `detect_backend`'s ordered parse is pinned by in-crate `backend_assertion_tests`, which run on any build.

## Why

whisper.cpp silently falls back to CPU at ~100× slower throughput when GPU
init fails, and a misconfigured CUDA_VISIBLE_DEVICES silently picks the wrong
GPU — a fallen-back run looks completely normal, just uselessly slow, which
is precisely the failure a human won't notice until a workspace-day is gone.

## Alternatives

- **Bake-time manual log inspection** — relies on a human noticing an *absent* log line; the silent-fallback run looks regular.
- **Trust the build flags** — fails the day upstream CUDA detection changes or the workspace toolkit drifts.
