# ddp-transcribe — Epic 5a: campaign-safety slice → release v0.3.2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` … `05-…md`). Open only the task you're working on. Task files are self-contained; do NOT load other task files into a subagent's context.

**Goal:** Ship the four campaign-relevant fixes surfaced by the 2026-07-29 FOLLOWUPS triage — tmp-sweep age guard, a truly dry `ingest --dry-run`, two-writer instrumentation, and a periodic in-run checkpoint hook — as release v0.3.2, deliberately excluding all pure-hygiene Epic 5 work (that sweep gets its own plan when the campaign winds down).

**Architecture:** Four small, independent changes to a live-campaign codebase, each chosen for minimal blast radius (operator design decisions 2026-07-29): the tmp sweep gains an mtime age guard (threshold = the stale-claim threshold); ingest dry-run rolls back the existing per-file transaction instead of committing (perfect stat fidelity, no logic fork); the stale-claim sweep gains per-row `swept_stale` events plus a real hostname for worker attribution (making any future pending-count bump forensically explainable); and `process` gains `--checkpoint-cmd/--checkpoint-every` — a supervised periodic task invoking the operator's sync script through the bounded runner, failures warn-and-count, never abort.

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4 (all existing). **No new dependencies** (test mtime manipulation uses `std::fs::FileTimes`, stable since 1.75).

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **Clippy gates:** `unwrap_used`/`expect_used` denied in production code (tests allow via the existing per-file `#![allow(...)]` headers).
- **Mutators return `Result<usize>`** (ADR-0006); stats structs use input-side verb-named parallel counters (ADR-0007).
- **The campaign is LIVE.** Nothing in this slice may change claim/status semantics: ADR-0024 (sweep recovers blind — the new events are observability, not behavior), ADR-0023 (claim guards stay), ADR-0008 (artifacts-before-mark ordering untouched), ADR-0036 (retry decision stays in `record_fetch_failure`).
- **ADR-0021 (bounded subprocess capture) governs the checkpoint hook** — it must run through `process::run`'s bounded machinery, not a raw `tokio::process` spawn.
- **ADR-0025 (worker supervision):** the checkpoint task joins the existing JoinSet + CancellationToken protocol; it must return `Ok(())` on cancellation and NEVER return `Err` for a hook failure (an `Err` trips `token.cancel()` and kills the whole run).
- **Destroy-on-uncertainty is forbidden:** the tmp sweep skips (and warns) when mtime is unreadable — mirror of the 38463ca ingest principle.
- **ADR authoring** via `write-adr:write-lean-adr` / `adg` only; pre-commit hook runs `adg lean index` + `check` — fix, never bypass.
- **New integration tests** using only public API are auto-discovered (no Cargo.toml `[[test]]` block, ADR-0005); tests touching cfg-gated helpers need the `required-features` block.
- **Dead-code hygiene** per ADR-0002; **commit deviation disclosure** per ADR-0003.
- No `Cargo.toml` version bump on the branch — 0.3.1 → 0.3.2 happens in the post-merge tag commit (ADR-0043).

## Ground truth (verified in code 2026-07-29, main @ 837eb8a, suite 330 passed / 0 failed / 10 ignored)

