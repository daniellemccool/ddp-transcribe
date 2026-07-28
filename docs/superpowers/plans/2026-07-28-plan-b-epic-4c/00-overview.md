# ddp-transcribe — Plan B Epic 4c: fetch-time metadata capture (raw-first, schema v5, post-run loader)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` … `05-…md`). Open only the task you're working on. Task files are self-contained; do NOT load the spec or other task files into a subagent's context.

**Goal:** Every yt-dlp fetch also captures a raw metadata envelope (title/description, uploader, engagement counts, caption tracks) into a new `video_metadata_raw` table — regardless of fetch outcome, with zero extra network requests — and a new post-run `load-metadata` subcommand parses those blobs into typed nullable columns on `videos` (schema v5), replayably.

**Architecture:** Raw-first: the fetcher appends `--no-simulate --print "%(.{…})j"` (+ `--write-subs --write-auto-subs`) to the existing invocation, captures ≤64 KB of stdout (validated live: ~615 B/video), wraps it UNPARSED in a versioned envelope with any subtitle-sidecar contents embedded, and returns it alongside BOTH the success and failure outcome; the pipeline inserts the envelope before outcome dispatch. Parsing happens only in `load-metadata` (streaming keyset pagination, batched UPDATEs) — any parse bug is fixable forever by re-parse, never re-fetch. Spec: `docs/superpowers/specs/2026-07-28-epic-4c-fetch-metadata-design.md`.

**Tech Stack:** Rust 2021, tokio, rusqlite, clap 4, serde/serde_json (all existing). **No new dependencies.**

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **Clippy gates:** `unwrap_used` / `expect_used` **denied** in production code (allowed in tests via existing crate-root `cfg_attr` + per-test-file `#![allow(clippy::unwrap_used, clippy::expect_used)]` headers — new test files copy the header from an existing `tests/*.rs`).
- **Mutators return `Result<usize>`** (row-change count) per ADR-0006. Read-only queries live in `src/state/queries.rs` and return typed row structs (Epic 4b precedent).
- **Stats structs:** input-side counters, verb-named fields, per ADR-0007.
- **Schema changes** per ADR-0022: bump `SCHEMA_VERSION` AND extend the migrate ladder in the same task; test both directions (old DB open fails typed; `migrate` idempotent). Never auto-migrate on open.
- **Metadata must never create a new failure mode** (spec decision 5): capture/insert/parse failures log + count and the video proceeds exactly as today. This is the epic's own invariant — review rejects any path where a metadata error changes a video's status outcome.
- **ADR authoring** via `write-adr:write-lean-adr` / `adg lean new --from-stdin` ONLY — never hand-edit `docs/decisions/`. Pre-commit hook runs `adg lean index --root .` + `adg lean check`; fix inconsistencies, never bypass.
- **New integration tests** using cfg-gated helpers need `[[test]] required-features = ["test-helpers"]` per ADR-0005. Tests using only public API (binary via assert_cmd, `Store::open`, raw rusqlite) are auto-discovered and need NO Cargo.toml block.
- **Dead-code hygiene** per ADR-0002; **commit deviation disclosure** per ADR-0003.
- **Cookie-path secrecy (ADR-0035):** nothing in the captured envelope path may log or store the cookie file path.
- The pilot snapshot `ddp-run-export.sqlite` (repo root of the 4b worktree, untracked) is NOT used by this epic; if any check touches a real DB, use a scratch copy.

## Ground truth (verified live 2026-07-28, yt-dlp 2026.07.04)

- Print template `%(.{id,title,description,uploader,uploader_id,channel_id,timestamp,duration,view_count,like_count,comment_count,repost_count,subtitles,automatic_captions})j` emits ONE line of JSON, ~615 bytes on a real corpus video, all fields populated (`subtitles`/`automatic_captions` may be `null`/`{}` — 0/46 probed corpus videos had tracks).
- `process::run` ALREADY returns `CommandOutcome` (with captured stdout) on nonzero exit — only `RunError` paths (timeout/spawn/io) lose stdout, which is acceptable (no metadata on those). **No `src/process.rs` behavior change is needed**; Task 02 only lifts the `#[allow(dead_code)]` on `CommandOutcome.stdout` (its comment says it exists because all call sites pass 0 — no longer true).
- Production scale: 144 donors / 4,847,408 watch entries / 2,982,471 unique videos (2026-07-28 inbox). Efficiency notes in the spec.

