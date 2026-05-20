# SRC A10 Operational Runbook

Procedures for operating the uu-tiktok pipeline on the SURF Research Cloud
A10 dev workspace. Pairs with:

- `docs/SRC-BAKE-NOTES.md` — bake findings (what worked, throughput numbers)
- `docs/SRC-BAKE-CHECKLIST.md` — first-time-on-a-workspace pre-flight
- ADR 0011 — workspace lifecycle (pause-not-delete on grant wallets)
- `docs/superpowers/plans/2026-05-13-plan-b-epic-2/20-bake-orchestrator.md` — Epic 2 bake plan

## Layout: storage vs native

The A10 workspace has two storage surfaces. They are NOT interchangeable.

| Path | Persistence | I/O profile | Use for |
|---|---|---|---|
| `/data/transcription-pipeline-storage` (Research Drive mount) | Survives workspace pause/resume; survives workspace delete | NFS-like; **weak fsync semantics** | Canonical git repo, whisper models, post-bake archives, anything that must survive workspace lifecycle |
| `/home/dmccool/src` (A10 native disk) | Ephemeral (may not survive workspace restart); fast NVMe | POSIX fsync; safe for SQLite WAL | Working git checkout, `target/`, `state.sqlite`, transcripts during a bake, work/scratch |

**Why the split matters:** `SRC-BAKE-CHECKLIST.md` Phase 3 documents that SQLite
WAL "does not play nicely with NFS-style mounts" — behaviors range from
"works" through `database is locked` under contention through silent
corruption. Plan B Epic 2's pipelined orchestrator (N=3 fetch workers
contending on `claim_next` via `BEGIN IMMEDIATE`) makes the WAL hazard
materially worse than Plan A's single-claimer path. Running the orchestrator
against a state DB on the mount is unsafe regardless of how the bake numbers
look — flaky races and silent corruption become reachable.

