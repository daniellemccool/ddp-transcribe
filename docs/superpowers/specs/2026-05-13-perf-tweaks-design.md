# Perf-tweaks worktree design

**Date:** 2026-05-13
**Status:** Approved (brainstorm → spec)
**Branch:** `feat/perf-tweaks` (worktree at `../uu-tiktok-perf-tweaks`)
**Concurrent work:** Plan B Epic 2 runs in a separate session on a separate branch; this worktree must land **before T11** of Epic 2 to avoid collision on `src/pipeline.rs` and `src/process.rs`.

## Purpose

Land a small batch of efficiency and robustness tweaks across `yt-dlp`, `whisper.cpp` invocation, and subprocess plumbing — six items total, identified during code review and confirmed independent of Epic 2's anticipated changes via cross-session triage. One additional item is explicitly deferred to FOLLOWUPS pending an ADR amendment.

## Scope

### Implement in this worktree (6 items)

| # | What | Where |
|---|---|---|
| 1 | Lazy `lang_state` — `Option<WhisperState>`, allocate on first opt-in request | `src/transcribe.rs:434–448` |
| 2 | `params.set_no_timestamps(true)` alongside `set_print_timestamps(false)` | `src/transcribe.rs:492` |
| 3a | `serde_json::to_vec` (compact) instead of `to_vec_pretty` for transcript metadata | `src/pipeline.rs:151` |
| 4 | Add `-S` sort flag to yt-dlp args for fallback ordering | `src/fetcher/ytdlp.rs:~50` |
| 5 | Bounded streaming stdout/stderr capture (`VecDeque<u8>` ring) + `stdout_capture_bytes` field on `CommandSpec` + `Option<Vec<u8>>` stdout on `CommandOutcome`; eliminate `ring_buffer_tail` helper. **AD0021** lands with this commit. | `src/process.rs:100–149`, `docs/decisions/AD0021-…md` |
| 6 | Extend ffmpeg postprocessor args to `-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1` | `src/fetcher/ytdlp.rs:55` |

### Deferred to FOLLOWUPS (1 item)

| # | What | Reason |
|---|---|---|
| 3b | Drop `text` field from `RawToken` in `src/output/artifacts.rs` | `AD0010` froze the raw_signals schema v1 with a pass-through rule (`id` + `text` load-bearing for downstream filtering of special tokens like `[BEG]`, `<\|en\|>`). Dropping `text` is a schema break that needs an AD0010-amendment ADR and bake validation that downstream filtering still works. Outside the scope of this worktree. Amends existing FOLLOWUPS L89 entry in `docs/followups/plan-c.md`. |

## Per-item code design

### #1 Lazy `lang_state` allocation

- Replace `let mut lang_state = match ctx.create_state() { … }` at `src/transcribe.rs:440` with `let mut lang_state: Option<WhisperState> = None;`
- In the request loop, before any lang-probs branch fires:
  ```rust
  if req.config.compute_lang_probs && lang_state.is_none() {
      lang_state = Some(match ctx.create_state() {
          Ok(s) => s,
          Err(e) => {
              let _ = req.reply.send(Err(TranscribeError::Bug {
                  detail: format!(
                      "lazy lang_state create_state failure \
                       (should be classified, not Bug, in Epic 3): {e}"
                  ),
              }));
              continue;
          }
      });
  }
  ```
- **Error-variant decision (resolved in spec):** lazy `create_state` failure surfaces as `TranscribeError::Bug { detail }`. This matches the existing pattern at `src/errors.rs` for the `AudioDecodeError → TranscribeError::Bug` conversion (carries an Epic-3-will-reclassify breadcrumb in the detail string). No new variant is added; Epic 3's failure-classification taxonomy will rebuild the enum.
- Update the explanatory comment block at lines 434–439 to describe the new lifecycle: "lazily allocated; only present when a `compute_lang_probs=true` request has run during this worker's lifetime. AD0016 invariant preserved (state stays inside the worker thread)."

### #2 `set_no_timestamps(true)`

- Single new line immediately after `params.set_print_timestamps(false);` at `src/transcribe.rs:492`:
  ```rust
  params.set_no_timestamps(true);
  ```
