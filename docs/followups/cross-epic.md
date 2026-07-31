# FOLLOWUPS — Cross-epic / ADR maintenance / verify-then-archive

Cross-epic and verify-then-archive review items. See `../FOLLOWUPS.md` for
the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

These entries do not slot cleanly into a single Plan B epic task; they either
itemize multi-epic touchpoints, or they are blocked on a precondition no
scheduled epic supplies.

**Plan B close-out status (2026-07-30).** The Epic 1 forward-pointers are all
verified and terminal: the `0013` cfg-gate entry and the `0011`/`0017`/`0013`
bullets of the T1 codex review are archived (`../archive/followups-resolved.md`,
"Resolved by Plan B Epic 5b"), and that review's remaining bullets — 0009
fallback Engine API, 0016 multi-engine GPU memory, error-variant enumeration —
were re-routed to `plan-c.md`, since all three are gated on multi-engine or
CUDA-fallback work. What is left below is **not Plan-B scope by decision**, not
by omission: two bake-dependent items and one fixture-dependent one, the
standing architecture-doc drift obligation, and one entry filed by Epic 5b
itself that needs its own change rather than a rider.

---

### `cleanup_after_success` removes the attempt dir while the store mutex is held

**Found in:** Epic 5b Task 09 fix round (2026-07-30), while classifying blocking
IO for [ADR-0047](../decisions/0047-blocking-io-on-the-worker-hot-path-runs-on-spawn-blocking-inline-only-when-nothing-can-be-starved.md).
**Disposition:** Not a defect and not a starvation risk — the removal is
class (b) under 0047 (an attempt dir is bounded by construction: depth 1,
contents written by exactly one yt-dlp invocation), and 0047's table records the
reasoning. What is left is needless lock-hold time: the `remove_dir_all` runs
inside the critical section that only needs to cover the DB acknowledgement, so
every other worker's `claim_next` waits behind an unlink that has nothing to do
with the store. Deliberately **not** folded into Epic 5b — moving the removal
outside the lock touches the ordering half of
[ADR-0008](../decisions/0008-artifacts-are-durable-on-disk-before-mark-succeeded.md)
(removal must still follow the `mark_succeeded` commit, and `StaleAfterSuccess`
must still keep its directory), so it needs its own change with its own
argument, not a rider on a hygiene sweep.
**Trigger to revisit:** the next epic that touches the artifact-write / mark
ordering, or contention evidence — a `claim_next` latency profile showing waits
attributable to the post-commit unlink.

`cleanup_after_success` (`src/pipeline/mod.rs`) is reached from
`mark_after_artifacts`, which is sync and holds the store mutex per ADR-0008's
ordering contract. The unlink is therefore inside the lock. The fix is not to
wrap it in `spawn_blocking` at the same point — that would have to detach the
task, trading a bounded inline unlink for an unobservable one racing the next
`acquire` (0047 rejects exactly this) — but to move the removal *after* the
lock is released, which is a change to where the ordering contract's boundary
sits.

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

### Architecture doc-set drift detection

**Found in:** T08 (cross-cutting additions) — architecture doc set plan (2026-05-20).
**Disposition:** Standing maintenance concern. The architecture doc set (`docs/reference/architecture/`) was written against post-Epic-2-close `main` and carries NO in-flight stamps (dropped by operator approval — the set was written post-close). No current-epic action required.
**Trigger to revisit:** At each future epic's planning time, check whether the epic touches an architecture-doc-covered surface and add a "revise `docs/reference/architecture/<file>.md`" task (per `index.md` §6 drift-detection).

Known forward-touch points:

- **Epic 3 (failure-classification taxonomy)** will reshape `state-machine.md` and `orchestration.md`: typed `RetryableKind`/`UnavailableReason`/`ClassifiedFailure`, terminal-failure routing, and the `failed_retryable` retry path. Revise both deepdives and update `index.md` §4 (ADR map) for any ADRs added.
- **Epic 4 (operator status subcommand)** will reshape `orchestration.md` §Batch validation contract: the ADR 0017 done-predicate is currently represented only by `compute_process_stats`; the Epic 4 `status` subcommand expands it. Revise `orchestration.md` and update `index.md` §4 accordingly.

**Discharged instances** (the entry itself is standing maintenance and is never
archived — only its per-epic instances close):

- **Epic 5b (Plan B close-out), discharged 2026-07-30.** All four deepdives plus
  `index.md` revised: `index.md` §4 gained ADR 0045/0046/0047 rows and the 0002
  amendment note, §6's in-flight-stamp paragraph re-attributed to Epic 5b;
  `data-input.md` gained per-acquire attempt directories, exactly-one-WAV
  discovery, the `.work` sweep and the 0047 row; `state-machine.md` gained the
  `updated_at` lifecycle-mutation contract, `requeue_failures` +
  `operator_requeued` + the ADR-0046 row, and the `upsert_metadata_raw` claim
  guard; `orchestration.md` gained the startup `.work` sweep and the 0047 row;
  `transcription.md`'s GPU-verification section was rewritten from
  "scaffolded, not yet wired" to the shipped assertion. Every `src/main.rs:N`
  citation across the set was repointed to `src/commands.rs` after the thin-bin
  restructure moved those call sites.
