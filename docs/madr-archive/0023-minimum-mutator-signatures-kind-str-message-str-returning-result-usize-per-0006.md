---
status: accepted
date: "2026-05-18"
comments:
    - author: Danielle McCool
      date: "2026-05-18 22:34:27"
      text: marked decision as decided
---

# Minimum mutator signatures: (kind: &str, message: &str) returning Result<usize> per 0006

## Context and Problem Statement

Epic 2's state machine adds two failure-classification mutators (`mark_retryable_failure`, `mark_terminal_failure`) and uses them from the serial loop (Phase 1) and orchestrator (Phase 2). Epic 3 will introduce typed-enum failure classification (`RetryableKind`, `UnavailableReason`, `ClassifiedFailure`). Question: what signature should Epic 2's mutators use today, given Epic 3 will refine them?

## Considered Options

* `(kind: &str, message: &str) -> Result<usize>` — string-typed kind, 0006-conformant return
* Typed enum today (front-load Epic 3's `ClassifiedFailure`)
* Free-form `&serde_json::Value` payload (let callers store whatever)

## Decision Drivers

- 0006 conformance (row-change count return)
- Compose cleanly with Epic 3's typed enums
- Don't lock Epic 3's design in
- Minimum surface area today

## Decision Outcome

Chosen option: "`(kind: &str, message: &str) -> Result<usize>` — string-typed kind, 0006-conformant return", because Typed enum today pre-decides Epic 3 before the failure-mode catalog is empirically grounded; free-form payload invites drift (every caller defines its own schema). String-kind composes cleanly with Epic 3 typed enums via `ClassifiedFailure::tag()` / `::message()`..

## Consequences

- Concrete shape is `(video_id: &str, worker_id: &str, kind: &str, message: &str) -> Result<usize>`. The four-arg form adds the same `WHERE status = 'in_progress' AND claimed_by = ?` predicate as the tightened `mark_succeeded`, so all stale-claim mutators have symmetric semantics.
- Phase 1's classifier wiring (T9) emits string kinds like `"FetchOrTranscribe"` until Epic 3 lands typed enums.
- Phase 2's fetch worker reuses the same string-kind path.
- Epic 3 introduces typed enums via `ClassifiedFailure` and a wider signature; the (kind, msg) surface composes — Epic 3's dispatcher calls `mark_retryable_failure(id, &kind.tag(), &kind.message())` or similar.
- Epic 3's first task is the enum + classifier dispatcher; signature broadens at that point with `succeeds: ["0023"]` if the change is structural.

## Comments

* **2026-05-18 22:34:27 — @Danielle McCool:** marked decision as decided
