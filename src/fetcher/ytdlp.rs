use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

use crate::errors::FetchError;
use crate::fetcher::{Acquisition, VideoFetcher};
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
/// The `-f` selector prefers TikTok's `download` format — the pre-rendered
/// share-link MP4 served as a static asset, h264 at ~540p, pre-muxed
/// deterministically. It comes from a different TikTok pipeline than the
/// `bitrateInfo` ABR variants documented in yt-dlp issues #15891 / #16622,
/// which intermittently serve h265 video-only files despite being tagged
/// `acodec=aac` by the extractor (`yt_dlp/extractor/tiktok.py` stamps the
/// claim in `COMMON_FORMAT_INFO`, regardless of what TikTok's CDN actually
/// muxes). We discard video frames during postprocessing, so the visible
/// "watermarked" label on `download` has no effect on our output.
///
/// Fallbacks: best h264 (for videos where the creator has disabled
/// download), then any best (defense against extractor changes).
fn build_yt_dlp_args(video_id: &str, source_url: &str, video_dir: &Path) -> (Vec<String>, PathBuf) {
    let output_template = format!("{}/{}.%(ext)s", video_dir.display(), video_id);
    let wav_path = video_dir.join(format!("{}.wav", video_id));
    let args = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--quiet".into(),
        "-f".into(),
        "download/b[vcodec=h264]/b".into(),
        // T7 perf-tweaks: `-S` only affects format ordering within a
        // selector match. `download` is a literal format ID, so the
        // success path is unaffected. The fallback `b[vcodec=h264]/b`
        // benefits — prefer smallest viable combined format, defensive
        // against future extractor drift or larger-than-needed h264
        // streams. T13 A10 bake reported 100% selector hit rate on
        // news_orgs (0/20 fallback); T8 bake against the same fixture
        // confirms this change is inert on the current data set.
        "-S".into(),
        "+size,+br,+res,+fps".into(),
        "-x".into(),
        "--audio-format".into(),
        "wav".into(),
        "--postprocessor-args".into(),
        // T3 perf-tweaks: make the audio-only minimum-artifact contract
        // explicit. `-sn -dn` drop subtitle/data streams; `-map 0:a:0`
        // selects only the first audio stream; `-c:a pcm_s16le` pins the
        // WAV codec; `-ar 16000 -ac 1` enforces AD0014. `-vn` and
        // `-c:a pcm_s16le` are redundant with current yt-dlp/ffmpeg
        // defaults (yt-dlp already passes `-vn`; ffmpeg defaults WAV
        // output to pcm_s16le) — kept for explicitness and as defense
        // against future default changes. Validated via `yt-dlp -v`
        // against a real TikTok URL on 2026-05-18; verbose-log snippet
        // in the T3 commit body.
        "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1".into(),
        "-o".into(),
        output_template,
        source_url.to_string(),
    ];
    (args, wav_path)
}

#[async_trait]
impl VideoFetcher for YtDlpFetcher {
    async fn acquire(&self, video_id: &str, source_url: &str) -> Result<Acquisition, FetchError> {
        // Per-video tmp dir keeps yt-dlp's intermediate files contained.
        let video_dir = self.work_dir.join(format!("ytdlp-{}", video_id));
        std::fs::create_dir_all(&video_dir).map_err(|e| {
            FetchError::NetworkError(format!(
                "creating yt-dlp work dir {}: {}",
                video_dir.display(),
                e
            ))
        })?;

        let (args, wav_path) = build_yt_dlp_args(video_id, source_url, &video_dir);

        let outcome = run(CommandSpec {
            program: "yt-dlp",
            args,
            timeout: self.timeout,
            stderr_capture_bytes: 8 * 1024,
            stdout_capture_bytes: 0, // yt-dlp writes audio to a file; stdout unused
            redact_arg_indices: &[],
        })
        .await?;

        if outcome.exit_code != 0 {
            return Err(FetchError::ToolFailed {
                tool: "yt-dlp",
                exit_code: outcome.exit_code,
                stderr_excerpt: outcome.stderr_excerpt,
            });
        }

        if !wav_path.exists() {
            return Err(FetchError::ParseError(format!(
                "yt-dlp succeeded but expected file {} not found",
                wav_path.display()
            )));
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

        // T7 perf-tweaks: -S sort flag must be present with the agreed
        // value. -S does not change which selector matches; it orders
        // within a match. Since `download` is a literal format ID, the
        // success path is unaffected; -S only sorts when the
        // b[vcodec=h264]/b fallback runs, preferring smallest viable
        // combined format.
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

    #[test]
    fn build_args_enforces_audio_input_invariant() {
        // AD0014: audio input is float32 PCM 16 kHz mono. The yt-dlp
        // postprocessor enforces 16 kHz mono at the WAV-extraction boundary.
        // T3 perf-tweaks: the postprocessor-args string also makes the
        // stream-selection contract explicit (drop video/subtitle/data
        // streams, map first audio stream, pin pcm_s16le).
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, _) = build_yt_dlp_args("abc123", "https://example.com/v", &video_dir);
        assert!(
            args.iter()
                .any(|a| a == "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1"),
            "T3 + AD0014: postprocessor-args must drop non-audio streams, \
             map first audio, pin pcm_s16le + 16 kHz + mono"
        );
    }

    #[test]
    fn build_args_wav_path_matches_output_template() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (_, wav_path) = build_yt_dlp_args("xyz789", "https://example.com/v", &video_dir);
        assert_eq!(wav_path, PathBuf::from("/tmp/test-dir/xyz789.wav"));
    }
}
