# FOLLOWUPS resolved — archive

Append-only history of resolved FOLLOWUPS entries. When an entry in
`docs/FOLLOWUPS.md` is resolved, move it here with the resolving commit
SHA. Do not edit prior entries in place. Sibling files: `docs/FOLLOWUPS.md`
(active-scope), `docs/cosmetic-followups.md` (deferred indefinitely),
`docs/bake-findings.md` (operational observations).

---

## Resolved by Plan B Epic 1

The three entries below were resolved by Plan B Epic 1 work but had not
yet been moved out of `docs/FOLLOWUPS.md` at the AD0020 restructure.
Resolving commits are not annotated inline; the EPIC-5-SKETCH resolution
map (`docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md` lines
120-148) is the authoritative pointer until per-entry SHAs are
backfilled.

### `transcribe::transcribe` error mapping is inconsistent and lossy

**Found in:** T12 code quality review (opus).
**Resolution:** Plan B Epic 1 (T11 deletes the `transcribe::transcribe` function and reroutes via `WhisperEngine`). Per EPIC-5-SKETCH map.

Three concerns in `src/transcribe.rs::transcribe`, none blocking for Plan A's
serial happy path:

1. **Inline `.map_err(|e| match e {...})` instead of `From<RunError> for TranscribeError`.**
   T6 chose the `From` idiom for `FetchError` so fetcher code can use `?`
   directly; T12 chose the inline match. Brief's intentional choice (no
   `From<RunError> for TranscribeError` impl in `errors.rs`), but Plan B's
   failure-classification work should harmonize on one idiom across both
   error types.

2. **`exit_code: -1` sentinel collapses non-Timeout RunError variants.**
   `RunError::Spawn`, `RunError::Io`, and any Plan B additions all collapse
   to `TranscribeError::Failed { exit_code: -1, stderr_excerpt: other.to_string() }`.
   Same loss-of-signal already flagged for T6's `From<RunError> for FetchError`
   and `status.code().unwrap_or(-1)`. Whisper-cli OOM (signal kill) and
   missing whisper-cli binary become indistinguishable to a downstream
   classifier.

3. **`exit_code: 0` for post-success artifact-read failure is misleading.**
   When `std::fs::read_to_string(&txt_path)` fails after a 0-exit
   whisper-cli run, the error is built as
   `TranscribeError::Failed { exit_code: 0, stderr_excerpt: "reading {path}: {io_err}" }`.
   A downstream consumer reading `exit_code: 0` would conclude the tool
   succeeded; the failure was actually in the artifact-reading step.
   Parallel to T11's `wav_path.exists() == false → FetchError::ParseError`
   mismatch. Plan B should introduce a dedicated variant
   (e.g., `TranscribeError::ArtifactMissing` /
   `TranscribeError::ArtifactUnreadable`).

---

### `pipeline_fakes` test gaps: `transcribed_at` RFC 3339, wav cleanup, re-run idempotence

**Found in:** T14 code quality review (opus); narrowed in T11 (Plan B Epic 1).
**Resolution:** Plan B Epic 1 — T11 reads and deserializes the `.json` artifact and asserts `model`, `transcript_source`, `fetcher`, plus the full `raw_signals` projection (schema_version, language, segments, tokens). Per EPIC-5-SKETCH map ("Pipeline hardcodes fetcher/transcript_source (T14)" and "`pipeline_fakes` test doesn't verify .json (T14)" both marked Resolved by Plan B Epic 1).

Three smaller gaps remained from the original T14 finding after T11's narrowing:

1. `transcribed_at` is not asserted to be RFC 3339; a regression that
   changed `Utc::now().to_rfc3339()` to a non-RFC format would still pass.
2. The staged `fake.wav` cleanup post-success (`!fake_wav.exists()`) is
   not asserted; a regression that skipped `std::fs::remove_file` would
   still pass.
3. Re-run idempotence (`max_videos: Some(2)` against one pending row
   returns `claimed: 1` on the second invocation, not 2) is not exercised.

Per the resolution map these gaps were closed out alongside the T11
artifact-deserialization assertions; if any of the three remain
empirically uncovered, re-open as a new active entry rather than
editing this archive.

---

### Wav cleanup-before-mark_succeeded ordering inverted in T11; documented in pipeline.rs

**Found in:** T11 (pipeline integration).
**Resolution:** Resolved in T11 — the pipeline order was inverted (`mark_succeeded → remove_file` rather than `remove_file → mark_succeeded`); the entry was kept in FOLLOWUPS as a future-reader signpost rather than a pending action.

Plan A's `pipeline::process_one` did `remove_file(wav) → mark_succeeded`
in that order. If `mark_succeeded` failed (rare; SQLite write error), the
wav was already gone — recovery had no audio to re-transcribe. T11
reversed the order: `mark_succeeded → remove_file`. If `mark_succeeded`
fails, the wav stays on disk and a future retry can pick it up.

The inverted order trades one form of waste for another: if `remove_file`
fails after `mark_succeeded`, the wav lingers (operator sweeps), but the
DB and artifacts are durable. This is the strictly safer trade. The
ordering is intentional and documented in `src/pipeline.rs::process_one`'s
inline comments — not a regression to revert.

Epic 2's state-machine work may revisit this when adding stale-claim
recovery or retry: at that point, a typed "wav still on disk" signal
might become useful for re-claiming a row.

---

## Resolved by perf-tweaks worktree (2026-05-18)

