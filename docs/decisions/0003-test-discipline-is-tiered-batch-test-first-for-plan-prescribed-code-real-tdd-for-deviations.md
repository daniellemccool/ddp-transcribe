---
status: accepted
date: "2026-04-16"
category: Process
applies_to:
    - tests/**/*.rs
    - src/**/*.rs
priority: default
---

# Reserve real TDD for deviations

## Decision

Plan-prescribed code uses test-first batch development (tests and
implementation both dictated by the task file, landed together); any
deviation — bug fix, behavior the plan omitted, ADR-driven change — uses
real TDD (failing test first, seen to fail meaningfully); coverage-fill
tests are neither and are labeled as such.

## Guidance

- Fixing a bug: write the failing test, watch it fail for the real reason (not a compile error), then fix. The design is being discovered, which is where TDD earns its cost.
- Adding a test for behavior that already works: the commit says "add coverage test", not "TDD" — honesty about which tier a test belongs to is the point of the rule.
- Plan-prescribed batches don't need one-test-at-a-time ceremony; the dispatch overhead buys nothing when the implementation is transcription.
- A brief that deviates from its plan says so in the commit message (brief-deviation honesty).

## Why

Mislabeling practice corrodes trust in the test suite's provenance: "TDD
throughout" on mechanically transcribed code claims a design pressure that
never happened, while a real deviation skipping its failing-test step is
where undetected regressions actually enter.

## Alternatives

- **Strict TDD everywhere** — multiplies subagent dispatch cost with no design payoff on transcribed code.
- **No ordering discipline** — loses the failing-test evidence exactly where design is being discovered.
