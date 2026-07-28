# ddp-transcribe — state machine

The state machine is the durable record of the pipeline's progress. It lives in a sqlite database (one row per watched-video to process) and arbitrates between concurrent orchestrator workers via row-level claim contention.

## Schema and lifecycle states

The state machine is a single sqlite database accessed via the `Store` type in `src/state/`. Each watched video produces one row in the primary `videos` table; the row's `status` column tracks where it sits in the lifecycle. Schema version is recorded in a `meta` table; mismatches fail closed at `Store::open` per [ADR 0022](../../decisions/0022-schema-version-hard-fails-at-store-open-migration-is-an-explicit-cli-subcommand.md) — there is no in-process auto-migrate.

### Schema overview

The schema is declared in `src/state/schema.rs`. Five application tables and one metadata table are created:

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
| `video_description` | TEXT | NULL | Creator caption text (schema v5; `load-metadata` writes it) |
| `uploader` | TEXT | NULL | Uploader handle (schema v5) |
| `uploader_id` | TEXT | NULL | Uploader numeric ID (schema v5) |
| `video_created_at` | INTEGER | NULL | Video upload time, Unix epoch (schema v5) |
| `view_count` | INTEGER | NULL | Point-in-time view count (schema v5) |
| `like_count` | INTEGER | NULL | Point-in-time like count (schema v5) |
| `comment_count` | INTEGER | NULL | Point-in-time comment count (schema v5) |
| `metadata_fetched_at` | INTEGER | NULL | When the envelope behind the seven columns above was captured (schema v5) |
| `first_seen_at` | INTEGER | NOT NULL | Unix epoch seconds of first ingest |
| `updated_at` | INTEGER | NOT NULL | Unix epoch seconds of last status change |

The eight schema-v5 columns are all nullable and are written **only** by `load-metadata`; NULL means the loader has not run over that video's envelope (or the envelope was unparseable). The engagement counts are snapshots as of `metadata_fetched_at`, not current values — see [ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md). (`src/state/schema.rs`)

**`watch_history`** — one row per `(respondent_id, video_id, watched_at)` tuple; links a donor participant to their watch events. `watched_at` is the parsed Unix-epoch timestamp (per [ADR 0039](../../decisions/0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md), UTC-assumed, documentary evidence, empirically unresolved); `watched_at_raw` (schema v4, Epic 4b) preserves the verbatim DDP `Date` string as the hedge against that unresolved verdict. `in_window` (boolean) records whether the row falls inside the analysis window supplied at ingest — computed once at ingest and changed only by the `recompute-window` subcommand (per [ADR 0040](../../decisions/0040-analysis-window-is-computed-at-ingest-recompute-window-is-the-only-flag-mutator.md)); absent bounds at ingest time mean every row lands `in_window = 1`. References `videos` via foreign key. (`src/state/schema.rs:36–44`)

**`video_events`** — append-only audit log; one row per lifecycle transition. Event types: `claimed`, `succeeded`, `failed_retryable`, `failed_terminal`, plus the Epic 4a in-pipeline retry types `retry_requeued` and `cookie_parked` (written by `record_fetch_failure` at failure time) and the start-of-batch sweep types `swept_terminal` and `requeued` (written with `worker_id = 'sweep'` per [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)). The stale-sweep recovery (`in_progress`→`pending`) writes no event. References `videos`. (`src/state/schema.rs`)

**`meta`** — key/value table holding `schema_version`. (`src/state/schema.rs`)

**`batch_runs`** — one row per `process` invocation (Epic 4a): the durable census and its generating policy. Opened at batch start with the params JSON and the active classification TOML (`policy_toml`), closed at batch end with the census JSON. A census without its policy is not reproducible attrition documentation, so both ride in the same row. (`src/state/schema.rs`)

**`video_metadata_raw`** — one row per unique `video_id` (PK), holding `fetched_at` and the fetch-time metadata envelope `raw_json` verbatim (schema v5, Epic 4c). Written by `upsert_metadata_raw` on every fetch that produced a print line — success and classified-failure alike — with last-write-wins across retries. Nothing parses it at runtime; `load-metadata` reads it after the batch. References `videos` via foreign key. (`src/state/schema.rs`)

