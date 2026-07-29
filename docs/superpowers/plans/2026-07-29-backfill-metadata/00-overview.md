# ddp-transcribe — backfill-metadata subcommand + v0.3.1 release

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` … `06-…md`). Open only the task you're working on. Task files are self-contained; do NOT load the kickoff prompt or other task files into a subagent's context.

**Goal:** Recover the 10,235-video metadata gap (rc1-era succeeded videos with no `video_metadata_raw` envelope) with a new `backfill-metadata` subcommand — metadata-only yt-dlp per video, no media, no GPU, never touching video status — and ship it as release v0.3.1 together with the `global = true` CLI rider (all 10 non-global `GlobalArgs` flags; operator decision 2026-07-29).

**Architecture:** A keyset-paginated cohort query (`succeeded` with no raw-metadata row, two-cached-statement shape per the Epic 4c loader precedent) feeds a serial best-effort loop in a new bin-only `src/backfill.rs`: per video, one metadata-only yt-dlp invocation (`--skip-download --no-simulate --print <METADATA_PRINT_TEMPLATE>`) through the existing `process::run` bounded-capture machinery (same 64 KB cap), wrapped by the existing envelope builder, written via a NEW `Store::insert_metadata_raw_if_missing` (`ON CONFLICT DO NOTHING` — backfill never overwrites a fetch-path envelope; codex-advisor design review 2026-07-29). `load-metadata` then fills the typed columns exactly as for fetch-time captures. Design agreed with the operator 2026-07-29 (kickoff goal 1).

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4, serde/serde_json (all existing). **No new dependencies.**

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **Clippy gates:** `unwrap_used` / `expect_used` **denied** in production code (allowed in tests via existing crate-root `cfg_attr` + per-test-file `#![allow(clippy::unwrap_used, clippy::expect_used)]` headers — new test files copy the header from an existing `tests/*.rs`).
- **Mutators return `Result<usize>`** per ADR-0006. Read-only queries live in `src/state/queries.rs` and return typed row structs.
- **Stats structs:** input-side counters, verb-named fields, per ADR-0007.
- **Backfill never touches video status/lifecycle** — metadata-only by construction (extends the Epic 4c invariant, ADR-0042). The only Store mutator it may call is `insert_metadata_raw_if_missing` (never the last-write-wins `upsert_metadata_raw`, which stays fetch-path-only). Review rejects any path where a backfill error (or success) changes a `videos.status`, claim column, or attempt count.
- **No cookies on backfill invocations, ever** (ADR-0035: cookies ride only `SensitiveLoginGated` retries; the backfill cohort is `succeeded` videos). Review rejects a `--cookies` arg or a cookies parameter on the metadata-only argv builder.
- **No subtitle flags** (`--write-subs`, `--write-auto-subs`, `--sub-langs`, `--list-subs`) on any yt-dlp argv, and the print template must not name `subtitles`/`automatic_captions` — ADR-0042, unit-asserted.
- **ADR authoring** via `write-adr:write-lean-adr` / `adg lean new --from-stdin` ONLY — never hand-edit `docs/decisions/`. Pre-commit hook runs `adg lean index --root .` + `adg lean check`; fix inconsistencies, never bypass.
- **New integration tests** here use only public API (binary via assert_cmd, `Store::open` + pub upserts, raw rusqlite) — auto-discovered, NO `[[test]]` Cargo.toml block per ADR-0005.
- **Dead-code hygiene** per ADR-0002; **commit deviation disclosure** per ADR-0003.
- The repo-root `ddp-run-export.sqlite` (untracked production snapshot) is NOT used by any test; if a check needs a real DB, use a scratch copy.

## Ground truth (verified in code, 2026-07-29)

- `Cargo.toml`: package `ddp-transcribe`, `version = "0.1.0"` (bumped to 0.3.1 only in the post-merge tag commit, per ADR-0043 and the production-run FOLLOWUPS entry).
- `videos` schema (v6, `src/state/schema.rs`): `video_id TEXT PK NOT NULL`, `source_url TEXT NOT NULL`, `status` CHECK-constrained; typed metadata columns nullable. `video_metadata_raw (video_id PK, fetched_at, raw_json)` with FK to `videos`.
- `Store::open` sets `PRAGMA journal_mode = WAL` + `busy_timeout = 5000` (`src/state/mod.rs:99-102`) — backfill is safe to run alongside a live `process` on the VM.
- `src/process.rs`: `run(CommandSpec) -> Result<CommandOutcome, RunError>`; `CommandSpec { program: &'static str, args: Vec<String>, timeout, stderr_capture_bytes, stdout_capture_bytes, redact_arg_indices }`; `CommandOutcome { exit_code, stdout: Option<Vec<u8>>, stderr_excerpt, signal, elapsed }`. `run` returns `Ok` on ANY exit code; only timeout/spawn/io lose stdout (those videos simply count as capture-failed and a re-run retries them).
- `src/fetcher/ytdlp.rs`: `pub(crate) const METADATA_PRINT_TEMPLATE` (~line 39); `fn build_metadata_envelope(stdout: Option<&[u8]>, capture_cap: usize) -> Option<String>` is **private** (~line 177) and `STDOUT_CAP` is **function-local** inside `acquire` (~line 227) — Task 02 lifts both. Envelope JSON: `{"schema":1,"printed":"<line>"}`, exactly two keys; `len >= cap` ⇒ `None` (head dropped).
- `Store::upsert_metadata_raw(&mut self, video_id: &str, envelope_json: &str) -> Result<usize>` (`src/state/mod.rs:587`) — INSERT … ON CONFLICT last-write-wins, `fetched_at = unix_now()` at call time. Doc contract: best-effort, never changes pipeline outcome.
- Keyset precedent: `Store::metadata_raw_page` (`src/state/queries.rs:171`) is **two cached statements** chosen by cursor — the single OR-NULL shape planned as O(n²) over 3M rows. The cohort query MUST copy the two-statement split.
- `metadata_loader` is **bin-only** (`mod` in `src/main.rs`, absent from `src/lib.rs`) — `backfill` follows the same precedent; integration tests drive the binary via assert_cmd.
- `#[tokio::main] async fn main`; `cfg.ytdlp_timeout` default 300 s (`src/config.rs:50`), env-overridable via profile machinery.
- `GlobalArgs` (`src/cli.rs:19-88`): 11 fields; only `compute_lang_probs` has `global = true`. The 10 without: `profile, state_db, inbox, transcripts, log_format, whisper_model, classification, stale_claim_threshold, download_workers, channel_capacity`. Operator decision 2026-07-29: rider adds `global = true` to **all 10**.
- Production cohort (2026-07-29 snapshot): 10,235 succeeded videos with no envelope; ~10K lightweight requests ≈ 2–4 h on the VM, runnable alongside `process`.
- Expect some cohort videos to have died since fetch (dead/blocked ⇒ nonzero exit) — best-effort per video is the contract, not an error.

