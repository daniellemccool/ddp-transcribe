# FOLLOWUPS — Cross-epic / ADR maintenance / verify-then-archive

Cross-epic and verify-then-archive review items. See `../FOLLOWUPS.md` for
the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

These entries do not slot cleanly into a single Epic 2-5 task; they either
itemize multi-epic touchpoints, or they are Epic 1 forward-pointers whose
implementation state has not yet been verified against the shipped Epic 1
code. The controller should re-classify after Epic 2 kickoff (verifying
forward-pointers; routing the remaining mid-epic items).

---

### T1 codex code-quality review — deferred ADR refinements

**Found in:** T1 (ADR drafts for Plan B Epic 1) — codex-advisor code-quality review.
**Disposition:** Deferred. Three blocking findings were resolved inline via `adg comment` (0010 schema_version-as-string; 0012 cancellation-via-abort_callback; 0016 closed-oneshot shutdown carve-out). The six items below are non-blocking for Epic 1.

**Trigger to revisit:**

- **0009 fallback Engine API preservation:** if the CUDA build fallback is ever invoked, the superseding ADR must preserve the public `WhisperEngine` API (samples in, `TranscribeOutput` out, `Arc<AtomicBool>` cancel) so T2–T12 implementations don't have to rewrite. Re-surface when the fallback ADR is drafted.
- **0011 pause-safe checklist references 0017:** 0011's "before pause" checklist mentions only "no in_progress rows," but 0017 defines a stricter pause-safe contract (counts by status + artifact existence + schema-version check). Tighten 0011 to point at 0017's contract once Epic 4's `status` subcommand exists. Re-surface in Epic 4 task expansion.
- **0017 splits pause-safe vs batch-complete:** 0017 currently conflates "every row terminal" with pause-safety. `failed_retryable` rows are pause-safe (no active work) but not batch-complete. Split into two semantics: `idle/pause-safe` = no in_progress + artifacts consistent for `succeeded`; `batch complete` = no `pending` or `failed_retryable` unless operator-accepted. Re-surface in Epic 4 task expansion.
- **0013 global log callback invariant:** whisper.cpp's `whisper_log_set` is process-global, not per-engine. The invariant should be: install the callback once before any context init; route all whisper.cpp logs through one global bridge; do not replace per engine; backend capture must be scoped by init phase or protected by synchronization. Address in T6 implementation or amend 0013 when Plan C multi-engine surfaces.
- **0016 multi-engine GPU memory caution:** the "wraps `WhisperPool` of N Engines" alternative in 0016 risks duplicating model loads on a single GPU (each Engine owns its own `WhisperContext`). Prefer multi-state on one context for same-GPU parallelism; keep the wrapper option only for multi-GPU or process isolation. Amend 0016 when Plan C multi-state/multi-GPU work begins.
- **Error variants enumeration:** 0012/0013/0014/0016 reference typed error variants (`WhisperInitError::BackendMismatch`, `AudioDecodeError::*`, `TranscribeError::Cancelled`, worker-panic, closed-reply) but no ADR enumerates the canonical variant set. Add to T6/T7 implementation tasks (or write a small implementation-constraint ADR if the variants drift across files). Re-surface during T6 dispatch.

---

### T9 integration test only exercises empty-segment path on silence fixture

**Found in:** T9 (raw signals extraction) — codex-advisor code-quality review.
**Disposition:** T13's bake exercises the non-empty path with real spoken audio; no Epic 1 action.
**Trigger to revisit:** A spoken-English fixture is added to `tests/fixtures/audio/` (likely during T13 bake setup).