**`ingested_files`** — file-level ingest ledger (schema v6), one row per successfully ingested inbox file keyed by **basename** (the inbox directory may move between hosts), holding `size_bytes`, `mtime`, and `ingested_at`. The row is upserted inside the same per-file transaction that commits that file's `videos`/`watch_history` rows, so it exists iff the data is committed; a later ingest skips any file whose `(name, size, mtime)` triple still matches, instead of re-walking millions of no-op upserts. See [`data-input.md`](data-input.md) §Parsing strategy. (`src/state/schema.rs`)

The schema currently ships at version `"6"` (`src/state/schema.rs`; `SCHEMA_VERSION` constant). The ingest production hardening bumped v5 → v6, adding only the `ingested_files` table. Epic 4c bumped v4 → v5, and added exactly two things: the `video_metadata_raw` table and the eight nullable metadata columns on `videos` listed above. Nothing else changed — no index, no constraint, no existing column. (Epic 4b's v3 → v4 added `watch_history.watched_at_raw`; `in_window` has existed since v1, but ingest hardcoded it `true` until Epic 4b wired up real window bounds.) A partial index on `(status, attempt_count, first_seen_at, video_id) WHERE status = 'pending'` accelerates `claim_next`'s attempt-aware scan — Epic 4a reordered it to `attempt_count ASC` first so retries drain behind fresh work (`src/state/schema.rs`).

### Lifecycle states

The `status` column is CHECK-constrained to five string values (`src/state/schema.rs:11–13`):

- **`pending`** — newly ingested (or stale-sweep recovered); eligible for claim by `claim_next`.
- **`in_progress`** — actively claimed by a worker; not eligible for claim until the worker finishes or the stale-claim sweep recovers it.
- **`succeeded`** — terminal success; transcript artifact on disk, never re-attempted.
- **`failed_retryable`** — a recoverable error was parked here; `last_retryable_kind` and `last_retryable_message` are populated. **Not a sink** (since Epic 3): `claim_next` selects only `pending` rows. Epic 4a ([ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)) gives the state two in-pipeline exits at failure time via `record_fetch_failure` — `retry_requeued` (retryable and `attempt_count` under the lifetime cap → `pending`) and `cookie_parked` (a requires-cookie class with no cookies configured → stays `failed_retryable`, awaiting a cookie run) — plus the exhaust case that parks here when the cap is hit. The start-of-batch sweep ([ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)) re-adjudicates every parked row through the classification table: `sweep_requeue` (retryable under cap → `pending`) and `sweep_mark_terminal` (a terminal-dispositioned class → `failed_terminal`).
- **`failed_terminal`** — a non-recoverable failure was recorded by `mark_terminal_failure` (inline pipeline write-off per [ADR 0033](../../decisions/0033-failure-classes-are-evidence-derived-message-text-lies-about-causes.md)) or by `sweep_mark_terminal` (start-of-batch sweep write-off); `terminal_reason` and `terminal_message` are populated. Sink; not re-attempted.

### State-transition diagram

Edges are drawn only for transitions that exist in current code. Since Epic 4a the `failed_retryable` exits are taken by the pipeline itself (`record_fetch_failure` at failure time) and by the start-of-batch sweep — there is no operator subcommand in the loop.

```
                              +------------+
              ingest          |            |  claim_next
              ------->        |  pending   |  (BEGIN IMMEDIATE,
                       +----> |            |  ORDER BY attempt_count,
                       |      +-----+------+   first_seen_at, video_id)
                       |            |
                       |            | claim taken; attempt_count++
   stale sweep         |            v
   (per 0024:          |      +------------+
    in_progress        |      | in_progress|
    -> pending)        +------+            |
                              +-----+------+
                                    |
          mark_succeeded /  record_fetch_failure /  mark_terminal_failure
                  |                 |                 |
                  v                 v                 v
          +------------+  +-------------------+  +------------------+
          | succeeded  |  | failed_retryable  |  | failed_terminal  |
          | (terminal) |  | (retry exits      |  | (terminal, sink) |
          +------------+  |  below, ADR 0036) |  +------------------+
                          +-------------------+

   record_fetch_failure (in-pipeline, at failure time, ADR 0036): one
   transaction decides where the failed claim lands —
     · retryable, attempt_count < retries+1  -> pending    (retry_requeued)
     · requires-cookie, no cookies configured -> failed_retryable (cookie_parked)
     · cap hit / else                         -> failed_retryable (exhausted)

   Start-of-batch sweep (ADR 0036; worker_id = 'sweep'):

    +-------------------+  sweep_mark_terminal       +------------------+
    |                   |  (terminal disposition)    | failed_terminal  |
    | failed_retryable  +--------------------------> +------------------+
    |                   |
    |                   |  sweep_requeue
    |                   |  (retryable/cookie-pool,   +------------------+
    |                   |   attempt_count < cap)     |     pending      |
    |                   +--------------------------> +------------------+
    +-------------------+
```

Key code references:
- `claim_next` SELECT ordering: `ORDER BY attempt_count ASC, first_seen_at ASC, video_id ASC` (retries drain behind fresh work, Epic 4a); WHERE `status = 'pending'` (`src/state/mod.rs`)
- `sweep_stale_claims` WHERE clause: `WHERE status = 'in_progress' AND claimed_at IS NOT NULL AND claimed_at < ?` (`src/state/mod.rs`)
- `record_fetch_failure` makes the requeue/exhaust/park decision in one IMMEDIATE transaction (`src/state/mod.rs`)
- `mark_terminal_failure` sets `status = 'failed_terminal'` (`src/state/mod.rs`); callers are the pipeline's inline write-off dispatch arms (`fetch_worker` in `src/pipeline/pipelined.rs` and `run_serial`'s error arm in `src/pipeline/serial.rs`)
- `sweep_mark_terminal` sets `status = 'failed_terminal'` from `failed_retryable` (`src/state/mod.rs`)
- `sweep_requeue` sets `status = 'pending'` from `failed_retryable`, gated by `attempt_count < ?` in the predicate (`src/state/mod.rs`)

