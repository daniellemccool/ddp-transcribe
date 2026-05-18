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

### T8 lang_probs needs a SECOND WhisperState allocated in init phase

**Found in:** T7 (engine transcribe) — codex-advisor code-quality review.
**Disposition:** Forward-pointer for T8 dispatch. **Status unverified against shipped Epic 1 code** — confirm before archiving.
**Trigger to revisit:** During T8 implementer dispatch.

T8 implements `--compute-lang-probs` (per 0010 + PerCallConfig). Per
sharp-edges.md:13-15: `whisper_lang_auto_detect_with_state` re-encodes
the audio AND clobbers `state->decoders[0]` + `state->logits`. So it
MUST run on a separate WhisperState from the primary inference state —
otherwise concurrent state corruption.

T7's worker currently allocates ONE state in the init phase. T8 should:
1. Allocate a SECOND state (e.g., `lang_state`) in the same init phase,
   alongside the primary `state`. Surface allocation failure via
   `WhisperInitError::StateCreate` (same variant as T7).
2. When `req.config.compute_lang_probs` is true, call `lang_state.lang_detect(&samples)`
   (or equivalent whisper-rs API) BEFORE the primary `state.full(...)` —
   the lang_detect call populates `state.full_lang_probs()` (or whichever
   getter returns the full distribution).
3. Reuse `lang_state` across requests — like the primary state.
4. If `compute_lang_probs` is false (default), skip the lang_detect call
   entirely so the unused state is just held in memory (no extra encoder pass).

Memory cost: ~500MB-1GB for the second state (per concurrency.md). On A10
this is fine; on dev machine it doubles the working set during testing.

---

### 0013 backend assertion must be cfg(feature = "cuda")-gated

**Found in:** T6 (engine init) — codex-advisor code-quality review.
**Disposition:** Forward-pointer for T13's bake-runbook implementer. **Status unverified against shipped Epic 1 code** — confirm before archiving.
**Trigger to revisit:** During T13 dispatch.

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

### T9 extraction must reject non-finite f32 values from whisper-rs

**Found in:** T4 (TranscribeOutput types) — codex-advisor code-quality review.
**Disposition:** Forward-pointer for T9's implementer brief. **Status unverified against shipped Epic 1 code** — confirm before archiving.
**Trigger to revisit:** During T9 dispatch.

When T9 extracts `p`, `plog`, and `no_speech_prob` from whisper-rs into
`TokenRaw` / `SegmentRaw`, validate that the values are finite before
constructing the output. `serde_json` will refuse to serialize `NaN`/`inf`,
so a bad value would surface only at T10's artifact-write step and abort
the inference for an unhelpful reason. Reject non-finite values at the
extraction boundary with a typed `TranscribeError` variant (likely
`TranscribeError::Bug` since whisper-rs returning NaN/inf would itself
indicate a model-loading or audio-input pathology that shouldn't happen
with the 0014 input invariant). Include the offending value, segment
index, and token index in the error for operator-readable diagnostics.

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