- **Pre-stated hypothesis from whisper.cpp source-level inspection** (verified against `~/src/whisper.cpp/src/whisper.cpp` HEAD at spec time, via the whisper-cpp skill):
  - `no_timestamps=true` couples to `single_segment=true` in the inference loop. When either is set, `seek_delta = 100*WHISPER_CHUNK_SIZE` is forced — each 30s decode window emits exactly one segment.
  - `state->no_speech_prob` is computed once per window at the start of the first decode (line `state->no_speech_prob = probs[whisper_token_nosp(ctx)];`). Multiple segments within a window already share the same value, so dropping to "one segment per window" **does not lose `no_speech_prob` granularity** — it was per-window already.
  - Timestamp tokens (`vocab.token_beg..n_logits`) are suppressed to `-INFINITY` in `whisper_process_logits` when `no_timestamps=true`. Non-timestamp tokens are unaffected.
  - **Expected impact on raw_signals shape:** segment count drops to `ceil(duration_s / 30)`; segment.tokens list per segment gets correspondingly longer; per-token `id`/`text`/`p`/`plog` are unchanged; per-segment `no_speech_prob` is unchanged.
  - **Expected perf win:** small (< 1% wall-clock). The save is from skipping timestamp-token sampling and the timestamp-based segment-splitting loop; the encoder still runs fully.
- **Bake's job (commit 9 below):** confirm the hypothesis empirically AND decide whether the segment-shape change is acceptable for the measured perf win. The bake is no longer a surprise-detector — it is a hypothesis confirmation + acceptance gate.
- **Fail handling:** if raw_signals shape regresses beyond the hypothesis (e.g., per-token `p`/`plog` change, or transcript text differs), revert commit 8 and add a `docs/bake-findings.md` entry documenting the deviation from the hypothesis.

### #3a Compact JSON for transcript metadata

- Single-line swap at `src/pipeline.rs:151`: `serde_json::to_vec(&metadata)` instead of `to_vec_pretty(&metadata)`.
- The `AD0008` artifact-write-before-mark_succeeded invariant is preserved; we change only the encoder, not the ordering.

### #4 yt-dlp `-S` fallback sort

- In `build_args` at `src/fetcher/ytdlp.rs:~50`, add `--format-sort` (or `-S` short form, matching the style of the existing args) with value `+size,+br,+res,+fps`. Verify the exact syntax against the pinned yt-dlp version during implementation.
- The first selector token `download` is a literal format ID — `-S` does not change which selector matches; it only orders within a match. So the success path (where `download` is available) is functionally unchanged.
- Existing `build_args_selects_download_format_first` test gets an extension asserting the `-S` flag's presence and value.

### #5 Bounded subprocess capture + AD0021

- Replace the unbounded `read_to_end` pair at `src/process.rs:115–121` with a streaming chunk-read loop that fills a `VecDeque<u8>` of size `cap`. When `cap` is reached, leading bytes are popped before pushing new ones — peak memory is bounded by construction.
- Add `stdout_capture_bytes: usize` to `CommandSpec`, parallel to existing `stderr_capture_bytes`. Value `0` means "do not capture stdout" — the streaming reader still drains stdout (so the child does not block on a full pipe) but discards bytes as they arrive.
- `CommandOutcome::stdout` becomes `Option<Vec<u8>>` per codex-advisor recommendation (idiomatic Rust, distinguishes "captured but empty" from "intentionally discarded"). `None` ⇔ `stdout_capture_bytes == 0`; `Some(bounded Vec)` otherwise.
- The `ring_buffer_tail` helper is removed entirely. Existing callers reading `stderr_excerpt` see no behavioral change (the excerpt is still bounded; just produced by construction now).
- AD0021 ADR is drafted via `adg add` and lands as a separate commit immediately following the code change. Per `AD0001`, this feature-derived ADR rides the feature branch and merges into main with the code.

### #6 Explicit ffmpeg flags inside yt-dlp postprocessor-args

