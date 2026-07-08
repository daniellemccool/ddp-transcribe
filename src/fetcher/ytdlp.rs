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
/// `policy` selects the `-f` selector (frugal-default / deterministic-retry;
/// 2026-07-08 probe of 20 fresh videos + pilot-DB failure/success classes):
///
/// - [`FetchPolicy::Frugal`] (`-f "b[acodec!=none]/b"`, the default): picks
///   the smallest audio-tagged combined format and never selects TikTok's
///   `download` static asset. Motivation: `download` (the pre-rendered
///   watermarked share-link MP4) ran ~3x larger than the smallest ABR
///   variant across the probe (116.1 MB vs 39.9 MB over the 14 videos where
///   both landed, ~66% waste) while contributing no value here — we discard
///   video frames during postprocessing, so `download`'s visible
///   "watermarked" label has no effect on our output. Worse, `download`'s
///   advertised-but-unservable failure mode (selection succeeds, transfer
///   dies with "Did not get any data blocks") is exactly the pilot's
///   `NoDataBlocks` class (2,318 rows) — a selection-time fallback chain
///   cannot recover mid-transfer once `download` is picked. The smallest
///   advertised audio-bearing format served 17/17 probe videos with a real
///   audio stream (verified via ffprobe), including TikTok's occasional
///   audio-only `audio` format (509 KB vs multi-MB video). Verified against
///   probe fixtures: `-f "b[acodec!=none]/b"` + the `-S` sort below picked
///   h264_540p_298119-1 (228 KB) on a poisoned-class video,
///   h264_540p_235617-1 (261 KB) on a small video, and `audio` on a
///   slideshow post.
/// - [`FetchPolicy::DeterministicAudio`] (`-f "download/b[vcodec=h264]/b"`,
///   the previous unconditional default): pre-muxed audio with
///   selection-time fallbacks (best h264, then any best). Applied only to a
///   retry whose prior failure classified `FfprobePostprocess` — the
///   retained caveat and the reason this override exists: yt-dlp issues
///   #15891 / #16622 document that ABR variants intermittently serve h265
///   video-only files despite being tagged `acodec=aac` by the extractor
///   (`yt_dlp/extractor/tiktok.py` stamps the claim in
///   `COMMON_FORMAT_INFO`, regardless of what TikTok's CDN actually muxes).
///   Such a fetch fails at wav extraction and classifies as
///   `FfprobePostprocess`; `download` comes from a different TikTok
///   pipeline than the ABR variants and isn't subject to that liar-metadata
///   bug, so retrying with it recovers the video at the cost of the 3x
///   footprint for that one retry.
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
        FetchPolicy::Frugal => "b[acodec!=none]/b",
        FetchPolicy::DeterministicAudio => "download/b[vcodec=h264]/b",
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

    /// Frugal-default (2026-07-08 probe): `FetchPolicy::Frugal` is `Default`
    /// and must be the format `-f` emits when no policy override applies —
    /// smallest audio-tagged combined format, never `download`.
    #[test]
    fn build_args_selects_frugal_format_by_default() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, _) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::default(),
            None,
        );
        assert_eq!(FetchPolicy::default(), FetchPolicy::Frugal);

        let f_idx = args
            .iter()
            .position(|a| a == "-f")
            .expect("-f flag must be present");
        assert_eq!(
            args.get(f_idx + 1).map(String::as_str),
            Some("b[acodec!=none]/b"),
            "frugal default must select the smallest audio-tagged combined \
             format and never touch TikTok's `download` static asset"
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

    /// `FetchPolicy::DeterministicAudio` (the format-blamed-retry override)
    /// emits the previous unconditional default: `download` first, then the
    /// h264/best selection-time fallbacks.
    #[test]
    fn build_args_deterministic_audio_selects_download_first() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _, _) = build_yt_dlp_args(
            "abc123",
            "https://example.com/v",
            &video_dir,
            FetchPolicy::DeterministicAudio,
            None,
        );

        let f_idx = args
            .iter()
            .position(|a| a == "-f")
            .expect("-f flag must be present");
        assert_eq!(
            args.get(f_idx + 1).map(String::as_str),
            Some("download/b[vcodec=h264]/b"),
            "DeterministicAudio must prefer TikTok's pre-muxed `download` \
             static asset, fall back to best h264, then best — sidesteps \
             yt-dlp #15891/#16622 ABR liar-metadata bug"
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
