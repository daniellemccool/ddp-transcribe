# Task 04: `load-metadata` subcommand — streaming, batched, replayable loader

**Files:**
- Create: `src/metadata_loader.rs` (envelope/printed parsing + orchestration + `LoadStats`)
- Modify: `src/state/queries.rs` (keyset page query over `video_metadata_raw`)
- Modify: `src/state/mod.rs` (`MetadataColumns` + `Store::apply_metadata_batch`)
- Modify: `src/cli.rs` (`Command::LoadMetadata { dry_run }`)
- Modify: `src/main.rs` (arm: missing-DB bail → `Store::open` → loader → log stats; `log_resolved_config` gains the exhaustive-match arm; `mod metadata_loader;` registration)
- Create: `tests/load_metadata.rs` (binary + public-API integration tests; auto-discovered, NO Cargo.toml block)

**Interfaces:**
- Consumes (exact, landed in Task 01): `video_metadata_raw (video_id, fetched_at, raw_json)`; `videos` columns (8) `video_description, uploader, uploader_id, video_created_at, view_count, like_count, comment_count, metadata_fetched_at`; envelope contract `{"schema":1,"printed":"<json string>"}` where `printed` parses to yt-dlp fields `id,title,description,uploader,uploader_id,channel_id,timestamp,duration,view_count,like_count,comment_count,repost_count` (any may be null/absent). Captions/subtitles are descoped (operator decision 2026-07-28) — the envelope has no `captions` key and `videos` has no `captions_json` column.
- Produces:
  - `queries.rs`: `pub struct RawMetadataRow { pub video_id: String, pub fetched_at: i64, pub raw_json: String }` and `Store::metadata_raw_page(&self, after_video_id: Option<&str>, limit: usize) -> anyhow::Result<Vec<RawMetadataRow>>` (keyset pagination `WHERE video_id > ?`, `ORDER BY video_id`, read-only).
  - `state/mod.rs`: `pub struct MetadataColumns { pub video_id: String, pub video_description: Option<String>, pub uploader: Option<String>, pub uploader_id: Option<String>, pub video_created_at: Option<i64>, pub view_count: Option<i64>, pub like_count: Option<i64>, pub comment_count: Option<i64>, pub metadata_fetched_at: i64 }` and `Store::apply_metadata_batch(&mut self, rows: &[MetadataColumns]) -> anyhow::Result<usize>` (one transaction per call, `prepare_cached` UPDATE, returns total changed per ADR-0006).
  - `metadata_loader.rs`: `pub struct LoadStats { pub rows_examined: u64, pub rows_loaded: u64, pub rows_skipped_unparseable: u64, pub rows_without_video: u64 }` (input-side, verb-named per ADR-0007, `Serialize` + `Display`) and `pub fn load_metadata(store: &mut Store, dry_run: bool) -> anyhow::Result<LoadStats>`.

**Semantics (binding):**
- Streaming: pages of 10,000 raw rows via keyset pagination — never collect the whole table (6–12 GB at production scale).
- Idempotent + replayable: re-running overwrites columns from current blobs (last-write-wins; `metadata_fetched_at = raw.fetched_at`).
- Unparseable envelope or unparseable `printed` ⇒ `rows_skipped_unparseable += 1`, warn-log with video_id, continue. NEVER fatal (epic invariant).
- `apply_metadata_batch` UPDATE matching 0 rows (raw row exists but the videos row is gone) ⇒ counted via `rows_without_video` (computed as batch-size minus changed-count), not an error.
- `--dry-run`: full examine/parse pass, all stats real, zero writes.
- Field mapping: `description`→`video_description`; `uploader`→`uploader`; `uploader_id`→`uploader_id`; `timestamp`→`video_created_at`; `view_count`/`like_count`/`comment_count` direct. `title`, `channel_id`, `duration`, `repost_count` stay raw-only (deliberately wider print set).

- [ ] **Step 1: Write the failing loader unit tests**

In `src/metadata_loader.rs` `mod tests` (write the module skeleton first: types above, `mod tests` below):

