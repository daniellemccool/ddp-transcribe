---
status: accepted
date: "2026-07-08"
category: State machine
applies_to:
    - src/state/mod.rs
    - src/batch.rs
    - tests/state_retry.rs
    - tests/state_sweep.rs
priority: default
---

# Let the re-fetch judge liveness

## Decision

Retry is pipeline behavior, not operator triage: `record_fetch_failure`
decides requeue / exhaust / park in one transaction at failure time, lifetime
attempts are capped at `retries + 1` against the claim-time `attempt_count`,
and claim ordering `attempt_count ASC, first_seen_at ASC, video_id ASC`
drains fresh work before retries. The re-fetch itself adjudicates liveness —
there is no probe.

## Guidance

- The requeue-vs-exhaust-vs-park decision lives in `record_fetch_failure`'s single transaction; review rejects callers re-implementing any arm of it.
- Carve-out: this record remains the normal retry authority, but an operator may explicitly restore failed rows to pending after an external condition has materially changed, through the `requeue-failures` command (its own record: a forensic, default-deny override of eligibility). It is not an alternate classifier or retry scheduler — the subsequent fetch remains the liveness oracle, and it grants eligibility without ever resetting `attempt_count`.
- Hand-written SQL against `videos` is unsupported emergency repair: acceptable only if it preserves the forensic event invariant (no status transition without its `video_events` row), and `requeue-failures` exists so that operators do not need it.
- Don't reintroduce liveness probes (oEmbed or otherwise), on or off the hot path — dead classes self-classify when the re-fetch fails with a terminal-dispositioned message.
- The start-of-batch sweep (`batch::run_sweep`) re-adjudicates parked `failed_retryable` rows through the classification table so historical pools and cross-batch stragglers ride the same mechanism; sweep-written events carry `worker_id = 'sweep'`.
- `--max-videos` counts every claim including retries; `attempt_count` bumps at claim time. `tests/state_retry.rs` and `tests/state_sweep.rs` are the executable spec — change retry semantics there deliberately.
- Claim ordering is fresh-first by design; under a hypothetical continuous-ingest daemon mode retries could starve behind unending fresh supply — revisit ordering (age/attempt interleave) only if such a mode lands. Fine under batch-drain, where the pending pool empties and retries then drain to completion.

## Why

The 2026-07-07 census (n=7,087) showed the oEmbed probe merely re-confirming
settled classes while operator triage added per-batch ceremony and dry-run +
execute double-probed. Self-classification on re-fetch also handles impure
classes correctly (NoPermission: 25/452 alive) where a blanket write-off
would discard recoverable videos.

## Context

Replaces the Epic 3 operator-driven flow — oEmbed probe adjudicating parked
failures plus a manual `triage` subcommand requeuing them (retired; archived
as `docs/madr-archive/0034-*`). The operator ruled that retry must be
pipeline behavior; the probe retired with the census as its closing evidence.

## Alternatives

- **Operator-driven probe triage (the predecessor)** — ceremony per batch; the probe's verdicts duplicated what the re-fetch already proves.
- **Automatic backoff/jitter retry inside the workers** — retries would compete with fresh work mid-batch and hide retry state from the state machine; end-of-queue requeue keeps ordering observable in SQL.
