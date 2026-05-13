---
adr_id: "0019"
comments:
    - author: Danielle McCool
      comment: "1"
      date: "2026-05-13 16:06:27"
links:
    precedes: []
    succeeds: []
status: decided
tags: []
title: Subagent report format and phase-boundary controller restart
---

## <a name="question"></a> Context and Problem Statement

Subagents dispatched via the Agent tool return their full implementation transcripts by default. For a 250-line implementer's transcript plus 30 tool calls, this is many hundreds of words flowing back into the orchestrator's conversation per task. Combined with the absence of a discipline for restarting the controller between phases, this contributed to compaction observed near the end of Plan B Epic 1.

The mitigation has two parts and they share the same shape: manage the controller's working set deliberately, both at the per-task level (subagent report cap) and at the per-phase level (controller restart with a small handoff doc).

## <a name="options"></a> Considered Options

1. <a name="option-1"></a> No discipline; default subagent verbosity; single controller session across an epic (status quo)
2. <a name="option-2"></a> Structured report cap of at most 250 words in every dispatch brief (format: STATUS / SUMMARY / CHANGED FILES / DEVIATIONS) PLUS phase-boundary controller restart with a PHASE-N-CLOSE.md handoff
3. <a name="option-3"></a> Hard report cap enforced by hook (PostToolUse on Agent that truncates over-budget responses)
4. <a name="option-4"></a> Report cap only, no phase-boundary restart (relies solely on cap)

## <a name="criteria"></a> Decision Drivers

Orchestrator working-set across long epics; observed compaction cost from Plan B Epic 1; subagent ability to communicate adequately in compressed form; clean phase identity versus continuous controller history; the cost of a hook (extra surface to maintain and debug) versus the cost of dispatch-brief discipline (relies on each brief including the format instruction).

## <a name="outcome"></a> Decision Outcome
We decided for [Option 2](#option-2) because: Every dispatch brief includes the structured-report format and 250-word cap (allowance for unusual cases). Plans define phase boundaries explicitly; at each phase boundary the controller writes PHASE-N-CLOSE.md (<=1 page) and ends; next phase starts fresh with the spec + close-out doc. Estimated 30-50% reduction in per-task subagent-result context flowing into the orchestrator's conversation.

## <a name="comments"></a> Comments
<a name="comment-1"></a>1. (2026-05-13 16:06:27) Danielle McCool: marked decision as decided
