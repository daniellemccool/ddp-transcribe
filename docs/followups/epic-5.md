# FOLLOWUPS — Epic 5 active entries

Active-scope review items targeted for Plan B Epic 5. See `../FOLLOWUPS.md`
for the scope index across all epics; `../cosmetic-followups.md`,
`../bake-findings.md`, `../archive/followups-resolved.md` for sibling
categories. The unverified-hypothesis prefix rule
(`**Hypothesis (unverified):**`) applies here per 0020.

---

### `Store::pragma_string` visibility is `pub`, not `pub(crate)`

**Found in:** T7 code quality review (opus).
**Disposition:** Defer to bin/lib structural reassessment (per ADR 0002).
**Trigger to revisit:** Plan A reassessment point — when bin/lib pattern is decided.

`Store::pragma_string` is currently `pub` (matches the per-task file's
verbatim spec text). It builds `format!("PRAGMA {}", name)` because PRAGMA
names cannot be parameterized in SQLite. Today the only caller is the
`pragma_journal_mode_is_wal` integration test passing the literal
`"journal_mode"`, but `pub` visibility means external library consumers
could pass attacker-controlled or malformed names.

Two reasonable fixes when this is revisited:

- Lower visibility to `pub(crate)` (matches `conn`/`conn_mut`); only the
  integration test would need adjustment, possibly via a `test-helpers`
  feature gate.
- Switch the implementation to `rusqlite::Connection::pragma_query_value`,
  which validates the pragma name internally.

Coupled to 0002's deferred bin/lib structural decision because the
"is this part of the public library API?" question depends on whether the
project ends up thin-binary, fat-library or stays with the dual-`mod`
pattern.

---

### `Store::read_meta` could use `OptionalExtension::optional()`

**Found in:** T7 code quality review (opus).
**Disposition:** Style improvement; defer indefinitely.
**Trigger to revisit:** any future edit to `Store::read_meta`.

The current implementation uses `map_or_else` to translate
`QueryReturnedNoRows` to `Ok(None)`. Functionally correct but verbose. The
idiomatic rusqlite pattern is `query_row(...).optional()` with the
`OptionalExtension` trait. Pure refactoring — not blocking anything; touch
this code only when there's a real reason to.

---

### `output::cleanup_tmp_files` minor cleanups: missing context, overcounted removals

**Found in:** T8 code quality review (opus).
**Disposition:** Cosmetic; bundle with the next real edit to this function.
**Trigger to revisit:** any task that touches `cleanup_tmp_files`, or T15 (init-cmd) when wiring the call site.

Two small inconsistencies in `src/output/artifacts.rs::cleanup_tmp_files`:

1. The inner `std::fs::read_dir(&path)?` and the surrounding `entry?` /
   `shard_entry?` lines bubble up raw `io::Error` without path context. The
   outer `read_dir(transcripts_root)` is contextualized via `with_context`.
   On a permission-denied inside one shard dir, the operator gets a path-less
   error.

2. `let _ = std::fs::remove_file(&p); removed += 1;` increments
   unconditionally. If `remove_file` fails (permission, EBUSY), the returned
   count overstates the cleanup. Best-effort semantics are fine; the count
   just shouldn't claim success it didn't achieve.

Neither is a behavioural bug for Plan A's happy-path single-process loop.
Worth fixing when this function next gets touched.

---

### `output::shard_distributes_uniformly` test rationale is reversed

**Found in:** T8 code quality review (opus).
**Disposition:** Cosmetic; comment is misleading but the assertion still
catches the stated regression.
**Trigger to revisit:** any future edit to the test, or whenever a
`VideoId` newtype absorbs `shard()` and the test moves with it.

`src/output/mod.rs::shard_distributes_uniformly` uses monotonic counter
input (`base + i` for `i in 0..10000`), which produces exactly 100 items per
last-two-digits bucket. The ±50% assertion (`50..=150`) passes with a
margin of 0%, not because the bound is "lenient for synthetic input" as the
comment claims.