## Task index

| # | File | Deliverable |
|---|------|-------------|
| 01 | `01-cohort-queries.md` | `Store::succeeded_missing_metadata_page` (two-statement keyset) + `count_succeeded_missing_metadata` + `MissingMetadataVideo` row struct + `Store::insert_metadata_raw_if_missing` mutator + integration tests + `EXPLAIN QUERY PLAN` check |
| 02 | `02-metadata-only-argv.md` | `STDOUT_CAP` hoisted to module scope; `build_metadata_envelope` → `pub(crate)`; new `build_metadata_only_args`; unit tests incl. ADR-0042 subtitle/cookie exclusions |
| 03 | `03-backfill-module.md` | Bin-only `src/backfill.rs`: `BackfillStats` (ADR-0007) + serial best-effort `backfill_metadata` loop with `--limit` support |
| 04 | `04-cli-wiring-integration.md` | `Command::BackfillMetadata { limit, dry_run }`, main dispatch arm (+ `log_resolved_config` arm), integration tests via a yt-dlp PATH shim + one `#[ignore]`d live test |
| 05 | `05-global-args-rider.md` | `global = true` on all 10 flags + both-position parse tests + `clap debug_assert` guard |
| 06 | `06-docs-adr-release.md` | ADR-0042 revision (backfill carve-out) via adg; stale 6–12 GB comment fix; runbook section; FOLLOWUPS resolutions; release-notes draft |

Dependency chain: 01 → 02 → 03 → 04 strictly sequential (03 needs 01's query and 02's builders; 04 needs 03's entry point). 05 is independent (any time). 06 last. Every task leaves the tree green.

## What this plan deliberately omits

- No Epic 5 hygiene beyond the rider (kickoff: mid-campaign churn without operational payoff is risk without reward).
- No cookie support, no lifecycle writes, no schema change, no new columns — `load-metadata` (existing, unchanged) fills the typed columns after backfill.
- No concurrency in the backfill loop: serial is deliberate (natural rate limiting toward TikTok; ~10K requests fit the 2–4 h budget). Review rejects "speed it up" worker pools.
- The VM-side execution (dry-run → `--limit 5` smoke → full run) is operator-driven per the runbook section Task 06 writes — not automated here.

## Release checklist (post-merge, operator + orchestrator; ADR-0043)

1. Merge the PR to `main`.
2. On `main`: bump `Cargo.toml` `version` to `0.3.1` (run `cargo build` so `Cargo.lock` follows) — this is the tag commit ("-V must finally mean something", production-run FOLLOWUPS).
3. `git tag -a v0.3.1` with release notes (Task 06 drafts them) → push commit + tag (HTTPS/gh, not SSH).
4. Bump catalog `pipeline_git_ref` to `v0.3.1`.
5. In-place upgrade on the VM per runbook (build + cp + `-h` check; delete-and-relaunch validation stays deferred to a natural pause per kickoff).
6. VM smoke: `ddp-transcribe backfill-metadata --dry-run` (expect ~10,235) → `--limit 5` live smoke → verify 5 new `video_metadata_raw` rows parse under `load-metadata --dry-run` → full run.

## Dispatch, review, and phase conventions (0018 / 0019)

- Implementers on Opus (operator preference, kickoff header). Fix-loop subagents on Sonnet.
- Every dispatch brief requires the structured report: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`, ≤250 words. Implementer test summaries state the TOTAL passed summed across all `test result:` lines.
- Three-tier review per task: implementer → Sonnet spec-compliance reviewer (whose brief includes the codex-advisor call, ≤200-word replies, distilled to ≤300 words) → codex-advisor reached ONLY through tier 2 (never by the orchestrator during task reviews).
- Single phase (6 tasks, no controller restart); final whole-branch review on the most capable model before merge.
- Branch: `feat/backfill-metadata` in a worktree per `superpowers:using-git-worktrees`.