## Task index

| # | File | Deliverable |
|---|------|-------------|
| 01 | `01-schema-v5-raw-store.md` | Schema v5 (`video_metadata_raw` + 9 nullable `videos` columns), migrate ladder v4→v5, `Store::upsert_metadata_raw` |
| 02 | `02-fetcher-capture.md` | `MetadataCapture` type; `VideoFetcher::acquire` returns `(Option<MetadataCapture>, Result<…>)`; yt-dlp argv + envelope builder + sidecar embed; FakeFetcher injection; call sites thread the tuple (capture discarded until Task 03) |
| 03 | `03-pipeline-persistence.md` | Pipelined + serial paths insert the envelope via `upsert_metadata_raw` BEFORE outcome dispatch, best-effort; integration tests prove raw rows for succeeded AND failed videos |
| 04 | `04-load-metadata.md` | `load-metadata` subcommand: streaming loader (`src/metadata_loader.rs`), `Store::apply_metadata_batch`, `LoadStats` per 0007, `--dry-run`, idempotence, missing-DB bail |
| 05 | `05-production-hardening.md` | Operator-review fixes: unique `atomic_write` tmp names (cross-process collision), artifact fsyncs OUTSIDE the store lock (pipelined phase-4 split + ADR-0008 revision), honest `cleanup_tmp_files` count |
| 06 | `06-close-docs.md` | Ignored live e2e; capture-chain ADR via adg; architecture/ops doc updates; FOLLOWUPS (incl. bin/lib module-tree finding → Epic 5); EPIC-4C-CLOSE |

Dependency chain: 01 → 02 → 03 → 04 → 05 → 06, strictly sequential (03 needs both 01's mutator and 02's capture; 05 is independent of 01–04 but runs before close so its ADR-0008 revision lands ahead of the doc pass). Every task leaves the tree green.

## Cross-cutting context subagents may need (as-landed `main` shapes, verified 2026-07-28, post-PR-#21)

