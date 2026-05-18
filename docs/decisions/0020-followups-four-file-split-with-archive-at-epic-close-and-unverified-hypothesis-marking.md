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

# FOLLOWUPS four file split with archive at epic close and unverified hypothesis marking

## Context and Problem Statement

docs/FOLLOWUPS.md grew to 1163 lines through Plan B Epic 1's reviews. It mixes (a) live-scope blockers that are planning input, (b) deferred-but-scheduled entries, (c) cosmetic items deferred indefinitely, (d) bake-time operational findings, and (e) post-resolution archival prose. These have different lifecycles and reading audiences. Ingesting the file at planning time forces all five categories into the orchestrator's working set, and growth is up-only.

A related discipline gap surfaced during Plan B Epic 1's post-bake retrospective (2026-05-13): two FOLLOWUPS entries had recorded unverified hypotheses (a yt-dlp format-selector workaround and a curl-cffi libcurl-linkage guess) that subsequent operators applied as if confirmed. Applying the fixes built on those guesses cost time and produced a wrong-direction commit. Future entries that record unverified hypotheses need an explicit marker so the next operator knows to verify before acting.

## Considered Options

* Status quo: single FOLLOWUPS.md, mixed content across all five categories. No structural change; all entries continue to accumulate in one file regardless of lifecycle stage.
* Four-file split: FOLLOWUPS.md carries active-scope entries with a one-line scope index at the top; cosmetic-followups.md holds indefinitely-deferred items; bake-findings.md isolates operational/bake-time findings; archive/followups-resolved.md is append-only history. At epic close, resolved entries move to archive. Unverified hypotheses are prefixed `**Hypothesis (unverified):**` in their entry.
* Per-entry files in a docs/followups/ directory (one file per finding). Each entry becomes its own markdown file with a frontmatter status field; tooling or convention groups them by lifecycle stage.
* Index-only FOLLOWUPS.md with full entries in per-task plan files. FOLLOWUPS.md becomes a thin pointer list; substantive content lives next to the plan or task that surfaced it.

## Decision Drivers

Planning-time ingestion cost (the orchestrator currently loads ~25k tokens of FOLLOWUPS content before any epic-specific planning); ease of "what's left for epic N" lookup; growth control over the project's lifetime; alignment with the epic-close-out workflow (resolved entries naturally archive at close); minimal tooling burden (one-time restructure plus a discipline at epic close, no new scripts); and the diagnostic-before-fix discipline the post-bake retro surfaced (unverified hypotheses must be marked so the next operator knows to verify before acting).

## Decision Outcome
We decided for [Option 2](#option-2) because: FOLLOWUPS.md carries active-scope entries grouped by target epic with a one-line scope index at top. cosmetic-followups.md and bake-findings.md are off the orchestrator's planning-time reading path. archive/followups-resolved.md is append-only. At epic close, resolved entries move to archive with resolving commit SHA. FOLLOWUPS entries recording unverified hypotheses must prefix them with '**Hypothesis (unverified):**' per Plan B Epic 1's post-bake retro.

## Comments

* **2026-05-13 16:06:27 — @Danielle McCool:** 1. (2026-05-13 16:06:27) Danielle McCool: marked decision as decided
