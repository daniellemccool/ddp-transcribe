# ddp-transcribe

Video-transcription pipeline for data-donation studies: reads donated DDP
watch-history JSONs from an inbox folder, fetches each video's audio
(`yt-dlp` + `ffmpeg`), transcribes it (`whisper.cpp`), and stores transcripts
and state for downstream analysis. Single Rust binary, SQLite-backed.
TikTok is the currently supported source.

> **Formerly `uu-tiktok`.** Historical docs (ADRs, plans, bake notes under
> `docs/`) use the old name and the old `UU_TIKTOK_*` env prefix; they are
> dated records and have deliberately not been rewritten.

> **Status: live-campaign codebase.** Plan B is complete through the Epic 5b
> close-out: embedded whisper.cpp via `whisper-rs`, the durable state machine,
> the pipelined orchestrator with failure classification and in-batch retry,
> the analysis window, metadata capture, the read-only `status` surface, the
> in-run checkpoint hook, and the `requeue-failures` operator override. This is
> what runs the production batch, not a skeleton. Two things still surprise
> newcomers:
> - Only the `Dev` profile is wired — `--profile` exists but has one value.
> - `process` exits with code **3** when it claimed zero videos — intentional (nothing to do), not a failure.

## Why scrape, and not TikTok's Research API?