- Proposed postprocessor-args string at `src/fetcher/ytdlp.rs:55`: change from `"ffmpeg:-ar 16000 -ac 1"` to `"ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1"`.
- The added flags: `-vn -sn -dn` drop video/subtitle/data streams; `-map 0:a:0` selects only the first audio stream; `-c:a pcm_s16le` makes the WAV codec explicit.
- **Mandatory pre-implementation validation (added per second-review High-2):** ffmpeg's `-map` flag is position-sensitive, and yt-dlp's generic `--postprocessor-args ffmpeg:…` form passes args at a specific point in yt-dlp's own internally-constructed ffmpeg command line. The change could be inert (yt-dlp ignores some args) OR break extraction on some inputs if `-map` lands in the wrong position. **Before locking the arg string, run `yt-dlp -v --postprocessor-args 'ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1' …` against a representative TikTok URL and inspect the verbose log for the actual ffmpeg invocation.** Adjust the arg string (or split into per-postprocessor-key forms like `ExtractAudio+ffmpeg:…`) until the verbose output shows ffmpeg invoked with the flags in the intended positions. Document the verbose snippet in the implementation commit message.
- **Codec sanity check (also pre-implementation):** confirm against `AD0014` and the `hound` decoder whether `pcm_s16le` is correct. yt-dlp's `--audio-format wav` default may already produce `pcm_s16le`, in which case the codec flag is redundant and should be dropped. If `AD0014`'s float-format note implies `pcm_f32le` is preferred, adjust accordingly.
- Existing arg-assertion tests at `src/fetcher/ytdlp.rs:139–141` need updating to match the final string after both validations land.

## Test plan

All new test files require `[[test]] required-features = ["test-helpers"]` in `Cargo.toml` per `AD0005`.

### Cross-thread / closure-internal observability — pattern decision

