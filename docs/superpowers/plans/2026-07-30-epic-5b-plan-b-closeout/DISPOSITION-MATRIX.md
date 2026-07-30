# Epic 5b Phase 0 — FOLLOWUPS disposition matrix

Built by Task 01 (2026-07-30, branch `feat/epic-5b-plan-b-closeout`, docs-only).
Every active FOLLOWUPS entry in the **Epic 5** and **cross-epic** groups of
`docs/FOLLOWUPS.md` gets exactly one row with a terminal disposition. Tasks
09–13 execute the rows; Task 13 records final state and drives the ADR-0020
lifecycle (archive with resolving SHA / re-route / leave out-of-scope).

**Disposition vocabulary** (per the plan's Task-01 binding semantics):
`fix (Task NN)` · `verify-and-archive (Task 13)` · `accepted-archive (Task 13)` ·
`re-route Plan C` · `out of Plan-B scope` · `archive-integrity check (Task 13)` ·
`superseded sketch item`.

## Completeness cross-check (Step 2)

| Source | Count |
|---|---|
| `grep -c '^- ' docs/FOLLOWUPS.md` (whole file, all groups) | 54 |
| Epic 5 group `^- ` lines | 22 = **21 entries** + 1 "Full Epic 5 entries" pointer |
| Cross-epic group `^- ` lines | 7 = **6 entries** + 1 "Full cross-epic entries" pointer |
| `grep -c '^### ' docs/followups/epic-5.md` | 21 — 1:1 with the Epic 5 index lines, no orphans either way |
| `grep -c '^### ' docs/followups/cross-epic.md` | 6 — 1:1 with the cross-epic index lines |
| **Matrix entry rows (E1–E21 + X1–X6)** | **27** — one per index entry, matches 21 + 6 |
| Non-entry rows (A1–A3 archive-integrity, S1 superseded sketch) | 4 |
| **Total rows** | **31** |

The four non-entry rows have no scope-index line by design: A1–A3 are already
archived (integrity check only, never re-archived) and S1 exists only in
planning sketches.

## OPERATOR-DECIDES rows

Five rows carry judgment calls that must not be silently resolved. The
controller presents them as one batched question before Phase 1 and records
the rulings in this file: **E12** (`run_serial` retire-or-keep), **E15**
(closed-reply logging identity), **E18** (tmp-sweep TOCTOU accept-vs-keep),
**E19** (`upsert_metadata_raw` claim-guard resolve-vs-Plan C), **E20 items 1–2**
(ingest-ledger basename key + 1s fingerprint resolution). Rulings go in the
**Ruling** column below; until then it reads `PENDING`.

---

## Epic 5 group

| # | Entry (index line) | Body ref | Disposition | Executing task | Notes |
|---|---|---|---|---|---|
| E1 | T7: `Store::pragma_string` `pub` vs `pub(crate)` | epic-5.md §`Store::pragma_string` visibility | fix | Task 10 (Task 04 may narrow it first as part of the visibility sweep) | Lower to `pub(crate)`; integration test reaches it via `test-helpers` (ADR-0005). Blocked-on-0002 clause is discharged by the Phase-1 thin-bin decision. |
| E2 | T7: `Store::read_meta` `OptionalExtension` | epic-5.md §`Store::read_meta` could use `OptionalExtension::optional()` | fix | Task 10 | Pure refactor; `map_or_else` → `query_row(...).optional()`. |
| E3 | T8: `output::cleanup_tmp_files` polish | epic-5.md §`output::cleanup_tmp_files` minor cleanups | fix | Task 09 | Item 1 (inner `read_dir`/`entry?` path context) only — item 2 already struck through, resolved by `964e9c2`. Body's 2026-07-29 triage rider (`mark_after_artifacts` sync rusqlite in an async fn) is in-scope for Task 09's sync-IO classification. |
| E4 | T8: `output::shard_distributes_uniformly` rationale | epic-5.md §`output::shard_distributes_uniformly` test rationale is reversed | fix | Task 09 | Comment refresh (reversed variance claim); tightening the bound is optional per the body. |
| E5 | T9: `videos.updated_at` frozen by `upsert_video` | epic-5.md §`videos.updated_at` is frozen at first-seen | fix | Task 10 | **Pre-ruled (spec §Phase 0, operator):** document `updated_at` as **lifecycle-mutation time; a no-op ingest is clock-neutral**. `INSERT OR IGNORE` stays; no rename. Task 10 verifies lifecycle mutators bump it and pins re-ingest neutrality with a test. |
| E6 | T9/T10: `Store::conn`/`conn_mut` accessor hygiene | epic-5.md §`Store::conn` / `Store::conn_mut` accessor hygiene after T10 | fix | Task 10 | Delete `conn_mut` + its allow; refresh `conn`'s comment (it gained a real consumer in 5a). |
| E7 | T13: `ingest::walk_recursive` polish | epic-5.md §`ingest::walk_recursive` minor polish | fix | Task 09 | Item 1 (missing-inbox `Ok(())` → `bail!` at the `ingest()` root) and the surviving inner `entry?` half of item 2. |
| E8 | T15: `output::shard_dir` unused | epic-5.md §`output::shard_dir` is unused | fix | Task 09 | Delete the fn, its unit test's dependence, and its `#[allow(dead_code)]` (amended ADR-0002). |
| E9 | T11 (split at Epic 3 close): `YtDlpFetcher::acquire` coupling to `{video_id}.wav` | epic-5.md §`YtDlpFetcher::acquire` tight coupling | fix | Task 07 | Superseded in shape by the spec's stronger contract: fresh per-acquire dir + **exactly-one-WAV** discovery (0/1/>1 cases) — never a stdout-reported path. Finding 4 of the original entry is already in the Plan C group; findings 1–2 are archived (see A2's section). |
| E10 | Epic 3 final review: test-hardening bundle (signal capture, classifier precedence/case, kind-string e2e) | epic-5.md §Epic 3 close: test-hardening bundle | fix | Task 12 | All three items; each new test must be shown to fail when its subject is broken. |
| E11 | Epic 3 final review: `state/mod.rs` hygiene bundle (post-4a sweep mutators) | epic-5.md §`state/mod.rs` hygiene bundle | fix | Task 10 | Four items: bare `tx.commit()?` context, `attempt_count == 2` cycle assertion, defensive `claimed_by`/`claimed_at` clear, `kept_capped` no-event assertion. Claim/status semantics untouched (ADR-0023/0024). |
| E12 | Epic 3 final review: `run_serial` fetch/transcribe downcast asymmetry | epic-5.md §`run_serial` fetch/transcribe downcast asymmetry | **OPERATOR-DECIDES** — retire-or-keep. Ruling: **PENDING** | Task 13 (wording only) | `run_serial` is **retained by Phase 1 regardless** (spec: deleting it during a behavior-preserving phase changes API/test scope). No task in this plan deletes `run_serial` or fixes the asymmetry, so the reachable terminal states are: **(a) keep** → `accepted-archive (Task 13)`, tripwire comment stays; **(b) keep + fix** → needs a rider on Task 10 or 12 (unscheduled today); **(c) retire** → needs an unscheduled deletion commit, then archive-as-moot. Decision affects only how Task 13 words the archive entry unless (b)/(c) is chosen. |
| E13 | Epic 3 final review: `FetchOpts` derived `Debug` doesn't redact `cookies_file` | epic-5.md §`FetchOpts`'s derived `Debug` does not redact `cookies_file` | fix | Task 07 | Hand-rolled `Debug` (or field skip) redacting to `[COOKIES-REDACTED]`. |
| E14 | Epic 3 final review: `scrub_cookie_path` empty-path guard | epic-5.md §`scrub_cookie_path` has no guard against an empty cookie path | fix | Task 07 | One-line early return; test `scrub_cookie_path("")` is a no-op. |
| E15 | T5-Epic1: worker-side closed-reply path silently swallows error | epic-5.md §Worker-side closed-reply path silently swallows the error | **OPERATOR-DECIDES** — log-with-id (thread an id) vs log-without-id. Ruling: **PENDING** | Task 11 | **Code finding (verified 2026-07-30, this task):** the `let _ = req.reply.send(...)` sites are in `src/transcribe.rs` — **seven, not the six the body names**: the six error replies at 508/527/701/722/725/765 plus the **success** reply at **:770**, which the body omits and which drops a completed `TranscribeOutput` just as silently. All operate on `TranscribeRequest` (`src/transcribe.rs:284`), whose fields are `samples`, `config: PerCallConfig`, `cancel`, `deadline`, `reply` — **no video id, no request id**. `PerCallConfig` (:275) carries only `language` + `compute_lang_probs`. The id exists one layer up: `pipelined.rs`'s transcribe-channel work item **`FetchedItem` (`src/pipeline/pipelined.rs:179`) does carry `claim: Claim`** (hence `claim.video_id`), but `Transcriber::transcribe(samples, config, timeout)` and `WhisperEngineHandle`'s `mpsc::Sender<TranscribeRequest>` drop it. So logging a video/request id at the swallow site requires adding a field to `TranscribeRequest` (and a way to populate it — trait-signature change, a `PerCallConfig` field, or a worker-generated request seq). Log-without-id (elapsed wallclock + a request counter local to the worker) is the no-API-change option. |
| E16 | Epic 4b final review: status polish + test-debt bundle | epic-5.md §Epic 4b final review: status polish + test-debt bundle | fix | Task 11 | All five items (event-detail fallback, fixtures, `run_verify` `e.ok()` miscount, `--respondent-id` typo must error, mid-file `use`). |
| E17 | Epic 4c operator review: `main.rs` re-declares the library's entire module tree | epic-5.md §`main.rs` re-declares the library's entire module tree | fix | Tasks 02→03→04 | The whole of Phase 1: ADR (02), unification + façade + `lto="thin"` (03), visibility narrowing + dead-code-allow purge (04). Archived at Task 13 against the Phase-1 SHAs. |
| E18 | Epic 5a T01 review: tmp sweep's mtime-read → `remove_file` TOCTOU window | epic-5.md §Tmp sweep's age guard has an inherent TOCTOU window | **OPERATOR-DECIDES** — `accepted-archive (Task 13)` vs keep active. Ruling: **PENDING** | Task 13 | Body already records it as an accepted plan-level tradeoff with an evidence-gated trigger ("only if a run abort is ever traced to a swept tmp whose writer was live"). Archiving it as accepted removes the standing tripwire; keeping it active leaves a non-Plan-B-scope entry in the Epic 5 group, which the DoD requires to be empty or re-routed — so "keep" implies re-route (Plan C or production-run group), not "do nothing". |
| E19 | Epic 4c T03 review: `upsert_metadata_raw` is not claim-guarded | epic-5.md §`upsert_metadata_raw` is not claim-guarded | **OPERATOR-DECIDES** — resolve (add the guard) vs `re-route Plan C`. Ruling: **PENDING** | Task 10 if "resolve"; Task 13 re-routes if "Plan C" | Accepted last-write-wins today (ADR-0042); blast radius is metadata snapshot staleness only, self-healing on the next successful fetch. Cost of resolving: thread `worker_id` into a call site whose point is that it runs unconditionally on both the success and failure paths. |
| E20 | PR #23 review: ingest file-ledger hardening bundle | epic-5.md §Ingest file-ledger hardening bundle | Items 1–2: **OPERATOR-DECIDES** (design choices). Item 3: `fix` | Task 09 | **Item 1 — basename-only ledger key** (`ingested_files.file_name`): `walk_json_files` recurses, so same-named files in different subdirs collide; today's inbox is flat and filenames embed participant+key. Options: keep basename (accepted-archive), or key on an inbox-relative path (schema/migration + host-portability tradeoff the basename choice was made for). Ruling: **PENDING**. **Item 2 — (size, mtime) is a 1s-resolution change detector**: a same-size rewrite inside one mtime tick is invisible; row-level upserts remain the correctness backstop, so the cost is freshness only. Options: accept + document (accepted-archive), or strengthen the fingerprint (content hash / nanosecond mtime) at ingest-scan cost. Ruling: **PENDING**. **Item 3 — missing mid-tx rollback regression test:** `fix (Task 09)` unconditionally. |
| E21 | v0.3.1 review: CLI test-hardening bundle | epic-5.md §`global = true` both-position CLI test asserts parse acceptance only (entry covers all four items) | fix | Task 12 | Value-propagation + duplicate-precedence assertions; `backfill-metadata --dry-run` `conflicts_with = "limit"` (the one production change); dry-run PATH shim; `statuses()` snapshot widened to claim/attempt columns. |

