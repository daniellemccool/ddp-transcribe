---
status: accepted
date: "2026-04-16"
category: Process
applies_to:
    - docs/superpowers/plans/**/*.md
priority: default
---

# Split plans per task

## Decision

Implementation plans live as one overview file (front matter, conventions,
exit criteria, task index) plus one file per task — never as a single
monolithic plan document.

## Guidance

- A new epic's plan adopts the split from day one: `00-overview.md` + `NN-task.md` files; review rejects a plan that grows past a few hundred lines in one file.
- Subagent dispatches read only their task file (plus the overview when needed), never the whole plan.
- The overview holds everything cross-task (conventions, exit criteria, index) so task files stay self-contained for a single dispatch.

## Why

A 3,347-line monolith forced ~44k tokens into every reviewer dispatch;
the split cut that to ~5k per task read and paid for itself within 2–3
tasks — on thermally-constrained hardware the dispatch-cost math is the
discipline.

## Alternatives

- **Single file + prompt discipline forbidding reads** — unenforceable; someone always loads it.
- **Controller pastes task text inline** — couples every dispatch to controller context and loses file-level review.
