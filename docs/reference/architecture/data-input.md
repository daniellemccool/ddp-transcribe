# uu-tiktok — data input

The data-input subsystem covers two stages of the donor's journey: ingest (parsing the TikTok DDP export into rows the state machine can claim) and fetch (downloading the watched-video MP4 and extracting audio for transcription).

## Ingest

The ingest stage reads a donor's TikTok DDP export from a local inbox directory, parses the JSON, and upserts each identifiable watched-video entry into the state machine's `videos` and `watch_history` tables. The entry point is `pub fn ingest(inbox: &Path, store: &mut Store)` in `src/ingest.rs:30`.

### DDP export shape

The inbox is a directory tree. `ingest` walks it recursively, collecting every `*.json` file (`src/ingest.rs:117–138`). A single JSON file corresponds to one participant's export.

Respondent identity is derived from the filename, not from the file contents. The expected filename convention is:

```
assignment={N}_task={N}_participant={ID}_source=tiktok_key={N}-tiktok.json
```

The `participant=` segment is extracted and used as the `respondent_id` for every row produced from that file (`src/ingest.rs:143–159`). If the segment is absent the file-level parse aborts with an error.

Each JSON file is an array of section objects. The parser deserialises eagerly (not streaming) via serde_json into `Vec<Section>` (`src/ingest.rs:39`). Only sections whose key is `tiktok_watch_history` are consumed; unknown keys are ignored by serde's default field matching. Each entry in that array has two string fields, capitalised as TikTok exports them:

| Field  | Meaning                                      |
|--------|----------------------------------------------|
| `Date` | Watch timestamp, e.g. `2024-01-01 12:00:00 UTC` |
| `Link` | Raw URL from the DDP (canonical or short)    |

(`src/ingest.rs:167–173`)

### Parsing strategy

**File-level failures abort.** If a file's JSON is malformed or the filename lacks a `participant=` segment, ingest propagates the error and stops (`src/ingest.rs:36–40, 155`).

**Entry-level problems skip with a structured warn log and a counter increment.** There are three skip categories:

- *Short link* (`vm.tiktok.com/…` or `tiktok.com/t/…`) — cannot extract a video ID without following a redirect; logged and counted as `short_links_skipped` (`src/ingest.rs:73–80`).
- *Invalid URL* — not a recognisable TikTok URL; logged and counted as `invalid_urls_skipped` (`src/ingest.rs:81–91`).
- *Unparseable date* — two date formats are tried (`%Y-%m-%d %H:%M:%S` and `%Y-%m-%d %H:%M:%S UTC`); failure logs and increments `date_parse_failures` (`src/ingest.rs:93–104, 175–186`).

**URL canonicalization** is applied to every entry before the URL is stored (`src/ingest.rs:70`). `src/canonical.rs:35` classifies each URL into one of three `Canonical` variants — `VideoId(String)`, `NeedsResolution(String)`, or `Invalid(String)` — extracting the 19-digit numeric video ID from canonical-form URLs.

**Deduplication** is enforced by `INSERT OR IGNORE` at the database level. Both `upsert_video` and `upsert_watch_history` use `INSERT OR IGNORE`, so duplicate entries in the export (same `video_id` or same `(respondent_id, video_id)` pair) produce no second row; the return value of 0 rows-changed is counted as `watch_history_duplicates` (`src/ingest.rs:109–113`).

### What becomes a row in state

A successfully processed entry produces one row in each of two tables:

**`videos`** — one row per distinct `video_id`, written by `store.upsert_video(video_id, source_url, canonical=true)` (`src/ingest.rs:107`). The row is inserted with `status = 'pending'` (literal in the SQL, `src/state/mod.rs:172`). `attempt_count` is not set by the ingest INSERT; it uses the schema default. `first_seen_at` and `updated_at` are set to `unix_now()`.

**`watch_history`** — one row per `(respondent_id, video_id, watched_at)`, written by `store.upsert_watch_history(...)` (`src/ingest.rs:109`). Stores the respondent identity alongside the watch timestamp (as a Unix epoch i64) and the `in_window` flag.

Both tables use `INSERT OR IGNORE` (`src/state/mod.rs:169, 189`), so re-running ingest against the same export is safe. For the full lifecycle of a `videos` row after ingest, see [state-machine.md](state-machine.md).

---

## Fetcher

The fetcher downloads each claimed video using `yt-dlp` as a subprocess. The orchestrator's fetch workers each invoke the fetcher once per claim; the fetcher returns the local path of the downloaded audio file (or an error), and the file is passed downstream for transcription. Subprocess output is bounded per [ADR 0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md) — output streams are drained fully to prevent child processes from blocking, with stderr retaining the trailing 8 KiB and stdout discarded.

