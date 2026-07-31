---
status: accepted
date: "2026-07-28"
category: Fetcher
applies_to:
    - src/fetcher/ytdlp.rs
    - src/fetcher/mod.rs
    - src/metadata_loader.rs
    - src/backfill.rs
    - src/state/schema.rs
priority: invariant
companions:
    - tests/load_metadata.rs
    - tests/state_metadata.rs
    - tests/backfill_cohort.rs
    - tests/backfill_metadata.rs
    - tests/e2e_real_tools.rs
---

# Fetch-time metadata is captured raw-first; parsing is a replayable post-run step

## Decision

Metadata rides the existing yt-dlp invocation as an unparsed `--print` line,
wrapped in a versioned envelope and written to `video_metadata_raw` before the
fetch outcome is interpreted; only the `load-metadata` subcommand parses it,
into nullable `videos` columns. A parse bug is fixed by re-parsing, never by
re-fetching.

## Guidance

- Metadata must never create a new failure mode: capture, insert, and parse errors log and count, and the video's outcome is exactly what it would have been without metadata. Review rejects any path where a metadata error changes a status transition. Enforced by construction — `build_metadata_envelope` runs before the exit-code check in `YtDlpFetcher::acquire`, both pipeline paths upsert before outcome dispatch and only `warn!` on error, `load_metadata` counts `rows_skipped_unparseable` instead of returning `Err`, and `backfill_metadata` counts `captures_failed`/`inserts_failed` per video and continues rather than aborting the pass.
- On the fetch path, capture adds no network request and no second tool invocation: `--no-simulate --print <template>` reads the info dict the download already extracted. Review rejects a separate enrichment invocation or any extra request per fetched video.
- `backfill_metadata` is the one sanctioned metadata-only invocation, existing solely to recover cohorts that predate fetch-time capture (the rc1-era corpus). It writes through `insert_metadata_raw_if_missing`, never the fetch path's last-write-wins upsert; it never touches video status or lifecycle; and it never carries cookies (`build_metadata_only_args` takes only a URL, so the login-gated-retries-only cookie scope holds by construction). A backfilled envelope is schema-identical to a fetch-time one; `load_metadata` cannot tell them apart. Review rejects widening this carve-out into a routine enrichment pass over already-captured videos.
- Caption/subtitle tracks are deliberately NOT collected — no `--write-subs`, `--write-auto-subs`, `--sub-langs`, or `--list-subs` may be added to the fetch argv, and `METADATA_PRINT_TEMPLATE` must not name `subtitles`/`automatic_captions` (unit-asserted). A subtitle download failure raises `DownloadError` in the pinned yt-dlp, so it could flip a good fetch to a spurious failure, and listing-only capture still spends the primary invocation's timeout budget. The creator's caption *text* is captured — it is `description` → `videos.video_description`.
- The envelope's `schema` field gates loader compatibility: `parse_envelope` skips and counts a version it does not know rather than guessing. A payload-shape change bumps `schema`; version 1 is never reinterpreted.
- The printed field set is deliberately wider than the typed columns (`title`, `channel_id`, `duration`, `repost_count` stay raw-only). Adding a column later must be a `load-metadata` re-run, not a re-fetch — do not prune the template to match the column set.
- Engagement counts are point-in-time snapshots keyed by `metadata_fetched_at`, and `video_metadata_raw` is last-write-wins **among claim holders** per `video_id`: `upsert_metadata_raw` carries the same `status = 'in_progress' AND claimed_by = ?` predicate as every other in-flight mutator, so a worker whose claim was swept out from under it cannot overwrite the newer holder's envelope. Never render the counts as current values, and never compare them across videos without their fetch timestamps.

## Why

The Research API is unavailable to this study, so yt-dlp's info dict is the only
metadata source — and at the 2,982,471 unique videos measured in the production
inbox (2026-07-28), the fetch is the irreplaceable operation: re-fetching the
corpus costs weeks of wall clock and fresh rate-limit exposure, while re-parsing
stored blobs costs minutes of local CPU. Storing the line unparsed moves every
future metadata mistake from the expensive side of that asymmetry to the cheap
one. Probe evidence the same day (yt-dlp 2026.07.04): 46/46 corpus videos printed
a populated title, one line of JSON at ~615 B.

## Alternatives

- **`--write-info-json`** — a per-video file write and cleanup obligation across ~3M fetches, for data that fits in one stdout line.
- **A separate enrichment pass** — doubles network volume against the same rate limiter for no data the download invocation does not already hold. The `backfill-metadata` subcommand is the bounded exception, not a reversal: it exists only where no download invocation ever ran the capture, and its cohort query drains as envelopes land.
- **Parsing straight into typed columns at fetch time** — a parse bug then becomes unrecoverable without re-fetch, which is the exact cost this record exists to avoid.
- **A JSONL sidecar log** — torn lines under concurrent writers, plus a new operational artifact to ship and reconcile alongside the state DB.
- **Collecting caption/subtitle tracks** — built, then removed before merge on 2026-07-28 operator adjudication. Corrected coverage evidence: ~36% of corpus videos list any caption track (4/11 re-probe; an earlier 0/46 reading was an artifact of probing without the listing flags). That yield does not justify the added failure surface and request pressure; spoken content reaches the study through whisper transcription instead.
