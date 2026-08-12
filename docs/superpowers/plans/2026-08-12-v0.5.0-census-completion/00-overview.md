# ddp-transcribe — v0.5.0: census-completion release

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` onward). Open only the task you're working on. Task files are self-contained; do NOT load other task files into a subagent's context.

**Goal:** Ship v0.5.0 — publication-recency claim order (schema v7),
claim-time canonical fetch-URL derivation (superseding the incident-2 SQL
rewrite), a mass-failure circuit breaker (default 50, exit code 4), and
transport observability in `params_json` — so the campaign VM can be
promoted off v0.3.0 per ADR-0043 and resume the remaining ~1.93M-video
census on the WAF-surviving fetch path.

**Architecture:** Spec: `docs/superpowers/specs/2026-08-12-census-completion-strategy-design.md`
(operator-approved 2026-08-12; its Decisions D1–D6 are binding over this
plan where they conflict). Evidence base:
`docs/operations/incident-2026-08-06-tiktok-tls-403.md` and
`docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md`.
Each feature lands ADR-first (per repo governance), then test-first code.
Ordering: ADR+schema+claim-order first (the DB migration gates everything),
then URL derivation (pipeline + backfill), then the breaker, then
observability, then close-out.

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4 (all existing). **No new
dependencies.**

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1 && cargo build --release`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **ADR authoring via `write-adr:write-lean-adr` / `adg` only** (`adg lean new --from-stdin`); pre-commit hook runs `adg lean index` + `check` — fix, never bypass. ADR numbers are assigned by `adg` at authoring time — never pre-assign in code or docs; task files refer to them as "the claim-order ADR" etc.
- **TDD** for all feature tasks per ADR-0003 (batch test-first for plan-prescribed code; watch each new test fail for the real reason).
- **Mutators return `Result<usize>`** (ADR-0006); stats counters input-side, verb-named (ADR-0007).
- **Commit deviation disclosure** per ADR-0003.
- **No `Cargo.toml` version bump on the branch** — 0.4.0 → 0.5.0 happens in the post-merge tag commit (ADR-0043). The SRC catalog `pipeline_git_ref` moves only at promotion time, after the full gate and the operator's staged validation.
- **Binding invariants in scope:** 0008 (artifacts before mark_succeeded — untouched), 0025 (cancel/drain/shutdown order — the breaker reuses the token, never adds a second mechanism), 0026 (drain on claim_next None — breaker coexists, never polls), 0033/0037 (classification untouched), 0035 (cookie gate untouched — derivation must not alter `cookie_opts_for` semantics), 0038 (format selector untouched), 0042 (metadata capture untouched; `source_url` provenance never rewritten), 0046 (no attempt-count resets — operator ruling 2026-08-12), 0047 (any new blocking IO classified).
- **The incident-2 "remaining steps" SQL rewrite is superseded by this release and must never run** (spec D2).

## Dispatch conventions (ADR-0018 / ADR-0019 — binding on every task)

