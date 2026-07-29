# Kickoff — production-run operations & backfill session (post-v0.3.0)

**Written:** 2026-07-29, at the close of the go-live session. Read with
`docs/FOLLOWUPS.md` (scope index) and `docs/operations/src-vm.md` (runbook).
Working disciplines per `CLAUDE.md`; three-tier review per 0018 (codex-advisor
on every non-trivial task); implementers on Opus per operator preference.

## Where the production run stands (verified against the 2026-07-29 snapshot)

- **The campaign is LIVE** on the SRC workspace (`uutiktok`, A10, two GPU
  instances) running **v0.3.0** — the first release under the ADR-0043
  tag-and-relaunch contract (catalog `pipeline_git_ref` bumped to match after
  an in-place upgrade; the delete-and-relaunch validation of the catalog build
  is still unexercised — do it at a natural pause, not mid-run).
- Verified from the run-boundary snapshot: schema v6; 15,412 succeeded /
  2,582 failed_terminal (evidence-classed write-offs) / 351 retryable
  (112 SensitiveLoginGated cookie-parked) / ~2.96M pending; ingest ledger
  active (141 files); **metadata capture at 100% coverage on post-upgrade
  successes** (5,177/5,177 envelopes + 80 failure-path captures), all 5,257
  envelopes parse clean under `load-metadata --dry-run`.
- **The rc1 incident** (workspace built the May-vintage `v0.2.0-rc1` pin) is
  fully remediated in code/docs (ADR-0043, runbook, PR #23 hardening) but left
  one data debt: **10,235 succeeded videos with no metadata envelope** (the
  rc1-era cohort — transcripts complete and verified; only
  title/description/engagement missing).
- Snapshot relay: hop 1 (VM → interim volume) fires at each `process` run
  boundary; hop 2 (`push-to-yoda.sh`, operator-driven) → yoda → local. Verify
  `schema_version = 6` on any snapshot before analyzing it (stale-relay lesson).

## Session goals, in priority order

### 1. `backfill-metadata` subcommand → release v0.3.1

Recover the 10,235-video metadata gap without re-transcription. Design agreed
in principle with the operator (2026-07-29):

- Select `succeeded` videos with no `video_metadata_raw` row (keyset-paginated,
  Epic 4c loader precedent); for each, run a metadata-only yt-dlp invocation —
  `--skip-download --no-simulate --print METADATA_PRINT_TEMPLATE`, no media, no
  GPU — through the existing `process::run` machinery (bounded stdout, same
  64 KB cap and envelope builder as `acquire`); upsert via the existing
  `Store::upsert_metadata_raw`; `load-metadata` then fills the columns.
- Best-effort per video (a dead/blocked video logs + counts and moves on —
  expect some of the cohort to have died since fetch); stats per ADR-0007;
  `--limit` for smoke runs; `--dry-run` prints the cohort size.
- Never touches video status/lifecycle (metadata-only by construction — the
  Epic 4c invariant extends here).
- ~10K lightweight requests ≈ 2–4 h on the VM, runnable alongside `process`.
- **Release per ADR-0043**: merge → bump `Cargo.toml` version to 0.3.1 in the
  tag commit (see the production-run FOLLOWUPS entry — `-V` must finally mean
  something) → `git tag -a v0.3.1` → push tag → bump catalog
  `pipeline_git_ref` → in-place upgrade on the VM (build + cp + `-h` check).
- **Candidate rider** (small, operator-facing, from Epic 5's list): the
  `global = true` fix for the six GlobalArgs flags rejected after subcommands
  (SRC-bake + T11 entry) — one line per flag + tests; it removes a real
  daily operator paper-cut. Include unless review pushes back on scope.

### 2. Capacity estimate — now empirical, not projected

The production-run FOLLOWUPS entry predates live data. The next run-boundary
snapshot carries what the estimate needs: per-instance throughput
(`video_events` timestamps across a multi-hour window), the live failure mix
at sustained load, and boot-disk headroom. Deliverable: projected completion
date for the remaining ~2.96M at measured throughput, with/without an analysis
window — **the PI's window decision (corpus spans 2025-08→2026-07) is the
single biggest lever and is still open**. Present her the choices with the
measured numbers.

### 3. Cookie-efficacy run (deferred since Epic 4a)

112 rows are cookie-parked (`SensitiveLoginGated`) and 239 legacy placeholders
remain in the retry pool. With a `~/tiktok-cookies.txt` on the VM,
`process --cookies-file …` attempts them under ADR-0035 (cookies only on
gated retries). Small, operator-driven, unblocks a known cohort.

### 4. Standing operational cadence

- `load-metadata` at any convenient boundary (replayable; casual timing).
- Snapshot verification battery per boundary (the session-1 checklist:
  schema, counts, envelope coverage ratio, `--dry-run` parse audit).
- `video_metadata_raw` prune/VACUUM decision **after** the first full batch's
  `load-metadata` (FOLLOWUPS entry; decide with real numbers).

## Deliberately NOT folded into this session

- **Epic 5 hygiene sweep** (bin/lib module tree, `state/mod.rs` bundle,
  sync-IO sweep, ingest-ledger hardening bundle, test-debt bundles) — stays
  routed to Epic 5. Mid-campaign code churn without operational payoff is
  risk without reward; the one exception is the `global = true` rider above.
- **Plan C items** (short-link resolution etc.) — unchanged.
- **Deployment repo** (`d3i-infra/researchcloud-ddp-transcribe`): two notes to
  hand its owner, not to do here — (a) hop-1 rsync should
  `--exclude='*.tmp-*'` (v0.3.0's unique tmp names aren't hidden files, so
  they can ride into shard tars; harmless but untidy); (b) the catalog item's
  delete-and-relaunch path is unvalidated against v0.3.0+.

## Open operator/PI decisions (blockers for their respective goals)

1. PI: analysis window (goal 2's lever; also gates `recompute-window`).
2. Operator: cookies file availability for goal 3.
3. Operator: go/no-go on the `global = true` rider in v0.3.1.
