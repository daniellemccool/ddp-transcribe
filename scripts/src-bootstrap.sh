#!/usr/bin/env bash
# scripts/src-bootstrap.sh — bring up the A10 native workspace from the
# Research Drive mount.
#
# Layout (per docs/SRC-RUNBOOK.md):
#   /data/transcription-pipeline-storage   ← persistent (Research Drive, NFS-like)
#   /home/dmccool/src                      ← ephemeral, fast, POSIX-fsync-honest
#
# Run after `srun --resume` (or whatever resumes the workspace from pause)
# at the start of each session.
#
# Usage:
#   bash scripts/src-bootstrap.sh             # bootstrap branch from storage's HEAD
#   bash scripts/src-bootstrap.sh feat/foo    # bootstrap a specific branch
set -euo pipefail

STORAGE=/data/transcription-pipeline-storage
NATIVE=/home/dmccool/src
REPO_STORAGE="$STORAGE/uu-tiktok"
REPO_NATIVE="$NATIVE/uu-tiktok"
BRANCH="${1:-$(git -C "$REPO_STORAGE" symbolic-ref --short HEAD)}"

if [[ ! -d "$REPO_STORAGE/.git" ]]; then
  echo "ERROR: $REPO_STORAGE is not a git repository. Aborting." >&2
  exit 1
fi

mkdir -p "$NATIVE"

if [[ ! -d "$REPO_NATIVE/.git" ]]; then
  echo "[bootstrap] cloning $REPO_STORAGE → $REPO_NATIVE (first run)"
  git clone "$REPO_STORAGE" "$REPO_NATIVE"
else
  echo "[bootstrap] $REPO_NATIVE exists; fetching latest from $REPO_STORAGE"
  git -C "$REPO_NATIVE" fetch origin
fi

git -C "$REPO_NATIVE" checkout "$BRANCH"
git -C "$REPO_NATIVE" reset --hard "origin/$BRANCH" 2>/dev/null || \
  git -C "$REPO_NATIVE" reset --hard "$BRANCH"

# Symlink models from storage (read-once, large; no fsync hazard for read-only).
if [[ -d "$STORAGE/models" ]]; then
  ln -sfn "$STORAGE/models" "$REPO_NATIVE/models"
  echo "[bootstrap] models symlinked: $REPO_NATIVE/models → $STORAGE/models"
else
  echo "[bootstrap] WARN: $STORAGE/models not present; download models before baking"
fi

# Build on native. cargo target/ lives on $NATIVE for fast incremental builds.
echo "[bootstrap] building release binary (this may take a few minutes on first run)"
cd "$REPO_NATIVE"
cargo build --release --features cuda

echo ""
echo "[bootstrap] complete. Native workspace ready at: $REPO_NATIVE"
echo ""
echo "Bake invocation (copy-paste from here; do NOT run from $REPO_STORAGE):"
echo ""
cat <<EOF
cd $REPO_NATIVE
mkdir -p $NATIVE/work/inbox $NATIVE/work/transcripts
cp tests/fixtures/ddp/news_orgs/*.json $NATIVE/work/inbox/   # if not already populated

./target/release/uu-tiktok \\
  --state-db $NATIVE/work/state.sqlite \\
  --inbox $NATIVE/work/inbox \\
  --transcripts $NATIVE/work/transcripts \\
  --whisper-model $REPO_NATIVE/models/ggml-large-v3-turbo-q5_0.bin \\
  --compute-lang-probs \\
  init

./target/release/uu-tiktok \\
  --state-db $NATIVE/work/state.sqlite \\
  --inbox $NATIVE/work/inbox \\
  --transcripts $NATIVE/work/transcripts \\
  ingest

time ./target/release/uu-tiktok \\
  --state-db $NATIVE/work/state.sqlite \\
  --inbox $NATIVE/work/inbox \\
  --transcripts $NATIVE/work/transcripts \\
  --whisper-model $REPO_NATIVE/models/ggml-large-v3-turbo-q5_0.bin \\
  --compute-lang-probs \\
  process
EOF
