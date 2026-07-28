---
status: accepted
date: "2026-07-08"
category: Fetcher
applies_to:
    - src/pipeline/mod.rs
    - src/fetcher/mod.rs
    - src/fetcher/ytdlp.rs
priority: default
---

# Fetch format is download-first; the frugal selector runs only on NoDataBlocks retries

## Decision

Fresh claims and every retry kind except one fetch with the pilot-proven
`download`-first format selector (`DeterministicAudio`); the frugal
(smallest-audio-first) selector is keyed exclusively to retries whose
`last_retryable_kind` is `NoDataBlocks`, where `download`-first is
unrecoverable by construction. This is a staged experiment, not a default
flip — the instrumented retry batch is the at-scale evidence a future flip
would need.

## Guidance

- `format_policy_for` (`src/pipeline/mod.rs`, beside the cookie gate — same composition point, same `last_retryable_kind` read, disjoint keyed labels) is the single decision point; review rejects format-selection logic at other call sites.
- Don't flip the default to frugal without the decision trigger: the NoDataBlocks backlog retry batch run to completion AND a census showing (1) ~zero `FfprobePostprocess` fallout from frugal retries and (2) a failure mix no worse than download-first's. The open yt-dlp liar-metadata issue (#16622 — ABR variants tagged `acodec=aac` that serve video-only) is a live risk against exactly the formats frugal prefers.
- The gate keys on the pinned literal label `NoDataBlocks` — it has no disposition to resolve through the classification table, so an operator table that renames that label **silently** disables the frugal retry — the loader performs no pinned-label check. Custom tables must keep the label verbatim.
- `record_fetch_failure` writes the policy tag (`"deterministic-audio"` / `"frugal"`) into every fetch-failure event's detail JSON; keep that instrumentation — per-policy attribution from the event stream IS the experiment.

## Why

The frugal probe (n=17 fresh videos, ~66% byte reduction, 17/17 real audio)
is not enough evidence to displace a pilot-scale download-first record with
an open upstream issue against the frugal formats — but the parked
NoDataBlocks backlog (~2,318 rows, 2,311 alive) is a pure win: those rows
died mid-transfer *after* `download` was selected, so a selection-time
fallback chain can never recover them and the retry must not re-pick
`download`. Staging captures the proven win and manufactures the missing
evidence in one move.

## Context

An operator evidence review on 2026-07-08 reversed this record's original
frugal-by-default decision to the staged form; the majority path stays
byte-identical argv to the pilot. A success signal is reconstructible from
the event chain without new bookkeeping: `failed_retryable`
(kind=NoDataBlocks) → later `claimed` → `succeeded` is a frugal success.

## Alternatives

- **Frugal-first unconditional default** (the original decision) — under-evidenced; reversed on review.
- **No frugal path at all** — abandons the unrecoverable backlog and the footprint learnings.
