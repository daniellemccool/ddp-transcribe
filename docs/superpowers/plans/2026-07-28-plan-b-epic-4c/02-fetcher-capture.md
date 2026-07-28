# Task 02: Fetcher metadata capture — argv, envelope builder, trait widening

> **ARCHIVE NOTE (descoped 2026-07-28):** caption/subtitle collection described below was removed by operator decision at commit afa0253 — the envelope is `{"schema":1,"printed":…}` only. The creator's caption TEXT (`description`, = Research API `video_description`) remains captured via the print template. See task-02-report.md § Descope.

**Files:**
- Modify: `src/fetcher/mod.rs` (`MetadataCapture` type; `VideoFetcher::acquire` return widened; `FakeFetcher` gains `canned_metadata`)
- Modify: `src/fetcher/ytdlp.rs` (argv additions; stdout capture 64 KB; envelope builder + sidecar embed/cleanup; lift `CommandOutcome.stdout` usage)
- Modify: `src/process.rs` (ONLY: remove the now-stale `#[allow(dead_code)]` + comment on `CommandOutcome.stdout` — the field is consumed from this task on; 0002 lift-point)
- Modify: `src/pipeline/mod.rs` (`fetch_and_decode` threads the tuple; capture returned to callers)
- Modify: `src/pipeline/pipelined.rs`, `src/pipeline/serial.rs` (mechanical destructuring; capture DISCARDED as `_metadata_capture` — Task 03 persists it; leave a `// Epic 4c Task 03 wires persistence` comment at both sites)
- Test: unit tests in `src/fetcher/ytdlp.rs` + `src/fetcher/mod.rs` (in-module)

**Interfaces:**
- Consumes: `process::run` already returns `Ok(CommandOutcome)` (stdout included) on nonzero exit; only `RunError` (timeout/spawn/io) yields no outcome.
- Produces (Task 03 relies on these EXACT items):
  - `pub struct MetadataCapture { pub envelope_json: String }` in `src/fetcher/mod.rs`.
  - `async fn acquire(&self, video_id: &str, source_url: &str, opts: &FetchOpts) -> (Option<MetadataCapture>, Result<Acquisition, FetchError>)` — capture is `Some` whenever yt-dlp produced a non-empty, non-truncated printed line, on success AND tool-failure paths; `None` on RunError paths, empty stdout, or capture at the 64 KB bound.
  - `pub(crate) async fn fetch_and_decode(…) -> (Option<MetadataCapture>, Result<(Vec<f32>, PathBuf), FetchPhaseError>)` in `src/pipeline/mod.rs` — decode errors still carry `Some(capture)`.
  - `FakeFetcher.canned_metadata: std::sync::Mutex<Option<String>>` — when `Some(s)`, every `acquire` returns `Some(MetadataCapture { envelope_json: s.clone() })` alongside its configured outcome.

**Envelope contract (Tasks 03/04 depend on it):** serialized from

```rust
#[derive(Debug, serde::Serialize)]
struct MetadataEnvelope<'a> {
    schema: u32,                                            // always 1
    printed: &'a str,                                       // yt-dlp's printed line, UNPARSED, trimmed of trailing newline
    captions: Option<std::collections::BTreeMap<String, String>>, // sidecar filename -> content; None when no tracks
}
```

The fetcher NEVER parses `printed` — it only wraps it. `BTreeMap` for deterministic serialization.

- [ ] **Step 1: Write the failing argv unit tests**

In `src/fetcher/ytdlp.rs` `mod tests`:

```rust
    /// Epic 4c: metadata capture flags ride the same invocation — zero extra
    /// network requests. --no-simulate is required or --print would imply
    /// simulate mode and skip the download.
    #[test]
    fn build_args_includes_metadata_print_and_subs_flags() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, _) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            None,
        );
        let p_idx = args
            .iter()
            .position(|a| a == "--print")
            .expect("--print flag must be present");
        assert_eq!(
            args.get(p_idx + 1).map(String::as_str),
            Some(METADATA_PRINT_TEMPLATE),
        );
        assert!(args.iter().any(|a| a == "--no-simulate"));
        assert!(args.iter().any(|a| a == "--write-subs"));
        assert!(args.iter().any(|a| a == "--write-auto-subs"));
        // Trailing positional is still the URL; cookie redaction unaffected.
        assert_eq!(args.last().map(String::as_str), Some("https://example.com/v"));
    }
```

