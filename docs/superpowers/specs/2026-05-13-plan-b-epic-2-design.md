# Plan B Epic 2 — State machine + pipelined orchestrator (design)

**Status:** decided (design); user review pending in next session before `writing-plans` transition.
**Author:** Danielle McCool (with Claude Opus 4.7 as drafting collaborator).
**Date:** 2026-05-13.
**Inputs:** Plan B Epic 1 bake numbers (`docs/SRC-BAKE-NOTES.md`), Plan B Epic 2 sketch (`docs/superpowers/plans/2026-05-12-plan-b/EPIC-2-SKETCH.md`), Plan B Epic 1's "what Epic 1 deliberately omits" list, codex-advisor session `019e1b70-1ea0-75b3-83ba-9a68f63d0545`.

## Goal

Plan B Epic 2 makes the pipeline **operationally recoverable** (Phase 1: minimum state machine + stale-claim sweep on the serial loop) and then **throughput-optimized via a pipelined orchestrator** (Phase 2: N fetch workers feeding 1 transcribe worker, bounded mpsc, JoinSet supervision). The split is MVP-first: Phase 1 alone answers the operator question "what happens when a fetch fails mid-batch?" without introducing concurrency complexity; Phase 2 builds on Phase 1's mutator surface unchanged.

## Phase structure (MVP-first split)

| Phase | Scope | Built on | Exit |
|-------|-------|----------|------|
| **Phase 1** | State machine on serial loop: schema-version check + `migrate` subcommand + new retryable/terminal columns + `mark_succeeded` WHERE-predicate + minimum mutators + stale-claim sweep, wired into existing `pipeline::run_serial` | Plan B Epic 1 (Engine API + serial loop) | Operator can run `process`, observe failure classifications in DB, re-run to retry transients. Serial loop unchanged in topology. |
| **Phase 2** | Pipelined orchestrator: N=3 fetch workers + 1 transcribe worker + bounded mpsc + tokio JoinSet supervision + bounded `process::run` capture | Phase 1's mutator surface | N=3 default outperforms serial by ~3.5×; coordinated shutdown drill leaves DB consistent. |

**Why split this way:** the original sketch proposed three sub-phases (schema-version → state machine → orchestrator). Empirical reframing during brainstorm: the schema-version + state-machine work is the operational-MVP slice that's exercisable end-to-end on the serial loop. Phase 2's concurrency work then becomes a topology change rather than a correctness change. Cleaner blast radius.

Phase boundary discipline per 0019: at Phase 1 close the controller writes `PHASE-1-CLOSE.md` (≤1 page: what landed, current state, Phase 2 entry point) and ends; Phase 2 starts a fresh controller with the spec + close-out doc + Phase 2 task list.

---

## Phase 1 — State machine on serial loop

### Task list