The comment says "real Snowflake IDs would be tighter" — that's reversed.
Real Snowflake low bits are pseudorandom; their per-bucket variance over
10k samples is Poisson-like (~10% std dev), so real IDs would be looser,
not tighter, than the artificially perfect counter cycle.

The test still catches the "uses high digits instead of low" regression via
the `counts.len() == 100` assertion (high digits are time-clustered, so a
high-digits implementation would collapse to 1-2 buckets). The bounds check
is decorative for this input; either tighten it (e.g., assert exact equality
to 100) or replace the input with a PRNG-driven sample to exercise the
bound meaningfully.

---

### `videos.updated_at` is frozen at first-seen by `upsert_video`

**Found in:** T9 code quality review (opus).
**Disposition:** Accepted for T9; re-evaluate as T10/T13 land.
**Trigger to revisit:** T10 (`claim_next` / `mark_succeeded`), T13 (ingest cmd),
or any future Store mutator that touches a `videos` row.

`Store::upsert_video` uses `INSERT OR IGNORE` and binds the same `now` value to
both `first_seen_at` and `updated_at`. On a re-upsert, neither column is
written. The brief's idempotence test only asserts `first_seen_at` is
unchanged, but `updated_at` is equally frozen — which contradicts the natural
reading of the column name ("when was this row last touched").

For pure-ingest semantics this is correct: nothing about the row changed. But
T10's `claim_next` / `mark_succeeded` and any later mutators MUST remember to
bump `updated_at` themselves, since `upsert_video` will not update it on
subsequent calls. If they forget, `updated_at` becomes a misnomer.

Two reasonable resolutions when this surfaces:

- Accept the contract: rename to `inserted_at` (or document `updated_at` as
  "last write to mutable columns, not including idempotent re-upsert").
- Switch `upsert_video` to `INSERT ... ON CONFLICT(video_id) DO UPDATE SET
  updated_at = excluded.updated_at` — preserves `first_seen_at` and
  `source_url` invariants while bumping `updated_at` on every observation.
  Add a regression test asserting `updated_at` strictly increases on
  re-upsert and `first_seen_at` does not.

The choice depends on whether `updated_at` is meant as "last-mutation marker"
(useful for stale-claim detection in Plan B) or "last meaningful state
change". Plan B's stale-claim recovery is the most likely first consumer that
will care.

---

### `Store::conn` / `Store::conn_mut` accessor hygiene after T10

**Found in:** T9 code quality review, re-confirmed in T10 review (opus).
**Disposition:** Cleanup commit, or fold into 0002's bin/lib
reassessment.
**Trigger to revisit:** Plan A reassessment point, or any task that
genuinely needs `&Connection` / `&mut Connection` outside `Store`'s
own `impl`.

`src/state/mod.rs` lines 105 and 111 carry `#[allow(dead_code)]` with
comments naming T9 and T10 as the first consumers. Both tasks have now
landed and both went via direct `self.conn` field access. The comments
are factually wrong.

Current state of consumers:
- `Store::conn` — used only by the `#[cfg(test)]` NULL-rejection
  unit tests at `src/state/mod.rs::tests::null_video_id_rejected_*` and
  `null_meta_key_rejected_*`. So it has one real consumer, gated to
  test compilation.
- `Store::conn_mut` — no consumer at all.

Resolution options:

- Lowest-cost: delete `conn_mut` outright; rewrite the `conn()` comment
  to say "used by cfg(test) schema invariant tests; keep until lib API
  stabilizes."
- Structural: defer to 0002's reassessment — under Option 4
  (thin-binary fat-library) the `pub(crate)` accessors may go away
  entirely.

Per 0002's cleanup discipline, the `rg "allow\(dead_code\)" src/`
audit catches this on every pass.

---

### `ingest::walk_recursive` minor polish: silent missing-inbox + missing inner context

**Found in:** T13 code quality review (opus).
**Disposition:** Cosmetic; bundle with the next real edit to `ingest::*`.
**Trigger to revisit:** any task that touches `walk_recursive` or `ingest`
error-handling.

Two small inconsistencies in `src/ingest.rs`:

