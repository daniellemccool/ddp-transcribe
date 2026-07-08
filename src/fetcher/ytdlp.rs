use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use crate::errors::FetchError;
use crate::fetcher::{Acquisition, FetchOpts, FetchPolicy, VideoFetcher};
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

/// Redact the cookie file path from a stderr excerpt so it never lands in
/// error messages or the state DB (ADR 0035). Pure function, factored out
/// of `acquire` for unit-testability.
fn scrub_cookie_path(excerpt: String, cookies: Option<&Path>) -> String {
    match cookies {
        Some(path) => excerpt.replace(&path.display().to_string(), "[COOKIES-REDACTED]"),
        None => excerpt,
    }
}

#[async_trait]
impl VideoFetcher for YtDlpFetcher {
    async fn acquire(
        &self,
        video_id: &str,
        source_url: &str,
        opts: &FetchOpts,
    ) -> Result<Acquisition, FetchError> {
        // Per-video tmp dir keeps yt-dlp's intermediate files contained.
        let video_dir = self.work_dir.join(format!("ytdlp-{video_id}"));
        std::fs::create_dir_all(&video_dir).map_err(|e| FetchError::WorkDirCreate {
            path: video_dir.clone(),
            detail: e.to_string(),
        })?;

        let (args, wav_path, redact) = build_yt_dlp_args(
            video_id,
            source_url,
            &video_dir,
            opts.format_policy,
            opts.cookies_file.as_deref(),
        );

        let outcome = run(CommandSpec {
            program: "yt-dlp",
            args,
            timeout: self.timeout,
            stderr_capture_bytes: 8 * 1024,
            stdout_capture_bytes: 0, // yt-dlp writes audio to a file; stdout unused
            redact_arg_indices: &redact,
        })
        .await?;

        if outcome.exit_code != 0 {
            let stderr_excerpt =
                scrub_cookie_path(outcome.stderr_excerpt, opts.cookies_file.as_deref());
            return Err(FetchError::ToolFailed {
                tool: "yt-dlp",
                exit_code: outcome.exit_code,
                signal: outcome.signal,
                stderr_excerpt,
            });
        }

        if !wav_path.exists() {
            return Err(FetchError::MissingOutput { path: wav_path });
        }

        Ok(Acquisition::AudioFile(wav_path))
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
}
