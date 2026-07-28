# Epic 4c — Fetch-time metadata capture (title, captions, engagement)

**Status:** design approved 2026-07-28 (operator brainstorm). Implementation plan to follow. Partially superseded 2026-07-28: caption/subtitle track collection was descoped by operator decision after implementation-time evidence (subtitle downloads are fatal-on-failure in pinned yt-dlp; the cited 0/46 coverage probe was a measurement artifact — corrected figure ~36%). See EPIC-4C-CLOSE §Deviations and ADR-0042; video_description (the creator's caption text) IS captured.
**Scope anchor:** must land before the production batch run — yt-dlp metadata capture is forward-only; retroactive capture would require re-fetching the corpus.

## Context

The PI requested three additional captured signals per watched video: (1) video title, (2) captions where available, (3) comments. TikTok Research API access is not expected, so yt-dlp — the pipeline's existing fetch mechanism — is the only capture channel.

**Production scale (measured 2026-07-28 against the live donor inbox, 144 files):** 4,847,408 watch entries, **2,982,471 unique videos**, watch dates spanning 2025-08 → 2026-07. Per-file entries: min 0 / median 17,585 / max 174,767. This is ~53× the pilot corpus (56,620). Clock-time efficiency is the binding design constraint; the analysis window (Epic 4b) is expected to cut the fetch set, but the design must not assume it.

**Empirical probe (55 sampled corpus videos, 2026-07-28, unauthenticated yt-dlp 2026.07.04):**
- Title/description: present on **46/46** fetchable videos (`title` = `desc` truncated to 72 chars; `description` = full text). Free at fetch time.
- Caption tracks: **0/46** exposed any subtitle/auto-caption track. "Captions where available" is honest but expected ≈ empty via this channel.
- Overlay/sticker text (the PI's hypothesized "text on screen over music"): **structurally absent** from yt-dlp's info dict — not extractable via yt-dlp at all (it is a Research API field, `sticker_info_list`).
- ~15% of sampled links already dead (per ADR-0033 taxonomy: `IpBlockedMessage`/`VideoNotAvailable*` = video removed, NOT IP blocking).
- Comments: **excluded** — yt-dlp's TikTok extractor has no comment support. (The DDP export's own `Comments` section contains comments the donor wrote and is separately parseable if the PI's ask means that; out of 4c scope.)

## Decisions

1. **Capture rides the existing fetch invocation — zero extra network requests.** Add `--no-simulate --print "%(.{…fields…})j"` to the yt-dlp argv; raise `stdout_capture_bytes` from 0 to a bound (64 KB). yt-dlp prints a field-limited JSON (~1–2 KB) from the info dict it already holds. Rejected: `--write-info-json` (extra file write/read/unlink × 3M on the ~25 KB full dict); separate enrichment pass (doubles network requests).
2. **Raw-first storage: blobs into the state DB, parsing deferred to a post-run loader.** The fetcher INSERTs the raw capture into a new `video_metadata_raw` table *before* interpreting fetch exit status, so metadata persists for videos that die mid-download (`NoDataBlocks`, `FfprobePostprocess`) with **zero changes to the failure mutators**. Parsing into typed columns happens post-run via a new `load-metadata` subcommand — any parse/mapping bug is forever fixable by re-parse, never re-fetch. Rejected: direct-to-schema parsing in the pipeline (parse mistakes at 3M scale unrecoverable without re-fetch); JSONL sidecar log (worse durability — torn lines, buffered-loss window; new file format; extra operational artifact).
3. **Typed metadata lands in `videos` columns (schema v5), not in transcript artifacts.** Transcript `.json`/`.txt` artifacts are unchanged — growing 3M small JSONs taxes every downstream parse pass; corpus-wide metadata questions should be one SQL query. Delivery-time tabular export (`SELECT` → CSV/Parquet) is deferred and trivial. Schema v5 lands now, while DB changes are cheap (pre-production, no backfill).
4. **Captions are captured best-effort at fetch time, content not URLs.** TikTok's track URLs are ephemeral; storing them is worthless post-hoc. When a track exists, its content is downloaded during the same fetch and embedded in the raw blob. Measured coverage ≈ 0%, so the cost is ≈ never incurred; if TikTok/yt-dlp ever surface tracks (e.g. via cookie-authenticated fetches), we get content for free. Only platform-served tracks (creator captions or TikTok auto-captions) — nothing locally generated.
5. **Metadata capture must never create a new failure mode.** A video that fetches fine but yields missing/unparseable/oversized metadata proceeds exactly as today, with a logged warning and no raw row (or a raw row the loader later skips with a counted warning). Same for caption-track download failures.

## Architecture

### Capture chain (per video, inside the existing fetch)

```
yt-dlp --no-simulate --print "%(.{fields})j" -x … <url>
  └─ stdout (≤64 KB, bounded capture)  ──►  fetcher parses NOTHING; wraps in envelope
       envelope = {"schema": 1, "printed": <raw line>, "captions": {<name>: <content>} | null}
  └─ subtitle sidecars (--write-subs --write-auto-subs, if any) ──► read, embed, delete
INSERT OR REPLACE INTO video_metadata_raw(video_id, fetched_at, raw_json=envelope)
  └─ executed regardless of subsequent fetch outcome (before exit-status interpretation)
```

- **Printed field set (generous — bulky arrays excluded):** `id, title, description, uploader, uploader_id, channel_id, timestamp, duration, view_count, like_count, comment_count, repost_count, subtitles, automatic_captions`. Exact list finalized at plan time against yt-dlp 2026.07.04 output; excludes `formats`, `thumbnails`, `http_headers`. The printed set is deliberately wider than the typed columns — extra fields live only in the raw blob, available to any future re-parse without re-fetch.
- **Oversized/absent capture policy:** if stdout hits the 64 KB bound (truncated ⇒ unparseable) or is empty, no raw row is written; the fetch proceeds normally and the event is logged + counted. The loader independently skip-counts any unparseable blob it encounters.
- **Caption download goes through yt-dlp** (`--write-subs --write-auto-subs`), never a hand-rolled HTTP client (the crate has none). Sidecar files land in the per-claim `video_dir`, are read (bounded, 256 KB/track), embedded into the envelope keyed by filename, and deleted with the existing cleanup.
- **`process.rs` contract change:** `CommandOutcome` must retain captured stdout on the *failure* path too (today capture is 0 so the question never arose). The fetcher reads stdout before classifying the outcome.
- **Envelope is versioned** (`schema: 1`) so the loader can evolve.
- `INSERT OR REPLACE` keyed by `video_id`: one blob per unique video, last-write-wins across retries (engagement counts are point-in-time; `fetched_at` records the snapshot moment).
- Write cost: one sub-ms WAL transaction per fetch, fully overlapped with network. DB growth ≈ 6–12 GB at 3M videos; prunable post-load (`DELETE` + `VACUUM`) if a lean export copy is wanted.

### Schema v5 (one migrate-ladder step, per ADR-0022)

- New table: `video_metadata_raw (video_id TEXT PRIMARY KEY, fetched_at INTEGER NOT NULL, raw_json TEXT NOT NULL)`.
- `videos` gains nullable columns (NULL = never loaded, ~free per row): `video_description TEXT`, `uploader TEXT`, `uploader_id TEXT`, `video_created_at INTEGER`, `view_count INTEGER`, `like_count INTEGER`, `comment_count INTEGER`, `captions_json TEXT`, `metadata_fetched_at INTEGER`.
- `SCHEMA_VERSION` "4" → "5"; ladder extended in the same task; both directions tested (older DB open fails typed; migrate idempotent). Never auto-migrate on open.

### `load-metadata` subcommand (post-run, replayable)

- `ddp-transcribe load-metadata [--dry-run]`: single pass over `video_metadata_raw`, parse envelope → printed JSON → `UPDATE videos SET … , metadata_fetched_at = raw.fetched_at WHERE video_id = …`, batched in transactions (e.g. 10k rows/tx). Minutes at 3M rows.
- Idempotent and replayable: re-running overwrites from the current blobs (last-write-wins). `--dry-run` reports counts without writing.
- Loader stats per ADR-0007 (input-side counters, verb-named): rows examined, parsed, loaded, skipped-unparseable (counted + logged, never fatal), captions embedded.
- Bails if `--state-db` missing (Migrate/Status arm precedent). Mutators return `Result<usize>` per ADR-0006.
- `captions_json` column gets the envelope's captions map verbatim when non-null.

### Explicit non-goals

- Transcript artifact schema unchanged; no `status` surface extension (a later epic may add `status --metadata` coverage counts).
- No pilot-corpus backfill (56,620 videos predate capture; production re-encounters the popular subset).
- No comments (Research API only; donor-authored comments in the DDP export are a separate, trivial ask if wanted).
- No delivery export subcommand (operator: `sqlite3 -csv` / one-off `SELECT` at delivery time).
- Production-run capacity planning (3M videos × fetch+transcribe throughput) is its own upcoming conversation — this epic only ensures the run captures metadata when it happens.

## Testing

- **Unit:** argv construction (print template + subs flags, redaction indices unchanged); envelope build (with/without captions, oversized truncation policy); loader parse of envelope fixtures (valid, torn/unparseable, absent fields → NULLs).
- **Integration (per ADR-0005/0003 discipline):** fake-fetcher metadata injection through the pipelined path — raw rows present for succeeded AND failed videos; schema v4→v5 migrate both-directions (hand-built v4 fixture per `tests/state_migrate.rs` style); `load-metadata` end-to-end against a seeded raw table (columns populated, idempotent re-run, `--dry-run` writes nothing, stats correct); missing-DB bail.
- **Ignored live e2e:** one real-URL fetch asserting a raw row with parseable printed JSON (model-gated ignore pattern).
- Verification gate unchanged: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` (mandatory single-threaded).

## Operator workflow delta

```
ddp-transcribe ingest --window-start … --window-end …   (unchanged)
ddp-transcribe process …                                 (metadata captured automatically)
ddp-transcribe load-metadata                             (NEW: post-run, minutes, replayable)
ddp-transcribe status …                                  (unchanged)
```

## PI communication notes (for the operator)

- Title/description: delivered for every fetchable video.
- Captions: captured where the platform serves them; measured coverage on today's corpus ≈ 0% — set expectations accordingly. On-screen overlay text is not obtainable without Research API access (`sticker_info_list`).
- Comments: not obtainable via this pipeline; donor-authored comments exist in the DDP exports if that satisfies the ask.
- Engagement counts are snapshots at fetch time (`metadata_fetched_at`), not stable quantities.
