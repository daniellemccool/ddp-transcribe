---
status: accepted
date: "2026-07-28"
category: Orchestration
applies_to:
    - src/status.rs
    - src/state/queries.rs
priority: invariant
companions:
    - tests/status.rs
---

# status is the read-only operator surface; the archived done-contract lives behind --verify

## Decision

`status` is DB-only and read-only by default, rendering an open/interrupted
`batch_runs` row honestly instead of skipping it or crashing on its NULLs.
`status --verify` runs the archived operational done-contract — per-shard
batched artifact-existence checks, a full `raw_signals.schema_version`
parse, and a pause-safe verdict (`pending == 0 ∧ in_progress == 0 ∧ zero
artifact/schema/read failures`) — exiting 1 when violated, 0 when pause-safe.

## Guidance

- `status` never mutates study state, and it bails rather than creates when the DB is missing — it's a report, not an initializer.
- The default surface is counts by lifecycle status, `failed_retryable` broken down by kind, in-progress claim ages, and the full `batch_runs` history. Detail modes `--video-id` (legible `detail_json` event history), `--respondent-id`, `--errors`, `--retryable` conflict with each other and with `--verify` at parse time (`--errors` and `--retryable` may combine); `--json` serializes the same report structs as the stable tooling schema.
- Interrupted `batch_runs` rows (`finished_at IS NULL`) render honestly (e.g. "INTERRUPTED, no census"); never skip them, never crash on their NULL `census_json`.
- `--json` output carries raw stored values — the legacy `"Fetch"` placeholder kind's "(legacy placeholder kind)" annotation is a human-render-only decoration, not a JSON field; don't launder it into the JSON schema.
- `raw_signals.schema_version` **sampling** (checking a subset instead of every artifact) is out of scope at Plan B batch sizes — a Plan C concern only if corpus size demands it; don't add it here.
- New operator-facing surfaces extend `status` in-tool per the operator-interface premise (the binary is the interface, wrapper scripts are not — archived `docs/madr-archive/0032-*`); don't grow a parallel reporting script.
- This record is the lean successor to archived `docs/madr-archive/0017-operational-done-contract-for-batch-validation.md` (0017 predates the lean migration and stays frozen there for its Context/Considered-Options prose).

## Why

The 2026-07-08 production batch produced both problems this record closes:
an interrupted `process` run whose only honest record was an open
`batch_runs` row nobody could render without special-casing, and a by-kind
retryable breakdown the operator repeatedly hand-wrote as ad hoc SQL against
`videos`. Freezing the done-contract behind `--verify`, rather than always
running it, keeps routine `status` calls cheap — the artifact walk and JSON
parse are only paid when an operator is deciding whether it's safe to pause
the workspace. Ground-truthed against that batch's snapshot: 51,903
succeeded / 3,928 failed_terminal / 789 failed_retryable; six-kind
`failed_retryable` breakdown.
