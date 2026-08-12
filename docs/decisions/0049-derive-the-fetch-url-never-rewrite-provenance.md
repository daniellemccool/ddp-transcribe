---
status: accepted
date: "2026-08-12"
source: docs/superpowers/specs/2026-08-12-census-completion-strategy-design.md
category: Fetcher
applies_to:
    - src/pipeline/mod.rs
    - src/canonical.rs
    - src/backfill.rs
    - src/state/mod.rs
    - src/output/artifacts.rs
priority: invariant
---

# Derive the fetch URL, never rewrite provenance

## Decision

For `canonical = 1` claims, the URL handed to yt-dlp is derived from the
primary key at fetch time — `https://www.tiktok.com/@x/video/<video_id>/`
— via one helper (`canonical::derived_fetch_url`), never by fetching
stored `source_url`. `source_url` remains immutable DDP provenance, never
rewritten in bulk or mutated by transport concerns: transport form is
code plus `params_json`, not data.

## Guidance

- Every production yt-dlp URL consumer uses the same helper — pipeline
  fetch (`src/pipeline/mod.rs`) and `backfill-metadata` (`src/backfill.rs`)
  alike; review rejects a second URL-format string literal anywhere in the
  codebase.
- Transcript artifacts carry `source_url` exactly as stored, untouched by
  the derived fetch form (ADR-0010 / ADR-0042 posture); `src/output/
  artifacts.rs` must never swap in the derived URL when writing
  provenance fields.
- The `@x` placeholder is deliberate: `CANONICAL_RE` (`src/canonical.rs:
  23-28`) requires a non-empty user segment (`@[^/]+`), and an empty `@`
  URL would classify `Invalid` if it ever re-entered canonicalization. Any
  non-empty segment fetches identically (verified 2026-08-11).
- Non-canonical rows (`canonical = 0`, Plan C short-links) keep fetching
  their stored `source_url`; review rejects widening derivation to them
  without their own evidence.
- Cookie scoping (ADR-0035) and format policy (ADR-0038) read the claim,
  not the URL — derivation must not alter either decision point.
- `Claim` (`src/state/mod.rs:424`, returned by `claim_next`) does not yet
  expose the DB's `canonical` column — only `video_id`, `source_url`,
  `attempt_count`, `last_retryable_kind`. Adding a `Claim.canonical` field
  is this decision's own DB-to-claim plumbing, not a pre-existing input;
  cookie scoping (ADR-0035) and format policy (ADR-0038) keep branching on
  `last_retryable_kind` alone, unchanged — `canonical` must not become a
  second input to either.

## Why

The 2026-08-06 and 2026-08-10 WAF incidents blocked the stored share-host
URL form (`www.tiktokv.com/share/video/…`) at the first hop; the canonical
form fetches unimpersonated (probe matrix in
`docs/operations/incident-2026-08-10-tiktok-waf-impersonation-block.md`,
on its branch if not yet merged — cite by path regardless). The
alternative — a 1.93M-row `UPDATE` of `source_url` (that record's
"remaining steps" handoff) — mutates researcher-visible provenance and is
superseded by this decision; that SQL must never run.
