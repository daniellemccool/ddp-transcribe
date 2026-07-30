# ddp-transcribe — Epic 5b: Plan B close-out → release v0.4.0

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` … `13-…md`). Open only the task you're working on. Task files are self-contained; do NOT load other task files into a subagent's context.

**Goal:** Close Plan B: thin-bin/fat-lib restructure, the `requeue-failures` operator command, the 0013 backend assertion, fetch/attempt-dir hardening, the sync-IO policy, and every remaining Epic 5 / cross-epic FOLLOWUPS entry driven to a terminal state — released as v0.4.0 (development-only; the live campaign workspace stays on v0.3.0).

**Architecture:** Spec: `docs/superpowers/specs/2026-07-30-epic-5b-plan-b-closeout-design.md` (rev 4, advisor-reviewed ×3, operator-approved — its Semantics are binding over this plan where they conflict). Phase 0 (Task 01) builds the disposition matrix that makes "done" auditable; Phase 1 (02–04) restructures onto the final crate shape so nothing structural is touched twice; Phase 2 (05–08) lands the operator command and hardening on the new tree; Phase 3 (09–13) executes the sweeps and closes out.

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4 (all existing). **No new dependencies.**

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.** From Task 03 onward the gate also includes `cargo build --release` (thin-LTO must actually compile); Task 08 additionally gates `cargo build --release --features cuda`.
- **Clippy gates:** `unwrap_used`/`expect_used` denied in production code (tests allow via the existing per-file/scoped `#![allow]`s).
- **Mutators return `Result<usize>`** (ADR-0006); stats/counters input-side verb-named (ADR-0007).
- **ADR authoring via `write-adr:write-lean-adr` / `adg` only** (`adg lean new --from-stdin`); pre-commit hook runs `adg lean index` + `check` — fix, never bypass. ADR numbers are assigned by `adg` at authoring time — never pre-assign in code or docs.
- **TDD** for all feature/behavior tasks; Phase-1 restructure tasks are behavior-preserving and use suite-preservation evidence instead (documented census).
- **Commit deviation disclosure** per ADR-0003.
- No `Cargo.toml` version bump on the branch — 0.3.2 → 0.4.0 happens in the post-merge tag commit (ADR-0043). The SRC catalog `pipeline_git_ref` is NOT moved (campaign parked on v0.3.0).
- Claim/status semantics change ONLY where the spec explicitly says so (`requeue-failures`); ADR-0024 (as amended)/0023/0008 untouched; ADR-0036 amended only per Task 05.

## Ground truth (verified in code 2026-07-30, main @ 1c3c2c3, suite 345 passed / 0 failed / 10 ignored)

- **Crate shape:** `src/main.rs` declares 18 `mod`s (bin-only: `backfill`, `cli`, `config`, `metadata_loader`, `status`); `src/lib.rs` declares 13 `pub mod`s over the same files — every file compiles twice; the 84 library inline tests run twice (`cargo test --features test-helpers -- --list` → 355 listed = 345 runnable + 10 ignored). `Cargo.toml` has **no `[profile.release]` section**. 46 `#[allow(dead_code)]` total baseline (`rg 'allow\(dead_code\)' src/`).
- **Dispatch/exit:** `main()` reads `cli.global.log_format` for `init_tracing` before anything else; Process arm `std::process::exit(3)` when `stats.claimed == 0` (main.rs ~:298); Status arm `std::process::exit(1)` when `!report.verify.pause_safe` (~:386). `Cli`/`GlobalArgs`/`Command` fields currently `pub` (src/cli.rs:11+); `LogFormat` enum {Human, Json} at src/cli.rs:249 (no `Copy` today). Subcommands: Init, Ingest, Process, Migrate, Status, RecomputeWindow, LoadMetadata, BackfillMetadata.
- **Events vocabulary** (inline `INSERT INTO video_events` sites, src/state/mod.rs): `'claimed'` :523, `'succeeded'` :591, `'failed_retryable'` :759, `'retry_requeued'` :877, `'failed_terminal'` :947, `'swept_stale'` :1040, `'swept_terminal'` :1121, `'requeued'` :1162 (batch-sweep re-adjudication — administrative), plus `'cookie_parked'` via the shared record-failure helper (doc comment :780). `event_type` is open TEXT, no CHECK, no migration needed.
- **Failure-clock allowlist (operator ruling):** `'failed_retryable','failed_terminal','retry_requeued','cookie_parked'` — `'requeued'`, `'swept_stale'`, `'swept_terminal'`, `'claimed'`, `'succeeded'` never reset the clock.
- **State:** `videos` columns include `attempt_count`, `last_retryable_kind/message` (schema.rs:24-25), `terminal_reason/terminal_message` (:26-27); `UPSERT_VIDEO_SQL` is `INSERT OR IGNORE` (state/mod.rs:304, row-count contract per 0006 — `updated_at` untouched by this epic's requeue work). `Store::transaction()` (deferred) ~:259; `Store::transaction_immediate()` ~:275; `sweep_stale_claims` select-then-update-then-events pattern ~:965-1051 (the template for requeue's tx shape). `batch::run_sweep` at src/batch.rs:182. `hostname_or_default()` in src/main.rs (~:513) — Task 03 moves it into the lib.
- **Fetcher:** `YtDlpFetcher::acquire` creates persistent `work_dir/ytdlp-{video_id}` (ytdlp.rs:240); metadata capture stores the whole printed stdout line UNPARSED (~:201) — never add untagged stdout lines. `VideoFetcher::name` (fetcher/mod.rs:96), impl :308; `Transcriber::name` (transcribe.rs:942+). Success path removes only the WAV, not its dir (pipeline/mod.rs:594 comment: leftover is "disk churn").
- **0013:** `WhisperInitError::BackendMismatch` exists unconstructed (`src/transcribe.rs:303`, `#[allow(dead_code)]` at :292); no `whisper_log_set` bridge anywhere. Cross-epic FOLLOWUPS "0013 global log callback invariant" (docs/followups/cross-epic.md:27) binds: install once before any context init, one global bridge, never replace per engine, synchronized/phase-scoped capture.
- **Classification labels:** custom labels may contain commas (validation only rejects empty, src/classification.rs:103) — hence repeatable `--error-kind`, no comma-splitting.
- **FOLLOWUPS sources:** scope index `docs/FOLLOWUPS.md`; bodies `docs/followups/epic-5.md` (558 lines), `docs/followups/cross-epic.md`; archive `docs/archive/followups-resolved.md` (already contains: provenance via Epic 1 T11 ~:59; `From<RunError>` @9974d69 ~:539; `--whisper-model` @7dfa771 ~:824 — integrity-check only, never re-archive). `reset-stale-claims` has NO active entry (sketch-only; dropped by operator ruling 2026-07-30).
- **Retry arithmetic (binding, spec §requeue):** pre-requeue `attempt_count = A` → next claim bumps to `A+1` → ADR-0036 auto-requeues only while `attempt_count < retries + 1` ⇒ automatic follow-up requires **`--retries > A` strictly**.
- **Worktrees:** `feat/perf-tweaks` fully merged (removable); `plan-b-epic-4b` tip is an ancestor of main but holds untracked `ddp-run-export.sqlite` — disposition before removal.

