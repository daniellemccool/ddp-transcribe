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
// Consumed by `backfill::run_backfill_metadata`. 0045 made `lib.rs` the
// single module root, so the lib compilation now sees that caller and the
// item stays `pub(crate)` with no suppression.
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
///
/// An EMPTY cookie path returns the excerpt unchanged: `str::replace` with an
/// empty pattern splices the replacement between every character, shredding
/// the excerpt the classifier and the operator both read (Epic 5b hardening
/// of the Epic 3 FOLLOWUPS entry).
fn scrub_cookie_path(excerpt: String, cookies: Option<&Path>) -> String {
    match cookies {
        Some(path) if !path.as_os_str().is_empty() => {
            excerpt.replace(&path.display().to_string(), "[COOKIES-REDACTED]")
        }
        _ => excerpt,
    }
}

/// Filename prefix of a per-acquire attempt directory. The startup sweep
/// ([`crate::output::artifacts::cleanup_work_dirs`]) collects `.work` entries
/// by this prefix, and it is also how the test `FakeFetcher` recognizes a
/// staged attempt dir — keep the three in sync.
pub(crate) const ATTEMPT_DIR_PREFIX: &str = "ytdlp-";

/// Process-local monotonic sequence for attempt-dir names. Same convention as
/// [`crate::output::artifacts::atomic_write`]'s tmp names: pid makes the name
/// unique across concurrent processes, the sequence within this one.
static ATTEMPT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Path of a FRESH attempt directory for one `acquire` call:
/// `{work_dir}/ytdlp-{video_id}.{pid}-{seq}`.
///
/// Never reused: a retry of the same video, a concurrent fetch worker, and a
/// second process all get their own dir. That is what makes the exactly-one-WAV
/// scan below sound (nothing else writes here) and what makes cleanup safe
/// (removing this dir cannot touch another attempt's in-flight output).
/// `acquire` deliberately does NOT pre-clean prior `ytdlp-{video_id}.*` dirs —
/// one may belong to a live sibling. Crash residue is the startup sweep's job.
fn attempt_dir_path(work_dir: &Path, video_id: &str) -> PathBuf {
    work_dir.join(format!(
        "{ATTEMPT_DIR_PREFIX}{video_id}.{}-{}",
        std::process::id(),
        ATTEMPT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ))
}

/// Find the single `*.wav` yt-dlp produced in THIS attempt's directory.
///
/// The output path is discovered by scanning the attempt dir — never by
/// parsing yt-dlp's stdout. Stdout carries the Epic 4c metadata line and is
/// stored UNPARSED in `video_metadata_raw`; adding an untagged path line
/// there would corrupt `load-metadata`'s input.
///
/// Because the dir is fresh per acquire, the scan sees only this invocation's
/// output: exactly one wav ⇒ success; zero ⇒ [`FetchError::MissingOutput`]
/// (the pre-existing no-output failure, reported against `expected_wav` so the
/// message keeps naming the path the argv asked for); more than one ⇒
/// [`FetchError::AmbiguousOutput`] — a distinct failure, never a guess.
fn find_single_wav(attempt_dir: &Path, expected_wav: &Path) -> Result<PathBuf, FetchError> {
    let entries = std::fs::read_dir(attempt_dir).map_err(|e| FetchError::SystemIo {
        tool: "yt-dlp".to_string(),
        // SystemIo (retryable), not WorkDirCreate (Bug): a transient read
        // failure on one attempt dir must not cancel the whole batch.
        detail: format!("reading attempt dir {}: {e}", attempt_dir.display()),
    })?;
    let mut wavs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| FetchError::SystemIo {
            tool: "yt-dlp".to_string(),
            detail: format!(
                "reading attempt dir entry in {}: {e}",
                attempt_dir.display()
            ),
        })?;
        let path = entry.path();
        if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        {
            wavs.push(path);
        }
    }
    match wavs.len() {
        1 => Ok(wavs.remove(0)),
        0 => Err(FetchError::MissingOutput {
            path: expected_wav.to_path_buf(),
        }),
        count => Err(FetchError::AmbiguousOutput {
            dir: attempt_dir.to_path_buf(),
            count,
        }),
    }
}