### Subprocess wrapping pattern

The fetcher is `src/fetcher/ytdlp.rs`, which implements the `VideoFetcher` trait defined in `src/fetcher/mod.rs:16`. Its `acquire` method calls `src/process::run(CommandSpec { … })` (`src/fetcher/ytdlp.rs:98`), the bounded-output subprocess runner per ADR 0021.

`process::run` uses `tokio::process::Command` (`src/process.rs:157`) — the async variant, matching the fetch worker's async context. No working directory is set on the command; it inherits the process's cwd. No environment variables are manipulated.

The path to the downloaded artifact is not parsed from yt-dlp's stdout. Instead, the fetcher constructs a predictable output path from the video ID before invocation (e.g. `{video_dir}/{video_id}.wav`) via the `-o` template (`src/fetcher/ytdlp.rs:43–44`), then checks for the file's existence after yt-dlp exits (`src/fetcher/ytdlp.rs:116–121`). If the file is absent despite a zero exit code, that is surfaced as a `FetchError::ParseError`.

Per-video work is isolated in a subdirectory `ytdlp-{video_id}` under the configured `work_dir` (`src/fetcher/ytdlp.rs:87`), keeping yt-dlp's intermediate files contained.

### yt-dlp invocation: flags and rationale

The flag list is built in the pure function `build_yt_dlp_args` (`src/fetcher/ytdlp.rs:42`). Every flag the code passes is listed here; the illustrative flags in the task plan do not all appear in the code.

- `--no-playlist` — prevents yt-dlp from expanding a single URL into a playlist. TikTok URLs sometimes resolve to a creator feed; this ensures we fetch only the specific video.

- `--no-warnings` and `--quiet` — suppress yt-dlp's informational output. yt-dlp writes audio to a file; stdout is not used for the output artifact, and noise in stderr would crowd out real error messages.

