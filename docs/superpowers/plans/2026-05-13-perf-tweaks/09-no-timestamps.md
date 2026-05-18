# Task 9 — `params.set_no_timestamps(true)` in WhisperEngine inference

**Goal:** Tell whisper.cpp not to generate timestamp tokens during inference. Per source-level inspection of `~/src/whisper.cpp/src/whisper.cpp`, this couples to `single_segment=true` semantics — each 30s decode window emits exactly one segment instead of multiple. Per-token `id`/`text`/`p`/`plog` are unchanged; per-window `no_speech_prob` granularity is unchanged (already per-window). Small perf win expected (< 1% wall-clock). T10 bake validates the hypothesis with byte-for-byte transcript text + per-token equality within 1e-6 tolerance.

**Spec commit:** 8 — `feat(transcribe): set no_timestamps=true to skip timestamp generation`.

**ADRs directly relevant:** none new. The raw_signals schema (AD0010) is preserved at the per-token level; segment-grouping change is shape-only and within the pass-through rule.

**Files:**
- Modify: `src/transcribe.rs:492` (one new line immediately after `set_print_timestamps(false)`)

---

- [ ] **Step 1: Verify the change is a one-line addition by inspecting current code**

In `src/transcribe.rs`, locate the params construction around line 488-492:

```rust
let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
params.set_print_progress(false);
params.set_print_realtime(false);
params.set_print_special(false);
params.set_print_timestamps(false);
```

The new line goes immediately after `set_print_timestamps(false)`.

- [ ] **Step 2: No unit test required**

whisper-rs 0.16.0 exposes `set_no_timestamps` but **no corresponding getter**. A `params.no_timestamps()` assertion would not compile. Behavioral verification is the T10 bake. The spec confirms this explicitly.

This means the implementation step is a single edit with no preceding TDD step. The "test" for this change is T10.

- [ ] **Step 3: Make the change**

In `src/transcribe.rs:492`, add the new line:

```rust
let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
params.set_print_progress(false);
params.set_print_realtime(false);
params.set_print_special(false);
params.set_print_timestamps(false);
// T9 perf-tweaks: skip timestamp-token generation. Per whisper.cpp source
// inspection (see spec § "#2 set_no_timestamps(true)") this couples to
// single_segment=true; each 30s decode window emits exactly one segment
// instead of multiple. Per-token id/text/p/plog unchanged; per-segment
// no_speech_prob unchanged (was per-window already). Small wall-clock
// win from skipping timestamp-token sampling. T10 bake validates byte-
// for-byte transcript text equality + per-token equality within 1e-6.
params.set_no_timestamps(true);
```

- [ ] **Step 4: Run the existing test suite**

Run:
```bash
cargo test --features test-helpers
```

Expected: all existing tests pass. The change should not break anything mechanical — `set_no_timestamps` is a `FullParams` setter that doesn't change types or call sites. If anything fails, it's almost certainly an unrelated regression — investigate before continuing.

- [ ] **Step 5: cargo fmt + clippy**

Run:
```bash
cargo fmt --all && cargo clippy --all-targets --features test-helpers -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/transcribe.rs
git commit -m "$(cat <<'EOF'
feat(transcribe): set no_timestamps=true to skip timestamp generation

Adds `params.set_no_timestamps(true)` to the FullParams construction
in the WhisperEngine inference loop. whisper.cpp source-level
inspection (~/src/whisper.cpp/src/whisper.cpp) confirms this couples
to single_segment=true semantics in the inference loop — each 30s
decode window emits exactly one segment instead of multiple
timestamp-split segments. Per-token id/text/p/plog values are
unchanged; per-segment no_speech_prob granularity is unchanged
(whisper.cpp computes state->no_speech_prob per-window regardless).

Expected raw_signals shape change:
- segment count drops to ceil(duration_s / 30)
- segment.tokens list per segment gets correspondingly longer
- transcript text unchanged
- per-token p/plog unchanged within floating-point noise
- per-window no_speech_prob unchanged

Expected perf win: small (< 1% wall-clock). Saves timestamp-token
sampling + the timestamp-based segment-splitting loop. T10 bake
validates the hypothesis with explicit tolerances. If any signal
diverges beyond the documented tolerance, T10's revert commit
backs this out.

No unit test: whisper-rs 0.16.0 exposes set_no_timestamps but no
getter. Bake is the verification.

Refs: AD0010 (raw_signals schema; segment-grouping change is shape-
only within the pass-through rule)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `params.set_no_timestamps(true);` appears in `src/transcribe.rs` immediately after `params.set_print_timestamps(false);`.
- [ ] No unit test was added (whisper-rs has no getter).
- [ ] Full test suite passes.
- [ ] T10 bake is the verification step; this task hands off to T10.
