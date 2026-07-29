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
- T5-Epic1: Worker-side closed-reply path silently swallows error → Epic 2 (tracing context)
- T17: sync `write_artifacts_and_mark` inside `tokio::sync::Mutex` guard inside async fn can stall under `TOKIO_WORKER_THREADS=1` → Epic 2 close OR Epic 5 ops-hygiene
- Full Epic 2 entries: [followups/epic-2.md](followups/epic-2.md)

**Epic 3 (failure classification taxonomy)** — closed 2026-07-07. All ten entries resolved
(archived with resolving SHAs in [archive/followups-resolved.md](archive/followups-resolved.md),
section "Resolved by Plan B Epic 3") or split-and-re-filed: `YtDlpFetcher::acquire`
finding 3 → Epic 5, finding 4 → Plan C (see those groups below).

**Epic 5 (Plan A → Plan B cleanup sweep)**
- T7: `Store::pragma_string` `pub` vs `pub(crate)` → Epic 5 (lower to `pub(crate)`)
- T7: `Store::read_meta` `OptionalExtension` → Epic 5 (refactor when touched)
- T8: `output::cleanup_tmp_files` polish → Epic 5 (bundle with sync-IO sweep)
- T8: `output::shard_distributes_uniformly` rationale → Epic 5 (refactor comment when touched)
- T9: `videos.updated_at` frozen by `upsert_video` → Epic 5 (decision after Epic 2 ships)
- T9/T10: `Store::conn`/`conn_mut` accessor hygiene → Epic 5 (delete `conn_mut`; refresh comment)
- T13: `ingest::walk_recursive` polish → Epic 5 (bundle with sync-IO sweep)
- T15: `output::shard_dir` unused → Epic 5 (delete)
- T11 (split at Epic 3 close): `YtDlpFetcher::acquire` coupling to `{video_id}.wav` output filename → Epic 5 (fetch hardening)
- Epic 3 final review: test-hardening bundle (signal-capture spawn+kill test, `classify_message` precedence/case test, `transcribe_worker` kind-string end-to-end assertion) → Epic 5
- Epic 3 final review: `state/mod.rs` hygiene bundle — sweep mutators post-4a rename (bare `tx.commit()?`, attempt_count==2 assertion, defensive claimed_by/claimed_at clearing, sweep capped-requeue no-event assertion) → Epic 5
- Epic 3 final review: `run_serial` fetch/transcribe downcast asymmetry (see tripwire in `src/pipeline/serial.rs`; the bundle's discarded-count half resolved by Epic 4a T06, archived) → Epic 5 (or moot if `run_serial` retires)
- Epic 3 final review: `FetchOpts` derived `Debug` doesn't redact `cookies_file` → Epic 5
- Epic 3 final review: `scrub_cookie_path` empty-path guard → Epic 5
- Transcript-storage assessment: pipelined transcribe worker holds Store mutex across artifact writes+fsyncs though only `mark_succeeded` needs it (0008-ordering-sensitive; own reviewed change) → Epic 5 (perf sweep)
- Epic 4b final review: status polish + test-debt bundle (`render_event_detail_inline` non-string fallback, missing test fixtures, `run_verify` `e.ok()` miscount, `--respondent-id` typo silently zero-fills, mid-file `use` in status.rs) → Epic 5 hygiene bundle
- Epic 4b final review: `ingest --dry-run` is not dry — pre-existing wart, raised stakes now that `ingest` takes window flags → Epic 5
- Epic 4c operator review: `main.rs` re-declares the library's entire module tree (double compilation, broadened `pub` surface, a driver of the accumulated `dead_code` allows) → Epic 5 hygiene bundle (cites ADR-0002's deferred bin/lib reassessment)
- Epic 4c T05 review: startup `cleanup_tmp_files` sweep can delete a concurrent live process's in-flight tmp — pre-existing, multi-process deployments only → Epic 5 (bundle with the `cleanup_tmp_files` polish entry)
- Epic 4c T03 review: `upsert_metadata_raw` is not claim-guarded — a stale worker can overwrite a newer envelope (accepted last-write-wins tradeoff; snapshot staleness only, self-heals) → Epic 5
- PR #23 review: ingest file-ledger hardening bundle (basename-only key collision risk, 1s-resolution change detector, no mid-tx rollback test) → Epic 5 (ingest/sync-IO sweep)
- v0.3.1 review: CLI test-hardening bundle — `global = true` both-position test asserts parse acceptance only (no value-propagation or duplicate-precedence assertion); `backfill-metadata` `--dry-run`/`--limit` needs `conflicts_with`; dry-run test needs a PATH shim; `statuses()` snapshot needs claim/attempt columns → Epic 5 (test-hardening bundle)
- Full Epic 5 entries: [followups/epic-5.md](followups/epic-5.md)