## Claim contention

Multiple fetch workers (three by default per [ADR 0027](../../decisions/0027-orchestrator-topology-3-fetch-workers-feed-1-transcribe-worker-over-a-capacity-2-channel.md)) all call `claim_next` concurrently. Per [ADR 0026](../../decisions/0026-workers-drain-and-exit-on-claim-next-none-no-polling.md), the contention strategy uses SQLite's `BEGIN IMMEDIATE` to serialize writers.

`claim_next` opens an `Immediate` transaction, selects the next `pending` row (Epic 4a ordering: `attempt_count ASC, first_seen_at ASC, video_id ASC` — fresh work before retries, then FIFO within each attempt tier), and in the same transaction updates it to `in_progress` with `claimed_by` set to the `worker_id` string passed in, bumping `attempt_count` to `prev + 1`. Because SQLite serializes `BEGIN IMMEDIATE` writers at the WAL level, no two workers can execute this select-then-update simultaneously — double-claiming is structurally impossible.

`claim_next` returns `Option<Claim>`. `None` means no claimable rows exist right now; per ADR 0026, the orchestrator does not poll on `None` — it drains the batch and exits.

The `claimed_by` field is set to the calling worker's `worker_id` string (a UUID-like identifier passed in by the orchestrator). All subsequent in-flight mutators (`mark_succeeded`, `record_fetch_failure`, `mark_terminal_failure`) include `AND claimed_by = ?` in their WHERE predicates, so a swept row is never double-updated.

## Stale-claim sweep

Per [ADR 0024](../../decisions/0024-stale-claim-sweep-recovers-rows-blind-no-validation-no-attempt-bump.md), `Store::sweep_stale_claims(threshold)` resets rows stuck in `in_progress` back to `pending` after `threshold` has elapsed since `claimed_at`. The default threshold is 30 minutes, set in `src/config.rs:52` (`Duration::from_secs(30 * 60)`).

The sweep runs once at orchestrator startup — at the top of both `run_serial` (`src/pipeline/serial.rs:34`) and `run_pipelined` (`src/pipeline/pipelined.rs:504–505`) — before any `claim_next` call.

