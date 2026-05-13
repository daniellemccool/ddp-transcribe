# Task 3 — Make ffmpeg postprocessor flags explicit

**Goal:** Extend the ffmpeg postprocessor-args string yt-dlp passes through (`src/fetcher/ytdlp.rs:55`) to explicitly drop video/subtitle/data streams, select only the first audio stream, and pin the WAV codec — making the "audio-only minimum-artifact" contract explicit rather than relying on yt-dlp+ffmpeg defaults.

**Spec commit:** 2 — `chore(fetcher): make ffmpeg postprocessor flags explicit`.

**ADRs directly relevant:**
- **AD0014** — audio-input invariant (float32 PCM 16 kHz mono via `hound`). The existing `-ar 16000 -ac 1` enforces sample rate + channel count; this task adds the stream/codec selection alongside.

**Files:**
- Modify: `src/fetcher/ytdlp.rs:55` (the postprocessor-args string)
- Modify: `src/fetcher/ytdlp.rs:139-141` (existing `build_args_enforces_audio_input_invariant` test must match the new string)

---

## Pre-implementation validation (mandatory)

**This step is non-skippable.** ffmpeg's `-map` flag is position-sensitive, and yt-dlp's generic `--postprocessor-args ffmpeg:…` form passes args at a specific point in yt-dlp's internally-constructed ffmpeg command line. The change could be inert (yt-dlp ignores some args), redundant (yt-dlp's defaults already supply them), or *break extraction* if `-map` lands in the wrong position. Verify before locking the arg string.

- [ ] **Step 1: Capture yt-dlp's current (pre-change) ffmpeg invocation**

Pick a representative TikTok URL the operator has access to (any public video; the bake fixture `news_orgs` has 20 candidates).

Run:
```bash
mkdir -p /tmp/ytdlp-verbose-check
cd /tmp/ytdlp-verbose-check
yt-dlp -v \
  --no-playlist --no-warnings \
  -f 'download/b[vcodec=h264]/b' \
  -x --audio-format wav \
  --postprocessor-args 'ffmpeg:-ar 16000 -ac 1' \
  -o '%(id)s.%(ext)s' \
  '<PASTE A REAL TIKTOK URL HERE>' 2>&1 | tee pre.log
```