1. `walk_recursive` returns `Ok(())` if the root inbox doesn't exist, so an
   operator who passes a typo to `--inbox` gets a successful run with
   `files=0` and no error. Cheap defense: `bail!` at the top-level `ingest()`
   call when the root doesn't exist. Deeper subdirectories disappearing
   mid-walk is a different story (race; acceptable to ignore).

2. The outer `read_dir(transcripts_root)` is contextualized via
   `with_context`; the inner `entry?` and recursive `walk_recursive(&path,
   out)?` calls bubble up raw `io::Error` without path context. Same minor
   pattern as `output::cleanup_tmp_files` already in FOLLOWUPS. On a
   permission-denied inside one shard subdirectory, the operator gets a
   path-less error.

Both fine for Plan A's happy-path single-process loop; worth fixing when
this code next gets touched.

---

### `output::shard_dir` is unused; allow comment falsely names T13/T14 as consumers

**Found in:** T15 code quality review (opus) — Plan A close-out 0002 audit.
**Disposition:** Dead helper; delete or find a real caller.
**Trigger to revisit:** Plan A → Plan B reassessment, or next edit to
`src/output/mod.rs`.

`src/output/mod.rs::shard_dir` carries `#[allow(dead_code)]` with the comment
"consumed by T13/T14 (ingest-cmd, process-cmd)". Neither task consumes it;
`pipeline.rs` binds a local `shard_dir` variable but calls
`opts.transcripts_root.join(shard(&claim.video_id))` directly. The function
has no real caller outside its own unit test. Either delete it, or have
`pipeline.rs` call it instead of re-doing the join inline. Bundles naturally
with the `VideoId` newtype refactor that 0004 anticipates.

---

### `YtDlpFetcher::acquire` tight coupling to yt-dlp's `{video_id}.wav` output filename

**Found in:** T11 code quality review (opus); finding 3 of the original
four-finding `YtDlpFetcher::acquire` entry. Split out at Epic 3 close:
findings 1–2 were resolved by Epic 3 (`9974d69`, archived in
`../archive/followups-resolved.md`), finding 4 moved to
`docs/followups/plan-c.md`.
**Disposition:** Epic 5 fetch hardening.
**Trigger to revisit:** Epic 5 planning; or any yt-dlp version bump that
changes output-template behavior.

The post-fetch existence check (now `FetchError::MissingOutput`) assumes
yt-dlp's `--audio-format wav` + `%(ext)s` template always produces exactly
`{video_id}.wav`. If yt-dlp emits a sanitized variant, intermediate partial
files, or a suffix for collisions, the check fails despite a successful
exit. A robustness improvement: scan `video_dir` for any `.wav` after
success, or glob `{video_id}.*.wav`.

---

### Epic 3 close: test-hardening bundle (signal capture, classifier precedence, kind-string end-to-end)

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Grouped opportunistic hardening; bundle into one Epic 5 pass rather than three separate commits.
**Trigger to revisit:** Epic 5 test-sweep planning.

Three gaps in current coverage, none blocking:

1. No end-to-end test actually spawns a child, sends it a signal, and
   asserts `FetchError::ToolFailed { signal: Some(_), .. }` comes out the
   other end of `process::run`. Today's tests construct `ToolFailed` by hand
   with a canned `signal` value; the real kill→capture path (`ExitStatus`'s
   Unix `signal()` extension) is untested.
2. `classify_message` (`src/failure.rs`) has load-bearing match-arm order
   (write-off classes before retryable, network markers last) and matches
   are plain `str::contains` — no test pins the precedence when two markers
   both appear in one stderr blob, nor exercises case-sensitivity (TikTok/
   yt-dlp message casing has drifted before).
3. `transcribe_worker`'s end-to-end dispatch (three-arm classifier) has no
   test asserting the actual `RetryableKind`/`UnavailableReason` *tag
   string* written to `last_retryable_kind` — coverage today is via the
   inline worker-test audit verdicts in `tests/pipeline_fakes/`, which the
   Epic 3 review already flagged as tracking this gap rather than closing it.

---

