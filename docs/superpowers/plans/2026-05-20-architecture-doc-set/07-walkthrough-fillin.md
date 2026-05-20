# Task 7 — Walk-through fill-in (`index.md` §3)

**Goal:** Replace the stubbed §3 in `docs/reference/architecture/index.md` with real prose that threads the donor's journey through all four deepdive files. The stubs from T02 give the structural skeleton (5 stages with `→ see ...` links); this task replaces each stub paragraph with prose that names concrete components, cites file paths, and reads coherently top-to-bottom.

**ADRs referenced:** 0008 (mentioned in stage 4); other ADRs are referenced indirectly through deepdive cross-links.

**Files:**
- Modify: `docs/reference/architecture/index.md` (§3 only)

**Pre-reqs:** T01-T06 complete. The deepdives MUST exist before this task runs; the walk-through cites concrete sections inside them.

---

- [ ] **Step 1: Reread the four deepdives to refresh memory**

```bash
head -40 docs/reference/architecture/data-input.md
head -40 docs/reference/architecture/state-machine.md
head -40 docs/reference/architecture/transcription.md
head -40 docs/reference/architecture/orchestration.md
```

For each deepdive, identify the 2-3 most prominent named components / types / functions that a walk-through paragraph could name. Examples: `Store::claim_next`, `mark_succeeded`, the n=3+1 topology, the in-process whisper context, the sharded `output/<NN>/*.json` artifact path. The walk-through paragraphs should name these so a reader can follow into the deepdive and find the named thing.

- [ ] **Step 2: Rewrite Stage 1 — Ingest**

Replace the stub `**Stage 1 — Ingest.** ...` paragraph with prose along these lines (adjust for whatever the actual ingest code does, verified during T03):

```markdown
**Stage 1 — Ingest.** The operator runs `uu-tiktok ingest <export>` against an unpacked DDP archive. The parser (`src/ingest.rs`) walks the relevant JSON file inside the export, extracts each watched-video entry, and inserts one row per video into the state machine's `videos` table with status `pending`. Duplicates and malformed entries are handled per the parser's policy; the operator sees a summary stat at the end. → see [`data-input.md`](data-input.md).
```

The "unpacked DDP archive" / "relevant JSON file" / "summary stat" phrases must be verified against the deepdive's actual claims — match the prose to what `data-input.md` says.

- [ ] **Step 3: Rewrite Stage 2 — Claim**

Replace the stub `**Stage 2 — Claim.** ...` paragraph with prose along these lines:

```markdown
**Stage 2 — Claim.** The orchestrator's fetch workers call `Store::claim_next` from `src/state/store.rs`; the `BEGIN IMMEDIATE` transaction serializes contending workers so each `pending` row goes to exactly one worker. The row flips to `in_progress`, gets stamped with the worker's identity in `claimed_by`, and the worker receives a `Claim`. If a previous run crashed mid-claim, the stale-claim sweep that runs at orchestrator startup releases stuck rows back to `pending` before the new run claims them. → see [`state-machine.md`](state-machine.md) and [`orchestration.md`](orchestration.md).
```

Adjust names (`Store::claim_next`, `Claim`, etc.) to match the deepdives.

- [ ] **Step 4: Rewrite Stage 3 — Fetch and transcribe**

```markdown
**Stage 3 — Fetch and transcribe.** A claimed video's URL is passed to the fetcher (`src/fetcher/`), which invokes `yt-dlp` as a bounded subprocess and produces a local audio file. `src/audio.rs` normalizes it to 16kHz mono float32 PCM via `hound`. The PCM buffer plus the original `Claim` get sent over the bounded mpsc channel (capacity 2) to the single transcribe worker, which runs inference via the embedded `whisper-rs` context (one model loaded for the whole run). The transcriber emits one segment per chunk and accumulates token-level signals (`no_speech_prob`, `lang_probs`, segment-level metrics) for the final artifact. → see [`data-input.md`](data-input.md) and [`transcription.md`](transcription.md).
```

Adjust the named components to match the deepdives' actual claims. If the audio path differs (e.g., audio extraction happens in the fetcher rather than between fetcher and audio.rs), correct the prose accordingly.

- [ ] **Step 5: Rewrite Stage 4 — Persist**

```markdown
**Stage 4 — Persist.** The transcribe worker hands the completed transcription to the output writer (`src/output/`), which writes the JSON artifact to `output/<NN>/<video_id>.json` — sharded by the last two digits of the video ID. Only after the artifact is on disk does the worker call `Store::mark_succeeded`, which checks the claim is still live (via the WHERE predicate) before flipping `in_progress` to `succeeded`. The ordering is durability-critical: a crash between artifact-write and `mark_succeeded` leaves the row in `in_progress` for a future retry, with the artifact already on disk and ready to be overwritten on the next attempt (per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md)). → see [`transcription.md`](transcription.md) and [`state-machine.md`](state-machine.md).
```

- [ ] **Step 6: Rewrite Stage 5 — Failure paths**

```markdown
**Stage 5 — Failure paths.** Three failure modes thread through the pipeline. *Retryable failures* (network timeout, transient yt-dlp errors) call `Store::mark_retryable_failure` with a kind/message; the row leaves `in_progress` for the retry path. *Terminal failures* (video deleted upstream, hard parse error) call `Store::mark_terminal_failure` — the row is recorded permanently with the failure reason. *Stale claims* (worker crashed with `kill -9`, kernel OOM) leave a row stuck in `in_progress`; on the next orchestrator startup, the sweep at the top of the run releases those rows back to `pending`. Worker panics or unhandled errors trigger the orchestrator's cancellation token, which winds the JoinSet down in the load-bearing order described in [`orchestration.md`](orchestration.md). → see [`orchestration.md`](orchestration.md) and [`state-machine.md`](state-machine.md).
```

- [ ] **Step 7: Remove the parenthetical disclaimer from §3's intro**

The §3 intro paragraph (from T02) currently ends with `*(This section's full prose is written in T07 after the deepdives exist; stubs below.)*`. Remove that parenthetical now — the prose is in. The intro should now read:

```markdown
## 3. The donor's journey

A single donor's DDP export becomes a directory of transcript artifacts. Five stages thread through the four deepdive files.

**Stage 1 — Ingest.** [...]
```

- [ ] **Step 8: Verify line count and citations**

```bash
wc -l docs/reference/architecture/index.md
```

Expected: 230-290 lines (the walk-through fill-in adds ~30-40 lines over the T02 stub version).

```bash
grep -c "→ see " docs/reference/architecture/index.md
```

Expected: exactly 5 (one per stage in the walk-through).

```bash
grep -oP '`src/\K[^`]+' docs/reference/architecture/index.md | sort -u | while read f; do
  # strip trailing `:N` line number if present
  base=$(echo "$f" | sed 's/:.*$//')
  test -f "src/$base" && echo "OK src: $base" || echo "MISSING src: $base"
done
```

Expected: every line `OK`. Investigate any `MISSING`.

- [ ] **Step 9: Commit**

```bash
git add docs/reference/architecture/index.md
git commit -m "$(cat <<'EOF'
docs(reference): fill in architecture/index.md donor's-journey walk-through

§3 stub paragraphs replaced with prose that threads through all four
deepdive files. Each stage names concrete components from the deepdives
and ends with a `→ see` cross-link.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
