# uu-tiktok — architecture

Onboarding reference for the uu-tiktok pipeline. Start here.

## 1. What this system is and who it serves

uu-tiktok is a research pipeline that ingests a TikTok user's Data Donation Programme (DDP) export, fetches each video the donor watched, and transcribes the audio using `whisper.cpp`. The output is a directory of JSON transcript artifacts — one per watched video — with raw confidence signals preserved (per [ADR 0010](../../decisions/0010-json-artifact-raw-signals-schema-pass-through-with-schema-version.md)) so downstream researchers can apply their own quality thresholds.

**Who's who:**

- **Donor** — the TikTok user who exported their DDP and shared it with the research project.
- **Researcher** — the consumer of the transcript artifacts; not a runtime participant in this pipeline.
- **DDP (Data Donation Programme)** — TikTok's user-data export, the input to ingest.

**Explicitly out of scope:** no UI, no scheduler, no multi-tenant story. The pipeline is a CLI tool that runs against one donor's DDP at a time, on a single dev workspace (see [ADR 0011](../../decisions/0011-spin-down-operational-practice-for-dev-workspace.md) for the dev-workspace operational practice).

## 2. Glossary

Alphabetical. Each entry: 1-2 sentence definition + the file where the concept is defined or implemented.

- **artifact** — a JSON transcript file written to disk by the output writer; one per watched video. Shape and schema in `src/output/`.
- **claim** — an exclusive lock on a state row taken by a worker before processing it. Arbitrated by sqlite `BEGIN IMMEDIATE` (see [ADR 0026](../../decisions/0026-claim-contention-no-polling-for-plan-b-batch-drain-on-claim-next-none.md)). Defined in `src/state/store.rs`.
- **DDP (Data Donation Programme)** — TikTok's user-data export bundle. The input format the pipeline parses at ingest. See `src/ingest.rs`.
- **donor** — the TikTok user whose DDP has been ingested into the pipeline.
- **engine state** — a `whisper_state` value (per-inference scratch space, distinct from the model context). Per [ADR 0016](../../decisions/0016-architecture-for-parallelism-engine-api-stable-across-single-and-multi-state.md), the engine API exposes states as the unit of concurrency.
- **hound** — Rust WAV-file library. Used for PCM I/O at the audio-prep boundary. See `src/audio.rs`.
- **lifecycle state** — the column in the state table recording a row's current status: `pending`, `in_progress`, `succeeded`, `retryable_failure`, `terminal_failure`. Definition in `src/state/`.
- **mark_succeeded** — the state mutator that flips a row from `in_progress` to `succeeded`, conditional on the caller's claim still being live (per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md)). Defined in `src/state/store.rs`.
- **mpsc payload** — the message type sent from fetch workers to the transcribe worker over the bounded mpsc channel: `(Claim, Vec<f32>, PathBuf)`. Per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md).
- **retryable failure** — a failure that may succeed on a future attempt (e.g., network timeout). Distinct from terminal failure. Recorded with `mark_retryable_failure` (per [ADR 0023](../../decisions/0023-minimum-mutator-signatures-kind-str-message-str-returning-result-usize-per-0006.md)).
- **rusqlite** — Rust bindings to sqlite, with the `bundled` feature. The state machine is implemented on top of these.
- **stale claim** — a claim row whose `claimed_at` is older than the configured threshold and whose owner is presumed gone (crash, kill -9). Cleaned up by the stale-claim sweep per [ADR 0024](../../decisions/0024-stale-claim-sweep-no-validation-no-attempt-count-bump-30-min-default-threshold.md).
- **terminal failure** — a failure that will not succeed on retry (e.g., video deleted upstream). Recorded with `mark_terminal_failure`. Per [ADR 0023](../../decisions/0023-minimum-mutator-signatures-kind-str-message-str-returning-result-usize-per-0006.md).
- **watched video** — a single TikTok video the donor watched, listed in their DDP. One row in state per video.
- **whisper-rs** — Rust bindings to `whisper.cpp`. Pinned to a specific version per [ADR 0009](../../decisions/0009-use-whisper-rs-for-whisper-cpp-embedding-with-version-pin-and-fallback-policy.md). Used in `src/transcribe.rs`.
- **whisper.cpp** — Georgi Gerganov's C++ Whisper implementation. The transcription engine, embedded via `whisper-rs`. Internals covered in `docs/reference/whisper-cpp-deepdive.md`.
- **yt-dlp** — Python tool used to download TikTok videos. Invoked as a subprocess by the fetcher. See `src/fetcher/`.

## 3. The donor's journey

A single donor's DDP export becomes a directory of transcript artifacts. Five stages thread through the four deepdive files. *(This section's full prose is written in T07 after the deepdives exist; stubs below.)*

**Stage 1 — Ingest.** The DDP export is parsed; rows are written into the state machine, one per watched video. → see [`data-input.md`](data-input.md).

**Stage 2 — Claim.** Orchestrator workers pull the next pending row, taking exclusive claim via `BEGIN IMMEDIATE`. → see [`state-machine.md`](state-machine.md) and [`orchestration.md`](orchestration.md).

