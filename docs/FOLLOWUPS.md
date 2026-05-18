# Followups — active-scope entries

Active-scope FOLLOWUPS entries scheduled for an upcoming Plan B epic (or
explicitly routed to Plan C). Each entry names the task or context where
the finding arose, the disposition, and the trigger that should re-surface
it. When an entry is resolved, move it (with the resolving commit SHA) to
`docs/archive/followups-resolved.md`; do not just delete.

Sibling files (off the orchestrator's planning-time reading path):

- `docs/cosmetic-followups.md` — items deferred indefinitely; touch when
  the surrounding file gets edited for unrelated reasons.
- `docs/bake-findings.md` — operational findings from bake runs; not
  code-quality FOLLOWUPS.
- `docs/archive/followups-resolved.md` — append-only history of resolved
  entries.

Per-epic entry bodies live in sibling files under `docs/followups/` and
are loaded only when an epic's task expansion needs them — see the
"Full entries" pointers in each scope-index group below.

**Discipline:** entries that record unverified hypotheses must prefix the
hypothesis with `**Hypothesis (unverified):**` so the next operator knows
to verify before acting (per 0020).

## Maintenance

- **Add an entry:** append the full body to the appropriate
  `docs/followups/<group>.md` file; add a one-line scope-index entry below
  pointing at it.
- **Modify:** edit the body in the sub-file. Update the scope-index line
  if the title or disposition changed.
- **Re-target** (e.g., Epic 3 → Epic 4): move the body between sub-files
  and update its scope-index line.
- **Resolve:** move the body to `docs/archive/followups-resolved.md` with
  the resolving commit SHA; remove its scope-index line.

---

## Scope index

Grouped by target epic; format `T<n>: <short title> → Epic <N> <task hint>`.
Routing is authoritative per `docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md`
lines 120-148.

**Epic 2 (concurrent fetch + state-machine)**
- T7: `Store::open` schema-version not read → Epic 2 first task (read-and-check policy)
- T10: `concurrent_claim_serializes_via_begin_immediate` doesn't race → Epic 2 (rewrite with Barrier)
- T10: `mark_succeeded` doesn't require `status='in_progress'` → Epic 2 (WHERE predicate)
- T10: `claim_next` polling semantics → Epic 2 (explicit sleep/backoff policy)
- T10: Missing round-trip test — succeeded not re-claimable → Epic 2 (add to state_claims tests)
- T5-Epic1: WhisperEngine teardown can hang once T7 lands real inference → Epic 2 (graceful shutdown)
- T5-Epic1: Worker-side closed-reply path silently swallows error → Epic 2 (tracing context)
- T11-Epic1: `Config::whisper_use_gpu`/`whisper_threads` unused → Epic 2 (config rationalization sweep)
- Full Epic 2 entries: [followups/epic-2.md](followups/epic-2.md)

**Epic 3 (failure classification taxonomy)**
- T6: `From<RunError> for FetchError` collapses Spawn/Io → Epic 3 (typed variants)
- T6: `status.code().unwrap_or(-1)` loses signal info → Epic 3 (add `signal` field)
- T10: `claim_next`/`mark_succeeded` lack `with_context` → Epic 3 (bundle with error restructure)
- T11: `YtDlpFetcher::acquire` error mapping → Epic 3 (classifier covers it)
- T5-Epic1: `From<AudioDecodeError> for TranscribeError` maps to Bug → Epic 3 (classification taxonomy)
- Full Epic 3 entries: [followups/epic-3.md](followups/epic-3.md)

**Epic 4 (operator-facing commands / timestamps)**
- T13: `parse_watched_at` UTC assumption → Epic 4 (0027 resolution path)
- Full Epic 4 entries: [followups/epic-4.md](followups/epic-4.md)

**Epic 5 (Plan A → Plan B cleanup sweep)**
- T7: `Store::pragma_string` `pub` vs `pub(crate)` → Epic 5 (lower to `pub(crate)`)
- T7: `Store::read_meta` `OptionalExtension` → Epic 5 (refactor when touched)
- T8: `output::cleanup_tmp_files` polish → Epic 5 (bundle with sync-IO sweep)
- T8: `output::shard_distributes_uniformly` rationale → Epic 5 (refactor comment when touched)
- T9: `videos.updated_at` frozen by `upsert_video` → Epic 5 (decision after Epic 2 ships)
- T9/T10: `Store::conn`/`conn_mut` accessor hygiene → Epic 5 (delete `conn_mut`; refresh comment)
- T13: `ingest::walk_recursive` polish → Epic 5 (bundle with sync-IO sweep)
- T15: `output::shard_dir` unused → Epic 5 (delete)
- SRC-bake: `--whisper-model` global flag rejected after subcommand → Epic 5 (one-line `global = true`)
- Full Epic 5 entries: [followups/epic-5.md](followups/epic-5.md)

**Plan C (short-link resolution, multi-engine, storage scale)**
- T5: `SHORT_LINK_RE` query parameters → Plan C (short-link resolution lands)
- T8: `output::shard` ASCII-only byte slice → Plan C (when `VideoId` newtype lands)
- T1-Epic1: Promote 0010's pass-through rule to a meta-process ADR → Plan C (if recurring pressure)
- T3-Epic1: `decode_wav` trusts float-format WAV sample values → Plan C (if alternate fetcher introduces float WAVs)
- T10-Epic1: Per-token text field doubles raw_signals payload → Plan C (compact JSON landed in perf-tweaks decdf6f; drop-text still deferred pending 0010 amendment)
- Full Plan C entries: [followups/plan-c.md](followups/plan-c.md)

**Cross-epic / ADR maintenance / verify-then-archive**
- T1-Epic1: codex code-quality review deferred ADR refinements (0009/0011/0013/0016/0017 + error variants) → multi-epic (Epic 4, T6/T7, Plan C)
- T9-Epic1: integration test only exercises empty-segment path on silence fixture → unscoped (when spoken-English fixture lands)
- T8-Epic1: `lang_probs` second `WhisperState` allocation guidance → verify Epic 1 implementation before archiving
- T13-Epic1: 0013 backend assertion must be `cfg(feature="cuda")`-gated → verify Epic 1 implementation before archiving
- T4-Epic1: T9 extraction must reject non-finite f32 from whisper-rs → verify Epic 1 implementation before archiving
- T7-Epic1: Revisit `SamplingStrategy::Greedy { best_of }` after T13 bake → unscoped tuning followup (see also `bake-findings.md`)
- T8-Epic1: Diagnostic log when `lang_detect`'s top id disagrees with primary inference → unscoped diagnostic (see also `bake-findings.md`)
- Full cross-epic entries: [followups/cross-epic.md](followups/cross-epic.md)
