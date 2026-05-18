# Task 4 — Schema columns: add 4 nullable columns + bump SCHEMA_VERSION

**Goal:** Add four nullable columns to the `videos` table (`last_retryable_kind`, `last_retryable_message`, `terminal_reason`, `terminal_message`) and bump `SCHEMA_VERSION` from `"1"` to `"2"`. The columns hold the string-typed failure classification per AD0022; Epic 3 will introduce typed-enum values that serialize into the same columns.

**ADRs touched:** AD0021 (schema-version policy).

**Files:**
- Modify: `src/state/schema.rs` (SCHEMA_SQL and SCHEMA_VERSION constants)
- Modify: `tests/state_ingest.rs` or new `tests/state_schema_v2.rs` (assert new columns exist; assert version bumped)

**Pre-reqs:** T1 + T2 + T3 complete (AD0021 decided; version-check live; migrate subcommand live).

---

- [ ] **Step 1: Write the failing test**

Create `tests/state_schema_v2.rs`:

```rust
//! Confirm the v2 schema: new nullable columns present on `videos`,
//! SCHEMA_VERSION constant is "2", fresh DB records "2" in meta.

use anyhow::Result;
use rusqlite::Connection;
use tempfile::TempDir;
use uu_tiktok::state::{Store, SCHEMA_VERSION};

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
fn schema_version_constant_is_v2() {
    assert_eq!(SCHEMA_VERSION, "2");
}

#[test]
fn fresh_db_has_new_nullable_columns() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let _ = Store::open(&path)?;

    let raw = Connection::open(&path)?;
    let cols = columns_in(&raw, "videos");
    for expected in [
        "last_retryable_kind",
        "last_retryable_message",
        "terminal_reason",
        "terminal_message",
    ] {
        assert!(
            cols.contains(&expected.to_string()),
            "expected column `{}` in videos: have {:?}",
            expected,
            cols
        );
    }

    let v: String = raw.query_row(
        "SELECT value FROM meta WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(v, "2");
    Ok(())
}

#[test]
fn new_columns_default_to_null() -> Result<()> {
    let tmp = TempDir::new()?;
    let path = tmp.path().join("state.sqlite");
    let mut store = Store::open(&path)?;
    store.upsert_video("vid_a", "https://example/a", false)?;

    let raw = Connection::open(&path)?;
    let (rk, rm, tr, tm): (Option<String>, Option<String>, Option<String>, Option<String>) = raw
        .query_row(
            "SELECT last_retryable_kind, last_retryable_message, terminal_reason, terminal_message
             FROM videos WHERE video_id = ?1",
            ["vid_a"],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
    assert_eq!(rk, None);
    assert_eq!(rm, None);
    assert_eq!(tr, None);
    assert_eq!(tm, None);
    Ok(())
}
```

Register in `Cargo.toml`:

```toml
[[test]]
name = "state_schema_v2"
required-features = ["test-helpers"]
```

Run:

```bash
cargo test --features test-helpers --test state_schema_v2
```

Expected: FAIL (`schema_version_constant_is_v2` fails because constant is still `"1"`; column-existence asserts fail).

- [ ] **Step 2: Update `src/state/schema.rs`**

Change `SCHEMA_VERSION`:

```rust
pub const SCHEMA_VERSION: &str = "2";
```

Add the four columns to the `videos` CREATE TABLE block. After `transcript_source   TEXT,`:

```rust
pub const SCHEMA_SQL: &str = r#"
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
    -- Plan B Epic 2: failure classification columns (AD0021, AD0022).
    -- String-typed today per AD0022; Epic 3's typed enums serialize into
    -- the same columns via tag()/message() projections.
    last_retryable_kind     TEXT,
    last_retryable_message  TEXT,
    terminal_reason         TEXT,
    terminal_message        TEXT,
    first_seen_at       INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_videos_pending
    ON videos (status, first_seen_at, video_id)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS watch_history (
    respondent_id  TEXT NOT NULL,
    video_id       TEXT NOT NULL,
    watched_at     INTEGER NOT NULL,
    in_window      INTEGER NOT NULL,
    PRIMARY KEY (respondent_id, video_id, watched_at),
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
CREATE INDEX IF NOT EXISTS idx_watch_history_video ON watch_history (video_id);

CREATE TABLE IF NOT EXISTS video_events (
    id           INTEGER PRIMARY KEY,
    video_id     TEXT NOT NULL,
    at           INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    worker_id    TEXT,
    detail_json  TEXT,
    FOREIGN KEY (video_id) REFERENCES videos(video_id)
);
CREATE INDEX IF NOT EXISTS idx_video_events_video ON video_events (video_id, at);

CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
"#;
```

Position note: the four new columns go BEFORE `first_seen_at`/`updated_at` (which are the existing trailing columns) so the column order matches what `ALTER TABLE ... ADD COLUMN` produced in T3. SQLite appends `ALTER TABLE ADD COLUMN` to the end, but a fresh-DB `CREATE TABLE` from this SQL also ends up with them in this position. Cross-check column order between fresh DB and migrated DB:

```bash
cargo test --features test-helpers --test state_migrate -- migrate_v1_to_v2_adds_columns_and_bumps_version
```

Both DBs end up with the columns present, even if logical ordering differs slightly between fresh-CREATE and ALTER paths. The `columns_in` assertion in the tests is order-insensitive (checks membership via `.contains`).

- [ ] **Step 3: Run the tests**

```bash
cargo test --features test-helpers --test state_schema_v2
```

Expected: PASS (all three).

- [ ] **Step 4: Run the full suite**

```bash
cargo test --features test-helpers
```

Expected: all existing tests still pass. T3's migrate test now exercises the same SCHEMA_VERSION constant; both pass.

- [ ] **Step 5: cargo fmt + clippy**

```bash
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/state/schema.rs tests/state_schema_v2.rs Cargo.toml
git commit -m "$(cat <<'EOF'
feat(state): bump schema v1 → v2; add last_retryable_kind/message + terminal_reason/message columns

Adds four nullable TEXT columns to `videos`:
- last_retryable_kind / last_retryable_message: written by
  mark_retryable_failure (T6) when a transient error classifies a row
  as failed_retryable. Epic 3 introduces typed RetryableKind that
  serializes into the same column via tag()/message() projections.
- terminal_reason / terminal_message: written by mark_terminal_failure
  (T7, surface-only in Epic 2; Epic 3's classifier dispatcher is the
  first caller).

Bumps SCHEMA_VERSION constant "1" → "2"; fresh DB now records "2" in
meta.schema_version. T3's `migrate` subcommand brings pre-Epic-2 DBs
up to this version.

Integration test (tests/state_schema_v2.rs, --features test-helpers):
column-existence on fresh DB; default-NULL on upsert_video; constant
value matches.

Refs: AD0021, AD0022

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-check

- [ ] `cargo test --features test-helpers --test state_schema_v2` passes
- [ ] `cargo test --features test-helpers --test state_migrate` still passes (v1→v2 round-trip)
- [ ] Full suite green
- [ ] Clippy/fmt clean
- [ ] The new columns are nullable; existing upsert paths don't need to provide them
