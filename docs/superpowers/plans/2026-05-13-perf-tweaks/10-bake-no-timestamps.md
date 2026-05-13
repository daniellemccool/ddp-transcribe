# Task 10 — Bake T#2: `set_no_timestamps(true)` against `news_orgs` fixture

**Goal:** Empirically confirm T9's hypothesis (per-token p/plog/text unchanged; per-window no_speech_prob unchanged; only segment-grouping shape changes; small wall-clock win). Revert T9 if ANY of the four signal-equality checks fails, per the spec's tight gate. Honest reporting if wall-clock shows no improvement (signals identical but no perf win → still revert, since the rationale evaporates).

**Spec commit:** 9 — `bake(transcribe): T#2 set_no_timestamps quality + perf notes`.

**ADRs directly relevant:**
- **AD0010** — raw_signals schema v1 pass-through; the bake's per-token equality check is what enforces "we didn't break the schema's contract by accident."
- **AD0017** — `news_orgs` 20-video fixture is the ratified bake set.

**Files:**
- Append: `docs/SRC-BAKE-NOTES.md` (a new dated section)
- Possibly append: `docs/bake-findings.md` (if any signal diverges or wall-clock doesn't improve, document the finding)
- Possibly revert: T9 commit (commit 8 in the spec sequence) if the bake reveals divergence beyond tolerance

**Where this task runs:** the SRC A10 workspace. This bake requires real inference; CPU runs would take far too long to bake 80 transcriptions. The GPU verification gate (AD0013) applies — confirm CUDA backend is active before starting.

---

- [ ] **Step 1: Confirm the bake environment**

On the A10 workspace, from the worktree:

```bash
# Confirm GPU
ldd target/release/uu-tiktok 2>/dev/null | grep -E "cuda|cublas" | head -5
# Build release with CUDA if not already
cargo build --release --features cuda
# Confirm news_orgs fixture path (from T8)
echo $NEWS_ORGS_URL_LIST  # or: cat <fixture_path>
# Confirm models on disk; the bake should use the production model
# (large-v3-turbo-q5_0 per T13's "production model recommendation"):
ls models/ggml-large-v3-turbo-q5_0.bin
```

Expected: release binary linked against CUDA; fixture has 20 URLs; production model on disk. If using a different model (e.g., `tiny.en` for speed), record that choice in the bake notes.

- [ ] **Step 2: Capture pre-T9 baseline (2× per video)**

Check out the commit immediately BEFORE T9 (commit 7 in the spec sequence — the T8 bake-notes commit):
```bash
git log --oneline | head -10
git checkout <SHA_BEFORE_T9>
cargo build --release --features cuda
```

Set up a bake directory:
```bash
export BAKE_DIR=/tmp/bake-t10-pre
mkdir -p "$BAKE_DIR"/{run1,run2}
```

For each URL in `news_orgs`, run the binary twice (pre-T9 baseline). The `process` subcommand transcribes; each run writes a `<video_id>.json` artifact.

```bash
for run in run1 run2; do
  while IFS= read -r url; do
    video_id=$(basename "$url" | sed 's/[^a-zA-Z0-9]//g' | head -c 32)
    # The actual ingest+process flow per existing CLI conventions:
    # (adjust to whatever the operator's standard bake invocation is)
    ./target/release/uu-tiktok process --max-videos 1 --url "$url" \
      --model models/ggml-large-v3-turbo-q5_0.bin \
      --output-dir "$BAKE_DIR/$run"
  done < <(cat <FIXTURE_PATH>)
done
```

(If the CLI doesn't accept a single URL — Plan A/B uses ingest from a DDP JSON — the operator should adapt: ingest the fixture into a fresh state DB, then `process` to completion. Per-video JSON artifacts land in `output_dir/<shard>/<video_id>.json`.)

Expected: ~20 × 2 = 40 JSON artifacts under `/tmp/bake-t10-pre/run{1,2}/...`.

- [ ] **Step 3: Verify intra-side determinism (pre-T9)**

For each video, compare run1 vs run2 to confirm `Greedy { best_of: 1 }` is deterministic:

```bash
cd /tmp/bake-t10-pre
for json1 in run1/*/*.json; do
  json2="${json1/run1/run2}"
  if ! diff -q <(jq -S . "$json1") <(jq -S . "$json2") > /dev/null; then
    echo "INTRA-SIDE DIVERGENCE: $json1 vs $json2"
    diff <(jq -S . "$json1") <(jq -S . "$json2") | head -20
  fi
done
```

Expected: empty output (all pairs identical). If any pair diverges, **stop and resolve** — non-determinism breaks the bake's premise. Possible cause: floating-point reordering across runs (rare on a single A10 with the same model + same input); investigate before continuing.

If intra-side equality holds, use `run1/` as the baseline for cross-side comparison.

- [ ] **Step 4: Switch to T9 and capture post-T9 (2× per video)**

```bash
cd <worktree>
git checkout feat/perf-tweaks  # or whichever branch has T9 as HEAD
cargo build --release --features cuda
export BAKE_DIR=/tmp/bake-t10-post
mkdir -p "$BAKE_DIR"/{run1,run2}
# Repeat the loop from Step 2 with the new BAKE_DIR.
```

Verify intra-side determinism (same loop as Step 3) on `/tmp/bake-t10-post/`.

- [ ] **Step 5: Cross-side signal-equality checks**

Run the comparison script. Create `/tmp/bake-t10-check.sh`:

```bash
#!/usr/bin/env bash
# Per-video signal-equality check for T10 bake.
set -euo pipefail

P_PRE=/tmp/bake-t10-pre/run1
P_POST=/tmp/bake-t10-post/run1
TOLERANCE=0.000001  # 1e-6 absolute

failures=0
total=0
for pre in "$P_PRE"/*/*.json; do
  vid="$(basename "$pre" .json)"
  post="${pre/$P_PRE/$P_POST}"
  total=$((total + 1))
  if [[ ! -f "$post" ]]; then
    echo "MISSING POST: $vid"
    failures=$((failures + 1))
    continue
  fi

  # 1. Transcript text byte-for-byte equality.
  if ! diff -q <(jq -r '.text // ""' "$pre" 2>/dev/null || jq -r '.transcript // .raw_signals.text // ""' "$pre") \
                <(jq -r '.text // ""' "$post" 2>/dev/null || jq -r '.transcript // .raw_signals.text // ""' "$post") > /dev/null; then
    # (The exact .text path depends on the TranscriptMetadata schema; adjust the jq path to whatever the artifact format uses.)
    echo "$vid: TRANSCRIPT TEXT DIVERGENCE"
    failures=$((failures + 1))
    continue
  fi

  # 2. Flattened token sequence (id + text) equality.
  if ! diff -q <(jq -c '[.raw_signals.segments[].tokens[] | {id, text}]' "$pre") \
                <(jq -c '[.raw_signals.segments[].tokens[] | {id, text}]' "$post") > /dev/null; then
    echo "$vid: TOKEN SEQUENCE DIVERGENCE"
    failures=$((failures + 1))
    continue
  fi

  # 3. Per-token p / plog equality within 1e-6 absolute.
  # Use a small Python or jq+awk one-liner for the numeric comparison.
  if ! python3 - <<PY
import json, sys
a = json.load(open("$pre"))
b = json.load(open("$post"))
ta = [t for s in a["raw_signals"]["segments"] for t in s["tokens"]]
tb = [t for s in b["raw_signals"]["segments"] for t in s["tokens"]]
if len(ta) != len(tb):
    sys.exit(1)
for x, y in zip(ta, tb):
    if abs(x["p"] - y["p"]) > $TOLERANCE or abs(x["plog"] - y["plog"]) > $TOLERANCE:
        sys.exit(1)
PY
  then
    echo "$vid: PER-TOKEN p/plog DIVERGENCE BEYOND TOLERANCE"
    failures=$((failures + 1))
    continue
  fi

  # 4. Per-window no_speech_prob: walk the flattened token sequence pre and
  # post, find window boundaries (post's segments ARE the windows), assert
  # each window's no_speech_prob exists in the pre and matches.
  if ! python3 - <<PY
import json
a = json.load(open("$pre"))
b = json.load(open("$post"))
post_segs = b["raw_signals"]["segments"]
# For each post segment, find the pre segments whose tokens map to it
# (pre's segments may be finer-grained but share no_speech_prob within a window).
pre_token_index = 0
pre_segs = a["raw_signals"]["segments"]
for post_seg in post_segs:
    n_tokens = len(post_seg["tokens"])
    # Walk pre tokens for the same span; collect no_speech_prob values seen.
    nsp_values = set()
    consumed = 0
    while consumed < n_tokens and pre_token_index < sum(len(s["tokens"]) for s in pre_segs):
        # Find which pre segment owns the current pre_token_index.
        cum = 0
        for ps in pre_segs:
            if cum + len(ps["tokens"]) > pre_token_index:
                nsp_values.add(round(ps["no_speech_prob"], 6))
                step = min(n_tokens - consumed, cum + len(ps["tokens"]) - pre_token_index)
                pre_token_index += step
                consumed += step
                break
            cum += len(ps["tokens"])
    # Per hypothesis, all pre segments in this window share one no_speech_prob;
    # post's segment carries that same value.
    if abs(post_seg["no_speech_prob"] - max(nsp_values)) > 1e-6:
        import sys; sys.exit(1)
PY
  then
    echo "$vid: PER-WINDOW no_speech_prob DIVERGENCE"
    failures=$((failures + 1))
    continue
  fi

  echo "$vid: PASS"
done

echo
echo "Summary: $((total - failures))/$total passed."
exit $failures
```

(Adjust jq paths if the actual `TranscriptMetadata` JSON shape uses different field names — e.g., the spec uses `raw_signals.segments[].tokens[]`; verify against a real artifact before trusting the script.)

Run:
```bash
chmod +x /tmp/bake-t10-check.sh
/tmp/bake-t10-check.sh | tee /tmp/bake-t10-results.txt
```

- [ ] **Step 6: Wall-clock comparison**

Extract per-video wall-clock from each side. Source: structured logs emitted by the binary, OR `time` measurements wrapping each run. Pre-stage compute medians:

```bash
# Per-video median pre-T9 wall-clock vs post-T9 wall-clock.
# Replace with the actual log-grep that extracts wallclock from your operator setup.
# The bake notes precedent at docs/SRC-BAKE-NOTES.md shows "log-timestamp delta
# between `audio acquired` and `transcribed` log lines."
```

For each video, compute `median(pre_run1_ms, pre_run2_ms)` and `median(post_run1_ms, post_run2_ms)`. Compute aggregate: count of `post_median <= pre_median` (should be 20/20 per hypothesis).

- [ ] **Step 7: Decide based on outcomes**

Four possible outcomes (per spec):

**(a) All four signal checks PASS + wall-clock improves or stays equal on ≥ 18/20 videos** → Hypothesis confirmed. Append to `docs/SRC-BAKE-NOTES.md` (Step 8a).

**(b) All four signal checks PASS + wall-clock regresses on ≥ 3/20 videos** → Signals identical, but perf rationale evaporates. Spec says: revert anyway. Proceed to Step 8c (revert).

**(c) Any signal check FAILS** → Hypothesis is wrong; reverting T9 is mandatory. Proceed to Step 8c (revert).

**(d) Mixed: some videos pass signals but wall-clock disappoints unevenly** → Operator-judgment call. Conservative path: revert. If the operator decides to keep T9, the bake notes must document the per-video divergences honestly.

- [ ] **Step 8a (success): Append to `docs/SRC-BAKE-NOTES.md`**

```markdown
---

## Perf-tweaks T10 bake: set_no_timestamps quality + perf

**Date:** <YYYY-MM-DD>
**Branch:** `feat/perf-tweaks` @ <T9_SHA>
**Workspace:** SRC A10 (or wherever bake ran)
**Fixture:** news_orgs (20 URLs)
**Model:** <model used, e.g., ggml-large-v3-turbo-q5_0.bin>

**Outcome:** hypothesis confirmed.

| Signal | Result |
|--------|--------|
| Transcript text byte-equal | 20/20 PASS |
| Flattened token sequence (id+text) equal | 20/20 PASS |
| Per-token p/plog within 1e-6 | 20/20 PASS |
| Per-window no_speech_prob equal | 20/20 PASS |

**Segment-shape change (expected per hypothesis):** post-change segment count averaged <X> per video vs <Y> pre-change. Token-count per segment averaged <M> vs <N>.

**Wall-clock:** post-median per-video <= pre-median on <K>/20 videos. Aggregate per-video saved: <S> ms median (<P>%).

**Determinism note:** intra-side run1 vs run2 was byte-identical for all 20 videos on both sides, confirming `Greedy { best_of: 1 }` reproducibility under this fixture + this model + this CUDA build.
```

- [ ] **Step 8b (defer-with-honesty / partial outcome):** N/A in this task — the spec's gate is binary. If outcome (d) materializes and the operator wants to keep T9 anyway, that's an explicit override; document the per-video divergences in `docs/bake-findings.md` and proceed to Step 8a but with "PARTIAL — operator override" in the outcome line.

- [ ] **Step 8c (failure: revert T9)**

```bash
git revert <T9_SHA>
```

Commit message body must include the specific divergence:

```
revert: bake gate failed for T#2 set_no_timestamps

Pre-stated hypothesis from whisper.cpp source inspection did NOT
hold against the news_orgs fixture. Specifically: <which of the
four signal checks failed; with concrete numbers>.

Per spec § Bake plan #2 fail handling: no schema-impact /
AD0010-amendment work is attempted from inside this worktree.
Plan C may revisit if the perf win becomes more valuable.

Pre-bake intra-side runs: byte-identical (20/20).
Post-bake intra-side runs: <determinism status>.
Cross-side divergence: <summary>.

See docs/bake-findings.md for the full per-video divergence log.
```

Also append `docs/bake-findings.md`:

```markdown
---

## set_no_timestamps=true diverged from whisper.cpp source hypothesis

**Found in:** Perf-tweaks T10 bake (commits <T9_SHA> + <REVERT_SHA>).
**Disposition:** Reverted; deferred indefinitely.
**Trigger to revisit:** if whisper-rs or whisper.cpp ships a relevant update to no_timestamps semantics, or if the perf win becomes more valuable (e.g., under N-way concurrent inference where the small per-call save adds up).

**Hypothesis (unverified) at spec time:** per `~/src/whisper.cpp/src/whisper.cpp`, no_timestamps=true couples to single_segment=true. Per-token p/plog should be unchanged; per-window no_speech_prob unchanged.

**Observation:** <which signal diverged; concrete numbers; representative sample videos>.

**Possible causes (operator hypothesis, unverified):** <ideas, e.g., temperature fallback paths interact with the no_timestamps logits suppression in a way the source-read missed; whisper-rs 0.16.0's set_no_timestamps wraps whisper.cpp differently than the C API direct call; the pinned whisper.cpp commit has a delta from HEAD that affects this code path>.
```

- [ ] **Step 9: Commit the bake notes**

For outcome (a):
```bash
git add docs/SRC-BAKE-NOTES.md
git commit -m "$(cat <<'EOF'
bake(transcribe): T#2 set_no_timestamps quality + perf notes

Hypothesis from spec § "#2 set_no_timestamps(true)" empirically
confirmed against news_orgs fixture on the A10 workspace.

- 20/20: transcript text byte-equal pre vs post
- 20/20: flattened token (id + text) sequence equal
- 20/20: per-token p / plog within 1e-6 absolute
- 20/20: per-window no_speech_prob equal

Segment-shape change as predicted: segment count drops; tokens
per segment grow; confidence-signal granularity preserved.

Wall-clock: <K>/20 videos saw post-median <= pre-median. Median
per-video improvement: <S> ms (<P>%).

Determinism (intra-side run1 vs run2): byte-identical on both
sides under Greedy { best_of: 1 }.

Refs: AD0010, AD0017

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

For outcome (c), the revert + bake-findings commits in Step 8c are this task's outputs; no separate bake-notes commit.

---

## Self-check

- [ ] Intra-side determinism confirmed pre and post; the 4 signal checks were applied cross-side.
- [ ] Wall-clock medians computed per-video.
- [ ] Either `docs/SRC-BAKE-NOTES.md` gained a new dated section (outcome a) OR T9 was reverted AND `docs/bake-findings.md` documents the divergence (outcome c).
- [ ] The commit body honestly reports the outcome — no overclaiming.
- [ ] If reverted, T11 (FOLLOWUPS update) will NOT need to mention T9's resolution (since T9 was reverted, FOLLOWUPS doesn't have an entry to retire).
