---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-04-17 08:26:50"
      text: '1. (2026-04-17 08:26:50) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Store mutators return Result of usize for row change count

## Context and Problem Statement

T13's `process_watch_entry` needed to detect "was this video newly inserted or did it already exist" so it could increment `unique_videos_seen` correctly. The brief reached for `Store::get_video_for_test(...)` — a `#[cfg(any(test, feature = "test-helpers"))]` test helper per 0005 — from production code, which breaks `cargo build` without the feature flag. We need a convention for how Store mutators communicate state-change outcomes to callers, applicable to T9's `upsert_video`/`upsert_watch_history`, T10's `mark_succeeded` (and Plan B's future `mark_failed_terminal` / `mark_failed_retryable` / `update_progress`), and any later mutator that callers need to distinguish "newly created" from "no-op."

## Considered Options

* All Store mutators return `Result<usize>` carrying `rusqlite::Connection::execute`'s row-change count
* Mutators return `Result<()>`; callers query first to detect new-vs-existing (the brief's pattern that triggered the bug)
* Mutators return a typed `InsertOutcome` enum (e.g., `Created` / `AlreadyExisted`)
* Mutators take a callback closure invoked only on the "newly created" branch (visitor-style)
* Mutators return `Result<()>` and expose a separate `Store::*_exists(...)` query API for callers that need the distinction

## Decision Drivers

Production code must NOT depend on `#[cfg(any(test, feature = "test-helpers"))]` library items per 0005. Cheap to implement (no extra query roundtrip per mutation). Composes with rusqlite's native `Connection::execute` return type. Forward-compatible with Plan B's mark_failed_* mutators and any later mutator. Discoverable from the type signature so future implementers do not repeat the test-helper trap.



## Decision Outcome
We decided for [Option 1](#option-1) because: Option 1 passes through rusqlite::Connection::execute native usize return — zero-cost forward of information that already exists. Symmetric with T9 upsert_watch_history which already used Result<usize>; T13 retroactively brought T9 upsert_video into the same shape. T10 mark_succeeded currently returns Result<()>; future bin consumer (T14) will naturally also need to detect "did this update apply" so the convention should expand to mark_succeeded too as part of T14 work. Plan B mark_failed_terminal / mark_failed_retryable / update_progress mutators MUST follow the same convention. Rejected option 2 (Result<()>): caused the T13 bug — callers reach for cfg-gated test helpers from production code. Rejected option 3 (typed InsertOutcome enum): overkill for binary insert/no-op outcomes; if more states emerge in Plan B (e.g., updated existing row vs no-op), that is the right time to richen the type, not now. Option 3 also forces every caller to pattern-match an enum even when they only want the count. Rejected option 4 (callback closure): visitor-style is heavyweight for a simple boolean question; it also fragments the call site (mutation logic separated from caller logic by indentation). Rejected option 5 (separate _exists query API): introduces a TOCTOU race between the existence check and the mutation; defeats the atomicity that INSERT OR IGNORE provides for free. Trigger: T13 surfaced this when T9 Result<()> forced the brief author to reach for store.get_video_for_test from production code, which broke cargo build without the feature flag. Recording the convention so T14, T15, and Plan B mutators do not repeat the trap. Consequences: positive - production code never needs test helpers for state-change detection; zero-cost; symmetric across all mutators. Negative - callers must know that 0 means "row already existed" and 1 means "newly inserted" - implicit semantic; a typed enum would be self-documenting but at the cost of visibility everywhere. Negative - if a future operation can change MORE than one row per call (e.g., bulk upserts), the usize count loses the per-row outcome distinction; bulk operations should return a richer type when introduced. Trade-off: chose simplicity and zero-cost over rich typing. Revisit at Plan B if state-change semantics need to express more than binary outcomes per single-row mutation.

## Comments

* **2026-04-17 08:26:50 — @Danielle McCool:** 1. (2026-04-17 08:26:50) Danielle McCool: marked decision as decided
