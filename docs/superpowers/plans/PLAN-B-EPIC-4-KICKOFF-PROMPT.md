# Plan B Epic 4 kickoff prompt — paste into a fresh Claude Code session when ready

> **Author note:** Written 2026-07-07 during Epic 3 close-out operations, while the
> first (and only) production `triage --dry-run` was in flight on the SRC A10
> workspace. That session produced binding operator rulings that **reshape Epic 4's
> scope**: the operator-driven triage mechanism retires days after shipping, retry
> becomes pipeline behavior, and classification policy becomes operator-editable
> configuration. Where this document contradicts `EPIC-4-SKETCH.md` (2026-05-12) or
> Epic 3's shipped docs (`EPIC-3-CLOSE.md` runbook, ADR 0034's triage flow), this
> document is newer and operator-ruled. Epic 3's components (classifiers, three-arm
> dispatch, mutators, cookie gate) are all reused; only the *operator interface*
> around them changes.

---

## Prompt to paste

I want to begin planning **Plan B Epic 4** for the ddp-transcribe pipeline
(`/home/dmm/src/uu-tiktok`). Epics 1–3 are complete on `main` (PR #15). Epic 4's
scope is set by operator rulings from the 2026-07-07 close-out session (below),
reconciled with the older `EPIC-4-SKETCH.md` scope.

### Step 1: Orient yourself before discussing scope

Read in order:

1. `docs/superpowers/plans/2026-05-12-plan-b/EPIC-4-SKETCH.md` — the pre-Epic-3
   sketch (time-window filter, `status` subcommand, DDP timezone). Still live scope,
   but no longer the opening move.
2. `docs/superpowers/plans/2026-07-07-plan-b-epic-3/EPIC-3-CLOSE.md` — what Epic 3
   shipped. Its operator runbook (probe-triage flow) is superseded by the rulings
   below; the component inventory is accurate.
3. ADRs 0033/0034/0035 **including their 2026-07-07 comments** (`adg view --id NNNN
   --model docs/decisions`): the 0033 recurring-misreading guard (IpBlockedMessage
   = video removed, NOT an IP issue), and the 0032 operator-interface ruling
   (wrapper scripts are non-normative conveniences).
4. `docs/followups/epic-4.md` — routed entries; reconciliation notes below.
5. Current `src/` on `main`: `src/failure.rs` (taxonomy + classifiers),
   `src/triage.rs` + `src/probe.rs` (retiring), `src/pipeline/{mod,pipelined,serial}.rs`
   (three-arm dispatch), `src/state/mod.rs` (mutators incl. `requeue_retryable`).
6. `ddp-run-export.sqlite` (repo root, untracked) — complete pilot-run DB:
   7,087 `failed_retryable` rows, all `attempt_count = 1`.

### Step 2: Operator rulings (2026-07-07 — binding, not proposals)

1. **Retry is pipeline behavior, not an operator ceremony.** During a batch, a
   retryable fetch failure records the attempt and returns the row to the **end of
   the pending queue**, to be retried within the same batch. New parameter
   `--retries N`, **default 1** (so at most two fetch attempts per video by
   default). Rows that exhaust retries remain `failed_retryable`, which becomes the
   "exhausted, needs adjudication" pool rather than a staging area.
2. **Write-off classes go terminal inline and are never requeued.** Already live
   via Epic 3 Task 07's dispatch; unchanged.
3. **Classification policy is operator-editable configuration.** An ordered table
   of `{pattern (exact substring), label, disposition}` with disposition ∈
   `retryable | terminal | requires-cookie`; first-match-wins (order is load-
   bearing); explicit fallback disposition for unmatched messages (default-cautious
   `retryable`, today's `YtDlpOther`). The evidence-derived default table compiles
   into the binary (tool stays self-contained; no file required); a config file
   overrides it; config errors **hard-fail at startup** (same philosophy as the
   0022 schema gate). Boundary: config governs interpretation of *tool output*
   (yt-dlp stderr); structural errors (`ToolNotFound`, timeout, decode) stay in code.
4. **`requires-cookie` disposition:** rows are requeued only when a cookies-file is
   configured for the batch (cookies then ride automatically via the existing
   kind-gate); with no cookies configured they are NOT requeued — a cookie-less
   retry is a guaranteed refail that burns the attempt. The parked set
   (`failed_retryable` + kind) IS the "different queue"; no new state needed. Note:
   live cookie efficacy is unproven (Epic 3 Task 08 integration-tested against
   fakes only) — treat the first real cookie batch as the experiment it is.
5. **The 7,087 pilot rows wait for this mechanism** (operator chose Option B over a
   final manual triage run). All have `attempt_count = 1`, so under the default cap
   they get exactly one automatic retry when swept in.
6. **Triage retires.** The `triage` subcommand, `src/probe.rs`, `--rate`, and the
   `curl` runtime dependency are removed. The oEmbed probe was a *validation
   instrument* — its n=36 evidence session plus the one full dry-run census
   calibrated the write-off classes — and its service is complete. Operator verdict
   on the shipped flow: "nothing in triage has seemed useful."
7. **The census becomes durable batch output** (not tmux scrollback), and the run
   record must capture **which classification table was active** (policy
   provenance). With policy operator-editable, attrition documentation without the
   generating policy is not reproducible — treat provenance as non-negotiable.
8. **Binding premise** (ADR-0032 comment, 2026-07-07): the operator interface is
   the tool itself. No design may assume wrapper scripts; shell scripts exist only
   as SRC-specific integration glue (data movement, provisioning).

### Step 3: Design proposals awaiting confirmation at kickoff

- **End-of-queue mechanism:** claim ordering `ORDER BY attempt_count ASC,
  first_seen_at ASC, video_id ASC` — fresh work drains first, retries sort behind
  it; no schema change. (Amends the claim-ordering contract; ADR it.)
- **Start-of-batch sweep:** `process` opens by sweeping `failed_retryable` rows
  through the classification table — write-off/terminal dispositions terminalize,
  eligible retryables requeue under the cap, `requires-cookie` gated per ruling 4.
  One mechanism serves both the historical pool (ruling 5) and rows that fail at
  the tail of one batch and are picked up by the next.
- **`--max-videos` accounting:** lean toward retries consuming work-budget slots
  (honest accounting); confirm.
- **Config format:** TOML (small new dependency, kind to hands) vs JSON (serde_json
  already present, zero new deps). Undecided; planner proposes.
- **Sketch reconciliation:** `EPIC-4-SKETCH.md`'s `status` subcommand naturally
  absorbs the census/attrition reporting and the AD0017 done-contract; the
  time-window filter + DDP timezone resolution are orthogonal — planner proposes
  either co-scoping or an explicit 4a (retry/config/status) / 4b (window/timezone)
  split.

### Step 4: Census calibration data (dry-run of 2026-07-07)

Reproducible baseline from `ddp-run-export.sqlite`: 7,087 `failed_retryable`;
message-class write-offs **3,309** (the plan's ~3,915 estimate included the
"do not have permission to view this post" class, ~600 rows, which shipped as
probed-not-written-off); probed remainder 3,778.

**FILL IN from the completed dry-run census before kickoff:** ProbeDead count
(how many probes re-confirmed already-settled classes — the no-permission rows
are the expected bulk, and a high count justifies moving that pattern to
`terminal` in the default table), requeue-eligible count, `kept_unreachable`
(network-posture check), `SensitiveLoginGated` count (sizes the cookie pool).

### Step 5: ADR work (via `adg` per repo governance — never hand-edit)

- **Supersede 0034** (operator triage + oEmbed oracle) → in-pipeline capped retry;
  the fetcher is the liveness oracle; probe retired with honors.
- **Amend 0033**: the write-off *patterns* move from hardcoded global constraint to
  the config table's compiled defaults; the misfire semantics and evidence
  citations stand (see the 0033 comment of 2026-07-07 before touching anything —
  IpBlockedMessage means VIDEO REMOVED).
- **New ADRs:** in-batch retry semantics + claim-ordering amendment;
  classification-config schema + policy provenance. Plus the sketch's AD0027–0029
  if co-scoped.

### Step 6: FOLLOWUPS reconciliation (`docs/followups/epic-4.md`)

- Moot-by-redesign once this lands (resolve + archive per 0020 at close): triage
  progress-output gap, census tag-annotation item (census is being redesigned),
  config-echo papercut (fold into the retry/config work's logging touch).
- Stand as scoped: `parse_watched_at` UTC assumption (sketch scope), `requeued`
  event detail_json attempt-counts (natural fit with `status`), architecture-doc
  naming sweep (bundle with this epic's doc rewrite — which must also update
  `EPIC-3-CLOSE.md`-adjacent operational docs and `docs/operations/src-vm.md`).

### What Epic 4 deliberately does NOT reopen

- ADR 0008 artifact ordering, 0024 stale sweep, 0027 orchestrator topology, 0032
  storage topology.
- No backoff/jitter sophistication — retries are end-of-queue, once by default.
- Cookie *acquisition* operations (getting a valid cookies file is an operator
  task outside the tool).
- `run_serial` retirement (still Epic 5, per 0002 note).
