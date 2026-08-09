# SRC VM operations — ddp-transcribe on the A10 workspace

**Status:** canonical as of 2026-07-07; every path and procedure below was verified
live against the running workspace that day. Supersedes the untracked
`docs/local/SRC-RUNBOOK.md` / `SRC-BAKE-*` set and `docs/local/src-bootstrap.sh`,
which describe a storage-hop git topology that does not exist on the VM — treat
those as historical. Epic 4a amended the *operate* section (triage retired,
retry moved in-pipeline, classification became an operator TOML); Epic 4c added
the post-run `load-metadata` step and the v4 → v5 migration; the topology is
stable. **2026-07-29:** the update procedure changed from pull-main to
tag-and-relaunch (ADR-0043), after an incident where the workspace's pinned
`pipeline_git_ref` had drifted four epics stale — see below. **2026-07-30
(v0.3.2, campaign-safety slice):** in-run checkpointing (`--checkpoint-cmd` /
`--checkpoint-every`), an actually-dry `ingest --dry-run`, an age-guarded
startup tmp sweep, and `swept_stale` forensic events — all covered below.
**2026-07-30 (v0.4.0, Plan B close-out — tag not yet cut):** the
`requeue-failures` operator override, per-attempt `.work/` fetch directories
with an age-gated startup sweep, and the ADR-0013 backend assertion actually
firing on CUDA builds. Anything marked "v0.4.0" below is not on the workspace
until that tag is cut and `pipeline_git_ref` is bumped.

**Version state:** the workspace still runs **v0.3.0**. v0.3.1 was tagged but
never deployed, so the next upgrade jumps **0.3.0 → 0.3.2** in one step and
both tags' release notes apply; `-V` must print `0.3.2` afterwards. Anything
below marked "as of v0.3.1" therefore arrives with that same upgrade.

## Topology (what actually exists)

- **Source of truth:** GitHub `daniellemccool/ddp-transcribe`. The VM's working
  checkout at `~/src/ddp-transcribe` tracks it directly over HTTPS. There is **no
  git mirror on the storage volume** (the runbook-era storage-hop clone was never
  materialized).
- **Hot path (boot disk, POSIX-fsync-honest, per ADR 0032):**
  - `~/ddp-state/state.sqlite` — THE state DB. Beware look-alikes: `~/state.sqlite.old`
    and export copies exist; always pass `--state-db` explicitly.
  - `~/ddp-work/models/` — whisper models (production: `ggml-large-v3-turbo-q5_0.bin`).
  - `~/ddp-work/transcripts/` — sharded transcript output (write-once artifacts).
- **Storage volume (`~/data/transcription-pipeline-storage`, NFS-like, per ADR 0032):**
  `inbox/` (DDP JSONs in), `transcripts/` (durable sink), `archive/`,
  `state-snapshot.sqlite` (consistent `.backup` snapshot). Not safe for SQLite WAL
  or rename-durability — never point the hot path here.
- **Volume sync:** `~/sync-to-storage.sh` (rsync artifacts + `sqlite3 .backup`
  snapshot; flock-serialized; safe to run by hand) at batch boundaries;
  `~/restore-from-storage.sh` re-seeds a rebuilt workspace.
- **Toolchain:** rustup per-user in `~/.cargo` (PATH line is in `~/.bashrc` since
  2026-07-07 — it was missing for the VM's whole prior life; builds only ever
  worked in the install-day session). Do NOT `apt install cargo`.
- **Script status:** the generated `~/run-pipeline-gpu{0,1}.sh` and the sync/restore
  pair are **non-normative conveniences / SRC integration glue** (ADR-0032 comment,
  2026-07-07). The operator interface is the binary itself.

## Update procedure — tag-and-relaunch (production, per ADR-0043)

