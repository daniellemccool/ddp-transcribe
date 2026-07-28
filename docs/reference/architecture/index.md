# ddp-transcribe — architecture

Onboarding reference for the ddp-transcribe pipeline (formerly `uu-tiktok`; see [ADR 0031](../../madr-archive/0031-rename-uu-tiktok-to-ddp-transcribe-surface-level-generalization-for-multi-study-reuse.md)). Start here.

## 1. What this system is and who it serves

ddp-transcribe is a research pipeline that ingests a TikTok user's Data Donation Programme (DDP) export, fetches each video the donor watched, and transcribes the audio using `whisper.cpp`. The output is a directory of transcript artifacts — a plain-text `.txt` and a JSON `.json` per watched video — with raw confidence signals preserved in the JSON (per [ADR 0010](../../decisions/0010-raw-signals-passes-whisper-confidence-signals-through-raw.md)) so downstream researchers can apply their own quality thresholds.

**Who's who:**

- **Donor** — the TikTok user who exported their DDP and shared it with the research project.
- **Researcher** — the consumer of the transcript artifacts; not a runtime participant in this pipeline.
- **DDP (Data Donation Programme)** — TikTok's user-data export, the input to ingest.

**Explicitly out of scope:** no UI, no scheduler, no multi-tenant story. The pipeline is a CLI tool that runs against one donor's DDP at a time, on a single dev workspace (see [ADR 0011](../../madr-archive/0011-spin-down-operational-practice-for-dev-workspace.md) for the dev-workspace operational practice).

## 2. Glossary

Alphabetical. Each entry: 1-2 sentence definition + the file where the concept is defined or implemented.