- [ ] **Step 2: Run to confirm it fails**

Run: `cargo test -p ddp-transcribe --lib build_args_includes_metadata -- --test-threads=1` (adapt: `cargo test build_args_includes_metadata -- --test-threads=1`)
Expected: COMPILE FAIL — `METADATA_PRINT_TEMPLATE` not defined.

- [ ] **Step 3: Add the template const + argv flags**

In `src/fetcher/ytdlp.rs`, above `build_yt_dlp_args`:

```rust
/// Field-limited dict print (Epic 4c). One line of JSON (~0.6 KB measured
/// live 2026-07-28) from the info dict yt-dlp already holds — the bulky
/// `formats`/`thumbnails` arrays are deliberately excluded. The printed
/// set is wider than the typed schema-v5 columns; extras live only in the
/// raw envelope, available to future re-parses without re-fetch.
pub(crate) const METADATA_PRINT_TEMPLATE: &str = "%(.{id,title,description,uploader,uploader_id,channel_id,timestamp,duration,view_count,like_count,comment_count,repost_count,subtitles,automatic_captions})j";
```

In `build_yt_dlp_args`, insert after the `"--quiet".into(),` element:

```rust
        // Epic 4c metadata capture: --print implies --simulate unless
        // --no-simulate is passed; the printed line lands on stdout before
        // the media transfer, so tool-failure outcomes still carry it.
        "--no-simulate".into(),
        "--print".into(),
        METADATA_PRINT_TEMPLATE.into(),
        // Platform-served caption tracks only (creator or TikTok auto
        // captions) — sidecars land in video_dir as {id}.{lang}.{ext};
        // measured corpus coverage ≈ 0%, so this is normally inert.
        "--write-subs".into(),
        "--write-auto-subs".into(),
```

(Cookie redact indices are computed AFTER these pushes via `args.len() - 1`, so they stay correct; the existing cookie tests must stay green.)

- [ ] **Step 4: Run argv tests to verify they pass**

Run: `cargo test build_args -- --test-threads=1`
Expected: PASS (all existing `build_args_*` tests + the new one).

- [ ] **Step 5: Write the failing envelope-builder tests**

The builder is a pure function over (stdout bytes, capture cap, sidecar files). In `src/fetcher/ytdlp.rs` `mod tests`:

```rust
    #[test]
    fn build_envelope_wraps_printed_line_unparsed() {
        let stdout = b"{\"id\": \"123\", \"title\": \"t\"}\n".to_vec();
        let env = build_metadata_envelope(Some(&stdout), 64 * 1024, &[]).expect("envelope");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["printed"], "{\"id\": \"123\", \"title\": \"t\"}");
        assert!(v["captions"].is_null());
    }

    #[test]
    fn build_envelope_none_on_empty_or_truncated_stdout() {
        // Empty stdout → no envelope.
        assert!(build_metadata_envelope(Some(&[]), 64, &[]).is_none());
        // At-the-bound stdout means the bounded reader may have dropped
        // leading bytes (truncated ⇒ unparseable) → no envelope.
        let at_cap = vec![b'x'; 64];
        assert!(build_metadata_envelope(Some(&at_cap), 64, &[]).is_none());
        // No capture at all → no envelope.
        assert!(build_metadata_envelope(None, 64, &[]).is_none());
    }

    #[test]
    fn build_envelope_embeds_caption_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let vtt = dir.path().join("abc123.en.vtt");
        std::fs::write(&vtt, "WEBVTT\n\n00:01.000 --> 00:02.000\nParis\n").unwrap();
        let stdout = b"{\"id\": \"abc123\"}".to_vec();
        let env =
            build_metadata_envelope(Some(&stdout), 64 * 1024, &[vtt.clone()]).expect("envelope");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert!(v["captions"]["abc123.en.vtt"]
            .as_str()
            .unwrap()
            .contains("Paris"));
    }

    #[test]
    fn collect_caption_sidecars_filters_by_extension_and_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("abc123.en.vtt");
        let wrong_prefix = dir.path().join("other.en.vtt");
        let wrong_ext = dir.path().join("abc123.wav");
        for p in [&keep, &wrong_prefix, &wrong_ext] {
            std::fs::write(p, "x").unwrap();
        }
        let found = collect_caption_sidecars(dir.path(), "abc123");
        assert_eq!(found, vec![keep]);
    }
```