- `-f download/b[vcodec=h264]/b` — format selector with two fallbacks. `download` is TikTok's pre-rendered share-link MP4 (h264 at ~540p, pre-muxed, served as a static asset). This is preferred over the `bitrateInfo` ABR variants, which intermittently mux h265-video-only files despite being tagged `acodec=aac` by yt-dlp's extractor (yt-dlp issues #15891/#16622). The fallback `b[vcodec=h264]` handles videos where the `download` format is absent (creator-disabled downloads); `b` is the last-resort.

- `-S +size,+br,+res,+fps` — within-selector sort order. Has no effect when `download` matches (it is a literal format ID); sorts within the `b[vcodec=h264]/b` fallback to prefer the smallest viable stream, providing defence against unexpectedly large h264 variants.

- `-x` — extract audio only; instructs yt-dlp to run its audio-extraction post-processor and discard the video container.

- `--audio-format wav` — requests WAV as the output container for the extracted audio.

- `--postprocessor-args "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1"` — passes explicit ffmpeg flags to the audio-extraction post-processor. `-vn -sn -dn` drop video, subtitle, and data streams; `-map 0:a:0` selects only the first audio stream; `-c:a pcm_s16le` pins the WAV codec; `-ar 16000 -ac 1` enforces the 16 kHz mono invariant required by whisper.cpp (see [ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16khz-mono-via-hound.md)). The `-vn` and codec flags are redundant with current yt-dlp/ffmpeg defaults but are kept explicit as defence against future default changes (`src/fetcher/ytdlp.rs:65–75`).

- `-o {video_dir}/{video_id}.%(ext)s` — output template placing the file at the predictable path the fetcher checks for after yt-dlp exits.

(`src/fetcher/ytdlp.rs:45–79`)

### Output capture

`process::run` pipes both stdout and stderr of the yt-dlp child process (`src/process.rs:159–161`). Both streams are drained via `read_bounded` (`src/process.rs:97`) — a streaming reader backed by a `VecDeque<u8>` that drops the leading byte when full, retaining only the trailing `cap` bytes.

The fetcher configures these caps asymmetrically (`src/fetcher/ytdlp.rs:102–103`):

- **stderr**: `stderr_capture_bytes: 8 * 1024` — the last 8 KiB is retained and surfaced in `CommandOutcome.stderr_excerpt`. This is what appears in `FetchError::ToolFailed.stderr_excerpt` on failure.
- **stdout**: `stdout_capture_bytes: 0` — stdout is drained to prevent the child blocking on a full pipe, but no bytes are retained (`CommandOutcome.stdout == None`). yt-dlp writes audio to a file, not stdout, so the discard is intentional.

Both streams are still drained concurrently via `tokio::try_join!` (`src/process.rs:178`) so neither can block. The asymmetry in *retention* is specific to the fetcher's call site; the `run` helper itself is symmetric-capable. The *why* for bounded capture is covered by [ADR 0021](../../decisions/0021-bounded-subprocess-output-capture-via-streaming-vecdeque-u8.md).

### Timeout policy

The fetcher applies an explicit per-invocation wall-clock timeout. The default is **300 seconds** (5 minutes), set in `src/config.rs:47` (dev profile) and passed through `src/main.rs:108` to `YtDlpFetcher::new`. The timeout is stored on the `YtDlpFetcher` struct and forwarded to each `CommandSpec` (`src/fetcher/ytdlp.rs:14, 101`).

`process::run` wraps the full read-and-wait future in `tokio::time::timeout` (`src/process.rs:184`). On expiry, it calls `child.start_kill()` (immediate SIGKILL) and returns `RunError::Timeout` (`src/process.rs:220–224`). A `kill_on_drop(true)` flag set at spawn (`src/process.rs:163`) provides a backstop in case control flow changes; the two kills are intentionally redundant.

`RunError::Timeout` maps to `FetchError::ToolTimeout` (`src/process.rs:77`), which the fetch worker treats as retryable (see Retry classification below).

### Retry classification

After a yt-dlp invocation, the fetcher distinguishes two exit paths from `process::run`:

1. **Process-level error** (`RunError` → `FetchError` via `From`) — `ToolTimeout`, `NetworkError` (spawn failure or pipe I/O error). These never reach the exit-code check.
2. **Non-zero exit code** — mapped to `FetchError::ToolFailed { tool, exit_code, stderr_excerpt }` (`src/fetcher/ytdlp.rs:109–113`).

`FetchError` also has a `ParseError` variant for the case where yt-dlp exits zero but the expected WAV file is absent (`src/fetcher/ytdlp.rs:117–121`).

**Current state (post-Epic 2):** the fetch worker does not branch on these variants. Every `Err(e)` from `fetcher.acquire` collapses to `format!("{e:#}")` and is unconditionally passed to `mark_retryable_failure` with the literal placeholder kind `"Fetch"` (`src/pipeline/pipelined.rs:207`). There is no call to `mark_terminal_failure` in the fetch worker path. `mark_retryable_failure`'s `kind` parameter is typed `&str` per [ADR 0023](../../decisions/0023-minimum-mutator-signatures-kind-str-message-str-returning-result-usize-per-0006.md); a richer typed taxonomy (`RetryableKind`, `UnavailableReason`, etc.) and variant-driven routing are deferred to Epic 3. The in-code forward-pointers are at `src/pipeline/pipelined.rs:205–206` and `src/process.rs:70–73`.

### Audio extraction handoff

The fetcher does not extract audio itself via a separate subprocess. Audio extraction is delegated to yt-dlp's own ffmpeg post-processor through the `-x --audio-format wav --postprocessor-args` flags described above. By the time `YtDlpFetcher::acquire` returns, the artifact on disk is already a WAV file.

The `Acquisition::AudioFile(PathBuf)` returned by `acquire` (`src/fetcher/mod.rs:12`) carries the path to this WAV. The transcribe worker passes it to `src/audio::decode_wav` (`src/audio.rs:43`), which validates the format (16 kHz, mono) and decodes the PCM samples to `Vec<f32>`. The format contract — 16 kHz mono float32 in `[-1.0, 1.0]` — is documented in [ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16khz-mono-via-hound.md); the conversion is the `/32768.0` normalisation at `src/audio.rs:74`. For what happens next, see [transcription.md](transcription.md).

---

## ADRs governing this subsystem

| ADR  | Title                                               | Where it applies                                                                        |
|------|-----------------------------------------------------|-----------------------------------------------------------------------------------------|
| 0014 | Audio input invariant float32 PCM 16kHz mono via hound | Audio prep boundary: yt-dlp postprocessor enforces the format; `src/audio.rs` validates it. |
| 0021 | Bounded subprocess output capture via streaming VecDeque\<u8\> | Fetcher's yt-dlp invocation — both streams drained; stderr retains trailing 8 KiB. |
| 0023 | Minimum mutator signatures (kind: &str, message: &str) returning Result\<usize\> per 0006 | Retry classification surface — the `kind` parameter `mark_retryable_failure` receives from the fetch worker. |
