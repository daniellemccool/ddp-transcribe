//! Pre-Epic-2 → current schema migration ladder (0022; extended by Epic 4a
//! for v2→v3). Opens the DB raw, bypassing Store::open's version check;
//! runs the ladder + UPDATE meta inside one transaction. Idempotent on
//! already-migrated DBs.

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
    // Today two stages exist (v1→v2, v2→v3); Epic 4a+ will append more
    // blocks as the schema bumps further. Unknown starting versions
    // still bail below.
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