## Cross-epic group

| # | Entry (index line) | Body ref | Disposition | Executing task | Notes |
|---|---|---|---|---|---|
| X1 | T1-Epic1: codex code-quality review deferred ADR refinements (0009/0011/0013/0016/0017 + error variants) | cross-epic.md §T1 codex code-quality review — deferred ADR refinements | **Split.** 0013 global-log-callback invariant: `fix (Task 08)`. 0011 + 0017: already struck through in the body (resolved 2026-07-29 via ADR-0041) — no action. 0009 fallback-Engine-API, 0016 multi-engine GPU memory, error-variant enumeration: `re-route Plan C` | Task 08 (0013 half); Task 13 (split + re-route) | The 0013 bullet is binding on Task 08 (install the callback once before any context init; one global bridge; never replaced per engine; init capture phase-scoped or synchronized). The three Plan-C bullets are all gated on multi-engine / CUDA-fallback work that Plan B does not do. Task 13 archives the 0013 half with Task 08's SHA and moves the remainder into the Plan C group with that rationale. |
| X2 | T9-Epic1: integration test only exercises empty-segment path on silence fixture | cross-epic.md §T9 integration test only exercises empty-segment path | `out of Plan-B scope` | — (entry stays active) | Blocked on a CC0 spoken-English fixture landing in `tests/fixtures/audio/`; no Plan-B task adds one. Task 13 leaves it in the cross-epic group and states in the DoD count that it is not Plan-B scope. |
| X3 | T13-Epic1: 0013 backend assertion must be `cfg(feature="cuda")`-gated | cross-epic.md §0013 backend assertion must be cfg(feature = "cuda")-gated | fix | Task 08 | The 2026-05-18 audit verdict ("NOT confirmed against shipped code") is the reason Task 08 is an implementation task rather than an audit. `EXPECTED_BACKEND` cfg-gate per the body; softening the assertion to a warning is rejected by the ADR. |
| X4 | T7-Epic1: revisit `SamplingStrategy::Greedy { best_of }` after T13 bake | cross-epic.md §Revisit `SamplingStrategy::Greedy { best_of }` | `out of Plan-B scope` | — (entry stays active) | Bake-data-dependent tuning; no Epic 5b task runs a quality bake. See also `docs/bake-findings.md`. |
| X5 | T8-Epic1: diagnostic log when `lang_detect`'s top id disagrees with primary inference | cross-epic.md §Diagnostic log when lang_detect's top id disagrees | `out of Plan-B scope` | — (entry stays active) | Bake-time diagnostic; Task 08 touches engine init but not the `lang_detect` path, and folding it in would exceed that task's stated scope. |
| X6 | T08-arch-docs: architecture doc-set drift detection | cross-epic.md §Architecture doc-set drift detection | `out of Plan-B scope` (standing maintenance — never archived) | Task 13 discharges **this epic's** instance | Task 13's docs pass revises `index.md` §4/§6 plus the state-machine / orchestration / data-input deepdives for the thin-bin shape, `requeue-failures`, and the attempt-dir lifecycle. The entry itself is a standing obligation and stays in the cross-epic group by design. |

