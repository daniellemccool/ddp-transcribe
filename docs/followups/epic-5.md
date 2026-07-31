# FOLLOWUPS — Epic 5 active entries

No active entries. Plan B Epic 5 is closed: Epic 5a shipped the
campaign-safety slice (v0.3.2) and Epic 5b the Plan-B close-out (v0.4.0).

All twenty-one entries that were carried here are archived in
`../archive/followups-resolved.md`, section "Resolved by Plan B Epic 5b —
close-out slice / v0.4.0 (2026-07-30)", each with its resolving commit SHA.
Three of them are archived as **accepted** rather than fixed, under operator
rulings recorded 2026-07-30 in
`../superpowers/plans/2026-07-30-epic-5b-plan-b-closeout/DISPOSITION-MATRIX.md`:
the tmp sweep's TOCTOU window, and items 1–2 of the ingest file-ledger bundle
(basename-only ledger key; the one-second-resolution `(size, mtime)` change
detector). Each keeps its evidence-gated re-open condition in the archive — a
re-opening is filed as a **new** entry, never a second resolution appended to
the archived one.

Nothing was dropped silently. The `reset-stale-claims` operator subcommand that
Plan B's Epic 5 sketch anticipated never became a FOLLOWUPS entry and is
**superseded** by the startup stale-claim sweep, which recovers claims at every
process start with per-row `swept_stale` forensics since Epic 5a; the sketch's
`requeue-retryables` shipped as `requeue-failures`
([ADR-0046](../decisions/0046-requeue-failures-is-a-forensic-default-deny-override-of-retry-eligibility.md)),
renamed because terminal rows are eligible under `--include-terminal`.

See `../FOLLOWUPS.md` for the scope index across all epics.
