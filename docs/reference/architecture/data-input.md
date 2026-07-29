# ddp-transcribe — data input

The data-input subsystem covers two stages of the donor's journey: ingest (parsing the TikTok DDP export into rows the state machine can claim) and fetch (downloading the watched-video MP4, extracting audio for transcription, and capturing the video's metadata envelope). A post-run loader (`load-metadata`) turns the captured envelopes into typed columns; it is described at the end of the fetcher section.

## Ingest

The ingest stage reads a donor's TikTok DDP export from a local inbox directory, parses the JSON, and upserts each identifiable watched-video entry into the state machine's `videos` and `watch_history` tables. The entry point is `pub fn ingest(inbox: &Path, store: &mut Store)` in `src/ingest.rs:30`.

### DDP export shape

The inbox is a directory tree. `ingest` walks it recursively, collecting every `*.json` file (`src/ingest.rs:117–138`). A single JSON file corresponds to one participant's export.

Respondent identity is derived from the filename, not from the file contents. The expected filename convention is:

```
assignment={N}_task={N}_participant={ID}_source=tiktok_key={N}-tiktok.json
```

The `participant=` segment is extracted and used as the `respondent_id` for every row produced from that file. If the segment is absent the file is skipped and counted (see *Parsing strategy* below).

Each JSON file is an array of section objects. The parser deserialises eagerly (not streaming) via serde_json into `Vec<Section>` (`src/ingest.rs:39`). Only sections whose key is `tiktok_watch_history` are consumed; unknown keys are ignored by serde's default field matching. Each entry in that array has two string fields, capitalised as TikTok exports them:

| Field  | Meaning                                      |
|--------|----------------------------------------------|
| `Date` | Watch timestamp, e.g. `2024-01-01 12:00:00 UTC` |
| `Link` | Raw URL from the DDP (canonical or short)    |

(`src/ingest.rs:167–173`)

### Parsing strategy

**File-level failures skip and count; they never abort the run.** A filename with no `participant=` segment, a file that cannot be stat'd or read, and JSON that is not a `Vec<Section>` all produce one `warn!` carrying the underlying cause, one `files_skipped_unparseable` increment, and a move to the next file. This was a July-2026 production fix: the donation platform writes decline stubs (`{"status":"data_submission declined"}` — a top-level object) into the same inbox, and one of them used to veto a 142-file run at `serde_json::from_slice`. A file that parses but carries no watch history is *processed* (zero rows), not skipped — the three file-level counters are parallel per [ADR 0007](../../decisions/0007-stats-structs-count-the-input-side-with-verb-named-parallel-counters.md), so each walked file increments exactly one of `files_processed`, `files_skipped_unparseable`, `files_skipped_already_ingested`.

**The ingest ledger (`ingested_files`, schema v6) skips unchanged files before they are read.** Before opening a file, ingest stats it and looks up its basename in the ledger; an exact `(file_name, size_bytes, mtime)` match logs at *debug* (this is the normal fast path, not a problem), increments `files_skipped_already_ingested`, and moves on. The ledger row is written inside the same transaction that commits that file's rows, so a row exists iff that file's data is committed — a crash mid-file leaves no ledger row and the next run reprocesses it. Changed files reprocess, with the row-level `INSERT OR IGNORE` upserts as the correctness backstop. A file with no representable unix mtime (or a non-UTF-8 basename) is treated as unmatchable: processed every run, no ledger row — redundant work, never skipped data. The migration deliberately leaves the ledger empty on an existing DB, so the first post-migration run pays one full walk.

**Entry-level problems skip with a structured warn log and a counter increment.** There are three skip categories:

- *Short link* (`vm.tiktok.com/…` or `tiktok.com/t/…`) — cannot extract a video ID without following a redirect; logged and counted as `short_links_skipped` (`src/ingest.rs:73–80`).
- *Invalid URL* — not a recognisable TikTok URL; logged and counted as `invalid_urls_skipped` (`src/ingest.rs:81–91`).
- *Unparseable date* — two date formats are tried (`%Y-%m-%d %H:%M:%S` and `%Y-%m-%d %H:%M:%S UTC`); failure logs and increments `date_parse_failures` (`src/ingest.rs:93–104, 175–186`). Both formats are interpreted as UTC — per [ADR 0039](../../decisions/0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md), a documentary-evidence verdict that remains **empirically unresolved** (an operator spot-check couldn't discriminate UTC from local time at ±1h). The verbatim `Date` string is preserved alongside the parsed value in `watch_history.watched_at_raw` (schema v4, Epic 4b) as the hedge against that unresolved verdict; re-ingest backfills a NULL `watched_at_raw` on an existing row but never re-parses or overwrites a non-NULL one.

**URL canonicalization** is applied to every entry before the URL is stored (`src/ingest.rs:70`). `src/canonical.rs:35` classifies each URL into one of three `Canonical` variants — `VideoId(String)`, `NeedsResolution(String)`, or `Invalid(String)` — extracting the 19-digit numeric video ID from canonical-form URLs.

**Deduplication** is enforced by `INSERT OR IGNORE` at the database level. Both `upsert_video` and `upsert_watch_history` use `INSERT OR IGNORE`, so duplicate entries in the export (same `video_id`, or same `(respondent_id, video_id)` pair) produce no second row. The `watch_history_duplicates` counter is incremented specifically from the watch-history upsert's 0-rows-changed return (`src/ingest.rs:109–113`); the `upsert_video` return value is not tracked.

### What becomes a row in state

A successfully processed entry produces one row in each of two tables:

**`videos`** — one row per distinct `video_id`, written by `store.upsert_video(video_id, source_url, canonical=true)` (`src/ingest.rs:107`). The row is inserted with `status = 'pending'` (literal in the SQL, `src/state/mod.rs:172`). `attempt_count` is not set by the ingest INSERT; it uses the schema default. `first_seen_at` and `updated_at` are set to `unix_now()`.

**`watch_history`** — one row per `(respondent_id, video_id, watched_at)`, written by `store.upsert_watch_history(...)` (`src/ingest.rs:109`). Stores the respondent identity alongside the watch timestamp (as a Unix epoch i64), the verbatim raw date string (`watched_at_raw`), and the `in_window` flag.

Both tables use `INSERT OR IGNORE` (`src/state/mod.rs:169, 189`), so re-running ingest against the same export is safe. For the full lifecycle of a `videos` row after ingest, see [state-machine.md](state-machine.md).

### Analysis window (`--window-start` / `--window-end`)

`ingest` optionally accepts `--window-start`/`--window-end` (`YYYY-MM-DD`, UTC calendar dates; Epic 4b, [ADR 0040](../../decisions/0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md)). Bounds are inclusive: `--window-start` maps to that date's `00:00:00Z`; `--window-end` covers its whole day (the CLI derives an exclusive upper bound at the *following* day's `00:00:00Z` — `WindowBounds::from_dates`, `src/ingest.rs`). Either side may be absent (unbounded on that side); both absent means every row ingests `in_window = 1` — the pre-4b behavior. `cli::validate_window_order` rejects `--window-start` after `--window-end` before the store even opens (equal dates are a valid single-day window).

`in_window` is computed **once**, at ingest time, from these bounds and stored on the `watch_history` row (`WindowBounds::contains`, `src/ingest.rs`). It is never re-derived at query time and never changed by a later `ingest` run (duplicate-PK rows are computed but not rewritten — `IngestStats.computed_out_of_window` counts the input-side computation per ADR-0007, not the write). The **only** way to change `in_window` after ingest is the explicit `recompute-window` subcommand (`Store::recompute_window`, `src/state/mod.rs`), which refuses to run with no window flags at all, supports `--clear` (explicit no-filter opt-in — sets `in_window = 1` for every row) and `--dry-run` (reports the row count that would change via `Store::count_window_mismatches`, without writing). Day-granularity windows are deliberate: they absorb the sub-day ambiguity that [ADR 0039](../../decisions/0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md) leaves unresolved for all but boundary-adjacent rows — only rows within the ambiguity offset (~1h) of a window edge can be misclassified, and the count of such rows is bounded by the offset.