### `state/mod.rs` hygiene bundle (sweep mutators — formerly Epic-3 triage mutators)

**Found in:** Epic 3 final whole-branch review. Items 3–4 re-pointed at the
Epic 4a surfaces (T08: triage retired; `triage_mark_terminal`/`requeue_retryable`
became `sweep_mark_terminal`/`sweep_requeue`, `run_triage` became
`batch::run_sweep`) — the underlying concerns carry over unchanged.
**Disposition:** Grouped cleanup; low risk, no behavior change expected.
**Trigger to revisit:** next edit to `src/state/mod.rs`, or Epic 5 sweep.

1. `claim_next`'s empty-candidate path (`src/state/mod.rs:341`) commits with
   a bare `tx.commit()?` — every other transaction in the file uses
   `.context("commit ...")`. Harmless (an empty SELECT's commit essentially
   can't fail) but inconsistent with the file's own convention.
2. No test pins `attempt_count == 2` after a row is claimed, fails, and is
   requeued+reclaimed once — the attempt-counting invariant across the
   claim→fail→requeue→reclaim cycle is exercised piecemeal, not end-to-end.
3. `sweep_mark_terminal` and `sweep_requeue` operate on
   `failed_retryable` rows, which per the current schema are never
   `claimed_by`/`claimed_at`-set — so the missing defensive clear is inert
   today. Worth adding anyway if a future schema change ever lets a
   claimed row reach these mutators.
4. The `kept_capped` path in `batch::run_sweep` (`src/batch.rs` — a
   `sweep_requeue` predicate miss on the attempt cap) writes no
   `video_events` row and no test asserts that absence — currently correct
   (nothing happened to the row; the event insert is gated on `changed > 0`),
   but an implicit invariant that should be pinned so a future change
   doesn't accidentally start emitting spurious events.

---

### `run_serial` fetch/transcribe downcast asymmetry

**Found in:** Epic 3 final whole-branch review. The bundle's other half —
discarded mutator row-change counts — was resolved by Epic 4a T06
(`c7c4f1b`: `mark_terminal_failure` gated on `changed > 0`; retryable
dispatch goes through `record_fetch_failure_serial`, whose typed
`StaleClaim` outcome is counted as `stale_after_failure`); archived.
**Disposition:** Opportunistic hardening.
**Trigger to revisit:** `run_serial` retirement decision (see "Deferred / open" in `docs/superpowers/plans/2026-07-07-plan-b-epic-3/EPIC-3-CLOSE.md`), or Epic 5 sweep.

`src/pipeline/serial.rs`'s fetch-side classification downcasts the
top-level anyhow error (`downcast_ref::<FetchPhaseError>`) while the
transcribe-side walks the whole chain (`e.chain().find_map(...)`) — an
asymmetry flagged inline by a tripwire comment at `src/pipeline/serial.rs`
(near the `downcast_ref` call). Fix if `run_serial` survives the
retirement decision; moot if it's deleted.

---

### `FetchOpts`'s derived `Debug` does not redact `cookies_file`

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Small hardening; not exploitable today (no code path logs
`FetchOpts` via `{:?}`) but a footgun for future callers.
**Trigger to revisit:** any future logging/tracing call that formats a
`FetchOpts` value, or Epic 5 sweep.

`src/fetcher/mod.rs`'s `FetchOpts` derives `Debug`, so `{:?}` prints the raw
`cookies_file` path verbatim — inconsistent with `scrub_cookie_path`'s
redaction of the same path everywhere else it can reach an error message or
argv (ADR 0035). A hand-rolled `Debug` impl that redacts `cookies_file` to
`Some("[COOKIES-REDACTED]")` / `None` would close the gap before any caller
relies on the derived form.

---

### `scrub_cookie_path` has no guard against an empty cookie path

**Found in:** Epic 3 final whole-branch review.
**Disposition:** Small hardening; edge case, not observed in practice
(cookie file paths come from `--cookies-file`, which clap will not populate
with an empty string absent an explicit `--cookies-file ""`).
**Trigger to revisit:** Epic 5 sweep, or if `--cookies-file ""` is ever
observed in the wild.

`src/fetcher/ytdlp.rs::scrub_cookie_path` does
`excerpt.replace(&path.display().to_string(), "[COOKIES-REDACTED]")`. If
`path` is empty, `str::replace` with an empty pattern inserts the
replacement between every character of `excerpt`, corrupting the stderr
excerpt beyond readability. A one-line guard (`if path.as_os_str().is_empty()
{ return excerpt; }`) closes it.

---

### Pipelined transcribe worker holds the Store mutex across artifact writes, not just `mark_succeeded`

**Found in:** transcript-storage assessment (Epic 4a close-out, format-selector
worktree). Related to, but narrower than, the T17 entry above (that entry is
about stall risk under `TOKIO_WORKER_THREADS=1`; this one is about lock-scope
minimization specifically).
**Disposition:** Deferred; today's per-video artifact-write cost (~5-20ms, 4
fsyncs, ~10-25KB) is small next to the ~1-2s transcription call it follows, so
the extra mutex hold time is not currently a measured bottleneck.
**Trigger to revisit:** Epic 5 perf sweep, or if a future bake shows fetch
workers starved on `claim_next` during the write+mark phase.

