---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-05-13 16:06:27"
      text: '1. (2026-05-13 16:06:27) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Subagent report format and phase boundary controller restart

## Context and Problem Statement

Subagents dispatched via the Agent tool return their full implementation transcripts by default. For a 250-line implementer's transcript plus 30 tool calls, this is many hundreds of words flowing back into the orchestrator's conversation per task. Combined with the absence of a discipline for restarting the controller between phases, this contributed to compaction observed near the end of Plan B Epic 1.

The mitigation has two parts and they share the same shape: manage the controller's working set deliberately, both at the per-task level (subagent report cap) and at the per-phase level (controller restart with a small handoff doc).

## Considered Options

* No discipline; default subagent verbosity; single controller session across an epic (status quo)
* Structured report cap of at most 250 words in every dispatch brief (format: STATUS / SUMMARY / CHANGED FILES / DEVIATIONS) PLUS phase-boundary controller restart with a PHASE-N-CLOSE.md handoff
* Hard report cap enforced by hook (PostToolUse on Agent that truncates over-budget responses)
* Report cap only, no phase-boundary restart (relies solely on cap)

## Decision Drivers

Orchestrator working-set across long epics; observed compaction cost from Plan B Epic 1; subagent ability to communicate adequately in compressed form; clean phase identity versus continuous controller history; the cost of a hook (extra surface to maintain and debug) versus the cost of dispatch-brief discipline (relies on each brief including the format instruction).

## Decision Outcome
We decided for [Option 2](#option-2) because: Every dispatch brief includes the structured-report format and 250-word cap (allowance for unusual cases). Plans define phase boundaries explicitly; at each phase boundary the controller writes PHASE-N-CLOSE.md (<=1 page) and ends; next phase starts fresh with the spec + close-out doc. Estimated 30-50% reduction in per-task subagent-result context flowing into the orchestrator's conversation.

## Comments

* **2026-05-13 16:06:27 — @Danielle McCool:** 1. (2026-05-13 16:06:27) Danielle McCool: marked decision as decided