---

## Fetcher

The fetcher downloads each claimed video using `yt-dlp` as a subprocess. The orchestrator's fetch workers each invoke the fetcher once per claim; the fetcher returns a `(Option<MetadataCapture>, Result<Acquisition, FetchError>)` pair — the raw metadata envelope alongside the local path of the downloaded audio file (or an error) — and the audio file is passed downstream for transcription. Subprocess output is bounded per [ADR 0021](../../decisions/0021-subprocess-output-capture-is-bounded-by-construction.md) — output streams are drained fully to prevent child processes from blocking, with stderr retaining the trailing 8 KiB and stdout retaining the trailing 64 KiB (Epic 4c; stdout retention was 0 before the metadata capture landed).

### Subprocess wrapping pattern

The fetcher is `src/fetcher/ytdlp.rs`, which implements the `VideoFetcher` trait defined in `src/fetcher/mod.rs:16`. Its `acquire` method calls `src/process::run(CommandSpec { … })` (`src/fetcher/ytdlp.rs:98`), the bounded-output subprocess runner per ADR 0021.

`process::run` uses `tokio::process::Command` (`src/process.rs:157`) — the async variant, matching the fetch worker's async context. No working directory is set on the command; it inherits the process's cwd. No environment variables are manipulated.

The path to the downloaded artifact is not parsed from yt-dlp's stdout. Instead, the fetcher constructs a predictable output path from the video ID before invocation (e.g. `{video_dir}/{video_id}.wav`) via the `-o` template (`src/fetcher/ytdlp.rs:43–44`), then checks for the file's existence after yt-dlp exits (`src/fetcher/ytdlp.rs:116–121`). If the file is absent despite a zero exit code, that is surfaced as a `FetchError::ParseError`.

Per-video work is isolated in a subdirectory `ytdlp-{video_id}` under the configured `work_dir` (`src/fetcher/ytdlp.rs:87`), keeping yt-dlp's intermediate files contained.

### yt-dlp invocation: flags and rationale

The flag list is built in the pure function `build_yt_dlp_args` (`src/fetcher/ytdlp.rs:42`). Every flag the code passes is listed here; the illustrative flags in the task plan do not all appear in the code.

- `--no-playlist` — prevents yt-dlp from expanding a single URL into a playlist. TikTok URLs sometimes resolve to a creator feed; this ensures we fetch only the specific video.

- `--no-warnings` and `--quiet` — suppress yt-dlp's informational output. yt-dlp writes audio to a file; stdout carries only the metadata print line (below), and noise in stderr would crowd out real error messages.

- `--no-simulate --print "%(.{id,title,description,uploader,uploader_id,channel_id,timestamp,duration,view_count,like_count,comment_count,repost_count})j"` — the Epic 4c metadata capture ([ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)). `--print` implies `--simulate` unless `--no-simulate` is passed, so both flags are required for the download to still happen. The template is a field-limited dict print of the info dict yt-dlp has already extracted for the download — one line of JSON (~615 B measured live 2026-07-28), with the bulky `formats`/`thumbnails` arrays deliberately excluded. It costs **zero extra network requests and no second invocation**. The template is stored as `METADATA_PRINT_TEMPLATE` (`src/fetcher/ytdlp.rs`); a unit test asserts it names neither `subtitles` nor `automatic_captions` — caption/subtitle collection is a deliberate non-goal (see below).

