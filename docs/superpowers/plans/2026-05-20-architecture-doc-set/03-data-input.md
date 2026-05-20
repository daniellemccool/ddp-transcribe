# Task 3 — Populate `data-input.md`

**Goal:** Replace every `(TBD)` in `docs/reference/architecture/data-input.md` with real content describing the ingest path (DDP parsing) and the fetcher (yt-dlp integration at *design depth* — flags + effects + rationale, not a yt-dlp tutorial).

**ADRs referenced:** [0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md) (bounded subprocess output capture, applies to fetcher).

**Files:**
- Modify: `docs/reference/architecture/data-input.md`
- Read (to extract integration depth): `src/ingest.rs`, `src/fetcher/` (mod.rs and any helpers), `src/process.rs`, `src/audio.rs`

**Pre-reqs:** T01 complete (skeleton), T02 complete (index foundations — the glossary terms `donor`, `DDP`, `yt-dlp`, `watched video`, `hound` are now defined for cross-reference).

---

## How this task is structured

Each step writes one section. Before writing, *read the corresponding source file(s)* to ground claims in what the code actually does. The plan provides the section structure and a content checklist for each step. Specific facts (filenames, function names, line numbers, flag values) must come from the source, not from this plan.

The discipline:
- If a claim is in the spec or ADRs, redirect rather than restate.
- If a claim is about the code (which file does what, which flags are passed), cite `src/path/file.rs:N`.
- If a flag's name is self-explanatory, naming it is enough. If not, describe what it does and why we chose it.

---

- [ ] **Step 1: Survey the ingest code**

```bash
wc -l src/ingest.rs && head -40 src/ingest.rs
grep -n "^fn\|^pub fn\|^impl" src/ingest.rs
```

Note:
- The file's size (tells you the scope of what to describe)
- The public functions / entry points
- What format(s) the parser accepts (likely JSON given the DDP)

Read further as needed using `Read` to understand:
- What top-level shape the DDP export has (file structure, e.g., `Activity/Video Browsing History.json` or similar)
- What fields are extracted per watched video
- How malformed entries are handled (skip, error, log?)
- What ingest writes to the state machine (which columns get populated on insert)

- [ ] **Step 2: Write the Ingest section**

Replace the `## Ingest` `(TBD)` and its three sub-sections (`### DDP export shape`, `### Parsing strategy`, `### What becomes a row in state`) with content covering:

**`### DDP export shape`** — describe the DDP file structure as the parser expects it. Cite the exact file path within the export that the parser reads (e.g., `Activity/Video Browsing History.json` — verify against the code; do not assume). Describe the JSON structure at the level a reader needs to understand: top-level shape, array vs object, field names extracted. Do not reproduce TikTok's full DDP documentation — that's external-tool material and stays out.

**`### Parsing strategy`** — describe how `src/ingest.rs` walks the export. Common things to cover:
- Streaming vs eager parsing.
- How malformed entries are handled (skip with log? error and abort?).
- Deduplication policy (do duplicate video IDs in the export get collapsed, or each entry inserted separately?).
- Any normalization applied (URL canonicalization, video-ID extraction).

**`### What becomes a row in state`** — describe the column mapping. After ingest, each watched-video entry produces one row in the state machine's primary table. Cite the columns populated and any defaults (e.g., `status = 'pending'`, `attempt_count = 0`). Don't reproduce the schema here — that's the state machine doc's job. Cross-link: "for the row's full lifecycle, see [`state-machine.md`](state-machine.md)."

**Style guidance:**
- Length: aim for ~60-80 lines for the whole Ingest section.
- Cite `src/ingest.rs:N` for specific behavioral claims.
- Where canonicalization happens, cite `src/canonical.rs` if relevant (verify by reading it; canonical.rs may be unused or used elsewhere).
- ADR-redirect-first: if any ingest behavior is governed by an ADR, redirect.

- [ ] **Step 3: Survey the fetcher code**

