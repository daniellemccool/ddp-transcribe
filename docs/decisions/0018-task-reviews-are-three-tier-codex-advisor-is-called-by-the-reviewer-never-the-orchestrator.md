---
status: accepted
date: "2026-05-13"
category: Process
applies_to:
    - docs/superpowers/plans/**/*.md
priority: default
---

# Task reviews are three-tier; codex-advisor is called by the reviewer, never the orchestrator

## Decision

Task review runs three tiers, and the orchestrator never calls
codex-advisor directly during task reviews: the dispatched spec-compliance
reviewer invokes codex-advisor inside its own session and distills the
response to ≤300 words of actionable items for the orchestrator.

## Guidance

- Plan and dispatch briefs encode the delegation: the reviewer's brief includes the codex-advisor call (requesting ≤200-word replies) and the distillation cap; review rejects a plan that routes advisor output through the orchestrator's conversation.
- The orchestrator spot-checks the full advisor signal on demand (`tail -200 "$(codex-advisor transcript)"` — the subcommand prints the log's path, not its content) every 4–5 tasks rather than ingesting every reply.
- The three tiers, per dispatch: the implementer (brief-verbatim code + deviation honesty), the Sonnet spec-compliance reviewer (mechanical does-this-match-the-brief + declared-ADR check), and the codex-advisor code-quality review (subtle correctness, cross-file consistency, testing gaps) — delegated through the second tier.
- Keep codex-advisor in the loop rather than dropping to a single model family — the diversity of failure modes is the tier's value.

## Why

Direct advisor calls deposited 200–500 words per invocation into the
orchestrator across ~25 invocations per epic — a material driver of the
context compaction observed at Epic 1's end. Delegation moves that cost
into disposable reviewer sessions (~12k tokens per epic off the
orchestrator) without losing the model-diversity signal.

## Alternatives

- **Orchestrator calls codex directly** — the compaction-causing status quo this replaced.
- **Drop codex-advisor** — sacrifices model-family diversity in review.
- **codex as the primary reviewer** — re-roles it away from the second-opinion function that makes it useful.
