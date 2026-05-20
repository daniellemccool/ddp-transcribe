#!/usr/bin/env bash
# Download the whisper.cpp large-v3-turbo-q5_0 model — Plan B's production
# model per Epic 1's bake findings (best transcripts + best wallclock at
# one-third the size of medium.en; multilingual).
#
# Default target is the Research Drive mount so the model persists across
# A10 workspace lifecycle. Override MODEL_DIR if you want it elsewhere.
set -euo pipefail

MODEL_DIR="${MODEL_DIR:-/data/transcription-pipeline-storage/models}"
MODEL_NAME="ggml-large-v3-turbo-q5_0.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_NAME}"

mkdir -p "$MODEL_DIR"
if [ -f "$MODEL_DIR/$MODEL_NAME" ]; then
    echo "$MODEL_NAME already present at $MODEL_DIR — skipping"
    exit 0
fi

echo "Downloading $MODEL_NAME (~573 MB) to $MODEL_DIR ..."
curl -L --fail -o "$MODEL_DIR/$MODEL_NAME" "$URL"
echo "Done. Use via --whisper-model $MODEL_DIR/$MODEL_NAME (or via the symlinked path $REPO_NATIVE/models/$MODEL_NAME after running scripts/src-bootstrap.sh)."
