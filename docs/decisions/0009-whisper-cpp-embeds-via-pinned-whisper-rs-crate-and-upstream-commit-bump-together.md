---
status: accepted
date: "2026-05-12"
category: Whisper engine
applies_to:
    - Cargo.toml
    - src/transcribe.rs
priority: invariant
---

# Bump whisper-rs with upstream

## Decision

whisper.cpp is embedded in-process through the whisper-rs binding, pinned
exactly (`whisper-rs = "=0.16.0"`, tracking whisper.cpp v1.8.3 via
whisper-rs-sys) so behavior is reproducible across workspace re-provisions.
The crate pin and the tracked upstream commit move together, never
independently.

## Guidance

- Bump the pin only as a pair (crate version + tracked whisper.cpp commit, both lines in `Cargo.toml`), then re-run the bake measurements and verify they match prior numbers before merging.
- The documented fallback if a CUDA build breaks irrecoverably is patching whisper-cli's JSON writer and returning to the subprocess pattern — recorded in a superseding ADR at that point; review rejects drive-by fallbacks to custom FFI or an HTTP server.
- Build prerequisite: whisper-rs-sys runs bindgen, which needs libclang (Arch: `clang`).

## Why

Embedding exists to capture per-video confidence signals (token p/plog,
no_speech_prob) that whisper-cli's JSON never emits, and to amortize the
model load that dominated the subprocess pipeline's runtime. An unpinned
binding lets a re-provision silently change transcription behavior under a
research corpus mid-study.

## Alternatives

- **Custom FFI binding in-repo** — prohibitive maintenance for a one-developer project; whisper-rs already tracks the evolving C API 1:1 and is the upstream-recommended binding.
- **whisper-server over HTTP** — out-of-process hop, serialization cost, extra failure surface; right for cross-machine fan-out, not single-process single-GPU.
- **Stay on the whisper-cli subprocess** — keeps the per-invocation model load; kept only as the documented fallback.
