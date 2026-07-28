---
status: accepted
date: "2026-04-17"
category: Artifacts
applies_to:
    - src/pipeline/mod.rs
    - src/output/artifacts.rs
priority: invariant
---

# Artifacts are durable on disk before mark_succeeded

## Decision

The pipeline writes both transcript artifacts (`.txt`, then `.json`) via
`output::artifacts::atomic_write` and only then calls `mark_succeeded` —
the DB acknowledges success last. The pair `write_artifacts_durable` →
`mark_after_artifacts` (`src/pipeline/mod.rs`) owns this ordering for
every pipeline variant.

## Guidance

- Review rejects any reordering that can produce a `succeeded` row without both artifacts on disk — that state is silent corruption with no recovery path short of manual DB surgery, whereas `in_progress` with partial artifacts is always recoverable by re-claim.
- `write_artifacts_durable` takes no store handle and must keep none: callers run it outside any store lock (its two file fsyncs plus a directory fsync would otherwise serialize every other worker's claim/failure dispatch) and lock only around `mark_after_artifacts`. `write_artifacts_and_mark` is their composition, kept for the serial path; the pipelined transcribe worker calls the halves directly. Re-merging the two under one lock is a rejected simplification.
- `atomic_write` stays idempotent (write tmp → fsync → rename → fsync parent) so a recovery re-run safely overwrites partial artifacts, and its tmp name is unique per process and per call (`{name}.tmp-{pid}-{seq}`) so a concurrent writer of the same target renames its own complete file instead of clobbering another's in-flight one; keep new artifact kinds on the same helper.
- Any future mutator combining DB state with on-disk artifacts inherits the same shape: disk first, DB acknowledgement last.
- A cancellation or crash mid-write must still leave the ordering intact (the pipelined transcribe worker relies on this inside its cancel window).

## Why

Every crash point then resolves to either terminal success or
recoverable-in_progress: the DB never claims artifacts that don't exist.
The inverse ordering means an operator query sees `succeeded`, downstream
finds no transcript, and nothing in the system ever notices. The cost — one
redundant re-fetch + re-transcribe after a crash — is negligible for
short-form video.

## Context

The acknowledged in-between state (`in_progress` + complete artifacts, when
a crash lands between the `.json` write and `mark_succeeded`) is exactly the
state the stale-claim sweep may blindly revert; the redo is safe because
`atomic_write` overwrites. This asymmetry — disk can be ahead of the DB, but
the DB is never ahead of disk — is what failure classification and the
operational done-contract lean on.

## Alternatives

- **mark_succeeded first, then write** — the silent-corruption shape.
- **Two-phase `pending_artifacts` flag** — adds a column for a problem the ordering already solves.
- **Atomic cross-domain transaction** — impossible over SQLite + filesystem.