- **artifact** — the transcript files written to disk by the output writer; each watched video produces a `.txt` (plain transcript) and a `.json` (metadata + raw signals). Shape and schema in `src/output/`.
- **claim** — an exclusive lock on a state row taken by a worker before processing it. Arbitrated by sqlite `BEGIN IMMEDIATE` (see [ADR 0026](../../decisions/0026-workers-drain-and-exit-on-claim-next-none-no-polling.md)). Defined in `src/state/mod.rs`.
- **classification table** — the ordered, first-match-wins TOML rule table (Epic 4a, [ADR 0037](../../decisions/0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)) that maps yt-dlp stderr to a label + disposition (`retryable` / `terminal` / `requires-cookie`). Operator-editable via `--classification`, with an evidence-derived compiled default. Defined in `src/classification.rs`; the active table's TOML is snapshotted per batch into `batch_runs.policy_toml`.
- **DDP (Data Donation Programme)** — TikTok's user-data export bundle. The input format the pipeline parses at ingest. See `src/ingest.rs`.
- **donor** — the TikTok user whose DDP has been ingested into the pipeline.
- **engine state** — a `whisper_state` value (per-inference scratch space, distinct from the model context); the internal unit of inference concurrency, kept inside the worker behind the `WhisperEngine` wrapper. Per [ADR 0016](../../decisions/0016-engine-api-stays-stable-across-single-and-multi-state-internals.md), the engine API is designed to stay stable whether it drives one state or many.
- **hound** — Rust WAV-file library. Used for PCM I/O at the audio-prep boundary. See `src/audio.rs`.
- **lifecycle state** — the column in the state table recording a row's current status: `pending`, `in_progress`, `succeeded`, `failed_retryable`, `failed_terminal`. Definition in `src/state/`.
- **load-metadata** — the post-run subcommand (Epic 4c) that parses stored metadata envelopes into the typed schema-v5 columns on `videos`. Streaming, batched, idempotent, replayable; `--dry-run` reports counts without writing. Defined in `src/metadata_loader.rs`.
- **mark_succeeded** — the state mutator that flips a row from `in_progress` to `succeeded`, conditional on the caller's claim still being live (per [ADR 0008](../../decisions/0008-artifacts-are-durable-on-disk-before-mark-succeeded.md)). Defined in `src/state/mod.rs`.
- **metadata envelope** — the versioned wrapper `{"schema":1,"printed":"<yt-dlp --print line>"}` that the fetcher stores **unparsed** in `video_metadata_raw` on every fetch (Epic 4c, [ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)). `schema` is the loader's compatibility gate. Built in `src/fetcher/ytdlp.rs`, carried as `MetadataCapture` (`src/fetcher/mod.rs`).
- **mpsc payload** — the `FetchedItem` struct sent from fetch workers to the transcribe worker over the bounded mpsc channel: `claim`, `samples` (`Vec<f32>`), `samples_len`, `wav_path`, and `fetcher_name` (`src/pipeline/pipelined.rs:65`). Extends the `(Claim, Vec<f32>, PathBuf)` triple named in [ADR 0027](../../decisions/0027-orchestrator-topology-3-fetch-workers-feed-1-transcribe-worker-over-a-capacity-2-channel.md) with `samples_len` and `fetcher_name`.
- **record_fetch_failure** — the Epic 4a state mutator that makes the in-pipeline retry decision (requeue / exhaust / park) in one transaction at failure time, replacing the Epic 3 `mark_retryable_failure` in every pipeline caller (per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)). Defined in `src/state/mod.rs`.
- **retryable failure** — a failure that may succeed on a future attempt (e.g., network timeout). Distinct from terminal failure. Routed by `record_fetch_failure`, which requeues the row to `pending` under the lifetime attempt cap or parks/exhausts it in `failed_retryable` (per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)).
- **rusqlite** — Rust bindings to sqlite, with the `bundled` feature. The state machine is implemented on top of these.
- **stale claim** — a claim row whose `claimed_at` is older than the configured threshold and whose owner is presumed gone (crash, kill -9). Cleaned up by the stale-claim sweep per [ADR 0024](../../decisions/0024-stale-claim-sweep-recovers-rows-blind-no-validation-no-attempt-bump.md).
- **start-of-batch sweep** — the Epic 4a pass (`batch::run_sweep`, `src/batch.rs`) that adjudicates every parked `failed_retryable` row through the classification table before the drain begins: terminal classes write off (`sweep_mark_terminal`), retryables and the cookie pool requeue under the cap (`sweep_requeue`). Replaces the retired operator `triage` subcommand (per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)). Distinct from the stale-claim sweep.
- **terminal failure** — a failure that will not succeed on retry (e.g., video deleted upstream). Recorded with `mark_terminal_failure` (inline pipeline write-off per [ADR 0033](../../decisions/0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md)) or `sweep_mark_terminal` (start-of-batch sweep write-off per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)), both setting `status = 'failed_terminal'` — see [`state-machine.md`](state-machine.md).
- **watched video** — a single TikTok video the donor watched, listed in their DDP. One row in state per video.
- **whisper-rs** — Rust bindings to `whisper.cpp`. Pinned to a specific version per [ADR 0009](../../decisions/0009-whisper-cpp-embeds-via-pinned-whisper-rs-crate-and-upstream-commit-bump-together.md). Used in `src/transcribe.rs`.
- **whisper.cpp** — Georgi Gerganov's C++ Whisper implementation. The transcription engine, embedded via `whisper-rs`. Internals covered in `docs/reference/whisper-cpp-deepdive.md`.
- **yt-dlp** — Python tool used to download TikTok videos. Invoked as a subprocess by the fetcher. See `src/fetcher/`.

## 3. The donor's journey

A single donor's DDP export becomes a directory of transcript artifacts. Five stages thread through the four deepdive files.

**Stage 1 — Ingest.** The operator runs `ddp-transcribe ingest <inbox>` against a directory of DDP JSON files, optionally with `--window-start`/`--window-end` (inclusive UTC calendar dates; Epic 4b, [ADR 0040](../../decisions/0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md)). `src/ingest.rs` walks every `*.json` file, derives the respondent ID from the filename's `participant=` segment, and deserialises the `tiktok_watch_history` sections via serde_json. Each entry's URL is classified by `src/canonical.rs` into a `Canonical` variant; short links and invalid URLs are skipped with a counter increment, as are entries with unparseable dates. Valid entries become rows in the `videos` table (status `pending`) and `watch_history` via `INSERT OR IGNORE` — so re-running ingest against the same export is safe. Each `watch_history` row gets an `in_window` flag computed from the supplied window bounds (both absent = everything in-window) and the verbatim DDP `Date` string preserved in `watched_at_raw` (schema v4) — the hedge against [ADR 0039](../../decisions/0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md)'s unresolved timezone verdict. Re-ingest backfills NULL `watched_at_raw` on existing rows but never touches `in_window`; only the explicit `recompute-window` subcommand changes it after ingest. The operator sees a skip/duplicate summary at the end. → see [`data-input.md`](data-input.md).