## Task index

| # | File | Phase | Deliverable |
|---|------|-------|-------------|
| 01 | `01-disposition-matrix.md` | 0 | FOLLOWUPS disposition matrix (every Epic 5 + cross-epic row → terminal disposition; operator sign-off on judgment rows) |
| 02 | `02-crate-shape-adr.md` | 1 | New lean ADR (thin bin / fat lib + façade) + ADR-0002 amendment |
| 03 | `03-unification.md` | 1 | Single lib module root; `commands::dispatch` + `CommandExit`; thin main; `Cli` accessor; `lto="thin"`; 345→261 census evidence |
| 04 | `04-visibility-purge.md` | 1 | `pub(crate)` sweep + `unreachable_pub` warn + dead-code-allow purge |
| 05 | `05-requeue-adr.md` | 2 | Requeue-failures contract ADR + ADR-0036 amendment (carve-out + `--retries > A` arithmetic) |
| 06 | `06-requeue-failures-cmd.md` | 2 | `requeue-failures` subcommand (strict selector grammar, failure-clock CTE, `operator_requeued` events) |
| 07 | `07-fetch-hardening.md` | 2 | Fresh per-acquire dirs + exactly-one-WAV + attempt-dir lifecycle + `.work` age-gated sweep + redaction/guard fixes |
| 08 | `08-backend-assertion.md` | 2 | ADR-0013 backend assertion via `whisper_log_set` bridge + CUDA gate |
| 09 | `09-sync-io-policy.md` | 3 | Sync-IO audit + policy ADR + application (incl. walk_recursive/output polish, `shard_dir` deletion) |
| 10 | `10-state-hygiene.md` | 3 | state/mod.rs hygiene bundle + `updated_at` lifecycle-mutation documentation |
| 11 | `11-status-polish-test-debt.md` | 3 | Epic 4b status polish bundle + closed-reply logging fix (per matrix ruling) |
| 12 | `12-test-hardening-bundles.md` | 3 | Epic 3 + v0.3.1 CLI test-hardening bundles |
| 13 | `13-closeout-release.md` | 3 | Matrix execution/archive-integrity checks, FOLLOWUPS lifecycle, docs pass, RELEASE-NOTES-v0.4.0 |

**Ordering:** 01 first. 02 → 03 → 04 strictly sequential. 05 → 06. 07, 08 independent after 04. 09–12 independent after 04 (10 after 06 to avoid state/mod.rs churn conflicts). 13 last. Every task leaves the tree green.

## Dispatch, review, and phase conventions (0018 / 0019)

- Implementers on Opus (operator preference); fix-loop subagents on Sonnet.
- Reports: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`, ≤250 words; test totals summed across all `test result:` lines.
- Three-tier review per task: implementer → Sonnet spec-compliance reviewer (brief includes the codex-advisor call, ≤200-word replies, distilled ≤300 words) → codex-advisor only through tier 2.
- Single final whole-branch review on the most capable model before merge.
- Branch: `feat/epic-5b-plan-b-closeout` in a worktree per `superpowers:using-git-worktrees`.

## Release checklist (post-merge; ADR-0043)

Merge → bump `Cargo.toml` to 0.4.0 in the tag commit (+`Cargo.lock`) → `git tag -a v0.4.0` (notes from Task 13 draft) → push. **No catalog change, no VM deploy** — the campaign stays parked on v0.3.0; the paused older workspace is available if a live feedback run is ever needed.
