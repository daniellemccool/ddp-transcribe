---
adr_id: "0018"
comments:
    - author: Danielle McCool
      comment: "1"
      date: "2026-05-13 15:46:23"
links:
    precedes: []
    succeeds: []
status: decided
tags: []
title: Three-tier review protocol with codex-advisor delegated via Sonnet reviewer
---

## <a name="question"></a> Context and Problem Statement

Plan B Epic 1's per-epic overview (lines 95-103) documented a three-tier review where the orchestrator (Opus) called codex-advisor directly during each task review. Each invocation deposited a 200-500-word advisor reply into the orchestrator's conversation. Across Epic 1's 13 tasks with ~2 advisor calls each, this contributed materially to the compaction observed near the end of the epic. The pattern is project-wide working practice (not Epic 1-specific) and needs a meta-process ADR so future epics inherit it correctly with the working-set cost shifted off the orchestrator.

## <a name="options"></a> Considered Options

1. <a name="option-1"></a> Orchestrator calls codex-advisor directly (Plan B Epic 1 status quo)
2. <a name="option-2"></a> Sonnet spec-compliance reviewer calls codex-advisor inside its own dispatch and distills the response (<=300 words) to the orchestrator
3. <a name="option-3"></a> Skip codex-advisor entirely; rely on Sonnet for code-quality review (sacrifices model-family-diversity)
4. <a name="option-4"></a> Replace Sonnet with codex-advisor as primary spec-compliance reviewer (re-roles codex)

## <a name="criteria"></a> Decision Drivers

Orchestrator working-set budget across multi-task dispatches; preservation of model-family-diversity benefit from codex-advisor; reviewer's ability to interpret codex's signal in context of the specific brief plus diff; on-demand availability of codex's full reply via 'codex-advisor transcript' for periodic spot-checks (e.g., every 4-5 tasks).

## <a name="outcome"></a> Decision Outcome
We decided for [Option 2](#option-2) because: Sonnet reviewer calls codex-advisor inside the dispatched reviewer session, distills the response to <=300 words of actionable items, reports to the orchestrator. Reviewer prompt requests <=200-word codex responses. Orchestrator spot-checks via 'codex-advisor transcript | tail -200' every 4-5 tasks. Estimated saving: ~500 words x ~25 invocations per epic ~= 12k tokens off the orchestrator. Supersedes Plan B Epic 1 overview lines 95-103.

## <a name="comments"></a> Comments
<a name="comment-1"></a>1. (2026-05-13 15:46:23) Danielle McCool: marked decision as decided