**Stage 2 — Claim.** At orchestrator startup, `Store::sweep_stale_claims` (`src/state/mod.rs`) resets any `in_progress` row whose `claimed_at` is older than the 30-minute threshold back to `pending` — no attempt-count bump and no audit-log row. Three fetch workers then each call `Store::claim_next` (`src/state/mod.rs`), which opens a `BEGIN IMMEDIATE` transaction, selects the next `pending` row (`attempt_count ASC, first_seen_at ASC, video_id ASC` since Epic 4a, per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md) — fresh work first, requeued retries at the end of the queue, FIFO within each attempt tier), flips it to `in_progress` with the worker's ID in `claimed_by`, and returns a `Claim`. The `BEGIN IMMEDIATE` serialisation makes double-claiming structurally impossible; workers that see `claim_next` return `None` exit immediately (drain semantics — no polling). → see [`state-machine.md`](state-machine.md) and [`orchestration.md`](orchestration.md).

**Stage 3 — Fetch and transcribe.** Each fetch worker calls `fetch_and_decode` (`src/pipeline/mod.rs`): `src/fetcher/ytdlp.rs` invokes `yt-dlp` with `--audio-format wav --postprocessor-args "ffmpeg:-ar 16000 -ac 1 …"`, so the artifact on disk is already a 16 kHz mono `pcm_s16le` WAV by the time `acquire` returns. `src/audio.rs` then opens the WAV with `hound`, validates sample rate and channel count, and decodes the `i16` samples to `Vec<f32>` (dividing by 32768.0) — it does not resample or mix down. The fetch worker packages the result as a `FetchedItem` (`src/pipeline/pipelined.rs`) — carrying `claim`, `samples`, `samples_len`, `wav_path`, and `fetcher_name` — and sends it over the bounded mpsc channel (capacity 2) to the single transcribe worker. That worker runs inference via the embedded `whisper-rs` context (`src/transcribe.rs`), which holds model weights in a `WhisperContext` loaded once at startup and reuses per-inference scratch via `WhisperState`. The same yt-dlp invocation also carries `--no-simulate --print <template>`, so `acquire` returns a metadata envelope alongside its outcome and the worker writes it to `video_metadata_raw` **before** dispatching on success or failure — zero extra network requests, and metadata survives a fetch that died mid-transfer (Epic 4c, [ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)). → see [`data-input.md`](data-input.md) and [`transcription.md`](transcription.md).

**Stage 4 — Persist.** The transcribe worker writes two files atomically: `{transcripts_root}/{shard}/{video_id}.txt` (plain transcript) and `{transcripts_root}/{shard}/{video_id}.json` (metadata + raw signals), where `{shard}` is the last two digits of the video ID (per [ADR 0004](../../decisions/0004-transcript-output-shards-by-the-last-two-digits-of-the-video-id.md)). Each file goes through `artifacts::atomic_write` — written to a per-process-unique `.tmp-{pid}-{seq}` path, fsynced, and renamed — making the write durable before the rename completes. Since Epic 4c the phase is a pair (`src/pipeline/mod.rs`): `write_artifacts_durable` does the writes and fsyncs with no `Store` involvement, then `mark_after_artifacts` calls `Store::mark_succeeded` (`src/state/mod.rs`), which checks `AND claimed_by = ?` before flipping `in_progress` to `succeeded`. The pipelined worker calls the halves directly so its store lock covers only the DB acknowledgement; the serial path calls `write_artifacts_and_mark`, the composition of the two. A crash between artifact-write and `mark_succeeded` leaves the row in `in_progress` for the stale sweep to recover; the re-attempt overwrites the artifact idempotently (per [ADR 0008](../../decisions/0008-artifacts-are-durable-on-disk-before-mark-succeeded.md)). → see [`transcription.md`](transcription.md) and [`state-machine.md`](state-machine.md).

