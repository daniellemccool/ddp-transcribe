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

    /// Conditional secondary invocation (Epic 4c): fetch caption content
    /// for a video whose printed line listed caption tracks.
    ///
    /// Wholly best-effort. ANY failure — `RunError` or nonzero exit — logs
    /// a warning and yields no captions; the caller's fetch outcome is
    /// untouched by anything that happens here. That is why caption content
    /// no longer rides the outcome-defining primary argv.
    ///
    /// The stderr excerpt is deliberately NOT logged: yt-dlp echoes the
    /// cookie path in several of its own error messages, and this log line
    /// has no scrub step (ADR 0035). `stdout_capture_bytes: 0` for the same
    /// reason there's nothing to read — this run prints nothing we want.
    async fn fetch_caption_sidecars(
        &self,
        video_id: &str,
        source_url: &str,
        video_dir: &Path,
        cookies: Option<&Path>,
    ) -> Vec<PathBuf> {
        let (args, redact) = build_caption_fetch_args(video_id, source_url, video_dir, cookies);
        let succeeded = matches!(
            run(CommandSpec {
                program: "yt-dlp",
                args,
                timeout: self.timeout,
                stderr_capture_bytes: 8 * 1024,
                stdout_capture_bytes: 0,
                redact_arg_indices: &redact,
            })
            .await,
            Ok(outcome) if outcome.exit_code == 0
        );
        if !succeeded {
            tracing::warn!(video_id, "caption fetch failed; captions omitted");
            return Vec::new();
        }
        collect_caption_sidecars(video_dir, video_id)
    }
}

/// Field-limited dict print (Epic 4c). One line of JSON (~0.6 KB measured
/// live 2026-07-28) from the info dict yt-dlp already holds — the bulky
/// `formats`/`thumbnails` arrays are deliberately excluded. The printed
/// set is wider than the typed schema-v5 columns; extras live only in the
/// raw envelope, available to future re-parses without re-fetch.
///
/// `--print` itself costs zero extra network requests: the info dict is
/// already extracted for the download. The `subtitles`/`automatic_captions`
/// members of this template are only a *listing* of available tracks —
/// fetching caption **content** costs one extra, conditional invocation
/// (see [`build_caption_fetch_args`]), which never rides the argv below.
///
/// The listing is not free-standing: yt-dlp populates `subtitles` only when
/// the `writesubtitles` param is set (`extract_subtitles`,
/// `yt_dlp/extractor/common.py:3870`), so the primary argv must carry
/// `--write-subs --write-auto-subs --sub-langs "-all"` for this template
/// member to be anything but empty. See `build_yt_dlp_args`.
pub(crate) const METADATA_PRINT_TEMPLATE: &str = "%(.{id,title,description,uploader,uploader_id,channel_id,timestamp,duration,view_count,like_count,comment_count,repost_count,subtitles,automatic_captions})j";

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
        // Caption LISTING only — no track is downloaded here.
        //
        // yt-dlp populates the info dict's `subtitles` field only when the
        // `writesubtitles` param is set (`extract_subtitles`,
        // `yt_dlp/extractor/common.py:3870`), so without these flags the
        // printed template's `subtitles` member is always empty and the
        // secondary invocation's gate could never open. The zero-language
        // selection `-all` is what keeps this safe: `_write_subtitles`
        // early-returns on an empty selection (`elif not subtitles: return
        // ret`, `yt_dlp/YoutubeDL.py:4458`) before any download is
        // attempted. Live-verified 2026-07-28 on a captioned corpus video —
        // printed `subtitles` populated, ZERO sidecar files written, exit 0.
        //
        // Track DOWNLOADS must never ride this invocation: a subtitle
        // download failure raises `DownloadError` unless `ignoreerrors` is
        // set (`yt_dlp/YoutubeDL.py:~4498`), which would flip a good fetch
        // to nonzero exit and manufacture a spurious video failure. Content
        // capture therefore stays on the guarded, best-effort secondary
        // invocation (`build_caption_fetch_args`) — the epic's
        // never-a-new-failure-mode invariant holds by construction.
        "--write-subs".into(),
        "--write-auto-subs".into(),
        "--sub-langs".into(),
        "-all".into(),
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

