#!/usr/bin/env bash
# scripts/src-archive-results.sh — copy bake artifacts from A10 native disk
# back to the Research Drive mount for persistence across workspace pause/resume.
#
# Run after a bake completes, before pausing the workspace.
set -euo pipefail

STORAGE=/data/transcription-pipeline-storage
NATIVE=/home/dmccool/src
WORK="$NATIVE/work"
STAMP="$(date +%Y%m%d-%H%M%S)"
LABEL="${1:-bake}"   # optional label, e.g. "epic-2-n3"
DEST="$STORAGE/bake-results/${LABEL}-${STAMP}"

if [[ ! -d "$WORK" ]]; then
  echo "ERROR: $WORK does not exist. Did you run scripts/src-bootstrap.sh first?" >&2
  exit 1
fi

mkdir -p "$DEST"

# Transcripts: full directory tree.
if [[ -d "$WORK/transcripts" ]]; then
  rsync -a "$WORK/transcripts/" "$DEST/transcripts/"
  echo "[archive] transcripts → $DEST/transcripts/"
fi

# State DB: copy with the WAL/shm sidecars so the snapshot is consistent.
if [[ -f "$WORK/state.sqlite" ]]; then
  cp "$WORK/state.sqlite" "$DEST/state.sqlite"
  [[ -f "$WORK/state.sqlite-wal" ]] && cp "$WORK/state.sqlite-wal" "$DEST/state.sqlite-wal"
  [[ -f "$WORK/state.sqlite-shm" ]] && cp "$WORK/state.sqlite-shm" "$DEST/state.sqlite-shm"
  echo "[archive] state.sqlite (+ WAL/shm if present) → $DEST/"
fi

# Provenance: capture the exact commit SHA the bake ran against.
git -C "$NATIVE/ddp-transcribe" rev-parse HEAD > "$DEST/COMMIT_SHA"
git -C "$NATIVE/ddp-transcribe" status --short > "$DEST/COMMIT_STATUS" 2>&1 || true
echo "[archive] commit SHA + working-tree status → $DEST/"

echo ""
echo "[archive] complete. Results at: $DEST"
echo "[archive] next: edit docs/SRC-BAKE-NOTES.md (in $NATIVE/ddp-transcribe), commit, push."
