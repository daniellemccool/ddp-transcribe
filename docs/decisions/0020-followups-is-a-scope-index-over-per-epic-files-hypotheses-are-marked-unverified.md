---
status: accepted
date: "2026-05-13"
category: Process
applies_to:
    - docs/FOLLOWUPS.md
    - docs/followups/**/*.md
    - docs/cosmetic-followups.md
    - docs/bake-findings.md
    - docs/archive/followups-resolved.md
priority: default
---

# FOLLOWUPS is a scope index over per-epic files; hypotheses are marked unverified

## Decision

Review debt is split by lifecycle: `docs/FOLLOWUPS.md` carries the
active-scope index (entry bodies in per-epic `docs/followups/*.md` files,
loaded only at task expansion); `docs/cosmetic-followups.md` and
`docs/bake-findings.md` sit off the planning-time reading path;
`docs/archive/followups-resolved.md` is append-only history. An entry
recording an unverified hypothesis prefixes it `**Hypothesis
(unverified):**`.

## Guidance

- Add an entry by appending the body to the right `docs/followups/<group>.md` and a one-line pointer to the FOLLOWUPS.md scope index; review rejects full entry bodies accumulating in the index file.
- At epic close, move resolved entries to the archive **with the resolving commit SHA** — never just delete.
- Never record a guess as a finding: unmarked hypotheses have been applied as confirmed fixes before, producing wrong-direction commits. The marker is what tells the next operator to verify first.
- Cosmetic and bake-finding items go to their own files, not the active index — planning-time ingestion cost is the budget this structure protects.

## Why

The single-file FOLLOWUPS grew to 1,163 lines of mixed-lifecycle content
(~25k tokens) that every planning session ingested wholesale; the split
keeps the orchestrator's planning read to the index, and the hypothesis
marker exists because two unverified guesses were once applied as
confirmed and cost a wrong-direction commit.

## Alternatives

- **Single mixed file** — up-only growth on the planning-time reading path.
- **One file per finding** — tooling burden without the lifecycle separation that actually matters.
