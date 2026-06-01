# uu-tiktok — state machine

The state machine is the durable record of the pipeline's progress. It lives in a sqlite database (one row per watched-video to process) and arbitrates between concurrent orchestrator workers via row-level claim contention.

## Schema and lifecycle states

The state machine is a single sqlite database accessed via the `Store` type in `src/state/`. Each watched video produces one row in the primary `videos` table; the row's `status` column tracks where it sits in the lifecycle. Schema version is recorded in a `meta` table; mismatches fail closed at `Store::open` per [ADR 0022](../../decisions/0022-schema-version-policy-hard-fail-at-store-open-migrate-via-dedicated-cli-subcommand.md) — there is no in-process auto-migrate.

### Schema overview

The schema is declared in `src/state/schema.rs`. Three application tables and one metadata table are created:

**`videos`** — one row per distinct `video_id`; the lifecycle row.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `video_id` | TEXT PK | NOT NULL | 19-digit TikTok video ID |
| `source_url` | TEXT | NOT NULL | Raw URL from the DDP export |
| `canonical` | INTEGER | NOT NULL | Boolean (1 = canonical URL form) |
| `status` | TEXT | NOT NULL | Constrained to the five values below |
| `claimed_by` | TEXT | NULL | Worker ID string while `in_progress` |
| `claimed_at` | INTEGER | NULL | Unix epoch seconds of claim |
| `attempt_count` | INTEGER | NOT NULL | Incremented by `claim_next` on each claim |
| `succeeded_at` | INTEGER | NULL | Unix epoch seconds of success |
| `duration_s` | REAL | NULL | Audio duration (written on success) |
| `language_detected` | TEXT | NULL | whisper.cpp detected language |
| `fetcher` | TEXT | NULL | Fetcher kind tag (written on success) |
| `transcript_source` | TEXT | NULL | Transcript provenance tag |
| `last_retryable_kind` | TEXT | NULL | Short tag from last retryable failure |
| `last_retryable_message` | TEXT | NULL | Error message from last retryable failure |
| `terminal_reason` | TEXT | NULL | Reason tag from terminal failure |
| `terminal_message` | TEXT | NULL | Error message from terminal failure |
| `first_seen_at` | INTEGER | NOT NULL | Unix epoch seconds of first ingest |
| `updated_at` | INTEGER | NOT NULL | Unix epoch seconds of last status change |

(`src/state/schema.rs:4–30`)

**`watch_history`** — one row per `(respondent_id, video_id, watched_at)` tuple; links a donor participant to their watch events. References `videos` via foreign key. (`src/state/schema.rs:36–44`)

**`video_events`** — append-only audit log; one row per claim/success/failure transition (`claimed`, `succeeded`, `failed_retryable`, `failed_terminal`). The stale-sweep recovery (`in_progress`→`pending`) writes no event. References `videos`. (`src/state/schema.rs:46–55`)

**`meta`** — key/value table holding `schema_version`. (`src/state/schema.rs:57–62`)

The schema currently ships at version `"2"` (`src/state/schema.rs:1`; `SCHEMA_VERSION` constant). A partial index on `(status, first_seen_at, video_id) WHERE status = 'pending'` accelerates `claim_next`'s FIFO scan (`src/state/schema.rs:32–34`).

### Lifecycle states

The `status` column is CHECK-constrained to five string values (`src/state/schema.rs:11–13`):

- **`pending`** — newly ingested (or stale-sweep recovered); eligible for claim by `claim_next`.
- **`in_progress`** — actively claimed by a worker; not eligible for claim until the worker finishes or the stale-claim sweep recovers it.
- **`succeeded`** — terminal success; transcript artifact on disk, never re-attempted.
- **`failed_retryable`** — a recoverable error was recorded by `mark_retryable_failure`; `last_retryable_kind` and `last_retryable_message` are populated. **Currently a sink**: `claim_next` only selects `pending` rows (see diagram below), and no automated reset-to-`pending` path is implemented; that is the Epic 3 retry-policy charter.
- **`failed_terminal`** — a non-recoverable failure was recorded by `mark_terminal_failure`; `terminal_reason` and `terminal_message` are populated. Sink; not re-attempted.

### State-transition diagram

Edges are drawn only for transitions that exist in current code; the `failed_retryable` sink is documented explicitly.

```
                              +------------+
              ingest          |            |  claim_next
              ------->        |  pending   |  (BEGIN IMMEDIATE,
                       +----> |            |  WHERE status='pending')
                       |      +-----+------+
                       |            |
                       |            | claim taken; attempt_count++
   stale sweep         |            v
   (per 0024:          |      +------------+
    in_progress        |      | in_progress|
    -> pending)        +------+            |
                              +-----+------+
                                    |
          mark_succeeded /  mark_retryable_failure /  mark_terminal_failure
                  |                 |                 |
                  v                 v                 v
          +------------+  +-------------------+  +------------------+
          | succeeded  |  | failed_retryable  |  | failed_terminal  |
          | (terminal) |  | (sink; Epic 3     |  | (terminal, sink; |
          +------------+  |  adds retry       |  |  no caller until  |
                          |  policy)          |  |  Epic 3)          |
                          +-------------------+  +------------------+
```

