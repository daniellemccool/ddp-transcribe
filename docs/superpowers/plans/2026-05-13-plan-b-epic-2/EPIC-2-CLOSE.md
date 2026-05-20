# Plan B Epic 2 — Epic Close

**Branch:** `feat/plan-b-epic-2`
**Epic 2 last commit:** this commit (T20 bake notes + EPIC-2-CLOSE + FOLLOWUPS archival)
**Status:** all Phase 2 tasks (T12–T20) complete. Plan B Epic 2 fully shipped.

---

## What landed

Phase 2 maps to these commits (Phase 1 closed at `518fc8a`; Phase 2 runs from `62a2eb6` forward):

| Task | Commit(s) | Subject |
|---|---|---|
| T12 | `62a2eb6` | ADRs 0025–0027 (JoinSet+CancellationToken shutdown order; no-polling drain; N=3 topology defaults) |
| T13 | `b1d9bea` | `tokio-util = "0.7"` dep (no features needed) |
| T14 | verify-only | No commit — confirmed bounded subprocess capture landed in `9e84b54` (perf-tweaks) |
| T15 | `cfb2647` | Pipeline module reshape: `run_pipelined` skeleton + `fetch_and_decode` / `transcribe_and_write` helpers |
| T16 | `dd23814` | `fetch_worker` — claim/fetch/decode/send loop with `stale_after_failure` counter |
| T17 | `6d95598` | `transcribe_worker` — biased `select!{cancel|recv}` → transcribe → write+mark |
| T18 | `eee573d` + `a66d38b` | Supervision + shutdown ORDER (LOAD-BEARING per 0025); `select!` wrap fixup on `engine.transcribe()` |
| T19 | `1e308e4` | `--download-workers` + `--channel-capacity` CLI flags with 0027 defaults |
| T20 | this commit | Bake notes (Epic 2 section) + EPIC-2-CLOSE + FOLLOWUPS archival |
| Cleanup 1/3 | `ecfc10f` | Test-quality fixes (operator review) |
| Cleanup 2/3 | `1ffd9fc` | Test enrichment + migration row-survival + FOLLOWUPS archival |
| Cleanup 3/3 | `7afe72c` | Epic 3+ FOLLOWUPS entries (operator test-suite review) |
| Bootstrap docs | `98df269` + `d928905` | A10 bootstrap + archive scripts + runbook; UX fixes from first SRC bake session |

**Ordering note:** Cleanup commits landed between T18 and T19 per the operator's pre-T20 sweep; T19 (CLI flags) was held until Phase 2 core workers were complete.

---

## Phase 2 exit criteria — all met

- ✅ `cargo fmt --all --check` clean.
- ✅ `cargo clippy --all-targets -- -D warnings` clean.
- ✅ `cargo test --features test-helpers -- --test-threads=1` green (all binaries; 156 passed, 8 ignored, 0 failed).
- ✅ `cargo build --release --features cuda` succeeded on A10 workspace.
- ✅ Operator workflow viable:
  - `run_pipelined` with N=3 fetch workers + 1 transcribe worker drains 20-video `news_orgs` fixture.
  - `--download-workers` and `--channel-capacity` CLI flags honored with 0027 defaults.
  - Coordinated-shutdown drill: pre-kill `in_progress` count matched post-restart `recovered` count exactly.
  - `--stale-claim-threshold 1s` restart recovered all 6 in-flight rows; final `succeeded=20 failed=0`.
- ✅ Bake measurements (2026-05-20 on A10):
  - N=1: 41.7s wall / 2.09s per-clip; N=3: 29.0s / 1.45s (1.44× speedup); N=5: 28.8s / 1.44s (1.45×).
  - User CPU constant at ~56s across all three — design intent confirmed.
  - Language detection: fr/nl/en/tl all >0.987 confidence.
  - All `stale_after_success` and `stale_after_failure` counters stayed at 0 on happy-path runs.

See `docs/SRC-BAKE-NOTES.md` § Plan B Epic 2 for the full bake record.

---

## FOLLOWUPS state at close

**Archived this commit (5 entries):**

1. `Store::open` records `schema_version` but never reads-and-checks it — resolved by T2 (`0151e2e`) + T3 (`a9ec705`, `c13f64a`).
2. `concurrent_claim_serializes_via_begin_immediate` doesn't actually race — resolved by T10 (`518fc8a`).
3. `mark_succeeded` doesn't require `status = 'in_progress'` — resolved by T5 (`a8696e6`).
4. Plan B reassessment: `claim_next` polling semantics — resolved by ADR 0026 (`62a2eb6`).
5. Missing round-trip test: succeeded videos must not be re-claimable — resolved by T5 (`a8696e6`).

**Active entries remaining in `docs/followups/epic-2.md` (4 entries, cross-epic):**

- Worker-side closed-reply path silently swallows error → Epic 3+ (tracing context, no per-video warn yet).
- `--max-videos` ignored by `run_pipelined` (silent regression; startup warn in place) → Epic 3 cleanup.
- `fetch_worker` cancellation latency bounded by largest await, not `token.cancel()` → Epic 3 graceful-shutdown.
- sync `write_artifacts_and_mark` inside `tokio::sync::Mutex` guard inside async fn stall risk → Epic 5 ops-hygiene.

**`docs/followups/cross-epic.md`** retains the plan-brief library-API drift entry (T13/T19/T16-Epic2 → Epic 3 planning kickoff checklist adoption).

---

## Epic 3 entry point

Epic 3 inherits: the pipelined orchestrator (`run_pipelined` with N=3 fetch + 1 transcribe), the full state-machine mutator family (`mark_succeeded`, `mark_retryable_failure`, `mark_terminal_failure`, `sweep_stale_claims`), ADRs 0022–0027 as the canonical design slate, and the four carry-over FOLLOWUPS above. The A10 bootstrap runbook (`docs/ops/`) and `news_orgs` fixture are the canonical bake surface.

**Suggested first task:** typed failure-classification taxonomy — `RetryableKind` / `UnavailableReason` / `ClassifiedFailure` replacing the placeholder `"FetchOrTranscribe"` string-kind. This is the stated Epic 3 charter in the design spec and resolves the `From<RunError> for FetchError`, `status.code().unwrap_or(-1)`, and `From<AudioDecodeError> for TranscribeError` entries already queued in `docs/followups/epic-3.md`.

---

## Codex-advisor session

Pinned session UUID at Epic 2 close: confirm via `codex-advisor id` at Epic 3 start. Re-init only if the session has been lost.
