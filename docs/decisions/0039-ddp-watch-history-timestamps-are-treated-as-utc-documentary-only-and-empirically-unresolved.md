---
status: accepted
date: "2026-07-14"
category: Ingest
applies_to:
    - src/ingest.rs
    - src/state/schema.rs
    - src/state/mod.rs
priority: invariant
---

# DDP watch-history timestamps are treated as UTC, documentary-only and empirically unresolved

## Decision

`parse_watched_at` (`src/ingest.rs`) interprets DDP `Date` strings as UTC
(`Utc.from_utc_datetime`) — a documentary-evidence verdict, not an
empirically confirmed one. TikTok's May-2026 export pipeline labels its
output `" UTC"` (commit `2d89860`); an operator spot-check could not
discriminate UTC from local time at ±1h precision. The convention is
treated as UTC-assumed, empirically unresolved, for both the labeled
(May-2026) and unlabeled (July-2026) export formats.

## Guidance

- Treat `watch_history.watched_at` as UTC-**assumed**, not UTC-confirmed.
  Consumers compare it against UTC instants but must use day-granularity
  windows (never sub-day windows) so the residual ~1h-or-more uncertainty
  cannot flip a row across a boundary.
- `watched_at_raw` (schema v4, Epic 4b) MUST be preserved verbatim alongside
  the parsed `i64` — it is the hedge that makes this verdict's unresolved
  empirical status non-fatal. Never drop it to save space.
- `parse_watched_at`'s `FORMATS` comments cite this ADR and record which
  format is the labeled convention (`" UTC"` suffix, May-2026 PI bake) versus
  the unlabeled one (no suffix, July-2026 real-donor exports, retained by
  pipeline continuity with the labeled convention, not independent evidence).
- A new `Date` format variant surfacing in `date_parse_failures`, or a future
  export batch that permits a tighter spot-check (distinctive, dateable watch
  events rather than a general session), re-opens this record.

## Why

TikTok's May-2026 export pipeline stamps its own output with a literal
`" UTC"` suffix (commit `2d89860`) — the most economical reading: the
backend convention is UTC and the July-2026 renderer dropped the label, not
the convention. The empirical spot-check compared the operator's memory
against two known sessions (2026-02-18 20:15, 2025-12-21 01:42); her
verbatim answer — "Can't discriminate — one hour is within my memory's
error bars for these sessions" — neither confirmed nor contradicted UTC at
the ±1h NL offset. Recording "unresolved" rather than "confirmed" keeps the
trail honest; `watched_at_raw` turns a wrong guess into a query fix, never a
re-ingest.
