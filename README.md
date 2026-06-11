# ddp-transcribe

Video-transcription pipeline for data-donation studies: reads donated DDP
watch-history JSONs from an inbox folder, fetches each video's audio
(`yt-dlp` + `ffmpeg`), transcribes it (`whisper.cpp`), and stores transcripts
and state for downstream analysis. Single Rust binary, SQLite-backed.
TikTok is the currently supported source.

> **Formerly `uu-tiktok`.** Historical docs (ADRs, plans, bake notes under
> `docs/`) use the old name and the old `UU_TIKTOK_*` env prefix; they are
> dated records and have deliberately not been rewritten.

> **Plan A — walking skeleton.** Only the `Dev` profile is wired. Known
> Plan-A quirks when you test:
> - `ingest --dry-run` is not yet implemented — it logs a note and runs a real ingest anyway.
> - `process` exits with code **3** when it claimed zero videos — this is intentional, not a failure.

## Quickstart

### Prerequisites

External tools on `PATH`:

- `yt-dlp` (fetches audio)
- `ffmpeg` (invoked by yt-dlp's postprocessor to resample to 16 kHz mono)
- `whisper-cli` (the whisper.cpp binary; transcribes)

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

Log verbosity is controlled by `RUST_LOG` (e.g. `RUST_LOG=debug`).

### `init`

Creates `state.sqlite` and applies the schema. Idempotent — if the DB
already carries a `schema_version`, it logs and exits 0.

### `ingest [--dry-run]`

Walks `--inbox`, parses each DDP watch-history JSON, canonicalizes each
`Link` to a video id, and upserts into `videos` (new rows `pending`) and
`watch_history`. Summary counts (files processed, unique videos, duplicate
watch rows, short-links skipped, invalid URLs skipped) are logged.

`--dry-run` is accepted but not yet implemented — the command runs a real
ingest and logs a note.

### `process [--max-videos N]`

Claims pending videos one at a time, fetches audio via `yt-dlp`, hands the
WAV to `whisper-cli`, writes the transcript `.txt` + `.json` + metadata
under `<transcripts>/<shard>/`, then marks the row succeeded or failed.
`--max-videos` caps the batch.

Exit codes:

- `0` — at least one video was claimed (regardless of per-video outcome).
- `3` — zero videos were claimed (inbox empty, everything already done, or
  all pending rows are currently claimed by another worker).
- non-zero other — unrecoverable error (DB open, artifact dir creation, etc.).

## Repo layout

```
src/
  main.rs             # binary entry + subcommand dispatch
  cli.rs              # clap definitions (flags, subcommands)
  config.rs           # resolved runtime config (profile → values)
  canonical.rs        # TikTok URL → canonical video_id
  ingest.rs           # DDP JSON → videos + watch_history upserts
  state/              # rusqlite Store + schema (WAL, claim_next, mark_succeeded, ...)
  fetcher/            # VideoFetcher trait + YtDlpFetcher
  transcribe.rs       # whisper-cli wrapper
  pipeline.rs         # per-video orchestration (fetch → transcribe → artifacts)
  process.rs          # batch loop used by the `process` subcommand
  output/artifacts.rs # atomic transcript/metadata writes + tmp cleanup
  errors.rs           # typed error enums

tests/                # integration tests; most gated by feature `test-helpers`
  fixtures/ddp/       # sample donation-extractor JSON(s)
  e2e_real_tools.rs   # ignored by default; real yt-dlp/ffmpeg/whisper-cli + network

scripts/              # one-off dev scripts (model fetch)

docs/
  superpowers/specs/  # canonical design spec
  superpowers/plans/  # Plan A tasks + Plan B kickoff prompt
  decisions/          # ADRs (+ index.yaml)
  reference/          # scraped TikTok developer documentation
  FOLLOWUPS.md        # deferred work captured during Plan A
```

## Where to read more

- [`docs/superpowers/specs/2026-04-16-uu-tiktok-pipeline-design.md`](docs/superpowers/specs/2026-04-16-uu-tiktok-pipeline-design.md)
  — the canonical design: scope, architecture, data model, error taxonomy,
  operational model. Start here for *why*.
- [`docs/decisions/`](docs/decisions/) — ADRs covering concrete choices
  (e.g. transcript sharding `0004`, `test-helpers` feature `0005`,
  artifact-write ordering `0008`). See `index.yaml`.
- [`docs/superpowers/plans/2026-04-16-plan-a/`](docs/superpowers/plans/2026-04-16-plan-a/)
  — per-task Plan A files (what was built, in order).
- [`docs/superpowers/plans/PLAN-B-KICKOFF-PROMPT.md`](docs/superpowers/plans/PLAN-B-KICKOFF-PROMPT.md)
  — what Plan B will build on top of the walking skeleton.
- [`docs/FOLLOWUPS.md`](docs/FOLLOWUPS.md) — deferred work and known gaps
  (including the whisper-model-path override).
- [`docs/reference/tiktok-for-developers/`](docs/reference/tiktok-for-developers/)
  — local copy of TikTok's scraped developer docs (Research API, DDP,
  Content Posting, etc.). Used for lookup during design; not a runtime dep.