The SRC catalog item provisions the campaign machine by `git checkout
<pipeline_git_ref>`, where the ref is a pinned annotated release tag
(currently the `v0.x` series) — it never tracks `main`. A rebuild mid-run
(crash, restore-from-storage, workspace relaunch) must reproduce
byte-equivalent behavior, which only a fixed tag guarantees. Shipping code
to a campaign machine is therefore a **promotion**, done from the
workstation/dev repo, not a `git pull` on the VM:

1. Merge the change to `main` on `daniellemccool/ddp-transcribe`.
2. Cut an annotated release tag with release notes:
   `git tag -a vX.Y.Z -m "<release notes>"`.
3. Push the tag: `git push origin vX.Y.Z`.
4. Bump the SRC catalog item's `pipeline_git_ref` to `vX.Y.Z`.
5. Delete and relaunch the workspace so it provisions fresh at the new tag.

Every epic close-out checklist must ask "does this need a release tag?" —
2026-07-29 incident: the campaign workstation faithfully rebuilt
`pipeline_git_ref = v0.2.0-rc1` (pre-Epic-3 code) while `main` was four
epics ahead, because no tag had been cut past `v0.2.0-rc1` since. The
provisioning was not broken; the promotion habit was.

Verify after every relaunch:

```bash
ddp-transcribe -V && ddp-transcribe -h | head -12   # subcommand list matches expectations
```

After the pending 0.3.0 → 0.3.2 upgrade, `-V` must print **0.3.2**, `-h` must
list `backfill-metadata`, and `process -h` must show `--checkpoint-cmd`.

### Dev / emergency escape hatch (diverges from the pinned tag — NOT the production procedure)

For local iteration or an emergency hotfix on a live workspace only, the
three-step manual path below rebuilds in place. This intentionally leaves the
VM's binary **off** the pinned `pipeline_git_ref` — the workspace will not
reproduce this state on a rebuild, and the divergence must be resolved by
cutting a real tag and bumping `pipeline_git_ref` (steps above) at the first
opportunity, not left standing:

```bash
git -C ~/src/ddp-transcribe checkout main && git -C ~/src/ddp-transcribe pull
cd ~/src/ddp-transcribe && cargo build --release --features cuda   # ~2 min warm
sudo cp ~/src/ddp-transcribe/target/release/ddp-transcribe /usr/local/bin/ddp-transcribe
```

Step 3 is load-bearing: `/usr/local/bin/ddp-transcribe` is what PATH (and any
script) resolves — a skipped `cp` means old code runs while you believe you
deployed. Verify after this path too:

```bash
ddp-transcribe -V && ddp-transcribe -h | head -12   # subcommand list matches expectations
```

After deploying an ingest-production-hardening (v6-schema) binary, migrate
the state DB before running anything else against it:

```bash
ddp-transcribe --state-db ~/ddp-state/state.sqlite migrate   # -> v6, idempotent
```

