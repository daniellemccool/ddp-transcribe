---
status: accepted
date: "2026-05-18"
category: State machine
applies_to:
    - src/state/mod.rs
priority: default
companions:
    - tests/state_claims.rs
---

# Stale-claim sweep recovers rows blind: no validation, no attempt bump

## Decision

`sweep_stale_claims(threshold)` flips `in_progress` rows older than the
threshold back to `pending` with no artifact validation, no `attempt_count`
bump, and no `video_events` row — it is operator-crash recovery, not an
application event. The default threshold is 30 minutes, flag-tunable via
`--stale-claim-threshold`.

## Guidance

- Don't add artifact validation or success-marking to the sweep; "in_progress with complete artifacts" is an accepted intermediate state (the artifacts-before-mark rule) and the redo cost of a re-fetch + re-transcribe is negligible.
- Don't bump `attempt_count` in the sweep — that would mix crash-recovery semantics into the retry cap and burn retry budget on operator kills.
- The sweep emits a single aggregate log line (recovered count + threshold) and writes no event rows; keep it distinct from the start-of-batch classification sweep, which is application behavior and does write events.
- Keep the default threshold far above worst-case per-video wallclock (~25s at bake) so a future multi-instance setup can't steal claims from healthy peers.

## Why

Validation and attempt-bumping each corrupt an adjacent mechanism: validation
duplicates the success path the pipeline already owns, and a bump makes a
`kill -9` spend the row's retry budget. Blind revert is safe precisely
because artifacts-before-mark makes redo idempotent.

## Context

Process crashes leave rows parked in `in_progress` with a live-looking claim;
without the sweep they were stranded forever. Deferred-by-design (revisit
only if measurement supports it): `--validate-artifacts-on-sweep` and
`--bump-attempts-on-sweep`.

## Alternatives

- **Validate artifacts, mark_succeeded when present** — measured redo cost (one re-fetch + one re-transcribe per in-flight row) doesn't justify duplicating the success path.
- **Count sweeps as attempts** — conflates operator recovery with application retry policy.
- **Conservative 1-hour default** — needlessly delays recovery; 30 min is already ~70× worst-case per-video time.
