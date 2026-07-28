---
status: superseded by ADR-0036
date: "2026-07-08"
comments:
    - author: Danielle McCool
      date: "2026-07-07 04:37:05"
      text: marked decision as decided
---

# Operator triage subcommand: oEmbed oracle via curl subprocess, message-class fast path, attempt-capped requeue

## Context and Problem Statement

failed_retryable is a sink on current main: claim_next selects only pending and
nothing resets failed rows. The 65k run left 7,087 rows there, of which ~2,400
are recoverable (probe-alive) and ~4,400 are dead. The architecture docs
promise "Epic 3 adds retry policy". Where does retry execution live, and how
are dead rows distinguished from recoverable ones at scale?

## Considered Options

* Single operator-driven `triage` subcommand: message-class fast path for write-off classes, oEmbed probe (curl via bounded process::run) for the rest; dead → failed_terminal, alive → pending under attempt cap; operator re-runs `process`
* Automatic in-pipeline retry with per-kind backoff
* Probe inside the pipeline classifier at failure time (network call on the hot path)

## Decision Outcome

Chosen option: "Single operator-driven `triage` subcommand: message-class fast path for write-off classes, oEmbed probe (curl via bounded process::run) for the rest; dead → failed_terminal, alive → pending under attempt cap; operator re-runs `process`", because keeps the pipeline hot path network-pure; message-class fast path + oEmbed probe gave perfect separation; requeue-with-kind-write-back normalizes historical rows without wasted refetches.

## Consequences

* The pipeline hot path stays network-pure; TikTok endpoint availability can
  never stall `process`.
* Probe transport is the system `curl` binary through process::run (argv
  direct, bounded capture per 0021, explicit timeout) — no new HTTP-client
  dependency. `curl` on PATH becomes a runtime requirement for `triage` only.
* Probe oracle: GET https://www.tiktok.com/oembed?url=https://www.tiktok.com/@x/video/<id>;
  HTTP 200 → alive, 400/404 → dead, anything else → unreachable (row untouched).
  Validated 2026-07-06/07 (n=36, perfect separation). External-endpoint drift
  risk: re-validate on a sample if verdict distributions shift.
* Requeue is capped at attempt_count < 3 by default (--max-attempts). Requeue
  re-classifies the stored message and writes the normalized kind back, so
  historical placeholder-"Fetch" rows acquire taxonomy kinds without a wasted
  refetch.
* Both triage transitions write video_events rows (triaged_terminal, requeued)
  — operator actions are auditable, unlike the 0024 sweep.
* The per-kind census output is the study's attrition documentation.

## Comments

* **2026-07-07 04:37:05 — @Danielle McCool:** marked decision as decided
