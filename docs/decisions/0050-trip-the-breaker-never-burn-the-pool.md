---
status: accepted
date: "2026-08-12"
category: Orchestration
applies_to:
    - src/pipeline/pipelined.rs
    - src/pipeline/mod.rs
    - src/batch.rs
    - src/commands.rs
    - src/cli.rs
priority: invariant
---

# Trip the breaker, never burn the pool

## Decision

`process` aborts the run when a run-global streak of consecutive claims
resolves without a single success — default threshold **50**
(operator-ratified 2026-08-12), overridable via `--breaker-threshold`, `0`
disables. Tripping cancels the ADR-0025 supervision token, the census
records `breaker_tripped`, and the process exits with code **4**.

## Guidance

- The streak counts every claim outcome that is not a success — inline-terminal
  write-offs, retryable parks, cookie parks, transcribe-phase failures — and
  resets on any completed transcription (including `StaleAfterSuccess`: the
  transport worked; the breaker measures transport, not claim races).
- Trip = `token.cancel()` on the ADR-0025 token, exactly once (swap-guarded) —
  workers finish their current row's state write and drain, exactly as
  ADR-0025 already guarantees. Review rejects a second cancellation channel,
  a worker `abort()`, a polling loop (ADR-0026 stands — workers still drain
  on `claim_next` → `None`), and the breaker writing video state itself: it
  never adds a second shutdown mechanism.
- DB-visible per the standing operator ruling: the verdict must be
  answerable from `batch_runs.census_json` alone; log lines are
  operationally invisible.
- Exit code 4 joins the existing map (0 success, 1 verify-failed, 3
  zero-claims) via `CommandExit` — the library still never calls
  `process::exit`.

## Why

Incident 2026-08-06 retried into a refusing endpoint for 60 unattended hours
at ~8 failures/s — 1.81M attempts burned and an IP-reputation own-goal;
incident 2026-08-10 cost only ~1,400 attempts because the operator happened
to be watching. At the campaign's ~77% success rate the false-trip
probability at threshold 50 is ≈ 0.23⁵⁰ ≈ 10⁻³²; a pure dead-video run
(~20% dead) cannot plausibly trip it either (0.2⁵⁰). A WAF wave trips in
seconds.
