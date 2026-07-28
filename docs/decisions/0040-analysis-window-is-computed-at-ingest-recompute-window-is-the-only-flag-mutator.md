---
status: accepted
date: "2026-07-28"
category: State machine
applies_to:
    - src/ingest.rs
    - src/state/mod.rs
    - src/state/schema.rs
priority: invariant
companions:
    - tests/ingest.rs
    - tests/recompute_window.rs
---

# Analysis window is computed at ingest; recompute-window is the only flag mutator

## Decision

`watch_history.in_window` is computed once at ingest from inclusive UTC
calendar dates: `--window-start` maps to 00:00:00Z inclusive; `--window-end`
covers its whole day (the following day's 00:00:00Z is the exclusive upper
bound); an absent side is unbounded; both absent means every row is
in-window. After ingest, only the explicit `recompute-window` subcommand may
change the flag — it requires at least one of `--window-start` /
`--window-end` / `--clear` (bare invocation is a usage error), `--clear` is
the deliberate no-filter opt-in, `--dry-run` reports the row count that
would change without writing, and the mutator returns the number of rows
actually flipped (0006). The verbatim DDP `Date` string persists in
`watch_history.watched_at_raw` (schema v4); re-ingest backfills NULL raws on
existing rows but never touches `in_window`.

## Guidance

- Consumers filter `WHERE in_window = 1`; never re-derive window membership from `watched_at` at query time — `in_window` is the single flag of record.
- Day-granularity windows are deliberate: they absorb the sub-day timezone ambiguity ADR-0039 leaves unresolved ("UTC-assumed (documentary evidence), empirically unresolved") for all but boundary-adjacent rows — only rows within the ambiguity offset (~1h) of a window edge can be misclassified, and the count of such rows is bounded by the offset.
- No ingest-time or fetch-time code path may set `in_window` implicitly (e.g. "helpfully" recomputing it on a coincidental re-ingest); review rejects that. `Store::recompute_window` is the only mutator, invoked only from the `recompute-window` subcommand.
- `watched_at_raw` is never dropped, normalized, or overwritten once non-NULL — it is the hedge that makes the timezone verdict non-fatal either way.
- `recompute-window --clear` and a `--window-start`/`--window-end` pair are mutually exclusive at parse time (clap `ArgGroup`); `cli::validate_window_order` rejects `--window-start` after `--window-end` before the store opens (equal dates are a valid single-day window).

## Why

A bare `recompute-window` invocation that silently wiped a study's window
filter would be an unrecoverable-in-practice operator mistake — by the time
anyone noticed, re-deriving the correct filter from `watched_at_raw` is the
only way back, so the flag never moves without an explicit, validated
instruction. Computing `in_window` once at ingest, rather than deriving it
live at query time, keeps `status`/export queries a single indexed predicate
instead of a per-row date comparison, and keeps a prospective
`recompute-window` change previewable — via `--dry-run`'s row count — before
it writes.