**Stage 5 — Failure paths and automatic in-batch retry.** Every worker error runs through the classifier (`src/failure.rs` over the active classification table, per [ADR 0033](../../decisions/0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md) + [ADR 0037](../../decisions/0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)) into a three-arm verdict: retryable/requires-cookie failures route to `Store::record_fetch_failure`, which decides in one transaction (Epic 4a, [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)) whether to requeue the row to `pending` under the lifetime attempt cap, exhaust it, or park it awaiting cookies; the proven-dead classes route to `Store::mark_terminal_failure` inline; Bug-class errors abort the run via the cancellation token. **There is no operator triage step** — retry is pipeline behavior. A `process` run first opens a `batch_runs` row, then runs a **start-of-batch sweep** (`src/batch.rs`) that adjudicates every parked `failed_retryable` row through the classification table (dead classes → `failed_terminal` via `sweep_mark_terminal`; retryables and the cookie pool → `pending` via `sweep_requeue`), then drains — fresh work first, requeued retries behind it (`claim_next` orders `attempt_count ASC`) — and closes the `batch_runs` row with a durable census. The re-fetch itself is the liveness oracle, so the oEmbed probe and `triage` subcommand of Epic 3 retired (ADR 0036 supersedes [ADR 0034](../../madr-archive/0034-operator-triage-subcommand-oembed-oracle-via-curl-subprocess-message-class-fast-path-attempt-capped-requeue.md)). Requeued `SensitiveLoginGated` rows are fetched with cookies via `process --cookies-file` ([ADR 0035](../../decisions/0035-cookies-ride-only-sensitivelogingated-retries-with-argv-redaction.md)). Stale claims (from a `kill -9` or kernel OOM) are handled by the startup stale-sweep described in Stage 2, which writes no audit row. For the clean-drain shutdown path and the error-triggered cancellation-token path that winds down the `JoinSet`, see [`orchestration.md`](orchestration.md). → see [`orchestration.md`](orchestration.md) and [`state-machine.md`](state-machine.md).