Because the Research API cannot do this study's job — evaluated and
rejected, not overlooked. Research-API access confines analysis to
TikTok's Virtual Compute Environment, which permits **aggregate outputs
only** (descriptive/inferential statistics; "scripts that request
individual data will be rejected"), offers **no network egress**, and has
**no way to transcribe media** — its queryable surface is video metadata
(ids, timestamps, counts, description text). An exposure study needs the
join between an individual donor's watch history and the *spoken content*
of the specific videos they watched; under the VCE that join is impossible
by design. So the donor's DDP export supplies the individual-level half,
and this pipeline supplies the content half by fetching the public pages
of donated video ids with `yt-dlp`. There is no planned Research-API
fetcher backend — early plan documents that gesture at a "multi-fetcher"
future predate this evaluation. (TikTok's developer docs are mirrored
under `docs/reference/tiktok-for-developers/` for reference.)

## Quickstart

### Prerequisites

External tools on `PATH`:

- `yt-dlp` (fetches audio)
- `ffmpeg` (invoked by yt-dlp's postprocessor to resample to 16 kHz mono)

whisper.cpp is **embedded** (`whisper-rs`), not shelled out to — there is no
`whisper-cli` on the runtime path. You still supply a model file.

Build-time dependencies (whisper.cpp is compiled from source by
`whisper-rs-sys` on every build, CPU or GPU):

- a Rust toolchain (stable; edition 2021)
- `cmake` (drives the vendored whisper.cpp build)
- `clang` (bindgen needs `libclang.so` to generate the C bindings — the
  build fails with `Unable to find libclang` without it)

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

### GPU (CUDA) build

```sh
cargo build --release --features cuda
```

Additional prerequisites: the CUDA toolkit (`nvcc`) and an NVIDIA GPU with
compute capability ≥ 7.5 (CUDA 13 dropped older architectures). Notes from a
verified Arch Linux setup (RTX 2080, CUDA 13.3, 2026-08):

- packages: `cuda cmake clang` (plus `yt-dlp` and `rustup` from the base
  prerequisites); the `cuda` package pulls its own compatible host compiler
  (`gcc15`)
- `nvcc` lives in `/opt/cuda/bin` — on `PATH` via `/etc/profile.d/cuda.sh`
  after a fresh shell, or set it explicitly for the build
- `nvcc` rejects a too-new system gcc (13.3 supports ≤ gcc 15): set
  `CUDAHOSTCXX=/usr/bin/g++-15`
- pin the kernel architectures to your card with `CUDAARCHS` (e.g. `75` for
  Turing) — skipping this makes cmake probe the GPU at build time, which
  fails in environments where the build host can't see the device

All together:

```sh
PATH=/opt/cuda/bin:$PATH CUDAHOSTCXX=/usr/bin/g++-15 CUDAARCHS=75 \
  cargo build --release --features cuda
```

A CUDA build **refuses to run on CPU** (ADR-0013): startup must log
`backend="GPU" device="CUDA0"`; if whisper.cpp reports no GPU, the engine
aborts instead of silently running ~100× slower. First full CUDA compile of
the whisper.cpp kernels takes ~8 minutes; warm rebuilds are ~2.

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

The batch. On startup it clears leftover partial-write temp files **and**
leftover fetch work directories older than `--stale-claim-threshold`, opens a
`batch_runs` row snapshotting the run's params and the active classification
policy, and re-adjudicates previously parked retryable failures through that
policy. Then it drains: claims abandoned by a crashed run are swept back to
`pending`, and `--download-workers` fetch workers claim pending videos and pull
audio via `yt-dlp`, feeding one transcribe worker over a bounded channel. Each
success writes the transcript `.txt` + `.json` under `<transcripts>/<shard>/`
*before* the row is marked succeeded. Failures are classified — retryable ones
requeue in-batch under the retry budget, terminal ones are written off. The run
ends by drain (no pending work left), stamping a census onto the `batch_runs`
row and printing it.

Every fetch attempt gets its own directory under `<transcripts>/.work/` —
`ytdlp-<video_id>.<pid>-<seq>`, never reused across retries, workers or
processes. The downloaded WAV is *discovered* by scanning that directory
(exactly one `.wav` is success; zero and more-than-one are distinct failures —
the fetcher never guesses), and the whole directory is removed at the end of
the attempt: after the row is marked succeeded, or on a decode/transcribe
failure, whose retry re-fetches into a fresh one. Only crash, `kill` and
cancellation residue survives, and the age-gated startup sweep collects it —
the age gate is what makes a fresh directory belonging to a second live
instance safe.

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

### `requeue-failures`

The operator escape hatch for rows no automatic mechanism can reach any more:
failures blocked by the lifetime attempt cap, or already written off as
terminal, whose *external* cause has since changed (fresh cookies, a region
unblocked, a yt-dlp bump). It grants eligibility — it is not a second
classifier, and the next fetch is still the liveness oracle.

**Eligibility is default-deny.** A bare `requeue-failures` is a usage error.
You must give at least one *qualifying* selector, or `--all`:

| Selector | Effect |
|---|---|
| `--error-kind <K>` | Match this failure kind. Repeatable; repeats OR together. Exact byte equality — no case folding, and no comma splitting (classification labels may legally contain commas). Matches `last_retryable_kind` on retryable rows, `terminal_reason` on terminal ones. |
| `--max-attempts <N>` | Skip rows with `attempt_count >= N`. |
| `--older-than <DUR>` | Match rows whose last *failure* event is strictly older than this (humantime: `30d`, `12h`). |
| `--all` | Every `failed_retryable` row — never terminals. Conflicts with every qualifying selector: `--all --older-than 30d` is a parse error, not a silent intersection. |
| `--include-terminal` | Also consider `failed_terminal` rows. Opt-in twice over — it *requires* a qualifying selector, so `--include-terminal --all` is rejected. |
| `--max <N>` | Cap rows moved, taken in `attempt_count ASC, video_id ASC` order. A **modifier**: never grants eligibility on its own. |
| `--dry-run` | Read-only; prints per-kind counts plus a total. Also a modifier. |

The `--older-than` clock is the newest of the row's `failed_retryable`,
`failed_terminal`, `retry_requeued` and `cookie_parked` events. Administrative
events (`requeued`, `swept_stale`, `swept_terminal`, `claimed`, `succeeded`)
never reset it, and a row with no qualifying event never matches.

Eligible rows go back to `pending` with `claimed_by`/`claimed_at` cleared.
**`attempt_count` is never reset** and the failure fields are retained — the
command grants another claim, it does not erase history. Each row gets one
`operator_requeued` event recording prior status, prior kind/reason and attempt
count, attributed as `operator:<hostname>-<pid>`. Zero matches exits 0 with an
explicit `0 rows matched`.

**Arithmetic, because it bites.** Requeueing does not by itself buy a *retried*
attempt. For a row at `attempt_count = A`, the next claim bumps it to `A + 1`,
and in-pipeline retry only fires while `attempt_count < retries + 1` — so an
automatic retry after the forced fetch needs `--retries > A` **strictly**.
A row exhausted at `A = 3` under `--retries 2` gets exactly one forced attempt
unless the following `process` runs with `--retries 4` or higher.

```sh
# Preview the cap-exhausted cookie-gated cohort, then move 500 of them:
ddp-transcribe requeue-failures --error-kind SensitiveLoginGated --dry-run
ddp-transcribe requeue-failures --error-kind SensitiveLoginGated --max 500
ddp-transcribe process --cookies-file ~/tiktok-cookies.txt --retries 4
```

Hand-written SQL against `videos` is unsupported emergency repair — it mutates
status without leaving the `video_events` row every audit depends on. This
subcommand exists so you never need it (ADR-0046).

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
  lib.rs              # the crate's single module root + the four-name public facade
  main.rs             # thin binary: parse, tracing init, dispatch, the one process::exit
  commands.rs         # subcommand dispatch (one arm per subcommand) -> CommandExit
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

`lib.rs` is the only place modules are declared; `main.rs` carries no `mod`
line at all and reaches library code through exactly four names — `Cli`,
`LogFormat`, `dispatch`, `CommandExit`. Everything else is `pub(crate)` or
private, and the library never calls `process::exit`: a subcommand returns a
`CommandExit` and `main` performs the exit. See ADR-0045.

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
