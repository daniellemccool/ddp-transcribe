# Task 01: Schema v5 (`video_metadata_raw` + typed `videos` columns) + `Store::upsert_metadata_raw`

**Files:**
- Modify: `src/state/schema.rs` (SCHEMA_VERSION "4"→"5"; new table; 9 nullable `videos` columns)
- Modify: `src/state/migrate.rs` (ladder block v4→v5; module doc "three"→"four" stages)
- Modify: `src/state/mod.rs` (new mutator `upsert_metadata_raw`)
- Modify: `tests/state_migrate.rs` (v4→v5 coverage; fixtures gain nothing new — v4 fixtures already exist from Epic 4b)
- Create: `tests/state_metadata.rs` (public-API tests for the raw mutator; auto-discovered, NO Cargo.toml block)

**Interfaces:**
- Consumes: existing `Store::open`, `unix_now()`, migrate ladder shape.
- Produces (Tasks 03/04 rely on these EXACT items):
  - `Store::upsert_metadata_raw(&mut self, video_id: &str, envelope_json: &str) -> anyhow::Result<usize>` — INSERT-or-overwrite keyed by `video_id`, stamps `fetched_at = unix_now()`, returns row-change count per ADR-0006.
  - Schema v5: table `video_metadata_raw (video_id TEXT PRIMARY KEY NOT NULL, fetched_at INTEGER NOT NULL, raw_json TEXT NOT NULL, FOREIGN KEY (video_id) REFERENCES videos(video_id))`.
  - `videos` nullable columns: `video_description TEXT`, `uploader TEXT`, `uploader_id TEXT`, `video_created_at INTEGER`, `view_count INTEGER`, `like_count INTEGER`, `comment_count INTEGER`, `captions_json TEXT`, `metadata_fetched_at INTEGER`.

- [ ] **Step 1: Write the failing migrate test**

In `tests/state_migrate.rs`, add one test following the file's existing hand-built-DB style. The file already contains a v3-builder (from Epic 4b's `migrate_upgrades_v3_to_v4_idempotently`) — copy its construction into a v4 builder: identical tables PLUS `watch_history.watched_at_raw TEXT`, with `meta.schema_version = '4'`, one `videos` row, one `watch_history` row.

```rust
#[test]
fn migrate_upgrades_v4_to_v5_idempotently() {
    // ... hand-build a v4 DB at `db` (copy the v3 builder + watched_at_raw column,
    //     meta.schema_version = '4', one videos row 'vid_a') ...
    ddp_transcribe::state::migrate::run_migrate(&db).expect("v4→v5");
    ddp_transcribe::state::migrate::run_migrate(&db).expect("idempotent second run");

    let conn = rusqlite::Connection::open(&db).unwrap();
    let version: String = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, ddp_transcribe::state::SCHEMA_VERSION);

    // New table exists and is empty.
    let raw_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_metadata_raw", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw_count, 0);

    // Pre-v5 videos rows carry NULL in every new column.
    let (desc, fetched): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT video_description, metadata_fetched_at FROM videos WHERE video_id='vid_a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(desc.is_none() && fetched.is_none());
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test --test state_migrate migrate_upgrades_v4_to_v5 -- --test-threads=1`
Expected: FAIL — `run_migrate` errors with "already at current version"? No: the hand-built DB says v4 and `SCHEMA_VERSION` is still "4", so the second assertion path never runs; the first `expect` passes but the `video_metadata_raw` query fails with "no such table". Either failure mode is the RED state.

- [ ] **Step 3: Bump schema.rs**

In `src/state/schema.rs`: change line 1 to `pub const SCHEMA_VERSION: &str = "5";`. In `SCHEMA_SQL`, append to the `videos` CREATE TABLE (after `terminal_message`), keeping the comment style:

```sql
    -- Plan B Epic 4c (schema v5): typed metadata columns populated by the
    -- post-run `load-metadata` subcommand from video_metadata_raw blobs.
    -- All nullable; NULL = never loaded. metadata_fetched_at records the
    -- capture moment (engagement counts are point-in-time snapshots).
    video_description   TEXT,
    uploader            TEXT,
    uploader_id         TEXT,
    video_created_at    INTEGER,
    view_count          INTEGER,
    like_count          INTEGER,
    comment_count       INTEGER,
    captions_json       TEXT,
    metadata_fetched_at INTEGER,
```

and append the new table after `batch_runs`:

```sql
CREATE TABLE IF NOT EXISTS video_metadata_raw (
    -- Raw fetch-time metadata envelope (Epic 4c): versioned JSON wrapping
    -- yt-dlp's --print output UNPARSED plus any embedded caption tracks.
    -- One row per unique video, last-write-wins across retries. Parsed
    -- only by `load-metadata` — replayable without re-fetch.
    video_id   TEXT PRIMARY KEY NOT NULL,
    fetched_at INTEGER NOT NULL,
    raw_json   TEXT NOT NULL,
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
```

- [ ] **Step 4: Extend the migrate ladder**

In `src/state/migrate.rs`, module doc: update "Today three stages exist (v1→v2, v2→v3, v3→v4)" to four stages ending v4→v5. After the `if version == "3"` block:

```rust
    if version == "4" {
        tx.execute_batch(
            "ALTER TABLE videos ADD COLUMN video_description TEXT;
             ALTER TABLE videos ADD COLUMN uploader TEXT;
             ALTER TABLE videos ADD COLUMN uploader_id TEXT;
             ALTER TABLE videos ADD COLUMN video_created_at INTEGER;
             ALTER TABLE videos ADD COLUMN view_count INTEGER;
             ALTER TABLE videos ADD COLUMN like_count INTEGER;
             ALTER TABLE videos ADD COLUMN comment_count INTEGER;
             ALTER TABLE videos ADD COLUMN captions_json TEXT;
             ALTER TABLE videos ADD COLUMN metadata_fetched_at INTEGER;
             CREATE TABLE IF NOT EXISTS video_metadata_raw (
                 video_id   TEXT PRIMARY KEY NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 raw_json   TEXT NOT NULL,
                 FOREIGN KEY (video_id) REFERENCES videos(video_id)
             );",
        )
        .context("v4→v5: metadata columns ×9 + video_metadata_raw table")?;
        version = "5".to_string();
    }
```

