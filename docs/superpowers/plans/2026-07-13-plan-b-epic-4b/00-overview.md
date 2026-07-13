# ddp-transcribe — Plan B Epic 4b: `status` subcommand, time-window + timezone, CLI hardening

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` … `08-…md`). Open only the task you're working on. Task files are self-contained; do NOT load the spec or other task files into a subagent's context.

**Goal:** The operator can interrogate pipeline state from the tool itself: a `status` subcommand renders counts, failure breakdowns by kind, in-progress claim ages, and honest `batch_runs` history (interrupted runs say so); `status --verify` fulfills the archived ADR-0017 done-contract (artifacts on disk, `raw_signals.schema_version`, pause-safe verdict, non-zero exit on violation). Ingest gains the analysis-window filter (`--window-start`/`--window-end` → `in_window`), preserves the raw DDP `Date` string (`watched_at_raw`, schema v4), and the DDP timezone assumption gets an evidence-based ADR verdict. `--retries` gets range validation and the config echo stops advertising config the subcommand never consumes.

**Architecture:** Read-only status queries live in a new `src/state/queries.rs` (`impl Store` block + row structs — keeps the 1,000-line `state/mod.rs` from growing); rendering + the `--verify` checks live in a new `src/status.rs`. Window math (`WindowBounds`) lives in `src/ingest.rs`; the `recompute_window` mutator takes plain `Option<i64>` bounds so `state` never imports from `ingest`. Schema v4 = one nullable column (`watch_history.watched_at_raw`); `in_window` already exists in v3 (ingest currently hardcodes `true`).

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4, serde, chrono (all existing). **No new dependencies.**

**Reference:** kickoff at `docs/superpowers/plans/PLAN-B-EPIC-4B-KICKOFF-PROMPT.md`; sketch at `docs/superpowers/plans/2026-05-12-plan-b/EPIC-4-SKETCH.md`; ground truth in `docs/superpowers/plans/2026-07-07-plan-b-epic-4a/EPIC-4A-CLOSE.md` § "First production batch". Subagents should not need any of them — task files are self-contained.

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **Clippy gates:** `unwrap_used` / `expect_used` **denied** in production code (allowed in tests via existing crate-root `cfg_attr` + per-test-file `#![allow(clippy::unwrap_used, clippy::expect_used)]` headers — new test files copy the header from an existing `tests/*.rs`).
- **Mutators return `Result<usize>`** (row-change count) per ADR-0006; failure mutators keep string kinds + the claim guard per ADR-0023. Read-only queries return typed row structs (`list_failed_retryable` precedent).
- **Stats structs:** input-side counters, verb-named fields, per ADR-0007.
- **Schema changes** per ADR-0022: bump `SCHEMA_VERSION` AND extend the migrate ladder in the same task; test both directions (old DB open fails typed; `migrate` idempotent). Never auto-migrate on open.
- **ADR authoring** via the `write-adr:write-lean-adr` skill / `adg lean new --from-stdin` ONLY — never hand-edit `docs/decisions/`. `adg lean index --root .` + `adg lean check` run as the pre-commit hook; fix inconsistencies, never bypass.
- **New integration tests** that use cfg-gated helpers need `[[test]] required-features = ["test-helpers"]` per ADR-0005. Tests using only public API (binary via assert_cmd, `Store::open`, raw `rusqlite`) are auto-discovered and need NO Cargo.toml block (`tests/cli.rs`, `tests/state_open.rs` precedent).
- **Dead-code hygiene** per ADR-0002 (inline lift-point comment + "0002 dead-code note:" commit paragraph).
- **Commit disclosure** per ADR-0003: any deviation from the task brief is disclosed prominently in the commit message.
- **Operator-interface premise (0032 comment):** operator commands are baked into the tool. No wrapper-script assumptions anywhere in code or docs.
- **`status` never creates or mutates study state.** The Status/RecomputeWindow arms bail if `--state-db` does not exist (mirror the Migrate arm) — `Store::open` on a missing path would silently create an empty DB and report zero counts.
- **The pilot snapshot `ddp-run-export.sqlite` (repo root, untracked, schema v3) is the epic's built-in acceptance fixture.** Ground-truth table below. Tasks run the binary against it read-only; NEVER run `migrate` against it in place — Task 05 copies it to a scratch path first.
- **Legacy placeholder kind `"Fetch"` (301 cookie-parked rows in the snapshot):** `status` renders the stored value with a `(legacy placeholder kind)` annotation in human output; JSON carries the raw stored value. Never relabel or misattribute it to a taxonomy kind.

## Ground truth for acceptance (2026-07-08 production batch → `ddp-run-export.sqlite`)

`status` output that disagrees with this table is wrong:

