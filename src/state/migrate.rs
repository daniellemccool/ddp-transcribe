//! Pre-Epic-2 → current schema migration ladder (0022; extended by Epic 4a
//! for v2→v3, Epic 4b for v3→v4, Epic 4c for v4→v5, the ingest
//! production hardening for v5→v6, and ADR-0048 (claim newest-published
//! first) for v6→v7). Opens the DB raw,
//! bypassing Store::open's version check; runs the ladder + UPDATE meta
//! inside one transaction. Idempotent on already-migrated DBs.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::SCHEMA_VERSION;

/// Run the migration. Idempotent: no-op if `meta.schema_version` already
/// matches `SCHEMA_VERSION`. Hard-fails if the recorded version is newer
/// than this binary (downgrade not supported).
pub fn run_migrate(path: &Path) -> Result<()> {
    let mut conn = Connection::open(path)
        .with_context(|| format!("opening DB for migrate at {}", path.display()))?;

    // Read current version (raw — no schema apply, no version check).
    let found: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .context("reading schema_version from meta")?;

    let found = match found {
        Some(v) => v,
        None => {
            // No meta.schema_version row at all. Treat as v1 (Plan A) since
            // the schema apply path in Store::open would have inserted it
            // for any non-pre-Plan-A DB. 0022 records this as the
            // pre-Plan-A migration target if ever needed.
            "1".to_string()
        }
    };

    if found == SCHEMA_VERSION {
        tracing::info!(
            version = SCHEMA_VERSION,
            "migrate: already at current version, no-op"
        );
        return Ok(());
    }

    if found
        .parse::<u32>()
        .ok()
        .is_some_and(|n| n > SCHEMA_VERSION.parse::<u32>().unwrap_or(0))
    {
        anyhow::bail!(
            "migrate: DB is at v{found}, binary expects v{SCHEMA_VERSION}. Downgrade not supported."
        );
    }

    // Sequential ladder: each stage advances a local `version` string.
    // Today six stages exist (v1→v2, v2→v3, v3→v4, v4→v5, v5→v6, v6→v7);
    // future epics will append more blocks as the schema bumps further.
    // Unknown starting versions still bail below.
    let tx = conn
        .transaction()
        .context("begin transaction for schema migrate")?;

    let mut version = found.clone();

    if version == "1" {
        tx.execute_batch(
            "ALTER TABLE videos ADD COLUMN last_retryable_kind TEXT;
             ALTER TABLE videos ADD COLUMN last_retryable_message TEXT;
             ALTER TABLE videos ADD COLUMN terminal_reason TEXT;
             ALTER TABLE videos ADD COLUMN terminal_message TEXT;",
        )
        .context("v1→v2: ALTER TABLE videos ADD COLUMN ×4")?;
        version = "2".to_string();
    }

    if version == "2" {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS batch_runs (
                 run_id       INTEGER PRIMARY KEY,
                 started_at   INTEGER NOT NULL,
                 finished_at  INTEGER,
                 params_json  TEXT NOT NULL,
                 policy_toml  TEXT NOT NULL,
                 census_json  TEXT
             );
             DROP INDEX IF EXISTS idx_videos_pending;
             CREATE INDEX IF NOT EXISTS idx_videos_pending_v3
                 ON videos (status, attempt_count, first_seen_at, video_id)
                 WHERE status = 'pending';",
        )
        .context("v2→v3: batch_runs + attempt-aware pending index")?;
        version = "3".to_string();
    }

    if version == "3" {
        tx.execute_batch("ALTER TABLE watch_history ADD COLUMN watched_at_raw TEXT;")
            .context("v3→v4: watch_history.watched_at_raw")?;
        version = "4".to_string();
    }

    if version == "4" {
        tx.execute_batch(
            "ALTER TABLE videos ADD COLUMN video_description TEXT;
             ALTER TABLE videos ADD COLUMN uploader TEXT;
             ALTER TABLE videos ADD COLUMN uploader_id TEXT;
             ALTER TABLE videos ADD COLUMN video_created_at INTEGER;
             ALTER TABLE videos ADD COLUMN view_count INTEGER;
             ALTER TABLE videos ADD COLUMN like_count INTEGER;
             ALTER TABLE videos ADD COLUMN comment_count INTEGER;
             ALTER TABLE videos ADD COLUMN metadata_fetched_at INTEGER;
             CREATE TABLE IF NOT EXISTS video_metadata_raw (
                 video_id   TEXT PRIMARY KEY NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 raw_json   TEXT NOT NULL,
                 FOREIGN KEY (video_id) REFERENCES videos(video_id)
             );",
        )
        .context("v4→v5: metadata columns ×8 + video_metadata_raw table")?;
        version = "5".to_string();
    }

    if version == "5" {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS ingested_files (
                 file_name   TEXT PRIMARY KEY NOT NULL,
                 size_bytes  INTEGER NOT NULL,
                 mtime       INTEGER NOT NULL,
                 ingested_at INTEGER NOT NULL
             );",
        )
        .context("v5→v6: ingested_files ledger table")?;
        // Deliberately NOT backfilled from the existing watch_history rows:
        // the ledger's contract is "this exact (name, size, mtime) triple was
        // fully committed", and a migrated DB has no record of which files
        // produced its rows. An empty ledger means the first post-migration
        // run pays the full walk once, then every later run is fast.
        version = "6".to_string();
    }

    if version == "6" {
        // Recency order relies on fixed-width DIGIT ids: lexicographic DESC
        // on TEXT equals numeric DESC only when every id is the same length
        // AND all-digits — a 19-character id with a trailing letter breaks
        // the guarantee just as badly as the wrong width does. Claim-order
        // ADR (0048): refuse the migration rather than mis-order claims.
        let bad: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM videos
                 WHERE canonical = 1
                   AND (LENGTH(video_id) != 19 OR video_id GLOB '*[^0-9]*')",
                [],
                |r| r.get(0),
            )
            .context("v6→v7: canonical id-width census")?;
        if bad != 0 {
            anyhow::bail!(
                "v6→v7: {bad} canonical rows have non-19-digit video_ids; \
                 recency claim order requires fixed-width 19-digit ids — refusing to migrate"
            );
        }
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_videos_pending_v3;
             CREATE INDEX IF NOT EXISTS idx_videos_pending_v4
                 ON videos (status, attempt_count, video_id DESC)
                 WHERE status = 'pending';",
        )
        .context("v6→v7: recency claim index")?;
        version = "7".to_string();
    }

    if version != SCHEMA_VERSION {
        anyhow::bail!("migrate: don't know how to upgrade from v{found} to v{SCHEMA_VERSION}");
    }

    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SCHEMA_VERSION],
    )
    .context("upsert meta.schema_version to current")?;

    tx.commit().context("commit migrate transaction")?;

    tracing::info!(from = %found, to = SCHEMA_VERSION, "migrate: complete");
    Ok(())
}