/// Best-effort removal of an attempt directory. Used by `acquire`'s own
/// failure returns (the caller never receives a handle on those paths, so the
/// fetcher cleans up what it created) and by the pipeline's lifecycle points
/// via [`crate::pipeline::FetchedAudio`]. A failure here is warn-logged and
/// swallowed: the leftover dir is disk churn the startup sweep collects, never
/// a reason to change a video's outcome.
pub(crate) fn remove_attempt_dir(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %dir.display(), error = %e, "could not remove fetch attempt dir");
        }
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
        // Fresh per-acquire dir (Epic 5b): keeps yt-dlp's intermediate files
        // contained AND makes this invocation the only writer, which is what
        // the exactly-one-WAV scan below relies on. Ownership: every failure
        // return in this function removes the dir before returning (the caller
        // gets no handle on those paths); once `Ok(Acquisition)` is returned
        // the CALLER owns it and runs the lifecycle cleanup.
        let video_dir = attempt_dir_path(&self.work_dir, video_id);
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
            program: "yt-dlp".to_string(),
            args,
            timeout: self.timeout,
            stderr_capture_bytes: 8 * 1024,
            stdout_capture_bytes: STDOUT_CAP, // Epic 4c: --print line captured
            redact_arg_indices: &redact,
        })
        .await
        {
            Ok(o) => o,
            Err(e) => {
                remove_attempt_dir(&video_dir);
                return (None, Err(e.into()));
            }
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
            remove_attempt_dir(&video_dir);
            return (
                capture,
                Err(FetchError::ToolFailed {
                    tool: "yt-dlp".to_string(),
                    exit_code: outcome.exit_code,
                    signal: outcome.signal,
                    stderr_excerpt,
                }),
            );
        }

        // Exactly-one-WAV discovery over THIS attempt's dir. `wav_path` is the
        // name the `-o` template asked for; the scan is what decides, because
        // the extension yt-dlp actually wrote is not knowable from the argv
        // and stdout must stay the unparsed metadata capture.
        let wav_path = match find_single_wav(&video_dir, &wav_path) {
            Ok(p) => p,
            Err(e) => {
                remove_attempt_dir(&video_dir);
                return (capture, Err(e));
            }
        };

        (
            capture,
            Ok(Acquisition::AudioFile {
                wav: wav_path,
                attempt_dir: Some(video_dir),
            }),
        )
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

    /// Epic 5b (FOLLOWUPS: "`scrub_cookie_path` has no guard against an empty
    /// cookie path"): `str::replace` with an empty pattern inserts the
    /// replacement between EVERY character, shredding the excerpt. An empty
    /// path must leave the excerpt untouched.
    #[test]
    fn scrub_cookie_path_noop_on_empty_path() {
        let excerpt = "ERROR: unable to download video data".to_string();
        let scrubbed = scrub_cookie_path(excerpt.clone(), Some(Path::new("")));
        assert_eq!(
            scrubbed, excerpt,
            "an empty cookie path must not mangle the excerpt"
        );
    }

    /// Epic 5b: every acquire gets its OWN directory, so a re-acquire of the
    /// same video (retry, or a sibling process) can never observe or delete
    /// another attempt's in-flight output.
    #[test]
    fn attempt_dirs_are_unique_per_acquire() {
        let work = Path::new("/tmp/work");
        let a = attempt_dir_path(work, "7234567890123456789");
        let b = attempt_dir_path(work, "7234567890123456789");
        assert_ne!(a, b, "two acquires of the same video id must not collide");
        for d in [&a, &b] {
            assert_eq!(d.parent(), Some(work));
            let name = d
                .file_name()
                .and_then(|s| s.to_str())
                .expect("attempt dir name");
            assert!(
                name.starts_with(&format!("{ATTEMPT_DIR_PREFIX}7234567890123456789.")),
                "attempt dir must keep the sweepable `{ATTEMPT_DIR_PREFIX}` prefix \
                 and name the video: {name}"
            );
        }
    }

    /// Exactly-one-WAV discovery scans ONLY this attempt's dir; the reported
    /// path is never parsed out of stdout (stdout is the unparsed metadata
    /// capture — an untagged line there would corrupt `load-metadata`).
    #[test]
    fn find_single_wav_returns_the_only_wav() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let wav = dir.join("vid.wav");
        std::fs::write(&wav, b"riff").unwrap();
        // Non-wav residue (yt-dlp part files / the source media) is ignored.
        std::fs::write(dir.join("vid.mp4"), b"x").unwrap();
        std::fs::write(dir.join("vid.wav.part"), b"x").unwrap();
        let found = find_single_wav(dir, &wav).expect("exactly one wav");
        assert_eq!(found, wav);
    }

    /// Zero WAVs keeps the existing no-output failure (`MissingOutput`), which
    /// classifies as a retryable `ytdlp_other`.
    #[test]
    fn find_single_wav_zero_is_missing_output() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("vid.mp4"), b"x").unwrap();
        let expected = dir.join("vid.wav");
        match find_single_wav(dir, &expected) {
            Err(FetchError::MissingOutput { path }) => assert_eq!(path, expected),
            other => panic!("expected MissingOutput, got {other:?}"),
        }
    }

    /// More than one WAV is a DISTINCT failure — never pick one. Guessing
    /// would transcribe an arbitrary file and stamp it as the video's
    /// transcript.
    #[test]
    fn find_single_wav_multiple_is_ambiguous_failure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("vid.wav"), b"x").unwrap();
        std::fs::write(dir.join("vid-1.wav"), b"x").unwrap();
        match find_single_wav(dir, &dir.join("vid.wav")) {
            Err(FetchError::AmbiguousOutput { dir: d, count }) => {
                assert_eq!(d, dir);
                assert_eq!(count, 2);
            }
            other => panic!("expected AmbiguousOutput, got {other:?}"),
        }
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
