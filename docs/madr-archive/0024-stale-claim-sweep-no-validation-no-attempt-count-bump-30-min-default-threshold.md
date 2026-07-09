---
status: accepted
date: "2026-05-18"
comments:
    - author: Danielle McCool
      date: "2026-05-18 22:34:44"
      text: marked decision as decided
---

# Stale-claim sweep: no validation, no attempt_count bump, 30-min default threshold

## Context and Problem Statement

Process crashes mid-batch leave rows in `status='in_progress'` with valid `claimed_by`/`claimed_at`. Plan A had no recovery; Plan B Epic 2 introduces a `sweep_stale_claims(threshold)` mutator that flips stale rows back to `pending`. Open design questions: (a) should the sweep validate that no artifacts exist before reverting? (b) should it bump `attempt_count`? (c) what threshold default?

## Considered Options

* No validation, no attempt_count bump, 30-min default
* Validate artifacts present → mark_succeeded; else revert (validate-and-mark-succeeded)
* Bump attempt_count on every sweep (count sweeps as retries)
* Configurable threshold with conservative default (1 hour)

## Decision Drivers

- 0008 invariant ("in_progress + complete artifacts" is an accepted intermediate state)
- Don't corrupt Epic 3's retry-policy semantics
- Prevent stealing from healthy peers in any future multi-instance scenario
- Bake worst-case fetch is ~25s; threshold must be well above that

## Decision Outcome

Chosen option: "No validation, no attempt_count bump, 30-min default", because Validate-and-mark-succeeded is deferred to Plan C if measured to matter — Phase 2 coordinated-shutdown drill shows redo cost on kill -KILL is one re-fetch + one re-transcribe per in-flight row, negligible against the N=3 vs N=1 throughput delta. attempt_count bump would mix operator-recovery semantics (sweep) with Epic 3 application-retry semantics. 30 min is conservatively above bake worst-case fetch (~25s)..

## Consequences

- 30-min default is conservative against bake worst-case (~25s end-to-end per video, single-state). Far above worst-case prevents stealing from healthy peers in any future multi-instance scenario. The threshold is `--stale-claim-threshold` flag-tunable (T11) so operators can tighten for testing.
- A sweep on a v2 DB with stale claims emits exactly one log line per recovered row (no event row inserted in `video_events` — the sweep is an operator-recovery action, not an application event).
- Plan C may add `--validate-artifacts-on-sweep` and `--bump-attempts-on-sweep` flags if measurement supports them; this ADR records that those are deferred-by-design.
- The redo cost of a kill -KILL mid-batch is bounded by the threshold + the per-video budget (~30min + ~7s = ~30min worst-case wallclock until full recovery).

## Comments

* **2026-05-18 22:34:44 — @Danielle McCool:** marked decision as decided
