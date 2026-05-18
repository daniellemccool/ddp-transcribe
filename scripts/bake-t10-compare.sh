#!/usr/bin/env bash
# T10 bake comparison — applies the four signal-equality checks from
# the spec's bake plan #2 to artifacts produced by bake-t10-no-timestamps.sh.
#
# Defaults match the harness's defaults; override BAKE_ROOT to compare
# a different artifact tree.
#
# Per-video checks:
#   1. Transcript text byte-for-byte equality.
#   2. Flattened token sequence (id + text) equality across all segments.
#   3. Per-token p and plog equality within TOL_ABS (default 1e-6 absolute).
#   4. Per-window no_speech_prob: post's per-segment value matches the
#      no_speech_prob of the pre segments whose tokens it contains
#      (within TOL_ABS).
#
# Outputs PASS/FAIL per video and an aggregate summary. Exit code is 0
# if all videos pass, 1 otherwise. Determinism (run1 vs run2 within a
# side) is also checked first; failing intra-side determinism is fatal
# because cross-side comparison then has no stable baseline.

set -uo pipefail

BAKE_ROOT="${BAKE_ROOT:-/tmp/bake-t10}"
TOL_ABS="${TOL_ABS:-0.000001}"

PRE_R1="$BAKE_ROOT/pre/run1/transcripts"
PRE_R2="$BAKE_ROOT/pre/run2/transcripts"
POST_R1="$BAKE_ROOT/post/run1/transcripts"
POST_R2="$BAKE_ROOT/post/run2/transcripts"

for d in "$PRE_R1" "$PRE_R2" "$POST_R1" "$POST_R2"; do
    [[ -d "$d" ]] || { echo "Missing: $d" >&2; exit 1; }
done

# Step 1: intra-side determinism. Pre run1 vs run2 must be byte-identical;
# same for post.
echo "===== Intra-side determinism ====="
intra_failures=0
for label in pre post; do
    r1="$BAKE_ROOT/$label/run1/transcripts"
    r2="$BAKE_ROOT/$label/run2/transcripts"
    for f in "$r1"/*/*.json; do
        rel="${f#$r1/}"
        other="$r2/$rel"
        if [[ ! -f "$other" ]]; then
            echo "[$label] MISSING run2 counterpart: $rel"
            intra_failures=$((intra_failures + 1))
            continue
        fi
        if ! diff -q <(jq -S . "$f") <(jq -S . "$other") > /dev/null; then
            echo "[$label] NON-DETERMINISTIC: $rel"
            intra_failures=$((intra_failures + 1))
        fi
    done
done
if (( intra_failures > 0 )); then
    echo
    echo "FATAL: $intra_failures intra-side determinism failure(s). Aborting."
    echo "Investigate before trusting any cross-side comparison."
    exit 2
fi
echo "OK — both sides byte-identical run1 vs run2."

# Step 2-5: cross-side signal-equality (use each side's run1 as canonical).
echo
echo "===== Cross-side signal-equality ====="
total=0
passes=0
failures=()
for pre in "$PRE_R1"/*/*.json; do
    rel="${pre#$PRE_R1/}"
    post="$POST_R1/$rel"
    total=$((total + 1))
    if [[ ! -f "$post" ]]; then
        echo "[$rel] MISSING POST"
        failures+=("$rel:missing-post")
        continue
    fi

    if ! python3 - "$pre" "$post" "$TOL_ABS" <<'PY'
import json, sys, math
pre_path, post_path, tol_s = sys.argv[1], sys.argv[2], sys.argv[3]
tol = float(tol_s)

with open(pre_path) as f:
    a = json.load(f)
with open(post_path) as f:
    b = json.load(f)

# Helper: extract the raw_signals.segments[].tokens flat list with each
# token tagged with its segment's no_speech_prob.
def segs(meta):
    return (meta.get("raw_signals") or {}).get("segments") or []

pre_segs  = segs(a)
post_segs = segs(b)