The sweep does **not** bump `attempt_count` and does **not** validate that the worker actually died (ADR 0024). It uses a simple time-based predicate: `claimed_at < (now - threshold_secs)` (`src/state/mod.rs:501–503`). It also writes **no** `video_events` audit row — unlike `claim_next`, `mark_succeeded`, `record_fetch_failure`, `mark_terminal_failure`, and the sweep mutators (`sweep_mark_terminal` / `sweep_requeue`), which are the sites that insert into `video_events`, a swept recovery leaves no audit-log entry. The motivating failure mode is a `kill -9` on the orchestrator process, which leaves rows in `in_progress` indefinitely; without the sweep, those rows block all future progress.

Redirect to ADR 0024 for the rationale against validation and attempt-count bumps.

## Mutator contracts

Per [ADR 0006](../../decisions/0006-store-mutators-return-result-usize-row-change-counts.md) and [ADR 0023](../../decisions/0023-failure-mutators-take-string-kinds-and-keep-the-claim-guard.md), all `Store` mutators return `Result<usize>` where the inner value is the SQLite row-change count. A return of `0` means the WHERE predicate did not match — the claim was stale (swept or re-assigned) — without requiring a separate query.

Current mutators on `Store`:

- **`claim_next(worker_id: &str) -> Result<Option<Claim>>`** — special; not a row-change mutator. Opens a `BEGIN IMMEDIATE` transaction, selects the next `pending` row (Epic 4a ordering: `attempt_count ASC, first_seen_at ASC, video_id ASC`), updates it to `in_progress`, bumps `attempt_count`, and returns the claim. Returns `None` if no `pending` rows exist. Since Epic 3 the returned `Claim` also snapshots `last_retryable_kind` at claim time — the input to kind-gated cookie routing per [ADR 0035](../../decisions/0035-cookies-ride-only-sensitivelogingated-retries-with-argv-redaction.md). (`src/state/mod.rs`)

- **`mark_succeeded(video_id, worker_id, artifacts) -> Result<usize>`** — flips `in_progress → succeeded`; writes transcript metadata columns. WHERE predicate: `status = 'in_progress' AND claimed_by = ?`. Returns `0` if the claim was stale (per ADR 0008, artifacts are already durable on disk before this call; `0` is survivable). (`src/state/mod.rs`)

- **`record_fetch_failure(video_id, worker_id, label: &str, message: &str, fetch_policy: &str, max_attempts: i64, requires_cookie: bool, cookies_configured: bool) -> Result<FailureRecordOutcome>`** — Epic 4a ([ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)): the in-pipeline retry decision in one IMMEDIATE transaction. `max_attempts` is the lifetime cap (`retries + 1`, computed by the caller); `requires_cookie` carries the classification's disposition. Compares the row's `attempt_count` against the cap and returns a typed `FailureRecordOutcome` — `Requeued` (→ `pending`, event `retry_requeued`), `ParkedForCookies` (a requires-cookie class with no cookies configured → stays `failed_retryable` without consuming retry budget, event `cookie_parked`), `Exhausted` (cap hit → parks in `failed_retryable`), or `StaleClaim` (the `status = 'in_progress' AND claimed_by = ?` predicate missed; no mutation — the caller counts it as `stale_after_failure`). Documents internally how the 0006 row-change contract is honored under the typed return. Replaces the Epic 3 `mark_retryable_failure` in every pipeline caller. (`src/state/mod.rs`)

- **`mark_terminal_failure(video_id, worker_id, reason: &str, message: &str) -> Result<usize>`** — flips `in_progress → failed_terminal`; writes `terminal_reason` and `terminal_message`. Same stale-claim predicate. Callers are the pipeline's inline write-off dispatch arms, invoked when the classifier returns a `terminal` disposition (the proven-dead message classes per ADR 0033). See also [`data-input.md`](data-input.md) §Retry classification. (`src/state/mod.rs`)

- **`sweep_stale_claims(threshold: Duration) -> Result<usize>`** — resets stale `in_progress` rows to `pending`. Returns the count of recovered rows. (`src/state/mod.rs`)

