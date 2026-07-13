# Plan B Epic 4b kickoff prompt — paste into a fresh Claude Code session when ready

> **Author note:** Written 2026-07-13 during post-4a operations (first-batch
> census recovery + Yoda delivery experiments). Epic 4a closed 2026-07-08.
> The `EPIC-4-SKETCH.md` scope (2026-05-12) is still the core of 4b, but it
> predates Epics 3–4a: its ADR numbers (AD0027–0029) are stale (adg assigns
> the next lean numbers, currently 0039+), triage no longer exists, and
> `batch_runs` now exists and adds duties the sketch never anticipated.
> Where this document contradicts the sketch, this document is newer.

---

## Prompt to paste

I want to begin planning **Plan B Epic 4b** for the ddp-transcribe pipeline
(`/home/dmm/src/uu-tiktok`). Epics 1–4a are complete on `main`. 4b's scope is
the operator-facing `status` subcommand (the 0017 done-contract), the
time-window filter + DDP timezone resolution, and a small CLI-hardening pass.

### Step 1: Orient yourself before discussing scope

Read in order:

1. `docs/superpowers/plans/2026-05-12-plan-b/EPIC-4-SKETCH.md` — core scope
   (status / time-window / timezone) and the brainstorm notes at the bottom
   (batched stat calls; schema-version sampling deferral; `watched_at_raw`
   preservation; stale-claim visibility). Staleness warnings above apply.
2. `docs/superpowers/plans/2026-07-07-plan-b-epic-4a/EPIC-4A-CLOSE.md` — what
   4a shipped, its "Deferred to 4b" list, and the **"First production batch —
   actual results"** section (ground-truth numbers; see Step 4).
3. `docs/followups/epic-4.md` — all five routed entries (UTC assumption;
   open `batch_runs` row rendering; `--retries` i64 validation; config-echo
   scoping; the operator-interface premise — read that one BEFORE sketching
   any operator command).
4. `docs/madr-archive/0017-operational-done-contract-for-batch-validation.md`
   — the done-contract `status` implements (0017 was not lean-migrated; the
   archive record is authoritative). Also lean ADRs 0036 (retry semantics)
   and 0037 (classification provenance): `status` renders the world those
   two created.
5. `ddp-run-export.sqlite` (repo root, untracked) — post-4a production DB
   snapshot (schema v3; 56,620 videos; 2 `batch_runs` rows, one of them
   open/interrupted). This is the development fixture for `status`.
6. Original spec § CLI surface > `status`:
   `docs/superpowers/specs/2026-04-16-uu-tiktok-pipeline-design.md` (~line 367).

### Step 2: Scope (settled inputs, not proposals)

**`status` subcommand** (0017 done-contract + accumulated duties):

- Counts by status; `--video-id` event history; `--respondent-id` summary;
  `--errors` / `--retryable` failure lists; `--json` for tooling.
- Artifact-existence check for succeeded rows (batch per-shard `read_dir` +
  set lookup, NOT per-row stat — brainstorm note). `raw_signals.schema_version`
  check (full parse OK at Plan B scale; sampling is Plan C).
- **`batch_runs` history rendering**: census + policy provenance per run.
  MUST render open rows honestly (`finished_at IS NULL` → "INTERRUPTED",
  no census — never skip, never crash on NULLs). See the epic-4.md entry;
  the pilot DB's run 1 is the real test case.
- `in_progress` rows with `claimed_at` ages (is anything stuck / safe to
  pause? — cross-reference the 0024 stale-sweep semantics).
- **Per-event detail surface**: the archived Epic 3 followup "`requeued`
  event detail_json lacks attempt-count context" was resolved at the
  `batch_runs` layer, with the note that a richer per-event rendering
  "remains a 4b `status` concern." That thread lands here: `--video-id`
  should render event `detail_json` payloads (`{"kind","message"}` shapes,
  sweep `requeued` events, etc.) legibly, not as raw JSON blobs.