`src/pipeline/pipelined.rs` (`transcribe_worker`, ~lines 562-574) acquires the
shared `Store` mutex once and holds it across the whole
`write_artifacts_and_mark` call — both the artifact writes/fsyncs (which need
no `Store` access) and the `mark_succeeded` DB write (which does). Per 0008,
the ordering (artifacts durable before `mark_succeeded`) is load-bearing and
must not change; only the *lock scope* is the finding. A future perf pass
could split `write_artifacts_and_mark` into a lock-free artifact-write phase
followed by a narrowly-scoped `store.lock().await` around just
`mark_succeeded`, preserving the 0008 ordering while shrinking the window
other fetch workers are blocked from `claim_next`. Any such change is
0008-ordering-sensitive and should land as its own reviewed change, not
bundled into an unrelated task.

---

### Epic 4b final review: status polish + test-debt bundle

**Found in:** Epic 4b final whole-branch review.
**Disposition:** Grouped opportunistic hardening + test-debt; bundle into
one Epic 5 pass rather than five separate commits.
**Trigger to revisit:** Epic 5 hygiene sweep planning.

Five gaps on the `status`/`--verify` surface, none blocking:

1. `render_event_detail_inline` (`src/status.rs`) drops non-string values of
   the known keys (`kind`, `policy`, `new_kind`, `reason`) with no raw-JSON
   fallback — an unexpected non-string value under a known key renders as
   nothing instead of falling back to the raw `detail_json`.
2. Missing test fixtures: malformed `detail_json`; the `{"reason","message"}`
   shape; a corrupt-JSON artifact exercising the `unreadable_artifacts`
   path; valid metadata with `raw_signals` absent exercising the
   schema-version-mismatch path; a single-status zero-fill assertion; and a
   `WindowBounds` end-only-bound case.
3. `run_verify`'s per-entry `e.ok()` drop on the `read_dir` iterator
   (`src/status.rs`) miscounts exotic `DirEntry` errors as missing rather
   than unreadable — an entry that exists but can't be named/stat'd
   silently disappears from the shard's filename set instead of surfacing
   as an infra fault.
4. `status --respondent-id` with a typo'd/nonexistent id reports an
   all-zeros `RespondentSummary` instead of an error — inconsistent with
   `--video-id`, which errors (and exits 1) when the video row is missing.
   `Store::respondent_summary`'s aggregate `COUNT(*)` query has nothing to
   distinguish "zero matching rows" from "unknown respondent."
5. `src/status.rs` has imports mid-file (`use crate::state::queries::{...}`,
   `use crate::state::ParkedRow` around line 499) instead of grouped with
   the top-of-file `use` block.

---

### `ingest --dry-run` is not dry

