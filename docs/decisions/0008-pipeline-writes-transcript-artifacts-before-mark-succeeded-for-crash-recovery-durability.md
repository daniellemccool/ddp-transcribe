---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-04-17 09:43:12"
      text: '1. (2026-04-17 09:43:12) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Pipeline writes transcript artifacts before mark_succeeded for crash recovery durability

## Context and Problem Statement

T14's `pipeline::process_one` orchestrates fetch → transcribe → write `.txt` artifact → write `.json` metadata → remove staged WAV → `mark_succeeded`. The DB's `succeeded` status is the LAST acknowledgement. If the process crashes (or is OOM-killed, or hits `process::exit`) at any point before `mark_succeeded` commits, the `videos` row stays at `in_progress` and the next `claim_next` re-claims the same row — refetching, re-transcribing, atomic-overwriting the partial artifacts. The pipeline must commit to one ordering and stick with it; the wrong ordering is a silent data-loss bug. Affects T14's `process_one` today and any later mutator that combines DB state with on-disk artifacts (Plan B's failure-classification, Plan C's API-direct path that may skip artifact writes entirely).

## Considered Options

* Write artifacts (`.txt`, `.json`) and then `mark_succeeded` (DB acknowledges success last)
* `mark_succeeded` first, then write artifacts (DB acknowledges success first)
* `mark_succeeded` with a `pending_artifacts` flag, write artifacts, then clear the flag (two-phase commit)
* Single transactional write that atomically updates DB and writes files (impossible without distributed-tx semantics across SQLite + filesystem)

## Decision Drivers

Crash recovery: `in_progress` is recoverable (the next `claim_next` re-runs the pipeline and overwrites partial artifacts via `output::artifacts::atomic_write`); `succeeded` without artifacts on disk is silent corruption. `output::artifacts::atomic_write` is idempotent (write tmp → fsync → rename → fsync parent), so re-running over a partially-written shard directory is safe. Plan B's failure-classification will need to inspect both DB state and artifact-presence to decide `mark_failed_terminal` vs `mark_failed_retryable`; the ordering must let it distinguish "tool failed before any artifact" from "tool succeeded but mark_succeeded never ran." Operator-debuggable: an inspector seeing `in_progress` + partial-artifacts can trust the pipeline will recover; seeing `succeeded` + no-artifacts is a panic.



## Decision Outcome
We decided for [Option 1](#option-1) because: Option 1 is the only ordering where every failure mode resolves to a recoverable state. The state machine is: (a) crash before fetcher returns - row stays in_progress no artifacts; recoverable via re-claim. (b) Crash during transcribe - row in_progress no artifacts; recoverable. (c) Crash between txt write and json write - row in_progress with partial artifacts; recoverable because atomic_write is idempotent (write tmp fsync rename fsync parent) and the next iteration overwrites both. (d) Crash between json write and mark_succeeded - row in_progress with both artifacts present; recoverable because the next iteration overwrites them and then succeeds. (e) Crash AFTER mark_succeeded - row succeeded with both artifacts present; the desired terminal state. Every state is either terminal-success or recoverable-via-re-claim. Rejected option 2 (mark_succeeded first then write artifacts): introduces a state row succeeded but no artifacts on disk that is silent corruption. The next operator query against the DB sees succeeded; downstream code looking up the transcript file finds nothing; no fix path exists short of a manual schema-level recovery (set status back to in_progress for rows where the .txt is missing). This is the wrong shape for a walking skeleton or anything else. Rejected option 3 (two-phase with pending_artifacts flag): would work but adds a column for a problem option 1 already solves. Worth the complexity only if Plan B introduces operations that need to claim a transitional state for other reasons (e.g., post-success classification work). Rejected option 4 (cross-domain atomic transaction): impossible without distributed-tx semantics that SQLite plus filesystem do not provide. Trigger: T14 wired the full pipeline and the ordering was implicit in the brief. Recording the invariant explicitly so future implementers do not optimize by reordering. Plan B failure-classification work will need to inspect both DB state and artifact-presence to decide mark_failed_terminal vs mark_failed_retryable; the option 1 ordering lets that classifier distinguish tool failed before any artifact in_progress no artifacts from tool succeeded but mark_succeeded never ran in_progress both artifacts present. Consequences: positive - every failure mode is recoverable; the DB never lies about artifact presence; idempotent re-run is safe via atomic_write. Negative - re-running the full fetch and transcribe on the recovery path costs network and CPU; for very long videos this could be expensive but Plan A only handles short-form TikTok. Negative - between artifact-write and mark_succeeded the DB and disk can disagree by one row; an external observer reading the DB during that window sees in_progress while the artifacts are already on disk. Acceptable for Plan A single-process serial loop where no external observer exists. Trade-off: chose recoverable in_progress over silent succeeded-without-artifacts. The cost of one re-fetch on crash is acceptable to avoid a panic-class data-integrity bug.

## Comments

* **2026-04-17 09:43:12 — @Danielle McCool:** 1. (2026-04-17 09:43:12) Danielle McCool: marked decision as decided
