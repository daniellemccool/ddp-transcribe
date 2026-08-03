---
status: accepted
date: "2026-05-18"
category: State machine
applies_to:
    - src/state/mod.rs
    - src/state/schema.rs
    - src/state/migrate.rs
priority: default
companions:
    - tests/state_open.rs
    - tests/state_migrate.rs
    - tests/state_schema_version.rs
---

# Migrate only via the subcommand

## Decision

`Store::open` reads the recorded schema version and hard-fails on mismatch
with a typed `SchemaVersionMismatch { expected, found }` error carrying
operator instructions. Migration only happens through the dedicated `migrate`
CLI subcommand — never automatically on open.

## Guidance

- A schema change bumps `SCHEMA_VERSION` (`src/state/schema.rs`) and adds an idempotent ALTER block to the `migrate` subcommand (`src/state/migrate.rs`), applied with the version bump in one transaction; review rejects any auto-migrate-on-open path or a schema change that skips either half.
- `migrate` opens the DB raw (bypassing the version gate), is a no-op on an already-current DB, and refuses downgrades (recorded version newer than the binary).
- Test both directions for every bump: opening an old DB fails with the typed error; `migrate` on a current DB is a no-op.

## Why

The pre-policy `Store::open` recorded the version but never read it back, so
an old DB silently ran against newer code with whatever-happens-happens
semantics on missing columns. Hard-fail forces one visible operator action
while preserving bake data and donor watch history — auto-migrate invites
silent drift, and wipe-and-re-ingest has no restore path for the bake
artifacts.

## Alternatives

- **Auto-migrate on open** — silent drift; the operator never learns the DB changed shape.
- **Log-and-warn but continue** — relies on operators reading warnings; same silent-drift end state.
- **Wipe-and-re-ingest on mismatch** — destroys bake artifacts and donor history with no source-of-truth restore.
