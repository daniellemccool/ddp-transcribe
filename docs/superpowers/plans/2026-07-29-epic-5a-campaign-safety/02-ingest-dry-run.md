# Task 02: `ingest --dry-run` becomes actually dry (rollback-based)

**Files:**
- Modify: `src/ingest.rs` (`ingest` signature + per-file commit/rollback fork)
- Modify: `src/main.rs` (Ingest arm ~:50-77: honor the flag, drop the sham log at :57-59, dry-run suffix on output)
- Modify: `tests/ingest.rs` (new dry-run tests; adjust any direct `ingest(...)` callers to the new arity)

**Interfaces:**
- Consumes (existing): `pub fn ingest(inbox: &Path, store: &mut Store, window: WindowBounds) -> Result<IngestStats>` (src/ingest.rs:101); per-file structure: ledger fingerprint read (:127, may skip file) → `store.transaction()` (:154) → row upserts (:282-300) → `upsert_ingested_file_tx` (:172) → `tx.commit()` (:176); rusqlite `Transaction` rolls back on drop. `IngestStats` (src/ingest.rs:16-51). Dispatch arm src/main.rs:50-77 with the `dry_run` bool already parsed (`src/cli.rs:128-139`).
- Produces: `pub fn ingest(inbox: &Path, store: &mut Store, window: WindowBounds, dry_run: bool) -> Result<IngestStats>` — identical work and identical stats in both modes; dry-run rolls the per-file transaction back instead of committing.

**Semantics (binding):**
- **Rollback, not skip** (operator decision 2026-07-29): the full per-file transaction executes — every upsert, every counter derived from row-change returns (`watch_history_duplicates`, `backfilled_raw_dates`) stays exactly as real — and then `drop(tx)` (or explicit `tx.rollback()?`) replaces `tx.commit()` when `dry_run`.
- The ledger **read** (skip-if-fingerprint-match) stays active in dry-run, so `files_skipped_already_ingested` reports what a real run would skip. The ledger **write** is inside the rolled-back tx — nothing persists.
- Dry-run over a fresh DB leaves `videos`, `watch_history`, and the ingest-ledger table all empty afterwards; a subsequent real ingest processes everything (the ledger was not poisoned).
- Window semantics untouched (ADR-0040): `in_window` is computed identically in both modes; `computed_out_of_window` counts identically.
- Output line mirrors the sibling subcommands: append ` (dry-run)` to the human summary; keep the structured `tracing::info!` with a `dry_run` field. Delete the "dry-run: not yet implemented; running real ingest" log entirely.
- IMPORTANT honesty note for the runbook (Task 05 picks it up): dry-run acquires the same brief per-file write locks as a real ingest (BEGIN IMMEDIATE + rollback) — safe under WAL/busy_timeout alongside a live `process`, but not lock-free.

- [ ] **Step 1: Write the failing integration tests**

Append to `tests/ingest.rs` (match the file's existing seeding/fixture helpers — it already builds inbox dirs with DDP JSON fixtures; reuse its helper for a small valid inbox):

```rust
#[test]
fn dry_run_reports_real_stats_but_writes_nothing() {
    // Arrange: whatever existing helper builds a 2-video inbox + fresh DB.
    // (Reuse the fixture helper the file's first test uses — do not invent
    // a new fixture format.)
    let (dir, db) = seeded_inbox_and_db(); // adapt to the real helper name

    let out_dry = run_ingest(&db, &dir, &["--dry-run"]); // adapt: assert_cmd or direct call, matching file style
    assert!(out_dry.contains("(dry-run)"), "output was: {out_dry}");

    let conn = rusqlite::Connection::open(&db).unwrap();
    let videos: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0)).unwrap();
    let watches: i64 = conn.query_row("SELECT COUNT(*) FROM watch_history", [], |r| r.get(0)).unwrap();
    assert_eq!((videos, watches), (0, 0), "dry-run must persist nothing");
    // Ledger table too — adapt table name to schema (see src/state/schema.rs).
    let ledger: i64 = conn.query_row("SELECT COUNT(*) FROM ingested_files", [], |r| r.get(0)).unwrap();
    assert_eq!(ledger, 0, "dry-run must not poison the ingest ledger");
}

#[test]
fn real_ingest_after_dry_run_ingests_everything() {
    let (dir, db) = seeded_inbox_and_db();
    run_ingest(&db, &dir, &["--dry-run"]);
    let out = run_ingest(&db, &dir, &[]);
    assert!(!out.contains("already-ingested 1"), "ledger must be clean after dry-run; output: {out}");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let videos: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0)).unwrap();
    assert!(videos > 0, "real run after dry-run ingests normally");
}
```

(The helper names, invocation style — binary via assert_cmd vs direct `ddp_transcribe::ingest` call — and the stats-line text MUST be adapted to what `tests/ingest.rs` already does; the assertions above are the binding content. Verify the ledger table name from `src/state/schema.rs` before writing.)

- [ ] **Step 2: Run to confirm failure** — `cargo test --test ingest -- --test-threads=1`: the first new test fails (dry-run currently ingests for real).

- [ ] **Step 3: Implement**

`src/ingest.rs`:

```rust
pub fn ingest(inbox: &Path, store: &mut Store, window: WindowBounds, dry_run: bool) -> Result<IngestStats> {
```

At the per-file commit site (:172-176 region):

```rust
        if dry_run {
            // Full-fidelity dry-run: every upsert ran (so every row-change-
            // derived counter is real), then the transaction is discarded.
            // Dropping a rusqlite Transaction rolls back; do it explicitly
            // for legibility.
            tx.rollback().context("rolling back dry-run ingest transaction")?;
        } else {
            upsert_ingested_file_tx(&tx, name, *size, *mtime)?; // existing call — keep its real position/order
            tx.commit().with_context(|| format!("committing ingest of {name}"))?;
        }
```

(Adapt to the actual code order — if `upsert_ingested_file_tx` currently precedes other statements, keep it inside the tx in BOTH modes and let the rollback discard it; the binding requirement is only commit-vs-rollback. Match existing `.context` phrasing style.)

`src/main.rs` Ingest arm: delete the :57-59 sham block; pass `dry_run` through; extend the completion output:

```rust
            let stats = ingest::ingest(&cfg.inbox, &mut store, window, dry_run)?;
            tracing::info!(%stats, dry_run, "ingest complete");
            println!("ingest: {stats}{}", if dry_run { " (dry-run)" } else { "" });
```

(Match the arm's real logging shape — the existing field-by-field `tracing::info!` at main.rs:64-77 stays; add the `dry_run` field to it rather than inventing a second log line, and adapt the human line to however the arm currently prints. The binding requirements: no sham log, flag threaded, visible dry-run marker in output.)

- [ ] **Step 4: Run tests to verify they pass** — both new tests + all existing ingest/state_ingest/recompute_window/cli tests (arity fix may touch other callers; `rg 'ingest\(' tests/ src/`).

- [ ] **Step 5: Full verification**

`cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1` — green.

- [ ] **Step 6: Commit**

```bash
git add src/ingest.rs src/main.rs tests/ingest.rs
git commit -m "fix(ingest): --dry-run is actually dry — full per-file transaction rolled back, real stats, ledger untouched"
```
