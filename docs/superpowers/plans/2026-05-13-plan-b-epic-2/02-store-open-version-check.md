# Task 2 — Store::open schema-version check + typed SchemaVersionMismatch error

**Goal:** `Store::open` reads `meta.schema_version` and compares against the `SCHEMA_VERSION` constant. On mismatch, return a typed `SchemaVersionMismatch { expected, found }` error containing an operator-readable instruction directing them to `uu-tiktok migrate`. Resolves FOLLOWUPS T7 (`Store::open` schema-version not read).

**ADRs touched:** AD0021 (schema-version policy: hard-fail).

**Files:**
- Modify: `src/state/mod.rs` (the `Store::open` body + add `StateError` type)
- Modify: `src/state/schema.rs` (no change to the SQL; constant unchanged this task — T4 bumps it)
- Modify: `tests/state_ingest.rs` or new `tests/state_schema_version.rs` (round-trip test for the version check)

**Pre-reqs:** T1 complete (AD0021 decided).

---

- [ ] **Step 1: Add `StateError` (typed error) to `src/state/mod.rs`**

Above the `Store` struct definition, add a `StateError` enum. Use `thiserror` (already a project dep). The "operator-readable instruction" message is part of the `Display` impl — when the operator sees the error stringified, they should know what to do.

```rust
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(
        "schema version mismatch: state.sqlite is at v{found}, this binary requires v{expected}. \
         Run `uu-tiktok migrate` to upgrade the database, then retry."
    )]
    SchemaVersionMismatch { expected: String, found: String },
}
```

(`thiserror` is already in `Cargo.toml` — confirm via `grep thiserror Cargo.toml`; if absent, add it. Project convention: confirmed present.)

- [ ] **Step 2: Write the failing tests first**

Add a new test file `tests/state_schema_version.rs` (gated on `test-helpers` per AD0005 — register it in `Cargo.toml` if not already):

```rust
//! Schema-version handling on Store::open. Per AD0021, mismatches are
//! a typed error that directs the operator to `uu-tiktok migrate`.

use anyhow::Result;
use rusqlite::Connection;
use tempfile::TempDir;
use uu_tiktok::state::{Store, StateError};

fn open_raw(path: &std::path::Path) -> Connection {
    Connection::open(path).unwrap()
}

#[test]
fn open_on_fresh_db_succeeds_and_records_version() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let store = Store::open(&path)?;
    let v = store.read_meta("schema_version")?.expect("recorded on first run");
    assert_eq!(v, uu_tiktok::state::SCHEMA_VERSION);
    Ok(())
}

#[test]
fn open_on_mismatched_version_returns_typed_error() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");

    // First open initializes at the current SCHEMA_VERSION.
    let _ = Store::open(&path)?;

    // Manually downgrade meta.schema_version to simulate a pre-Epic-2 DB.
    let raw = open_raw(&path);
    raw.execute(
        "UPDATE meta SET value = '0' WHERE key = 'schema_version'",
        [],
    )?;
    drop(raw);

    let err = Store::open(&path).expect_err("expected schema-version mismatch");
    // Downcast to StateError.
    let state_err = err
        .downcast_ref::<StateError>()
        .expect("error chain contains StateError");
    match state_err {
        StateError::SchemaVersionMismatch { expected, found } => {
            assert_eq!(expected, uu_tiktok::state::SCHEMA_VERSION);
            assert_eq!(found, "0");
        }
    }

    // Display string mentions `uu-tiktok migrate`.
    let display = format!("{}", state_err);
    assert!(
        display.contains("uu-tiktok migrate"),
        "operator-readable instruction missing from Display: {}",
        display
    );

    Ok(())
}
```

Register the test in `Cargo.toml` if not already present:

```toml
[[test]]
name = "state_schema_version"
required-features = ["test-helpers"]
```

Run:

```bash
cargo test --features test-helpers --test state_schema_version
```

Expected: FAIL — both tests fail because `StateError` isn't exported and `Store::open` doesn't perform the check yet.

- [ ] **Step 3: Re-export StateError from `src/state/mod.rs`**

Near the top of `src/state/mod.rs`, after `pub use schema::SCHEMA_VERSION;`, ensure `StateError` is public:

```rust
pub use schema::SCHEMA_VERSION;
// (StateError is defined below; the enum itself is already pub. No re-export needed.)
```

If the integration test can't see `StateError` because `src/state/mod.rs` isn't at the crate root, confirm `src/lib.rs` has `pub mod state;`. (If the project is a binary-only crate at this point, T2 needs to also create `src/lib.rs` and have `main.rs` consume from it; see AD0005 for the test-helpers feature pattern. Confirm shape before editing.)

