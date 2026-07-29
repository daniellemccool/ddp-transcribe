# Task 01: Cohort queries + insert-if-missing mutator

**Files:**
- Modify: `src/state/queries.rs` (row struct + two read-only queries in the existing `impl Store` block)
- Modify: `src/state/mod.rs` (`Store::insert_metadata_raw_if_missing`, beside `upsert_metadata_raw` ~line 587)
- Create: `tests/backfill_cohort.rs` (public-API integration tests; auto-discovered, NO Cargo.toml block)

**Interfaces:**
- Consumes (existing, unchanged): `videos (video_id TEXT PK, source_url TEXT NOT NULL, status TEXT CHECK(...), …)`; `video_metadata_raw (video_id TEXT PK, fetched_at INTEGER, raw_json TEXT)`; `Store::open`; `Store::upsert_video(&mut self, video_id: &str, source_url: &str, canonical: bool) -> Result<usize>`; `Store::upsert_metadata_raw(&mut self, video_id: &str, envelope_json: &str) -> Result<usize>`; `unix_now()` (module-local helper in `state/mod.rs`).
- Produces (Tasks 03/04 rely on these exact names):
  - `pub struct MissingMetadataVideo { pub video_id: String, pub source_url: String }` (in `queries.rs`)
  - `Store::succeeded_missing_metadata_page(&self, after_video_id: Option<&str>, limit: usize) -> anyhow::Result<Vec<MissingMetadataVideo>>`
  - `Store::count_succeeded_missing_metadata(&self) -> anyhow::Result<u64>`
  - `Store::insert_metadata_raw_if_missing(&mut self, video_id: &str, envelope_json: &str) -> anyhow::Result<usize>` (1 = inserted, 0 = a row already exists — the caller counts it, never errors)

**Semantics (binding):**
- Cohort = `status = 'succeeded'` AND no `video_metadata_raw` row. Read-only; lives in `queries.rs` per the Epic 4b precedent.
- **Two cached statements chosen by the cursor** — copy `metadata_raw_page`'s shape exactly (`src/state/queries.rs:159-214`). The single `WHERE (?1 IS NULL OR video_id > ?1)` shape plans as a full ordered index scan ⇒ O(n²) over the 3M-row table; its doc comment records this. Do NOT use the OR-NULL shortcut.
- Anti-join via `NOT EXISTS` (PK probe on `video_metadata_raw` per outer row).
- No covering index exists for `status='succeeded'` (the only secondary index is pending-only, `idx_videos_pending_v3`); the page seeks on the `video_id` PK and filters status per row. Acceptable at cohort scale (~10K of ~3M); say so in the query's doc comment.
- `insert_metadata_raw_if_missing` is `INSERT … ON CONFLICT(video_id) DO NOTHING` — the backfill must never overwrite an envelope the fetch path wrote (codex-advisor design review 2026-07-29). Returns the row-change count per ADR-0006 (0 = already filled). The existing last-write-wins `upsert_metadata_raw` stays fetch-path-only; do not modify it.

- [ ] **Step 1: Write the failing integration tests**

Create `tests/backfill_cohort.rs`:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Cohort queries + insert-if-missing for backfill-metadata: succeeded
//! videos with no video_metadata_raw row. Public API only (Store::open +
//! pub upserts) plus raw rusqlite status flips, so this file needs no
//! `[[test]]` block per 0005.

use ddp_transcribe::state::Store;

/// Five videos: v1 succeeded+envelope, v2 succeeded (no envelope),
/// v3 pending, v4 succeeded (no envelope), v5 failed_terminal.
/// Cohort is exactly {v2, v4}.
fn seeded_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let db = dir.path().join("state.sqlite");
    {
        let mut store = Store::open(&db).unwrap();
        for (id, url) in [
            ("v1", "https://example/1"),
            ("v2", "https://example/2"),
            ("v3", "https://example/3"),
            ("v4", "https://example/4"),
            ("v5", "https://example/5"),
        ] {
            store.upsert_video(id, url, false).unwrap();
        }
        store
            .upsert_metadata_raw("v1", r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#)
            .unwrap();
    }
    // Flip statuses with raw rusqlite — no public mutator sets
    // `succeeded` without a claim, and tests must not grow one.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE videos SET status = 'succeeded' WHERE video_id IN ('v1','v2','v4')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE videos SET status = 'failed_terminal' WHERE video_id = 'v5'",
        [],
    )
    .unwrap();
    db
}

