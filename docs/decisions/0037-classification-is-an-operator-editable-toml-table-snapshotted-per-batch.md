---
status: accepted
date: "2026-07-08"
category: Failure classification
applies_to:
    - src/classification.rs
priority: invariant
companions:
    - src/batch.rs
---

# Classification is an operator-editable TOML table, snapshotted per batch

## Decision

yt-dlp stderr classification is driven by an ordered, first-match-wins TOML
rule table (`schema = 1`; `[[rule]]` entries of pattern, label, disposition ∈
retryable | terminal | requires-cookie) with a compiled evidence-derived
default, operator override via `--classification`, hard-fail validation at
startup, and the active table's full TOML snapshotted into
`batch_runs.policy_toml` on every run.

## Guidance

- Responding to yt-dlp wording drift or a new message class is an operator table edit, not a code release; review rejects re-hardcoding stderr patterns into the classifier chain.
- Validation hard-fails at startup (same philosophy as the schema-version gate): bad schema, empty pattern, or `requires-cookie` as the fallback disposition refuses the run. Patterns are exact, case-sensitive substrings.
- The compiled default admits only proven-pure terminal classes (IpBlockedMessage, VideoNotAvailable10231, VideoNotAvailable10240); a new terminal rule needs the evidence-derived record's bar.
- The table governs tool-output interpretation only — structural errors (timeout, spawn, missing output) stay code-mapped.
- Never skip the per-batch `policy_toml` snapshot: a census without its generating policy is not reproducible attrition documentation.

## Why

yt-dlp's wording drifts and new classes appear between releases (status code
10240 emerged as 606/606 dead mid-study); classification must move at
operator speed. The provenance snapshot exists because attrition analysis is
a research deliverable — a count of "terminal" rows is meaningless without
the exact policy that produced it.

## Alternatives

- **Hardcoded classifier chain** — every wording drift is a code release.
- **JSON config** — no comments; the table is operator documentation as much as config.
