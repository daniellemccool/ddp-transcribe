---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-04-16 21:57:55"
      text: '1. (2026-04-16 21:57:55) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Split Plan A into per task files for cheaper subagent context

## Context and Problem Statement

The original Plan A document was 3347 lines. This forced subagents (and the controller) to load the full document for any single-task operation, consuming significant tokens at each implementer/reviewer dispatch and stressing thermally-constrained hardware. How should the implementation plan be organized so subagent-driven execution stays token-economical?

## Considered Options

* Keep single-file plan; tighten subagent prompts to forbid reading it
* Split into per-task files (overview + 15 tasks)
* Hybrid: keep single file plus a short overview sidecar
* Restructure dispatch: subagents never read the plan; controller pastes task text inline only

## Decision Drivers

Subagent token budget per task. Controller's own context-window economy across the dispatch loop. Maintainability of per-task content (find or edit a specific task without scrolling 3000+ lines). Discoverability for future Plan B and Plan C planning sessions. Preservation of the original holistic view (mitigated by git history).

## Decision Outcome
We decided for [Option 2](#option-2) because: Token math at session start: even if only the code-quality reviewer reads the full plan once per task, that is approximately 44k tokens times 14 remaining tasks times 2 reviews per task = ~1.2M tokens of avoidable context loading versus ~5k per per-task read. The split pays for itself within 2-3 tasks. The split also benefits the controller's own context economy — one Read per task with cache reuse versus offset-paginated reads of a 3300-line file. Implemented in commit e414240; original plan preserved at commit 43d1081. Per-task files range from 85 to 376 lines, with one overview file (191 lines) holding the front matter, conventions, exit criteria, and task index. Future Plans B and C should adopt the same structure from day one.

## Comments

* **2026-04-16 21:57:55 — @Danielle McCool:** 1. (2026-04-16 21:57:55) Danielle McCool: marked decision as decided
