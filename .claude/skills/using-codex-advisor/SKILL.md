---
name: using-codex-advisor
description: Use when consulting `codex-advisor` for design tradeoffs or code-quality review in this repo. Covers the 0018 delegation policy (orchestrators do NOT call codex-advisor directly during task reviews; Sonnet spec-compliance reviewers call it and distill the response), the pinned-session model, and output-cap discipline. Operational surface is in `codex-advisor --help`.
---

# Using `codex-advisor` in this repo

## The policy (0018)

**During per-task review cycles, the orchestrator (Opus) does NOT call `codex-advisor` directly.** The Sonnet spec-compliance reviewer calls it, reads the response, and reports a distilled signal back to the orchestrator (≤300 words). codex's full reply lives in the pinned-session transcript (`codex-advisor transcript`) for spot-checks; it does NOT land in the orchestrator's conversation. This keeps the orchestrator's working set lean across multi-task dispatches.

**Direct orchestrator-to-codex calls are appropriate for:**
- Brainstorming / design sessions (no per-task review cycle yet)
- Independent code-quality questions outside of task review
- One-off "second opinion" consultations on architectural choices

**Reviewer-side prompt to codex-advisor:** ask for ≤200-word responses. The reviewer's own report to the orchestrator should distill to actionable items: "codex flagged X (load-bearing because Y); flagged Z (not load-bearing, defer)."

## Pinned-session model

`codex-advisor` maintains exactly one pinned session at a time. Its UUID is queryable via `codex-advisor id`; never write it into static docs — it changes when re-pinned. The current pinned session carries the design context for whatever's active (e.g., the Plan B brainstorm session carried Plan B context through Epic 1).

## Operational surface

Run `codex-advisor --help` for the CLI surface (`init`, `orient`, `ask`, `transcript`, `id`, `reset`). The skill content above is policy; the help is reference.

## Spot-checking from the orchestrator

Every 4–5 tasks, the orchestrator can `codex-advisor transcript | tail -200` to skim for signals the reviewer's distillation may have under-weighted. Cheap; periodic.

## Re-pinning / fresh sessions

If the pinned session is lost, reset, or needs to switch context (e.g., a major plan transition), `codex-advisor init <priming-prompt>` starts a fresh role. Canonical priming prompts for this project's common roles should be drafted when needed and persisted to a future revision of this skill — but until that's been exercised, use ad-hoc prompts.
