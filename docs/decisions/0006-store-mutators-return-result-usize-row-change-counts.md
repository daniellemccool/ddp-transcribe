---
status: accepted
date: "2026-04-17"
category: State machine
applies_to:
    - src/state/mod.rs
priority: default
---

# Store mutators return Result<usize> row-change counts

## Decision

`Store` mutators expose the mutation outcome in their return value, with
`Result<usize>` — rusqlite's `Connection::execute` row-change count — as the
default shape: 1 = applied / newly created, 0 = no-op / row already existed.
Callers never query before or after a mutation to detect whether it applied.

## Guidance

- New mutators (`mark_*`, `upsert_*`, `record_*`) return `Result<usize>`; review rejects `Result<()>` mutators and separate `*_exists` query APIs added for outcome detection.
- Production code never reaches for `#[cfg(any(test, feature = "test-helpers"))]` items to detect a state change — that breaks `cargo build` without the feature flag.
- The count semantic is per single-row, binary-outcome mutation. A mutator with a richer verdict (`record_fetch_failure`'s requeue/exhaust/park) or a multi-row effect returns a typed outcome instead — but its internals still drive each branch off the row-change count; the type makes that information unambiguous, it never discards it.

## Why

Returning `Result<()>` caused a real defect: a caller needing "newly inserted
vs already existed" reached for a cfg-gated test helper from production code
and broke the non-test build. Query-first detection adds a TOCTOU race that
`INSERT OR IGNORE` atomicity gives us for free; the row-change count is a
zero-cost forward of information rusqlite already returns.

## Context

`process_watch_entry` needed to increment `unique_videos_seen` only for newly
inserted videos. With `upsert_video` returning `Result<()>`, the only visible
way to detect insertion was `Store::get_video_for_test(...)` — a test-helpers
item — from production code. The convention was recorded so every later
mutator (`mark_succeeded`, the failure mutators, `record_fetch_failure`)
exposes the outcome in its signature.

## Alternatives

- **Typed `InsertOutcome` enum (`Created` / `AlreadyExisted`)** — self-documenting but overkill for a binary outcome; forces every caller to pattern-match even when it only wants the count. Richen the type when a mutator genuinely has more than two outcomes.
- **Callers query first, mutators return `Result<()>`** — the pattern that triggered the bug, plus an extra roundtrip per mutation.
- **Callback closure invoked on the "newly created" branch** — heavyweight for a boolean question; fragments the call site.
- **Separate `Store::*_exists(...)` query API** — TOCTOU race between check and mutation.
