---
status: accepted
date: "2026-05-20"
comments:
    - author: Danielle McCool
      date: "2026-05-20 13:55:11"
      text: marked decision as decided
---

# Claim contention: no polling for Plan B — batch-drain on claim_next==None

## Context and Problem Statement

Phase 2's fetch workers race for pending rows via `claim_next`. When `claim_next` returns `None`, the worker can either (a) exit (drain semantics) or (b) sleep-poll waiting for new work. EPIC-2-SKETCH originally proposed 100ms–2s polling backoff. Should Plan B's fetch workers poll, or exit on first `None`?

## Considered Options

* Exit on first `None` (drain semantics)
* Sleep-poll 100ms–2s with exponential backoff
* Block on a condition variable signaled by `claim_next`

## Decision Drivers

- Plan B is batch-drain: ingest happens in a separate `ingest` phase before `process`. The pool is frozen at process startup.
- Polling burns CPU without producing work
- Daemon mode (continuous ingest) is Plan C territory

## Decision Outcome

Chosen option: "Exit on first `None` (drain semantics)", because Plan B `process` is batch-drain: ingest is a separate prior phase. Polling burns CPU without producing work; condvars add machinery that Plan C will redesign anyway. Daemon mode is Plan C territory..

## Consequences

- A `process` invocation with 0 pending rows exits immediately (claimed=0 → process exits 3 per existing main.rs behavior).
- Mid-process behavior is unaffected: workers race for whatever pending rows exist at startup; once drained, all workers see `None` and exit; the orchestrator joins them and exits 0.
- The orchestrator does NOT need a "wait for more work" signal — `JoinSet::join_next()` returning `None` after all workers exit IS the drain signal.
- This is an explicit deviation from EPIC-2-SKETCH's polling proposal (the sketch was written before the Plan A → Plan B operational model was clarified).
- Polling is deferred to Plan C / daemon mode where ingest is live. At that point a 0026 amendment or successor ADR documents the polling policy.

## Comments

* **2026-05-20 13:55:11 — @Danielle McCool:** marked decision as decided