- **Subagent reports:** every dispatch brief requires a structured report,
  ≤250 words: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`. Full
  implementation transcripts never flow back to the controller.
- **Three-tier review per task:** (1) the implementer (brief-verbatim code
  + deviation honesty); (2) a Sonnet spec-compliance reviewer (mechanical
  does-this-match-the-brief + declared-ADR check) which (3) itself invokes
  codex-advisor (requesting ≤200-word replies) and distills to ≤300 words
  of actionable items. The orchestrator never calls codex-advisor directly
  during task reviews; it spot-checks
  `tail -200 "$(codex-advisor transcript)"` every 4–5 tasks.
- **Phase boundaries** (controller writes ≤1-page `PHASE-N-CLOSE.md`, ends
  its session; next phase starts fresh from spec + close-out):
  - **Phase 1 — ordering:** tasks 01–03 (ADR, schema v7, claim order)
  - **Phase 2 — transport:** tasks 04–06 (ADR, pipeline derivation, backfill)
  - **Phase 3 — safety & visibility:** tasks 07–09 (ADR, breaker, observability)
  - **Phase 4 — close-out:** task 10

## Ground truth (verified in code 2026-08-12, main @ af33fde / d8429b7)

Task files repeat the slices they need; this is the cross-task map.

- **Claim path:** `claim_next` `src/state/mod.rs:669` (IMMEDIATE tx; select
  678-682 `ORDER BY attempt_count ASC, first_seen_at ASC, video_id ASC`);
  `Claim { video_id, source_url, attempt_count, last_retryable_kind }`
  424-435. Pending index `idx_videos_pending_v3` `src/state/schema.rs:44-46`
  `(status, attempt_count, first_seen_at, video_id) WHERE status='pending'`.
- **Canonical:** `videos.canonical` set only at insert; production ingest
  passes `true` for every inserted row (`src/ingest.rs:407`,
  `upsert_video_tx(tx, &video_id, &entry.link, true)`); short links and
  invalid URLs are skipped at ingest. `CANONICAL_RE` `src/canonical.rs:23-28`
  requires `@[^/]+` (non-empty user) and `\d{19}`.
- **Migration:** `SCHEMA_VERSION = "6"` `src/state/schema.rs:1`; ladder is
  inline `if version == "N"` blocks in `run_migrate` `src/state/migrate.rs:17`
  (v5→v6 template at 127-142; tail bail 144-147). Tests
  `tests/state_migrate.rs` (`synthesize_v5_db:758`,
  `migrate_upgrades_v5_to_v6_idempotently:851`).
- **Pipeline composition:** `format_policy_for` `src/pipeline/mod.rs:305`;
  `cookie_opts_for` :334; **single production `fetcher.acquire` call site**
  inside `acquire_audio` `src/pipeline/mod.rs:374-384`
  (`fetcher.acquire(&claim.video_id, &claim.source_url, opts)`) — both
  serial (`fetch_and_decode` :487) and pipelined (`pipelined.rs:330`) paths
  flow through it. Artifacts: `TranscriptMetadata.source_url`
  `src/output/artifacts.rs:50`, written from `claim.source_url` at
  `src/pipeline/mod.rs:625`.
- **Pipelined orchestration:** cap atomic `claims_counter`
  `src/pipeline/pipelined.rs:982` (checked/bumped under the store lock
  279-301); failure dispatch: terminal `mark_terminal_failure` :420 →
  census gate :441-444; retryable `record_fetch_failure` :459 →
  `handle_record_fetch_failure_outcome` :470-478; transcribe-side
  `record_fetch_failure` :785 → :796; success `mark_after_artifacts`
  :690-700 (def `src/pipeline/mod.rs:676-685`). Supervision: `drop(tx)`
  :1058, `supervised_workers = 1 + download_workers` :1064, join loop
  :1068+, `token.cancel()` at :1070-1072 (checkpoint clean-drain), :1086
  (worker Err), :1094 (panic). `ProcessStats` literal :1113-1126.
- **batch_runs:** `params_json` built in `src/commands.rs:155-163`
  (`json!({retries, max_videos, cookies_present, download_workers,
  worker_host, checkpoint_cmd, checkpoint_every_secs})`);
  `open_batch_run` `src/state/mod.rs:1501`, `close_batch_run` :1518.
  `RunCensus` `src/batch.rs:35-54`, `From<&ProcessStats>` :56-72,
  `Display` :89-146. Census/exit ordering in Process arm:
  `close_batch_run` at `src/commands.rs:300-312`, `print!("{census}")`
  :313, `NoClaims` return :314-316. `CommandExit {Success=0, NoClaims=3,
  VerifyFailed=1}` :21-34; the library never calls `process::exit`.
- **CLI:** `Process` variant `src/cli.rs:150-183` (`max_videos`,
  `cookies_file`, `retries` (default 1, ranged), `checkpoint_cmd`,
  `checkpoint_every`); `download_workers` is a global (`src/cli.rs:88-94`).
- **Backfill:** `build_metadata_only_args(source_url)`
  `src/fetcher/ytdlp.rs:174-185`; call site `src/backfill.rs:82-89` with
  `&video.source_url` from `succeeded_missing_metadata_page` (:72).
- **Test infra:** `FakeFetcher` in the library
  (`src/fetcher/mod.rs:137-174`, 7 public fields, struct-literal
  construction in tests — **every field addition must update all
  literals**); builders `always_fails:196`, `fails_with_stderr:214`,
  `gated_then_always_fails:231`; canned-map miss and `always_fails` both
  yield retryable failures. `FakeTranscriber`
  `tests/pipeline_fakes/fakes.rs:17+`. Pipelined tests + full
  `ProcessOptions` literal shape: `tests/pipeline_fakes/pipelined_tests.rs`
  (`run_pipelined_honors_max_videos_cap:248`). Claim-order tests:
  `tests/state_claims.rs` (`claim_next_orders_by_first_seen_at:40` asserts
  the OLD ordering and is replaced by Task 03;
  `fresh_rows_claim_before_requeued_retries:757` must keep passing).
- **Version:** `Cargo.toml:3` = `0.4.0` (bump happens at tag time, 0043).

## Task index

| # | File | Delivers |
|---|------|----------|
| 01 | `01-claim-order-adr.md` | The claim-order ADR (D1 rationale as a governed decision) |
| 02 | `02-schema-v7-migration.md` | v6→v7 migration: recency claim index + 19-digit assertion |
| 03 | `03-claim-next-recency.md` | `claim_next` orders `attempt_count ASC, video_id DESC` |
| 04 | `04-fetch-url-adr.md` | The fetch-URL-derivation ADR (transport is code, provenance is data) |
| 05 | `05-fetch-url-derivation.md` | Claim-time canonical URL derivation in both pipeline paths |
| 06 | `06-backfill-url-derivation.md` | `backfill-metadata` uses the same derivation |
| 07 | `07-breaker-adr.md` | The circuit-breaker ADR (threshold 50, exit 4, DB-visible) |
| 08 | `08-breaker-impl.md` | Breaker: counter, trip→cancel→drain, census fields, CLI flag |
| 09 | `09-transport-observability.md` | `params_json` fetch-url form + yt-dlp env echo |
| 10 | `10-closeout.md` | FOLLOWUPS resolutions, runbook amendment, incident-2 supersession note, full gate |
