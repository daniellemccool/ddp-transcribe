use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use crate::errors::FetchError;
use crate::fetcher::{Acquisition, FetchOpts, FetchPolicy, MetadataCapture, VideoFetcher};
use crate::process::{run, CommandSpec};

pub struct YtDlpFetcher {
    /// Directory under which yt-dlp writes per-video subdirectories. Caller
    /// supplies a writable path under `transcripts_root`.
    pub work_dir: PathBuf,
    pub timeout: Duration,
}

impl YtDlpFetcher {
    pub fn new(work_dir: impl AsRef<Path>, timeout: Duration) -> Self {
        Self {
            work_dir: work_dir.as_ref().to_path_buf(),
            timeout,
        }
    }
}

/// Field-limited dict print (Epic 4c). One line of JSON (~0.6 KB measured
/// live 2026-07-28) from the info dict yt-dlp already holds — the bulky
/// `formats`/`thumbnails` arrays are deliberately excluded. The printed
/// set is wider than the typed schema-v5 columns; extras live only in the
/// raw envelope, available to future re-parses without re-fetch.
///
/// `--print` costs zero extra network requests, unconditionally: the info
/// dict is already extracted for the download, and metadata capture adds
/// no second yt-dlp invocation of any kind.
///
/// Captions/subtitles are deliberately absent (operator descope
/// 2026-07-28): nothing here requests them, so `subtitles` and
/// `automatic_captions` would be permanently empty were they listed.
pub(crate) const METADATA_PRINT_TEMPLATE: &str = "%(.{id,title,description,uploader,uploader_id,channel_id,timestamp,duration,view_count,like_count,comment_count,repost_count})j";

/// Stdout capture cap for yt-dlp invocations that print the metadata
/// line (~615 B/video measured live 2026-07-28; at-cap means the head
/// was dropped, so the envelope builder treats a full buffer as
/// unparseable). Shared by `acquire` and the backfill-metadata path.
pub(crate) const STDOUT_CAP: usize = 64 * 1024;