```rust
    #[test]
    fn parse_envelope_maps_printed_fields_to_columns() {
        let envelope = r#"{"schema":1,"printed":"{\"id\":\"v1\",\"description\":\"hello #tag\",\"uploader\":\"acct\",\"uploader_id\":\"123\",\"timestamp\":1768924271,\"view_count\":9900000,\"like_count\":572300,\"comment_count\":865}"}"#;
        let cols = parse_envelope("v1", 1753700000, envelope).expect("parses");
        assert_eq!(cols.video_id, "v1");
        assert_eq!(cols.video_description.as_deref(), Some("hello #tag"));
        assert_eq!(cols.uploader.as_deref(), Some("acct"));
        assert_eq!(cols.uploader_id.as_deref(), Some("123"));
        assert_eq!(cols.video_created_at, Some(1768924271));
        assert_eq!(cols.view_count, Some(9_900_000));
        assert_eq!(cols.like_count, Some(572_300));
        assert_eq!(cols.comment_count, Some(865));
        assert_eq!(cols.metadata_fetched_at, 1753700000);
    }

    #[test]
    fn parse_envelope_absent_fields_become_null() {
        let envelope = r#"{"schema":1,"printed":"{\"id\":\"v1\"}"}"#;
        let cols = parse_envelope("v1", 1, envelope).expect("parses");
        assert!(cols.video_description.is_none() && cols.view_count.is_none());
    }

    #[test]
    fn parse_envelope_rejects_garbage_and_bad_printed() {
        assert!(parse_envelope("v1", 1, "not json").is_none());
        assert!(parse_envelope("v1", 1, r#"{"schema":1,"printed":"not json"}"#).is_none());
        // Unknown future schema version: skip, don't guess.
        assert!(parse_envelope("v1", 1, r#"{"schema":2,"printed":"{}"}"#).is_none());
    }
```

- [ ] **Step 2: Run to confirm failure** — `cargo test parse_envelope -- --test-threads=1` → COMPILE FAIL (module/function absent).

- [ ] **Step 3: Implement `src/metadata_loader.rs`**

```rust
//! Post-run metadata loader (Epic 4c): parses `video_metadata_raw`
//! envelopes into typed `videos` columns. Streaming (keyset pages),
//! batched (one tx per page), idempotent, replayable — a parse bug is
//! fixed by re-running, never by re-fetching.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::state::{MetadataColumns, Store};

const PAGE_SIZE: usize = 10_000;

/// Loader stats: input-side counters, verb-named (ADR-0007).
#[derive(Debug, Default, Serialize)]
pub struct LoadStats {
    /// Raw rows examined this pass.
    pub rows_examined: u64,
    /// Rows whose columns were written (dry-run counts them as loadable).
    pub rows_loaded: u64,
    /// Rows skipped because the envelope or its printed line failed to
    /// parse (or carried an unknown schema version). Never fatal.
    pub rows_skipped_unparseable: u64,
    /// Parsed rows whose videos row no longer exists (UPDATE matched 0).
    pub rows_without_video: u64,
}

impl std::fmt::Display for LoadStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "examined {} / loaded {} / skipped-unparseable {} / without-video {}",
            self.rows_examined,
            self.rows_loaded,
            self.rows_skipped_unparseable,
            self.rows_without_video
        )
    }
}

#[derive(Deserialize)]
struct Envelope {
    schema: u32,
    printed: String,
}

/// The typed subset of yt-dlp's printed fields the loader maps to columns.
/// Everything else in `printed` stays raw-only by design.
#[derive(Deserialize)]
struct PrintedFields {
    description: Option<String>,
    uploader: Option<String>,
    uploader_id: Option<String>,
    timestamp: Option<i64>,
    view_count: Option<i64>,
    like_count: Option<i64>,
    comment_count: Option<i64>,
}

/// Parse one envelope into column values. `None` = unparseable (caller
/// counts + warns; never fatal per the epic invariant).
fn parse_envelope(video_id: &str, fetched_at: i64, raw_json: &str) -> Option<MetadataColumns> {
    let env: Envelope = serde_json::from_str(raw_json).ok()?;
    if env.schema != 1 {
        return None;
    }
    let printed: PrintedFields = serde_json::from_str(&env.printed).ok()?;
    Some(MetadataColumns {
        video_id: video_id.to_string(),
        video_description: printed.description,
        uploader: printed.uploader,
        uploader_id: printed.uploader_id,
        video_created_at: printed.timestamp,
        view_count: printed.view_count,
        like_count: printed.like_count,
        comment_count: printed.comment_count,
        metadata_fetched_at: fetched_at,
    })
}

/// One full pass over video_metadata_raw. Streaming keyset pagination;
/// one write transaction per page via `Store::apply_metadata_batch`.
pub fn load_metadata(store: &mut Store, dry_run: bool) -> Result<LoadStats> {
    let mut stats = LoadStats::default();
    let mut after: Option<String> = None;

    loop {
        let page = store.metadata_raw_page(after.as_deref(), PAGE_SIZE)?;
        let Some(last) = page.last() else { break };
        after = Some(last.video_id.clone());

        let mut batch: Vec<MetadataColumns> = Vec::with_capacity(page.len());
        for row in &page {
            stats.rows_examined += 1;
            match parse_envelope(&row.video_id, row.fetched_at, &row.raw_json) {
                Some(cols) => batch.push(cols),
                None => {
                    stats.rows_skipped_unparseable += 1;
                    tracing::warn!(video_id = row.video_id.as_str(), "unparseable metadata envelope; skipped");
                }
            }
        }

        if dry_run {
            stats.rows_loaded += batch.len() as u64;
        } else {
            let changed = store.apply_metadata_batch(&batch)?;
            stats.rows_loaded += changed as u64;
            stats.rows_without_video += (batch.len() - changed) as u64;
        }
    }
    Ok(stats)
}
```

