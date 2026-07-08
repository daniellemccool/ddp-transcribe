---
status: accepted
date: "2026-07-08"
comments:
    - author: Danielle McCool
      date: "2026-07-08 12:04:28"
      text: marked decision as decided
---

# Operator-editable TOML classification table with compiled evidence-derived default and batch provenance

## Context and Problem Statement

Epic 3 hardcoded yt-dlp stderr classification in src/failure.rs. yt-dlp
wording drifts and new message classes appear (status code 10240 emerged
as 606/606 dead at the 2026-07-07 census); responding must be an operator
table edit, not a code release.

## Considered Options

* Hardcoded classifier chain (Epic 3 status quo)
* Ordered TOML rule table (compiled default, file override, hard-fail validation, provenance snapshot per batch)
* JSON config (zero new deps, no comments)

## Decision Outcome

Chosen option: "Ordered TOML rule table (compiled default, file override, hard-fail validation, provenance snapshot per batch)", because yt-dlp wording drifts; an operator table edit must respond to new dead classes without a code release, and each census must carry its generating policy.

### Consequences

`schema = 1`; ordered `[[rule]]` entries `{pattern, label, disposition ∈
retryable | terminal | requires-cookie}`; first-match-wins; explicit
fallback (requires-cookie forbidden as fallback); exact case-sensitive
substrings; validation hard-fails at startup per 0022 philosophy. The
compiled default is the evidence-derived census table (only proven-pure
classes are terminal: IpBlockedMessage, VideoNotAvailable10231,
VideoNotAvailable10240; NoPermission stays retryable at 25/452 alive).
Config governs tool-output interpretation only — structural errors stay
code-mapped. The active table's full TOML snapshots into
`batch_runs.policy_toml` per run: a census without its generating policy is
not reproducible attrition documentation.

## Comments

* **2026-07-08 12:04:28 — @Danielle McCool:** marked decision as decided
