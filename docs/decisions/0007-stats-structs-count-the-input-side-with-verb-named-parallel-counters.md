---
status: accepted
date: "2026-04-17"
category: Code conventions
applies_to:
    - src/ingest.rs
    - src/pipeline/mod.rs
    - src/batch.rs
priority: default
---

# Count the input side in stats

## Decision

Stats structs count what the run observed on the input side (verbs like
`*_processed`, `*_seen`, `*_skipped`, `*_failures`), never what the database
did; the DB-side verbs (`_inserted`, `_ignored`, `_updated`, `_deleted`) are
reserved and must not be used for input-side counts. Counters are parallel,
not nested: each input row increments exactly one of processed / skipped /
failed.

## Guidance

- New stats structs (and new fields on existing ones) use input-side verbs; review rejects a field named `_inserted` that actually counts input, and rejects `_processed` semantics that include skipped/failed rows (reconstruct the total as the sum of the parallel counters).
- Idempotence tests assert "same input → same metrics"; if a caller genuinely needs "did the DB grow", that is a separate `Store` query, not a second counter family on the struct.
- Per-attempt semantics follow: a fail-once-then-recover video counts `claimed = 2, failed = 1, succeeded = 1` — attempts, not distinct videos.

## Why

DB-side counters log "0 0 0 0" on an idempotent re-run — useless to the
operator asking "did I read the same input?" — and ambiguous names are a
proven bug source: the convention was recorded after an implementation
tracked DB-side counts while its test asserted input-side, with the field
name making both readings look correct.

## Alternatives

- **DB-side counters** — uninformative on re-run; wrong shape for retry/recovery operations.
- **Both families on one struct** — doubles every struct's field count for a need that hasn't materialized.
- **Primitive counters, callers derive** — pushes interpretation to every log-emission site.
