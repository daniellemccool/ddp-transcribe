---
status: accepted
date: "2026-05-18"
category: State machine
applies_to:
    - src/state/mod.rs
priority: default
companions:
    - tests/state_claims.rs
    - tests/state_sweep.rs
---

# Sweep stale claims blind, no bump

## Decision

`sweep_stale_claims(threshold)` flips `in_progress` rows older than the
threshold back to `pending` with no artifact validation and no
`attempt_count` bump — it is operator-crash recovery, not an application
retry. Each recovered row writes one forensic `swept_stale` event carrying
the stale claim's provenance.

## Guidance

- The sweep reverts blind — no artifact validation, no `attempt_count` bump — and its `swept_stale` events are forensics only, never a status/retry/classification input.
- Don't add artifact validation or success-marking to the sweep; "in_progress with complete artifacts" is an accepted intermediate state (the artifacts-before-mark rule) and the redo cost of a re-fetch + re-transcribe is negligible.
- Don't bump `attempt_count` in the sweep — that would mix crash-recovery semantics into the retry cap and burn retry budget on operator kills.
- Recovery is logged once in aggregate (recovered count + threshold) and recorded per row as `worker_id='sweep'`, `detail_json` = `was_claimed_by`/`claimed_at`/`threshold_secs`, gathered by a SELECT that repeats the UPDATE's predicate verbatim inside the one IMMEDIATE transaction, so the event set is exactly the recovered set (`debug_assert_eq!` pins it). Review rejects any condition added to that predicate — on either side.
- No consumer may branch on a `swept_stale` event (status, retry or classification); reading one for display or an operator query is fine. Keep the sweep distinct from the start-of-batch classification sweep, which is application behavior.
- Keep the default threshold (30 minutes, flag-tunable via `--stale-claim-threshold`) far above worst-case per-video wallclock (~25s at bake) so a future multi-instance setup can't steal claims from healthy peers.

## Why

Validation and attempt-bumping each corrupt an adjacent mechanism: validation
duplicates the success path the pipeline already owns, and a bump makes a
`kill -9` spend the row's retry budget. Blind revert is safe precisely
because artifacts-before-mark makes redo idempotent.

The original rule also banned event rows, on the reasoning that crash
recovery is not an application event. That clause was amended (2026-07-29,
campaign-safety slice) because it left the only legitimate
`in_progress → pending` transition invisible: during a two-instance
production run a pending-count rise could not be told apart from a lost
concurrent write. Recording provenance changes no predicate and no status
semantics, so the blind revert stands while the anomaly becomes falsifiable —
matching events mean the sweep did it, none mean writer loss.

## Context

Process crashes leave rows parked in `in_progress` with a live-looking claim;
without the sweep they were stranded forever. Deferred-by-design (revisit
only if measurement supports it): `--validate-artifacts-on-sweep` and
`--bump-attempts-on-sweep`.

## Alternatives

- **Validate artifacts, mark_succeeded when present** — measured redo cost (one re-fetch + one re-transcribe per in-flight row) doesn't justify duplicating the success path.
- **Count sweeps as attempts** — conflates operator recovery with application retry policy.
- **Conservative 1-hour default** — needlessly delays recovery; 30 min is already ~70× worst-case per-video time.
- **Per-row provenance via structured tracing instead of `video_events`** — keeps the no-events clause literally intact but puts the forensic trail in logs that rotate away and don't join to the row; the anomaly is investigated from a state-DB snapshot, so the trail has to live in the DB.
