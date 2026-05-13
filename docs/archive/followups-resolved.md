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