/// Build the yt-dlp argv and the expected output WAV path for a single video.
///
/// Pure function: no I/O, no global state. Unit-testable.
///
/// `policy` selects the `-f` selector (staged experiment, ADR 0038):
///
/// - [`FetchPolicy::DeterministicAudio`] (`-f "download/b[vcodec=h264]/b"`,
///   the default): TikTok's `download` format — the pre-rendered share-link
///   MP4 served as a static asset, h264 at ~540p, pre-muxed
///   deterministically — with selection-time fallbacks (best h264, then any
///   best). Byte-identical to the selector the pilot ran, retained as
///   default on that pilot-scale record (the frugal probe below is n=17).
///   `download` comes from a different TikTok pipeline than the
///   `bitrateInfo` ABR variants documented in yt-dlp issues #15891 /
///   #16622, which intermittently serve h265 video-only files despite
///   being tagged `acodec=aac` by the extractor
///   (`yt_dlp/extractor/tiktok.py` stamps the claim in
///   `COMMON_FORMAT_INFO`, regardless of what TikTok's CDN actually muxes)
///   — #16622 is still open against exactly the ABR formats the frugal
///   selector prefers, part of why the frugal flip is staged rather than
///   immediate. We discard video frames during postprocessing, so the
///   visible "watermarked" label on `download` has no effect on our output.
/// - [`FetchPolicy::Frugal`] (`-f "b[acodec!=none]/b"`): picks the smallest
///   audio-tagged combined format and never selects `download`. Applied
///   only to a retry whose prior failure classified `NoDataBlocks` —
///   `download`'s advertised-but-unservable failure mode (selection
///   succeeds, transfer dies with "Did not get any data blocks"; the
///   entire 2,318-row pilot class), which a selection-time fallback chain
///   cannot recover mid-transfer, so the retry must not re-pick `download`.
///   2026-07-08 probe evidence: the smallest advertised audio-bearing
///   format served 17/17 probe videos with a real audio stream (verified
///   via ffprobe), including TikTok's occasional audio-only `audio` format
///   (509 KB vs multi-MB video), at ~3x smaller footprint than `download`
///   (39.9 MB vs 116.1 MB over the 14 videos where both landed). Verified
///   against probe fixtures: `-f "b[acodec!=none]/b"` + the `-S` sort
///   below picked h264_540p_298119-1 (228 KB) on a poisoned-class video,
///   h264_540p_235617-1 (261 KB) on a small video, and `audio` on a
///   slideshow post. The bare `/b` fallback keeps the retry net closed: a
///   video advertising only audio-less formats (or an ABR liar per the
///   issues above) still downloads via `/b`, fails at wav extraction,
///   classifies as `FfprobePostprocess`, and simply retries on the
///   deterministic default — no special routing needed. A selector that
///   failed at selection time instead would strand such videos on a
///   generic label.
///
/// `cookies` (Epic 3, ADR 0035): when `Some`, appends `--cookies <path>`
/// immediately before the trailing `source_url` positional. The returned
/// `Vec<usize>` names the index (or indices) of the cookie path argument(s)
/// so the caller can pass `CommandSpec::redact_arg_indices` — the path must
/// never reach the structured subprocess log.
fn build_yt_dlp_args(
    video_id: &str,
    source_url: &str,
    video_dir: &Path,
    policy: FetchPolicy,
    cookies: Option<&Path>,
) -> (Vec<String>, PathBuf, Vec<usize>) {
    let selector = match policy {
        FetchPolicy::DeterministicAudio => "download/b[vcodec=h264]/b",
        FetchPolicy::Frugal => "b[acodec!=none]/b",
    };
    let output_template = format!("{}/{}.%(ext)s", video_dir.display(), video_id);
    let wav_path = video_dir.join(format!("{video_id}.wav"));
    let mut args = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--quiet".into(),
        // Epic 4c metadata capture: --print implies --simulate unless
        // --no-simulate is passed; the printed line lands on stdout before
        // the media transfer, so tool-failure outcomes still carry it.
        // These two flags cost zero extra network requests — the info dict
        // is already extracted for the download.
        "--no-simulate".into(),
        "--print".into(),
        METADATA_PRINT_TEMPLATE.into(),
        "-f".into(),
        selector.into(),
        // T7 perf-tweaks: `-S` only affects format ordering within a
        // selector match. When `policy` is `DeterministicAudio`, `download`
        // is a literal format ID, so the success path is unaffected there;
        // the `b[vcodec=h264]/b` fallback (and the `Frugal` selector's own
        // match) benefit — prefer smallest viable combined format, defensive
        // against future extractor drift or larger-than-needed streams.
        // T13 A10 bake reported 100% selector hit rate on news_orgs (0/20
        // fallback); T8 bake against the same fixture confirms this change
        // is inert on the current data set.
        "-S".into(),
        "+size,+br,+res,+fps".into(),
        "-x".into(),
        "--audio-format".into(),
        "wav".into(),
        "--postprocessor-args".into(),
        // T3 perf-tweaks: make the audio-only minimum-artifact contract
        // explicit. `-sn -dn` drop subtitle/data streams; `-map 0:a:0`
        // selects only the first audio stream; `-c:a pcm_s16le` pins the
        // WAV codec; `-ar 16000 -ac 1` enforces 0014. `-vn` and
        // `-c:a pcm_s16le` are redundant with current yt-dlp/ffmpeg
        // defaults (yt-dlp already passes `-vn`; ffmpeg defaults WAV
        // output to pcm_s16le) — kept for explicitness and as defense
        // against future default changes. Validated via `yt-dlp -v`
        // against a real TikTok URL on 2026-05-18; verbose-log snippet
        // in the T3 commit body.
        "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1".into(),
        "-o".into(),
        output_template,
    ];

    let mut redact = Vec::new();
    if let Some(cookie_path) = cookies {
        args.push("--cookies".into());
        args.push(cookie_path.display().to_string());
        redact.push(args.len() - 1);
    }

    args.push(source_url.to_string());
    (args, wav_path, redact)
}

