# SRC VM operations — ddp-transcribe on the A10 workspace

**Status:** canonical as of 2026-07-07; every path and procedure below was verified
live against the running workspace that day. Supersedes the untracked
`docs/local/SRC-RUNBOOK.md` / `SRC-BAKE-*` set and `docs/local/src-bootstrap.sh`,
which describe a storage-hop git topology that does not exist on the VM — treat
those as historical. Epic 4a amended the *operate* section (triage retired,
retry moved in-pipeline, classification became an operator TOML); the topology
and update procedure are stable.

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

## Update procedure (three steps, all required)

```bash
git -C ~/src/ddp-transcribe checkout main && git -C ~/src/ddp-transcribe pull
cd ~/src/ddp-transcribe && cargo build --release --features cuda   # ~2 min warm
sudo cp ~/src/ddp-transcribe/target/release/ddp-transcribe /usr/local/bin/ddp-transcribe
```

Step 3 is load-bearing: `/usr/local/bin/ddp-transcribe` is what PATH (and any
script) resolves — a skipped `cp` means old code runs while you believe you
deployed. Verify after every update:

```bash
ddp-transcribe -V && ddp-transcribe -h | head -12   # subcommand list matches expectations
```

## Operating (current, Epic 4a)

Canonical invocation is the bare binary with explicit paths (CWD-independent):

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
- The config log line prints `whisper_model_path` even for subcommands that never
  load a model — cosmetic (FOLLOWUPS, Epic 4b).
- Bulk file transfer off the volume: iRODS/Yoda per-file overhead dominates below
  ~1 MB/file; parallelize disjoint shard ranges or use a bundle transfer. 120k
  files single-stream ≈ 24 h (measured 2026-07-07).
