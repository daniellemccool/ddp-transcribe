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
`pipeline_git_ref` had drifted four epics stale — see below.

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
deployed.

After deploying an Epic 4c (v5-schema) binary, migrate the state DB before
running anything else against it:

```bash
ddp-transcribe --state-db ~/ddp-state/state.sqlite migrate   # -> v5, idempotent
```

The ladder is sequential, so one `migrate` call takes a v3 DB all the way to
v5. v3 → v4 adds `watch_history.watched_at_raw` (the timezone-verdict hedge,
ADR-0039); v4 → v5 adds the `video_metadata_raw` table and eight nullable
metadata columns on `videos` (ADR-0042). `migrate` is a no-op if the DB is
already at v5. The binary refuses to
open an un-migrated DB for any other subcommand — `Store::open` hard-fails
with a typed `SchemaVersionMismatch` error naming the expected/found
versions and instructing `migrate` (ADR-0022).

## Operating (current, Epic 4c)

The operator sequence for a batch is **`ingest` → `process` → `load-metadata` →
`status`**. Canonical invocation is the bare binary with explicit paths
(CWD-independent). **Every global flag (`--state-db`, `--transcripts`,
`--inbox`, `--whisper-model`, `--classification`, …) goes BEFORE the
subcommand** — the parser rejects them after it.

```bash
CUDA_VISIBLE_DEVICES=0 ddp-transcribe \
    --state-db ~/ddp-state/state.sqlite \
    --transcripts ~/ddp-work/transcripts \
    --whisper-model ~/ddp-work/models/ggml-large-v3-turbo-q5_0.bin \
    [--classification ~/ddp-classification.toml] \
    process [--max-videos N] [--cookies-file ~/tiktok-cookies.txt] [--retries N]
```

- Second GPU instance: same command with `CUDA_VISIBLE_DEVICES=1`. Concurrent
  claiming against one state DB is designed-for. Both instances run the
  start-of-batch sweep; the mutator predicates make this safe, but the
  second instance's sweep census will report the first's wins as
  `kept_capped` (with warns) — expected, not a bug.
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
  export/transfer is reading the volume's transcript tree).
- Long runs belong in `tmux`.
- The pilot's parked `failed_retryable` rows are adjudicated automatically by
  the start-of-batch sweep on the first 4a run; expect the census to report
  ~3,915 swept_terminal + ~2,871 requeued + 301 parked_for_cookies (no cookies)
  on that first run.

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
  misfire; the IP is fine (ADR-0033 comment, 2026-07-07).
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
- Global flags are **not** `global = true` in clap (except `--compute-lang-probs`),
  so `ddp-transcribe process --state-db …` is a parse error. Always put
  `--state-db` / `--transcripts` / `--whisper-model` / `--inbox` /
  `--classification` before the subcommand, as every example here does. (Filed
  for Epic 5: make them true globals.)
- Bulk file transfer off the volume: iRODS/Yoda per-file overhead dominates below
  ~1 MB/file; parallelize disjoint shard ranges or use a bundle transfer. 120k
  files single-stream ≈ 24 h (measured 2026-07-07).
