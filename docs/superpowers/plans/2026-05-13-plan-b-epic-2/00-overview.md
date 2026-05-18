# UU TikTok Pipeline — Plan B Epic 2: State Machine + Pipelined Orchestrator

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Each task is its own file** in this directory (`01-adr-drafts-phase1.md` … `20-bake-orchestrator.md`). Open only the task you're working on. Do NOT load the full design spec or all task files into a subagent's context — they're large and the per-task files are self-contained.

**Goal:** Make the pipeline operationally recoverable (Phase 1: minimum state machine + stale-claim sweep on the serial loop) and then throughput-optimized via a pipelined orchestrator (Phase 2: N fetch workers feeding 1 transcribe worker over a bounded mpsc channel, supervised by `tokio::JoinSet` + `tokio_util::sync::CancellationToken`). The phase split is MVP-first: Phase 1 alone answers "what happens when a fetch fails mid-batch?" without introducing concurrency complexity; Phase 2 builds on Phase 1's mutator surface unchanged.

**Architecture:** Phase 1 adds typed schema-version handling at `Store::open`, a `migrate` CLI subcommand, four new nullable columns (`last_retryable_kind`, `last_retryable_message`, `terminal_reason`, `terminal_message`), a tightened `mark_succeeded` predicate, three new mutators (`mark_retryable_failure`, `mark_terminal_failure`, `sweep_stale_claims`), and wires the sweep + a string-kind classifier into the existing `pipeline::run_serial`. Phase 2 adds `pipeline::run_pipelined` alongside `run_serial`, with N=3 async fetch workers feeding one async transcribe worker over a bounded `mpsc::Sender<(Claim, Vec<f32>, PathBuf)>` channel; all workers spawn into a `tokio::task::JoinSet` supervised by a shared `tokio_util::sync::CancellationToken`. Shutdown order is load-bearing: `token.cancel()` → drop fetch→transcribe sender → join workers → `engine.shutdown()` LAST (per AD0024).

**Tech Stack:** Rust 2021, tokio (existing), rusqlite (existing), `whisper-rs` (Plan B Epic 1), `hound` (Plan B Epic 1). **One new direct dep**: `tokio-util` with the `sync` feature (see spec correction below). No other Cargo additions.

**Reference:** Full design in `docs/superpowers/specs/2026-05-13-plan-b-epic-2-design.md`. The spec is the source of truth for "why"; **subagents implementing tasks should not need to open the spec** — each task file is self-contained, with cross-cutting context (bake numbers, FOLLOWUPS resolution map, deliberately-omits list) consolidated here in the overview.

**This is Plan B Epic 2 of 5 epics**. Epic 1 (embedded `whisper-rs` + raw confidence signals + CUDA bake) is complete on `main`. Epic 3–5 sketches live in `docs/superpowers/plans/2026-05-12-plan-b/EPIC-3-SKETCH.md` through `EPIC-5-SKETCH.md`; detailed expansions happen at the start of each epic.

---

## Spec correction (tokio-util)

Spec § "Process inheritance" line 211 states: *"`Cargo.toml` additions: none. Plan B Epic 1 already added `whisper-rs`, `hound`. Epic 2 uses `tokio_util` (transitively present) for `CancellationToken`; verify the `sync` feature is enabled."*

**This is wrong.** Verified at plan-writing time: `grep 'tokio-util' Cargo.lock` is empty. The `tokio-util` crate is **not** transitively present and `CancellationToken` lives in `tokio_util::sync::CancellationToken`, not in `tokio::sync`. Epic 2 needs an explicit `tokio-util = { version = "0.7", features = ["sync"] }` direct dependency.

Slotted as a standalone task (T13, *Cargo deps: tokio-util*) between T12 (Phase 2 ADR drafts) and the renumbered T14 (bounded `process::run` capture). Matches Epic 1's convention (T2 was a clean standalone cargo-deps task) and keeps T18's supervision-wiring focused on the 4 supervision components rather than mixing in a dep-add.

Renumbering consequence: the spec's T13–T19 become this plan's T14–T20. The plan's task index below is authoritative for numbering; the spec's task numbers are referenced parenthetically as "(spec T13)" for traceability where useful.

---

## Phase boundary discipline (AD0019)

Phase 1 produces an operationally recoverable pipeline on the serial loop. Phase 2 layers in concurrency without changing correctness. The split is execution discipline as well as task organization:

