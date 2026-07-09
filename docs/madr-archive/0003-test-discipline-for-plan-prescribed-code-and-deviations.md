---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-04-16 22:27:00"
      text: '1. (2026-04-16 22:27:00) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Test discipline for plan prescribed code and deviations

## Context and Problem Statement

The plan claims 'TDD throughout' but the per-task files supply both tests AND implementation verbatim. The actual practice is test-first batch development: drop in N tests, drop in the implementation, see all tests pass — there is no fail-then-pass cycle for a single behavior because the implementation is dictated upfront. The 'see it fail' step is essentially a compile error from the missing module rather than a meaningful test failure. How should we honestly characterize and structure our test discipline for the rest of Plan A and beyond?

## Considered Options

* Keep current practice but stop calling it TDD; rename to test-first batch development
* Switch to strict TDD for all remaining tasks (one test at a time per implementer dispatch)
* Hybrid: test-first batch for plan-prescribed code; full TDD for any deviation (bug fixes additions plan changes); coverage-fill labeled separately
* Drop test-first ordering entirely; allow tests and impl in any order

## Decision Drivers

Honesty about practice versus aspiration. Cost of dispatching subagents (each dispatch costs context tokens and on this hardware also generates heat). Real TDD has design value when implementation is being discovered; mechanical transcription has none. Coverage-fill tests cannot be TDD because the behavior already works. Bug fixes and additions outside the plan SHOULD use real TDD because the design ISN'T pre-specified.

## Decision Outcome
We decided for [Option 3](#option-3) because: Plan-prescribed code is mechanical transcription with no design value to discover. A one-test-at-a-time cycle would just multiply dispatch overhead without changing outcomes. But for any deviation — bug fixes missing test cases the plan didn't include ADR-driven changes or additions like the SHORT_LINK_RE query-string fix that will land in Plan C — the design IS being discovered and real TDD provides genuine value. Coverage-fill tests (where the behavior already works and the test passes on first run) are NEITHER TDD nor batch-test-first; they are a separate exercise and should be labeled as such in commit messages. Practical impact: when adding a coverage-fill test the commit message should say 'add coverage test' not 'TDD'. When fixing a bug write the failing test first see it fail meaningfully then fix. The per-task files still say 'TDD throughout' verbatim but this ADR refines what that means in practice. Triggered by T5's review observations and the addition of a vt.tiktok.com coverage test that exists only because the regex claims support but the plan's test list omits it.

## Comments

* **2026-04-16 22:27:00 — @Danielle McCool:** 1. (2026-04-16 22:27:00) Danielle McCool: marked decision as decided