/// Build the argv for the conditional secondary caption-only invocation
/// (Epic 4c). Pure function; unit-testable.
///
/// `--skip-download` means no media transfer: this run only fetches the
/// caption tracks the primary run's printed line said exist, writing
/// sidecars into `video_dir` under the SAME `-o` template the primary uses
/// (so `collect_caption_sidecars` finds them by the same naming rule).
///
/// This invocation's outcome is discarded — any failure is best-effort and
/// leaves the video's fetch outcome untouched. That is the whole point of
/// splitting it out of the primary argv.
///
/// Cookies mirror the primary exactly (ADR 0035): when `Some`, append
/// `--cookies <path>` immediately before the trailing `source_url`
/// positional, and report the path arg's index so the caller can pass
/// `CommandSpec::redact_arg_indices`.
fn build_caption_fetch_args(
    video_id: &str,
    source_url: &str,
    video_dir: &Path,
    cookies: Option<&Path>,
) -> (Vec<String>, Vec<usize>) {
    let output_template = format!("{}/{}.%(ext)s", video_dir.display(), video_id);
    let mut args = vec![
        "--no-playlist".into(),
        "--no-warnings".into(),
        "--quiet".into(),
        "--skip-download".into(),
        "--write-subs".into(),
        "--write-auto-subs".into(),
        // Without an explicit language selection yt-dlp downloads a single
        // preferred language (English-biased), so a multi-track video would
        // silently lose every other track despite the envelope's captions
        // map being keyed per file. Take them all — the envelope embeds one
        // entry per sidecar, and the per-track cap bounds the cost.
        "--sub-langs".into(),
        "all".into(),
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
    (args, redact)
}

/// Shallow peek at the printed line: does it list any caption track?
///
/// This is the ONE place the fetcher inspects the printed JSON — a two-key
/// existence check to decide whether the conditional secondary caption
/// invocation is warranted. The envelope still stores the line UNPARSED;
/// typed parsing remains `load-metadata`'s job.
///
/// Corrected coverage (n=11 re-probe, 2026-07-28): 4/11 corpus videos list
/// caption tracks, ~36%. The earlier "≈0%" figure was an artifact of
/// probing without the listing flags — yt-dlp leaves `subtitles` empty
/// unless `writesubtitles` is set, so the gate looked structurally shut.
/// The secondary therefore really runs, for roughly a third of fetches,
/// once per video that reaches a clean primary; its efficiency matters.
///
/// True iff `subtitles` or `automatic_captions` is a non-empty JSON object.
/// Anything else — absent key, `null`, `{}`, a non-object, or input that
/// isn't JSON at all — is false, so the secondary invocation is skipped and
/// the fetch behaves exactly as it did before caption capture existed.
fn printed_lists_caption_tracks(printed: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(printed) else {
        return false;
    };
    ["subtitles", "automatic_captions"].iter().any(|key| {
        value
            .get(*key)
            .and_then(serde_json::Value::as_object)
            .is_some_and(|tracks| !tracks.is_empty())
    })
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

/// Subtitle-sidecar extensions yt-dlp can write for TikTok tracks.
///
/// Bare `json` matters specifically for TikTok: the extractor's `EXT_MAP`
/// maps `creator_caption` tracks to extension `json`, so creator-authored
/// captions — the very tracks this feature exists to capture — land as
/// `{id}.{lang}.json` and would otherwise be neither embedded nor cleaned
/// up. Broadening the match is safe here because nothing else writes
/// `{id}*.json` into the per-video dir: the primary invocation extracts
/// only the WAV and we never pass `--write-info-json`.
const CAPTION_EXTS: &[&str] = &[
    "vtt", "srt", "ass", "lrc", "json", "json3", "srv1", "srv2", "srv3", "ttml",
];

/// Names of files still sitting in `video_dir` under the `{video_id}.`
/// prefix that are neither the WAV we want nor anything the caption pass
/// collected — run after the secondary invocation's collect-and-delete.
///
/// Purely observational: this reports, it never deletes. A caption track
/// arriving with an extension outside [`CAPTION_EXTS`] (a yt-dlp naming
/// change, a new TikTok track type) would otherwise accumulate silently;
/// surfacing it in the logs catches that without a pattern mistake ever
/// being able to remove a media file. Sorted for deterministic log output.
fn uncollected_sidecar_residue(video_dir: &Path, video_id: &str) -> Vec<String> {
    let prefix = format!("{video_id}.");
    let wav = format!("{video_id}.wav");
    let mut residue: Vec<String> = std::fs::read_dir(video_dir)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && *name != wav)
        .collect();
    residue.sort();
    residue
}

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
        .filter_map(std::result::Result::ok)
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

/// Delete every caption sidecar `collect_caption_sidecars` finds for
/// `video_id` in `video_dir`.
///
/// The per-video dir (`ytdlp-{video_id}`) survives failed attempts, so a
/// sidecar written by a killed or aborted earlier attempt would otherwise
/// be collected and embedded by a later attempt as if it were freshly
/// fetched. `acquire` calls this on EVERY attempt, right after
/// `create_dir_all` and before the primary run, so what the envelope
/// embeds can only be what this attempt fetched.
///
/// Best-effort: a failed unlink warns and is otherwise ignored — cleanup
/// must never manufacture a fetch failure.
fn clear_caption_sidecars(video_dir: &Path, video_id: &str) {
    for stale in collect_caption_sidecars(video_dir, video_id) {
        if let Err(e) = std::fs::remove_file(&stale) {
            tracing::warn!(
                file = %stale.display(),
                error = %e,
                "stale caption sidecar removal failed"
            );
        }
    }
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
            Ok(len) if usize::try_from(len).unwrap_or(usize::MAX) <= CAPTION_TRACK_CAP => {
                match std::fs::read_to_string(path) {
                    Ok(content) => {
                        captions.insert(name.to_string(), content);
                    }
                    Err(e) => {
                        tracing::warn!(file = name, error = %e, "caption sidecar unreadable; skipping");
                    }
                }
            }
            Ok(len) => {
                tracing::warn!(
                    file = name,
                    bytes = len,
                    "caption sidecar over cap; skipping"
                );
            }
            Err(e) => {
                tracing::warn!(file = name, error = %e, "caption sidecar stat failed; skipping");
            }
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
        captions: if captions.is_empty() {
            None
        } else {
            Some(captions)
        },
    };
    // Serialization of strings/maps cannot fail in practice; treat an error
    // as "no envelope" per the never-a-new-failure-mode invariant.
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

        // Epic 4c: the per-video dir survives failed attempts, so wipe any
        // caption sidecar an earlier attempt left behind BEFORE this
        // attempt runs — otherwise a later attempt would embed a stale
        // track as if it had just fetched it.
        clear_caption_sidecars(&video_dir, video_id);

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
        // yield metadata.
        //
        // Shallow-peek the printed line first: only when it lists caption
        // tracks (~36% of the corpus, n=11 re-probe 2026-07-28) is the
        // secondary caption invocation worth its network cost. Everything
        // from here to the envelope is best-effort; the primary `outcome`
        // above already decided this video's fate and nothing below can
        // change it.
        //
        // The secondary is additionally gated on a clean primary exit, so a
        // persistently failing video doesn't pay a page re-extraction on
        // every retry. Keying on exit code (not `wav_path.exists()`) is
        // deliberate: a metadata-only success still deserves its captions.
        // Whichever attempt eventually exits 0 captures them, and the
        // envelope's last-write-wins upsert keeps that row.
        let printed = match outcome.stdout.as_deref() {
            Some(bytes) => String::from_utf8_lossy(bytes)
                .trim_end_matches(['\n', '\r'])
                .to_string(),
            None => String::new(),
        };
        let caption_fetch_warranted =
            outcome.exit_code == 0 && printed_lists_caption_tracks(&printed);
        let sidecars = if caption_fetch_warranted {
            self.fetch_caption_sidecars(
                video_id,
                source_url,
                &video_dir,
                opts.cookies_file.as_deref(),
            )
            .await
        } else {
            Vec::new()
        };

        // Sidecars are read + embedded, then deleted so the per-video dir
        // stays clean.
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
        if caption_fetch_warranted {
            let residue = uncollected_sidecar_residue(&video_dir, video_id);
            if !residue.is_empty() {
                tracing::warn!(
                    video_id,
                    files = residue.join(", "),
                    "uncollected caption/sidecar residue"
                );
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
    /// The primary carries the caption flags at ZERO-language selection
    /// (`--sub-langs "-all"`) — a listing, never a download. Without the
    /// flags yt-dlp leaves the printed `subtitles` field empty
    /// (`extract_subtitles` is gated on `writesubtitles`), which would nail
    /// the secondary invocation's gate permanently shut. With `-all`,
    /// `_write_subtitles` early-returns before any track download can fail.
    ///
    /// The asymmetry against `build_caption_fetch_args` (which passes
    /// `--sub-langs all`, download-everything) IS the design: a subtitle
    /// download failure raises `DownloadError`, so downloads must only ever
    /// ride the guarded, best-effort secondary invocation. The epic's
    /// primary invariant — metadata capture never creates a new failure
    /// mode — is enforced by construction.
    #[test]
    fn build_args_lists_captions_without_downloading() {
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

        // Listing flags present — without them the printed `subtitles`
        // field is always empty and the secondary can never be triggered.
        assert!(args.iter().any(|a| a == "--write-subs"));
        assert!(args.iter().any(|a| a == "--write-auto-subs"));
        let langs_idx = args
            .iter()
            .position(|a| a == "--sub-langs")
            .expect("--sub-langs must be present on the primary");
        assert_eq!(
            args.get(langs_idx + 1).map(String::as_str),
            Some("-all"),
            "zero-language selection: `_write_subtitles` early-returns before \
             any track download, so a sub failure can never flip this \
             invocation's exit code"
        );

        // The secondary is the ONLY place a track is actually downloaded —
        // download-everything there, download-nothing here.
        let (caption_args, _) =
            build_caption_fetch_args("abc123", "https://example.com/v", &video_dir, None);
        let caption_langs_idx = caption_args
            .iter()
            .position(|a| a == "--sub-langs")
            .expect("--sub-langs must be present on the secondary");
        assert_eq!(
            caption_args.get(caption_langs_idx + 1).map(String::as_str),
            Some("all"),
            "the -all/all asymmetry is the design: downloads are fatal on \
             failure, so they ride only the guarded secondary invocation"
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
        let env = build_metadata_envelope(Some(&stdout), 64 * 1024, std::slice::from_ref(&vtt))
            .expect("envelope");
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
        // TikTok's `creator_caption` tracks land as bare `.json` (the
        // extractor's EXT_MAP) — the creator-authored captions this feature
        // exists for, so they must be collected, not just `json3`.
        let keep_creator_json = dir.path().join("abc123.en.json");
        let wrong_prefix = dir.path().join("other.en.vtt");
        let wrong_ext = dir.path().join("abc123.wav");
        for p in [&keep, &keep_creator_json, &wrong_prefix, &wrong_ext] {
            std::fs::write(p, "x").unwrap();
        }
        let found = collect_caption_sidecars(dir.path(), "abc123");
        // Sorted for determinism: ".en.json" < ".en.vtt".
        assert_eq!(found, vec![keep_creator_json, keep]);
    }

    /// The secondary invocation downloads no media and writes sidecars
    /// under the same `-o` template the primary uses.
    #[test]
    fn build_caption_fetch_args_skips_download_and_writes_subs() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let (args, redact) =
            build_caption_fetch_args("abc123", "https://example.com/v", &video_dir, None);
        assert!(args.iter().any(|a| a == "--skip-download"));
        assert!(args.iter().any(|a| a == "--write-subs"));
        assert!(args.iter().any(|a| a == "--write-auto-subs"));
        // Without an explicit selection yt-dlp takes one preferred language
        // (English-biased) and a multi-track video loses the rest.
        let langs_idx = args
            .iter()
            .position(|a| a == "--sub-langs")
            .expect("--sub-langs must be present or only one language is fetched");
        assert_eq!(args.get(langs_idx + 1).map(String::as_str), Some("all"));
        let o_idx = args
            .iter()
            .position(|a| a == "-o")
            .expect("-o flag must be present");
        assert_eq!(
            args.get(o_idx + 1).map(String::as_str),
            Some("/tmp/test-dir/abc123.%(ext)s"),
            "sidecars must land where collect_caption_sidecars looks"
        );
        assert!(
            !args.iter().any(|a| a == "-x" || a == "-f"),
            "caption run must not select or extract media"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("https://example.com/v")
        );
        assert!(redact.is_empty());
    }

    /// ADR 0035: the secondary invocation mirrors the primary's cookie
    /// handling exactly — same path, same dynamically computed redact index.
    #[test]
    fn build_caption_fetch_args_mirrors_primary_cookie_redaction() {
        let video_dir = PathBuf::from("/tmp/test-dir");
        let cookie = PathBuf::from("/secret/tiktok-cookies.txt");
        let (args, redact) =
            build_caption_fetch_args("abc123", "https://example.com/v", &video_dir, Some(&cookie));
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

    /// The shallow peek gates the secondary invocation. Anything short of a
    /// non-empty track object means "no captions exist" ⇒ no extra request.
    #[test]
    fn printed_lists_caption_tracks_false_when_no_tracks() {
        // Keys absent entirely.
        assert!(!printed_lists_caption_tracks(r#"{"id":"abc","title":"t"}"#));
        // Explicit nulls — yt-dlp's shape when the extractor found nothing.
        assert!(!printed_lists_caption_tracks(
            r#"{"subtitles":null,"automatic_captions":null}"#
        ));
        // Present but empty objects.
        assert!(!printed_lists_caption_tracks(
            r#"{"subtitles":{},"automatic_captions":{}}"#
        ));
    }

    #[test]
    fn printed_lists_caption_tracks_true_under_either_key() {
        assert!(printed_lists_caption_tracks(
            r#"{"subtitles":{"en":[{"ext":"vtt"}]},"automatic_captions":{}}"#
        ));
        assert!(printed_lists_caption_tracks(
            r#"{"subtitles":null,"automatic_captions":{"en":[{"ext":"vtt"}]}}"#
        ));
    }

    /// Non-JSON input (a yt-dlp error line, a truncated print, garbage) must
    /// not panic and must not provoke a secondary request.
    #[test]
    fn printed_lists_caption_tracks_false_on_non_json() {
        assert!(!printed_lists_caption_tracks(
            "ERROR: unable to extract info"
        ));
        assert!(!printed_lists_caption_tracks(""));
        assert!(!printed_lists_caption_tracks(r#"{"subtitles":"#));
    }

    /// Stale-sidecar guard: the per-video dir survives failed attempts, so
    /// a caption file an aborted earlier attempt wrote must be gone before
    /// the next attempt can embed it as if fresh. Non-caption artifacts in
    /// the same dir are untouched.
    #[test]
    fn clear_caption_sidecars_removes_stale_and_spares_other_files() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("abc123.en.vtt");
        let wav = dir.path().join("abc123.wav");
        std::fs::write(&stale, "WEBVTT\n\nstale\n").unwrap();
        std::fs::write(&wav, "RIFF").unwrap();

        clear_caption_sidecars(dir.path(), "abc123");

        assert!(!stale.exists(), "stale caption sidecar must be removed");
        assert!(wav.exists(), "non-caption artifacts must be untouched");
    }

    /// Residue hygiene: after the caption pass collects and deletes what it
    /// recognises, anything left under the `{video_id}.` prefix is reported
    /// (never deleted) so an unknown-extension track surfaces in the logs
    /// instead of accumulating. The WAV we actually want is not residue.
    #[test]
    fn uncollected_sidecar_residue_reports_leftovers_but_not_the_wav() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "abc123.wav",
            "abc123.en.unknownsub",
            "abc123.part",
            "other.en.vtt",
        ] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        let residue = uncollected_sidecar_residue(dir.path(), "abc123");
        assert_eq!(
            residue,
            vec![
                "abc123.en.unknownsub".to_string(),
                "abc123.part".to_string()
            ],
            "the WAV and other videos' files are not residue; output is sorted"
        );
    }

    #[test]
    fn uncollected_sidecar_residue_empty_on_a_clean_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("abc123.wav"), "RIFF").unwrap();
        assert!(uncollected_sidecar_residue(dir.path(), "abc123").is_empty());
    }

    /// The per-track cap is per-TRACK, not per-set: an oversize file is
    /// skipped while a small sibling is still embedded. A truncated subtitle
    /// file is corrupt rather than useful, so skipping is the right call.
    #[test]
    fn build_envelope_skips_oversize_caption_track_per_track() {
        let dir = tempfile::tempdir().unwrap();
        let stdout = b"{\"id\": \"abc123\"}".to_vec();

        let huge = dir.path().join("abc123.en.vtt");
        std::fs::write(&huge, "x".repeat(CAPTION_TRACK_CAP + 1)).unwrap();

        // Oversize track alone ⇒ nothing embedded at all.
        let env = build_metadata_envelope(Some(&stdout), 64 * 1024, std::slice::from_ref(&huge))
            .expect("envelope");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert!(
            v["captions"].is_null(),
            "an over-cap track is skipped, leaving no captions map"
        );

        // A small sibling alongside it is still embedded.
        let small = dir.path().join("abc123.nl.vtt");
        std::fs::write(&small, "WEBVTT\n\n00:01.000 --> 00:02.000\nParis\n").unwrap();
        let env =
            build_metadata_envelope(Some(&stdout), 64 * 1024, &[huge, small]).expect("envelope");
        let v: serde_json::Value = serde_json::from_str(&env).unwrap();
        assert!(
            v["captions"]["abc123.en.vtt"].is_null(),
            "over-cap track stays skipped when a sibling is present"
        );
        assert!(v["captions"]["abc123.nl.vtt"]
            .as_str()
            .unwrap()
            .contains("Paris"));
    }
}