(Then `cargo test parse_envelope -- --test-threads=1` → compile still fails on `MetadataColumns`/`metadata_raw_page` — proceed to the state layer before green.)

- [ ] **Step 4: State layer — page query + batch mutator**

`src/state/queries.rs` (follow the file's existing `impl Store` read-only block + row-struct style):

```rust
/// One raw metadata envelope row (Epic 4c loader input).
#[derive(Debug)]
pub struct RawMetadataRow {
    pub video_id: String,
    pub fetched_at: i64,
    pub raw_json: String,
}

// in the impl Store block:
    /// Keyset page over video_metadata_raw ordered by video_id. Streaming
    /// input for `load_metadata` — never materializes the whole table
    /// (6–12 GB at production scale).
    pub fn metadata_raw_page(
        &self,
        after_video_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RawMetadataRow>> {
        let mut stmt = self.conn().prepare_cached(
            "SELECT video_id, fetched_at, raw_json FROM video_metadata_raw
             WHERE (?1 IS NULL OR video_id > ?1)
             ORDER BY video_id
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![after_video_id, limit as i64], |r| {
                Ok(RawMetadataRow {
                    video_id: r.get(0)?,
                    fetched_at: r.get(1)?,
                    raw_json: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
```

`src/state/mod.rs` (near `SuccessArtifacts`, mirroring mutator style):

```rust
/// Typed column values for one video, produced by the metadata loader
/// (Epic 4c). All-nullable except the snapshot timestamp.
#[derive(Debug, Clone)]
pub struct MetadataColumns {
    pub video_id: String,
    pub video_description: Option<String>,
    pub uploader: Option<String>,
    pub uploader_id: Option<String>,
    pub video_created_at: Option<i64>,
    pub view_count: Option<i64>,
    pub like_count: Option<i64>,
    pub comment_count: Option<i64>,
    pub metadata_fetched_at: i64,
}

// in impl Store:
    /// Apply one loader batch in a single transaction (Epic 4c). Overwrites
    /// unconditionally — last-write-wins replay semantics. Returns the
    /// total row-change count per 0006; a row whose video_id no longer
    /// exists in videos contributes 0 (the loader counts it, not an error).
    pub fn apply_metadata_batch(&mut self, rows: &[MetadataColumns]) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("begin immediate for apply_metadata_batch")?;
        let mut changed = 0usize;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE videos SET
                     video_description = ?2, uploader = ?3, uploader_id = ?4,
                     video_created_at = ?5, view_count = ?6, like_count = ?7,
                     comment_count = ?8, metadata_fetched_at = ?9
                 WHERE video_id = ?1",
            )?;
            for row in rows {
                changed += stmt
                    .execute(params![
                        row.video_id,
                        row.video_description,
                        row.uploader,
                        row.uploader_id,
                        row.video_created_at,
                        row.view_count,
                        row.like_count,
                        row.comment_count,
                        row.metadata_fetched_at,
                    ])
                    .with_context(|| format!("apply_metadata_batch for {}", row.video_id))?;
            }
        }
        tx.commit().context("commit apply_metadata_batch")?;
        Ok(changed)
    }
```

Register `pub mod metadata_loader;` where the crate's modules are declared (`src/lib.rs` or `src/main.rs` module list — match how `ingest`/`status` are registered; check `rg 'mod status' src/`).

Run: `cargo test parse_envelope -- --test-threads=1` → PASS (4/4).

- [ ] **Step 5: CLI + main arm**

`src/cli.rs`, append to `Command`:

```rust
    /// Parse captured raw metadata (video_metadata_raw) into the typed
    /// videos columns. Post-run; idempotent and replayable — re-running
    /// overwrites from the current blobs.
    LoadMetadata {
        /// Examine and parse everything, write nothing, report counts.
        #[arg(long)]
        dry_run: bool,
    },
```

`src/main.rs`:
- `log_resolved_config`'s exhaustive match gains a `Command::LoadMetadata { .. }` arm — mirror the Status arm's non-model shape (log `profile` + `state_db`, no `whisper_model_path`).
- Dispatch arm, mirroring the Migrate arm's missing-DB bail exactly:

```rust
        Command::LoadMetadata { dry_run } => {
            if !cli.global.state_db.exists() {
                anyhow::bail!(
                    "load-metadata: state DB not found at {} (run init/ingest first)",
                    cli.global.state_db.display()
                );
            }
            let mut store = Store::open(&cli.global.state_db)?;
            let stats = ddp_transcribe::metadata_loader::load_metadata(&mut store, dry_run)?;
            tracing::info!(%stats, dry_run, "load-metadata complete");
            println!("load-metadata: {stats}{}", if dry_run { " (dry-run)" } else { "" });
        }
```

(Adapt the import path to the crate's actual layout — main.rs uses `crate::`-style module paths if the binary and lib share `src/`; copy how the Status arm calls into `status`/`state`. Error-message wording must match the Migrate/RecomputeWindow bail phrasing style.)

- [ ] **Step 6: Write the failing integration tests**

Create `tests/load_metadata.rs` (header from an existing tests file; assert_cmd + raw rusqlite, public API only — auto-discovered):

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `load-metadata` end-to-end: seeded raw envelopes → typed columns.

use assert_cmd::Command as AssertCommand;

fn seeded_db(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let db = dir.path().join("state.sqlite");
    let mut store = ddp_transcribe::state::Store::open(&db).unwrap();
    store.upsert_video("vid_a", "https://example/a", false).unwrap();
    store.upsert_video("vid_b", "https://example/b", false).unwrap();
    store
        .upsert_metadata_raw(
            "vid_a",
            r#"{"schema":1,"printed":"{\"id\":\"vid_a\",\"description\":\"desc A\",\"uploader\":\"acct\",\"timestamp\":1768924271,\"view_count\":42}"}"#,
        )
        .unwrap();
    store
        .upsert_metadata_raw("vid_b", "definitely not json")
        .unwrap();
    db
}

#[test]
fn load_metadata_populates_columns_and_reports_stats() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);

    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("examined 2"), "stdout was: {out}");
    assert!(out.contains("loaded 1") && out.contains("skipped-unparseable 1"));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let (desc, uploader, created, views, fetched): (Option<String>, Option<String>, Option<i64>, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT video_description, uploader, video_created_at, view_count, metadata_fetched_at
             FROM videos WHERE video_id='vid_a'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(desc.as_deref(), Some("desc A"));
    assert_eq!(uploader.as_deref(), Some("acct"));
    assert_eq!(created, Some(1768924271));
    assert_eq!(views, Some(42));
    assert!(fetched.is_some(), "metadata_fetched_at stamped from raw row");

    // vid_b (unparseable) untouched.
    let b_desc: Option<String> = conn
        .query_row("SELECT video_description FROM videos WHERE video_id='vid_b'", [], |r| r.get(0))
        .unwrap();
    assert!(b_desc.is_none());
}