- [ ] **Step 5: Fix stragglers the compiler/tests name**

Run: `cargo test --features test-helpers -- --test-threads=1 2>&1 | rg 'FAILED|panicked' | head`. Any test asserting `SCHEMA_VERSION == "4"` or hand-building "current-version" DBs must be updated (Epic 4b hit `tests/state_schema_v2.rs`'s literal — check it and `tests/state_schema_version.rs`). Fix only what failures name; disclose each in the commit per ADR-0003.

- [ ] **Step 6: Run the migrate test to verify it passes**

Run: `cargo test --test state_migrate -- --test-threads=1`
Expected: PASS including the new test and Epic 4b's v3→v4 test (its fixture now migrates v3→v4→v5; its version assertion reads `SCHEMA_VERSION` so it stays green).

- [ ] **Step 7: Write the failing mutator tests**

Create `tests/state_metadata.rs` (copy the `#![allow(clippy::unwrap_used, clippy::expect_used)]` header from `tests/state_migrate.rs`):

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `Store::upsert_metadata_raw` — Epic 4c raw envelope storage.
//! Public-API only (Store::open + raw rusqlite): auto-discovered, no
//! Cargo.toml [[test]] block per ADR-0005.

use ddp_transcribe::state::Store;

fn store_with_video(dir: &tempfile::TempDir) -> (Store, std::path::PathBuf) {
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store.upsert_video("vid_a", "https://example/a", false).unwrap();
    (store, db)
}

#[test]
fn upsert_metadata_raw_inserts_and_returns_one() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, db) = store_with_video(&dir);

    let n = store
        .upsert_metadata_raw("vid_a", r#"{"schema":1,"printed":"{}","captions":null}"#)
        .unwrap();
    assert_eq!(n, 1);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let (raw, fetched_at): (String, i64) = conn
        .query_row(
            "SELECT raw_json, fetched_at FROM video_metadata_raw WHERE video_id='vid_a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(raw.contains(r#""schema":1"#));
    assert!(fetched_at > 0);
}

#[test]
fn upsert_metadata_raw_overwrites_last_write_wins() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, db) = store_with_video(&dir);

    store.upsert_metadata_raw("vid_a", r#"{"schema":1,"printed":"first","captions":null}"#).unwrap();
    let n = store
        .upsert_metadata_raw("vid_a", r#"{"schema":1,"printed":"second","captions":null}"#)
        .unwrap();
    assert_eq!(n, 1);

    let conn = rusqlite::Connection::open(&db).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM video_metadata_raw", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "keyed upsert: one row per video");
    let raw: String = conn
        .query_row("SELECT raw_json FROM video_metadata_raw WHERE video_id='vid_a'", [], |r| r.get(0))
        .unwrap();
    assert!(raw.contains("second"), "last write wins");
}
```

(If `upsert_video`'s exact signature differs — check `src/state/mod.rs` around `UPSERT_VIDEO_SQL` — adapt the fixture call; `tests/state_migrate.rs` and `src/pipeline/serial.rs`'s tests show working call shapes, e.g. `store.upsert_video("vid_a", "https://example/a", false)`.)

- [ ] **Step 8: Run to confirm both fail** (no such method)

Run: `cargo test --test state_metadata -- --test-threads=1`
Expected: COMPILE FAIL — `upsert_metadata_raw` not found.

- [ ] **Step 9: Implement the mutator**

In `src/state/mod.rs`, inside the main `impl Store` block (near `mark_succeeded`, matching its doc-comment style):

```rust
    /// Insert or overwrite the raw fetch-time metadata envelope for a video
    /// (Epic 4c). Keyed by video_id — one row per unique video, last write
    /// wins across retries (engagement counts are point-in-time; fetched_at
    /// records the snapshot moment). Returns the row-change count per 0006.
    ///
    /// Callers treat failures as best-effort (log + continue): metadata
    /// must never change a video's pipeline outcome.
    pub fn upsert_metadata_raw(&mut self, video_id: &str, envelope_json: &str) -> Result<usize> {
        let now = unix_now();
        let changed = self
            .conn
            .execute(
                "INSERT INTO video_metadata_raw (video_id, fetched_at, raw_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(video_id) DO UPDATE SET
                     fetched_at = excluded.fetched_at,
                     raw_json   = excluded.raw_json",
                params![video_id, now, envelope_json],
            )
            .with_context(|| format!("upsert_metadata_raw for {video_id}"))?;
        Ok(changed)
    }
```

(`ON CONFLICT … DO UPDATE` rather than `INSERT OR REPLACE`: REPLACE deletes+reinserts, which interacts badly with the FOREIGN KEY and fires delete triggers; the upsert form is also the `meta.schema_version` precedent in `migrate.rs`.)

- [ ] **Step 10: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green (baseline 283 + 3 new = 286 passed; sum the `test result:` lines).

- [ ] **Step 11: Commit**

```bash
git add src/state/schema.rs src/state/migrate.rs src/state/mod.rs tests/state_migrate.rs tests/state_metadata.rs
git commit -m "feat(state): schema v5 — video_metadata_raw envelope table + typed metadata columns on videos"
```

Disclose in the commit body any straggler test fixed in Step 5 (ADR-0003).