/// Argv for a metadata-only invocation (backfill-metadata): print the
/// metadata line, transfer no media. `--skip-download` suppresses the
/// transfer; `--no-simulate` keeps `--print` from downgrading the run
/// to simulation (same as the fetch argv). Deliberately takes no
/// cookies parameter — the backfill cohort is succeeded videos, and
/// cookies ride only SensitiveLoginGated retries (ADR-0035).
// 0002: consumed by `src/backfill.rs`, which is bin-only (`mod backfill;`
// lives in main.rs, not lib.rs) — so the lib compilation never sees a
// caller and fires dead_code on this `pub(crate)` item. Permanent for as
// long as the backfill loop stays bin-only; nothing lifts it. (Widening
// to `pub` would silence the lint via the pub-item exemption, but that
// trades an honest allow for a wider library API.)
#[allow(dead_code)]
pub(crate) fn build_metadata_only_args(source_url: &str) -> Vec<String> {
    vec![
        "--no-playlist".to_string(),
        "--no-warnings".to_string(),
        "--quiet".to_string(),
        "--skip-download".to_string(),
        "--no-simulate".to_string(),
        "--print".to_string(),
        METADATA_PRINT_TEMPLATE.to_string(),
        source_url.to_string(),
    ]
}

/// Redact the cookie file path from a stderr excerpt so it never lands in
/// error messages or the state DB (ADR 0035). Pure function, factored out
/// of `acquire` for unit-testability.
fn scrub_cookie_path(excerpt: String, cookies: Option<&Path>) -> String {
    match cookies {
        Some(path) => excerpt.replace(&path.display().to_string(), "[COOKIES-REDACTED]"),
        None => excerpt,
    }
}

/// Build the versioned raw envelope (Epic 4c). Returns `None` when there is
/// nothing trustworthy to store: no capture, empty stdout, or stdout at the
/// capture bound (the bounded reader keeps the LAST `cap` bytes, so a full
/// buffer means the head was dropped ⇒ unparseable). The printed line is
/// embedded UNPARSED — parsing is `load-metadata`'s job, replayably.
///
/// Shape: `{"schema":1,"printed":"<unparsed line>"}`. `schema` is the
/// loader's compatibility gate.
pub(crate) fn build_metadata_envelope(stdout: Option<&[u8]>, capture_cap: usize) -> Option<String> {
    let bytes = stdout?;
    if bytes.is_empty() || bytes.len() >= capture_cap {
        return None;
    }
    let printed = String::from_utf8_lossy(bytes);
    let printed = printed.trim_end_matches(['\n', '\r']);
    if printed.is_empty() {
        return None;
    }

    #[derive(serde::Serialize)]
    struct MetadataEnvelope<'a> {
        schema: u32,
        printed: &'a str,
    }
    let env = MetadataEnvelope { schema: 1, printed };
    // Serialization of strings cannot fail in practice; treat an error as
    // "no envelope" per the never-a-new-failure-mode invariant.
    serde_json::to_string(&env).ok()
}

#[async_trait]
impl VideoFetcher for YtDlpFetcher {
    async fn acquire(
        &self,
        video_id: &str,
        source_url: &str,
        opts: &FetchOpts,
    ) -> (Option<MetadataCapture>, Result<Acquisition, FetchError>) {
        // Per-video tmp dir keeps yt-dlp's intermediate files contained.
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
        // yield metadata. Everything here is best-effort; the primary
        // `outcome` above already decided this video's fate and nothing
        // below can change it.
        let capture = build_metadata_envelope(outcome.stdout.as_deref(), STDOUT_CAP)
            .map(|envelope_json| MetadataCapture { envelope_json });
        if capture.is_none() {
            // Spec policy: empty/truncated/absent print output ⇒ no raw row,
            // fetch proceeds normally, event logged (the loader independently
            // skip-counts unparseable blobs on its side).
            tracing::warn!(video_id, "no metadata envelope captured for this fetch");
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

    fn name(&self) -> &'static str {
        "ytdlp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Staged experiment (ADR 0038): `FetchPolicy::DeterministicAudio` is
    /// `Default` and must be the format `-f` emits when no policy override
    /// applies — byte-identical to the selector the pilot ran.
    #[test]
    fn build_args_selects_download_format_by_default() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, _) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            None,
        );
        assert_eq!(FetchPolicy::default(), FetchPolicy::DeterministicAudio);