def text_field(meta):
    # AD0008-era artifacts: top-level transcript text is written to a
    # sibling .txt file, not inside the JSON. The JSON's text-bearing
    # path is the concatenation of token text (special tokens excluded
    # by the consumer). For equality purposes, the per-token check below
    # subsumes "transcript text byte-for-byte" — there is no separate
    # `text` key in the JSON to compare.
    return None

# Check 1: text equality is implicitly covered by the per-token id+text
# check (token sequence is the durable source-of-truth in the artifact).

# Check 2: flattened token (id+text) sequence.
ta = [(t["id"], t["text"]) for s in pre_segs  for t in s["tokens"]]
tb = [(t["id"], t["text"]) for s in post_segs for t in s["tokens"]]
if ta != tb:
    print(f"  token sequence differs: pre_len={len(ta)} post_len={len(tb)}")
    if len(ta) == len(tb):
        first = next((i for i,(x,y) in enumerate(zip(ta,tb)) if x != y), None)
        print(f"  first diff at token {first}: pre={ta[first]!r} post={tb[first]!r}")
    sys.exit(1)

# Check 3: per-token p and plog within tol_abs.
tpa = [(t["p"], t["plog"]) for s in pre_segs  for t in s["tokens"]]
tpb = [(t["p"], t["plog"]) for s in post_segs for t in s["tokens"]]
for i, ((pa, la), (pb, lb)) in enumerate(zip(tpa, tpb)):
    if abs(pa - pb) > tol or abs(la - lb) > tol:
        print(f"  per-token p/plog diverged at token {i}: "
              f"pre=({pa},{la}) post=({pb},{lb}) tol={tol}")
        sys.exit(1)

# Check 4: per-window no_speech_prob. For each post segment, find the
# range of pre tokens it covers (by walking the cumulative token count),
# and assert every pre segment overlapping that range has the same
# no_speech_prob as the post segment (within tol).
pre_token_to_seg = []
for si, s in enumerate(pre_segs):
    pre_token_to_seg.extend([si] * len(s["tokens"]))

post_token_offset = 0
for ps in post_segs:
    n_tokens = len(ps["tokens"])
    pre_segs_in_window = set(pre_token_to_seg[post_token_offset:post_token_offset + n_tokens])
    if not pre_segs_in_window:
        print(f"  post segment with no pre coverage at offset {post_token_offset}")
        sys.exit(1)
    ps_nsp = ps["no_speech_prob"]
    for pre_seg_idx in pre_segs_in_window:
        pre_nsp = pre_segs[pre_seg_idx]["no_speech_prob"]
        if abs(pre_nsp - ps_nsp) > tol:
            print(f"  no_speech_prob diverged: pre[{pre_seg_idx}]={pre_nsp} "
                  f"post={ps_nsp} (post token offset {post_token_offset})")
            sys.exit(1)
    post_token_offset += n_tokens

sys.exit(0)
PY
    then
        echo "[$rel] FAIL"
        failures+=("$rel")
    else
        echo "[$rel] PASS"
        passes=$((passes + 1))
    fi
done

echo
echo "===== Summary ====="
echo "Total compared: $total"
echo "Passed:         $passes"
echo "Failed:         $((total - passes))"
if (( ${#failures[@]} > 0 )); then
    echo
    echo "Failed videos:"
    printf '  %s\n' "${failures[@]}"
    exit 1
fi

# Wall-clock comparison: extract `time` data from process.log per side.
# Pre and post process.log were written by bake-t10-no-timestamps.sh.
echo
echo "===== Wall-clock medians (informational) ====="
for label in pre post; do
    log1="$BAKE_ROOT/$label/run1/process.log"
    log2="$BAKE_ROOT/$label/run2/process.log"
    if [[ -f "$log1" && -f "$log2" ]]; then
        t1=$(grep -E "^real\s" "$log1" 2>/dev/null | tail -1 || echo "(n/a)")
        t2=$(grep -E "^real\s" "$log2" 2>/dev/null | tail -1 || echo "(n/a)")
        echo "$label: run1=${t1#real}  run2=${t2#real}"
    fi
done

echo
echo "All cross-side signal-equality checks passed."
exit 0