Add `tempfile` usage is already available (dev-dependency; `tests/*` use it — for in-module tests confirm `tempfile` appears under `[dev-dependencies]` in Cargo.toml; it does, via existing usage in `src/` test modules or tests/. If not reachable from unit tests, move these two sidecar tests into `tests/state_metadata.rs`-style integration instead — prefer keeping them unit-level if the dep resolves.)

- [ ] **Step 6: Run to confirm they fail** (functions not defined)

Run: `cargo test build_envelope -- --test-threads=1` → COMPILE FAIL.

- [ ] **Step 7: Implement the envelope builder + sidecar collector**

In `src/fetcher/ytdlp.rs` (above the `VideoFetcher` impl):

```rust
/// Subtitle-sidecar extensions yt-dlp can write for TikTok tracks.
const CAPTION_EXTS: &[&str] = &["vtt", "srt", "ass", "lrc", "json3", "srv1", "srv2", "srv3", "ttml"];

/// Max bytes embedded per caption track; larger tracks are skipped (a
/// truncated subtitle file is corrupt, not useful) with a warn log.
const CAPTION_TRACK_CAP: usize = 256 * 1024;

/// Find caption sidecars for `video_id` in `video_dir`: files named
/// `{video_id}.<lang>.<caption ext>` (yt-dlp's naming under our `-o`
/// template). Pure directory scan; returns sorted paths for determinism.
fn collect_caption_sidecars(video_dir: &Path, video_id: &str) -> Vec<PathBuf> {
    let prefix = format!("{video_id}.");
    let mut found: Vec<PathBuf> = std::fs::read_dir(video_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
            name.starts_with(&prefix) && CAPTION_EXTS.contains(&ext)
        })
        .collect();
    found.sort();
    found
}

/// Build the versioned raw envelope (Epic 4c). Returns `None` when there is
/// nothing trustworthy to store: no capture, empty stdout, or stdout at the
/// capture bound (the bounded reader keeps the LAST `cap` bytes, so a full
/// buffer means the head was dropped ⇒ unparseable). The printed line is
/// embedded UNPARSED — parsing is `load-metadata`'s job, replayably.
fn build_metadata_envelope(
    stdout: Option<&[u8]>,
    capture_cap: usize,
    caption_files: &[PathBuf],
) -> Option<String> {
    let bytes = stdout?;
    if bytes.is_empty() || bytes.len() >= capture_cap {
        return None;
    }
    let printed = String::from_utf8_lossy(bytes);
    let printed = printed.trim_end_matches(['\n', '\r']);
    if printed.is_empty() {
        return None;
    }

    let mut captions = std::collections::BTreeMap::new();
    for path in caption_files {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        match std::fs::metadata(path).map(|m| m.len()) {
            Ok(len) if len as usize <= CAPTION_TRACK_CAP => match std::fs::read_to_string(path) {
                Ok(content) => {
                    captions.insert(name.to_string(), content);
                }
                Err(e) => tracing::warn!(file = name, error = %e, "caption sidecar unreadable; skipping"),
            },
            Ok(len) => tracing::warn!(file = name, bytes = len, "caption sidecar over cap; skipping"),
            Err(e) => tracing::warn!(file = name, error = %e, "caption sidecar stat failed; skipping"),
        }
    }

    #[derive(serde::Serialize)]
    struct MetadataEnvelope<'a> {
        schema: u32,
        printed: &'a str,
        captions: Option<std::collections::BTreeMap<String, String>>,
    }
    let env = MetadataEnvelope {
        schema: 1,
        printed,
        captions: if captions.is_empty() { None } else { Some(captions) },
    };
    // Serialization of strings/maps cannot fail in practice; treat an error
    // as "no envelope" per the never-a-new-failure-mode invariant.
    serde_json::to_string(&env).ok()
}
```