**Operator visibility (Epic 4b).** `ddp-transcribe status` (`src/status.rs`) is the read-only reporting surface over the state DB: counts by lifecycle status, `failed_retryable` by kind, in-progress claim ages, and full `batch_runs` history (an interrupted run's open row renders honestly rather than being skipped). Detail modes `--video-id`, `--respondent-id`, `--errors`/`--retryable`, and `--json` cover finer-grained operator questions ([ADR 0041](../../decisions/0041-status-is-the-read-only-operator-surface-the-0017-done-contract-lives-behind-verify.md)). `status --verify` runs the ADR-0017 done-contract — per-shard artifact existence, a full `raw_signals.schema_version` parse, and a pause-safe verdict — before an operator pauses the workspace (per [ADR 0011](../../madr-archive/0011-spin-down-operational-practice-for-dev-workspace.md)). `ddp-transcribe recompute-window` (`src/state/mod.rs`) is the only way to change `in_window` after ingest, and refuses to run bare ([ADR 0040](../../decisions/0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md)). → see [`state-machine.md`](state-machine.md).

**Post-run metadata load (Epic 4c).** `ddp-transcribe load-metadata` (`src/metadata_loader.rs`) is the only thing that parses the envelopes Stage 3 captured. It streams `video_metadata_raw` in keyset pages, parses each envelope, and writes the eight typed schema-v5 columns on `videos` one transaction per page; `--dry-run` reports real counts without writing. It is idempotent and replayable — a parse bug is fixed by re-running it, never by re-fetching, which is the whole point of storing the print line unparsed ([ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)). → see [`data-input.md`](data-input.md) and [`state-machine.md`](state-machine.md).

## 4. ADR map

Every ADR currently in `docs/decisions/`, grouped by the subsystem it governs. Cross-cutting ADRs (those that touch multiple subsystems) appear once under their primary group with a note.

### Meta-process / project conventions

| ADR | Title | Notes |
|-----|-------|-------|
| 0001 | Per-task file split for plans | Why this plan is a directory, not a single file. |
| 0002 | Dead-code suppression strategy | Build-time conventions. |
| 0003 | Test discipline + brief-deviation honesty | Commit-message conventions. |
| 0005 | `test-helpers` Cargo feature | Why integration tests need this feature flag. |
| 0007 | Stats structs use input-side counters | Reporting conventions. |
| 0018 | Three-tier review with codex-advisor | Code-review protocol. Architecture doc is not subject to this tier. |
| 0019 | Subagent report format + phase restart | Plan-execution conventions. |
| 0020 | FOLLOWUPS four-file split | How active follow-up debt is tracked. |

### Data input (ingest + fetcher)

| ADR | Title | Notes |
|-----|-------|-------|
| 0021 | Bounded subprocess output capture | Applies to fetcher's yt-dlp invocation. Also referenced by orchestration. |
| 0039 | DDP watch-history timestamps are UTC-assumed, documentary-only, empirically unresolved | `parse_watched_at`'s timezone verdict; `watched_at_raw` is the hedge. |
| 0042 | Fetch-time metadata is captured raw-first; parsing is a replayable post-run step | The `--print` capture chain, the unparsed envelope in `video_metadata_raw`, and `load-metadata`. Cross-cuts the state machine (schema v5, `upsert_metadata_raw` / `apply_metadata_batch`). |

### State machine

| ADR | Title | Notes |
|-----|-------|-------|
| 0006 | `Store` mutators return `Result<usize>` | Row-change-count contract. |
| 0022 | Schema-version policy | Hard-fail at `Store::open`; migrate via dedicated CLI. |
| 0023 | Minimum mutator signatures | `(kind, message)` returning `Result<usize>`. |
| 0024 | Stale-claim sweep | No validation, no attempt-count bump, 30-min default threshold. |
| 0026 | Claim contention via `BEGIN IMMEDIATE` | No polling; batch-drain on `claim_next` None. |
| 0034 | Operator triage subcommand (superseded by 0036) | Historical. The triage subcommand + oEmbed probe retired in Epic 4a. |
| 0036 | In-batch capped retry + end-of-queue claim ordering; fetcher is the liveness oracle | `record_fetch_failure`; `claim_next` `attempt_count ASC` ordering; start-of-batch sweep mutators + audit events; `batch_runs` lifecycle. Cross-cuts orchestration. |
| 0037 | Operator-editable TOML classification table + compiled default + batch provenance | Labels/dispositions written by the failure mutators; `batch_runs.policy_toml` snapshot. Cross-cuts orchestration (classifier dispatch). |
| 0040 | Analysis window is computed at ingest; `recompute-window` is the only flag mutator | `watch_history.in_window` + `watched_at_raw`; day-granularity absorbs 0039's unresolved ambiguity. |

### Transcription (audio + whisper-rs + output)

| ADR | Title | Notes |
|-----|-------|-------|
| 0004 | Transcript output sharding | Two-digit shard by video ID suffix. |
| 0008 | Artifact-before-mark_succeeded | Cross-cuts state machine. |
| 0009 | `whisper-rs` version pin + fallback policy | Embedding strategy. |
| 0010 | JSON artifact schema with raw signals pass-through | Output schema versioning. |
| 0012 | Cooperative cancellation via per-request `Arc<AtomicBool>` | Abort callback wiring. |
| 0013 | GPU verification at startup | Assert backend and log device name. |
| 0014 | Audio input invariant: float32 PCM 16kHz mono via hound | The format whisper.cpp requires. |
| 0015 | Explicit non-use of `whisper_full_parallel` | Why we drive parallelism via engine states instead. |
| 0016 | Engine API stable across single- and multi-state | Concurrency model. |

### Orchestration

| ADR | Title | Notes |
|-----|-------|-------|
| 0017 | Operational done contract for batch validation | Archived (MADR); superseded in practice by 0041's `status --verify`. |
| 0025 | JoinSet + CancellationToken shutdown order is load-bearing | The shutdown sequence. |
| 0027 | Orchestrator topology: n=3 fetch + 1 transcribe, mpsc capacity 2 | Channel shape. |
| 0033 | Evidence-derived failure taxonomy with inline write-off | Three-arm classifier dispatch in the workers. Cross-cuts state machine (kinds/reasons written to failure columns). |
| 0035 | Cookies scoped to SensitiveLoginGated retries with argv redaction | Kind-gated `cookie_opts_for` fetch opts. Cross-cuts data input (yt-dlp invocation). |
| 0041 | `status` is the read-only operator surface; the 0017 done-contract lives behind `--verify` | Counts/kinds/claim-ages/batch history; `--verify` pause-safe verdict, exit 1 on violation. Lean successor to archived 0017. |

### Operations (out of architecture-doc scope)

| ADR | Title | Notes |
|-----|-------|-------|
| 0011 | Spin-down operational practice for dev workspace | Dev-workspace lifecycle; lives outside the architecture doc. |
| 0032 | Transcription hot path on boot disk; volume seed-at-provision/sink-at-downtime | SRC batch-deployment storage model (1M-video target); outside the architecture doc. |

## 5. Where to look for X

| You want | Path |
|----------|------|
| Build the binary | `cargo build --features cuda` (see `Cargo.toml`) |
| Run the test suite | `cargo test --features test-helpers -- --test-threads=1` (per `CLAUDE.md`) |
| Active plans | `docs/superpowers/plans/` (latest: `ls docs/superpowers/plans/ \| sort \| tail -1`) |
| Decisions / rationale | `docs/decisions/` (lean ADRs managed via `adg lean`; MADR-era history in `docs/madr-archive/`) |
| Active follow-up debt | `docs/FOLLOWUPS.md` (per [ADR 0020](../../decisions/0020-followups-is-a-scope-index-over-per-epic-files-hypotheses-are-marked-unverified.md)) |
| Operational scripts | `scripts/` |
| Whisper.cpp internal reference | `docs/reference/whisper-cpp-deepdive.md` |
| This doc set's design rationale | `docs/superpowers/specs/2026-05-20-architecture-doc-set-design.md` |

## 6. How this doc is maintained

The architecture doc is updated *within an epoch*, not on every PR. Update triggers:

1. **New ADR added** → add a row to the ADR map in §4. Add an inline reference in a deepdive file *only* if the ADR's content is relevant to that subsystem.
2. **Subsystem code structure changes significantly** (file split, module rename, public-type reshape) → update the affected deepdive file's layout/key-types description. Not triggered by line-level changes.
3. **Integration surface changes** (yt-dlp flags change, `whisper-rs` version bumps, schema migration) → update the integration-depth section in `data-input.md` or `transcription.md`.
4. **Epic close** — when an epic affecting `state/` or `pipeline` closes, the corresponding in-flight stamp is removed and the relevant deepdive file gets a revision pass against the new code state.

Drift detection happens at planning time: when a new plan is written, the planner checks whether any architecture-doc-covered surface is touched, and if so, adds a "update `docs/reference/architecture/<file>.md`" task. The Sonnet spec-compliance reviewer per [ADR 0018](../../decisions/0018-task-reviews-are-three-tier-codex-advisor-is-called-by-the-reviewer-never-the-orchestrator.md) checks "did this plan touch architecture-doc surfaces?" during plan review.

The architecture doc itself is **not** subject to the codex-advisor / Sonnet review tier per [ADR 0018](../../decisions/0018-task-reviews-are-three-tier-codex-advisor-is-called-by-the-reviewer-never-the-orchestrator.md) — that tier governs code review. The architecture doc's reviewer is the human user.

### Writing conventions

- **ADR-redirect-first.** Where an ADR captures rationale, point at the ADR rather than restating it. The architecture doc owns the *what* (noun layer) and the *narrative* (donor's journey); ADRs own the *why*.
- **Citation style.** Inline `src/path/file.rs:N` for any specific behavioral claim. Line numbers drift; the file path stays valid.
- **In-flight stamp.** While an epic is actively reshaping the `state-machine.md` or `orchestration.md` subsystem, that file carries an "as of commit `<sha>`" stamp pointing at the active plan; the stamp is removed at epic close. Neither file carries one currently — Plan B Epic 4c (which last reshaped them: schema v5 metadata columns + `video_metadata_raw`, the `write_artifacts_durable` / `mark_after_artifacts` split) has closed.
- **Diagrams.** ASCII only. Currently two: a topology diagram in `orchestration.md`, a state-transition diagram in `state-machine.md`.