- Failed-retryable breakdown by `last_retryable_kind` (the operator query
  used repeatedly in production — see Step 4's table; note 301 cookie-parked
  rows still carry the legacy placeholder kind `"Fetch"`, a display wrinkle
  `status` must not misattribute).

**Time-window filter + timezone:**

- Resolve the `parse_watched_at` UTC assumption (full entry in epic-4.md;
  empirical path preferred: known-donor DDP comparison). Record as an ADR.
- `ingest --window-start/--window-end` → `in_window` flag on `watch_history`
  (schema v4 bump via the 0022 migrate ladder); `recompute-window`
  subcommand refusing bare invocation.
- Preserve the raw DDP `Date` string (`watched_at_raw` column) so
  reinterpretation never requires re-ingest (brainstorm safety note).

**CLI hardening (small, bundled):**

- `--retries` / sweep cap: ranged value parser (mirror the existing
  `RangedU64ValueParser` pattern in `src/cli.rs`); kills the negative-budget
  and `i64::MAX` overflow edges (epic-4.md entry).
- Config echo scoped to config the invoked subcommand actually consumes
  (epic-4.md entry).

**Standing constraints:** the operator interface is the tool itself (0032
comment — no wrapper-script assumptions); verification command per
CLAUDE.md (`--test-threads=1` mandatory); mutators per 0006/0023; stats
structs per 0007; new integration tests need `required-features =
["test-helpers"]` per 0005; architecture-doc drift check at planning time
(status touches the state-machine surface → revise the matching deepdive +
`index.md` §4, per the standing cross-epic followup).

### Step 3: What Epic 4b deliberately does NOT include

- **Cookie-efficacy run** — operational, not code; runs on the new
  catalog-item workspace when it exists (301 `SensitiveLoginGated` rows
  wait). `status` is the tool that will read its results.
- Epic 5 cleanup items (`run_serial` retirement, state/mod.rs hygiene
  bundle, sync-IO sweep) — routed, untouched.
- Deployment/delivery work (researchcloud-ddp-transcribe, ddp-inspector) —
  different repos, different sessions.
- Backoff/jitter, richer retry semantics — 0036 stands as shipped.

### Step 4: Ground truth for acceptance (2026-07-08 production batch)

`status` developed against `ddp-run-export.sqlite` must reproduce the known
reality recorded in EPIC-4A-CLOSE.md's results section, notably: 56,620
videos → 51,903 succeeded / 3,928 failed_terminal / 789 failed_retryable
(= 301 cookie-parked + 488 exhausted); per-kind failed_retryable breakdown
(418 NoPermission, 36 FfprobePostprocess, 32 NoVideoFormats, 1 HttpError,
1 NoDataBlocks, 301 legacy-`"Fetch"`); 2 batch_runs rows — run 1 open
(started 2026-07-08 11:41 UTC, no finish, no census), run 2 closed with
census. A `status` whose output disagrees with that table is wrong; treat
this as the epic's built-in integration fixture.

### Step 5: ADR work (via `adg` per repo governance — never hand-edit)

- New: DDP timestamp timezone treatment; window-flag semantics (computed at
  ingest, updated only via explicit `recompute-window`); `status` output
  schema / done-contract fulfillment (subsumes the sketch's AD0027–0029 —
  numbers assigned by `adg lean new`).
- Check at close whether 0017's archive record needs a lean successor or a
  comment recording fulfillment.

### Step 6: FOLLOWUPS lifecycle (per 0020)

At epic close, the five epic-4.md entries resolve to
`docs/archive/followups-resolved.md` with resolving SHAs; the scope-index
lines in `docs/FOLLOWUPS.md` are removed. The timezone entry resolves
whichever way the evidence lands (UTC-confirmed = doc comment; local-time =
schema/consumer work) — record the verdict, don't just close it.

### Step 7: Execution defaults

Plan per `superpowers:writing-plans` with per-task files (0001); execute via
`superpowers:subagent-driven-development`; three-tier review per 0018;
report caps per 0019. Sketch estimate: ~half a week, 4–5 tasks.