Two tests need to observe state that lives inside a worker thread (#1) or a function-local closure (#5 Test C). The cleanest pattern across both is **`Arc<AtomicUsize>` instrumentation passed as an optional `test-helpers`-gated parameter**:

- **#1 `lang_state` lazy-alloc counter.** Add a `#[cfg(feature = "test-helpers")]` optional `lang_state_allocations: Arc<AtomicUsize>` field on the `WhisperEngine` config (or pass as an argument to the worker-spawn function). The worker thread increments it inside the lazy-alloc branch with `lang_state_allocations.fetch_add(1, Ordering::Relaxed)`. The test reads the counter after sending requests and asserts the expected value (0 for non-opt-in worker, 1 for first-and-only opt-in, still 1 for many opt-in requests).

- **#5 Test C `VecDeque<u8>` peak length.** Add a `#[cfg(feature = "test-helpers")]` optional `peak_buffer_len: Option<Arc<AtomicUsize>>` parameter on the streaming reader function. The reader does `peak_buffer_len.as_ref().map(|p| p.fetch_max(deque.len(), Ordering::Relaxed))` after each push/pop. The test wires up a counter, runs a pathological 10 MB emit with `stderr_capture_bytes = 8192`, and asserts the counter's final value is `<= 8192`.

Both use the same `Arc<AtomicUsize>` shape, both are `#[cfg(feature = "test-helpers")]`-gated so they cost nothing in production builds, and both keep the worker / closure internals private from production callers. This consistency makes the implementer's job mechanical rather than open-design.

### Per-item

**#1 — `tests/transcribe_lang_state.rs`** (new file). Uses the `lang_state_allocations: Arc<AtomicUsize>` counter pattern defined in the "Cross-thread / closure-internal observability" subsection above; no test-helpers accessor on `WhisperEngine` internals.
- Worker that never receives `compute_lang_probs=true` requests: send N non-opt-in requests, assert `lang_state_allocations.load() == 0`.
- First opt-in request: assert counter goes to 1. Subsequent opt-in requests: assert counter stays at 1 (state reused, not reallocated).
- A `create_state` failure on the lazy path returns `TranscribeError::Bug { detail: … }` for that request only; the worker keeps running and handles a follow-up non-opt-in request fine. Inject the failure by either mocking `ctx.create_state` (if feasible behind a test-helpers facade) or by exhausting VRAM in a manner the test environment can reproduce — concrete approach left to the implementation plan since it depends on whisper-rs's test seams.

**#2** — **no unit-level assertion.** whisper-rs 0.16.0 exposes `set_no_timestamps` but no corresponding getter; a `params.no_timestamps()` assertion would not compile. The hypothesis from §"#2 `set_no_timestamps(true)`" is verified empirically by the bake's per-token equality check (§ Bake plan #2 below).

**#3a** — extend `tests/output_artifacts.rs`:
- Round-trip: compact bytes parse back into `TranscriptMetadata`; structural equality assertion.
- Negative: compact form contains no leading-whitespace indentation bytes (no `\n  ` or `\n    ` sequences); this is the structural test that compact-vs-pretty differs.
- Size reduction is **informational, not asserted**. The one-segment one-token fixture is too small to make a meaningful relative-size assertion (the savings come from large multi-token-per-segment payloads). If the implementation plan wants a size-reduction test, it should build a fixture with at least 5 segments and ~50 tokens to make the assertion non-brittle; otherwise leave size as a `println!` for visual inspection.

**#4** — extend `build_args_selects_download_format_first` in `src/fetcher/ytdlp.rs` (or move to `tests/fetch_ytdlp.rs` if it exists):
- Assert `--format-sort` flag is present with the agreed value.

**#5 — `tests/process_bounded_capture.rs`** (new file — filename appears in Epic 2's anticipated list; this worktree writes it first):
- *Test A — stderr overflow:* spawn a child emitting `N × stderr_capture_bytes` bytes; assert `stderr_excerpt.len() <= stderr_capture_bytes` AND the bytes equal the tail of the emitted stream.
- *Test B — stdout opt-in / opt-out:* `stdout_capture_bytes == 0` ⇒ `outcome.stdout == None`; `stdout_capture_bytes == N` ⇒ `outcome.stdout == Some(bounded Vec)`.
- *Test C — peak memory bounded:* uses the `peak_buffer_len: Arc<AtomicUsize>` counter pattern from the observability subsection above. Wire the counter into the streaming reader, run a pathological 10 MB emit with `stderr_capture_bytes = 8192`, then assert `peak_buffer_len.load() <= 8192`.
- *Test D — exit code passthrough:* bounded capture does not lose process exit signal.

**#6** — extend the `build_args` test in `src/fetcher/ytdlp.rs:139–141`: assert the new postprocessor-args string contains all six ffmpeg flags in order.

## Bake plan

Two bake commits gate the two behavioral changes. Both append to `docs/SRC-BAKE-NOTES.md` (precedent: T13 A10 bake). Bake-only operational findings (e.g., "this change was inert against the current fixture") go to `docs/bake-findings.md` per the existing convention.

### Bake for #4 (yt-dlp `-S` sort) — commit 7 in the sequence

- Fixture: `news_orgs` 20-video set (`AD0017` ratified; T13 used it).
- Two runs: pre-change baseline (from commit 6's parent), post-change (from commit 6).
- Capture per-video: download success/fail, downloaded format ID, audio bytes-on-disk pre-decode.
- **Pass criteria:**
  - Selector hit rate stays at 100% — `download` format still wins on all videos where it won pre-change.
  - For any video where the fallback (`b[vcodec=h264]/b`) ran, post-change byte count ≤ pre-change byte count.
  - **Honest reporting if fallback never runs:** T13 reported 0/20 fallback on news_orgs; the bake notes must explicitly state when the change is unmeasurable against the current fixture, in which case the change is documented as defensive (for future fixtures or extractor drift) rather than a measured win.
- **Fail handling:** revert commit 6; add a `docs/bake-findings.md` entry; do not block the other commits.

### Bake for #2 (`set_no_timestamps`) — commit 9 in the sequence

- Same `news_orgs` fixture.
- Two runs (pre + post), each video run 2× per side to verify determinism under `Greedy { best_of: 1 }` (assert intra-side identical raw_signals across the 2× runs).
- Capture per-video: (a) full transcript text, (b) full `raw_signals.segments` payload — including per-segment `no_speech_prob` and the complete per-token `id`/`text`/`p`/`plog` arrays in emission order, (c) end-to-end transcription wall-clock.
- **Pass criteria — hypothesis-aligned, tight on confidence signals:**
  - **Transcript text:** byte-for-byte equality with the pre-change baseline.
  - **Per-token sequence:** concatenated `tokens.id` and `tokens.text` arrays across all segments are equal pre vs. post (token grouping into segments may change per the hypothesis, but the flattened token sequence must not).
  - **Per-token `p` and `plog`:** equal pre vs. post within absolute tolerance `1e-6` (covers floating-point reordering noise without permitting actual sampling drift). If equality is exact, great; if differences exceed `1e-6`, the hypothesis is wrong and the change must be reverted.
  - **Per-segment `no_speech_prob`:** for each pre-change segment, the post-change segment that contains its tokens must have an identical `no_speech_prob` value (since whisper.cpp computes it per-window — see hypothesis). Implementation: walk the flattened token sequence pre and post, find the window-boundary segment break, assert `no_speech_prob` matches.
  - **Wall-clock:** per-video post-change median ≤ pre-change median (median over the 2× intra-side runs). The hypothesis predicts a small win; a meaningful regression here is also a failure signal.
- **Fail handling:** if any of the four signal-equality checks fails (transcript bytes, token sequence, p/plog within tolerance, per-window `no_speech_prob`), revert commit 8 with subject `revert: bake gate failed for set_no_timestamps — see commit body`. The revert commit body documents which signal diverged and by how much. A new FOLLOWUPS entry routes the finding for future re-investigation. If only wall-clock regresses (signals identical), still revert — the perf rationale evaporates. **No schema-impact / AD0010-amendment work is attempted from inside this worktree even if signals diverge** — that's Plan C scope.

## AD0021 — Bounded subprocess output capture

**Numbering note.** Plan B Epic 2's sketch (`docs/superpowers/plans/2026-05-12-plan-b/EPIC-2-SKETCH.md`) anticipated this ADR as AD0023, but that numbering predates the recent landing of AD0018/AD0019/AD0020 (meta-process ADRs) on `main`. The next available number at spec time is AD0021. Epic 2's per-task expansion should number its remaining anticipated ADRs from AD0022 onward.

**Title:** Bounded subprocess output capture via streaming `VecDeque<u8>`

**Status:** accepted

**Context.** `src/process.rs::run` previously read child stdout AND stderr each into unbounded `Vec<u8>` via `tokio::io::AsyncReadExt::read_to_end`, then sliced an 8 KiB tail of stderr via `ring_buffer_tail` for inclusion in `CommandOutcome::stderr_excerpt`. The `stderr_capture_bytes` field only bounded the *retained excerpt*, not the peak memory during read. A misbehaving subprocess emitting many gigabytes to stderr would allocate all of it before truncation. FOLLOWUPS T6 documented this gap; Plan B Epic 2 anticipated an ADR to close it (numbered AD0023 in the Epic 2 sketch, renumbered to AD0021 here per the numbering note above).

**Decision.**

1. Replace `read_to_end` with a streaming chunk-read loop that maintains a `VecDeque<u8>` of size `cap`. When `cap` is reached, leading bytes are popped before pushing new ones — peak memory is bounded by construction at `cap` bytes.
2. Add `stdout_capture_bytes: usize` to `CommandSpec`, parallel to existing `stderr_capture_bytes`. Value `0` means "do not capture stdout" — the streaming reader still drains stdout (so the child does not block on a full pipe) but discards bytes as they arrive.
3. `CommandOutcome::stdout` becomes `Option<Vec<u8>>`. `None` ⇔ `stdout_capture_bytes == 0` (intentional discard); `Some(bounded Vec)` otherwise (capture requested, may still be empty). This distinguishes "captured but empty" from "intentionally discarded" — codex-advisor recommendation, idiomatic Rust.
4. The `ring_buffer_tail` helper is removed. Capture is bounded by construction, not by post-hoc tail-slicing. Existing callers reading `stderr_excerpt` see no behavioral change.

**Consequences.**

- **Retained-output memory ceiling** is now `stdout_capture_bytes + stderr_capture_bytes` per subprocess (call-site controlled), not "tool exit + truncation." Total peak memory during a `run` call is `retained + O(read_chunk_size_stdout + read_chunk_size_stderr + tokio task overhead)` — the chunk buffers used by the streaming reader hold at most one `read()` worth of bytes each before draining into the `VecDeque<u8>`. This caveat exists because "bounded by construction" applies to the retained buffer, not to instantaneous transient allocations.
- Call sites must explicitly opt in to stdout capture; `yt-dlp` and ffmpeg-postprocessor paths get `0` (no behavioral change — they did not read `outcome.stdout`).
- Plan B Epic 2's T13 inherits this design. T13 may add symmetric stdout policy decisions (different defaults for specific tools) without authoring a new ADR; if it changes the design, it supersedes AD0021 with a new ADR per existing convention.
- Test coverage in new `tests/process_bounded_capture.rs` covers overflow/preservation/exit-code/stdout opt-in/peak-memory-bounded.

**Branch placement:** `feat/perf-tweaks` (feature-derived ADR per `AD0001`; rides the feature branch into main).

**Supersedes:** none.
**Superseded by:** none.

## FOLLOWUPS updates

### Resolved entries — move full body to `docs/archive/followups-resolved.md` with commit SHA, remove scope-index line

1. **FOLLOWUPS.md L47** — `T6: process::run unbounded stdout/stderr → Epic 2 (bounded streaming capture)`
   - Resolved by: commit 4 (#5) on `feat/perf-tweaks`. Note in archive entry: "Resolved earlier than originally scoped — landed in perf-tweaks worktree per cross-session coordination with Epic 2 (other session). Epic 2's T13 inherits the design and may add symmetric stdout policy decisions on top."

2. **FOLLOWUPS.md L48** — `T6: ring_buffer_tail misnamed → Epic 2`
   - Resolved by: same commit 4. Note: "Resolved by elimination (bounded-by-construction; tail-slicing is no longer needed)."

3. **FOLLOWUPS.md L87** — `T8-Epic1: Lazy-allocate lang_state on first opt-in request → Plan C`
   - Resolved by: commit 1 (#1) on `feat/perf-tweaks`. Note: "Brought forward from Plan C scope. AD0016 invariants preserved (lang_state still scoped to the worker thread)."

### Split-resolution entry — amend in place at `docs/followups/plan-c.md`, update scope-index line

4. **FOLLOWUPS.md L89** — `T10-Epic1: Per-token id+text ~2× JSON artifact size → Plan C (when storage cost pinches)`
   - Amend the body to acknowledge commit 3 (#3a, compact JSON) addressed the pretty-printing component of the disk pressure.
   - The drop-text-field component remains deferred to Plan C unchanged — that part requires an AD0010 amendment.
   - Updated scope-index line: `T10-Epic1: Per-token text field doubles raw_signals payload → Plan C (compact JSON landed in perf-tweaks; drop-text still deferred pending AD0010 amendment)`

### New entries — only on bake failure

- If `#2` bake fails → `docs/bake-findings.md` operational entry plus a new FOLLOWUPS entry pointing at it, trigger "when whisper-rs or whisper.cpp ships a relevant update to `no_timestamps` semantics."
- If `#4` bake reveals the `-S` sort is inert against `news_orgs` → `docs/bake-findings.md` entry only; no FOLLOWUPS needed (the change is defensive and documented).

## Worktree setup

```
git worktree add -b feat/perf-tweaks ../uu-tiktok-perf-tweaks main
cd ../uu-tiktok-perf-tweaks
git status                          # expect: clean
git rev-parse --abbrev-ref HEAD     # expect: feat/perf-tweaks
```

- HTTPS for first push: `git push -u origin feat/perf-tweaks` (per `CLAUDE.md`).
- Untracked files in the main checkout (`docs/archive/`, `docs/reference/whisper-cpp-deepdive.md`) do not transfer; they remain in the main checkout.
- The Epic 2 session must not push to `feat/perf-tweaks`; coordination is by branch isolation, not file locks.

## Commit sequence

| # | Subject | Touches | Purpose |
|---|---|---|---|
| 0 | (worktree + branch creation) | — | `git worktree add` only |
| 1 | `refactor(transcribe): lazy-allocate lang_state on first opt-in request` | `src/transcribe.rs`, `tests/transcribe_lang_state.rs`, `Cargo.toml` | #1; closes FOLLOWUPS L87 |
| 2 | `chore(fetcher): make ffmpeg postprocessor flags explicit` | `src/fetcher/ytdlp.rs` | #6; final arg string confirmed against AD0014 + hound |
| 3 | `perf(pipeline): write compact JSON for transcript metadata` | `src/pipeline.rs`, `tests/output_artifacts.rs` | #3a; partially closes FOLLOWUPS L89 |
| 4 | `feat(process): bounded streaming subprocess capture` | `src/process.rs`, `tests/process_bounded_capture.rs`, `Cargo.toml` | #5; closes FOLLOWUPS L47 + L48 |
| 5 | `docs(adr): add AD0021 bounded subprocess capture` | `docs/decisions/AD0021-…md`, `docs/decisions/index.yaml` | Drafted via `adg add` |
| 6 | `feat(fetcher): add -S sort flag for yt-dlp fallback` | `src/fetcher/ytdlp.rs` | #4 code change |
| 7 | `bake(fetcher): T#4 yt-dlp -S sort fallback bake notes` | `docs/SRC-BAKE-NOTES.md` and/or `docs/bake-findings.md` | Pass/fail gate for #4 |
| 8 | `feat(transcribe): set no_timestamps=true to skip timestamp generation` | `src/transcribe.rs` | #2 code change |
| 9 | `bake(transcribe): T#2 set_no_timestamps quality + perf notes` | `docs/SRC-BAKE-NOTES.md` and/or `docs/bake-findings.md` | Pass/fail gate for #2 |
| 10 | `docs(followups): retire L47/L48/L87, amend L89 for perf-tweaks` | `docs/FOLLOWUPS.md`, `docs/followups/epic-2.md`, `docs/followups/plan-c.md`, `docs/archive/followups-resolved.md` | Backfills resolving commit SHAs |

Total: 10 commits. Each independently reviewable and selectively revertable.

## Open questions carried into the implementation plan

1. **#6 mandatory pre-implementation validation (lifted from per-item section for visibility).** Two coupled checks must run before the arg string is locked: (a) `yt-dlp -v --postprocessor-args 'ffmpeg:…' …` against a representative TikTok URL to confirm `-map` and the other flags land in the correct positions in yt-dlp's internal ffmpeg invocation; (b) codec sanity check against `AD0014` + `hound` decoder to confirm `pcm_s16le` matches the expected WAV format. The implementation commit message must include the verbose-log snippet showing the resulting ffmpeg command line.
2. **#5 streaming reader idiom.** Likely a `tokio::io::AsyncReadExt::read` loop into a small fixed-size chunk buffer, draining into the `VecDeque<u8>` and instrumenting `peak_buffer_len` after each push/pop. Confirm cancellation-safe pattern compatible with the existing `tokio::try_join!` shape; consider `tokio::select!` on `child.wait()` + the two reader futures if `try_join!` semantics block clean cancellation.
3. **#4 `-S` value.** Verify `+size,+br,+res,+fps` syntax against the yt-dlp version pinned in this repo / CI. yt-dlp's sort syntax has evolved across versions; the pinned-version docs are authoritative.
4. **#2 bake determinism.** Confirm whisper.cpp's `Greedy { best_of: 1 }` is bit-identical for a fixed input by running each fixture video 2× per side (pre + post) and asserting intra-side equality of all raw_signals values BEFORE comparing across sides. If intra-side equality fails on `p`/`plog`, raise the absolute tolerance from `1e-6` to a measured value — but document the floor.

(Closed during second review:)
- ~~#1 error-variant decision.~~ Resolved in spec: lazy `create_state` failure uses `TranscribeError::Bug { detail }` per existing convention; Epic 3 will reclassify.

## Cross-session coordination

The other session (Epic 2) has confirmed:

- Items #1, #2, #3a, #4, #6 are fully independent of Epic 2 phase 1 (T1–T11) and phase 2 (T12–T16).
- Item #5 directly overlaps with Epic 2's T13 + AD0021. The agreed split: this worktree lands the bounded-capture design + AD0021; Epic 2's T13 then absorbs only the symmetric stdout policy decisions (defaults per tool) and does not author a new ADR.
- This worktree must merge to `main` **before** Epic 2's T11 begins to avoid conflict on `src/pipeline.rs`. After this worktree merges, Epic 2 rebases on the updated `main` and inherits the changes.

If Epic 2's schedule slips and this worktree is ready first, the merge can proceed; if Epic 2 starts T11 first, the perf-tweaks worktree pauses and rebases when Epic 2 lands T1–T10.
