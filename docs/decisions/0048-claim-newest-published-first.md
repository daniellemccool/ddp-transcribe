---
status: accepted
date: "2026-08-12"
source: docs/superpowers/specs/2026-08-12-census-completion-strategy-design.md
category: State machine
applies_to:
    - src/state/mod.rs
    - src/state/schema.rs
    - src/state/migrate.rs
    - tests/state_claims.rs
priority: invariant
---

# Claim newest-published first

## Decision

`claim_next` orders candidates `attempt_count ASC, video_id DESC` — lower
attempt_count claims first (unchanged), then publication recency, newest
first, using the TikTok snowflake property that the upper 32 bits of
`video_id` encode creation epoch (validated corpus-wide 2026-08-12: 0 of
4,580,091 watch events precede their video's decoded creation).
`first_seen_at` leaves the claim order.

## Guidance

- Within each attempt tier, a truncated campaign is a *complete census of
  all videos created after a cutoff* — `attempt_count ASC` has precedence
  over recency, and the operator explicitly accepted a one-time deviation
  from that guarantee: ~57k attempt-0 rows claim ahead of the
  recency-ordered attempt-1 pool (census-completion spec §8, resolution
  3, 2026-08-12 — see the third bullet for the attempt-count invariant
  that bounds it). Outside that named, bounded exception, the ordering is
  the study's truncation-bias posture (operator-ratified 2026-08-12;
  measurement in the census-completion spec §2): removal hazard is
  front-loaded (~15% in month one, ~20% by six months), so newest-first
  fetches the perishable stock first and concentrates observation where
  removal-censoring is smallest.
- Lexicographic `video_id DESC` equals numeric order only at fixed width;
  the v7 migration asserts 19-digit uniformity over canonical rows and
  refuses otherwise. Review rejects ordering changes that reintroduce
  width-sensitive comparisons without that guard.
- `attempt_count ASC` keeps precedence: retry fairness is unchanged, and
  attempt counts are never reset to re-sort the pool (ADR-0046 stands;
  operator ruling 2026-08-12 accepts the one-time attempt-0 head-start).
- The claim index must match the claim ORDER BY (`(status, attempt_count,
  video_id DESC) WHERE status='pending'`); review rejects order/index
  drift — a mismatched pair turns every claim into a 1.9M-row sort under
  the store lock.

## Why

Ingest fairness (`first_seen_at`) has no remaining purpose once a campaign
can be interrupted at any point: the ordering decides what an interrupted
run *means*. Newest-published-first turns a truncated campaign into a
statable, defensible object — a complete census of everything created
after a cutoff — instead of an arbitrary partial sample, and it does so
while minimizing exposure to TikTok's front-loaded removal hazard, which
would otherwise bias the retained corpus against exactly the content most
likely to vanish before it can be fetched.
