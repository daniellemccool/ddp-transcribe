# Task 08: ADR-0013 backend assertion via `whisper_log_set` (Phase 2)

**Files:**
- Modify: `src/transcribe.rs` (log-callback bridge, backend parse + assertion at engine init, `BackendMismatch` construction site — its `#[allow(dead_code)]` at :292 dies here)
- Test: inline `#[cfg(test)]` in transcribe.rs (parser + mismatch decision) + `tests/whisper_engine_init.rs` (assertion path wiring)

**Interfaces:**
- Consumes: `WhisperInitError::BackendMismatch` (src/transcribe.rs:303, currently unconstructed); whisper-rs's `whisper_log_set`-equivalent hook (consult the `whisper-cpp` skill / whisper-rs docs for the exact binding — `whisper_rs::install_whisper_log_trampoline` or the raw sys call; record which in the report).
- Produces: engine init that fails with `BackendMismatch` when `cfg(feature = "cuda")` expects GPU and the captured init log shows CPU-only; `tracing::info!(backend, device)` line on every successful init.

**Semantics (binding — the cross-epic FOLLOWUPS global-callback invariant + ADR-0013 Guidance):**
- The callback is **process-global**: installed ONCE before any context initialization (`std::sync::Once`/`OnceLock`), routes ALL whisper.cpp log lines through one global bridge, is NEVER replaced per engine; init capture is phase-scoped or mutex-protected so concurrent inits can't interleave capture.
- Parse the captured init lines with a pure function `fn detect_backend(log: &str) -> DetectedBackend` (unit-testable without a GPU; derive the match patterns from real whisper.cpp v1.8.3 init output — the `whisper-cpp` skill's deepdive documents the init log shape).
- Assertion is `cfg(feature = "cuda")`-gated per the FOLLOWUPS entry: CUDA builds hard-fail on CPU fallback at engine construction (never later, never a warning — ADR-0013: "review rejects softening"); non-CUDA builds only log the backend/device line.
- Bridge lines also flow to `tracing` at debug level (one global route — no `eprintln`).

- [ ] **Step 1: Failing tests.** Inline: `detect_backend` on captured-log fixtures (CUDA init excerpt → Gpu, CPU-only excerpt → Cpu, garbage → Unknown); the decision fn (expected-CUDA + Cpu ⇒ `BackendMismatch` carrying both). `tests/whisper_engine_init.rs`: assertion-path test per that file's existing gating conventions (model-dependent tests are `#[ignore]`/feature-gated there — follow suit; the mismatch decision itself is covered by the inline tests).
- [ ] **Step 2: Run to confirm failure** — `cargo test --features test-helpers transcribe -- --test-threads=1`.
- [ ] **Step 3: Implement** (Once-guarded install before first context init; capture-scope synchronization; assertion + info line at init; delete the stale allow at :292 per ADR-0002).
- [ ] **Step 4: CUDA gate.** `cargo build --release --features cuda` must compile; then the runtime smoke: run the ignored engine-init test (or a one-video `process` against a fixture) on this workstation's GPU and confirm the `backend/device` info line reports CUDA. If the local GPU is unavailable, STOP and report BLOCKED — the operator decides between the paused SRC workspace and deferring the smoke (never silently skip it).
- [ ] **Step 5: Full gate** (standard + release + cuda builds).
- [ ] **Step 6: Commit**

```bash
git add src/transcribe.rs tests/whisper_engine_init.rs
git commit -m "feat(transcribe): 0013 backend assertion — global whisper_log_set bridge, cfg(cuda)-gated BackendMismatch at init, backend/device info line"
```
