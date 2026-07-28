---
status: accepted
date: "2026-05-13"
category: Process
applies_to:
    - docs/superpowers/plans/**/*.md
priority: default
---

# Subagent reports are capped and structured; controllers restart at phase boundaries

## Decision

Every subagent dispatch brief requires a structured report of at most 250
words (STATUS / SUMMARY / CHANGED FILES / DEVIATIONS), and plans define
explicit phase boundaries at which the controller writes a ≤1-page
`PHASE-N-CLOSE.md` handoff and ends its session; the next phase starts a
fresh controller from the spec plus the close-out doc.

## Guidance

- Every dispatch brief in a plan includes the report format and cap (with allowance for genuinely unusual cases); review rejects briefs that let full implementation transcripts flow back to the orchestrator.
- Phase boundaries are planned, not improvised: the plan names them, and the close-out doc is the only controller state that crosses one.
- The discipline lives in the briefs, not a hook — a truncating hook would mangle the unusual cases that legitimately need more words.

## Why

Default subagent verbosity plus a single controller session across an epic
is what compacted Epic 1's orchestrator; the cap and the restart attack the
same working-set problem at the per-task and per-phase levels, cutting
30–50% of per-task result context.

## Alternatives

- **No discipline** — the compaction status quo.
- **Hook-enforced truncation** — extra surface to maintain; breaks the legitimate over-budget report.
- **Cap without restarts** — leaves per-phase accumulation unaddressed.
