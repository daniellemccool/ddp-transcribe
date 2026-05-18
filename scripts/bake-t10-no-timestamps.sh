#!/usr/bin/env bash
# T10 bake harness — set_no_timestamps quality + perf check.
#
# Runs the pipeline against the news_orgs fixture twice per side
# (pre-T9 baseline at PRE_REF, post-T9 at POST_REF) so intra-side
# determinism under `Greedy { best_of: 1 }` can be confirmed before
# cross-side comparison. Artifacts land under $BAKE_ROOT/{pre,post}/run{1,2}/.
#
# Defaults are tuned for the SRC A10 workspace; override via env:
#
#   MODEL=./models/ggml-tiny.en.bin BAKE_ROOT=/tmp/bake-t10-tiny ./scripts/bake-t10-no-timestamps.sh
#
# Companion: scripts/bake-t10-compare.sh runs the 4 signal-equality
# checks on the artifacts this script produces.

set -euo pipefail

REPO="$(git rev-parse --show-toplevel)"
FIXTURE="$REPO/tests/fixtures/ddp/news_orgs/participant=newsorg-fixture_source=tiktok.json"
MODEL="${MODEL:-$REPO/models/ggml-large-v3-turbo-q5_0.bin}"
BAKE_ROOT="${BAKE_ROOT:-/tmp/bake-t10}"
PRE_REF="${PRE_REF:-2f3167c}"      # commit before T9 (T8 bake-notes)
POST_REF="${POST_REF:-feat/perf-tweaks}"
MAX_VIDEOS="${MAX_VIDEOS:-25}"

[[ -f "$FIXTURE" ]] || { echo "Missing fixture: $FIXTURE" >&2; exit 1; }
[[ -f "$MODEL"   ]] || { echo "Missing model:   $MODEL"   >&2; exit 1; }

if [[ -n "$(git status --porcelain)" ]]; then
    echo "Working tree not clean; commit or stash first." >&2
    git status --short >&2
    exit 1
fi

run_side() {
    local label="$1"     # "pre" or "post"
    local ref="$2"       # SHA or branch
    local run_num="$3"   # 1 or 2

    local dir="$BAKE_ROOT/$label/run$run_num"
    rm -rf "$dir"
    mkdir -p "$dir/inbox" "$dir/transcripts"
    cp "$FIXTURE" "$dir/inbox/"

    echo
    echo "===== $label run$run_num @ $ref ====="
    git checkout --quiet "$ref"
    cargo build --release --features cuda

    local bin="$REPO/target/release/uu-tiktok"
    local -a global=(
        --state-db    "$dir/state.sqlite"
        --inbox       "$dir/inbox"
        --transcripts "$dir/transcripts"
        --whisper-model "$MODEL"
    )

    "$bin" "${global[@]}" init
    "$bin" "${global[@]}" ingest
    time "$bin" "${global[@]}" process --max-videos "$MAX_VIDEOS" \
        2>&1 | tee "$dir/process.log"
}

run_side pre  "$PRE_REF"  1
run_side pre  "$PRE_REF"  2
run_side post "$POST_REF" 1
run_side post "$POST_REF" 2

git checkout --quiet "$POST_REF"

echo
echo "Done. Artifacts under: $BAKE_ROOT/{pre,post}/run{1,2}/transcripts/"
echo "Next: bash scripts/bake-t10-compare.sh"
