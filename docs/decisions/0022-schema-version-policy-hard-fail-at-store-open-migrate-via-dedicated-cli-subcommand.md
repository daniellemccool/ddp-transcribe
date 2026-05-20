---
status: accepted
date: "2026-05-18"
comments:
    - author: Danielle McCool
      date: "2026-05-18 22:34:04"
      text: marked decision as decided
---

# Schema-version policy: hard-fail at Store::open, migrate via dedicated CLI subcommand

## Context and Problem Statement

Plan B Epic 2 introduces four new nullable columns and bumps SCHEMA_VERSION "1" → "2". Plan A's `Store::open` records the version on first run via `INSERT OR IGNORE INTO meta` but never reads it back; opening an older DB silently runs against newer code, with whatever-happens-happens semantics on missing columns. FOLLOWUPS T7 (`Store::open` schema-version not read) tracks the gap. Epic 2 needs a policy that (a) refuses to open a DB at the wrong version, (b) gives the operator a clear migration path, (c) preserves Plan A bake data, and (d) sets a precedent for Epic 3+ schema bumps.

## Considered Options

* Hard-fail at Store::open + dedicated `migrate` CLI subcommand
* Auto-migrate on open (silently apply ALTER TABLE if version mismatch)
* Log-and-warn on mismatch but continue
* Wipe-and-re-ingest (delete DB on mismatch; force operator to re-ingest)

## Decision Drivers

- Operational visibility (operator must know what happened)
- Preserves bake data and donor-watched-video history
- Doesn't tempt silent drift in production
- Sets a precedent that scales to Epic 3+ schema bumps

## Decision Outcome

Chosen option: "Hard-fail at Store::open + dedicated `migrate` CLI subcommand", because Auto-migrate carries silent-drift risk; log-and-warn relies on operators reading warnings; wipe-and-re-ingest loses Epic 1 bake artifacts and donor history (no source-of-truth restore path). Hard-fail forces the operator action and preserves data..

## Consequences

- `Store::open` returns typed `SchemaVersionMismatch { expected, found }` containing operator-readable instructions directing them to `uu-tiktok migrate`.
- The `migrate` subcommand opens the DB raw (bypassing the version check), runs `ALTER TABLE videos ADD COLUMN ... NULL` × 4 + UPDATE on the meta row in one transaction, and exits. Idempotent on already-v2 DBs.
- Every Epic 2+ schema bump becomes a new ALTER block in the `migrate` subcommand (idempotent layering).
- Pre-Epic-2 DBs require one operator action (`uu-tiktok migrate`) before `process` works.
- Tests cover both directions: opening a v1 DB without migration fails with the typed error; opening a v2 DB succeeds; running `migrate` on a v2 DB is a no-op.

## Comments

* **2026-05-18 22:34:04 — @Danielle McCool:** marked decision as decided
