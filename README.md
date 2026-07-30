# ddp-transcribe

Video-transcription pipeline for data-donation studies: reads donated DDP
watch-history JSONs from an inbox folder, fetches each video's audio
(`yt-dlp` + `ffmpeg`), transcribes it (`whisper.cpp`), and stores transcripts
and state for downstream analysis. Single Rust binary, SQLite-backed.
TikTok is the currently supported source.

> **Formerly `uu-tiktok`.** Historical docs (ADRs, plans, bake notes under
> `docs/`) use the old name and the old `UU_TIKTOK_*` env prefix; they are
> dated records and have deliberately not been rewritten.

> **Status: live-campaign codebase.** Plan B epics through 5a (campaign
> safety) have landed: embedded whisper.cpp via `whisper-rs`, the durable
> state machine, the pipelined orchestrator with failure classification and
> in-batch retry, the analysis window, metadata capture, the read-only
> `status` surface, and the in-run checkpoint hook. This is what runs the
> production batch, not a skeleton. Two things still surprise newcomers:
> - Only the `Dev` profile is wired — `--profile` exists but has one value.
> - `process` exits with code **3** when it claimed zero videos — intentional (nothing to do), not a failure.

## Quickstart

### Prerequisites

External tools on `PATH`:

- `yt-dlp` (fetches audio)
- `ffmpeg` (invoked by yt-dlp's postprocessor to resample to 16 kHz mono)

whisper.cpp is **embedded** (`whisper-rs`), not shelled out to — there is no
`whisper-cli` on the runtime path. You still supply a model file.

Plus a Rust toolchain (stable; edition 2021).

### One-time setup

Download the `tiny.en` whisper model (~75 MB) to `./models/`:

```sh
./scripts/fetch-tiny-model.sh
```

### Build

```sh
cargo build            # dev
cargo build --release  # needed for the e2e test against real tools
```

### Minimal end-to-end run

Using the shipped DDP fixture:

```sh
mkdir -p inbox
cp tests/fixtures/ddp/news_orgs/*.json inbox/

cargo run -- init
cargo run -- ingest
cargo run -- process --max-videos 1
```

Expect: `state.sqlite` in the cwd, a transcript `.txt` + `.json` under
`transcripts/<last-two-digits-of-video-id>/`, and log lines summarizing
counts. If `process` exits 3 with `claimed=0`, ingest found no processable
videos — check the inbox JSON.

### Tests

```sh
cargo test                                   # unit + non-gated integration tests
cargo test --features test-helpers           # everything except real-network e2e
cargo test --features test-helpers --test e2e_real_tools -- --ignored --nocapture
                                             # real tools + network; requires model at ./models/
```

Override the e2e video URL with `DDP_TRANSCRIBE_E2E_URL=<url>`.

## Commands

All subcommands accept the global flags below (or their env equivalents).

| Flag              | Env                        | Default                       | Notes                                                                |
|-------------------|----------------------------|-------------------------------|----------------------------------------------------------------------|
| `--profile`       | `DDP_TRANSCRIBE_PROFILE`        | `dev`                         | Only `dev` is wired.                                                 |
| `--state-db`      | `DDP_TRANSCRIBE_STATE_DB`       | `./state.sqlite`              |                                                                      |
| `--inbox`         | `DDP_TRANSCRIBE_INBOX`          | `./inbox`                     | DDP JSONs read from here.                                            |
| `--transcripts`   | `DDP_TRANSCRIBE_TRANSCRIPTS`    | `./transcripts`               | Artifacts written here.                                              |
| `--log-format`    | `DDP_TRANSCRIBE_LOG_FORMAT`     | `human`                       | `human` or `json`.                                                   |
| `--whisper-model` | `DDP_TRANSCRIBE_WHISPER_MODEL`  | `./models/ggml-tiny.en.bin`   | Path to whisper.cpp model file. `tiny.en` is English-only; for non-English audio use a multilingual model (e.g. `ggml-small.bin`). |
| `--classification` | `DDP_TRANSCRIBE_CLASSIFICATION` | compiled default            | TOML failure-classification policy. Validated hard-fail before the model loads; the active table is snapshotted into `batch_runs`. |
| `--compute-lang-probs` | `DDP_TRANSCRIBE_COMPUTE_LANG_PROBS` | off                | Emit a per-language probability distribution per video. Costs one extra encoder pass. |
| `--stale-claim-threshold` | `DDP_TRANSCRIBE_STALE_CLAIM_THRESHOLD` | `30m`         | Age after which a crashed run's `in_progress` claim is swept back to `pending`. Humantime (`45s`, `1h`). Also guards the startup temp-file cleanup. |
| `--download-workers` | `DDP_TRANSCRIBE_DOWNLOAD_WORKERS` | `3`                    | Parallel fetch workers feeding the single transcribe worker. Must be ≥ 1. |
| `--channel-capacity` | `DDP_TRANSCRIBE_CHANNEL_CAPACITY` | `2`                    | Bounded fetch→transcribe queue depth (backpressure). Must be ≥ 1. |

Log verbosity is controlled by `RUST_LOG` (e.g. `RUST_LOG=debug`).

### `init`

Creates `state.sqlite` and applies the schema. Idempotent — if the DB
already carries a `schema_version`, it logs and exits 0.

### `migrate`

Upgrades an older `state.sqlite` to the current schema version, in one
transaction, then bumps `meta.schema_version`. Every other subcommand
*refuses to open* a DB whose version doesn't match the binary's, so this is
the only way forward after an upgrade. Idempotent — a no-op when already
current.

### `ingest [--dry-run] [--window-start D] [--window-end D]`

Walks `--inbox`, parses each DDP watch-history JSON, canonicalizes each
`Link` to a video id, and upserts into `videos` (new rows `pending`) and
`watch_history`. Summary counts (files processed, unique videos, duplicate
watch rows, short-links skipped, invalid URLs skipped) are logged. A file
already ingested unchanged (same name, size and mtime) is skipped without
being read.

`--window-start` / `--window-end` (`YYYY-MM-DD`, UTC, both inclusive; either
may be omitted) set the analysis window. Each watch row's `in_window` flag is
computed once, here, and stored; nothing re-derives it at query time.

`--dry-run` does the full pass — every file read, parsed and upserted — inside
a single transaction spanning the whole inbox, then rolls that transaction
back. Nothing persists (not even the ingest ledger), and because every file
sees the earlier files' uncommitted rows, the reported counts are exactly a
real run's — including duplicates and raw-date backfills that only show up
across files.

The cost: a dry-run holds **one** write transaction (`BEGIN IMMEDIATE` …
rollback) for the whole inbox scan, file reads and JSON parsing included,
where a real ingest takes only brief per-file write locks. A full-inbox
dry-run alongside a running `process` can hold that lock past
`busy_timeout` (5s), in which case `process`'s claims start failing with
`SQLITE_BUSY` and its batch aborts. Run a dry-run only at a pause — no
`process` running — not alongside one.

### `process`

The batch. On startup it clears leftover partial-write temp files older than
`--stale-claim-threshold`, opens a `batch_runs` row snapshotting the run's
params and the active classification policy, and re-adjudicates previously
parked retryable failures through that policy. Then it drains: claims
abandoned by a crashed run are swept back to `pending`, and
`--download-workers` fetch workers claim pending videos and pull audio via
`yt-dlp`, feeding one transcribe worker over a bounded channel. Each success writes the transcript `.txt` + `.json` under
`<transcripts>/<shard>/` *before* the row is marked succeeded. Failures are
classified — retryable ones requeue in-batch under the retry budget, terminal
ones are written off. The run ends by drain (no pending work left), stamping
a census onto the `batch_runs` row and printing it.

| Flag | Default | Notes |
|------|---------|-------|
| `--max-videos N` | unlimited | Cap the batch. |
| `--retries N` | `1` | In-batch retry budget per video; lifetime attempts = `retries + 1`. |
| `--cookies-file PATH` | none | Netscape-format cookie jar, passed to `yt-dlp` **only** when retrying a video previously classified sensitive/login-gated. Never sent on a first attempt; the path is redacted from logs. Env: `DDP_TRANSCRIBE_COOKIES_FILE`. |
| `--checkpoint-cmd PATH` | none | Operator hook run periodically during the batch (e.g. a sync-to-storage script). Invoked with no arguments; a failure or timeout warns and increments a counter — it can never abort the run. |
| `--checkpoint-every DUR` | `15m` | Interval between hook runs *and* the hook's own timeout, so a slow hook reports itself instead of stacking copies. Requires `--checkpoint-cmd`. |

Exit codes:

- `0` — at least one video was claimed (regardless of per-video outcome).
- `3` — zero videos were claimed (inbox empty, everything already done, or
  all pending rows are currently claimed by another worker).
- non-zero other — unrecoverable error (DB open, artifact dir creation, a
  Bug-class worker failure, etc.).

### `status`

The read-only operator surface; never mutates the DB. Bare, it prints counts
by status, `failed_retryable` broken down by failure kind, ages of current
claims, and `batch_runs` history. Narrowing flags: `--video-id` (one video's
full event history), `--respondent-id` (per-respondent counts), `--errors`
(terminal failures with reasons), `--retryable` (parked retryables).
`--verify` runs the done-contract checks — artifacts present at their sharded
paths, `raw_signals.schema_version` parses, batch is pause-safe — and exits 1
when it isn't. `--json` emits machine-readable output instead of text.

### `recompute-window`

The only thing that changes `in_window` after ingest. One-shot; does not
re-read DDP files. Takes the same `--window-start` / `--window-end` bounds,
or `--clear` to explicitly opt into "no filter" (every row `in_window = 1`).
It refuses to run bare — silently wiping a study's window filter must be
impossible. `--dry-run` reports how many rows would change, without writing.

### `load-metadata [--dry-run]`

Post-run step: parses the raw metadata envelopes captured at fetch time into
the typed `videos` columns. Idempotent and replayable — it re-parses from the
stored blobs, so fixing the parser needs no re-fetch. `--dry-run` parses
everything and reports counts without writing.

### `backfill-metadata [--limit N] [--dry-run]`

Fills in raw metadata for already-succeeded videos that predate fetch-time
capture. Metadata-only `yt-dlp` per video — no media download, and it never
touches a video's status. Best-effort and re-runnable; run `load-metadata`
afterwards to populate the typed columns. `--limit` caps the attempt count
for smoke runs; `--dry-run` prints the cohort size and exits without invoking
`yt-dlp`.

## Repo layout

```
src/
  main.rs             # binary entry + subcommand dispatch
  cli.rs              # clap definitions (flags, subcommands)
  config.rs           # resolved runtime config (profile → values)
  canonical.rs        # TikTok URL → canonical video_id
  ingest.rs           # DDP JSON → videos + watch_history upserts
  state/              # rusqlite Store: schema, migrations, claims, read queries
  fetcher/            # VideoFetcher trait + YtDlpFetcher
  audio.rs            # WAV → float32 PCM 16 kHz mono, the engine's input invariant
  transcribe.rs       # embedded whisper.cpp engine (whisper-rs)
  pipeline/           # shared types + serial loop + pipelined orchestrator
  batch.rs            # batch lifecycle: start-of-batch sweep + run census
  failure.rs          # failure classification: tool error → three-arm verdict
  classification.rs   # the operator-editable classification policy (TOML)
  process.rs          # bounded subprocess runner (timeout + capped capture)
  output/             # transcript sharding + atomic artifact writes + tmp cleanup
  status.rs           # the read-only operator report
  metadata_loader.rs  # load-metadata: raw envelopes → typed columns
  backfill.rs         # backfill-metadata: envelopes for pre-capture videos
  errors.rs           # typed error enums

tests/                # integration tests; most gated by feature `test-helpers`
  fixtures/           # DDP JSONs, WAV audio, captured yt-dlp stderr
  pipeline_fakes/     # orchestrator tests against fake fetcher + transcriber
  e2e_real_tools.rs   # ignored by default; real yt-dlp/ffmpeg + model + network

scripts/              # dev helpers (model fetch, bake comparisons)

docs/
  reference/architecture/  # the architecture doc set — start here
  decisions/               # lean ADRs (README.md is the generated index)
  superpowers/specs/       # per-epic design docs
  superpowers/plans/       # per-task plan files
  operations/              # VM runbook + capacity worknotes
  FOLLOWUPS.md             # scope index over followups/*.md
  madr-archive/            # the frozen pre-migration MADR corpus
```

## Where to read more

- [`docs/reference/architecture/index.md`](docs/reference/architecture/index.md)
  — **start here.** The architecture doc set: the donor's journey end to end,
  plus four lifecycle-stage deepdives (data input, state machine,
  orchestration, transcription). Owns the *what*; redirects to ADRs for the
  *why*.
- [`docs/decisions/`](docs/decisions/) — lean ADRs covering concrete choices
  (e.g. transcript sharding `0004`, artifact-write ordering `0008`, shutdown
  order `0025`, the checkpoint hook `0044`). `README.md` there is the
  generated index; `madr-archive/` holds the frozen pre-migration corpus.
- [`docs/superpowers/specs/`](docs/superpowers/specs/) — one design doc per
  epic; `2026-04-16-uu-tiktok-pipeline-design.md` is the original Plan A
  design, still the best statement of overall scope and data model.
- [`docs/superpowers/plans/`](docs/superpowers/plans/) — per-task plan files,
  one directory per epic (what was built, in order).
- [`docs/operations/`](docs/operations/) — `src-vm.md` is the deployment
  runbook for the SRC A10 workspace; the capacity worknote carries measured
  throughput and the window-narrowing numbers.
- [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) — deferred work and known gaps: a
  scope index over the per-epic files in `docs/followups/`.
- [`docs/reference/tiktok-for-developers/`](docs/reference/tiktok-for-developers/)
  — local copy of TikTok's scraped developer docs (Research API, DDP,
  Content Posting, etc.). Used for lookup during design; not a runtime dep.
