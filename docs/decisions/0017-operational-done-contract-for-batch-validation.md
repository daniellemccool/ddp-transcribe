---
status: accepted
links:
    precedes: []
    succeeds: []
comments:
    - author: Danielle McCool
      date: "2026-05-12 13:07:38"
      text: '1. (2026-05-12 13:07:38) Danielle McCool: marked decision as decided'
legacy-outcome: true
---

# Operational done contract for batch validation

## Context and Problem Statement

When can an operator declare a batch 'done' and safe to spin down the workspace? Plan A's exit-3 mechanism (process returned 3 = nothing to claim) is insufficient — it doesn't verify artifacts on disk or schema compliance.

## Considered Options

* Define the contract in this ADR; implement in Epic 4's status subcommand. Contract = counts by status (all terminal), all succeeded rows have artifacts on disk, all raw_signals.schema_version match expected.
* Implement in Epic 1. Adds scope to Epic 1.
* Don't define until Epic 4. Risk: implementer of Epic 4 has no contract to fulfill.

## Decision Drivers

Contract must be precise enough to implement. Must be Epic-1-draftable but Epic-4-implementable. Must integrate with 0011's spin-down practice.

## Decision Outcome
We decided for [Option 1](#option-1) because: ADR drafted now (Epic 1) so the status subcommand has a clear contract to implement. Subcommand itself lands in Epic 4. Contract (formal): Counts by status — every row in videos has terminal status (no in_progress, no pending unless explicitly skipped via --max-videos). Artifact existence — every succeeded row has .txt and .json at the sharded path. Schema-version check — every .json's raw_signals.schema_version matches EXPECTED_RAW_SIGNALS_SCHEMA_VERSION. Optional — artifact backup to Research Drive completed (if configured). Pause-safe — all of the above pass AND no in_progress rows pending recovery. Cross-references: 0011 (spin-down practice) consumes the pause-safe check; 0010 defines raw_signals.schema_version that the schema-version check validates. Rejected alternatives: Option 2 (implement in Epic 1) — adds scope to Epic 1 without delivering the user-facing value that lives in Epic 4 (operator-facing status command); the contract is small enough to draft now but the implementation reads DB and filesystem state that Epic 4's subcommand harness owns. Option 3 (don't define until Epic 4) — leaves Epic 4's implementer with no contract to fulfill and risks the contract being shaped to fit whatever Epic 4's implementation happens to do, rather than what operators actually need.

## Comments

* **2026-05-12 13:07:38 — @Danielle McCool:** 1. (2026-05-12 13:07:38) Danielle McCool: marked decision as decided