**Production run (operational milestones, not code epics)**
- Epic 4c close: capacity estimate for the production batch — 2,982,471 uniques, throughput / window narrowing / disk (`video_metadata_raw` ~3–6 GB, transient WAVs) → before the first non-pilot `process` batch
- Epic 4c close: `video_metadata_raw` prune / VACUUM decision — keep for re-parse, prune for export, or reclaim in place → after the first production batch's `load-metadata` completes
- v0.3.0 promotion: Cargo package version must track release tags (`-V` printed 0.1.0 on both rc1 and v0.3.0) → **resolution in flight: the v0.3.1 tag commit** (archive it there, not at branch merge)
- v0.3.1 backfill: cookie-gated metadata residue — hypothesis (unverified) that part of the cohort is now login-gated; carries two rejected argv-hardening candidates (`--ignore-no-formats-error`, `--` separator) → after the first full `backfill-metadata` run's stats
- Full production-run entries: [followups/production-run.md](followups/production-run.md)

**Plan C (short-link resolution, multi-engine, storage scale)**
- T5: `SHORT_LINK_RE` query parameters → Plan C (short-link resolution lands)
- T8: `output::shard` ASCII-only byte slice → Plan C (when `VideoId` newtype lands)
- T1-Epic1: Promote 0010's pass-through rule to a meta-process ADR → Plan C (if recurring pressure)
- T3-Epic1: `decode_wav` trusts float-format WAV sample values → Plan C (if alternate fetcher introduces float WAVs)
- T10-Epic1: Per-token text field doubles raw_signals payload → Plan C (compact JSON landed in perf-tweaks decdf6f; drop-text still deferred pending 0010 amendment)
- T11 (split at Epic 3 close): yt-dlp argv `--` separator before `source_url` → Plan C (when resolved URLs reach the fetcher)
- Epic 3 final review: `scrub_cookie_path` canonicalized/relative path-variant hardening → Plan C (multi-engine work)
- Transcript-storage assessment: DB-at-runtime transcript storage (schema v4 + export subcommand + sync redesign; own epic) only if the ADR-0004 ~1M-small-files ceiling approaches or SQL-queryable transcripts become a research need → Plan C (storage scale)
- Full Plan C entries: [followups/plan-c.md](followups/plan-c.md)

**Cross-epic / ADR maintenance / verify-then-archive**
- T1-Epic1: codex code-quality review deferred ADR refinements (0009/0011/0013/0016/0017 + error variants) → multi-epic (Epic 4, T6/T7, Plan C)
- T9-Epic1: integration test only exercises empty-segment path on silence fixture → unscoped (when spoken-English fixture lands)
- T13-Epic1: 0013 backend assertion must be `cfg(feature="cuda")`-gated → audited 2026-05-18, NOT confirmed; deferred to Epic 5 cleanup
- T7-Epic1: Revisit `SamplingStrategy::Greedy { best_of }` after T13 bake → unscoped tuning followup (see also `bake-findings.md`)
- T8-Epic1: Diagnostic log when `lang_detect`'s top id disagrees with primary inference → unscoped diagnostic (see also `bake-findings.md`)
- T08-arch-docs: architecture doc-set drift detection → standing maintenance (revise matching deepdive + index.md §4 at each epic's planning time if it touches a covered surface)
- Full cross-epic entries: [followups/cross-epic.md](followups/cross-epic.md)