The ladder is sequential, so one `migrate` call takes a v3 DB all the way to
v6. v3 → v4 adds `watch_history.watched_at_raw` (the timezone-verdict hedge,
ADR-0039); v4 → v5 adds the `video_metadata_raw` table and eight nullable
metadata columns on `videos` (ADR-0042); v5 → v6 adds the `ingested_files`
ledger, created deliberately empty (the migration cannot know which files
produced a pre-v6 DB's rows). `migrate` is a no-op if the DB is already at
v6. The binary refuses to open an un-migrated DB for any other subcommand —
`Store::open` hard-fails with a typed `SchemaVersionMismatch` error naming
the expected/found versions and instructing `migrate` (ADR-0022).

Because the ledger migrates in empty, the first `ingest` run after migrating
pays one full walk (stat + read + parse every file, same cost as pre-ledger
behavior) to populate it; every later run skips files whose `(name, size,
mtime)` still matches the ledger before reading them.

## Operating (current, Epic 4c)

The operator sequence for a batch is **`ingest` → `process` → `load-metadata` →
`status`**. Canonical invocation is the bare binary with explicit paths
(CWD-independent). **Every global flag (`--state-db`, `--transcripts`,
`--inbox`, `--whisper-model`, `--classification`, …) is accepted on either side
of the subcommand as of v0.3.1** (all of them are clap `global = true`;
inherited unchanged by v0.3.2, which is the tag that actually delivers it to
this workspace); the examples here keep them before the subcommand, and on a
pre-v0.3.1 binary — which is what the VM runs today — that placement is the
only one that parses.

```bash
CUDA_VISIBLE_DEVICES=0 ddp-transcribe \
    --state-db ~/ddp-state/state.sqlite \
    --transcripts ~/ddp-work/transcripts \
    --whisper-model ~/ddp-work/models/ggml-large-v3-turbo-q5_0.bin \
    [--classification ~/ddp-classification.toml] \
    process [--max-videos N] [--cookies-file ~/tiktok-cookies.txt] [--retries N] \
            [--checkpoint-cmd ~/sync-to-storage.sh --checkpoint-every 15m]
```

- Second GPU instance: same command with `CUDA_VISIBLE_DEVICES=1`. Concurrent
  claiming against one state DB is designed-for. Both instances run the
  start-of-batch sweep; the mutator predicates make this safe, but the
  second instance's sweep census will report the first's wins as
  `kept_capped` (with warns) — expected, not a bug.
- **Restarting one instance while its sibling runs is safe as of v0.3.2.**
  `process` startup sweeps stale `.tmp-{pid}-{seq}` files under the
  transcripts root, and that sweep now only collects tmps **older than the
  stale-claim threshold** (`--stale-claim-threshold`, default 30 min) — far
  older than any in-flight write. Before v0.3.2 the sweep matched on the name
  alone, so a restart could unlink the live sibling's in-flight tmp, fail its
  `rename`, and abort *that whole batch run*. A tmp whose mtime cannot be read
  is skipped and warned, never deleted.
- Sweep-recovered rows are explainable as of v0.3.2: every row the stale-claim
  sweep flips `in_progress → pending` writes a `swept_stale` event carrying the
  stale claim's `was_claimed_by` / `claimed_at` / `threshold_secs`, and each
  instance now reports its real hostname in `worker_id` instead of the literal
  `host`. If the `pending` count rises mid-campaign, check the snapshot for
  matching `swept_stale` events before suspecting anything else (see the
  two-writer entry in `docs/followups/production-run.md`).
- `--retries` (default 1) is the automatic in-batch retry budget per video:
  it caps **lifetime** attempts at `retries + 1` (compared against the row's
  `attempt_count`, which is bumped at claim time). Retries drain at the end of
  the queue, behind fresh work.
- `--classification <file>` overrides the compiled-in evidence-derived policy
  with an operator TOML (validated at startup — a malformed table hard-fails
  before the model loads). Omit it to use the compiled default.
- Startup must show the ADR-0013 banner (`whisper_backend_init_gpu: using CUDA0
  backend`); its absence means CPU fallback — abort and investigate.
- `process` exit code 3 = zero videos claimed (queue drained) — not an error.
- After a `process` batch: run `~/sync-to-storage.sh` (do NOT run it while an
  export/transfer is reading the volume's transcript tree). For uncapped
  campaign runs, prefer the in-run checkpoint hook below — a batch that runs
  for hours would otherwise leave the volume (and the Yoda-pushed resume
  snapshot) stale until it exits.
- Long runs belong in `tmux`.
- The pilot's parked `failed_retryable` rows are adjudicated automatically by
  the start-of-batch sweep on the first 4a run; expect the census to report
  ~3,915 swept_terminal + ~2,871 requeued + 301 parked_for_cookies (no cookies)
  on that first run.

### In-run checkpointing (v0.3.2) — replaces the manual checkpoint ritual

The batch-end auto-sync (hop 1) only fires when a `process` invocation *exits*,
so an uncapped campaign run leaves the storage volume and the resume snapshot
stale for as long as it runs. As of v0.3.2 `process` can do it itself:

```bash
… process --checkpoint-cmd ~/sync-to-storage.sh --checkpoint-every 15m
```

- **This is the default for uncapped campaign runs.** It supersedes the manual
  "Campaign checkpoint ritual" in the researchcloud repo's `yoda-operations.md`
  — that doc needs a pointer note saying so; hand it to the deploy-repo owner
  (do not edit that repo from here).
- **Overlap with a manual `~/sync-to-storage.sh` is safe**: the script
  `flock`-serializes itself, so a hand-run sync and a checkpoint firing simply
  queue behind one another.
- **The first firing is one full interval after start** (sleep-then-run, never
  at t=0), and the interval doubles as the hook's **timeout** — a hook that
  cannot finish within its own interval reports itself as failed rather than
  stacking overlapping copies.
- **A failed or timed-out hook never stops the batch.** It warns and increments
  `checkpoints_failed`; `checkpoints_run` counts the clean firings. Both land in
  the end-of-run "process complete" summary and the batch census, and the
  configured `checkpoint_cmd` / `checkpoint_every_secs` are recorded in
  `batch_runs.params_json`, so a past run's checkpoint behavior is
  reconstructible from the state DB alone.
- **Run completion can wait on an in-flight hook**, bounded by one interval
  (the timeout): if the last video finishes while a sync is mid-copy, the run
  exits after that sync completes rather than killing it. At `15m` that is a
  worst-case 15-minute tail on exit — expected, not a hang.
- `--checkpoint-every` alone is rejected by clap (`requires =
  checkpoint_cmd`); its default is `15m` when `--checkpoint-cmd` is given.

**Mid-run rsync reality — exit 24 (observed live 2026-07-29).** A manual sync
run while `process` was working printed:

```
file has vanished: ".../.work/ytdlp-<id>/<id>.wav"
```

Those warnings are **expected and benign** — `.work/` holds per-fetch
transients that yt-dlp and the pipeline delete as they go, so rsync races them
by design. As of **v0.4.0** (the Epic 5b close-out; tag not yet cut at the time
of writing — this behavior ships with that tag, not with v0.3.2), each fetch
attempt gets its own directory
(`.work/ytdlp-<id>.<pid>-<seq>/`) and the pipeline removes the **whole
directory** at the end of that attempt — after the DB commit on success, or on
a decode/transcribe failure — so mid-sync `file has vanished` (and
`directory has vanished`) warnings for `.work/` remain expected, and the
example path above now carries the `.<pid>-<seq>` suffix. What is left behind
is only crash/`kill`/cancellation residue, and `process` sweeps that at
startup: any `.work/ytdlp-*` directory older than `--stale-claim-threshold` is
removed (`cleaned up leftover fetch work dirs` in the startup log). The age gate
is what makes a two-instance deployment safe — a fresh directory may belong to
the other instance's in-flight fetch — so a dir left by a crash **one minute**
before a restart survives that restart and goes on the next one.

But rsync **exits 24** ("some files vanished before they could be
transferred"), which has two operational consequences:

1. Manually, exit 24 breaks any `&&` chain after the sync — the sync itself
   succeeded for everything that still existed.
2. Once the checkpoint hook drives that script, **every** checkpoint cycle
   during an active run counts as `checkpoints_failed`, because the hook keys
   on the exit code.

Fixes to hand the deploy-repo owner (`sync-to-storage.sh` lives there, not
here): add `--exclude='.work/'` to the hop-1 rsync, keep the already-queued
`--exclude='*.tmp-*'`, and treat **rsync exit 24 as success** inside the
script. Until that lands, a non-zero `checkpoints_failed` composed entirely of
exit-24 runs is **noise, not signal** — check the warn lines' `exit_code`
before treating it as a sync outage.

### `ingest --dry-run` (v0.3.2) — actually dry

Before v0.3.2, `ingest --dry-run` logged "not yet implemented" and then ran a
**real** ingest. As of v0.3.2 it is genuinely dry: the full pass runs — every
file read, parsed and upserted — inside a single transaction spanning the whole
inbox scan, which is then rolled back. Nothing persists, the `ingested_files`
ledger included, and because each file sees the earlier files' uncommitted
rows, the reported counts are exactly what a real run would report (duplicates
and cross-file raw-date backfills included).

- **Lock honesty:** a dry-run holds **one** write transaction (`BEGIN
  IMMEDIATE` … rollback) for the entire inbox scan — file reads and JSON
  parsing included — where a real ingest takes only brief per-file write locks.
  A full-inbox dry-run beside a running `process` can hold that lock past
  `busy_timeout` (5s); `process`'s next claim then gets `SQLITE_BUSY` and its
  batch aborts. Run a full-inbox dry-run only at a pause — no `process`
  running — never alongside one.
- **It is not a no-op on a fresh path:** pointing `--state-db` at a
  non-existent file still creates an empty `state.sqlite` (plus WAL/SHM files)
  with the schema applied — pre-existing `Store::open` behavior, unrelated to
  the dry-run change. No donor data is written.

### Post-run metadata load (Epic 4c)

Every `process` run stores each video's raw yt-dlp metadata envelope in
`video_metadata_raw` — for failed fetches too (tool failures, that is —
timeouts and spawn failures lose the captured output; the retry self-heals),
at zero extra network cost. Nothing parses it during the run. After a batch:

```bash
ddp-transcribe --state-db ~/ddp-state/state.sqlite load-metadata --dry-run
ddp-transcribe --state-db ~/ddp-state/state.sqlite load-metadata
```

- Output is one `examined / loaded / skipped-unparseable / without-video` line.
  `--dry-run` does the full examine-and-parse pass and reports real counts
  without writing (its `without-video` is always 0 — it never touches `videos`).
- **Replayable by design:** the loader only reads stored blobs, so it is safe to
  re-run any time and a parse bug is fixed by re-running a newer binary — never
  by re-fetching the corpus (ADR-0042). Nothing is lost by running it early and
  again later.
- It writes eight columns on `videos`: `video_description`, `uploader`,
  `uploader_id`, `video_created_at`, `view_count`, `like_count`,
  `comment_count`, `metadata_fetched_at`.
- **For the PI:** `video_description` is the creator's **caption text** — what
  the Research API calls `video_description` — and it is captured for every
  video yt-dlp can reach, including ones whose download failed. Engagement
  counts are point-in-time snapshots as of `metadata_fetched_at`, not current
  values. **Subtitle/caption *tracks* are not collected** (operator descope,
  2026-07-28: track downloads are fatal-on-failure in the pinned yt-dlp and even
  listing them spends the fetch's timeout budget); neither are comments
  (Research API only). Spoken content reaches the study through whisper
  transcription, not through platform-served caption tracks.

### Metadata backfill for the pre-capture cohort (v0.3.1)

Fetch-time metadata capture landed in v0.3.0, so every video transcribed by the
rc1-era binary succeeded without an envelope: ~**10,235** succeeded videos had no
`video_metadata_raw` row at the 2026-07-29 snapshot. `backfill-metadata` walks
exactly that cohort (status `succeeded`, no envelope) and runs **one
metadata-only yt-dlp invocation per video** to recover it.

```bash
ddp-transcribe --state-db ~/ddp-state/state.sqlite backfill-metadata --dry-run
ddp-transcribe --state-db ~/ddp-state/state.sqlite load-metadata --dry-run   # note rows_skipped_unparseable before the smoke run
ddp-transcribe --state-db ~/ddp-state/state.sqlite backfill-metadata --limit 5
ddp-transcribe --state-db ~/ddp-state/state.sqlite load-metadata --dry-run   # rows_examined +5, rows_skipped_unparseable unchanged
ddp-transcribe --state-db ~/ddp-state/state.sqlite backfill-metadata
ddp-transcribe --state-db ~/ddp-state/state.sqlite load-metadata
```

- **Zero-code verification for the `--limit 5` smoke run:** the `load-metadata
  --dry-run` before and after it is a free check that the 5 backfilled
  envelopes actually parse, with no code changes needed. Note
  `rows_skipped_unparseable` from the first `--dry-run` (run before the smoke
  backfill); after the smoke run's `load-metadata --dry-run`,
  `rows_examined` should read exactly +5 with `rows_skipped_unparseable`
  unchanged — that combination proves the 5 backfilled envelopes are
  schema-identical and parse cleanly, without writing anything or reading a
  single line of code.

- **What it never does:** no media download, no GPU, no whisper model load, no
  writes to video status or lifecycle columns, and **no cookies** (the argv is a
  URL plus print flags — cookies stay scoped to login-gated fetch retries per
  ADR-0035). Its only write is an insert-if-missing into `video_metadata_raw`, so
  a fetch-path envelope is never overwritten.
- **Safe to run alongside a live `process` run.** The DB is WAL with a
  `busy_timeout`, and the loop is serial — one invocation at a time — so it adds
  a single-threaded trickle of yt-dlp requests, not a second fetch fleet. That
  serial shape *is* the rate limiter; do not parallelize it. Budget ~**2–4 h**
  for the full ~10k cohort.
- **The printed cohort count is an advisory snapshot**, taken once before the
  pass starts. If a `process` run is working concurrently it can drift from the
  run's own `examined` count — that divergence is expected, not a miscount.
- **`--dry-run` reports the FULL cohort and rejects `--limit`**: it prints the
  cohort size and exits without invoking yt-dlp at all, so a cap on attempts has
  nothing to cap. Since v0.4.0 clap rejects the pair as a usage error (exit 2)
  rather than silently ignoring `--limit`. Use `--limit N` (without `--dry-run`)
  for the smoke run.
- **The stats line** is `examined / captured / capture-failed / already-filled /
  insert-failed`. Every examined video increments exactly one of the four
  outcomes. `capture-failed` is *expected*, not an error signal: the cohort is
  months old and dead, deleted, or region/login-blocked videos can no longer be
  extracted. `already-filled` means the fetch path landed an envelope
  concurrently (theirs wins). `insert-failed` should be ~0; a non-trivial count
  means look at the DB, not the network.
- **Best-effort and re-runnable.** Rows leave the cohort as envelopes land, so a
  re-run only retries what is still missing and converges on the permanently
  unreachable residue. Timeouts and spawn failures lose the captured stdout —
  same behavior as the fetch path — and simply count as `capture-failed`; a
  re-run retries them.
- **Finish with `load-metadata`** at a convenient boundary: backfilled envelopes
  are schema-identical to fetch-time ones, so the loader fills the same typed
  columns with no special handling. Running it early and again later costs
  nothing (it is replayable by design).

### Requeueing failures after an external change (v0.4.0)

`batch::run_sweep` already re-adjudicates *parked* retryables through the
classification table at the start of every run, so the automatic machinery
handles the ordinary case. What it cannot reach is the **cap-exhausted** tail
and rows already written off as `failed_terminal`. When the *external* condition
that failed them changes — fresh cookies for the login-gated cohort, a region
unblocked, a yt-dlp bump — `requeue-failures` is the sanctioned way to give
those rows another claim.

The live case is the cap-exhausted cookie-gated cohort. Preview first — the
command is default-deny, but `--dry-run` is what tells you the size of what you
are about to re-drive:

```bash
ddp-transcribe --state-db ~/ddp-state/state.sqlite \
    requeue-failures --error-kind SensitiveLoginGated --dry-run
ddp-transcribe --state-db ~/ddp-state/state.sqlite \
    requeue-failures --error-kind SensitiveLoginGated --max 500
CUDA_VISIBLE_DEVICES=0 ddp-transcribe --state-db ~/ddp-state/state.sqlite \
    … process --cookies-file ~/tiktok-cookies.txt --retries 4
```

- **A bare `requeue-failures` is a usage error** (exit 2). You must pass a
  *qualifying* selector — `--error-kind <K>` (repeatable, exact byte match, no
  case folding, no comma splitting), `--max-attempts <N>`, `--older-than <DUR>`
  — or an explicit `--all`. `--max <N>` and `--dry-run` are **modifiers**: they
  never grant eligibility on their own. `--all` means every `failed_retryable`
  row and *conflicts* with the qualifying selectors, so `--all --older-than 30d`
  is a parse error, never a silent intersection.
- **Terminal rows are opt-in twice over.** `--include-terminal` requires a
  qualifying selector alongside it, so `--include-terminal --all` is rejected.
  On terminal rows `--error-kind` matches `terminal_reason`, never a retained
  retryable kind.
- **`--retries` must be strictly greater than the row's `attempt_count`, or you
  get exactly one attempt.** Requeueing does not reset `attempt_count` (by
  design — the cap stays auditable). The next claim bumps `A` to `A + 1`, and
  in-pipeline retry only fires while `attempt_count < retries + 1`, so an
  *automatic* retry needs `--retries > A`. The 301 cookie-parked pilot rows
  exhausted at `A = 3` under `--retries 2` get one forced attempt each unless
  the following `process` runs `--retries 4` or higher. Check the cohort's
  `attempt_count` with `status --retryable --json` before picking the number.
- **The `--older-than` clock is failure events only** — the newest of
  `failed_retryable`, `failed_terminal`, `retry_requeued`, `cookie_parked`.
  `swept_terminal` and the other administrative events do **not** reset it, and
  a row with no qualifying event never matches. `videos.updated_at` is not
  touched.
- **It is auditable by construction.** Every moved row gets one
  `operator_requeued` event carrying its prior status, prior kind/reason and
  attempt count, attributed as `worker_id = operator:<hostname>-<pid>` —
  distinct from the sweep's literal `worker_id = 'sweep'`. A later
  "why is this row pending again?" is answerable from the state DB alone.
- **Hand-written SQL against `videos` is unsupported emergency repair.** It
  mutates status without leaving the `video_events` row the state machine and
  every audit depend on. This subcommand exists precisely so that reaching for
  `sqlite3` is never the only option (ADR-0046, ADR-0036).
- Safe to run beside a live `process`: it takes one short `BEGIN IMMEDIATE`
  transaction and claims nothing. Rows it moves are picked up by ordinary
  claiming, behind fresh work (`claim_next` orders `attempt_count ASC`).

### Status quickstart (Epic 4b)

```bash
ddp-transcribe --state-db ~/ddp-state/state.sqlite status                 # counts + batch-run history
ddp-transcribe --state-db ~/ddp-state/state.sqlite status --retryable     # failed_retryable, by kind
ddp-transcribe --state-db ~/ddp-state/state.sqlite --transcripts ~/ddp-work/transcripts \
    status --verify                                                     # done-contract, before any pause
```

- `status` (no flags) is DB-only and read-only: counts by status, claim ages
  for in-progress rows, and the full `batch_runs` history — an interrupted
  run's open row renders honestly (`finished_at` NULL, no census) rather than
  being skipped.
- `status --retryable` lists the `failed_retryable` pool by kind. The
  301 cookie-parked rows from the pilot batch carry the legacy placeholder
  kind `Fetch` (pre-Epic-3 rows never re-classified) — annotated
  `(legacy placeholder kind)` in the human-readable render; `--json` carries
  the raw stored value.
- **Run `--transcripts ~/ddp-work/transcripts status --verify` before pausing
  the workspace** (ADR-0011 spin-down practice): it checks per-shard artifact
  existence and `raw_signals.schema_version`, and exits 0 only when
  `pending == 0 ∧ in_progress == 0` and every artifact/schema check passed —
  exit 1 means it is not safe to pause yet.

## Known VM facts (hard-won; do not re-derive)

- **`IpBlockedMessage` means the video was removed.** The yt-dlp stderr text is a
  misfire; the IP is fine (ADR-0033 comment, 2026-07-07). A *real* block looks
  entirely different — zero successes cliff-onset, uniform retryable `HttpError`
  on the first hop, `no metadata envelope captured` on every fetch, terminal
  writes stopped — and never says "IP" at all: 60 h / 1.8M real rejections
  produced that text zero times (signature table in
  `incident-2026-08-06-tiktok-tls-403.md`).
- The census persists in the state DB's `batch_runs` table with the active
  policy TOML — attrition documentation survives tmux.
- `/etc/rsc/cron_webdav.sh` and `cron_user.sh` curl processes are SURF platform
  tooling (WebDAV mount health, SRAM user sync) — benign, not ours.
- `~/ddp-transcribe-infra` is a disposable non-git snapshot of
  `d3i-infra/researchcloud-ddp-transcribe`; never edit it in place — changes are
  invisible to review. The repo lives on the workstation.
- Terminal garbling over SSH: `export TERM=xterm-256color`.
- The config-echo line is scoped per subcommand since Epic 4b — `init`/`ingest`/
  `migrate`/`status`/`recompute-window`/`load-metadata` no longer log
  `whisper_model_path` (resolved; was a cosmetic false-alarm risk in Epic 3/4a
  logs).
- Global flags became true clap globals in v0.3.1 (and stay so in v0.3.2), so
  `ddp-transcribe process --state-db …` now parses. On any binary older than
  that (rc1, v0.3.0 — including the workspace's current binary) only the
  before-the-subcommand placement every example here uses works —
  `process --state-db …` fails with
  `error: unexpected argument '--state-db' found`, which is a version signal,
  not a typo. Likewise `--checkpoint-cmd` is a v0.3.2 flag: an
  `unexpected argument` error for it means the upgrade did not land.
- Bulk file transfer off the volume: iRODS/Yoda per-file overhead dominates below
  ~1 MB/file; parallelize disjoint shard ranges or use a bundle transfer. 120k
  files single-stream ≈ 24 h (measured 2026-07-07).
- **`ddp-transcribe --version` reporting `0.1.0` is the v0.3.0 signature, not an
  anomaly.** The Cargo.toml bump-in-tag-commit discipline started at v0.3.1;
  rc1 and v0.3.0 both compiled with the manifest still at 0.1.0. On the pinned
  campaign binary, `0.1.0` is *confirmation* of v0.3.0. (v0.3.1+ report their
  real versions.)
- **The VM carries `~/.config/yt-dlp/config` → `--impersonate chrome` since
  2026-08-09** — the mitigation for the 2026-08-06 TLS-fingerprint 403 wave
  (`incident-2026-08-06-tiktok-tls-403.md`). Every yt-dlp invocation reads it
  (the pipeline never passes `--ignore-config`). Deleting or losing this file
  reverts the VM to 100% fetch failure with `HttpError` retryable parks —
  and it is **hand-applied only**: a re-provision loses it until the deploy
  repo's `ytdlp` role installs it (handoff section in the incident doc).
- **`www.tiktokv.com` (the DDP share-redirect host) 403s non-browser TLS
  fingerprints from SURF egresses** (both workspace IPs, verified 2026-08-09;
  residential passes unimpersonated). An `HttpError` wave with
  `no metadata envelope captured` on every fetch = the first hop is being
  rejected again — probe with and without `--impersonate chrome` before
  anything else, and do not let the run keep burning attempts.