```bash
ls src/fetcher/
wc -l src/fetcher/*.rs
grep -n "^fn\|^pub fn\|^impl\|Command::new\|\"yt-dlp\"\|--" src/fetcher/*.rs
grep -n "^fn\|^pub fn\|^impl" src/process.rs
```

Note:
- Where `yt-dlp` is invoked (likely `Command::new("yt-dlp")` or via `src/process.rs`'s subprocess runner).
- Which flags are passed (look for `.arg(...)` or `.args(&[...])` chains).
- Whether `src/process.rs` is the bounded-output-capture helper from ADR 0021, and whether the fetcher routes through it.
- How the output (the downloaded MP4 path, the audio extraction result) gets back to the caller.

Read further with `Read` to understand:
- The exact flag list passed to yt-dlp (write this down; you'll quote each in step 5).
- The output format (does yt-dlp write directly to a target path? Stream to stdout? Print a path?).
- The timeout policy (is there an explicit timeout? Inherited from `process::run`?).
- The error parsing (how does the fetcher distinguish "video unavailable" from "network error" from "yt-dlp itself broke"?).

- [ ] **Step 4: Write the Fetcher section header**

Replace the `## Fetcher` `(TBD)` with a 2-3 sentence intro:

```markdown
## Fetcher

The fetcher downloads each claimed video using `yt-dlp` as a subprocess. The orchestrator's fetch workers each invoke the fetcher once per claim; the fetcher returns the local path of the downloaded media (or an error), and a downstream step in the same worker extracts the audio for transcription. Subprocess output is bounded per [ADR 0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md) — neither stdout nor stderr is allowed to grow unbounded.
```

- [ ] **Step 5: Write `### Subprocess wrapping pattern`**

Describe how the fetcher invokes yt-dlp. Specifically cover:
- Whether invocation goes through `src/process.rs::run` (the bounded-output runner per 0021) or constructs its own `Command`. Cite the exact file:line.
- The form of the call — `tokio::process::Command` vs `std::process::Command`.
- Working directory and any environment-variable setup.
- How the path to the downloaded artifact is communicated back (e.g., via predictable filename from `--output` flag, or by parsing yt-dlp's stdout).

- [ ] **Step 6: Write `### yt-dlp invocation: flags and rationale`**

This is the integration-depth section the spec calls out specifically. For *each* yt-dlp flag the fetcher passes, write a bullet covering:
- The flag itself.
- What it does (one sentence, in our context — not a recitation of the yt-dlp man page).
- Why we use it (one sentence — what problem it solves for us, or what behavior it enforces).

Skip rationale if the flag's name is self-explanatory and the use case is obvious (e.g., `--quiet`). Spell out rationale for any flag whose presence is non-obvious to a reader who hasn't internalized our format requirements.

Example bullet shape (the actual flag list comes from the code):

```markdown
- `--format mp4` — restricts the download to MP4 containers. We want a predictable container for the downstream audio extraction step.
- `--no-playlist` — never expands a single URL into a playlist. TikTok URLs sometimes resolve to a user feed; this prevents accidental multi-fetch.
- `--output <template>` — places the file at a predictable path the fetcher can return without parsing yt-dlp's stdout.
```

The example bullets above are *illustrative*; verify the actual flag list against `src/fetcher/` and replace with the real list. Do not retain illustrative flags that aren't actually used.

- [ ] **Step 7: Write `### Output capture`**

One paragraph + cross-reference. The fetcher's stdout/stderr are bounded per [ADR 0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md) — a streaming `VecDeque<u8>` reader caps both streams at a configured byte budget. Describe the integration:
- Which helper in `src/process.rs` (or wherever) implements the cap.
- How the cap is configured (constant? env var? config file?).
- What happens when output exceeds the cap (truncated with a tail marker? Drop with log? Cite the code).
- That symmetric (stdout + stderr) capture is the policy.

Redirect the *why* (why streaming-bounded, why symmetric) to ADR 0021. Just describe *what* and *where*.

- [ ] **Step 8: Write `### Timeout policy`**

Describe how the fetcher bounds wall-clock time on a single yt-dlp run:
- Is there an explicit timeout? Cite the value and where it's set.
- Is the timeout per-fetch or per-process call?
- What happens on timeout — kill the subprocess, log, surface to the worker as a retryable failure?

If there is no explicit timeout, say so plainly. ("The fetcher does not impose an explicit wall-clock timeout on yt-dlp; the orchestrator's cancellation token kills the subprocess on shutdown — see [orchestration.md](orchestration.md).")

- [ ] **Step 9: Write `### Retry classification`**

Describe how the fetcher classifies a failed yt-dlp invocation into retryable vs terminal. Specifically:
- What categories of yt-dlp failure exist that we care about (network error, rate-limit, video-unavailable, parse error of yt-dlp output, yt-dlp itself crashes).
- How the code distinguishes them — exit code, stderr regex, both.
- Which category maps to `mark_retryable_failure` vs `mark_terminal_failure` (cite [ADR 0023](../../decisions/0023-minimum-mutator-signatures-kind-str-message-str-returning-result-usize-per-0006.md) for the mutator surface).

If the current code (as of Plan B Epic 2 in-flight) only has string-kind classification and the rich classifier is deferred to Epic 3, say so plainly. Honesty about in-flight state is preferred over speculation.

- [ ] **Step 10: Write `### Audio extraction handoff`**

Describe the handoff from raw fetched media to the audio prep step in `src/audio.rs`. Specifically:
- Whether the fetcher extracts audio itself (via a yt-dlp post-processor flag like `--extract-audio`) or whether a separate step does it.
- The output format of the extraction step (probably WAV; verify).
- The PCM format the transcription pipeline requires — 16kHz mono float32 — and where that conversion happens (audio.rs). Redirect to [ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16khz-mono-via-hound.md) for the invariant rationale.
- The cross-link to `transcription.md`'s "Audio preparation" section: "the resulting WAV is loaded by `src/audio.rs` (see [transcription.md](transcription.md))."

- [ ] **Step 11: Write the ADRs section**

Replace the `## ADRs governing this subsystem` `(TBD)` with a local subset table:

```markdown
## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0021 | Bounded subprocess output capture | Fetcher's yt-dlp invocation (stdout + stderr cap). |
| 0023 | Minimum mutator signatures | Retry classification surface (which mutator the fetcher calls on failure). |
| 0014 | Audio input invariant | Cross-references the audio prep boundary (full coverage in `transcription.md`). |
```

If during writing you discover other ADRs that apply (e.g., one of the meta-process ADRs governs how `process::run` is structured), add them. Verify by `grep -rn "ADR" src/ingest.rs src/fetcher/ src/process.rs` to see what the code itself references.

- [ ] **Step 12: Verify no `(TBD)` markers remain**

```bash
grep -n "(TBD" docs/reference/architecture/data-input.md
```

Expected: no output. If any `(TBD)` remains, return to the corresponding step.

- [ ] **Step 13: Verify line count is in range**

```bash
wc -l docs/reference/architecture/data-input.md
```

Expected: between 180 and 250 lines. Spec budget is ~210. Over 250 likely means the yt-dlp flag section drifted into tutorial territory (rationale paragraphs instead of one-sentence rationales) — tighten.

- [ ] **Step 14: Verify ADR links and src cites**

```bash
grep -oP '\(\.\./\.\./decisions/\K[^)]+' docs/reference/architecture/data-input.md | sort -u | while read f; do
  test -f "docs/decisions/$f" && echo "OK ADR: $f" || echo "MISSING ADR: $f"
done

grep -oP '`src/\K[^`:]+' docs/reference/architecture/data-input.md | sort -u | while read f; do
  test -f "src/$f" && echo "OK src: $f" || echo "MISSING src: $f"
done
```

Expected: every line starts with `OK`. Investigate any `MISSING`.

- [ ] **Step 15: Commit**

```bash
git add docs/reference/architecture/data-input.md
git commit -m "$(cat <<'EOF'
docs(reference): populate architecture/data-input.md

Ingest (DDP parsing) and fetcher (yt-dlp integration depth) — flags,
output capture per 0021, timeout, retry classification, audio handoff.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