| # | Task | Resolves |
|---|------|----------|
| T1 | ADR drafts for 0021 (schema-version policy), 0022 (mutator signatures), 0023 (stale-sweep semantics) via `scripts/adr-fill`. | — |
| T2 | `Store::open` schema-version check; returns typed `SchemaVersionMismatch { expected, found }` error with operator-readable instruction directing them to `migrate`. | FOLLOWUPS T7 |
| T3 | `migrate` CLI subcommand — opens DB raw (bypasses the version check), runs `ALTER TABLE videos ADD COLUMN ... NULL` × 4 + `UPDATE meta SET value='2'` inside one transaction. Integration test against synthesized pre-Epic-2 fixture DB. | — |
| T4 | Schema: new nullable columns `last_retryable_kind`, `last_retryable_message`, `terminal_reason`, `terminal_message`; `SCHEMA_VERSION` bumps `"1"` → `"2"`. | — |
| T5 | `Store::mark_succeeded` gains `WHERE status='in_progress' AND claimed_by = ?` predicate; returns 0 on stale claim. Updates existing tests; adds round-trip test (claim → mark_succeeded → claim returns None). | FOLLOWUPS T10 (mark_succeeded predicate + missing round-trip) |
| T6 | `Store::mark_retryable_failure(video_id, kind: &str, message: &str) -> Result<usize>` per 0006. Same `WHERE status='in_progress' AND claimed_by = ?` predicate. Companion test for stale-claim case. | — |
| T7 | `Store::mark_terminal_failure(video_id, reason: &str, message: &str) -> Result<usize>`. **SURFACE ONLY — no Epic 2 caller wires it.** Epic 3's classifier will. Test the mutator anyway. | — |
| T8 | `Store::sweep_stale_claims(threshold: Duration) -> Result<usize>` — UPDATE rows with `status='in_progress' AND claimed_at < (now - threshold)` to `status='pending'`, clear `claimed_by`/`claimed_at`, `updated_at` touched. No `attempt_count` bump. No artifact validation. | — |
| T9 | Wire sweep + classifier into `run_serial`: call `store.sweep_stale_claims(opts.stale_claim_threshold)` at top of `run_serial` before the claim loop; replace `pipeline.rs:66`'s `return Err(e)` with `store.mark_retryable_failure(video_id, "FetchOrTranscribe", &format!("{e}"))` and `continue`. Epic 3 will replace the string-based call with classifier dispatch. | — |
| T10 | Rewrite `concurrent_claim_serializes_via_begin_immediate` (currently sequential) using `std::thread::spawn` + `std::sync::Barrier` so both threads enter `claim_next` simultaneously. | FOLLOWUPS T10 (test rewrite) |
| T11 | CLI + Config plumbing: `--stale-claim-threshold` flag, `Config::stale_claim_threshold` field, default 30 minutes. | — |

### Phase 1 exit criteria

- `cargo test` passes (with `--features test-helpers` for integration tests).
- `cargo build --release` succeeds.
- Smoke test against SRC bake DB: `migrate` works (status → version 2, new columns NULL); `mark_succeeded` returns 0 on a stale-claimed row; stale-claim sweep recovers a synthetically-stale row.
- Existing serial loop continues to function; failures now classify as `failed_retryable` instead of aborting.

### Why Phase 1 is independently shippable

After Phase 1, the operator workflow is:

1. `uu-tiktok migrate` (one-time per Plan A → Epic 2 upgrade).
2. `uu-tiktok process` (serial loop with classification).
3. On failure, the row is `failed_retryable` with `last_retryable_kind` + `last_retryable_message` visible.
4. Re-run `process` to re-claim `pending` rows. (Epic 5's `requeue-retryables` will be the operator gesture to move `failed_retryable` back to `pending`.)
5. Process crash mid-batch is recovered via sweep on next `process` startup (after threshold elapses).

This is operationally recoverable. Phase 2 adds throughput; it does not change correctness.

---

## Phase 2 — Pipelined orchestrator

### Task list

| # | Task | Resolves |
|---|------|----------|
| T12 | ADR drafts for 0024 (Bug-class supervision + shutdown ORDER), 0025 (no polling for Plan B), 0026 (bounded `process::run` capture), 0027 (orchestrator topology defaults). | — |
| T13 | Bounded `process::run` capture — replace `read_to_end` (`src/process.rs:118–122`) with streaming reader maintaining a `VecDeque<u8>` of size `stderr_capture_bytes`. Symmetric `stdout_capture_bytes`. Rename `ring_buffer_tail` → `tail_excerpt`. Tier 2 test against synthetic stderr flood. **Note:** an "efficiency worktree" run by the operator may land the bounded buffer ahead of Phase 2. If so, T13's scope reduces to rename + symmetric stdout cap + verify the existing Tier 2 test passes after rename. 0026 records the decision either way. | FOLLOWUPS T6 (bounded capture + `ring_buffer_tail` rename) |
| T14 | Pipeline module reshape — `pipeline::run_pipelined` alongside `run_serial`. Match Plan A's module style (single file or submodule split — pick during T14 per existing-code-pattern discipline). | — |
| T15 | `async fn fetch_worker(token: CancellationToken, store: SharedStore, fetcher: &dyn VideoFetcher, sender: mpsc::Sender<FetchedItem>, opts: &ProcessOptions) -> Result<()>`. Loops: claim_next → fetcher.acquire → audio::decode_wav → send `(Claim, Vec<f32>, PathBuf)`. Exits on `claim_next == None` (no polling per 0025). On retryable error: `mark_retryable_failure` and continue. On Bug: return Err. | — |
| T16 | `async fn transcribe_worker(token: CancellationToken, receiver: mpsc::Receiver<FetchedItem>, engine: &WhisperEngine, store: SharedStore, opts: &ProcessOptions) -> Result<()>`. Loops `tokio::select! { _ = token.cancelled() => break, Some((claim, samples, wav)) = receiver.recv() => transcribe_one(...) }`. Engine Bug → return Err. | — |
| T17 | Supervision wiring in `main::Process` — spawn N fetch workers + 1 transcribe worker into `tokio::task::JoinSet` with a shared `tokio_util::sync::CancellationToken`. Loop `join_set.join_next()`; on first `Err` or panic, `token.cancel()` and drain remaining tasks. **0024 shutdown order** (load-bearing): `token.cancel()` → drop fetch→transcribe sender → join workers → `engine.shutdown()` last. Without this order the engine worker parks on `blocking_recv` until process exit (per codex-advisor's review during the brainstorm). Exit 1 on Bug, 0 on clean drain. **Also: remove dead `Config::whisper_use_gpu` + `Config::whisper_threads`** per FOLLOWUPS line 1059. | FOLLOWUPS WhisperEngine teardown can hang + Config dead fields |
| T18 | CLI + Config plumbing: `--download-workers` (default 3, validate ≥ 1), `--channel-capacity` (default 2, validate ≥ 1). | — |
| T19 | Bake/operational validation on SRC A10: run N=3 vs N=1 against `news_orgs` fixture; capture throughput delta; coordinated-shutdown drill (kill -KILL mid-batch, restart, confirm sweep recovers). Append to `docs/SRC-BAKE-NOTES.md`. | — |

### Phase 2 exit criteria

- `cargo test` (+`--features cuda,test-helpers` for the relevant integration tests) passes.
- Bake against the SRC A10 workspace shows N=3 outperforms serial-loop wallclock by ~3.5× on `news_orgs` fixture.
- Coordinated shutdown drill passes: `kill -KILL` mid-batch leaves DB consistent (some rows `in_progress`, recovered by sweep on next startup).
- Bounded `process::run` capture verified against synthetic stderr-flood test.

---

## ADR slate (0021–0027)

Seven new feature ADRs. All land on the `feat/plan-b-epic-2` branch (per branch-of-record convention in CLAUDE.md and 0018).

| ADR | Title | Phase | Decision summary |
|-----|-------|-------|-------------------|
| **0021** | Schema-version policy: hard-fail + `migrate` subcommand | 1 / T1 | `Store::open` returns a typed error on `meta.schema_version != SCHEMA_VERSION`; the new `migrate` CLI subcommand runs ALTER TABLE + UPDATE meta inside one transaction. Preserves Plan A bake data; sets a precedent for Epic 3+ schema changes. Rejected: auto-migrate (silent drift), log+warn (operationally invisible), wipe-and-re-ingest (loses classification history). |
| **0022** | Minimum mutator signatures: `(kind: &str, message: &str)` returning `Result<usize>` per 0006 | 1 / T1 | `mark_retryable_failure` and `mark_terminal_failure` take a string-typed kind/reason. Epic 3 will introduce typed enums via `ClassifiedFailure` and a wider signature; Epic 2's surface composes cleanly with that. No extra "context" field — Epic 3 introduces structure when it has a structural reason. |
| **0023** | Stale-claim sweep: no validation, no `attempt_count` bump, redo cost accepted | 1 / T1 | Rows with `status='in_progress' AND claimed_at < (now - threshold)` flip to `pending`. No artifact-presence check (0008's "in_progress + complete artifacts" state accepted). No `attempt_count` increment (mixing operator-recovery semantics with application-retry semantics would corrupt Epic 3's retry policy). Default threshold 30 minutes (well above bake worst-case ~25s, prevents stealing from healthy peers in any future multi-instance scenario). Validate-and-mark-succeeded optimization explicitly deferred to Plan C. |
| **0024** | Bug-class supervision: `tokio::JoinSet` + `tokio_util::sync::CancellationToken` + `tokio::select!` at engine call; **shutdown ORDER is load-bearing** | 2 / T12 | All fetch workers + transcribe worker spawn into a `JoinSet`. Shared `CancellationToken`. main loops on `join_set.join_next()`; first `Err(Bug)` or panic fires `token.cancel()` and drains. Transcribe worker wraps `engine.transcribe()` in `tokio::select! { _ = token.cancelled() => ..., r = engine.transcribe(...) => r }`. CancellationToken.cancel() drops the engine future → `CancelOnDrop` fires → abort_callback aborts whisper inference within milliseconds (composes with Epic 1's 0012 cancellation mechanism). **Shutdown order: `token.cancel()` → drop fetch→transcribe sender → join workers → `engine.shutdown()` LAST.** Without this, the engine worker parks on `blocking_recv` until process exit (codex-advisor's contribution during brainstorm). Process exits 1 on Bug, 0 on clean drain. |
| **0025** | Claim contention: no polling for Plan B | 2 / T12 | Fetch workers exit on `claim_next == None`; the empty-pool case at process start = "done." **Explicit deviation from EPIC-2-SKETCH's polling proposal** (which suggested 100ms–2s backoff). Plan B is batch-drain: ingest is a separate phase before `process`; the pool is frozen at process startup. Polling would be busywork. Polling deferred to Plan C / daemon mode where ingest is live. |
| **0026** | Bounded `process::run` capture | 2 / T12 | Replace `read_to_end` with streaming reader maintaining a `VecDeque<u8>` of size `stderr_capture_bytes`. Symmetric `stdout_capture_bytes` for defense-in-depth (yt-dlp writes audio to a file so stdout is small, but a misbehaving tool could flood). Rename `ring_buffer_tail` → `tail_excerpt` (FOLLOWUPS T6 noted the original name implied ring-buffer semantics that the function doesn't have). The "efficiency worktree" run by the operator may land this fix before Phase 2 begins; 0026 records the decision regardless of which commit lands the code. |
| **0027** | Orchestrator topology defaults: N=3 fetch + 1 transcribe; channel payload `(Claim, Vec<f32>, PathBuf)`; capacity 2; flag-tunable | 2 / T12 | Empirically derived from bake (see § "Bake empirical anchors"). N=3 is the curve-flattening point with comfortable outlier tolerance. Channel payload includes the decoded `Vec<f32>` so fetch workers do WAV decode in parallel (~50–100ms per clip), keeping the transcribe path lean; PathBuf rides through for cleanup after `mark_succeeded` per 0008. Channel capacity 2 buffers transcribe's small variance without committing meaningful memory. All defaults flag-tunable via `--download-workers` and `--channel-capacity`. |

---

## Bake empirical anchors (rationale for 0027)

Inputs from `docs/SRC-BAKE-NOTES.md` (n=8 on `news_orgs` fixture, post-Finding-1 + Finding-2 fixes):

- **Sequential per-video budget** on `large-v3-turbo-q5_0`: avg ~6.75s/video. Fetch range 1.7–21s (one 21s outlier — 39% of an 8-video wallclock). Transcribe range 0.27–2.0s (mostly sub-second; 49× realtime at GPU floor). Model load 6.1s amortizes to zero in a long-running daemon.
- **Per-engine resource envelope**: ~1.25 GB memory (single state + lang_state per 0012); 573 MB model VRAM. Comfortable on A10's 24 GB; CPU envelope 4–8 cores; ffmpeg subprocess spawn per fetch.

### Throughput math (N fetch workers + 1 transcribe worker)

Steady-state throughput = `min(N / avg_fetch, 1 / avg_transcribe)`.

| N fetch workers | Fetch throughput | Transcribe throughput | Bottleneck | Avg s/video |
|---|---|---|---|---|
| 1 (serial) | 0.18 v/s | 1 v/s | fetch | ~6.5 |
| 2 | 0.36 v/s | 1 v/s | fetch | ~2.75 |
| **3** | **0.55 v/s** | **1 v/s** | **fetch** | **~1.83** |
| 4 | 0.73 v/s | 1 v/s | fetch | ~1.38 |
| 6+ | 1.09+ v/s | 1 v/s | transcribe | ≈1 |

N=3 sits at the curve-flattening point. **Outlier handling:** with N=2, one stuck fetch worker halves effective fetch capacity; with N=3, it drops by only a third. **CPU envelope:** N=3 spawns ~6 concurrent subprocesses (each fetch is yt-dlp + ffmpeg postprocessor), comfortable on a 4–8 core workspace; N≥6 risks CPU contention.

### Channel sizing math

At N=3 fetch + capacity 2 + 1 in-flight on transcribe + 3 active fetches: peak ~6 items in flight, ~6 × ~3 MB per WAV ≈ 18 MB peak channel memory. Negligible against the 24 GB A10 budget. Capacity 2 provides backpressure smoothing for transcribe's small variance (transcribe is ~1s stable; never accumulates).

---

## FOLLOWUPS audit (Epic 2 resolution map)

Per 0020, resolved entries move from `docs/followups/epic-2.md` to `docs/archive/followups-resolved.md` (with resolving commit SHAs) at Epic 2 close.

**Resolved by Epic 2:**

- `Store::open` schema-version not read (T7 review) — Phase 1 T2 / T3
- `concurrent_claim_serializes_via_begin_immediate` doesn't race (T10 review) — Phase 1 T10
- `mark_succeeded` doesn't require status='in_progress' (T10 review) — Phase 1 T5
- Missing round-trip test (T10 review) — Phase 1 T5
- `claim_next` polling semantics (T10 review) — Phase 2 T12 / 0025
- `process::run` unbounded stdout/stderr (T6 review) — Phase 2 T13
- `ring_buffer_tail` misnamed (T6 review) — Phase 2 T13 rename
- WhisperEngine teardown can hang (T5 codex review) — Phase 2 T17 / 0024 shutdown order
- `Config::whisper_use_gpu` + `Config::whisper_threads` dead fields — Phase 2 T17 cleanup

**Stays in `docs/followups/epic-3.md`:**

- `From<RunError> for FetchError` collapses Spawn/Io (T6) — Epic 3's classifier covers it
- `status.code().unwrap_or(-1)` loses signal info (T6) — Epic 3 adds `signal: Option<i32>` field
- `YtDlpFetcher::acquire` error mapping (T11) — Epic 3 classifier
- `claim_next` / `mark_succeeded` inner stmts lack `with_context` (T10) — Epic 3 bundle with error restructure
- Residual yt-dlp no-audio retry (bake finding) — Epic 3 `RetryableFailure::NoAudioStream` + classifier-driven retry

**Stays for Epic 4 / 5 / Plan C:** unchanged per `docs/superpowers/plans/2026-05-12-plan-b/EPIC-5-SKETCH.md` resolution map.

---

## Epic 2 deliberately omits

Listed so the implementer doesn't accidentally pull them in:

- **Typed-enum failure classification** (`RetryableKind`, `UnavailableReason`, `ClassifiedFailure`) — Epic 3. Epic 2's `(kind: &str, msg: &str)` mutators are the surface Epic 3 composes typed enums on top of.
- **`Acquisition::Unavailable` variant** — Epic 3 owns entirely. Epic 2's orchestrator keeps the single-arm match against `Acquisition::AudioFile(p)`. Epic 3 introduces the variant when it has the typed `UnavailableReason` to attach.
- **yt-dlp residual no-audio retry** (FOLLOWUPS bake finding) — Epic 3. The bake observation tempts Epic 2 scope but the right home is Epic 3's classifier-driven retry.
- **`From<RunError> for FetchError` refinement / `status.code()` signal capture** — Epic 3.
- **Time-window filter / DDP timezone resolution / `status` subcommand** — Epic 4.
- **Multi-fetcher provenance fix / bin-lib reassessment / `requeue-retryables` / `reset-stale-claims`** — Epic 5.
- **Validate-and-mark-succeeded stale-recovery optimization** — Plan C if measured to matter.
- **Multi-state / multi-GPU implementation** — Plan C / production grant.

---

## Minimum-scope clarifications

Two design-time choices the reviewer should expect to see:

1. **`mark_terminal_failure` (Phase 1 T7) is added as surface only — no Epic 2 caller wires it.** Epic 3's classifier dispatcher is the first caller. This is intentional: landing the surface in Epic 2 means Epic 3 is a classifier-add task, not a mutator-add task. Without this note, a Phase 1 reviewer would flag T7 as dead code.

2. **The shutdown order in 0024 is load-bearing.** Codex-advisor specifically flagged that the natural sequence (cancel → join → drop senders) deadlocks the engine worker on `blocking_recv`. The correct sequence is `token.cancel()` → drop fetch→transcribe sender → join workers → `engine.shutdown()` LAST. The ADR text must encode this order, not just the components. Without it the spec describes a hang.

---

## Coordination check-ins (efficiency worktree)

A parallel "efficiency-tweaks" worktree is in flight (separate from Epic 2). Conflict-relevant items:

- **Item #5 (bounded `process::run` capture)**: planned to land BEFORE Phase 2 begins. T13's scope reduces to rename (`ring_buffer_tail` → `tail_excerpt`) + symmetric `stdout_capture_bytes`. 0026 still records the decision; commit SHA reference notes whether the bounded buffer landed on the worktree or in T13.
- **Item #3 partial (compact JSON in `pipeline.rs`)**: one-line `to_vec_pretty` → `to_vec`. Either lands on the worktree (and Phase 2 inherits) or T9 does it inline.
- **Item #3 full (drop token text from JSON)**: deferred past Epic 2. Would require an 0010 amendment (the pass-through rule made `id`+`text` load-bearing for filtering specials).
- **Items #1, #2, #4, #6**: independent of Epic 2; land freely.

---

## Process inheritance

All project-wide ADRs apply:

- **0001** — per-task file split (Epic 2's plan directory: `docs/superpowers/plans/2026-05-13-plan-b-epic-2/`).
- **0002** — dead-code suppression strategy; cleanup-on-consumption.
- **0003** — deviation honesty in commits (every brief-verbatim divergence disclosed prominently).
- **0005** — `test-helpers` Cargo feature for library items needed by integration tests.
- **0006** — mutator signatures return `Result<usize>` (Phase 1 T5/T6/T7/T8 all conform).
- **0007** — stats counter convention (input-side counters with verb-named fields).
- **0008** — pipeline writes artifacts before `mark_succeeded` (Phase 1's classifier dispatch preserves this; Phase 2's pipelined orchestrator preserves it too — fetch workers write nothing, transcribe worker writes artifacts then calls `mark_succeeded`).
- **0009–0017** — Plan B Epic 1 feature ADRs (Engine API, raw signals, cancellation, GPU verification, audio invariants, non-parallel whisper, parallelism architecture, done-contract).
- **0018** — three-tier review protocol with codex-advisor delegated via Sonnet reviewer.
- **0019** — subagent report format (≤250-word STATUS/SUMMARY/CHANGED/DEVIATIONS) and phase-boundary controller restart with `PHASE-N-CLOSE.md` handoff.
- **0020** — FOLLOWUPS document structure and lifecycle.

**`Cargo.toml` additions:** none. Plan B Epic 1 already added `whisper-rs`, `hound`. Epic 2 uses `tokio_util` (transitively present) for `CancellationToken`; verify the `sync` feature is enabled.

---

## Operational outcomes after Epic 2 ships

- Operator runs `process`; failures classify as retryable (string-kind, Epic 3 will type) or terminal (no Epic 2 caller yet); transient failures are visible in DB as `failed_retryable` rows with `last_retryable_kind` + `last_retryable_message`.
- Re-running `process` re-claims `pending` rows but not `failed_retryable` rows (Epic 5's `requeue-retryables` is the operator gesture to move retryables back to pending).
- Process crash mid-batch is recovered via stale-claim sweep on next startup (30-min threshold; configurable).
- N=3 default throughput is ~3.5× the serial-loop wallclock at `news_orgs` fixture scale.
- Coordinated shutdown drill (kill -KILL mid-batch, then restart) leaves DB consistent: in-flight rows stay `in_progress` until sweep recovers them.
- The architecture is future-proofed for multi-state per 0016 (single transcribe worker today; Plan C may add `WhisperPool` of N engines without touching the orchestrator's public shape).

---

## Open questions

Three FOLLOWUPS entries are classified as Active in `docs/followups/epic-2.md` with "Status unverified against shipped Epic 1 code — confirm before archiving" notes. They should be verified against current `src/` code state during Phase 1 dispatch; if shipped correctly in Epic 1, archive them then (the new session's first task list should include this audit):

- T8 lang_probs second-state — confirm `src/transcribe.rs` allocates `lang_state` as documented.
- T13 cfg-gated backend assertion — confirm the `cfg(feature = "cuda")`-gated `BackendMismatch` lands.
- T9 non-finite f32 rejection — confirm `extract_segments` rejects non-finite values.

No expected design impact; just bookkeeping.

---

## Self-review checklist (run by author after writing)

**Spec coverage:** every brainstorm decision (sub-phase ordering, N=3 worker count, hard-fail schema policy, no-validation stale-sweep with 30-min default, channel payload `(Claim, Vec<f32>, PathBuf)`, CancellationToken + JoinSet supervision with codex's shutdown ORDER, mutator signatures `(kind, msg)`, no polling for Plan B, bounded `process::run` capture) is captured with its rationale. FOLLOWUPS audit maps every active entry to either Epic 2 or a downstream epic. Coordination check-ins for the efficiency worktree note conflict-relevant items.

**Placeholder scan:** no "TBD", "TODO (epic 2)", or "implement later" anywhere. The T7 dead-code note is explicit, not a placeholder. The ADR slate names all seven ADRs with their decisions, not stubs.

**Type / API consistency:** Mutator signatures `(video_id, kind, message) -> Result<usize>` consistent across T6/T7. `tokio_util::sync::CancellationToken` referenced consistently in T15/T16/T17/0024. `mpsc::Sender<FetchedItem>` (payload `(Claim, Vec<f32>, PathBuf)`) consistent in T15/T16/0027.

**Scope:** 19 tasks total (11 + 8). Each produces a meaningful TDD-able increment. Phase 1 alone is operationally recoverable; Phase 2 adds throughput. No Epic 3+ work pulled in.

**Ambiguity:** every flag default named with units; every test mentioned has a test file location implied; every code-touch task names the file (`src/state/mod.rs`, `src/state/schema.rs`, `src/pipeline.rs`, `src/main.rs`, `src/process.rs`, `src/cli.rs`, `src/config.rs`).

**User review pending:** next session opens this file, runs the spec-self-review pass for fresh-eyes consistency, surfaces it for explicit user approval, then invokes `superpowers:writing-plans` to draft the per-task expansion at `docs/superpowers/plans/2026-05-13-plan-b-epic-2/`.