**Found in:** Epic 4b final whole-branch review. Pre-existing wart (the
`tracing::info!` warning predates 4b); raised stakes because Epic 4b gave
`ingest` window flags to preview.
**Disposition:** Not blocking 4b — `recompute-window --dry-run` (Task 06)
mitigates for rows already ingested — but the gap widens with each flag
`ingest` grows.
**Trigger to revisit:** Epic 5, or sooner if an operator is burned by it.

`cli::Command::Ingest`'s `dry_run` arm (`src/main.rs`, ~line 58) logs
`"dry-run: not yet implemented; running real ingest"` and then runs the real
ingest unconditionally — `--dry-run` has never actually been dry. Now that
`ingest` takes `--window-start`/`--window-end` (Epic 4b Task 05), an operator
who reaches for `--dry-run` to preview a window's effect before committing to
it instead mutates state for real. `recompute-window --dry-run` covers
re-deriving `in_window` for rows already in the DB, but not the first
ingest of a new export, where the mutation is `watch_history` inserts plus
`videos` upserts, not just the `in_window` recompute.

---

### `main.rs` re-declares the library's entire module tree

**Found in:** operator review, 2026-07-28 (during Epic 4c).
**Disposition:** Real structural debt, but far too broad for an Epic 4c rider —
it touches every module declaration in the crate and would have to land as its
own change with a full-suite rerun. Cites ADR-0002's deferred bin/lib
reassessment, which parked exactly this question.
**Trigger to revisit:** Epic 5 hygiene bundle, alongside `run_serial`
retirement, the `state/mod.rs` split, and the sync-IO sweep.

`src/main.rs` re-declares nearly every module that `src/lib.rs` already
exposes, so the binary compiles its own copy of most of the crate rather than
importing `ddp_transcribe::…`. Three consequences:

- **Double compilation** of the bulk of the crate on every build — paid on
  every `cargo build`, `cargo clippy --all-targets`, and `cargo test`.
- **Broadened public surface.** Items are `pub` because the *binary's* copy
  needs them across module boundaries, which pushes visibility wider than the
  library alone would require, and makes "is this part of the public API?"
  unanswerable (the same ambiguity ADR-0002 flagged and the `Store::pragma_string`
  entry above is blocked on).
- **A driver of the accumulated suppressions.** An item used only by the
  library's copy is dead in the binary's copy and vice versa, which is why a
  chunk of the crate's `dead_code` / `unused_imports` allows exist at all.
  Removing the duplication should let several of them be deleted outright
  rather than re-justified.

Idiomatic fix: `main.rs` declares no modules and imports `ddp_transcribe::…`
from the single canonical tree in `lib.rs`. Do it as one mechanical change with
the suppression cleanup as a follow-on, not interleaved.

---

### Startup `cleanup_tmp_files` sweep can delete a concurrent process's in-flight tmp

**Found in:** Epic 4c Task 05 review (restated by codex-advisor while reviewing
the unique-tmp-name change). Pre-existing behavior, not introduced by 4c.
**Disposition:** Blast radius is limited to multi-process deployments, which is
exactly the SRC two-GPU setup — but the failure is self-healing (the losing
write fails, the row stays `in_progress`, the stale sweep reclaims it and the
next attempt re-writes the artifact idempotently per ADR-0008). Not worth a
rushed fix inside 4c. The real blast radius is bigger than "one video loses its
write": a concurrent instance's startup sweep unlinking an in-flight tmp makes
`atomic_write`'s rename fail, `write_artifacts_durable` propagates that as an
error, and the transcribe worker's error cancels the orchestrator — so the
*whole batch run* aborts, not just the one video. It is recoverable (restart
picks the DB state back up, nothing is corrupted) but it is a run abort under
the two-instance SRC deployment, not a one-video loss.
**Trigger to revisit:** Epic 5, bundled with the other
`output::cleanup_tmp_files` polish entry above.

`cleanup_tmp_files` (`src/output/artifacts.rs`) sweeps every file whose name
contains `.tmp` under the transcripts root at startup. Epic 4c made each
in-flight tmp name unique per writing process (`{name}.tmp-{pid}-{seq}`), which
removed the *collision* — two processes can no longer write the same tmp path —
but the startup sweep still matches on the substring, so a second instance
starting up while the first is mid-write will happily unlink the first's
in-flight tmp file. The fix is to make the sweep skip tmp files belonging to a
live pid (or to any pid other than its own), not to narrow the glob — a crashed
run's leftovers must still be reclaimable.