- **Binary:** `ddp-transcribe`. `SCHEMA_VERSION = "4"` (`src/state/schema.rs:1`); migrate ladder v1→v2→v3→v4 in `src/state/migrate.rs` (sequential `if version == "N"` blocks inside one transaction, `INSERT … ON CONFLICT` meta upsert after).
- `src/process.rs`: `CommandSpec { program, args, timeout, stderr_capture_bytes, stdout_capture_bytes, redact_arg_indices }`; `CommandOutcome { exit_code, stdout: Option<Vec<u8>> (#[allow(dead_code)] today), stderr_excerpt, signal, elapsed }`; `run()` returns `Ok(CommandOutcome)` on ANY exit code, `Err(RunError)` only on timeout/spawn/io.
- `src/fetcher/mod.rs`: `Acquisition::AudioFile(PathBuf)` (single variant); `FetchPolicy::{DeterministicAudio (default), Frugal}` + `.tag()`; `FetchOpts { cookies_file, format_policy }`; `trait VideoFetcher { async fn acquire(&self, video_id, source_url, opts) -> Result<Acquisition, FetchError>; fn name() }`; cfg-gated `FakeFetcher` (fields `canned, always_fails, first_call_gate, canned_stderr, received_opts, fail_first_n`; constructors `always_fails()`, `fails_with_stderr()`, `gated_then_always_fails()`).
- `src/fetcher/ytdlp.rs`: pure `build_yt_dlp_args(video_id, source_url, video_dir, policy, cookies) -> (Vec<String>, PathBuf, Vec<usize>)` — argv is `--no-playlist --no-warnings --quiet -f <sel> -S +size,+br,+res,+fps -x --audio-format wav --postprocessor-args "ffmpeg:…" -o <tmpl> [--cookies <path>] <url>`; redact indices computed dynamically after cookie push. `YtDlpFetcher::acquire` runs `CommandSpec { stderr_capture_bytes: 8*1024, stdout_capture_bytes: 0, … }`, maps nonzero exit → `FetchError::ToolFailed` (stderr scrubbed via `scrub_cookie_path`), missing wav → `FetchError::MissingOutput`. Per-video dir: `work_dir.join(format!("ytdlp-{video_id}"))`.
- `src/pipeline/mod.rs`: `fetch_and_decode(fetcher, claim, opts) -> Result<(Vec<f32>, PathBuf), FetchPhaseError>` (line ~292) — calls `acquire` then `audio::decode_wav`. `cookie_opts_for` decides `FetchOpts` per claim.
- `src/pipeline/pipelined.rs`: `SharedStore = Arc<tokio::sync::Mutex<Store>>`; `fetch_worker` wraps `fetch_and_decode` in `tokio::select!` with cancellation (line ~306); success arm builds `FetchedItem`, failure arm classifies then locks store for the failure mutator. Lock discipline: guard held only for DB calls, never across fetch/transcribe awaits.
- `src/pipeline/serial.rs`: `process_one` (line ~256, `#[allow(dead_code)]`) calls `fetch_and_decode` at line ~271 with `store: &mut Store` in scope.
- `src/state/mod.rs`: `unix_now()`, `Store::open` (version gate → typed `StateError::SchemaVersionMismatch`), `conn()/conn_mut()` `pub(crate)`, mutator style = `transaction_with_behavior(Immediate)` + `params!` + `.with_context` + `Ok(changed)`; `SuccessArtifacts { duration_s, language_detected, fetcher, transcript_source }`; `mark_succeeded` guarded `WHERE status='in_progress' AND claimed_by = ?` per 0023.
- `src/state/queries.rs`: Epic 4b's read-only `impl Store` block + typed row structs — the loader's page query follows this precedent.
- `src/cli.rs`: `Command::{Init, Ingest{dry_run, window_start, window_end}, Process{max_videos, cookies_file, retries}, Migrate, Status{…}, RecomputeWindow{…}}`; `parse_window_date`, `validate_window_order` `pub(crate)`.
- `src/main.rs`: `log_resolved_config` is an EXHAUSTIVE match over `Command` (Epic 4b Task 07) — adding a variant forces a new arm. Migrate/Status/RecomputeWindow arms bail when `--state-db` doesn't exist; mirror for LoadMetadata.
- Tests: `tests/state_migrate.rs` (hand-built old-version DBs — copy its construction style for v4→v5), `tests/cli.rs` (assert_cmd binary tests, auto-discovered), `tests/pipeline_fakes/` (registered with `required-features = ["test-helpers"]`; `pipelined_tests.rs` drives `fetch_worker` with `FakeFetcher`). Suite baseline at branch start: 283 passed / 0 failed / ~8 model-gated ignored.

## What this epic deliberately omits (spec non-goals)

- Transcript artifact schema unchanged; no `status` surface extension; no pilot-corpus backfill; no delivery-export subcommand; no comments (Research API only); production-run capacity planning.

## Dispatch, review, and phase conventions (0018 / 0019)

- Every dispatch brief requires the structured report: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`, ≤250 words. Implementer test summaries state the TOTAL passed summed across all `test result:` lines.
- Three-tier review per task: implementer → Sonnet spec-compliance reviewer (whose brief includes the codex-advisor call, ≤200-word replies, distilled to ≤300 words) → codex-advisor reached ONLY through tier 2. Orchestrator spot-checks `tail -200` of the codex transcript log every 4–5 tasks.
- Fix-loop subagents on Sonnet. Single phase (5 tasks, no controller restart); final whole-branch review on the most capable model before merge.