- 56,620 videos → **51,903 succeeded / 3,928 failed_terminal / 789 failed_retryable / 0 pending / 0 in_progress**.
- failed_retryable by `last_retryable_kind`: **NoPermission 418, Fetch 301 (legacy placeholder), FfprobePostprocess 36, NoVideoFormats 32, NoDataBlocks 1, HttpError 1**.
- `batch_runs`: **run 1 open/INTERRUPTED** (started 2026-07-08 11:41:50 UTC, `finished_at IS NULL`, `census_json IS NULL`, retries=1) — must render honestly, never skip, never crash on NULLs; **run 2 closed** (started 15:47:11, finished 16:32:12 UTC, retries=2, census persisted). Both runs' `policy_toml` = the compiled default (3,065 bytes).
- watch_history: 64,956 rows, 2 respondents (`preview` 64,931, `newsorg-fixture` 25), all `in_window = 1`.

## Task index

| # | File | Deliverable |
|---|------|-------------|
| 01 | `01-timezone-verdict-adr.md` | DDP timestamp timezone verdict: evidence dossier → lean ADR via adg; `parse_watched_at` comment fix |
| 02 | `02-status-core.md` | `src/state/queries.rs` + `src/status.rs` + `Command::Status`: counts, retryable-by-kind, in_progress ages, honest `batch_runs` history, `--json` |
| 03 | `03-status-detail-surfaces.md` | `--video-id` event history (legible `detail_json`), `--respondent-id` summary, `--errors` / `--retryable` lists |
| 04 | `04-status-verify-done-contract.md` | `status --verify`: artifact existence (per-shard `read_dir`), `raw_signals.schema_version` parse, pause-safe verdict, exit semantics |
| 05 | `05-schema-v4-windowed-ingest.md` | Schema v4 (`watched_at_raw`), migrate ladder v3→v4, `ingest --window-start/--window-end`, raw-date preservation + backfill |
| 06 | `06-recompute-window.md` | `recompute-window` subcommand: refuses bare invocation, `--clear`, `--dry-run`, `Store::recompute_window` |
| 07 | `07-cli-hardening.md` | `--retries` ranged value parser; config echo scoped to consumed config |
| 08 | `08-close-docs-adrs-followups.md` | Window-semantics + status ADRs; 0017 fulfillment record; architecture-doc updates; FOLLOWUPS lifecycle; EPIC-4B-CLOSE |