---

### `upsert_metadata_raw` is not claim-guarded

**Found in:** Epic 4c Task 03 review (triaged to backlog at the time).
**Disposition:** Accepted tradeoff, documented in the mutator and in ADR-0042 —
recorded here so the acceptance is a decision on the record rather than an
oversight someone rediscovers. Revisit only if the blast radius grows.
**Trigger to revisit:** Epic 5, or sooner if metadata ever gains a consumer for
which snapshot freshness is load-bearing.

Every other in-flight `Store` mutator carries `AND claimed_by = ?` in its WHERE
predicate (ADR-0023), so a worker whose claim was swept out from under it
cannot write. `upsert_metadata_raw` deliberately does not: it runs *before*
outcome dispatch, at a point where the row's claim state has not been
re-checked, and it is `INSERT … ON CONFLICT DO UPDATE` (last-write-wins by
design, so retries refresh the envelope). A stale worker can therefore overwrite
a newer envelope with an older one.

The blast radius is confined to metadata: the affected row's *lifecycle* is
untouched (the guarded mutators still reject the stale claim), so the worst
outcome is a slightly stale engagement snapshot with a `metadata_fetched_at`
that honestly reports when that snapshot was taken. It self-heals on the next
successful fetch of the same video. Adding the guard would mean threading
`worker_id` into a call site whose whole point is that it runs unconditionally
on both the success and failure paths — which is why it was not done.

---

### Ingest file-ledger hardening bundle

**Found in:** PR #23 review (production ingest hardening, 2026-07-29) — three
Minors carried out of the review by agreement.
**Disposition:** None blocks the campaign; bundle for Epic 5's ingest/sync-IO
sweep.
**Trigger to revisit:** Epic 5, or immediately if the inbox ever gains
subdirectories with same-named files.

1. **Basename-only ledger key.** `ingested_files.file_name` is the basename by
   design (the inbox directory may move between hosts), but `walk_json_files`
   recurses — two same-named files in different subdirectories would collide in
   the ledger and the second could be wrongly skipped. Today's inbox is flat
   and filenames embed participant+key (collision-proof in practice); the risk
   is structural, not live.
2. **(size, mtime) is a one-second-resolution change detector.** A same-size
   rewrite within one mtime tick is invisible to the ledger (row-level upserts
   remain the correctness backstop, so the miss costs freshness of that file's
   rows, not corruption). Tests deliberately use size deltas to avoid the
   flake; the limitation is real and now documented here.
3. **No mid-transaction rollback regression test.** The ledger upsert rides the
   same per-file transaction as the row upserts (atomicity by construction —
   one `Transaction`, `?` before commit), but no test forces a mid-tx failure
   to pin the rollback. Add one alongside the Epic 5 test-hardening bundle.


---

### `global = true` both-position CLI test asserts parse acceptance only

**Found in:** metadata-backfill branch review (v0.3.1 `global = true` rider,
`7dfa771`).
**Disposition:** Hardening candidate; bundle with the Epic 5 test-hardening
sweep. The shipped test is not wrong, just shallow.
**Trigger to revisit:** Epic 5 test-hardening bundle, or the next change to
`GlobalArgs`.

`tests/cli.rs`'s both-position test proves the parser *accepts* every
`GlobalArgs` flag after the subcommand — it does not prove the value reaches
`GlobalArgs`, nor what happens when a flag is given on both sides of the
subcommand. Clap's `global = true` propagates a value forward and has
last-occurrence-wins semantics for duplicates, but that is inherited behavior
this repo has never pinned. A `clap::Parser::try_parse_from` unit test (no
process spawn, so cheap) asserting (a) the parsed field equals the value given
after the subcommand, and (b) the documented precedence when the same flag
appears both before *and* after it, would turn an acceptance check into a
behavior check.