```bash
ls src/lib.rs 2>/dev/null && echo "(lib.rs present)" || echo "(bin-only — confirm with controller)"
```

If `src/lib.rs` is absent, **STOP** and surface to the controller. Epic 1 explicitly defers bin/lib reassessment to Epic 5 (per AD0002 cleanup-on-consumption + AD0020 FOLLOWUPS). Integration tests reach into the crate via the test-helpers feature; confirm the existing test pattern (`tests/state_ingest.rs` etc.) before adding the new test file.

- [ ] **Step 4: Implement the version check in `Store::open`**

Replace the existing `Store::open` body in `src/state/mod.rs`. Current code:

```rust
pub fn open(path: &Path) -> Result<Self> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening SQLite database at {}", path.display()))?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .context("setting connection pragmas")?;

    conn.execute_batch(schema::SCHEMA_SQL)
        .context("applying schema")?;

    conn.execute(
        "INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION],
    )
    .context("recording schema_version")?;

    Ok(Self { conn })
}
```

New body:

```rust
pub fn open(path: &Path) -> Result<Self> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening SQLite database at {}", path.display()))?;

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .context("setting connection pragmas")?;

    // Apply schema idempotently. CREATE TABLE IF NOT EXISTS is safe on
    // both fresh DBs and existing DBs at any version — the column set
    // declared here is the CURRENT version; older DBs miss the new
    // columns and require `migrate` (per AD0021).
    conn.execute_batch(schema::SCHEMA_SQL)
        .context("applying schema")?;

    // Read schema_version. On a fresh DB the meta row doesn't exist yet;
    // record the current version. On an existing DB at the current version,
    // continue. On a mismatch, return SchemaVersionMismatch (AD0021).
    let found: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .context("reading schema_version from meta")?;

    match found {
        None => {
            // Fresh DB; record the current version.
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                params![SCHEMA_VERSION],
            )
            .context("recording schema_version on fresh DB")?;
        }
        Some(v) if v == SCHEMA_VERSION => {
            // Existing DB at current version; continue.
        }
        Some(v) => {
            return Err(StateError::SchemaVersionMismatch {
                expected: SCHEMA_VERSION.to_string(),
                found: v,
            }
            .into());
        }
    }

    Ok(Self { conn })
}
```

- [ ] **Step 5: Run the tests**

```bash
cargo test --features test-helpers --test state_schema_version
```

Expected: PASS — both `open_on_fresh_db_succeeds_and_records_version` and `open_on_mismatched_version_returns_typed_error` pass.

- [ ] **Step 6: Run the full test suite**

```bash
cargo test --features test-helpers
```

Expected: all existing tests still pass. Two specific concerns:

- `state_ingest.rs` and `state_claims.rs` open `Store` against fresh tempdirs; they hit the fresh-DB path and stay green.
- `main` opens `Store::open` for `init`, `ingest`, `process`. On a fresh DB the path is unchanged; on an existing v1 DB (Plan A users) `init` and `ingest` and `process` will all fail. **This is the intended AD0021 behavior** — the operator runs `uu-tiktok migrate` (T3) before re-running.

- [ ] **Step 7: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/state/mod.rs tests/state_schema_version.rs Cargo.toml
git commit -m "$(cat <<'EOF'
feat(state): Store::open reads schema_version; typed SchemaVersionMismatch error (AD0021)

Plan A's Store::open recorded schema_version on first run via
INSERT OR IGNORE INTO meta but never read it back. Epic 2 needs to
hard-fail on version mismatch so operators are explicitly directed
to the new `migrate` subcommand (per AD0021).

This task: Store::open now SELECTs meta.schema_version. On a fresh DB
the row doesn't exist yet; record the current version. On an existing
DB at the current version, continue. On mismatch, return a typed
StateError::SchemaVersionMismatch { expected, found } whose Display
impl directs the operator to `uu-tiktok migrate`.

Integration test (tests/state_schema_version.rs, --features test-helpers):
fresh-DB happy path + mismatch round-trip via raw connection.

Pre-Epic-2 v1 DBs (Plan A users) now fail every subcommand until
migrate runs. This is the intended AD0021 behavior — operationally
visible by design.

Refs: AD0021, FOLLOWUPS T7

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_schema_version` passes
- [ ] Full suite passes with `cargo test --features test-helpers`
- [ ] `StateError::SchemaVersionMismatch` Display includes the string "uu-tiktok migrate"
- [ ] Fresh-DB path still records `schema_version` (no regression for new operators)
- [ ] Clippy/fmt clean
- [ ] FOLLOWUPS T7 disposition decided: archived (resolved-by-Epic-2) after T3 lands the migrate subcommand
