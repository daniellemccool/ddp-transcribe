//! Pre-Epic-2 → Epic 2 schema migration (0022). Opens the DB raw,
//! bypassing Store::open's version check; runs ALTER TABLE + UPDATE meta
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
            "migrate: DB is at v{found}, binary expects v{}. Downgrade not supported.",
            SCHEMA_VERSION
        );
    }

    // v1 → v2 migration. Adding the migration ladder here: for each
    // version pair, run the corresponding ALTER block. Today only one
    // upgrade exists; Epic 3+ will append more blocks as the schema
    // bumps further.
    let tx = conn
        .transaction()
        .context("begin transaction for v1→v2 migrate")?;

    if found == "1" {
        tx.execute_batch(
            "ALTER TABLE videos ADD COLUMN last_retryable_kind TEXT;
             ALTER TABLE videos ADD COLUMN last_retryable_message TEXT;
             ALTER TABLE videos ADD COLUMN terminal_reason TEXT;
             ALTER TABLE videos ADD COLUMN terminal_message TEXT;",
        )
        .context("v1→v2: ALTER TABLE videos ADD COLUMN ×4")?;
    } else {
        anyhow::bail!(
            "migrate: don't know how to upgrade from v{found} to v{}",
            SCHEMA_VERSION
        );
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