Find the line in `pre.log` starting with `[debug] ffmpeg command line:` (or similar — yt-dlp's version may slightly vary). **Record the full ffmpeg argv** — this is the baseline. Note where `-ar 16000 -ac 1` lands in the argv (typically near the end, after output options).

- [ ] **Step 2: Capture yt-dlp's invocation with the proposed expanded args**

Run the same command with the proposed expansion:
```bash
yt-dlp -v \
  --no-playlist --no-warnings \
  -f 'download/b[vcodec=h264]/b' \
  -x --audio-format wav \
  --postprocessor-args 'ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1' \
  -o '%(id)s_post.%(ext)s' \
  '<SAME TIKTOK URL>' 2>&1 | tee post.log
```

Find the ffmpeg command line in `post.log`. **Verify the following:**
1. The new flags (`-vn -sn -dn -map 0:a:0 -c:a pcm_s16le`) all appear in the ffmpeg argv.
2. `-map 0:a:0` lands AFTER any `-i <input>` arg and BEFORE the output filename. If it lands before `-i`, ffmpeg will error or silently misbehave.
3. The output `.wav` file is produced and has the expected size/duration.
4. `ffprobe <output>.wav` reports: 1 channel, 16000 Hz sample rate, `pcm_s16le` codec.

If any of these checks fail, **stop and adjust the arg string** before continuing. Common adjustments:
- yt-dlp may map `--postprocessor-args ffmpeg:…` only to specific postprocessor steps; you may need the keyed form `--postprocessor-args ExtractAudio+ffmpeg:…` so the args land on the audio-extraction ffmpeg pass specifically.
- `-c:a pcm_s16le` may be redundant (yt-dlp's `--audio-format wav` may already produce `pcm_s16le` by default). If `ffprobe` on the pre.log output already shows `pcm_s16le`, you can drop the codec flag to keep the args minimal.

- [ ] **Step 3: Confirm the audio invariant (AD0014) round-trip**

The output WAV must decode cleanly through `hound`. From the worktree root:
```bash
cargo test --features test-helpers --test e2e_real_tools -- --ignored --nocapture 2>&1 | head -50
```

(Or use whichever existing test exercises real-fetch + decode_wav, if `e2e_real_tools` is gated to A10 only.) Goal: confirm the new args don't change the output WAV format in a way that breaks `hound::WavReader`.

If this test is unavailable on the dev machine, skip this step and rely on the bake (T8) to confirm. Document the skip in the commit message body.

---

## Implementation

- [ ] **Step 4: Update the postprocessor-args string in `src/fetcher/ytdlp.rs`**

In `src/fetcher/ytdlp.rs`, locate the string literal at line ~55 inside `build_yt_dlp_args`. Change:

```rust
        "--postprocessor-args".into(),
        "ffmpeg:-ar 16000 -ac 1".into(),
```

to:

```rust
        "--postprocessor-args".into(),
        // T3 perf-tweaks: make the audio-only minimum-artifact contract
        // explicit. `-vn -sn -dn` drop video/subtitle/data streams;
        // `-map 0:a:0` selects only the first audio stream; `-c:a
        // pcm_s16le` pins the WAV codec; `-ar 16000 -ac 1` enforces
        // AD0014. Validated via `yt-dlp -v` runtime introspection — see
        // commit body for the verbose-log snippet showing the resulting
        // ffmpeg command line.
        "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1".into(),
```

(If Step 2 validation showed `-c:a pcm_s16le` is redundant, drop it from the string above AND from the comment.)

- [ ] **Step 5: Update the existing `build_args_enforces_audio_input_invariant` test**

In `src/fetcher/ytdlp.rs` at lines 132-143, the existing test asserts the exact string `"ffmpeg:-ar 16000 -ac 1"` is in args. Update:

```rust
#[test]
fn build_args_enforces_audio_input_invariant() {
    // AD0014: audio input is float32 PCM 16 kHz mono. The yt-dlp
    // postprocessor enforces 16 kHz mono at the WAV-extraction boundary.
    // T3 perf-tweaks: the postprocessor-args string also makes the
    // stream-selection contract explicit (drop video/subtitle/data,
    // map first audio stream, pin pcm_s16le).
    let video_dir = PathBuf::from("/tmp/test-dir");
    let (args, _) = build_yt_dlp_args("abc123", "https://example.com/v", &video_dir);
    assert!(
        args.iter().any(|a| a == "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1"),
        "T3+AD0014: postprocessor-args must drop non-audio streams, \
         map first audio, pin pcm_s16le + 16 kHz + mono"
    );
}
```

(Adjust the assertion string and the doc to match the final validated arg string from Step 2 if it differs.)

- [ ] **Step 6: Run the unit tests**

Run:
```bash
cargo test --features test-helpers fetcher::ytdlp
```

Expected: all three `build_args_*` tests pass. The other two (`selects_download_format_first`, `wav_path_matches_output_template`) are unaffected by the change.

- [ ] **Step 7: Run the full test suite**

Run:
```bash
cargo test --features test-helpers
```

Expected: no regressions. The only test exercising the changed string is the one updated in Step 5.

- [ ] **Step 8: cargo fmt + clippy**

Run:
```bash
cargo fmt --all && cargo clippy --all-targets --features test-helpers -- -D warnings
```

Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/fetcher/ytdlp.rs
git commit -m "$(cat <<'EOF'
chore(fetcher): make ffmpeg postprocessor flags explicit

Extends the yt-dlp `--postprocessor-args ffmpeg:…` string from the
minimum `-ar 16000 -ac 1` (AD0014 enforcement) to also include
`-vn -sn -dn -map 0:a:0 -c:a pcm_s16le`. Makes the audio-only
minimum-artifact contract explicit at the postprocessor boundary
rather than relying on yt-dlp+ffmpeg defaults.

Validated via `yt-dlp -v` runtime introspection against a real
TikTok URL. Resulting ffmpeg command line shows the new flags
landing in the expected positions:

  <PASTE THE RELEVANT ffmpeg argv FROM Step 2 OF THE TASK PLAN>

ffprobe on the produced WAV confirms: 1 channel, 16000 Hz,
pcm_s16le codec. `hound::WavReader` decodes cleanly.

Refs: AD0014

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Replace `<PASTE THE RELEVANT ffmpeg argv …>` with the actual ffmpeg command line from Step 2. If the implementer did not run Step 2 (e.g., no internet access in the sandbox), the commit body MUST instead say: "Validation deferred to T8 bake — operator runs the verbose-log capture during bake. If bake reveals the flags land incorrectly, T3 is reverted as part of T8's fail handling."

---

## Self-check

- [ ] `yt-dlp -v` was run against a real TikTok URL pre- and post-change (or the deferral was documented in the commit body).
- [ ] The new ffmpeg argv shows the added flags in correct positions; ffprobe confirms output format.
- [ ] `cargo test --features test-helpers` passes; the updated `build_args_enforces_audio_input_invariant` assertion matches the final string.
- [ ] Commit message body includes the verbose-log snippet (or the documented deferral).
