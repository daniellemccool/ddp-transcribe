# Task 7 — Add `-S` sort flag for yt-dlp fallback ordering

**Goal:** Add `-S +size,+br,+res,+fps` to yt-dlp's args. The first selector token `download` is a literal format ID — `-S` does not change which selector matches, only the ordering within a match. So the success path (where `download` is available) is functionally unchanged; `-S` only affects ordering when the `b[vcodec=h264]/b` fallback runs, preferring smallest viable format. T13's bake reported 100% selector hit rate on `news_orgs`, so this change is likely defensive against extractor drift / future fixtures rather than a measured win — the bake in T8 confirms.

**Spec commit:** 6 — `feat(fetcher): add -S sort flag for yt-dlp fallback`.

**ADRs directly relevant:** none new. Existing AD0014 (audio-input invariant) is unaffected — `-S` orders format selection; the postprocessor still runs and enforces 16 kHz mono.

**Files:**
- Modify: `src/fetcher/ytdlp.rs:42-61` (`build_yt_dlp_args`: insert two new arg strings after the `-f` selector)
- Modify: `src/fetcher/ytdlp.rs:114-130` (extend `build_args_selects_download_format_first` test to also assert `-S` presence)

---

- [ ] **Step 1: Verify the `-S` syntax against the pinned yt-dlp version**

Run:
```bash
yt-dlp --version
yt-dlp --help 2>&1 | grep -A 3 "format-sort\| -S "
```

Expected: the help text confirms `-S, --format-sort` with `+size`, `+br`, `+res`, `+fps` accepted as sort fields (the `+` prefix prefers smallest). yt-dlp's sort syntax has evolved across versions; if the pinned version uses different sort-field names, adjust the value string accordingly. **Record the version + the confirmed syntax in the commit message body.**

- [ ] **Step 2: Write the test extension first**

Find `build_args_selects_download_format_first` at lines 114-130 of `src/fetcher/ytdlp.rs`. Extend it:

```rust
#[test]
fn build_args_selects_download_format_first() {
    let video_dir = PathBuf::from("/tmp/test-dir");
    let (args, _) = build_yt_dlp_args("abc123", "https://example.com/v", &video_dir);

    let f_idx = args
        .iter()
        .position(|a| a == "-f")
        .expect("-f flag must be present");
    assert_eq!(
        args.get(f_idx + 1).map(String::as_str),
        Some("download/b[vcodec=h264]/b"),
        "selector must prefer TikTok's pre-muxed `download` static asset, \
         fall back to best h264, then best — sidesteps yt-dlp #15891 \
         bitrateInfo h265 muxing bug"
    );

    // T7 perf-tweaks: -S sort flag must be present with the agreed value.
    // -S does not change which selector matches; it orders within a match.
    // Since `download` is a literal format ID, the success path is
    // unaffected; -S only sorts when the b[vcodec=h264]/b fallback runs,
    // preferring smallest viable size.
    let s_idx = args
        .iter()
        .position(|a| a == "-S")
        .expect("-S sort flag must be present after T7 perf-tweaks");
    assert_eq!(
        args.get(s_idx + 1).map(String::as_str),
        Some("+size,+br,+res,+fps"),
        "fallback ordering: smallest size first, then bitrate, resolution, fps"
    );
}
```

- [ ] **Step 3: Run the test — verify it fails**

Run:
```bash
cargo test --features test-helpers fetcher::ytdlp::tests::build_args_selects_download_format_first
```

Expected: test fails with `-S sort flag must be present after T7 perf-tweaks` panic message. This is the failing-test baseline.

- [ ] **Step 4: Add the `-S` args to `build_yt_dlp_args`**

In `src/fetcher/ytdlp.rs` at lines 45-59, modify the `args` vec literal. Insert two strings after the `-f` selector:

```rust
    let args = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--quiet".into(),
        "-f".into(),
        "download/b[vcodec=h264]/b".into(),
        // T7 perf-tweaks: -S only affects format ordering within a
        // selector match. `download` is a literal format ID, so the
        // success path is unaffected. The fallback `b[vcodec=h264]/b`
        // benefits — prefer smallest viable combined format, defensive
        // against future extractor drift or larger-than-needed h264 streams.
        "-S".into(),
        "+size,+br,+res,+fps".into(),
        "-x".into(),
        "--audio-format".into(),
        "wav".into(),
        "--postprocessor-args".into(),
        "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1".into(),
        "-o".into(),
        output_template,
        source_url.to_string(),
    ];
```

(The postprocessor-args string is whatever T3 locked in — preserve it; don't revert T3's change.)

- [ ] **Step 5: Run the test — verify it passes**

Run:
```bash
cargo test --features test-helpers fetcher::ytdlp
```

Expected: all three `build_args_*` tests pass.

- [ ] **Step 6: Run the full test suite**

Run:
```bash
cargo test --features test-helpers
```

Expected: no regression.

- [ ] **Step 7: cargo fmt + clippy**

Run:
```bash
cargo fmt --all && cargo clippy --all-targets --features test-helpers -- -D warnings
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/fetcher/ytdlp.rs
git commit -m "$(cat <<'EOF'
feat(fetcher): add -S sort flag for yt-dlp fallback ordering

Adds `-S +size,+br,+res,+fps` to the yt-dlp args alongside the
existing `-f download/b[vcodec=h264]/b` selector. The `-S` flag
orders format selection within a selector match, preferring smallest
size first.

Success path is unaffected: `download` is a literal format ID, so
when yt-dlp resolves the first selector token it returns exactly
that format — no ordering decision happens. `-S` only kicks in if
the `b[vcodec=h264]/b` fallback runs, where it prefers the smallest
viable combined format.

T13 A10 bake (Plan B Epic 1) reported 100% selector hit rate on the
news_orgs fixture (0/20 fallback). This change is therefore
defensive against future extractor drift or non-`download`-format
TikTok videos rather than a measured win on the current fixture.
T8 bake confirms.

yt-dlp version confirmed: <yt-dlp --version output from Step 1>
Sort syntax confirmed: -S accepts +size, +br, +res, +fps (smallest
first).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

(Replace `<yt-dlp --version output from Step 1>` with the actual version string.)

---

## Self-check

- [ ] `-S` and `+size,+br,+res,+fps` are in the args vec, in that order, immediately after the `-f` selector.
- [ ] The extended `build_args_selects_download_format_first` test asserts both.
- [ ] yt-dlp version + sort syntax confirmation captured in the commit body.
- [ ] T8 bake runbook is the next task — it confirms or refutes the change against `news_orgs`.
