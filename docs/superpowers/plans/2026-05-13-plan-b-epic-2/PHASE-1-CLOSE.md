# Plan B Epic 2 — Phase 1 Close

**Branch:** `feat/plan-b-epic-2`
**Phase 1 last commit:** `518fc8a` (T10 race-rewrite, 2026-05-20)
**Status:** all Phase 1 tasks (T1–T11) complete. Next controller starts Phase 2 (T12–T20) fresh per ADR 0019.

---

## What landed

22 commits ahead of `main`. Phase 1 maps to these commits (T1/T2 from the prior controller; T3–T11 from this controller):

| Task | Commit(s) | Subject |
|---|---|---|
| T1 | `a99a565` | ADRs 0022–0024 + Epic 1 FOLLOWUPS audit |
| T2 | `0151e2e` | `Store::open` schema-version check + `SchemaVersionMismatch` |
| T4 | `3d82026` | Schema v2 columns + `SCHEMA_VERSION` bump |
| T3 | `a9ec705` + `c13f64a` | `migrate` CLI subcommand (UPSERT meta.schema_version fix folded in) |
| T5 | `a8696e6` | `mark_succeeded` predicate + `Result<usize>` |
| T6 | `a6c1955` + `e5d51a4` | `mark_retryable_failure` mutator (NULL-clearing test fold-in) |
| T7 | `1d6b29c` + `0a8ad5a` | `mark_terminal_failure` (surface-only) + cross-family retention docs |
| T8 | `f447009` | `sweep_stale_claims` |
| T11 | `1ab78c3` | `--stale-claim-threshold` CLI flag (30-min default) |
| T9 | `64417e9` + `3fa9aed` | `run_serial` sweeps + classifies; `ProcessOutcome::StaleAfterSuccess` |
| T10 | `518fc8a` | `concurrent_claim_serializes_via_begin_immediate` rewritten with `Barrier(2)` |

Plus FOLLOWUPS hygiene commits (`5d15d58`, `0264462`, `1735331`, `00ee005`) and the kickoff CLAUDE.md updates (`60832ac`, `efef20f`).

**Ordering note:** T11 landed before T9 (brief permitted; cleaner consumption of `opts.stale_claim_threshold`).

---

## Phase 1 exit criteria — all met

- ✅ `cargo fmt --all --check` clean.
- ✅ `cargo clippy --all-targets -- -D warnings` clean.
- ✅ `cargo test --features test-helpers -- --test-threads=1` green (all binaries, including whisper-engine integration tests).
- ✅ `cargo build --release` succeeds (no `cuda` feature change in Phase 1).
- ✅ Operator workflow viable:
  - `uu-tiktok migrate` brings v1 → v2 (idempotent on v2).
  - `mark_succeeded` returns 0 on stale-claim; existing happy path unchanged.
  - `sweep_stale_claims` recovers synthetically-stale rows.
  - `run_serial` no longer aborts on first failure; classifies as `failed_retryable` and continues.

---

## FOLLOWUPS added during Phase 1

Logged for Phase 2 close (Epic 2 ships) or Phase 2 task scope:

| Entry | Scope | Trigger |
|---|---|---|
| Mutator test parity (backport `video_events` assertions to T5/T6; no-event-on-stale across all three) | Epic 2 cleanup | Before Phase 2 close |
| `sweep_stale_claims` hardening (`as i64` overflow, `threshold==0` semantics, future-`claimed_at` coverage) | Epic 2 cleanup | Before Phase 2 close |
| `mark_retryable_failure` Ok(0) silently swallowed in `run_serial` (symmetric to T5 carry-forward) | Phase 2 (T17/T18) | Concurrent workers landing |
| Failure-classification test enrichment (column-value assertions, transcribe-failure variant) | Epic 2 cleanup | Before Phase 2 close |
| Extended: 6 of 7 `GlobalArgs` fields missing `global = true` | Epic 5 cleanup | Existing entry; extended in `1735331` |

See `docs/followups/epic-2.md` and `docs/followups/epic-5.md` for full bodies.

---

## Operator environment note — important for Phase 2

This operator runs cargo with `CARGO_BUILD_JOBS=1`, `MAKEFLAGS=-j1`, `RAYON_NUM_THREADS=1`, `TOKIO_WORKER_THREADS=1`, `OMP_NUM_THREADS=1`, `CMAKE_BUILD_PARALLEL_LEVEL=1` set in the environment for thermal protection (Plan B Epic 1 documented this for `--test-threads=1`; Phase 1 inherited it).

**`TOKIO_WORKER_THREADS=1` is the one to re-examine in Phase 2.** Phase 2 spawns N=3 fetch workers + 1 transcribe worker into a `tokio::JoinSet`. On a single tokio worker thread:

- Cooperative `.await` yields will still interleave correctly between tasks. Most well-written async code works.
- Any task that blocks the worker (CPU-bound work without `spawn_blocking`, blocking syscall, std::thread::sleep) deadlocks the runtime — there's no second worker to make progress.
- The T20 bake's ~3.5× throughput target assumes I/O concurrency. If subprocess management or whisper.cpp invocations end up blocking the runtime thread instead of awaiting cleanly, the throughput target fails AND tests may deadlock.

**Phase 2 controller action:** confirm whether the operator's thermal envelope allows lifting `TOKIO_WORKER_THREADS=1` for Phase 2 specifically (e.g., per-binary opt-in via `tokio::runtime::Builder::worker_threads(...)`), or whether T17/T18 need to be designed against the single-worker constraint. Don't silently assume either way.

---

## Phase 2 entry point

Phase 2 controller starts fresh per ADR 0019. Inputs to load:

1. The full design spec: `docs/superpowers/specs/2026-05-13-plan-b-epic-2-design.md`.
2. This close-out doc.
3. Phase 2 task files: `12-adr-drafts-phase2.md` through `20-bake-orchestrator.md` (T12–T20).

**Do NOT re-load** the Phase 1 task files (`01-…` through `11-…`) — they're closed.

Phase 2 ADR slate (T12 drafts):

- 0025 — `JoinSet` + `CancellationToken`; shutdown ORDER is load-bearing
- 0026 — Claim contention: no polling for Plan B (batch-drain)
- 0027 — Orchestrator topology defaults (N=3 fetch + 1 transcribe; capacity 2)

First Phase 2 task (T12): draft + decide those three ADRs via `scripts/adr`.

---

## Codex-advisor session

The pinned session UUID at Phase 1 close was `019e1b70-1ea0-75b3-83ba-9a68f63d0545`. The Phase 2 controller should confirm via `codex-advisor id` and re-init only if the session has been lost.
