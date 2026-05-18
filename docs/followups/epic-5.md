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

### `--whisper-model` global flag rejected when placed after subcommand (missing `global = true`)

**Found in:** SRC bake (2026-05-06). `UU_TIKTOK_WHISPER_MODEL=... process`
works, and `--whisper-model X process ...` works, but
`process ... --whisper-model X` fails with
`error: unexpected argument '--whisper-model' found`.
**Disposition:** Clap UX papercut; env-var bypass available; not blocking.
**Trigger to revisit:** any operator pastes the flag after the subcommand
and gets the puzzling clap error, or when next touching `src/cli.rs` for
unrelated reasons.

In `src/cli.rs`, the `whisper_model` field on `GlobalArgs` is declared
without `global = true`. Clap therefore parses it strictly as a top-level
argument that must precede the subcommand:

```
uu-tiktok --whisper-model PATH process     # works
uu-tiktok process --whisper-model PATH     # rejected
UU_TIKTOK_WHISPER_MODEL=PATH uu-tiktok process    # works (env var bypass)
```

The env var sidesteps this entirely and is the production deployment
pattern, so this is not blocking. But the flag form is the more
discoverable path for ad-hoc operator use, and clap's `global = true`
attribute makes the flag work on either side of the subcommand without any
other code change:

```rust
#[arg(long, env = "UU_TIKTOK_WHISPER_MODEL", global = true)]
pub whisper_model: Option<PathBuf>,
```

Should land alongside any future change touching the same struct.