## Archive-integrity check rows (never re-archive)

These three are **already resolved and already archived**. Task 13 verifies the
shipped code still matches the archived resolution and records
`verified <date>, matches` here. A mismatch becomes a **NEW active FOLLOWUPS
entry** (and Task 13 reports DONE_WITH_CONCERNS) — never a second resolution
appended to the archive.

| # | Claim | Archive location (verified by Task 01) | Disposition | Executing task | Notes |
|---|---|---|---|---|---|
| A1 | Multi-fetcher provenance (fetcher / transcript_source no longer hardcoded) | `docs/archive/followups-resolved.md` §"Resolved by Plan B Epic 1" (:11) → `### pipeline_fakes test gaps: transcribed_at RFC 3339, wav cleanup, re-run idempotence` (:56); resolution text at **:59** cites Epic 1 **T11** asserting `model`, `transcript_source`, `fetcher` + the EPIC-5-SKETCH map lines "Pipeline hardcodes fetcher/transcript_source (T14)" / "pipeline_fakes doesn't verify .json (T14)" both marked Resolved by Plan B Epic 1. **Claim verified 2026-07-30.** | `archive-integrity check` | Task 13 | Check surface: `VideoFetcher::name` (`src/fetcher/mod.rs:96`, impl :308), `Transcriber::name` (`src/transcribe.rs:942+`), and the pipelined item stamping that writes them into the artifact JSON. No active scope-index line — correctly absent. |
| A2 | `From<RunError> for FetchError` no longer collapses Spawn and Io into `NetworkError` | Same file, §"Resolved by Plan B Epic 3" (:531) → entry at **:539**, resolution `9974d69`. **Claim verified 2026-07-30.** | `archive-integrity check` | Task 13 | Check `RunError::Spawn → FetchError::ToolNotFound`, `RunError::Io → FetchError::SystemIo`, `Timeout → ToolTimeout`, and the ADR-0033 routing (ToolNotFound → Bug, SystemIo → Retryable). The same commit archived the `YtDlpFetcher::acquire` findings 1–2 (:573) — E9 is the surviving finding 3. |
| A3 | `--whisper-model` (and the other 10 `GlobalArgs` flags) accepted on either side of the subcommand | Same file, §"Resolved by the metadata-backfill branch / v0.3.1" (:819) → entry at **:824**, resolution `7dfa771`. **Claim verified 2026-07-30.** | `archive-integrity check` | Task 13 | Check that every `GlobalArgs` field still carries `global = true` (11 total incl. `compute_lang_probs`) and that the clap-definition consistency test + `tests/cli.rs` both-position test survive Phase 1's `Cli` visibility narrowing. E21 hardens the same test — a Task 12 change here must not weaken it. |