#[test]
fn load_metadata_is_idempotent_and_replayable() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    for _ in 0..2 {
        AssertCommand::cargo_bin("ddp-transcribe")
            .unwrap()
            .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
            .assert()
            .success();
    }
    let conn = rusqlite::Connection::open(&db).unwrap();
    let desc: Option<String> = conn
        .query_row("SELECT video_description FROM videos WHERE video_id='vid_a'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(desc.as_deref(), Some("desc A"), "second run reproduces, not corrupts");
}

#[test]
fn load_metadata_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = seeded_db(&dir);
    let assert = AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "load-metadata", "--dry-run"])
        .assert()
        .success();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("dry-run"));

    let conn = rusqlite::Connection::open(&db).unwrap();
    let desc: Option<String> = conn
        .query_row("SELECT video_description FROM videos WHERE video_id='vid_a'", [], |r| r.get(0))
        .unwrap();
    assert!(desc.is_none(), "--dry-run must not write");
}

#[test]
fn load_metadata_refuses_missing_db() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("nope.sqlite");
    AssertCommand::cargo_bin("ddp-transcribe")
        .unwrap()
        .args(["--state-db", db.to_str().unwrap(), "load-metadata"])
        .assert()
        .failure();
    assert!(!db.exists(), "must not create an empty DB");
}
```

(Adapt `upsert_video` call shape to the real signature if it differs — see Task 01's note. If `upsert_video` requires `--features test-helpers` gating, this file switches to seeding via raw rusqlite `INSERT INTO videos …` with the NOT NULL columns filled — keeping the file registration-free per ADR-0005.)

- [ ] **Step 7: Run to confirm failures, then make green**

Run: `cargo test --test load_metadata -- --test-threads=1` — fix compile/wiring issues until all 4 pass alongside the unit tests.

- [ ] **Step 8: Full verification**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test --features test-helpers -- --test-threads=1`
Expected: all green (Task 03's 293 + 8 new = 301 passed).

- [ ] **Step 9: Commit**

```bash
git add src/metadata_loader.rs src/state/queries.rs src/state/mod.rs src/cli.rs src/main.rs tests/load_metadata.rs
git commit -m "feat(cli): load-metadata — streaming, batched, replayable raw-envelope loader into schema-v5 columns"
```