Phase 2 throughput claims (ADR 0027's ~3.5× target) are grounded in Epic 1
bake math on native I/O. Running the bake from the mount measures mount I/O,
not the design — write phase becomes the bottleneck, compressing the N=1
vs N=3 delta artificially.

## Getting updates onto the storage drive

The bootstrap script reads from the storage drive only — it never talks to
GitHub directly. Updates flow GitHub → storage drive → A10 native:

```bash
# Run from a workspace session before bootstrapping the A10 native checkout.
# Two steps: pull latest, then check out the branch you want to bake.
git -C /data/transcription-pipeline-storage/uu-tiktok fetch
git -C /data/transcription-pipeline-storage/uu-tiktok checkout <branch>   # e.g. feat/plan-b-epic-2
git -C /data/transcription-pipeline-storage/uu-tiktok pull
```

**The `git checkout` step is required if storage is currently on a
different branch (e.g., main).** `git pull` only updates the currently-
checked-out branch; without first switching, you'll bake against the wrong
commits. Alternatively, pass the branch explicitly to the bootstrap
script (which switches it on native), but storage stays where it was —
that's fine as long as native is what runs the bake.

The repo is public and SRC's network policy permits outbound HTTPS to
GitHub, so the `git fetch` / `git pull` work without credentials. Pre-
condition: the storage drive's repo has `origin` set to the GitHub URL
(verify with `git -C /data/transcription-pipeline-storage/uu-tiktok remote -v`;
set with `git remote set-url origin https://github.com/<user>/uu-tiktok.git`
if needed).

The A10 itself does not need GitHub credentials — it pulls from the
storage drive's local working tree, not from GitHub.

**Cannot push from SRC:** there's no secure way to authenticate as the
operator from the shared workspace. Commits and pushes happen on the dev
workstation; updates flow to the storage drive via the `git pull` above.

## Models on the storage drive

The whisper model files live on the storage drive at
`/data/transcription-pipeline-storage/models/`. The bootstrap script
symlinks them to `$REPO_NATIVE/models/`. Models are read-once at startup
so the symlink is safe (no fsync hazard).

If `bootstrap.sh` prints `WARN: /data/transcription-pipeline-storage/models
not present`, the models directory is empty or missing. Populate it once:

```bash
# Production model (Plan B): ~573 MB, multilingual, GPU-friendly.
bash /data/transcription-pipeline-storage/uu-tiktok/scripts/fetch-large-v3-turbo-model.sh

# Optional: tiny.en for dev profile.
MODEL_DIR=/data/transcription-pipeline-storage/models \
  bash /data/transcription-pipeline-storage/uu-tiktok/scripts/fetch-tiny-model.sh
```

Models persist on the storage drive across workspace lifecycle — download
once per storage drive, reuse forever.

## Bootstrap: at session start (after `srun --resume`)

```bash
# From wherever; the script handles paths.
bash /data/transcription-pipeline-storage/uu-tiktok/scripts/src-bootstrap.sh

# Or specify a branch explicitly (e.g., for a feature bake):
bash /data/transcription-pipeline-storage/uu-tiktok/scripts/src-bootstrap.sh feat/plan-b-epic-2
```

The script:

1. Clones `/data/transcription-pipeline-storage/uu-tiktok` → `/home/dmccool/src/uu-tiktok` (first run) or `git fetch` + **destructive reset** (subsequent runs).
2. Checks out the requested branch (defaults to storage's current HEAD branch).
3. Symlinks `/data/transcription-pipeline-storage/models` → `/home/dmccool/src/uu-tiktok/models`. Models are read-once; the symlink is fine — no fsync hazard.
4. `cargo build --release --features cuda` on native. `target/` lives on the A10's SSD for fast incremental builds.
5. Prints the bake command with explicit paths to copy-paste.

**On the destructive reset (`git reset --hard`):** the A10 is treated as
fully ephemeral. No persistent changes are ever made on it — no commits,
no work-in-progress edits to source. The reset is safe because there's
never anything to lose. If you ever DO need to capture something
generated on the A10 (e.g., a `cargo expand` output, a debug log), copy
it to the storage drive's `bake-results/` before re-running bootstrap.

## Running a bake

Use the explicit-path invocation the bootstrap script printed. The shape is:

```bash
cd /home/dmccool/src/uu-tiktok
mkdir -p /home/dmccool/src/work/inbox /home/dmccool/src/work/transcripts
cp tests/fixtures/ddp/news_orgs/*.json /home/dmccool/src/work/inbox/

./target/release/uu-tiktok \
  --state-db /home/dmccool/src/work/state.sqlite \
  --inbox /home/dmccool/src/work/inbox \
  --transcripts /home/dmccool/src/work/transcripts \
  --whisper-model /home/dmccool/src/uu-tiktok/models/ggml-large-v3-turbo-q5_0.bin \
  --compute-lang-probs \
  init

# ... ingest, then process. See 20-bake-orchestrator.md for full sequence + topology comparison runs.
```

Per ADR 0027, default topology is `--download-workers 3 --channel-capacity 2`. Override via flags or `UU_TIKTOK_DOWNLOAD_WORKERS` / `UU_TIKTOK_CHANNEL_CAPACITY`.

## After the bake: archive

```bash
bash /home/dmccool/src/uu-tiktok/scripts/src-archive-results.sh epic-2-n3
```

The script:

1. Creates `/data/transcription-pipeline-storage/bake-results/epic-2-n3-<timestamp>/`.
2. rsyncs `/home/dmccool/src/work/transcripts/` → archive dir.
3. Copies `state.sqlite` + WAL/shm sidecars → archive dir.
4. Records the bake's commit SHA + working-tree status → archive dir.

Bake **findings** (wallclock numbers, environment surprises, performance
observations) travel back to your dev workstation **by manual transcription**.
No commits or pushes happen from the A10 — the workspace can't hold state
reliably, and there's no secure way to authenticate to GitHub from SRC. The
data flow is:

1. **During the bake:** note wallclock numbers + observations.
2. **After the bake:** run the archive script (above) so the *artifacts*
   (transcripts, state.sqlite, commit SHA) persist on the storage drive.
3. **Back on dev workstation:** edit `docs/SRC-BAKE-NOTES.md` with the
   findings you captured. Commit and push from there.

The storage drive's `bake-results/` directory is the durable record of what
was produced; `SRC-BAKE-NOTES.md` (on the dev workstation, then committed
to git) is the durable record of what was observed.

## Spin-down (per ADR 0011)

After the bake archive lands and findings are noted (to be transcribed on
the dev workstation):

1. Confirm no active batches running: `pgrep -af uu-tiktok` should be empty.
2. Confirm no in-progress rows in the bake state DB (only matters if you're going to resume with that DB; the archive copy is safe regardless).
3. `cargo clean` is optional — `target/` is on ephemeral disk anyway; saves ~5 GB if you want a quick `du -sh /home/dmccool` before pause.
4. Pause the workspace via the SRC portal. Grant-wallet billing goes to zero per 0011.

The next session, run `src-bootstrap.sh` again. The Research Drive contents
persist (including `bake-results/` from earlier sessions); native disk may
or may not, but the bootstrap re-creates it idempotently from scratch.

If you ever delete the workspace entirely and provision a new one, attach
the same Research Drive and run the bootstrap script. The workspace is
disposable; the Research Drive is the system.

## When this runbook is wrong

If the storage layout changes (e.g., A10 grows native persistent storage,
or storage mount path changes), update this file and the two scripts as a
single commit. Cite the change in `docs/SRC-BAKE-NOTES.md`'s next bake
section so future operators see the rationale.