- [ ] **Step 8: Run envelope tests to verify they pass**

Run: `cargo test build_envelope collect_caption -- --test-threads=1` → PASS.

- [ ] **Step 9: Widen the trait + implementations**

In `src/fetcher/mod.rs`:

```rust
/// Raw fetch-time metadata capture (Epic 4c): the versioned envelope JSON
/// stored verbatim in `video_metadata_raw`. Produced on success AND
/// tool-failure paths; absent on structural failures (timeout/spawn/io).
#[derive(Debug, Clone)]
pub struct MetadataCapture {
    pub envelope_json: String,
}
```

Change the trait:

```rust
    /// Acquire the video's audio. The first tuple element is the raw
    /// metadata envelope when the tool produced one — present on success
    /// AND classified-failure paths (the printed line lands before the
    /// media transfer), absent on structural failures. Callers persist it
    /// BEFORE interpreting the outcome (Epic 4c).
    async fn acquire(
        &self,
        video_id: &str,
        source_url: &str,
        opts: &FetchOpts,
    ) -> (Option<MetadataCapture>, Result<Acquisition, FetchError>);
```

`YtDlpFetcher::acquire` (src/fetcher/ytdlp.rs) — restructure to build the capture once and return it on every classified path. Exact shape:

```rust
    async fn acquire(
        &self,
        video_id: &str,
        source_url: &str,
        opts: &FetchOpts,
    ) -> (Option<MetadataCapture>, Result<Acquisition, FetchError>) {
        let video_dir = self.work_dir.join(format!("ytdlp-{video_id}"));
        if let Err(e) = std::fs::create_dir_all(&video_dir) {
            return (
                None,
                Err(FetchError::WorkDirCreate {
                    path: video_dir.clone(),
                    detail: e.to_string(),
                }),
            );
        }

        let (args, wav_path, redact) = build_yt_dlp_args(
            video_id,
            source_url,
            &video_dir,
            opts.format_policy,
            opts.cookies_file.as_deref(),
        );

        const STDOUT_CAP: usize = 64 * 1024;
        let outcome = match run(CommandSpec {
            program: "yt-dlp",
            args,
            timeout: self.timeout,
            stderr_capture_bytes: 8 * 1024,
            stdout_capture_bytes: STDOUT_CAP, // Epic 4c: --print line captured
            redact_arg_indices: &redact,
        })
        .await
        {
            Ok(o) => o,
            Err(e) => return (None, Err(e.into())),
        };

        // Epic 4c: build the envelope BEFORE interpreting exit status — the
        // printed line lands pre-transfer, so mid-download deaths still
        // yield metadata. Sidecars are read + embedded, then deleted so the
        // per-video dir stays clean.
        let sidecars = collect_caption_sidecars(&video_dir, video_id);
        let capture = build_metadata_envelope(outcome.stdout.as_deref(), STDOUT_CAP, &sidecars)
            .map(|envelope_json| MetadataCapture { envelope_json });
        if capture.is_none() {
            // Spec policy: empty/truncated/absent print output ⇒ no raw row,
            // fetch proceeds normally, event logged (the loader independently
            // skip-counts unparseable blobs on its side).
            tracing::warn!(video_id, "no metadata envelope captured for this fetch");
        }
        for f in &sidecars {
            if let Err(e) = std::fs::remove_file(f) {
                tracing::warn!(file = %f.display(), error = %e, "caption sidecar cleanup failed");
            }
        }

        if outcome.exit_code != 0 {
            let stderr_excerpt =
                scrub_cookie_path(outcome.stderr_excerpt, opts.cookies_file.as_deref());
            return (
                capture,
                Err(FetchError::ToolFailed {
                    tool: "yt-dlp",
                    exit_code: outcome.exit_code,
                    signal: outcome.signal,
                    stderr_excerpt,
                }),
            );
        }

        if !wav_path.exists() {
            return (capture, Err(FetchError::MissingOutput { path: wav_path }));
        }

        (capture, Ok(Acquisition::AudioFile(wav_path)))
    }
```