## Superseded sketch item

| # | Item | Source | Disposition | Executing task | Notes |
|---|---|---|---|---|---|
| S1 | `reset-stale-claims` operator subcommand | Planning sketches only (`docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md`); **no active FOLLOWUPS entry and nothing in the archive** — confirmed by grep across `docs/FOLLOWUPS.md`, `docs/followups/`, `docs/archive/followups-resolved.md` (zero hits) | **superseded sketch item** | — (nothing to archive) | Operator ruling 2026-07-30: superseded by the startup stale-claim sweep, which recovers claims at every process start with per-row `swept_stale` forensics since 5a. Recorded here so the epic's DoD does not read as a silent drop. Same grep shows `requeue-retryables` (the sketch's earlier name for `requeue-failures`) also has no FOLLOWUPS entry. |

---

## Task 13 close-out ledger

Task 13 fills this in from the branch's `git log --oneline`; it is the
mechanical DoD check for "every Plan-B-scope entry terminal".

| Bucket | Rows | Expected end state |
|---|---|---|
| `fix` (Tasks 02–12) | E1–E11, E13, E14, E16, E17, E20 item 3, E21, X1 (0013 half), X3 | Archived in `docs/archive/followups-resolved.md` with the resolving task SHA; index lines removed |
| OPERATOR-DECIDES | E12, E15, E18, E19, E20 items 1–2 | Terminal per the recorded ruling (fix + archive, accepted-archive, or re-route) |
| `re-route Plan C` | X1 (0009 / 0016 / error-variant bullets), plus E19 if ruled Plan C | Body moved to `docs/followups/plan-c.md`, index line moved to the Plan C group with rationale |
| `out of Plan-B scope` | X2, X4, X5, X6 | Left active in the cross-epic group; stated explicitly in Task 13's commit body so "not empty" is a recorded decision, not an omission |
| `archive-integrity check` | A1, A2, A3 | `verified <date>, matches` recorded in the rows above; any mismatch filed as a NEW active entry |
| `superseded sketch item` | S1 | No archive action; the ruling stands recorded here |
