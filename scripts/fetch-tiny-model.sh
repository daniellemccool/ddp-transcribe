#!/usr/bin/env bash
# Download the whisper.cpp tiny.en model used for dev-profile transcription.
set -euo pipefail

MODEL_DIR="${MODEL_DIR:-./models}"
MODEL_NAME="ggml-tiny.en.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${MODEL_NAME}"

mkdir -p "$MODEL_DIR"
if [ -f "$MODEL_DIR/$MODEL_NAME" ]; then
    echo "$MODEL_NAME already present at $MODEL_DIR — skipping"
    exit 0
fi

echo "Downloading $MODEL_NAME (~75MB) to $MODEL_DIR..."
curl -L -o "$MODEL_DIR/$MODEL_NAME" "$URL"
echo "Done. Path to use in DDP_TRANSCRIBE_WHISPER_MODEL or default config: $MODEL_DIR/$MODEL_NAME"