- **`list_failed_retryable() -> Result<Vec<ParkedRow>>`** — read-only snapshot of all `failed_retryable` rows; the start-of-batch sweep's input query. (`src/state/mod.rs`)

- **`sweep_mark_terminal(video_id, reason: &str, message: &str) -> Result<usize>`** — flips `failed_retryable → failed_terminal` (sweep write-off). Predicate: `status = 'failed_retryable'` — no `claimed_by` clause, because the sweep operates on unclaimed parked rows, not in-flight claims. Writes a `swept_terminal` event with `worker_id = 'sweep'`; preserves `last_retryable_*` diagnostics. (`src/state/mod.rs`)

- **`sweep_requeue(video_id, new_kind: &str, max_attempts: i64) -> Result<usize>`** — flips `failed_retryable → pending` (sweep requeue). Predicate: `status = 'failed_retryable' AND attempt_count < ?` — the attempt cap is checked in the same statement as the flip, so cap enforcement is race-free. Writes the re-classified kind back to `last_retryable_kind` (historical placeholder kinds normalize to taxonomy labels before the row is claimable — cookie routing per [ADR 0035](../../decisions/0035-cookies-ride-only-sensitivelogingated-retries-with-argv-redaction.md) reads the kind at claim time) and a `requeued` event with `worker_id = 'sweep'`. (`src/state/mod.rs`)

- **`open_batch_run(params_json, policy_toml) -> Result<i64>`** / **`close_batch_run(run_id, census_json) -> Result<usize>`** — Epic 4a batch-lifecycle bookkeeping. `open_batch_run` inserts the `batch_runs` row (params + active policy TOML) and returns its `run_id`; it returns `Result<i64>` rather than the 0006 row-count because the caller needs the identity of the row it just created (an identity-insert carve-out from ADR 0006, documented in its doc comment). `close_batch_run` stamps the census JSON and finish time, returning the 0006 row-change count. (`src/state/mod.rs`)

- **`upsert_metadata_raw(video_id: &str, envelope_json: &str) -> Result<usize>`** — Epic 4c ([ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)): inserts the fetch-time metadata envelope into `video_metadata_raw`, stamping `fetched_at = unix_now()`. `INSERT … ON CONFLICT(video_id) DO UPDATE` — last-write-wins across retries. **Deliberately not claim-guarded**: it runs before outcome dispatch on both pipeline paths, and its worst case under a stale claim is a slightly stale engagement snapshot that self-heals on the next fetch. Callers treat any `Err` as best-effort (warn + continue). (`src/state/mod.rs`)

- **`apply_metadata_batch(rows: &[MetadataColumns]) -> Result<usize>`** — Epic 4c: `load-metadata`'s write half. Opens one `IMMEDIATE` transaction per page and `UPDATE`s the eight schema-v5 columns on `videos` for each parsed row, returning the summed row-change count. A row whose `videos` entry no longer exists matches 0 — counted as `rows_without_video`, never an error. (`src/state/mod.rs`)

The loader's read half is `Store::metadata_raw_page(after_video_id: Option<&str>, limit: usize) -> Result<Vec<RawMetadataRow>>` in `src/state/queries.rs` — keyset pagination ordered by `video_id`, with the first page and subsequent pages issued as separate prepared statements (a single `OR … IS NULL` shape planned as a full table scan per page, which is O(n²) over a 3M-row table).

Redirect signature rationale to ADRs 0006 and 0023.

## Schema-version policy

`Store::open` reads `meta.schema_version` and compares it against the binary's `SCHEMA_VERSION` constant (currently `"5"`, `src/state/schema.rs`). On mismatch it returns `StateError::SchemaVersionMismatch { expected, found }` whose `Display` impl instructs the operator to run `ddp-transcribe migrate` (`src/state/mod.rs`). There is no in-process auto-migrate.