Three entries resolved by the perf-tweaks worktree commits that merged before Plan B Epic 2's T11 began. Coordinated cross-session with the Epic 2 author — see `docs/superpowers/specs/2026-05-13-perf-tweaks-design.md` § Cross-session coordination.

### `process::run` buffers full stderr/stdout in memory before truncation

**Found in:** T6 code quality review (opus).
**Originally:** FOLLOWUPS L47, routed to Epic 2.
**Resolved by:** commit `9e84b54` (`feat(process): bounded streaming subprocess capture`) on `feat/perf-tweaks`. AD0021 records the design.

`src/process.rs` previously read entire stdout AND stderr streams into `Vec<u8>` via `read_to_end` before slicing the tail; the `*_capture_bytes` field only bounded the retained excerpt, not peak memory. The perf-tweaks worktree replaced this with a streaming reader filling a `VecDeque<u8>` of size `cap`; peak retained memory is now bounded by construction. `stdout` capture got a symmetric opt-in via `stdout_capture_bytes`; `CommandOutcome::stdout` is now `Option<Vec<u8>>` (`None` = intentionally discarded). Cross-session coordination: Plan B Epic 2's T13 inherits the design and may add per-tool stdout defaults on top of AD0021 without authoring a new ADR.

---

### `ring_buffer_tail` is misnamed (it's not a ring buffer)

**Found in:** T6 code quality review (opus).
**Originally:** FOLLOWUPS L48, routed to Epic 2.
**Resolved by:** same commit `9e84b54`. The helper is removed; capture is bounded by construction rather than by post-hoc tail-slicing. No rename needed.

---

### Lazy-allocate lang_state on first opt-in request

**Found in:** T8-Epic1 (lang_probs opt-in) — codex-advisor code-quality review.
**Originally:** FOLLOWUPS L87, routed to Plan C.
**Resolved by:** commit `17716ef` (`refactor(transcribe): lazy-allocate lang_state on first opt-in request`) on `feat/perf-tweaks`. Brought forward from Plan C scope.

`WhisperEngine` worker thread previously allocated `lang_state` unconditionally at startup; non-opt-in workers paid ~500MB-1GB VRAM/host overhead for an unused state. Replaced with `Option<WhisperState>` lazily allocated on the first request with `compute_lang_probs=true`. AD0016 invariant preserved (state stays inside the worker thread). New `tests/transcribe_lang_state.rs` asserts via an `Arc<AtomicUsize>` counter that non-opt-in workers never allocate and that opt-in workers allocate exactly once.

---

## Resolved by Plan B Epic 2 — T1 audit (2026-05-18)

Two `verify-then-archive` forward-pointers from Plan B Epic 1's codex-advisor reviews were audited against shipped Epic 1 `src/transcribe.rs` during Plan B Epic 2 T1 (commit landing alongside this archive update). Both confirmed shipped and archived here. The third audit candidate (`0013` backend assertion) was NOT confirmed and remains in `docs/followups/cross-epic.md` with an audit note (see commit message).

### T8 lang_probs needs a SECOND WhisperState allocated in init phase

**Found in:** T7 (engine transcribe) — codex-advisor code-quality review.
**Originally:** `docs/followups/cross-epic.md` (Plan B Epic 1 forward-pointer for T8 dispatch).
**Resolved by:** commit `a3b7261` (`feat(transcribe): wire --compute-lang-probs opt-in for lang_probs`) on `main` — initial second-state allocation alongside the primary inference state. Refined in `17716ef` (perf-tweaks: `refactor(transcribe): lazy-allocate lang_state on first opt-in request`) to lazy-on-first-opt-in.

**Resolution:** confirmed against shipped Epic 1 code. `src/transcribe.rs:461` declares `let mut lang_state: Option<whisper_rs::WhisperState> = None`; lines 485–491 lazily allocate it on the first `req.config.compute_lang_probs == true` request via `ctx.create_state()`; lines 619–628 use it for `pcm_to_mel` + `lang_detect` to populate `lang_probs`. The shipped behavior is a refinement of the original guidance (lazy instead of eager init-phase allocation), preserving the architectural goal (separate state for lang_probs avoids clobbering the primary state's decoders/logits) and improving the memory profile for non-opt-in workers.

---

### T9 extraction must reject non-finite f32 values from whisper-rs

**Found in:** T4 (TranscribeOutput types) — codex-advisor code-quality review.
**Originally:** `docs/followups/cross-epic.md` (Plan B Epic 1 forward-pointer for T9's implementer brief).
**Resolved by:** commit `ce55d9b` (`feat(transcribe): extract per-segment + per-token raw signals from whisper-rs`) on `main`.

**Resolution:** confirmed against shipped Epic 1 code. `src/transcribe.rs::extract_segments` validates finite values when constructing `SegmentRaw` and `TokenRaw`:

- line 109: `if !no_speech_prob.is_finite() || !(0.0..=1.0).contains(&no_speech_prob) { return Err(...) }`
- line 131: `if !td.p.is_finite() || !(0.0..=1.0).contains(&td.p) { return Err(...) }`
- line 138: `if !td.plog.is_finite() || td.plog > 0.0001 { return Err(...) }`

`extract_segments` returns `Result<Vec<SegmentRaw>, String>`; the worker maps this to `TranscribeError::Bug` at line 725. Behavior matches the guidance: reject non-finite at the extraction boundary so `serde_json::to_string` never sees `NaN`/`inf`.