Dependency chain: 01 first (its verdict feeds 05's ADR language and doc comments); 02 → 03 → 04 sequential (same files); 05 → 06 sequential (same files); 07 independent; 08 last. Execute sequentially 01→08. Note 02–04 acceptance-check against the v3 snapshot directly; 05 bumps the binary to v4, after which snapshot checks go through a migrated scratch copy.

## Dispatch, review, and phase conventions (0018 / 0019)

- **Every dispatch brief** requires the structured report: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`, **≤250 words** (exceed only for genuinely unusual cases, and say why). Full implementation transcripts never flow back to the orchestrator.
- **Three-tier review per task:** (1) the implementer (brief-verbatim code + ADR-0003 deviation honesty); (2) a Sonnet spec-compliance reviewer (mechanical does-this-match-the-brief + declared-ADR check) whose brief includes the codex-advisor call (request ≤200-word replies) and distills the advisor's response to **≤300 words** of actionable items; (3) codex-advisor code-quality review — reached ONLY through tier 2. The orchestrator never calls codex-advisor during task reviews; it may spot-check `codex-advisor transcript | tail -200` every 4–5 tasks.
- **Phase boundary after Task 04** (status surface complete): the controller writes `PHASE-1-CLOSE.md` (≤1 page: what landed, commit SHAs, open deviations, anything Task 05+ must know) in this plan directory and ends its session; a fresh controller starts Phase 2 (Tasks 05–08) from this overview plus that close-out doc.

## Cross-cutting context subagents may need (current `main` shapes, verified 2026-07-13)

- **Binary name:** `ddp-transcribe`. `src/cli.rs` (126 lines): `GlobalArgs` (profile / state_db / inbox / transcripts / log_format / whisper_model / classification / compute_lang_probs / stale_claim_threshold / download_workers / channel_capacity — the last two use `clap::builder::RangedU64ValueParser::<usize>::new().range(1..)`), `Command::{Init, Ingest{dry_run}, Process{max_videos, cookies_file, retries: i64 default_value_t = 1 — UNVALIDATED}, Migrate}`.
- `src/main.rs` (307): single `"config resolved"` echo at startup logging `profile`/`state_db`/`whisper_model_path` for every subcommand; Process arm ~72–272 (0025 shutdown ORDER comment — **never reorder around `engine.shutdown()`**); Migrate arm bails if `!path.exists()`.
- `src/state/schema.rs`: `SCHEMA_VERSION = "3"`. Tables: `videos` (status CHECK in pending/in_progress/succeeded/failed_terminal/failed_retryable; claimed_by/claimed_at; attempt_count; last_retryable_kind/_message; terminal_reason/_message; first_seen_at/updated_at), `watch_history` (respondent_id, video_id, watched_at, **in_window INTEGER NOT NULL** — exists since v1, ingest hardcodes `true`; PK (respondent_id, video_id, watched_at)), `video_events` (id, video_id, at, event_type, worker_id, detail_json), `meta`, `batch_runs` (run_id, started_at, finished_at NULL=interrupted, params_json, policy_toml, census_json NULL=no census).
- `src/state/migrate.rs` (114): sequential ladder v1→v2→v3 inside one transaction; bails on unknown/newer versions; `INSERT … ON CONFLICT` upserts `meta.schema_version` after the ladder.
- `src/state/mod.rs` (1035): `unix_now()`, `Store::open` (WAL pragmas, idempotent schema apply, 0022 version gate → typed `StateError::SchemaVersionMismatch`), `read_meta`, `pub(crate) conn()/conn_mut()`, `UPSERT_VIDEO_SQL`/`UPSERT_WATCH_HISTORY_SQL` consts + `upsert_video_tx`/`upsert_watch_history_tx(tx, respondent_id, video_id, watched_at, in_window)` (INSERT OR IGNORE, `prepare_cached`), `claim_next`, `mark_succeeded`, `mark_retryable_failure`, `record_fetch_failure`, `mark_terminal_failure`, `sweep_stale_claims`, `list_failed_retryable() -> Vec<ParkedRow{video_id, last_retryable_kind, last_retryable_message, attempt_count}>`, `sweep_mark_terminal`, `sweep_requeue`, `open_batch_run`, `close_batch_run`; cfg-gated `get_video_for_test`/`get_events_for_test`.
- `src/ingest.rs` (233): `IngestStats` (7 input-side counters), `ingest(inbox, store)` (per-file transaction batching), `process_watch_entry`, `parse_respondent_id_from_filename`, `parse_watched_at` (const `FORMATS` slice: `"%Y-%m-%d %H:%M:%S"` — comment WRONGLY says "synthetic fixtures"; and `"%Y-%m-%d %H:%M:%S UTC"` — comment says "production TikTok DDP"; parses as naive → `Utc.from_utc_datetime`).
- `src/batch.rs` (537): `SweepStats`, `RunCensus` (fields claimed/succeeded/failed/requeued_for_retry/exhausted_retries/parked_for_cookies/terminal_by_label/stale_after_success/stale_after_failure), `BatchCensus{sweep, run}` (Serialize + Display — this is what's inside `census_json`), private `truncate_to_char_boundary(&mut String, max_bytes)` (Task 03 makes it `pub(crate)`), `run_sweep`.
- `src/output/mod.rs`: `shard(video_id) -> &str` (last two chars; 100 buckets), `shard_dir(root, video_id)`. `src/output/artifacts.rs`: `EXPECTED_RAW_SIGNALS_SCHEMA_VERSION: &str = "1"`, `TranscriptMetadata` (Deserialize; `raw_signals: Option<RawSignals>`, `RawSignals.schema_version: String`), `atomic_write`, `cleanup_tmp_files`. Artifact layout: `{transcripts_root}/{shard}/{video_id}.txt` + `.json`.
- `src/classification.rs`: `ClassificationTable::compiled_default()`, `.source_toml() -> &str` (the compiled default TOML is 3,065 bytes — `batch_runs.policy_toml` equality against it identifies "compiled default" provenance), `.rule_count()`.
- `src/config.rs`: `Config::from_args(&GlobalArgs)`; Dev profile defaults (`whisper_model_path` default `./models/ggml-tiny.en.bin`).
- **`video_events` vocabulary + `detail_json` shapes** (for `--video-id` rendering): `claimed` (detail NULL), `succeeded` (NULL), `failed_retryable` `{"kind","message"}` (legacy seeding mutator) or `{"kind","message","policy"}` (exhausted via `record_fetch_failure`), `retry_requeued` `{"kind","message","policy"}`, `cookie_parked` `{"kind","message","policy"}`, `failed_terminal` `{"reason","message"}`, `swept_terminal` `{"reason","message"}` (worker `'sweep'`), `requeued` `{"new_kind"}` (worker `'sweep'`).
- Tests: assert_cmd + predicates in dev-deps; `tests/cli.rs` is the binary-test precedent (auto-discovered, no Cargo.toml block); `tests/state_migrate.rs` hand-builds old-version DBs (copy its construction style for the v3→v4 test).

## What this epic deliberately omits

- **Cookie-efficacy run** — operational, not code (301 `SensitiveLoginGated` rows wait for the catalog-item workspace). `status` is the tool that will read its results.
- Epic 5 cleanup (`run_serial` retirement, `state/mod.rs` hygiene bundle, sync-IO sweep) — routed, untouched.
- Deployment/delivery repos (researchcloud-ddp-transcribe, ddp-inspector).
- Backoff/jitter or richer retry semantics — 0036 stands as shipped.
- Schema-version **sampling** (Plan C scale concern; full parse is fine at Plan B scale).
- `status --logs` (sketch brainstorm) — not in the kickoff's settled scope; note it in EPIC-4B-CLOSE's deferred list.