#[test]
fn cohort_is_succeeded_without_envelope_only() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let store = Store::open(&db).unwrap();

    assert_eq!(store.count_succeeded_missing_metadata().unwrap(), 2);
    let page = store.succeeded_missing_metadata_page(None, 100).unwrap();
    let ids: Vec<&str> = page.iter().map(|v| v.video_id.as_str()).collect();
    assert_eq!(ids, ["v2", "v4"], "ordered by video_id; excludes enveloped/pending/terminal");
    assert_eq!(page[0].source_url, "https://example/2");
}

#[test]
fn cohort_page_walks_all_rows_exactly_once_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let store = Store::open(&db).unwrap();

    // Page size 1 forces the cursor across both cached statements.
    let mut after: Option<String> = None;
    let mut walked = Vec::new();
    loop {
        let page = store
            .succeeded_missing_metadata_page(after.as_deref(), 1)
            .unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.video_id.clone());
        walked.extend(page.into_iter().map(|v| v.video_id));
    }
    assert_eq!(walked, ["v2", "v4"]);
}

#[test]
fn insert_if_missing_never_overwrites_and_shrinks_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let mut store = Store::open(&db).unwrap();

    // Fresh insert: 1 row changed, cohort shrinks.
    let changed = store
        .insert_metadata_raw_if_missing("v2", r#"{"schema":1,"printed":"{\"id\":\"v2\"}"}"#)
        .unwrap();
    assert_eq!(changed, 1);
    assert_eq!(store.count_succeeded_missing_metadata().unwrap(), 1);
    let page = store.succeeded_missing_metadata_page(None, 100).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].video_id, "v4");

    // Conflict: 0 rows changed, existing envelope untouched.
    let changed = store
        .insert_metadata_raw_if_missing("v1", r#"{"schema":1,"printed":"{\"id\":\"OVERWRITE\"}"}"#)
        .unwrap();
    assert_eq!(changed, 0, "existing row wins; backfill never overwrites");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT raw_json FROM video_metadata_raw WHERE video_id = 'v1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.contains(r#"\"id\":\"v1\""#), "raw was: {raw}");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test backfill_cohort -- --test-threads=1`
Expected: COMPILE FAIL (`MissingMetadataVideo` / methods absent).

- [ ] **Step 3: Implement the queries**

In `src/state/queries.rs`, follow the file's row-struct + `impl Store` read-only block style (place near `RawMetadataRow` / `metadata_raw_page`):

```rust
/// One succeeded video missing its raw metadata envelope — input row for
/// the backfill-metadata cohort walk.
#[derive(Debug)]
pub struct MissingMetadataVideo {
    pub video_id: String,
    pub source_url: String,
}
```

In the `impl Store` block:

```rust
    /// Size of the backfill cohort: succeeded videos with no
    /// video_metadata_raw row (the rc1-era gap). Read-only.
    pub fn count_succeeded_missing_metadata(&self) -> Result<u64> {
        let n: i64 = self
            .conn()
            .prepare_cached(
                "SELECT COUNT(*) FROM videos v
                 WHERE v.status = 'succeeded'
                   AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m
                                   WHERE m.video_id = v.video_id)",
            )?
            .query_row([], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// Keyset page over the backfill cohort, ordered by video_id.
    ///
    /// Two cached statements chosen by the cursor, not one OR-NULL
    /// statement — see `metadata_raw_page`'s comment for the O(n²) plan
    /// the single-statement shape produces. The page seeks on the
    /// videos PK and filters status per row (the only secondary index
    /// is pending-only); fine at cohort scale (~10K of ~3M rows).
    /// Rows leave the cohort as envelopes land, which is safe: the
    /// cursor only moves forward, and vanished rows are all behind it.
    /// Videos succeeding behind the cursor during a live run are caught
    /// by a re-run (rerun-to-convergence semantics).
    pub fn succeeded_missing_metadata_page(
        &self,
        after_video_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MissingMetadataVideo>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let map = |r: &rusqlite::Row<'_>| {
            Ok(MissingMetadataVideo {
                video_id: r.get(0)?,
                source_url: r.get(1)?,
            })
        };
        let rows = match after_video_id {
            None => {
                let mut stmt = self.conn().prepare_cached(
                    "SELECT v.video_id, v.source_url FROM videos v
                     WHERE v.status = 'succeeded'
                       AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m
                                       WHERE m.video_id = v.video_id)
                     ORDER BY v.video_id
                     LIMIT ?1",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![limit], map)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            }
            Some(after) => {
                let mut stmt = self.conn().prepare_cached(
                    "SELECT v.video_id, v.source_url FROM videos v
                     WHERE v.status = 'succeeded'
                       AND v.video_id > ?1
                       AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m
                                       WHERE m.video_id = v.video_id)
                     ORDER BY v.video_id
                     LIMIT ?2",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![after, limit], map)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                rows
            }
        };
        Ok(rows)
    }
```

(Match the file's actual import conventions — it already uses `anyhow::Result` and `rusqlite::params`. Do not `unwrap()`/`expect()` in production code.)

- [ ] **Step 4: Implement the mutator**

In `src/state/mod.rs`, directly below `upsert_metadata_raw` (~line 601), mirroring its doc-contract style:

```rust
    /// Insert a raw metadata envelope only if the video has none
    /// (backfill-metadata's write path). Unlike `upsert_metadata_raw`
    /// (fetch-path, last-write-wins), the backfill must never overwrite
    /// an envelope the fetch path captured. Returns the row-change
    /// count per 0006: 1 = inserted, 0 = a row already exists (the
    /// caller counts it; it is not an error). Best-effort contract as
    /// for `upsert_metadata_raw`: metadata writes never change a
    /// video's pipeline outcome.
    pub fn insert_metadata_raw_if_missing(
        &mut self,
        video_id: &str,
        envelope_json: &str,
    ) -> Result<usize> {
        let now = unix_now();
        let changed = self
            .conn
            .execute(
                "INSERT INTO video_metadata_raw (video_id, fetched_at, raw_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(video_id) DO NOTHING",
                params![video_id, now, envelope_json],
            )
            .with_context(|| format!("inserting backfill metadata for {video_id}"))?;
        Ok(changed)
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --test backfill_cohort -- --test-threads=1`
Expected: 3 passed.

- [ ] **Step 6: Verify the query plans (codex-advisor finding)**

Against any seeded scratch DB (e.g. the one a test leaves in a kept tempdir, or build one ad hoc):

```bash
sqlite3 /path/to/scratch.sqlite "EXPLAIN QUERY PLAN
  SELECT v.video_id, v.source_url FROM videos v
  WHERE v.status = 'succeeded'
    AND v.video_id > 'a'
    AND NOT EXISTS (SELECT 1 FROM video_metadata_raw m WHERE m.video_id = v.video_id)
  ORDER BY v.video_id LIMIT 10;"
```

Expected for BOTH statement shapes: outer `SEARCH videos ... USING INDEX sqlite_autoindex_videos_1 (video_id>?)` (first-page shape: `SCAN` via the same index, no temp B-tree), inner `SEARCH video_metadata_raw USING ... INDEX` PK probe, and **no `USE TEMP B-TREE FOR ORDER BY`**. Paste the output into the task report.

- [ ] **Step 7: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green; record the suite total for later tasks.

- [ ] **Step 8: Commit**

```bash
git add src/state/queries.rs src/state/mod.rs tests/backfill_cohort.rs
git commit -m "feat(state): backfill cohort queries + insert-if-missing metadata mutator"
```