`transcribe_populates_raw_signals_segments_and_tokens` uses the silence fixture,
which whisper.cpp typically reduces to zero segments. The structural range
assertions (`p in [0.0, 1.0]`, `plog <= 0`, `id >= 0`) are therefore vacuously
true — the per-token extraction loop is never exercised. The non-finite-f32
detection in `extract_segments` and the range guards (codex #2) are similarly
exercised only implicitly via successful inference.

When a spoken-English fixture (say 5-10 seconds, CC0-licensed) is added to
`tests/fixtures/audio/`, this test gains real coverage. Until then, T13's
A10 bake against real TikTok audio is the integration check.

---

### 0013 backend assertion must be cfg(feature = "cuda")-gated

**Found in:** T6 (engine init) — codex-advisor code-quality review.
**Disposition:** Forward-pointer for T13's bake-runbook implementer. **Status unverified against shipped Epic 1 code** — confirm before archiving.
**Trigger to revisit:** During T13 dispatch.

**Audit (2026-05-18): NOT confirmed against shipped Epic 1 code. Re-investigate during Epic 5 cleanup sweep (dead-code reassessment per 0002).** The `WhisperInitError::BackendMismatch` enum variant exists in `src/transcribe.rs:303` with `#[allow(dead_code)]` and a forward-pointing comment at line 292 ("BackendMismatch is constructed by T13's backend-assertion path; suppress dead_code until then"). No `whisper_log_set` callback bridge, no construction site, no backend check are present in shipped Epic 1. Plan B Epic 1's T13 (bake runbook) shipped without the assertion. Either wire it during Epic 5 cleanup or remove the dead variant — both defensible.

T6 currently calls `ctx_params.use_gpu(true)` unconditionally. On non-CUDA
builds, whisper.cpp's CUDA backend is not compiled in and the load silently
falls back to CPU — which is what we want for local dev. T13 adds the
backend-mismatch assertion via `whisper_log_set`; the assertion must NOT
fire on non-CUDA builds where CPU is the expected backend. Gate it via
`cfg(feature = "cuda")` or an explicit `expected_backend` field on
`EngineConfig`, e.g.:

```rust
#[cfg(feature = "cuda")]
const EXPECTED_BACKEND: &str = "CUDA";
#[cfg(not(feature = "cuda"))]
const EXPECTED_BACKEND: &str = "CPU";
```

Then the log-callback bridge compares the captured backend string against
`EXPECTED_BACKEND` and returns `WhisperInitError::BackendMismatch` only on
mismatch.

---

### Revisit `SamplingStrategy::Greedy { best_of }` after T13 bake

**Found in:** T7 (engine transcribe) — codex-advisor code-quality review.
**Disposition:** Bake-data dependent; not blocking Epic 1. See also `bake-findings.md` if the A10 bake produced numbers worth acting on.
**Trigger to revisit:** After T13 produces per-clip wallclock + quality numbers on the A10 workspace.

T7 currently uses `SamplingStrategy::Greedy { best_of: 1 }` — memory-
conservative per sharp-edges.md:35 ("beam_size=5 takes ~7× the KV memory
of greedy"). Plan A's whisper-cli used the default best_of=5. On an A10
(24GB) memory pressure is unlikely to be the binding constraint, and
best_of=5 may give a meaningful quality bump worth the throughput cost.
T13's bake should measure both settings on representative TikTok audio
and pick the one that fits the project's quality/throughput budget. If
best_of != 1 wins, add a `best_of: u8` field to PerCallConfig (or to
EngineConfig if it's a session-level choice).

---

### Diagnostic log when lang_detect's top id disagrees with primary inference

**Found in:** T8 (lang_probs opt-in) — codex-advisor code-quality review.
**Disposition:** Bake-time debugging signal; not Epic 1 critical. See also `bake-findings.md` if the A10 bake observed mismatches.
**Trigger to revisit:** During T13's bake or when investigating language-detection accuracy regressions.

T8 currently discards the `i32` lang_id returned by `lang_state.lang_detect(...)`
(we destructure as `(_lang_id, probs_vec)`). When `req.config.language` is None
(auto-detect mode), the primary inference's `full_lang_id_from_state()` is
authoritative for the artifact, but a mismatch with `lang_detect`'s top id
would be diagnostically interesting — it would indicate the auto-detect
behavior is unstable across encoder passes.

Add a `tracing::debug!` (or `info!` if rare enough) when
`config.language.is_none() && top_lang_id_from_lang_detect != full_lang_id_from_state`,
including both ids and the top probability. Useful during T13 bake when
calibrating language-pin policy.

---

### Plan-brief library-API drift (T13/T19/T16 caught at implementation time)

**Found in:** T13 (`features = ["sync"]`), T19 (`clap::value_parser!(usize).range(1..)`), T16 (stale-test design via pre-claim with different worker_id) — three consecutive Phase 2 tasks where the plan-brief's library-API claim or test-design suggestion didn't match the actually-installed crate behavior, requiring the implementer to detect, deviate, and disclose per ADR 0003.
**Disposition:** Process improvement for future epics.
**Trigger to revisit:** Epic 3 planning kickoff.

Pattern: plan authors assume APIs based on memory or older crate versions.
Three independent catches in one epic suggests the plan-write-time checklist
should include a "verify each library-API claim against the actually-installed
crate version" step.

Suggested forms:

- During plan write, run `cargo doc --open` for each library mentioned and
  spot-check the actual API surface.
- For `Cargo.toml` claims (features, exact versions), check `Cargo.lock` for
  the resolved version + read its `Cargo.toml` from the cargo registry
  (`~/.cargo/registry/src/`).
- For test-design suggestions, hand-trace the production code semantics
  (caller ownership, predicate conditions) before publishing the suggested test.

Epic 3 planning should adopt this checklist; treat as project-level discipline
alongside ADR 0003's deviation-honesty norm.

---

### Architecture doc-set drift detection

**Found in:** T08 (cross-cutting additions) — architecture doc set plan (2026-05-20).
**Disposition:** Standing maintenance concern. The architecture doc set (`docs/reference/architecture/`) was written against post-Epic-2-close `main` and carries NO in-flight stamps (dropped by operator approval — the set was written post-close). No current-epic action required.
**Trigger to revisit:** At each future epic's planning time, check whether the epic touches an architecture-doc-covered surface and add a "revise `docs/reference/architecture/<file>.md`" task (per `index.md` §6 drift-detection).

Known forward-touch points:

- **Epic 3 (failure-classification taxonomy)** will reshape `state-machine.md` and `orchestration.md`: typed `RetryableKind`/`UnavailableReason`/`ClassifiedFailure`, terminal-failure routing, and the `failed_retryable` retry path. Revise both deepdives and update `index.md` §4 (ADR map) for any ADRs added.
- **Epic 4 (operator status subcommand)** will reshape `orchestration.md` §Batch validation contract: the ADR 0017 done-predicate is currently represented only by `compute_process_stats`; the Epic 4 `status` subcommand expands it. Revise `orchestration.md` and update `index.md` §4 accordingly.
