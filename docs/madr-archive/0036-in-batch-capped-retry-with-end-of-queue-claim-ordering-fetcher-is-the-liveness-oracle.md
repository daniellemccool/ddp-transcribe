---
status: accepted
date: "2026-07-08"
supersedes:
    - "0034"
comments:
    - author: Danielle McCool
      date: "2026-07-08 12:03:58"
      text: marked decision as decided
    - author: Danielle McCool
      date: "2026-07-08 12:05:09"
      text: marked decision as decided
    - author: Danielle McCool
      date: "2026-07-08 12:39:35"
      text: marked decision as decided
---

# In-batch capped retry with end-of-queue claim ordering; fetcher is the liveness oracle

## Context and Problem Statement

Epic 3 shipped operator-driven triage: an oEmbed probe adjudicated parked
failures and a manual subcommand requeued them. The 2026-07-07 census
(n=7,087) showed the probe re-confirming settled classes, the operator flow
added ceremony per batch, and dry-run + execute double-probed. The operator
ruled retry must be pipeline behavior.

## Considered Options

* Operator-driven probe triage (Epic 3 status quo, ADR 0034)
* In-batch capped retry: failure-time requeue to end-of-queue, re-fetch adjudicates liveness (fetch-as-oracle)
* Automatic backoff/jitter retry inside the workers

## Decision Outcome

Chosen option: "In-batch capped retry: failure-time requeue to end-of-queue, re-fetch adjudicates liveness (fetch-as-oracle)", because in-pipeline retry replaces operator triage; the re-fetch is the liveness oracle, and the census shows self-classification handles impure classes where blanket write-offs would discard recoverable videos.

### Consequences

`record_fetch_failure` decides requeue/exhaust/park in one transaction at
failure time; claim ordering becomes `attempt_count ASC, first_seen_at ASC,
video_id ASC` (fresh work drains before retries); `--retries` default 1 caps
LIFETIME attempts at `retries + 1` against `attempt_count` (bumped at claim
time); `--max-videos` counts every claim including retries; a start-of-batch
sweep adjudicates parked rows through the classification table so historical
pools and cross-batch stragglers ride the same mechanism. Dead classes
self-classify on re-fetch (write-off message → inline terminal), which the
census showed handles impure classes (NoPermission 25/452 alive) correctly
where blanket write-offs would discard recoverable videos. Supersedes 0034;
the probe retires with the census as its closing evidence.

Accepted tradeoff (claim ordering): `attempt_count ASC` is fresh-first, so
under a hypothetical continuous fresh-arrival load — a future daemon mode
that keeps ingesting while draining — retries could starve behind an
unending fresh supply. This is fine for the current batch-drain model
(0026): a batch has a finite pending pool that empties, after which retries
are all that remain and drain to completion. Revisit ordering (e.g. an
age/attempt interleave) only if a daemon mode lands.

## Comments

* **2026-07-08 12:03:58 — @Danielle McCool:** marked decision as decided
* **2026-07-08 12:05:09 — @Danielle McCool:** marked decision as decided
* **2026-07-08 12:39:35 — @Danielle McCool:** marked decision as decided