Key code references:
- `claim_next` WHERE clause: `WHERE status = 'pending'` (`src/state/mod.rs:237–239`)
- `sweep_stale_claims` WHERE clause: `WHERE status = 'in_progress' AND claimed_at IS NOT NULL AND claimed_at < ?` (`src/state/mod.rs:501–503`)
- `mark_retryable_failure` sets `status = 'failed_retryable'` (`src/state/mod.rs:372`)
- `mark_terminal_failure` sets `status = 'failed_terminal'` (`src/state/mod.rs:437`); `#[allow(dead_code)]` — no caller on current main, wired in Epic 3

## Claim contention

Multiple fetch workers (three by default per [ADR 0027](../../decisions/0027-orchestrator-topology-n-3-fetch-1-transcribe-mpsc-payload-claim-vec-f32-pathbuf-capacity-2.md)) all call `claim_next` concurrently. Per [ADR 0026](../../decisions/0026-claim-contention-no-polling-for-plan-b-batch-drain-on-claim-next-none.md), the contention strategy uses SQLite's `BEGIN IMMEDIATE` to serialize writers.

`claim_next` opens an `Immediate` transaction (`src/state/mod.rs:230–232`), selects the oldest `pending` row (FIFO by `first_seen_at, video_id`), and in the same transaction updates it to `in_progress` with `claimed_by` set to the `worker_id` string passed in. Because SQLite serializes `BEGIN IMMEDIATE` writers at the WAL level, no two workers can execute this select-then-update simultaneously — double-claiming is structurally impossible.

`claim_next` returns `Option<Claim>`. `None` means no claimable rows exist right now; per ADR 0026, the orchestrator does not poll on `None` — it drains the batch and exits. (`src/state/mod.rs:245–248`)

The `claimed_by` field is set to the calling worker's `worker_id` string (a UUID-like identifier passed in by the orchestrator, `src/state/mod.rs:258`). All subsequent mutators (`mark_succeeded`, `mark_retryable_failure`, `mark_terminal_failure`) include `AND claimed_by = ?` in their WHERE predicates, so a swept row is never double-updated.

## Stale-claim sweep

Per [ADR 0024](../../decisions/0024-stale-claim-sweep-no-validation-no-attempt-count-bump-30-min-default-threshold.md), `Store::sweep_stale_claims(threshold)` resets rows stuck in `in_progress` back to `pending` after `threshold` has elapsed since `claimed_at`. The default threshold is 30 minutes, set in `src/config.rs:52` (`Duration::from_secs(30 * 60)`).

The sweep runs once at orchestrator startup — at the top of both `run_serial` (`src/pipeline/serial.rs:34`) and `run_pipelined` (`src/pipeline/pipelined.rs:504–505`) — before any `claim_next` call.

The sweep does **not** bump `attempt_count` and does **not** validate that the worker actually died (ADR 0024). It uses a simple time-based predicate: `claimed_at < (now - threshold_secs)` (`src/state/mod.rs:501–503`). It also writes **no** `video_events` audit row — unlike `claim_next`, `mark_succeeded`, `mark_retryable_failure`, and `mark_terminal_failure` (the only sites that insert into `video_events`: `src/state/mod.rs:263`, `:329`, `:388`, `:453`), a swept recovery leaves no audit-log entry. The motivating failure mode is a `kill -9` on the orchestrator process, which leaves rows in `in_progress` indefinitely; without the sweep, those rows block all future progress.

Redirect to ADR 0024 for the rationale against validation and attempt-count bumps.

## Mutator contracts

Per [ADR 0006](../../decisions/0006-store-mutators-return-result-of-usize-for-row-change-count.md) and [ADR 0023](../../decisions/0023-minimum-mutator-signatures-kind-str-message-str-returning-result-usize-per-0006.md), all `Store` mutators return `Result<usize>` where the inner value is the SQLite row-change count. A return of `0` means the WHERE predicate did not match — the claim was stale (swept or re-assigned) — without requiring a separate query.

Current mutators on `Store`:

- **`claim_next(worker_id: &str) -> Result<Option<Claim>>`** — special; not a row-change mutator. Opens a `BEGIN IMMEDIATE` transaction, selects the oldest `pending` row, updates it to `in_progress`, and returns the claim. Returns `None` if no `pending` rows exist. (`src/state/mod.rs:226–274`)

- **`mark_succeeded(video_id, worker_id, artifacts) -> Result<usize>`** — flips `in_progress → succeeded`; writes transcript metadata columns. WHERE predicate: `status = 'in_progress' AND claimed_by = ?`. Returns `0` if the claim was stale (per ADR 0008, artifacts are already durable on disk before this call; `0` is survivable). (`src/state/mod.rs:286–337`)

