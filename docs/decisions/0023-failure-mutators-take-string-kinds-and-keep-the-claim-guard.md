---
status: accepted
date: "2026-05-18"
category: State machine
applies_to:
    - src/state/mod.rs
priority: default
companions:
    - src/failure.rs
    - src/classification.rs
---

# Keep string kinds and claim guard

## Decision

Failure-recording mutators identify the failure as plain strings —
`kind`/`label` and `message` parameters — and every mutator that moves a row
out of `in_progress` carries the `WHERE status = 'in_progress' AND
claimed_by = ?` predicate, reporting a stale claim through its return value,
symmetric with `mark_succeeded`.

## Guidance

- Keep kind/message as `&str` in `Store` signatures; typed failure enums live on the classifier side (`src/failure.rs`, `src/classification.rs`) and hand the Store their label/message strings — review rejects pushing enums or JSON payloads into the Store surface.
- Every claim-terminating mutator keeps the `claimed_by` predicate; an outcome of "0 rows changed" means the claim was stale and the caller must not treat the transition as applied.
- New failure kinds are new label strings riding the existing columns, not schema changes.

## Why

String-typed kinds kept the failure taxonomy free to evolve without Store
churn: the evidence-derived taxonomy and later the operator-editable
classification table both landed their labels through this same surface with
zero schema changes. Typing the enum into the Store before the failure-mode
catalog was empirically grounded would have locked the wrong shape in.

## Context

Recorded when the state machine gained its first failure mutators
(`mark_retryable_failure` / `mark_terminal_failure`, both
`(video_id, worker_id, kind, message) -> Result<usize>`), one epic before
typed classification existed. Later mutators kept the string surface while
widening around it — `record_fetch_failure` takes the label/message strings
plus policy arguments and returns a typed `FailureRecordOutcome` (its
requeue/exhaust/park verdict; row-count-driven internally, per the
count-convention record's carve-out).

## Alternatives

- **Typed enum in the Store signature from day one** — pre-decides the taxonomy before evidence; every later label addition becomes a Store change.
- **Free-form `&serde_json::Value` payload** — invites drift; every caller defines its own schema.
