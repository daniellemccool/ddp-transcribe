# ddp-transcribe — v0.5.1: deadline-attribution patch

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-…md` onward). Open only the task you're working on. Task files are self-contained; do NOT load other task files into a subagent's context.

**Goal:** Ship v0.5.1 — a per-request transcription deadline no longer kills
the whole run (it becomes a retryable per-item `Timeout`), an aborted run
still closes its `batch_runs` row (no more lost censuses), and yt-dlp's
internal retry count is capped — so the census tail's attempt-2 tier cannot
re-kill end-of-campaign runs.

**Architecture:** Bug-fix release, no new ADRs (no decision changes: ADR-0012
per-request cancellation and ADR-0025 shutdown order are untouched — this
patch corrects *variant attribution* so the existing, already-tested
`Timeout → Retryable` path is actually reached). Evidence: 2026-08-17
incident on the campaign VM — video `7645028780246895894` hit the 600 s
transcribe deadline; engine returned `Cancelled`; the transcribe worker
treated it as coordinated shutdown and exited; fetch workers cascaded into
`channel closed` errors; run died with an unclosed `batch_runs` row (rowid
20, `finished_at` NULL). Log timing: `processing item` 14:43:40.258224 →
`Cancelled` 14:53:40.258893 = 600.001 s.

**Tech Stack:** Rust 2021, tokio, rusqlite (all existing). No new
dependencies.

---

## Global constraints

Every task's requirements implicitly include:

- **Verification command:** `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1 && cargo build --release`. **`--test-threads=1` is mandatory on this workstation (thermal); never drop it.**
- **TDD** per ADR-0003 (batch test-first for plan-prescribed code; watch each new test fail for the real reason).
- **No `Cargo.toml` version bump on the branch** — 0.5.0 → 0.5.1 happens in the post-merge tag commit (ADR-0043).
- **No new ADRs this release** — these are defect corrections inside existing decisions. Binding invariants in scope: 0012 (cancellation stays per-request; only the *attribution* of a deadline-fired abort changes), 0025 (cancel/drain/shutdown order untouched), 0008 (artifacts before mark_succeeded — untouched), 0033/0037 (classification table untouched — `Timeout` already classifies `Retryable` via `src/failure.rs:188`), 0021 (subprocess capture bounds untouched).
- **Commit deviation disclosure** per ADR-0003.
- The engine-level deadline test is **model-gated** (`tests/whisper_engine_init.rs`, requires `./models/ggml-tiny.en.bin`, runs with `-- --ignored`). If the model file is absent, fetch it first: `mkdir -p models && curl -L -o models/ggml-tiny.en.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin` (~75 MB). If that is impossible, run everything else and report the gap as a deviation — do not skip silently.

## Dispatch conventions (ADR-0018 / ADR-0019 — binding on every task)

- **Subagent reports:** every dispatch brief requires a structured report,
  ≤250 words: `STATUS / SUMMARY / CHANGED FILES / DEVIATIONS`. Full
  implementation transcripts never flow back to the controller.
- **Three-tier review per task:** implementer → Sonnet spec-compliance
  reviewer → codex-advisor delegated through the reviewer (≤200-word
  replies, ≤300-word distillation). The orchestrator never calls
  codex-advisor directly during task reviews.
- **Phase boundaries:** this plan is a **single phase** (tasks 01–05, one
  controller session) — five small tasks, per the operator's standing
  ruling (2026-08-12) that plans under ~12 tasks run in one session; the
  final whole-branch review is the close-out gate.

## Ground truth (verified in code 2026-08-18, main @ 0348ad9)

- **The conflation:** `src/transcribe.rs` returns `TranscribeError::Cancelled` for BOTH the per-request cancel flag AND deadline elapse, at three sites: (1) the early dequeue check (~:988, `req.cancel.load(..) || Instant::now() >= req.deadline`); (2) the post-lang_detect recheck (~:1157, same predicate); (3) the post-inference attribution (~:1183, `Err(_) if was_cancelled` where `abort_fired` captured either predicate having fired). The abort callback itself (~:1047-1056) polls both conditions into one `should_abort`.
- **The unconstructed variant:** `TranscribeError::Timeout { duration: Duration }` exists (`src/errors.rs:85`), is classified `Retryable` by `classify_transcribe_error` (`src/failure.rs:188`), has a label mapping (`src/transcribe.rs:326` → `"timeout"`), and the enum's own doc comment (`src/errors.rs:73-83`) documents that the embedded engine never constructs it and "surfaces deadline-elapse via `Cancelled`" — that comment must be rewritten by Task 01.
- **The kill path:** `src/pipeline/pipelined.rs:774-787` — the `Err(TranscribeError::Cancelled)` arm returns `Ok(())` (worker exit, comment "Coordinated shutdown, not a row failure"), which closes the fetch→transcribe channel; fetch workers then `Err` on send. The neighboring `Err(e)` arm (:806+) already routes `Timeout` through `classify_transcribe_error` → `record_fetch_failure` → requeue/exhaust/park and continues. The long doc comment at :565-590 describes the cancellation composition and must gain the deadline distinction.
- **Deadline plumbing:** the public `transcribe(samples, per_call, timeout)` wrapper computes `deadline = Instant::now() + timeout` (~:1457) and builds the request; the request struct carries `pub deadline: Instant` (:292) but NOT the original `Duration` — Task 01 adds a `timeout: Duration` field so `Timeout { duration }` is constructible. One other request-literal exists in a unit test (~:1842, `deadline: Instant::now() + Duration::from_secs(60)`); sweep every construction site (`rg 'deadline:' src/transcribe.rs`).
- **Census loss:** `src/commands.rs:290` — `let stats = stats_result?;` propagates a `run_pipelined` `Err` before the census block (:314-330) runs; `close_batch_run(run_id, census_json)` (`src/state/mod.rs:1536`, returns `Result<usize>` per ADR-0006) is never called → `batch_runs.finished_at` stays NULL and the census is lost. Confirmed live: rowid 20 unclosed after the 2026-08-17 kill.
- **Test seams:** `FakeTranscriber` (`tests/pipeline_fakes/fakes.rs`) already has `FakeBehavior::AlwaysFailsTimeout` (:84-:108) returning `Err(TranscribeError::Timeout { .. })` — used by no pipelined test yet. Pipelined test idiom + full `ProcessOptions` literal shape: `tests/pipeline_fakes/pipelined_tests.rs` (`run_pipelined_honors_max_videos_cap`). `ProcessOptions` includes `breaker_threshold: usize` (set `0` to disable in tests where every claim fails — an all-fail run WILL trip the breaker at 50 otherwise). Engine deadline test: `tests/whisper_engine_init.rs:122` `transcribe_respects_short_deadline` (model-gated, currently asserts the `Cancelled` variant — flipped by Task 01).
- **yt-dlp argv:** `build_yt_dlp_args` (`src/fetcher/ytdlp.rs:~97`) and `build_metadata_only_args` (:~174) construct the argv; existing unit tests in the same file assert argv contents. yt-dlp's own `--retries` defaults to 10 (observed live 2026-08-13: `Giving up after 10 retries` after 10 × 20 s connect timeouts ≈ 3.5 min stall). No classification pattern pins retry-count message text (the giving-up message lands in the `YtDlpOther` fallback; ADR-0033 patterns unaffected).
- **Version:** `Cargo.toml:3` = `0.5.0` (bump at tag time).

## Task index

| # | File | Delivers |
|---|------|----------|
| 01 | `01-engine-timeout-variant.md` | Engine returns `Timeout` (not `Cancelled`) on deadline-fired aborts; enum doc + engine test flipped |
| 02 | `02-pipeline-regression-test.md` | Pipelined regression test: a run of all-timeout transcriptions completes with a census instead of dying |
| 03 | `03-census-on-error.md` | `Process` closes the `batch_runs` row with an aborted-census marker before re-raising a run error |
| 04 | `04-ytdlp-retries-cap.md` | `--retries 3` in the yt-dlp argv (media + metadata-only) |
| 05 | `05-closeout.md` | FOLLOWUPS resolution, runbook addendum, release-notes draft, full gate |