Migration runs in the dedicated `migrate` subcommand (`src/state/migrate.rs`), which bypasses the version check, applies the migration SQL in a single transaction, and bumps `meta.schema_version`. The migration ladder has five stages: v1 → v2 (adding the four failure-classification columns via `ALTER TABLE`), v2 → v3 (Epic 4a: the `batch_runs` table plus the attempt-aware pending index), v3 → v4 (Epic 4b: `watch_history.watched_at_raw`, per [ADR 0039](../../decisions/0039-ddp-watch-history-timestamps-are-treated-as-utc-documentary-only-and-empirically-unresolved.md)), v4 → v5 (Epic 4c: the eight metadata columns on `videos` plus the `video_metadata_raw` table, per [ADR 0042](../../decisions/0042-fetch-time-metadata-is-captured-raw-first-parsing-is-a-replayable-post-run-step.md)), and v5 → v6 (ingest production hardening: the `ingested_files` ledger, deliberately created empty — the migration cannot know which files produced a pre-v6 DB's rows). `migrate` is idempotent — a no-op if the DB is already current. Per [ADR 0022](../../decisions/0022-schema-version-hard-fails-at-store-open-migration-is-an-explicit-cli-subcommand.md), failing closed and asking for human action is the correct default.

## Operator visibility

`ddp-transcribe status` (`src/status.rs`, [ADR 0041](../../decisions/0041-status-is-the-read-only-operator-surface-the-0017-done-contract-lives-behind-verify.md)) renders the state machine described above: counts by `status`, `failed_retryable` broken down by `last_retryable_kind`, in-progress claim ages, and `batch_runs` history (an interrupted row with `finished_at IS NULL` renders honestly rather than being skipped or crashing on its NULL `census_json`). `--video-id` renders one video's `video_events` history using the Epic 4a `detail_json` vocabulary (`retry_requeued`, `cookie_parked`, `swept_terminal`, `requeued`, plus `claimed`/`succeeded`/`failed_terminal`); `--respondent-id`, `--errors`, `--retryable`, and `--json` cover the remaining operator questions. `status` never mutates the DB.

`status --verify` is the pause-safe predicate consumed by [ADR 0011](../../madr-archive/0011-spin-down-operational-practice-for-dev-workspace.md)'s spin-down practice, now in-tool rather than a manual checklist: per-shard artifact-existence checks against `--transcripts`, a full `raw_signals.schema_version` parse, and the verdict `pending == 0 ∧ in_progress == 0 ∧ zero artifact/schema/read failures`, exiting 1 when violated. It is the lean successor to the archived MADR-0017 done-contract.

## Crash-recovery durability

Per [ADR 0008](../../decisions/0008-artifacts-are-durable-on-disk-before-mark-succeeded.md), the transcript artifact is written to disk **before** `mark_succeeded` is called. Three crash cases:

- **Crash after artifact write, before `mark_succeeded`** — the row stays `in_progress`; stale sweep recovers it to `pending`; the next attempt re-writes the artifact (idempotent overwrite) and calls `mark_succeeded`.
- **Crash before artifact write** — the row stays `in_progress`; stale sweep recovers it; the next attempt re-runs the full fetch + transcribe.
- **Crash after `mark_succeeded`** — fully durable; both artifact and state row are committed.

Since Epic 4c the ordering is owned by a **pair** of functions in `src/pipeline/mod.rs` rather than one: `write_artifacts_durable` (both artifacts written, fsynced, renamed — no `Store` involvement) then `mark_after_artifacts` (`mark_succeeded` plus the WAV cleanup). The pipelined transcribe worker calls the two halves directly so its store lock covers only the DB acknowledgement, leaving the fsyncs unlocked; the serial path calls `write_artifacts_and_mark`, which is now just the composition of the pair. The invariant is unchanged — artifacts durable before `mark_succeeded` — but a reviewer checking it must check the call *sequence* at each site, not a single function body. Full discussion of the artifact side of this invariant is in [`transcription.md`](transcription.md). Redirect crash-recovery rationale to ADR 0008.

## Failure classification

Failures are classified into retryable or terminal at the state-machine surface via the pipeline mutators (plus the sweep pair above):

- **`record_fetch_failure`** decides requeue/exhaust/park and writes the `last_retryable_kind` / `last_retryable_message` columns on the retryable-side landings.
- **`mark_terminal_failure`** writes the `terminal_reason` and `terminal_message` columns on an inline write-off.

(`src/state/schema.rs` — all four diagnostic columns verified in schema)

Since Epic 4a, classification of yt-dlp stderr is driven by an operator-editable TOML table (`src/classification.rs`, [ADR 0037](../../decisions/0037-classification-is-an-operator-editable-toml-table-snapshotted-per-batch.md)) with an evidence-derived compiled default; `src/failure.rs` is a thin interpreter over it, and the message classes are now **label strings** (e.g. `NoDataBlocks`, `SensitiveLoginGated`, `NetworkTransient`, catch-alls `YtDlpOther`/`TranscribeOther`) rather than enum variants. Structural errors (process facts, not yt-dlp opinions) stay code-mapped. The proven-dead classes carry a `terminal` disposition and route to `mark_terminal_failure` inline — `IpBlockedMessage` ("Your IP address is blocked", a yt-dlp misfire meaning VIDEO REMOVED per ADR 0033), `VideoNotAvailable10231` ("status code 10231"), and `VideoNotAvailable10240` ("status code 10240", 606/606 dead at the 2026-07-07 census); everything else defaults cautiously to retryable (notably `NoPermission`, impure at 25/452 alive, which stays retryable). Labels serialize into the existing `TEXT` columns per ADR 0023 — no schema change. Historical rows written before Epic 3 carry placeholder kinds (`"Fetch"`, `"Transcribe"`, `"FetchOrTranscribe"`); the start-of-batch sweep's `sweep_requeue` normalizes them to the classification labels on requeue (preserving a real stored kind on a fallback hit — see [ADR 0036](../../decisions/0036-retry-is-in-pipeline-capped-failure-time-requeue-the-re-fetch-is-the-liveness-oracle.md)). See [`orchestration.md`](orchestration.md) for the caller's perspective on failure handling.

The two diagnostic column pairs are preserved across subsequent flips: the retryable-side mutators do not clear `terminal_reason`/`terminal_message`, and `mark_terminal_failure` does not clear `last_retryable_*` — so an operator inspecting any row sees both the most recent retryable and the most recent terminal diagnostics (the full per-transition history lives in `video_events`). (`src/state/mod.rs`)

## ADRs governing this subsystem

| ADR | Title | Where it applies |
|-----|-------|------------------|
| 0006 | `Store` mutators return `Result<usize>` | Mutator return contract. |
| 0008 | Artifact-before-`mark_succeeded` | Durability ordering. Cross-cuts transcription. |
| 0022 | Schema-version policy | `Store::open` hard-fail + `migrate` subcommand. |
| 0023 | Minimum mutator signatures | `mark_retryable_failure` / `mark_terminal_failure` signatures. |
| 0024 | Stale-claim sweep | `sweep_stale_claims` semantics. |
| 0026 | Claim contention via `BEGIN IMMEDIATE` | `claim_next` serialization. |
| 0033 | Evidence-derived failure taxonomy + inline write-off | Labels/reasons written by the failure mutators. |
| 0034 | Operator triage subcommand (superseded by 0036) | Historical; the triage subcommand and probe retired in Epic 4a. |
| 0035 | Cookies scoped to SensitiveLoginGated retries | `Claim.last_retryable_kind` snapshot consumed at claim time. |
| 0036 | In-batch capped retry + end-of-queue claim ordering | `record_fetch_failure`; `claim_next` `attempt_count ASC` ordering; `sweep_mark_terminal` / `sweep_requeue`; the `failed_retryable` exits. |
| 0037 | Operator-editable TOML classification table | Labels written by the failure mutators; `batch_runs.policy_toml` provenance. |
| 0039 | DDP timestamps are UTC-assumed, empirically unresolved | `watch_history.watched_at` / `watched_at_raw`. |
| 0040 | Analysis window computed at ingest; `recompute-window` is the only flag mutator | `watch_history.in_window`; schema v4. |
| 0041 | `status` is read-only; the 0017 done-contract lives behind `--verify` | Operator visibility above. |
| 0042 | Fetch-time metadata is captured raw-first; parsing is a replayable post-run step | `video_metadata_raw`; the eight metadata columns; `upsert_metadata_raw` / `apply_metadata_batch`; schema v5. Cross-cuts data input. |