- `-f download/b[vcodec=h264]/b` — format selector with two fallbacks. `download` is TikTok's pre-rendered share-link MP4 (h264 at ~540p, pre-muxed, served as a static asset). This is preferred over the `bitrateInfo` ABR variants, which intermittently mux h265-video-only files despite being tagged `acodec=aac` by yt-dlp's extractor (yt-dlp issues #15891/#16622). The fallback `b[vcodec=h264]` handles videos where the `download` format is absent (creator-disabled downloads); `b` is the last-resort.

- `-S +size,+br,+res,+fps` — within-selector sort order. Has no effect when `download` matches (it is a literal format ID); sorts within the `b[vcodec=h264]/b` fallback to prefer the smallest viable stream, providing defence against unexpectedly large h264 variants.

- `-x` — extract audio only; instructs yt-dlp to run its audio-extraction post-processor and discard the video container.

- `--audio-format wav` — requests WAV as the output container for the extracted audio.

- `--postprocessor-args "ffmpeg:-vn -sn -dn -map 0:a:0 -c:a pcm_s16le -ar 16000 -ac 1"` — passes explicit ffmpeg flags to the audio-extraction post-processor. `-vn -sn -dn` drop video, subtitle, and data streams; `-map 0:a:0` selects only the first audio stream; `-c:a pcm_s16le` pins the WAV codec; `-ar 16000 -ac 1` enforces the 16 kHz mono invariant required by whisper.cpp (see [ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16-khz-mono-validated-at-decode.md)). The `-vn` and codec flags are redundant with current yt-dlp/ffmpeg defaults but are kept explicit as defence against future default changes (`src/fetcher/ytdlp.rs:65–75`).

- `-o {video_dir}/{video_id}.%(ext)s` — output template placing the file at the predictable path the fetcher checks for after yt-dlp exits.

(`src/fetcher/ytdlp.rs:45–79`)

### Output capture

`process::run` pipes both stdout and stderr of the yt-dlp child process (`src/process.rs:159–161`). Both streams are drained via `read_bounded` (`src/process.rs:97`) — a streaming reader backed by a `VecDeque<u8>` that drops the leading byte when full, retaining only the trailing `cap` bytes.

The fetcher configures these caps asymmetrically (`src/fetcher/ytdlp.rs:102–103`):

- **stderr**: `stderr_capture_bytes: 8 * 1024` — the last 8 KiB is retained and surfaced in `CommandOutcome.stderr_excerpt`. This is what appears in `FetchError::ToolFailed.stderr_excerpt` on failure.
- **stdout**: `stdout_capture_bytes: 64 * 1024` (the `STDOUT_CAP` constant, `src/fetcher/ytdlp.rs`) — the `--print` line is the only thing yt-dlp writes there under `--quiet`, and 64 KiB is ~100× the measured line length. The cap doubles as a corruption detector: `read_bounded` keeps the *last* `cap` bytes, so a buffer filled to the cap means the head was dropped and the line is unparseable — `build_metadata_envelope` returns `None` in that case rather than storing a truncated blob.

Both streams are still drained concurrently via `tokio::try_join!` (`src/process.rs:178`) so neither can block. The asymmetry in *retention* is specific to the fetcher's call site; the `run` helper itself is symmetric-capable. The *why* for bounded capture is covered by [ADR 0021](../../decisions/0021-subprocess-output-capture-is-bounded-by-construction.md).

### Timeout policy

The fetcher applies an explicit per-invocation wall-clock timeout. The default is **300 seconds** (5 minutes), set in `src/config.rs:47` (dev profile) and passed through `src/main.rs:108` to `YtDlpFetcher::new`. The timeout is stored on the `YtDlpFetcher` struct and forwarded to each `CommandSpec` (`src/fetcher/ytdlp.rs:14, 101`).

`process::run` wraps the full read-and-wait future in `tokio::time::timeout` (`src/process.rs:184`). On expiry, it calls `child.start_kill()` (immediate SIGKILL) and returns `RunError::Timeout` (`src/process.rs:220–224`). A `kill_on_drop(true)` flag set at spawn (`src/process.rs:163`) provides a backstop in case control flow changes; the two kills are intentionally redundant.

`RunError::Timeout` maps to `FetchError::ToolTimeout` (`src/process.rs:77`), which the fetch worker treats as retryable (see Retry classification below).

### Retry classification

After a yt-dlp invocation, the fetcher distinguishes two exit paths from `process::run`:

1. **Process-level error** (`RunError` → `FetchError` via `From`) — `ToolTimeout`, `ToolNotFound` (spawn failure), `SystemIo` (pipe I/O error). These never reach the exit-code check.
2. **Non-zero exit code** — mapped to `FetchError::ToolFailed { tool, exit_code, signal, stderr_excerpt }` (`src/fetcher/ytdlp.rs:106–111`); `signal` carries the Unix kill signal when the child did not exit normally.

`FetchError` also has `WorkDirCreate` (filesystem failure creating the per-video work dir) and `MissingOutput` (yt-dlp exits zero but the expected WAV file is absent) variants (`src/fetcher/ytdlp.rs`).

**Current state (post-Epic 4a):** every `FetchError` (wrapped in the pipeline's `FetchPhaseError`) runs through `classify_fetch_phase`, whose message classification is driven by the active **classification table** (`src/classification.rs`, [ADR 0037](../../decisions/0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)): ordered first-match rules mapping yt-dlp stderr substrings to a **label string** (e.g. `NoDataBlocks`, `SensitiveLoginGated`) plus a disposition (`retryable` / `terminal` / `requires-cookie`); structural errors (timeout, spawn failure, missing output) stay code-mapped in `src/failure.rs`. The three-arm dispatch per [ADR 0033](../../decisions/0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md) + [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md): retryable and requires-cookie labels go to `record_fetch_failure`, which decides requeue-vs-exhaust-vs-park in one transaction at failure time (requeued rows re-enter `pending` at the end of the queue; the re-fetch itself adjudicates liveness — there is no probe); terminal-dispositioned labels (the proven-dead message classes) go to `mark_terminal_failure` inline; Bug-class errors (e.g. tool missing) abort the run. Rows already parked in `failed_retryable` are re-adjudicated by the start-of-batch sweep through the same table (a fallback hit preserves a real stored kind — see [`state-machine.md`](state-machine.md) §Failure classification). Labels ride the same `&str` mutator parameters per [ADR 0023](../../decisions/0023-failure-mutators-take-string-kinds-and-keep-the-claim-guard.md) — no schema change. See [`orchestration.md`](orchestration.md) §Failure handling and [`state-machine.md`](state-machine.md) §Failure classification.

### Audio extraction handoff

The fetcher does not extract audio itself via a separate subprocess. Audio extraction is delegated to yt-dlp's own ffmpeg post-processor through the `-x --audio-format wav --postprocessor-args` flags described above. By the time `YtDlpFetcher::acquire` returns, the artifact on disk is already a WAV file.

The `Acquisition::AudioFile(PathBuf)` returned by `acquire` (`src/fetcher/mod.rs`) carries the path to this WAV. The **fetch** worker decodes it via `src/audio::decode_wav` (`src/audio.rs:43`, called inside `fetch_and_decode` at `src/pipeline/mod.rs:158`), which validates the format (16 kHz, mono) and decodes the PCM samples to `Vec<f32>`; the decoded samples (not the WAV path) are what travel to the transcribe worker over the channel. The format contract — 16 kHz mono float32 in `[-1.0, 1.0]` — is documented in [ADR 0014](../../decisions/0014-audio-input-invariant-float32-pcm-16-khz-mono-validated-at-decode.md); the conversion is the `/32768.0` normalisation at `src/audio.rs:74`. For what happens next, see [transcription.md](transcription.md).

### Metadata capture chain (Epic 4c)

Every fetch also captures the video's metadata, raw-first, per [ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md). Four links:

1. **Print** — the `--no-simulate --print <template>` pair above puts one line of JSON on the child's stdout, from the info dict the download already required. No extra request, no second invocation.
2. **Envelope** — `build_metadata_envelope(stdout, cap)` (`src/fetcher/ytdlp.rs`) wraps that line **unparsed** in `{"schema":1,"printed":"<line>"}` and hands it back as `MetadataCapture { envelope_json }` (`src/fetcher/mod.rs`). It returns `None` — no row, a `warn!`, fetch unaffected — when stdout is absent, empty, or at the capture cap. Parsing is not the fetcher's job; `schema` is the loader's compatibility gate.
3. **Persist** — the envelope is built **before** the exit-code check, so a yt-dlp that died mid-transfer still yields metadata (the print lands before the media transfer). Both pipeline paths — `fetch_worker` (`src/pipeline/pipelined.rs`) and `process_one` (`src/pipeline/serial.rs`) — call `Store::upsert_metadata_raw` **before** dispatching on the fetch outcome, so `video_metadata_raw` covers succeeded and classified-failure videos alike (tool failures, that is — timeouts and spawn failures lose the captured output; the retry self-heals). On the pipelined path the store lock is scoped to the insert alone.
4. **Load** — nothing parses at runtime. The operator runs `ddp-transcribe load-metadata` after a batch; `src/metadata_loader.rs` streams `video_metadata_raw` in keyset pages of 10,000 (`Store::metadata_raw_page`), parses each envelope (`parse_envelope`), and writes the typed columns one transaction per page (`Store::apply_metadata_batch`). It is idempotent and replayable, and `--dry-run` reports real counts without writing.

**The best-effort invariant.** Metadata never creates a new failure mode. Capture failure logs and stores nothing; insert failure logs and continues; a parse failure increments `LoadStats::rows_skipped_unparseable` and moves to the next row. At no point can a metadata error change a video's status transition — that is the ADR-0042 invariant, and review rejects code that breaches it.

**What this does and does not collect.** The typed columns are `video_description` (the creator's caption text), `uploader`, `uploader_id`, `video_created_at`, `view_count`, `like_count`, `comment_count`, and `metadata_fetched_at` — the engagement counts are **point-in-time snapshots** taken at the moment named by `metadata_fetched_at`, not current values. The printed set is deliberately wider than the columns (`title`, `channel_id`, `duration`, `repost_count` stay raw-only) so a future column addition is a `load-metadata` re-run, not a re-fetch. **Not collected:** comments (Research API only, never available to this pipeline) and caption/subtitle tracks (operator descope, 2026-07-28 — see ADR 0042's alternatives for the evidence). Spoken content reaches the study only through whisper transcription, never through platform-served caption tracks.

---

## ADRs governing this subsystem

| ADR  | Title                                               | Where it applies                                                                        |
|------|-----------------------------------------------------|-----------------------------------------------------------------------------------------|
| 0014 | Audio input invariant float32 PCM 16kHz mono via hound | Audio prep boundary: yt-dlp postprocessor enforces the format; `src/audio.rs` validates it. |
| 0021 | Bounded subprocess output capture via streaming VecDeque\<u8\> | Fetcher's yt-dlp invocation — both streams drained; stderr retains trailing 8 KiB. |
| 0023 | Minimum mutator signatures (kind: &str, message: &str) returning Result\<usize\> per 0006 | Retry classification surface — the `label` parameter `record_fetch_failure` receives from the fetch worker stays `&str`. |
| 0037 | Operator-editable TOML classification table with compiled evidence-derived default | Drives the message-class labels/dispositions the fetch-side classifiers produce. |
| 0039 | DDP timestamps are UTC-assumed, empirically unresolved | `parse_watched_at`; `watched_at_raw` is the hedge. |
| 0040 | Analysis window computed at ingest; `recompute-window` is the only flag mutator | `--window-start`/`--window-end`; `watch_history.in_window`. |
| 0042 | Fetch-time metadata is captured raw-first; parsing is a replayable post-run step | The `--print` capture chain, the `video_metadata_raw` envelope, and `load-metadata`'s parse. Cross-cuts the state machine (schema v5). |