        let f_idx = args
            .iter()
            .position(|a| a == "-f")
            .expect("-f flag must be present");
        assert_eq!(
            args.get(f_idx + 1).map(String::as_str),
            Some("download/b[vcodec=h264]/b"),
            "default must prefer TikTok's pre-muxed `download` static asset, \
             fall back to best h264, then best — the pilot-proven selector; \
             sidesteps yt-dlp #15891/#16622 ABR liar-metadata bug"
        );

        // T7 perf-tweaks: -S sort flag must be present with the agreed
        // value, and applies regardless of policy.
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

    /// Epic 4c: the `--print` metadata flags ride the same invocation —
    /// zero extra network requests, since the info dict is already
    /// extracted for the download. `--no-simulate` is required or `--print`
    /// would imply simulate mode and skip the download.
    ///
    /// Operator descope 2026-07-28: captions/subtitles are NOT collected.
    /// No subtitle flag may appear on this argv — a subtitle download
    /// failure raises `DownloadError` (fatal in the pinned yt-dlp), and
    /// even listing-only capture (`--sub-langs "-all"`) spends the
    /// primary's timeout budget on TikTok's `_get_subtitles`. Metadata
    /// capture therefore adds exactly two flags and nothing else, so the
    /// epic's never-a-new-failure-mode invariant holds by construction.
    #[test]
    fn build_args_includes_metadata_print_only() {
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

        // Descoped: no subtitle surface at all.
        for flag in ["--write-subs", "--write-auto-subs", "--sub-langs"] {
            assert!(
                !args.iter().any(|a| a == flag),
                "{flag} must not appear: captions are descoped (2026-07-28)"
            );
        }
        assert!(
            !METADATA_PRINT_TEMPLATE.contains("subtitles")
                && !METADATA_PRINT_TEMPLATE.contains("automatic_captions"),
            "the print template must not request caption listings — nothing \
             sets `writesubtitles`, so they could only ever be empty"
        );

        // Trailing positional is still the URL; cookie redaction unaffected.
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.com/v")
        );
    }

    /// `FetchPolicy::Frugal` (the NoDataBlocks-retry experiment variant,
    /// ADR 0038) emits the smallest-audio-tagged selector and never
    /// `download`.
    #[test]
    fn build_args_frugal_selects_smallest_audio_tagged() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, _) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::Frugal,
            None,
        );

        let f_idx = args
            .iter()
            .position(|a| a == "-f")
            .expect("-f flag must be present");
        assert_eq!(
            args.get(f_idx + 1).map(String::as_str),
            Some("b[acodec!=none]/b"),
            "Frugal must select the smallest audio-tagged combined format \
             and never touch TikTok's `download` static asset — a \
             NoDataBlocks retry must not re-pick the format that died \
             mid-transfer"
        );
    }

    #[test]
    fn build_args_enforces_audio_input_invariant() {
        // 0014: audio input is float32 PCM 16 kHz mono. The yt-dlp
        // postprocessor enforces 16 kHz mono at the WAV-extraction boundary.
        // T3 perf-tweaks: the postprocessor-args string also makes the
        // stream-selection contract explicit (drop video/subtitle/data
        // streams, map first audio stream, pin pcm_s16le).
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, _) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            None,
        );
        assert!(
            args.iter()
                .any(|a| a == "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1"),
            "T3 + 0014: postprocessor-args must drop non-audio streams, \
             map first audio, pin pcm_s16le + 16 kHz + mono"
        );
    }

    #[test]
    fn build_args_wav_path_matches_output_template() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (_, wav_path, _) = build_yt_dlp_args(
            "xyz789",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            None,
        );
        assert_eq!(wav_path, PathBuf::from("/tmp/test-dir/xyz789.wav"));
    }

    /// Epic 3 T08: when a cookie path is supplied, `--cookies <path>` is
    /// appended before the trailing `source_url` positional, and the path
    /// arg's index is reported for `CommandSpec::redact_arg_indices` so it
    /// never lands in the structured subprocess log. Cookie-arg behavior is
    /// unaffected by `FetchPolicy`.
    #[test]
    fn build_args_appends_cookies_and_reports_redact_index() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let cookie = PathBuf::from("/secret/tiktok-cookies.txt");
        let (args, _, redact) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            Some(&cookie),
        );
        let ci = args
            .iter()
            .position(|a| a == "--cookies")
            .expect("--cookies present");
        assert_eq!(
            args.get(ci + 1).map(String::as_str),
            Some("/secret/tiktok-cookies.txt")
        );
        assert_eq!(
            redact,
            vec![ci + 1],
            "cookie path arg index must be redacted in logs"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.com/v"),
            "cookies must be inserted before the trailing source_url positional"
        );
    }

    /// Without a cookie path, argv is unchanged and nothing is flagged for
    /// redaction.
    #[test]
    fn build_args_without_cookies_is_unchanged() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, redact) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            None,
        );
        assert!(!args.iter().any(|a| a == "--cookies"));
        assert!(redact.is_empty());
    }

    /// Epic 3 T08 / ADR 0035 (Task 03 review forward-flag): the cookie path
    /// must not survive into a stored/logged stderr excerpt even when the
    /// tool's own error message happens to echo it back.
    #[test]
    fn scrub_cookie_path_redacts_when_present() {
        let cookie = PathBuf::from("/secret/tiktok-cookies.txt");
        let excerpt = "ERROR: could not read cookies file /secret/tiktok-cookies.txt: no such file"
            .to_string();
        let scrubbed = scrub_cookie_path(excerpt, Some(&cookie));
        assert_eq!(
            scrubbed,
            "ERROR: could not read cookies file [COOKIES-REDACTED]: no such file"
        );
    }

    #[test]
    fn scrub_cookie_path_noop_without_cookies() {
        let excerpt = "ERROR: some other failure".to_string();
        let scrubbed = scrub_cookie_path(excerpt.clone(), None);
        assert_eq!(scrubbed, excerpt);
    }

    /// Envelope contract after the 2026-07-28 descope:
    /// `{"schema":1,"printed":"<unparsed line>"}` — two keys, no more.
    #[test]
    fn build_envelope_wraps_printed_line_unparsed() {
        let stdout = b"{\"id\": \"123\", \"title\": \"t\"}\n".to_vec();
        let env = build_metadata_envelope(Some(&stdout), 64 * 1024).expect("envelope");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert_eq!(v["schema"], 1);
        assert_eq!(v["printed"], "{\"id\": \"123\", \"title\": \"t\"}");
        let obj = v.as_object().expect("envelope is a JSON object");
        assert!(
            !obj.contains_key("captions"),
            "captions are descoped: the key must be absent, not null"
        );
        assert_eq!(obj.len(), 2, "envelope carries exactly schema + printed");
    }

    #[test]
    fn build_envelope_none_on_empty_or_truncated_stdout() {
        // Empty stdout → no envelope.
        assert!(build_metadata_envelope(Some(&[]), 64).is_none());
        // At-the-bound stdout means the bounded reader may have dropped
        // leading bytes (truncated ⇒ unparseable) → no envelope.
        let at_cap = vec![b'x'; 64];
        assert!(build_metadata_envelope(Some(&at_cap), 64).is_none());
        // No capture at all → no envelope.
        assert!(build_metadata_envelope(None, 64).is_none());
    }

    #[test]
    fn metadata_only_args_exact_shape() {
        let args = build_metadata_only_args("https://www.tiktok.com/@u/video/123");
        assert_eq!(
            args,
            vec![
                "--no-playlist".to_string(),
                "--no-warnings".to_string(),
                "--quiet".to_string(),
                "--skip-download".to_string(),
                "--no-simulate".to_string(),
                "--print".to_string(),
                METADATA_PRINT_TEMPLATE.to_string(),
                "https://www.tiktok.com/@u/video/123".to_string(),
            ]
        );
    }

    #[test]
    fn metadata_only_args_never_download_subtitle_or_cookie_flags() {
        let args = build_metadata_only_args("https://example/v");
        for forbidden in [
            "-x",
            "-f",
            "-S",
            "-o",
            "--postprocessor-args",
            "--audio-format",
            "--cookies",
            "--write-subs",
            "--write-auto-subs",
            "--sub-langs",
            "--list-subs",
        ] {
            assert!(
                !args.iter().any(|a| a == forbidden),
                "metadata-only argv must not contain {forbidden}"
            );
        }
    }
}