- **Phase 1 close (after T11):** the controller writes `PHASE-1-CLOSE.md` (≤1 page: what landed, current state, Phase 2 entry point) and ends. No Phase 2 work in the same controller session.
- **Phase 2 start (T12):** a fresh controller starts with the spec + `PHASE-1-CLOSE.md` + the Phase 2 task list (this plan's T12–T20 files). The Phase 1 task files are not re-loaded.

This is execution-time discipline; the plan directory holds all 20 task files at write-time so the Phase 2 controller has the briefs ready when it starts.

---

## File Structure (after Epic 2)

```
uu-tiktok/
├── Cargo.toml                # +tokio-util (sync feature) — see T13
├── src/
│   ├── main.rs               # +migrate subcommand wiring (T3); Phase 2 spawns JoinSet (T18)
│   ├── cli.rs                # +migrate subcommand (T3); +--stale-claim-threshold (T11); +--download-workers, --channel-capacity (T19)
│   ├── config.rs             # +stale_claim_threshold field (T11); +download_workers, channel_capacity fields (T19); -whisper_use_gpu, -whisper_threads (dead — T18 cleanup)
│   ├── errors.rs             # unchanged in Epic 2
│   ├── canonical.rs          # unchanged
│   ├── process.rs            # REWRITTEN streaming-bounded capture (T14); rename ring_buffer_tail → tail_excerpt; symmetric stdout cap
│   ├── state/
│   │   ├── mod.rs            # +mark_retryable_failure (T6); +mark_terminal_failure (T7, surface only); +sweep_stale_claims (T8); mark_succeeded gains WHERE predicate (T5); Store::open gains schema-version check (T2)
│   │   └── schema.rs         # +4 nullable columns (T4); SCHEMA_VERSION "1" → "2" (T4)
│   ├── fetcher/              # unchanged
│   ├── transcribe.rs         # +WhisperEngineHandle (clone-able Arc<dyn Transcriber>) + WhisperEngine::transcriber_handle() (T18). Epic 1's WhisperEngine API stays — handle is additive so the run_serial path keeps using &engine directly.
│   ├── output/               # unchanged
│   ├── ingest.rs             # unchanged
│   └── pipeline.rs           # +sweep call at top of run_serial (T9); +mark_retryable_failure replaces return Err (T9); +run_pipelined alongside run_serial (T15); +fetch_worker (T16); +transcribe_worker (T17)
└── tests/
    ├── state_*.rs            # +mark_succeeded round-trip test (T5); +mark_retryable_failure test (T6); +mark_terminal_failure test (T7); +sweep_stale_claims tests (T8); concurrent_claim rewritten with Barrier (T10)
    ├── pipeline_fakes.rs     # may grow for run_pipelined coverage (T15+)
    └── e2e_real_tools.rs     # unchanged
```

**Files NOT changed in Epic 2 (Epic 3+ or later):**

- `src/fetcher/*` — Epic 3 owns the `Acquisition::Unavailable` variant and typed-enum failure classification
- `src/errors.rs` — Epic 2 stays on string-kind mutator signatures per AD0022; Epic 3 introduces `RetryableKind` / `UnavailableReason` / `ClassifiedFailure`

**Files minimally extended in Epic 2 (additive only, no API breakage):**

- `src/transcribe.rs` — adds `WhisperEngineHandle` (clone-able Arc<dyn Transcriber>) + `WhisperEngine::transcriber_handle(&self) -> Arc<dyn Transcriber>` so the pipelined orchestrator can hand each worker a handle while `main` keeps the `WhisperEngine` owned for the load-bearing `engine.shutdown()` LAST. Epic 1's existing `impl Transcriber for WhisperEngine` stays unchanged; `run_serial` continues calling it directly. (T18.)

---

## Dependency changes (`Cargo.toml`)

Epic 2 adds one direct dep:

```toml
[dependencies]
# ... existing dependencies ...
tokio-util = { version = "0.7", features = ["sync"] }
```

Exact pinning policy: tokio-util's `0.7` line tracks tokio's `1.x` line. Pin to `0.7` (caret) rather than `=0.7.X` exact-pin — tokio-util's `sync::CancellationToken` API is stable and we want patch-level upgrades to flow.

The `sync` feature gates `CancellationToken`, the only `tokio_util` type Epic 2 uses. Other features (`codec`, `io`, `compat`) stay off.

T13 is the dedicated cargo-deps task; no other task adds dependencies.

---

## Task Conventions (inherited from Plan A + Plan B Epic 1, unchanged)

- **TDD throughout.** Each task: write the failing test, run it to confirm the failure, write minimum implementation, run to confirm pass, commit.
- **Commit per task** with a focused message. The plan supplies the message.
- **`cargo test` runs cleanly at the end of every task.** If a step adds a test that depends on later code, mark the test `#[ignore]` until the supporting code lands.
- **No `unwrap()` in non-test code** unless justified by an invariant the type system enforces.
- **Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before each commit.** If clippy fires, fix the lint or `#[allow]` it with a one-line justification comment.
- **AD0003 deviation honesty.** Every brief deviation (clippy-driven cosmetic fixes, structural choices that diverge from verbatim brief) gets prominent disclosure in commit message bodies.

## Review cycle (3-tier per AD0018, inherited from Epic 1)

| Tier | Role | What it checks |
|------|------|----------------|
| **Opus implementer** | Writes the code per the task brief | TDD discipline; brief-verbatim implementation; ADR compliance; AD0003 deviation honesty in commits |
| **Sonnet spec-compliance reviewer** | Mechanical "does this match the brief" check | Brief steps were followed verbatim modulo documented deviations; ADRs declared in the task brief are honored; AD0002 dead-code cleanup applied; clippy/fmt clean |
| **codex-advisor code-quality reviewer** | Qualitative correctness review | Subtle correctness issues; cross-file consistency; race conditions; lifetime/Send/Sync hazards; perf footguns; testing gaps |

**Per AD0018**: the orchestrator does NOT call codex-advisor directly during task reviews. The Sonnet spec-compliance reviewer delegates to codex-advisor and distills the response back into a review report. The pinned codex-advisor session UUID (`019e1b70-1ea0-75b3-83ba-9a68f63d0545` as of plan write) maintains all the Plan B context. Reuse via `codex-advisor ask`; re-init with the priming prompt at the top of the spec if the session is lost.

**Cost-quality calibration** (inherited from Epic 1):

- Opus for implementation when the task has multi-subtle interactions: T5 (mark_succeeded predicate + round-trip), T8 (sweep semantics), T9 (pipeline classifier wiring), T15 (pipeline module reshape), T16 (fetch worker), T17 (transcribe worker), T18 (supervision wiring with load-bearing shutdown ORDER).
- Sonnet for mechanically tractable tasks: T1 (ADR drafts), T2 (schema-version check), T3 (migrate subcommand), T4 (schema columns), T6 (mark_retryable_failure mutator), T7 (mark_terminal_failure surface-only), T10 (concurrent_claim test rewrite), T11 (CLI/Config plumbing), T12 (Phase 2 ADR drafts), T13 (cargo-deps), T14 (bounded capture — substantial but mechanically tractable), T19 (CLI/Config), T20 (bake runbook).

**Single-flight Agent dispatch** still applies (thermal lock from Plan A via `~/.claude/hooks/agent-lock-acquire.sh`).

---

## Architectural Decision Records (ADRs) — Epic 2

ADRs live in `docs/decisions/` and are managed via the `adg` tool (see the `using-adg` skill). The format is MADR.

Epic 2 inherits AD0001–AD0020 (Plan A through Plan B Epic 1 plus the meta-process slate). Epic 2's T1 and T12 draft and decide seven new ADRs:

| ADR | Title | Branch | Drafted in |
|---|---|---|---|
| AD0021 | Schema-version policy: hard-fail + `migrate` subcommand | feat | T1 |
| AD0022 | Minimum mutator signatures: `(kind: &str, message: &str) -> Result<usize>` | feat | T1 |
| AD0023 | Stale-claim sweep: no validation, no `attempt_count` bump, 30-min default | feat | T1 |
| AD0024 | Bug-class supervision: `JoinSet` + `CancellationToken`; **shutdown ORDER is load-bearing** | feat | T12 |
| AD0025 | Claim contention: no polling for Plan B (batch-drain semantics) | feat | T12 |
| AD0026 | Bounded `process::run` capture (streaming `VecDeque<u8>`) | feat | T12 |
| AD0027 | Orchestrator topology defaults: N=3 fetch + 1 transcribe; payload `(Claim, Vec<f32>, PathBuf)`; capacity 2 | feat | T12 |

All seven land on `feat/plan-b-epic-2` per the branch-of-record convention (CLAUDE.md and AD0018). No project-wide ADRs land in Epic 2.

**Authorship convention** (from Plan A): the controller writes ADRs. Subagents that encounter a multi-alternative decision should pause and report back as `BLOCKED` or `DONE_WITH_CONCERNS` rather than choosing silently — they lack the project context to record reasoning effectively.

**Curated dispatch** (AD0019): each per-task brief declares which ADRs are directly relevant. Subagents read those plus this overview, not all ADRs.

**Cleanup discipline** (AD0002): when a task consumes a previously-dead type or field, remove the now-stale `#[allow(dead_code)]` as part of the work. T18 explicitly cleans up `Config::whisper_use_gpu` and `Config::whisper_threads` (dead since Epic 1). Periodic backstop: `rg "allow\(dead_code\)" src/`.

---

## Task Index — Phase 1 (state machine on serial loop)

| # | File | Subject | Spec § | ADRs touched |
|---|------|---------|--------|--------------|
| 1 | [01-adr-drafts-phase1.md](./01-adr-drafts-phase1.md) | Draft + decide AD0021, AD0022, AD0023 via `adg`; audit FOLLOWUPS Epic 1 "Open Questions" entries (T8 lang_probs second-state, T9 non-finite f32, T13 cfg-gated backend assertion) and archive if confirmed | T1 + Open Q | AD0021, AD0022, AD0023 |
| 2 | [02-store-open-version-check.md](./02-store-open-version-check.md) | `Store::open` reads `meta.schema_version`; returns typed `SchemaVersionMismatch { expected, found }` error directing operator to `migrate` | T2 | AD0021 |
| 3 | [03-migrate-subcommand.md](./03-migrate-subcommand.md) | `migrate` CLI subcommand: opens DB raw (bypasses version check), runs `ALTER TABLE … ADD COLUMN` × 4 + `UPDATE meta SET value='2'` in one transaction; integration test against synthesized pre-Epic-2 fixture | T3 | AD0021 |
| 4 | [04-schema-columns.md](./04-schema-columns.md) | Schema: add 4 nullable columns (`last_retryable_kind`, `last_retryable_message`, `terminal_reason`, `terminal_message`); bump `SCHEMA_VERSION` constant `"1"` → `"2"` | T4 | AD0021 |
| 5 | [05-mark-succeeded-predicate.md](./05-mark-succeeded-predicate.md) | `mark_succeeded` gains `WHERE status='in_progress' AND claimed_by = ?` predicate; returns 0 on stale claim; round-trip test (claim → mark_succeeded → claim returns None); existing tests updated | T5 | AD0006, AD0022 |
| 6 | [06-mark-retryable-failure.md](./06-mark-retryable-failure.md) | `Store::mark_retryable_failure(video_id, worker_id, kind: &str, message: &str) -> Result<usize>` with same `WHERE status='in_progress' AND claimed_by = ?` predicate; companion stale-claim test. (Spec line 36 lists a 3-arg signature; AD0022's predicate forces a 4th `worker_id` arg — the per-task brief is authoritative.) | T6 | AD0006, AD0022 |
| 7 | [07-mark-terminal-failure.md](./07-mark-terminal-failure.md) | `Store::mark_terminal_failure(video_id, worker_id, reason: &str, message: &str) -> Result<usize>`. **SURFACE ONLY — no Epic 2 caller wires it.** Epic 3's classifier dispatcher will. Test the mutator anyway. | T7 | AD0006, AD0022 |
| 8 | [08-sweep-stale-claims.md](./08-sweep-stale-claims.md) | `Store::sweep_stale_claims(threshold: Duration) -> Result<usize>`: UPDATE `status='in_progress' AND claimed_at < (now - threshold)` → `status='pending'`; clear `claimed_by`/`claimed_at`; touch `updated_at`; no `attempt_count` bump; no artifact validation | T8 | AD0006, AD0023 |
| 9 | [09-pipeline-classifier-wiring.md](./09-pipeline-classifier-wiring.md) | Wire sweep + classifier into `run_serial`: call `store.sweep_stale_claims(opts.stale_claim_threshold)` at top before the claim loop; replace `pipeline.rs`'s `return Err(e)` with `store.mark_retryable_failure(video_id, &opts.worker_id, "FetchOrTranscribe", &format!("{e}"))` and `continue` | T9 | AD0008, AD0022 |
| 10 | [10-concurrent-claim-test-rewrite.md](./10-concurrent-claim-test-rewrite.md) | Rewrite `concurrent_claim_serializes_via_begin_immediate` (currently sequential) using `std::thread::spawn` + `std::sync::Barrier` so both threads enter `claim_next` simultaneously | T10 | — |
| 11 | [11-cli-config-stale-threshold.md](./11-cli-config-stale-threshold.md) | CLI: `--stale-claim-threshold` flag (humantime); Config: `stale_claim_threshold` field; default 30 minutes | T11 | AD0023 |

### Phase 1 exit criteria

- `cargo test` passes (with `--features test-helpers` for integration tests).
- `cargo build --release` succeeds (no `cuda` feature change).
- Smoke test against a SRC bake DB or synthesized fixture: `migrate` works (schema version flips `"1"` → `"2"`; the four new columns exist as NULL); `mark_succeeded` returns 0 on a stale-claimed row (round-trip test); stale-claim sweep recovers a synthetically-stale row.
- Existing serial loop continues to function; failures now classify as `failed_retryable` instead of aborting; operator can re-run `process` to re-claim `pending` rows.

### Why Phase 1 is independently shippable

After Phase 1, the operator workflow is:

1. `uu-tiktok migrate` (one-time per Plan A → Epic 2 upgrade).
2. `uu-tiktok process` (serial loop with classification).
3. On failure, the row is `failed_retryable` with `last_retryable_kind` + `last_retryable_message` visible.
4. Re-run `process` to re-claim `pending` rows. (Epic 5's `requeue-retryables` will be the operator gesture to move `failed_retryable` back to `pending`.)
5. Process crash mid-batch is recovered via sweep on next `process` startup (after threshold elapses).

This is operationally recoverable. Phase 2 adds throughput; it does not change correctness.

---

## Task Index — Phase 2 (pipelined orchestrator)

| # | File | Subject | Spec § | ADRs touched |
|---|------|---------|--------|--------------|
| 12 | [12-adr-drafts-phase2.md](./12-adr-drafts-phase2.md) | Draft + decide AD0024, AD0025, AD0026, AD0027 via `adg` | T12 | AD0024, AD0025, AD0026, AD0027 |
| 13 | [13-cargo-deps-tokio-util.md](./13-cargo-deps-tokio-util.md) | Add `tokio-util = { version = "0.7", features = ["sync"] }` to `Cargo.toml`. **NEW SLOT** — spec line 211 wrongly claimed transitive presence | (new) | — |
| 14 | [14-bounded-process-capture.md](./14-bounded-process-capture.md) | Bounded `process::run` capture: replace `read_to_end` with streaming reader maintaining `VecDeque<u8>` of size `stderr_capture_bytes`; symmetric `stdout_capture_bytes`; rename `ring_buffer_tail` → `tail_excerpt`; Tier 2 test against synthetic stderr flood. **Full scope** (perf-tweaks worktree has only doc commits as of plan write — no impl) | T13 (spec) | AD0026 |
| 15 | [15-pipeline-module-reshape.md](./15-pipeline-module-reshape.md) | Pipeline module reshape: `pipeline::run_pipelined` alongside `run_serial`; share `process_one` helpers where useful; module-layout choice (single file vs `pipeline/{mod,serial,pipelined}.rs`) decided at task time per existing-code-pattern discipline | T14 (spec) | — |
| 16 | [16-fetch-worker.md](./16-fetch-worker.md) | `async fn fetch_worker(token, store, fetcher, sender, opts) -> Result<()>`: loop `claim_next → fetcher.acquire → audio::decode_wav → send (Claim, Vec<f32>, PathBuf)`; exit on `claim_next == None` (no polling per AD0025); on retryable error: `mark_retryable_failure` and continue; on Bug: return Err | T15 (spec) | AD0024, AD0025, AD0027 |
| 17 | [17-transcribe-worker.md](./17-transcribe-worker.md) | `async fn transcribe_worker(token, receiver, engine, store, opts) -> Result<()>`: loops `tokio::select! { _ = token.cancelled() => break, Some((claim, samples, wav)) = receiver.recv() => transcribe_one(...) }`; Engine Bug → return Err | T16 (spec) | AD0008, AD0024, AD0027 |
| 18 | [18-supervision-wiring.md](./18-supervision-wiring.md) | Supervision in `main::Process`: spawn N fetch workers + 1 transcribe worker into `tokio::task::JoinSet` with shared `CancellationToken`; loop `join_set.join_next()`; on first `Err`/panic, `token.cancel()` and drain. **Load-bearing shutdown ORDER per AD0024**: `token.cancel()` → drop fetch→transcribe sender → join workers → `engine.shutdown()` LAST. Also: remove dead `Config::whisper_use_gpu` + `Config::whisper_threads` per FOLLOWUPS | T17 (spec) | AD0002, AD0024 |
| 19 | [19-cli-config-orchestrator.md](./19-cli-config-orchestrator.md) | CLI: `--download-workers` (default 3, validate ≥ 1), `--channel-capacity` (default 2, validate ≥ 1); Config fields plumbed through | T18 (spec) | AD0027 |
| 20 | [20-bake-orchestrator.md](./20-bake-orchestrator.md) | Bake/operational validation on SRC A10: run N=3 vs N=1 against `news_orgs` fixture; capture throughput delta; coordinated-shutdown drill (`kill -KILL` mid-batch, restart, confirm sweep recovers). Append to `docs/SRC-BAKE-NOTES.md` | T19 (spec) | AD0027 |

### Phase 2 exit criteria

- `cargo test` (+`--features cuda,test-helpers` for the relevant integration tests) passes.
- Bake against the SRC A10 workspace shows N=3 outperforms serial-loop wallclock by ~3.5× on `news_orgs` fixture (per spec § "Throughput math").
- Coordinated shutdown drill passes: `kill -KILL` mid-batch leaves DB consistent (some rows `in_progress`, recovered by sweep on next startup).
- Bounded `process::run` capture verified against synthetic stderr-flood test (no unbounded memory growth).

---

## What Epic 2 Deliberately Omits

Listed so the implementer doesn't accidentally pull them in:

- **Typed-enum failure classification** (`RetryableKind`, `UnavailableReason`, `ClassifiedFailure`) — Epic 3. Epic 2's `(kind: &str, msg: &str)` mutators are the surface Epic 3 composes typed enums on top of.
- **`Acquisition::Unavailable` variant** — Epic 3 owns entirely. Epic 2's orchestrator keeps the single-arm match against `Acquisition::AudioFile(p)`.
- **yt-dlp residual no-audio retry** (FOLLOWUPS bake finding) — Epic 3 (`RetryableFailure::NoAudioStream` + classifier-driven retry).
- **`From<RunError> for FetchError` refinement / `status.code()` signal capture** — Epic 3.
- **Time-window filter / DDP timezone resolution / `recompute-window` subcommand / `status` subcommand** — Epic 4.
- **Multi-fetcher provenance fix / bin-lib reassessment / `requeue-retryables` / `reset-stale-claims`** — Epic 5.
- **Validate-and-mark-succeeded stale-recovery optimization** — Plan C if measured to matter.
- **Multi-state / multi-GPU implementation** — Plan C / production grant.

---

## FOLLOWUPS audit (Epic 2 resolution map)

Per AD0020, resolved entries move from `docs/followups/epic-2.md` to `docs/archive/followups-resolved.md` (with resolving commit SHAs) at Epic 2 close.

**Resolved by Epic 2:**

- `Store::open` schema-version not read (T7 review) — Phase 1 T2 / T3
- `concurrent_claim_serializes_via_begin_immediate` doesn't race (T10 review) — Phase 1 T10
- `mark_succeeded` doesn't require `status='in_progress'` (T10 review) — Phase 1 T5
- Missing round-trip test (T10 review) — Phase 1 T5
- `claim_next` polling semantics (T10 review) — Phase 2 T12 / AD0025
- `process::run` unbounded stdout/stderr (T6 review) — Phase 2 T14
- `ring_buffer_tail` misnamed (T6 review) — Phase 2 T14 rename
- WhisperEngine teardown can hang (T5 codex review) — Phase 2 T18 / AD0024 shutdown order
- `Config::whisper_use_gpu` + `Config::whisper_threads` dead fields — Phase 2 T18 cleanup

**Stays in `docs/followups/epic-3.md`:**

- `From<RunError> for FetchError` collapses Spawn/Io
- `status.code().unwrap_or(-1)` loses signal info
- `YtDlpFetcher::acquire` error mapping
- `claim_next` / `mark_succeeded` inner stmts lack `with_context`
- Residual yt-dlp no-audio retry (bake finding)

**Stays for Epic 4 / 5 / Plan C:** unchanged per `docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md`.

---

## Bake empirical anchors (rationale for AD0027)

From `docs/SRC-BAKE-NOTES.md` (n=8 on `news_orgs` fixture, post-Finding-1 + Finding-2 fixes):

- **Sequential per-video budget** on `large-v3-turbo-q5_0`: avg ~6.75s/video. Fetch range 1.7–21s (one 21s outlier — 39% of an 8-video wallclock). Transcribe range 0.27–2.0s (mostly sub-second; 49× realtime at GPU floor). Model load 6.1s amortizes to zero in a long-running daemon.
- **Per-engine resource envelope**: ~1.25 GB memory (single state + lang_state per AD0012); 573 MB model VRAM. Comfortable on A10's 24 GB; CPU envelope 4–8 cores; ffmpeg subprocess spawn per fetch.

Steady-state throughput = `min(N / avg_fetch, 1 / avg_transcribe)`.

| N fetch workers | Fetch throughput | Transcribe throughput | Bottleneck | Avg s/video |
|---|---|---|---|---|
| 1 (serial) | 0.18 v/s | 1 v/s | fetch | ~6.5 |
| 2 | 0.36 v/s | 1 v/s | fetch | ~2.75 |
| **3** | **0.55 v/s** | **1 v/s** | **fetch** | **~1.83** |
| 4 | 0.73 v/s | 1 v/s | fetch | ~1.38 |
| 6+ | 1.09+ v/s | 1 v/s | transcribe | ≈1 |

**N=3 sits at the curve-flattening point.** With N=2, one stuck fetch worker halves effective fetch capacity; with N=3, it drops by only a third. CPU envelope: N=3 spawns ~6 concurrent subprocesses (each fetch is yt-dlp + ffmpeg postprocessor), comfortable on a 4–8 core workspace; N≥6 risks CPU contention.

**Channel sizing:** at N=3 + capacity 2 + 1 in-flight on transcribe + 3 active fetches, peak ~6 items in flight × ~3 MB per WAV ≈ 18 MB peak channel memory. Negligible against the 24 GB A10 budget. Capacity 2 provides backpressure smoothing for transcribe's small variance.

---

## Self-Review Checklist (run by author after writing)

**Spec coverage:** every Phase 1 spec task (T1–T11) and Phase 2 spec task (T12–T19) maps to a per-task file in this plan, modulo the T13 cargo-deps insertion that renumbers Phase 2 (T13 spec → T14 plan, etc.). The seven ADRs (AD0021–AD0027) are all named with their drafting tasks. The "deliberately omits" list is mirrored verbatim from the spec.

**Placeholder scan:** no "TBD" / "TODO (epic 2)" / "implement later" anywhere. The T7 surface-only note is explicit (Epic 3's classifier is the first caller), not a placeholder. The ADR slate names all seven ADRs with one-line decision summaries.

**Type / API consistency:** Mutator signatures `(video_id, worker_id, kind, message) -> Result<usize>` consistent across T5/T6/T7 (mark_succeeded gains worker_id in T5; mark_retryable_failure and mark_terminal_failure follow the same shape per AD0022). `tokio_util::sync::CancellationToken` referenced consistently in T16/T17/T18/AD0024. `mpsc::Sender<FetchedItem>` (payload `(Claim, Vec<f32>, PathBuf)`) consistent in T16/T17/AD0027.

**Scope:** 20 tasks total (11 + 9). Each produces a meaningful TDD-able increment. Phase 1 alone is operationally recoverable; Phase 2 adds throughput. No Epic 3+ work pulled in.

**Spec correction surfaced:** tokio-util cargo-deps gap documented in its own section; T13 standalone task; renumbering footnote in the Phase 2 table.

**Phase boundary discipline:** Per AD0019, Phase 1 close writes `PHASE-1-CLOSE.md` and ends; Phase 2 starts a fresh controller with the spec + close-out + Phase 2 task list. Discipline is execution-time; the plan directory holds all 20 files at write-time.