`FakeFetcher`: add field + thread through:

```rust
    /// Epic 4c: when Some, every acquire returns this envelope string as
    /// its MetadataCapture (alongside whatever outcome the other knobs
    /// configure). Lets integration tests drive raw-row persistence
    /// through real worker dispatch.
    pub canned_metadata: std::sync::Mutex<Option<String>>,
```

All three constructors gain `canned_metadata: std::sync::Mutex::new(None),`. In `FakeFetcher::acquire`, compute the capture ONCE at the top (after the `received_opts` push):

```rust
        let capture = self
            .canned_metadata
            .lock()
            .expect("canned_metadata mutex")
            .clone()
            .map(|envelope_json| MetadataCapture { envelope_json });
```

then change every `return Err(…)` to `return (capture.clone(), Err(…))` — wait: `capture` moves; instead make each return site use `(capture, Err(…))` by restructuring to a single computed `outcome` then one return, OR simplest: since `MetadataCapture` is `Clone`, use `capture.clone()` at each early-return site and `(capture, …)` at the final one. Either shape is fine; keep it readable.

- [ ] **Step 10: Thread the tuple through `fetch_and_decode` and its callers**

`src/pipeline/mod.rs` (~line 292):

```rust
pub(crate) async fn fetch_and_decode(
    fetcher: &dyn VideoFetcher,
    claim: &Claim,
    opts: &FetchOpts,
) -> (
    Option<crate::fetcher::MetadataCapture>,
    Result<(Vec<f32>, PathBuf), FetchPhaseError>,
) {
    let (capture, acquisition) = fetcher
        .acquire(&claim.video_id, &claim.source_url, opts)
        .await;
    let acquisition = match acquisition {
        Ok(a) => a,
        Err(e) => return (capture, Err(e.into())),
    };
    …existing body…
    (capture, Ok((samples, wav_path)))
}
```

(The existing `?` on acquire and on `decode_wav` must become explicit matches/`map_err` that carry `capture` through — a decode failure still returns `Some(capture)`. Preserve the existing tracing call and comments.)

Callers (mechanical this task; persistence is Task 03):
- `src/pipeline/pipelined.rs` (~306): `r = fetch_and_decode(…) => r,` now binds the tuple; immediately after the select: `let (_metadata_capture, fetch_result) = fetch_result; // Epic 4c Task 03 wires persistence`.
- `src/pipeline/serial.rs` `process_one` (~271): `let (_metadata_capture, fetch_result) = fetch_and_decode(fetcher, claim, &fetch_opts).await; // Epic 4c Task 03 wires persistence` then `let (samples, wav_path) = fetch_result?;`.
- Any in-module test impls of `VideoFetcher` (serial.rs `mod tests` has one; grep `impl VideoFetcher` across `src/` and `tests/`) get the new signature returning `(None, <old result>)`.

- [ ] **Step 11: Lift the stdout dead-code allow**

In `src/process.rs`, on `CommandOutcome.stdout`, delete the `#[allow(dead_code)]` and rewrite its comment: the field is now consumed by `YtDlpFetcher` (Epic 4c); keep the None-vs-Some semantics sentence. Commit message carries the "0002 dead-code note:" paragraph recording the lift.

- [ ] **Step 12: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green (Task 01's 286 + 5 new unit tests = 291 passed; compiler-driven call-site fixes may touch pipeline_fakes test files — fix mechanically, disclose files touched).

- [ ] **Step 13: Commit**

```bash
git add -A src tests
git commit -m "feat(fetcher): capture yt-dlp metadata envelope at fetch time — --print + subs sidecars, tuple-widened acquire"
```

Body must include the 0002 dead-code note (stdout allow lifted) and disclose the intermediate `_metadata_capture` discard (persistence lands in the next task) per ADR-0003.
