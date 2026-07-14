# Epic 4b — Phase 1 close (Tasks 01–04: timezone verdict + full status surface)

**Branch:** `feat/plan-b-epic-4b` (worktree `.claude/worktrees/plan-b-epic-4b`, branched from local main `8010423`).
**State:** Tasks 01–04 complete, each through the three-tier review with all Critical/Important findings fixed and re-reviewed. Suite: **264 tests green** (baseline 249), fmt/clippy clean throughout.

## What landed

| Task | Commits | Deliverable |
|---|---|---|
| 01 | `1a8bc49` | **ADR-0039** — timezone verdict: **"UTC-assumed (documentary evidence), empirically unresolved"** (operator spot-check couldn't discriminate at ±1h; ' UTC' suffix on May-2026 export is the documentary anchor; July-2026 real-donor exports are no-suffix). `parse_watched_at` comments corrected (no-suffix = production format). |
| 02 | `d9d8125`, `5ac0bfa` | `status` core: `src/state/queries.rs`, `src/status.rs`, `Command::Status{json}`. Counts, retryable-by-kind, claim ages, batch-run history (INTERRUPTED rendered honestly), `--json`. Fix loop added policy-provenance test assertions (both branches). |
| 03 | `730d887`, `2cfebfc`, `c6694eb` | Detail surfaces: `--video-id` (legible detail_json: kind/policy/new_kind/reason inline, message excerpted 200B char-safe), `--respondent-id`, `--errors`/`--retryable`. Fix loop made detail modes conflict at parse time (errors+retryable stay combinable). `ParkedRow` Serialize + attempt_count allow lifted. `batch::truncate_to_char_boundary` now pub(crate). |
| 04 | `9278539`, `7e5966d` | `status --verify` (0017 done-contract): per-shard read_dir batching, full TranscriptMetadata parse, pause-safe verdict, exit 1 on violation, `--verify` conflicts with detail modes. Fix loop: non-NotFound read_dir errors count `unreadable_artifacts` (infra fault), not `artifacts_missing` (data fault); NotFound keeps absent-tree honesty. |

## Ground-truth acceptance (v3 snapshot, verified in Tasks 02–04)

Against `ddp-run-export.sqlite` (worktree root, untracked, READ-ONLY): 56,620 total = 51,903 succeeded / 3,928 failed_terminal / 789 failed_retryable / 0 pending / 0 in_progress; kinds NoPermission 418, Fetch 301 (legacy annotation), FfprobePostprocess 36, NoVideoFormats 32, NoDataBlocks 1, HttpError 1; run 1 INTERRUPTED, run 2 closed with census; `preview` watch_events 64,931; `--verify` away-from-volume: 51,903 missing, exit 1.

## Adjudicated deviations Phase 2 must know

1. **Tracing logs go to stderr** (`init_tracing`, Task 02) so `--json` stdout is pure JSON. Do not revert; Task 07's echo tests already read both streams.
2. **Compiled-default policy TOML is 3,071 bytes** (not the plan prose's 3,065 — bytes vs chars on three em-dashes). Task 08's close doc must say 3,071 B.
3. tests/status.rs has **15 tests** (plan expected fewer; extras: compiled-default provenance, mode-conflicts, infra-fault taxonomy). It stays UNREGISTERED in Cargo.toml (auto-discovery).
4. Snapshot data property: 3 of the 789 retryable video IDs start with '6' (plan's `grep '^  7'` acceptance undercounts).

## Open Minor findings (for the final whole-branch review — none block Phase 2)

- ADR-0039:65 "~1h-or-more" vs evidenced "±1h"; pronoun style :63.
- `summarize_run` parses census without gating on finished_at (safe via close_batch_run's atomic write; noted as implicit invariant).
- `VerifyReport.unreadable_artifacts` doc comment is stale post-7e5966d (now also covers shard-level infra faults, not only unparseable .json).
- Per-entry `filter_map(|e| e.ok())` in run_verify silently drops exotic mid-iteration DirEntry errors (→ counted missing).
- status.rs: mid-file `use` block (Task 03); duplicate `dir.join(format!("{id}.json"))`; no `{"reason","message"}`-shape or malformed-detail test fixture; runs[2] positional index in one test; no respondent-id×errors conflict assertion; optional stdout flush before exit(1).

## Phase 2 (Tasks 05–08) — start here

- Read `00-overview.md` + this file; ledger at `.superpowers/sdd/progress.md`. **BASE for Task 05 = `7e5966d`.**
- Task 05 bumps SCHEMA_VERSION to "4" — after it lands, snapshot checks go through a migrated scratch COPY (never migrate the snapshot itself).
- Task 05/08 reference **ADR-0039** and the exact verdict string "UTC-assumed (documentary evidence), empirically unresolved"; the window ADR documents day-granularity absorption of the unresolved sub-day ambiguity.
- Process note: run fix-loop subagents on sonnet (haiku fixer ceremony proved slow).