- **Tmp sweep:** `cleanup_tmp_files(transcripts_root) -> Result<usize>` at `src/output/artifacts.rs:183-211`; deletes any depth-2 file whose name contains `.tmp`; sole caller `src/main.rs:88` (Process arm, pre-engine). Tmp names are `{file}.tmp-{pid}-{seq}` (`atomic_write`, artifacts.rs:132-173). Deleting a live sibling's tmp makes that sibling's `rename` fail → `write_artifacts_durable` errors → worker `Err` → `token.cancel()` → **whole batch aborts** (pipelined.rs:~600-624, ~855-887). Inline tests live in `src/output/artifacts.rs` `#[cfg(test)]` (e.g. `cleanup_tmp_files_counts_only_real_deletions` :264).
- **Ingest:** `ingest(inbox, store, window) -> Result<IngestStats>` at `src/ingest.rs:101`; per-file: ledger fingerprint read (:127, skip if `(size,mtime)` match) → `store.transaction()` (:154) → `upsert_video_tx`/`upsert_watch_history_tx`/`backfill_watch_raw_tx` (:282-300) → `upsert_ingested_file_tx` (:172, same tx) → `tx.commit()` (:176). Dropping a rusqlite `Transaction` without commit rolls back. Dispatch arm `src/main.rs:50-77`; the sham dry-run log is `src/main.rs:57-59`. `IngestStats` fields at `src/ingest.rs:16-51`. `in_window` computed per row (:284-287) per ADR-0040. Existing dry-run flag precedents: `recompute-window`, `load-metadata`, `backfill-metadata`.
- **Instrumentation:** `sweep_stale_claims` (`src/state/mod.rs:952-985`) is the ONLY status-changing mutator that emits **no per-row `video_events`** (aggregate `tracing::info!` only) — a pending-count bump is currently unexplainable from the DB. The eight existing event inserts are inline `INSERT INTO video_events` (state/mod.rs:497,565,733,814,851,921,1039,1080); `event_type` is open TEXT, **no CHECK constraint, no migration needed**. `hostname_or_default()` (`src/main.rs:488-490`) reads `$HOSTNAME` (usually unexported) → falls back to literal `"host"`, so both live instances report `worker_host:"host"` and worker attribution relies on pid ranges. `worker_id = {hostname}-{pid}` composed at main.rs:175.
- **Checkpoint slot:** `run_pipelined` (`src/pipeline/pipelined.rs:767+`) spawns transcribe worker (:811) + N fetch workers (:829-843) into a `JoinSet`, then `drop(tx)` (:848 — load-bearing per ADR-0025), then the supervision loop (:855-887, first `Err`/panic → `token.cancel()`). A periodic task slots as one more `join_set.spawn(...)` before the `drop(tx)`. No `tokio::time::interval` precedent exists in `src/` — this introduces it. Hop-1 sync is external (`~/sync-to-storage.sh`, flock-serialized, per `docs/operations/src-vm.md:29`). `CommandSpec.program` is `&'static str` (`src/process.rs:10`) — Task 04 widens it to `String` (mechanical; call sites: ytdlp.rs, backfill.rs via ytdlp args, transcribe? — `rg 'CommandSpec' src/` and update all).
- Process-scoped flag precedent: `cookies_file`/`retries`/`max_videos` are `Process` subcommand args (`src/cli.rs:141-158`) threaded through `ProcessOptions` (main.rs:174-187) — the checkpoint flags follow this, NOT GlobalArgs. `humantime::parse_duration` value-parser precedent: `--stale-claim-threshold`.
- The v0.3.1 tag exists but is **not yet deployed** (VM runs v0.3.0); the eventual in-place upgrade jumps 0.3.0 → 0.3.2 directly, and the catalog `pipeline_git_ref` can point straight at v0.3.2.

## Task index

| # | File | Deliverable |
|---|------|-------------|
| 01 | `01-tmp-sweep-age-guard.md` | `cleanup_tmp_files(root, older_than)` age guard; caller passes `cfg.stale_claim_threshold`; skip+warn on unreadable mtime; FileTimes tests |
| 02 | `02-ingest-dry-run.md` | `ingest(..., dry_run)` rollback-based dry-run; real stats, zero writes (ledger included); main arm honors the flag; integration tests |
| 03 | `03-two-writer-instrumentation.md` | Per-row `swept_stale` events from the stale sweep; real hostname (`/proc/sys/kernel/hostname` → `$HOSTNAME` → `"host"`); tests |
| 04 | `04-checkpoint-hook.md` | `process --checkpoint-cmd <path> --checkpoint-every <dur>`; supervised periodic task via bounded `process::run` (`CommandSpec.program` → `String`); warn-and-count failures; shim tests |
| 05 | `05-docs-adr-release.md` | Checkpoint ADR (via adg); runbook updates; FOLLOWUPS lifecycle; RELEASE-NOTES-v0.3.2 draft |

Tasks 01–04 are mutually independent (separate files/subsystems); 05 last. Every task leaves the tree green.

## Deliberately NOT in this slice

- All Epic 5 hygiene (bin/lib module tree, sync-IO sweep, `requeue-retryables`/`reset-stale-claims`, test-debt bundles, state/mod.rs hygiene) — planned separately when the campaign winds down.
- Any fix for the two-writer anomaly itself — ADR-0024/0036 semantics are untouched; Task 03 only makes the next occurrence *explainable* (verify-before-fix per the FOLLOWUPS discipline).
- Cookie work, window work, capacity write-up (held pending A/B + PI).

## Release checklist (post-merge; ADR-0043)

Merge → bump `Cargo.toml` to 0.3.2 in the tag commit (+`Cargo.lock`) → `git tag -a v0.3.2` (notes from Task 05 draft) → push → catalog `pipeline_git_ref` → v0.3.2 → VM in-place upgrade at the operator's natural pause (0.3.0 → 0.3.2 jump; verify `-V` prints 0.3.2, `-h` shows `backfill-metadata`) → smoke: `ingest --dry-run` (must write nothing), one checkpoint cycle with `--checkpoint-every 2m --checkpoint-cmd ~/sync-to-storage.sh`, then the standing v0.3.1 backfill smoke sequence from the runbook.

## Dispatch, review, and phase conventions (0018 / 0019)

- Implementers on Opus (operator preference); fix-loop subagents on Sonnet.
- Reports: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`, ≤250 words; test totals summed across all `test result:` lines.
- Three-tier review per task: implementer → Sonnet spec-compliance reviewer (brief includes the codex-advisor call, ≤200-word replies, distilled ≤300 words) → codex-advisor only through tier 2.
- Single phase; final whole-branch review on the most capable model before merge.
- Branch: `feat/epic-5a-campaign-safety` in a worktree per `superpowers:using-git-worktrees`.
