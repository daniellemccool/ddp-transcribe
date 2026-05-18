# Task 3 — `migrate` CLI subcommand: ALTER TABLE + UPDATE meta in one transaction

**Goal:** Add a `migrate` subcommand that opens the DB **raw** (bypassing `Store::open`'s version check, which would reject a v1 DB), runs `ALTER TABLE videos ADD COLUMN ... NULL` × 4 + `UPDATE meta SET value='2'` inside one transaction, and commits. Idempotent on v2 DBs (no-op if already migrated).

**ADRs touched:** AD0021 (schema-version policy).

**Files:**
- Modify: `src/cli.rs` (add `Migrate` variant)
- Modify: `src/main.rs` (dispatch arm)
- Create: `src/state/migrate.rs` (new module containing the migration function)
- Modify: `src/state/mod.rs` (`mod migrate;` declaration)
- Create: `tests/state_migrate.rs` (integration test against a synthesized v1 fixture)

**Pre-reqs:** T1 + T2 complete (AD0021 decided; SchemaVersionMismatch error exists). **NOTE:** T4 (which bumps SCHEMA_VERSION constant and adds the four columns to the CREATE TABLE block) MUST land alongside or before T3 for the integration test's "post-migrate Store::open succeeds" assertion to work end-to-end. Implementer can choose to land T3+T4 as a paired commit or land T4 first; if T3 lands alone, mark the post-migrate-open assertion `#[ignore]` until T4.

---

- [ ] **Step 1: Add `Migrate` variant to `Command` enum in `src/cli.rs`**

Append to the `Command` enum:

```rust
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create state.sqlite and apply schema. Idempotent.
    Init,
    /// Walk --inbox, parse DDP JSONs, upsert into videos and watch_history.
    Ingest {
        #[arg(long)]
        dry_run: bool,
    },
    /// Run a batch: claim pending videos, fetch + transcribe, write artifacts.
    Process {
        #[arg(long)]
        max_videos: Option<usize>,
    },
    /// Upgrade a pre-Epic-2 (v1) state.sqlite to the current schema version.
    /// Idempotent: no-op if already at the current version.
    Migrate,
}
```

- [ ] **Step 2: Write the failing test**

Create `tests/state_migrate.rs`:

```rust
//! Migration test: synthesize a v1 DB (no new Epic 2 columns; meta.schema_version='1'),
//! run the migrate function, confirm v2 columns are present and meta.schema_version='2'.
//! Then run Store::open and confirm it succeeds (round-trip with T2's check).

use anyhow::Result;
use rusqlite::Connection;
use tempfile::TempDir;
use uu_tiktok::state::{migrate::run_migrate, Store};

/// Synthesize a Plan A v1 schema (no Epic 2 columns) at `path`.
fn synthesize_v1_db(path: &std::path::Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;

         CREATE TABLE IF NOT EXISTS videos (
             video_id            TEXT PRIMARY KEY NOT NULL,
             source_url          TEXT NOT NULL,
             canonical           INTEGER NOT NULL,
             status              TEXT NOT NULL CHECK (status IN
                                   ('pending','in_progress','succeeded','failed_terminal','failed_retryable')),
             claimed_by          TEXT,
             claimed_at          INTEGER,
             attempt_count       INTEGER NOT NULL DEFAULT 0,
             succeeded_at        INTEGER,
             duration_s          REAL,
             language_detected   TEXT,
             fetcher             TEXT,
             transcript_source   TEXT,
             first_seen_at       INTEGER NOT NULL,
             updated_at          INTEGER NOT NULL
         );

         CREATE TABLE IF NOT EXISTS meta (
             key   TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL
         );

         INSERT INTO meta (key, value) VALUES ('schema_version', '1');
        ",
    )?;
    Ok(())
}

fn columns_in(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn migrate_v1_to_v2_adds_columns_and_bumps_version() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    synthesize_v1_db(&path)?;

    // Pre-migrate: confirm v1 shape.
    {
        let raw = Connection::open(&path)?;
        let cols = columns_in(&raw, "videos");
        assert!(!cols.contains(&"last_retryable_kind".to_string()), "v1 lacks new column");
        let v: String = raw.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(v, "1");
    }

    run_migrate(&path)?;

    // Post-migrate: confirm v2 shape.
    {
        let raw = Connection::open(&path)?;
        let cols = columns_in(&raw, "videos");
        assert!(cols.contains(&"last_retryable_kind".to_string()));
        assert!(cols.contains(&"last_retryable_message".to_string()));
        assert!(cols.contains(&"terminal_reason".to_string()));
        assert!(cols.contains(&"terminal_message".to_string()));
        let v: String = raw.query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(v, "2");
    }

    // Round-trip with T2's Store::open: should succeed now.
    let _store = Store::open(&path)?;
    Ok(())
}

#[test]
fn migrate_is_idempotent_on_v2() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    // Fresh DB at current SCHEMA_VERSION (v2 after T4).
    let _ = Store::open(&path)?;

    // Migrate is a no-op on v2.
    run_migrate(&path)?;
    run_migrate(&path)?; // second run also no-op

    let raw = Connection::open(&path)?;
    let v: String = raw.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v, "2");
    Ok(())
}
```

Register in `Cargo.toml`:

```toml
[[test]]
name = "state_migrate"
required-features = ["test-helpers"]
```

Run:

```bash
cargo test --features test-helpers --test state_migrate
```

Expected: FAIL — `uu_tiktok::state::migrate` module doesn't exist.

- [ ] **Step 3: Create `src/state/migrate.rs`**

```rust
//! Pre-Epic-2 → Epic 2 schema migration (AD0021). Opens the DB raw,
//! bypassing Store::open's version check; runs ALTER TABLE + UPDATE meta
//! inside one transaction. Idempotent on already-migrated DBs.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

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
            // for any non-pre-Plan-A DB. AD0021 records this as the
            // pre-Plan-A migration target if ever needed.
            "1".to_string()
        }
    };

    if found == SCHEMA_VERSION {
        tracing::info!(version = SCHEMA_VERSION, "migrate: already at current version, no-op");
        return Ok(());
    }

    if found.parse::<u32>().ok().is_some_and(|n| n > SCHEMA_VERSION.parse::<u32>().unwrap_or(0)) {
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
        "UPDATE meta SET value = ?1 WHERE key = 'schema_version'",
        params![SCHEMA_VERSION],
    )
    .context("bump meta.schema_version to current")?;

    tx.commit().context("commit migrate transaction")?;

    tracing::info!(from = %found, to = SCHEMA_VERSION, "migrate: complete");
    Ok(())
}
```

- [ ] **Step 4: Wire `migrate` into `src/state/mod.rs`**

Add at the top of `src/state/mod.rs`:

```rust
pub mod migrate;
```

The `pub` is intentional: `tests/state_migrate.rs` (an integration test) consumes `migrate::run_migrate`. AD0005 (test-helpers feature) doesn't apply here because `run_migrate` is also called from `main.rs` (the binary). The function is genuinely public API.

- [ ] **Step 5: Wire the `Migrate` dispatch arm in `src/main.rs`**

Append to the `match cli.command` block, after the `Process` arm:

```rust
cli::Command::Migrate => {
    let path = &cfg.state_db;
    if !path.exists() {
        anyhow::bail!(
            "migrate: state.sqlite not found at {}. Run `uu-tiktok init` first.",
            path.display()
        );
    }
    state::migrate::run_migrate(path).context("running migrate")?;
    tracing::info!(path = %path.display(), "migrate complete");
}
```

`use rusqlite::OptionalExtension;` may be needed in `migrate.rs` for the `.optional()` call — confirm at fix-clippy time.

- [ ] **Step 6: Run the tests**

```bash
cargo test --features test-helpers --test state_migrate
```

Expected: PASS (both `migrate_v1_to_v2_adds_columns_and_bumps_version` and `migrate_is_idempotent_on_v2`). If T4 hasn't landed yet, the first test's "Round-trip with T2's Store::open" assertion may fail because `Store::open`'s schema-apply step uses the v2 schema; mark `#[ignore]` until T4 ships if implementing T3 first.

- [ ] **Step 7: Run the full suite**

```bash
cargo test --features test-helpers
```

Expected: all existing tests still pass.

- [ ] **Step 8: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean. (Clippy may flag `if let Some(v) = found.parse::<u32>().ok()` — accept whichever idiom passes.)

- [ ] **Step 9: Commit**

```bash
git add src/cli.rs src/main.rs src/state/mod.rs src/state/migrate.rs tests/state_migrate.rs Cargo.toml
git commit -m "$(cat <<'EOF'
feat(state): `migrate` CLI subcommand for v1→v2 schema upgrade (AD0021)

Adds:
- `Migrate` CLI variant
- `state::migrate::run_migrate(path)` function that opens the DB raw,
  reads meta.schema_version, and runs ALTER TABLE × 4 + UPDATE meta in
  one transaction
- main.rs dispatch arm
- Integration test (tests/state_migrate.rs, --features test-helpers)
  with a synthesized v1 fixture: confirms columns appear post-migrate,
  meta.schema_version bumps "1"→"2", and Store::open succeeds round-trip

Idempotent on already-v2 DBs (no-op + log). Hard-fails on newer-than-binary
versions (downgrade not supported). v0/pre-Plan-A DBs (no meta row) are
treated as v1 — same migration ladder applies.

Refs: AD0021

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_migrate` passes (both cases)
- [ ] Full suite passes
- [ ] `uu-tiktok migrate --state-db <v1.sqlite>` works manually on a synthesized v1 DB
- [ ] `uu-tiktok migrate` is a no-op on a v2 DB (idempotent)
- [ ] Clippy/fmt clean
