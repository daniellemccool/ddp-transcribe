# ddp-transcribe — Plan B Epic 4a: In-Pipeline Retry, Config-Driven Classification, Triage Retirement

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` … `08-…md`). Open only the task you're working on. Task files are self-contained; do NOT load the spec or other task files into a subagent's context.

**Goal:** Retry becomes pipeline behavior: `process` sweeps parked failures at start, retries retryable failures at the end of its own queue under a lifetime attempt cap, writes off proven-dead classes inline, parks cookie-gated rows unless cookies are supplied, and records a durable policy-attributed census in a new `batch_runs` table. Classification of yt-dlp stderr becomes an operator-editable TOML table with an evidence-derived compiled-in default. The `triage` subcommand and oEmbed probe retire.

**Architecture:** A new `src/classification.rs` owns the policy table (ordered first-match rules, three dispositions: `retryable` / `terminal` / `requires-cookie`); `src/failure.rs` becomes a thin interpreter over it (structural errors stay code-mapped; label strings replace the message-class enums). A new `Store::record_fetch_failure` mutator makes the requeue-vs-exhaust-vs-park decision in one transaction at failure time; `claim_next` orders `attempt_count ASC` so retries drain after fresh work. `main`'s Process arm opens a `batch_runs` row, runs the sweep, drains via the existing pipelined orchestrator, and closes the row with the census.

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4, serde (existing). **One new dependency: `toml = "0.8"`** (approved in the design; Cargo-team-maintained). `curl` runtime dependency is REMOVED with the probe.

**Reference:** Design spec at `docs/superpowers/specs/2026-07-07-epic-4a-in-pipeline-retry-design.md`; operator rulings + census evidence in `docs/superpowers/plans/PLAN-B-EPIC-4-KICKOFF-PROMPT.md`. Subagents should not need either — task files are self-contained.

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **Clippy gates:** `unwrap_used` / `expect_used` are **denied** in production code (allowed in tests via existing crate-root `cfg_attr` + per-test-file `allow` headers — new test files copy the header from an existing `tests/*.rs`).
- **Mutators return `Result<usize>`** (row-change count) per ADR-0006/0023; the new `record_fetch_failure` returns a typed outcome and documents how 0006 is honored internally.
- **Stats structs**: input-side counters, verb-named fields, per ADR-0007.
- **Artifact-before-`mark_succeeded`** ordering (ADR-0008) is untouched by this epic; do not reorder.
- **New integration tests** in `Cargo.toml` need `[[test]] required-features = ["test-helpers"]` per ADR-0005.
- **Dead-code hygiene** per ADR-0002 (two-part convention: inline lift-point comment + "0002 dead-code note:" commit paragraph); every task states which allows it adds or lifts.
- **Commit disclosure** per ADR-0003: any deviation from the task brief is disclosed prominently in the commit message.
- **Label strings are load-bearing** and shared by Tasks 01/03/04/06/07: `IpBlockedMessage`, `VideoNotAvailable10231`, `VideoNotAvailable10240`, `NoDataBlocks`, `NoPermission`, `SensitiveLoginGated`, `NoVideoFormats`, `FfprobePostprocess`, `HttpError`, `NetworkTransient`, `ToolTimeout`, `YtDlpOther`, `TranscribeOther`. Exact spelling, bare variant style, no renames.
- **The two evidence facts that must survive in doc comments:** "Your IP address is blocked" is a yt-dlp misfire meaning VIDEO REMOVED (not an IP issue — ADR-0033 comment 2026-07-07); `NoPermission` is impure (25/452 alive) and must stay `retryable`.
- **Retry cap semantics:** cap compares **lifetime** `attempt_count` (already bumped at claim time by `claim_next`) against `retries + 1`. `--retries` default **1**.
- **`--max-videos` caps total claims including retries** (already true mechanically — the shared `claims_counter` counts every claim; Task 06 pins it with a test).

## Task index

| # | File | Deliverable |
|---|------|-------------|
| 01 | `01-classification-config.md` | `src/classification.rs`: TOML table types + validation + compiled default + `toml` dep + 10240 fixture |
| 02 | `02-schema-v3-batch-runs.md` | Schema v3: `batch_runs` table, attempt-aware pending index, migrate ladder, open/close mutators |
| 03 | `03-classify-table-rewire.md` | `failure.rs` label-string rewire; classifiers take `&ClassificationTable`; cookie gate consults table; behavior identical |
| 04 | `04-record-fetch-failure.md` | `Store::record_fetch_failure`: requeue/exhaust/park decision in one transaction + outcome enum |
| 05 | `05-claim-ordering.md` | `claim_next` orders `attempt_count ASC` (end-of-queue retries) + ordering tests |
| 06 | `06-dispatch-and-cli.md` | Workers + serial dispatch through `record_fetch_failure`; `--retries` + `--classification` CLI; fails-N-then-succeeds fake; integration tests |
| 07 | `07-sweep-and-census.md` | Start-of-batch sweep; `batch_runs` open/close in Process arm; census struct + persist + print |
| 08 | `08-retirements-docs-adrs.md` | Delete triage/probe/curl; ADR slate via adg; docs updates; FOLLOWUPS moves; EPIC-4A-CLOSE |

Dependency chain: 01 → 03; 02 independent; 04 independent (strings/bools only); 05 after 04 (same file, avoids conflicts); 06 needs 01+03+04+05; 07 needs 01+02+03+04; 08 last. Execute sequentially 01→08.

## Cross-cutting context subagents may need (current `main` shapes, verified 2026-07-07)

- `src/failure.rs` (487 lines): `RetryableKind`/`UnavailableReason` enums with `tag()`, `classify_message` (ordered `.contains` chain), `classify_fetch_error`, `classify_transcribe_error`, `FailureContext` (fields `tool`/`exit_code`/`signal` carry `#[allow(dead_code)]`), fixture-driven tests via `macro fixture!`.
- `src/state/mod.rs` (817 lines): `claim_next` (BEGIN IMMEDIATE; SELECT `ORDER BY first_seen_at ASC, video_id ASC`; bumps `attempt_count` to `prev+1` at claim time; inserts `claimed` event), `mark_succeeded` / `mark_retryable_failure` / `mark_terminal_failure` (predicate `status='in_progress' AND claimed_by=?`, event insert gated on `changed > 0`), `sweep_stale_claims`, `list_failed_retryable`, `triage_mark_terminal` (event `triaged_terminal`, worker `'triage'`), `requeue_retryable` (cap in predicate `attempt_count < ?`, event `requeued`, worker `'triage'`), cfg-gated `get_video_for_test` / `get_events_for_test`.
- `src/state/schema.rs`: `SCHEMA_VERSION = "2"`; `idx_videos_pending ON videos (status, first_seen_at, video_id) WHERE status='pending'`. `src/state/migrate.rs`: v1→v2 ladder, bail on unknown versions.
- `src/pipeline/mod.rs` (488): `ProcessOptions` (has `cookies_file: Option<PathBuf>`, `max_videos`, `download_workers`, etc.), `ProcessStats` (`claimed/succeeded/failed/stale_after_success/stale_after_failure`), `FetchPhaseError`, `classify_fetch_phase`, `cookie_opts_for(claim, cookies_file)` (compares `last_retryable_kind == Some("SensitiveLoginGated")`), `fetch_and_decode`, `write_artifacts_and_mark` (0008 single source of truth).
- `src/pipeline/pipelined.rs` (743): `handle_mutator_result(result, worker_id, video_id, stale_counter, op)` at line ~41; `fetch_worker` error dispatch at ~253-309 (`classify_fetch_phase` → Bug `return Err` / Unavailable `mark_terminal_failure` / Retryable `mark_retryable_failure`, each via `handle_mutator_result`); `transcribe_worker` dispatch at ~498-535; `run_pipelined` at ~575 (sweep_stale_claims at start, JoinSet, `claims_counter: Arc<AtomicUsize>` enforcing max_videos race-free).
- `src/pipeline/serial.rs` (348): `run_serial` error dispatch at ~86-176 (FetchPhaseError top-level downcast + TranscribeError chain-walk; tripwire comment at ~80-85), `process_one` at ~190.
- `src/main.rs` (249): `#[tokio::main]`; Process arm at ~72-194 (engine init, 0025 shutdown ORDER comment — **do not reorder steps around `engine.shutdown()`**), Triage arm at ~206-225. `src/cli.rs` (180): `GlobalArgs` (profile/state-db/inbox/transcripts/log-format/whisper-model/compute-lang-probs/stale-claim-threshold/download-workers/channel-capacity), `Command::{Init, Ingest, Process{max_videos, cookies_file}, Migrate, Triage{dry_run, rate, max_attempts}}`, `parse_positive_rate`. `src/config.rs` (149): `Config::from_args`, Dev-profile defaults.
- `src/fetcher/mod.rs`: `FetchOpts { cookies_file: Option<PathBuf> }`; `FakeFetcher` fields `canned: Mutex<HashMap<String, PathBuf>>`, `always_fails: bool`, `first_call_gate: tokio::sync::Mutex<Option<…>>`, `canned_stderr: Mutex<Option<String>>`, `received_opts: Mutex<Vec<FetchOpts>>` (cfg-gated `any(test, feature = "test-helpers")`).
- `tests/pipeline_fakes/` (dir target, main.rs + fakes.rs + serial_tests.rs + fetch_worker_tests.rs + transcribe_worker_tests.rs + pipelined_tests.rs): `FakeBehavior::{…, AlwaysFailsRetryable, AlwaysFailsBug}` and helpers `pub(crate)` in fakes.rs.
- Message → label chain currently in `classify_message` (order): `"Your IP address is blocked"`→IpBlockedMessage; `"status code 10231"`→VideoNotAvailable10231; `"Did not get any data blocks"`→NoDataBlocks; `"do not have permission to view this post"`→NoPermission; `"not be comfortable for some audiences"`→SensitiveLoginGated; `"No video formats found"`→NoVideoFormats; `"unable to obtain file audio codec with ffprobe"`→FfprobePostprocess; `"HTTP Error"`→HttpError; network markers (`"Unable to download webpage"`, `"HTTPSConnectionPool"`, `"Connection aborted"`, `"ConnectionResetError"`, `"RemoteDisconnected"`, `"curl: (28)"`, `"SSL"`, `"Too Many Requests"`)→NetworkTransient; fallback→YtDlpOther.
- Production evidence for doc comments: census 2026-07-07 (n=7,087): IpBlocked 3,241 write-off; 10231 68; `"Video not available, status code 10240"` 606/606 probe-dead (single exact message — the entire former YtDlpOther population); NoPermission 427 dead / **25 alive** (impure → retryable); NoDataBlocks 2,311/2,318 alive; SensitiveLoginGated 301.
- The pilot DB (`ddp-run-export.sqlite`, repo root, untracked) has 7,087 `failed_retryable` rows, all `attempt_count = 1`, some with placeholder kind `"Fetch"` and full yt-dlp stderr in `last_retryable_message`.

## What this epic deliberately omits

- Time-window filter, DDP timezone resolution, full `status` subcommand (Epic 4b, per `EPIC-4-SKETCH.md`).
- Backoff/jitter — retries are end-of-queue FIFO, once by default.
- Cookie acquisition operations; proving live cookie efficacy (first real cookie batch is the experiment).
- `run_serial` retirement decision (Epic 5, per 0002 note in `src/pipeline/mod.rs`).
- Any change to ADR-0008 artifact ordering, 0024 stale sweep, 0027 topology, 0032 storage topology.
