# news_orgs fixture

20 public TikTok URLs from Dutch news organizations, used as the
bake/regression input by:

- `docs/SRC-BAKE-CHECKLIST.md` — operator runbook for fresh-bake runs
  on the SRC A10 workspace
- `docs/SRC-BAKE-NOTES.md` — captured findings per bake (Plan A walking
  skeleton, Plan B Epic 1, Plan B Epic 2 N=3 throughput)
- Plan B Epic 2 task `20-bake-orchestrator.md` — the N=3 vs N=1 throughput
  comparison + coordinated-shutdown drill against this fixture

## Naming caveat (discoverability)

This directory is `news_orgs/` (with underscore), but the JSON file inside
is `participant=newsorg-fixture_source=tiktok.json` — `newsorg` *without*
an underscore. The file naming follows the DDP export convention
(`participant=<name>_source=<platform>.json`) which collapses the
multi-word participant name.

A `grep -r 'news_orgs'` finds the directory references; a `grep -r
'newsorg'` finds the filename references. Both spellings refer to the
same fixture. Renaming was considered (would let one grep find both) but
deferred: the filename matches the DDP-export shape and any bake scripts
that hardcode the path would need updating in lockstep.

## Format

DDP-export JSON shape. Loaded by the ingest path in `src/ingest.rs` via
`Store::upsert_watch_history` (one row per video URL).