**Stage 3 — Fetch and transcribe.** A claimed video is downloaded by the fetcher; its audio is extracted, normalized to 16kHz mono float32 PCM, and handed off to the transcriber. → see [`data-input.md`](data-input.md) and [`transcription.md`](transcription.md).

**Stage 4 — Persist.** The transcript artifact is written to disk *before* the state row is flipped to `succeeded`, so a crash mid-write leaves the row in `in_progress` for re-attempt rather than orphaning the artifact (per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md)). → see [`transcription.md`](transcription.md) and [`state-machine.md`](state-machine.md).

**Stage 5 — Failure paths.** Retryable failures (network blips, transient yt-dlp errors) are recorded with a retry-friendly state. Terminal failures (video deleted upstream) are recorded permanently. Stale claims (worker crashed mid-process) are swept back to `pending` after a threshold. → see [`orchestration.md`](orchestration.md) and [`state-machine.md`](state-machine.md).

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

### State machine

| ADR | Title | Notes |
|-----|-------|-------|
| 0006 | `Store` mutators return `Result<usize>` | Row-change-count contract. |
| 0022 | Schema-version policy | Hard-fail at `Store::open`; migrate via dedicated CLI. |
| 0023 | Minimum mutator signatures | `(kind, message)` returning `Result<usize>`. |
| 0024 | Stale-claim sweep | No validation, no attempt-count bump, 30-min default threshold. |
| 0026 | Claim contention via `BEGIN IMMEDIATE` | No polling; batch-drain on `claim_next` None. |

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
| 0017 | Operational done contract for batch validation | When a batch is "done." |
| 0025 | JoinSet + CancellationToken shutdown order is load-bearing | The shutdown sequence. |
| 0027 | Orchestrator topology: n=3 fetch + 1 transcribe, mpsc capacity 2 | Channel shape. |

### Operations (out of architecture-doc scope)

| ADR | Title | Notes |
|-----|-------|-------|
| 0011 | Spin-down operational practice for dev workspace | Dev-workspace lifecycle; lives outside the architecture doc. |

## 5. Where to look for X

| You want | Path |
|----------|------|
| Build the binary | `cargo build --features cuda` (see `Cargo.toml`) |
| Run the test suite | `cargo test --features test-helpers -- --test-threads=1` (per `CLAUDE.md`) |
| Active plans | `docs/superpowers/plans/` (latest: `ls docs/superpowers/plans/ \| sort \| tail -1`) |
| Decisions / rationale | `docs/decisions/` (managed via `scripts/adr`; see the `using-adg` skill) |
| Active follow-up debt | `docs/FOLLOWUPS.md` (per [ADR 0020](../../decisions/0020-followups-four-file-split-with-archive-at-epic-close-and-unverified-hypothesis-marking.md)) |
| Operational scripts | `scripts/` |
| Whisper.cpp internal reference | `docs/reference/whisper-cpp-deepdive.md` |
| This doc set's design rationale | `docs/superpowers/specs/2026-05-20-architecture-doc-set-design.md` |

## 6. How this doc is maintained

The architecture doc is updated *within an epoch*, not on every PR. Update triggers:

1. **New ADR added** → add a row to the ADR map in §4. Add an inline reference in a deepdive file *only* if the ADR's content is relevant to that subsystem.
2. **Subsystem code structure changes significantly** (file split, module rename, public-type reshape) → update the affected deepdive file's layout/key-types description. Not triggered by line-level changes.
3. **Integration surface changes** (yt-dlp flags change, `whisper-rs` version bumps, schema migration) → update the integration-depth section in `data-input.md` or `transcription.md`.
4. **Epic close** — when an epic affecting `state/` or `pipeline` closes, the corresponding in-flight stamp is removed and the relevant deepdive file gets a revision pass against the new code state.

Drift detection happens at planning time: when a new plan is written, the planner checks whether any architecture-doc-covered surface is touched, and if so, adds a "update `docs/reference/architecture/<file>.md`" task. The Sonnet spec-compliance reviewer per [ADR 0018](../../decisions/0018-three-tier-review-protocol-with-codex-advisor-delegated-via-sonnet-reviewer.md) checks "did this plan touch architecture-doc surfaces?" during plan review.

The architecture doc itself is **not** subject to the codex-advisor / Sonnet review tier per [ADR 0018](../../decisions/0018-three-tier-review-protocol-with-codex-advisor-delegated-via-sonnet-reviewer.md) — that tier governs code review. The architecture doc's reviewer is the human user.

### Writing conventions

- **ADR-redirect-first.** Where an ADR captures rationale, point at the ADR rather than restating it. The architecture doc owns the *what* (noun layer) and the *narrative* (donor's journey); ADRs own the *why*.
- **Citation style.** Inline `src/path/file.rs:N` for any specific behavioral claim. Line numbers drift; the file path stays valid.
- **In-flight stamp.** While an epic is actively reshaping the `state-machine.md` or `orchestration.md` subsystem, that file carries an "as of commit `<sha>`" stamp pointing at the active plan; the stamp is removed at epic close. Neither file carries one currently — Plan B Epic 2 (which last reshaped them) has closed.
- **Diagrams.** ASCII only. Currently two: a topology diagram in `orchestration.md`, a state-transition diagram in `state-machine.md`.