- **`mark_retryable_failure(video_id, worker_id, kind: &str, message: &str) -> Result<usize>`** — flips `in_progress → failed_retryable`; writes `last_retryable_kind` and `last_retryable_message`. Same stale-claim predicate. `kind` is a stable short tag (`"Fetch"`, `"Transcribe"`); Epic 3's typed `RetryableKind` enum serializes into the same columns. (`src/state/mod.rs:356–395`)

- **`mark_terminal_failure(video_id, worker_id, reason: &str, message: &str) -> Result<usize>`** — flips `in_progress → failed_terminal`; writes `terminal_reason` and `terminal_message`. Same stale-claim predicate. **No caller on current main** (`#[allow(dead_code)]`, `src/state/mod.rs:420`); the mutator surface was landed in Epic 2 so Epic 3's classifier dispatch is an add-caller task, not an add-mutator task. See also [`data-input.md`](data-input.md) §Fetch error surface. (`src/state/mod.rs:421–460`)

- **`sweep_stale_claims(threshold: Duration) -> Result<usize>`** — resets stale `in_progress` rows to `pending`. Returns the count of recovered rows. (`src/state/mod.rs:484–512`)

Redirect signature rationale to ADRs 0006 and 0023.

## Schema-version policy

`Store::open` reads `meta.schema_version` and compares it against the binary's `SCHEMA_VERSION` constant (currently `"2"`, `src/state/schema.rs:1`). On mismatch it returns `StateError::SchemaVersionMismatch { expected, found }` whose `Display` impl instructs the operator to run `uu-tiktok migrate` (`src/state/mod.rs:43–48`). There is no in-process auto-migrate.

Migration runs in the dedicated `migrate` subcommand (`src/state/migrate.rs`), which bypasses the version check, applies the migration SQL in a single transaction, and bumps `meta.schema_version`. The current migration ladder handles the v1 → v2 upgrade (adding the four failure-classification columns via `ALTER TABLE`; `src/state/migrate.rs:68–75`). Per [ADR 0022](../../decisions/0022-schema-version-policy-hard-fail-at-store-open-migrate-via-dedicated-cli-subcommand.md), failing closed and asking for human action is the correct default.

## Crash-recovery durability

Per [ADR 0008](../../decisions/0008-pipeline-writes-transcript-artifacts-before-mark-succeeded-for-crash-recovery-durability.md), the transcript artifact is written to disk **before** `mark_succeeded` is called. Three crash cases:

- **Crash after artifact write, before `mark_succeeded`** — the row stays `in_progress`; stale sweep recovers it to `pending`; the next attempt re-writes the artifact (idempotent overwrite) and calls `mark_succeeded`.
- **Crash before artifact write** — the row stays `in_progress`; stale sweep recovers it; the next attempt re-runs the full fetch + transcribe.
- **Crash after `mark_succeeded`** — fully durable; both artifact and state row are committed.

The artifact-write ordering is enforced in `write_artifacts_and_mark` in `src/pipeline/mod.rs` (the writes land before `mark_succeeded`; `transcribe_and_write` wraps it on the serial path). Full discussion of the artifact side of this invariant is in [`transcription.md`](transcription.md). Redirect crash-recovery rationale to ADR 0008.

## Failure classification

Failures are classified into retryable or terminal at the state-machine surface via two mutators:

- **`mark_retryable_failure`** writes the `last_retryable_kind` and `last_retryable_message` columns.
- **`mark_terminal_failure`** writes the `terminal_reason` and `terminal_message` columns.

(`src/state/schema.rs:23–27` — all four columns verified in schema)

On current main (post-Epic 2), the classifier is **string-kind only**: every error from the fetch path is passed to `mark_retryable_failure` with the literal placeholder kind `"Fetch"`, and every transcription error with `"Transcribe"`. `mark_terminal_failure` has no caller — the mutator surface exists but dispatch logic does not (`src/state/mod.rs:420`). A richer typed taxonomy (`RetryableKind`, `UnavailableReason`, `ClassifiedFailure`) and variant-driven routing are the Epic 3 charter. See [`orchestration.md`](orchestration.md) for the caller's perspective on failure handling.

The two diagnostic columns written by each mutator are preserved across subsequent flips: `mark_retryable_failure` does not clear `terminal_reason`/`terminal_message`, and `mark_terminal_failure` does not clear `last_retryable_*` — so an operator inspecting any row sees both the most recent retryable and the most recent terminal diagnostics (the full per-transition history lives in `video_events`). (`src/state/mod.rs:347–352`, `src/state/mod.rs:414–419`)

## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0006 | `Store` mutators return `Result<usize>` | Mutator return contract. |
| 0008 | Artifact-before-`mark_succeeded` | Durability ordering. Cross-cuts transcription. |
| 0022 | Schema-version policy | `Store::open` hard-fail + `migrate` subcommand. |
| 0023 | Minimum mutator signatures | `mark_retryable_failure` / `mark_terminal_failure` signatures. |
| 0024 | Stale-claim sweep | `sweep_stale_claims` semantics. |
| 0026 | Claim contention via `BEGIN IMMEDIATE` | `claim_next` serialization. |
